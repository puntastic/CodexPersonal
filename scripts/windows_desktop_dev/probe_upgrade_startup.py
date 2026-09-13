"""Windows-only, task-authorized app-server startup rehearsal on a primary DB copy.

The source opens read-only. A new output directory keeps an untouched backup and
an isolated trial home. Neither package is selected for Desktop by this probe.
"""

import argparse
from collections import deque
import hashlib
import json
import os
from pathlib import Path
import queue
import sqlite3
import subprocess
import threading
import time


def backup(source, target):
    deadline = time.monotonic() + 30

    def progress(_status, _remaining, _total):
        if time.monotonic() > deadline:
            raise TimeoutError("online SQLite backup exceeded 30 seconds")

    with sqlite3.connect(source.as_uri() + "?mode=ro", uri=True) as src:
        with sqlite3.connect(target) as dst:
            src.backup(dst, pages=512, progress=progress, sleep=0.05)


def schema(path):
    with sqlite3.connect(path.as_uri() + "?mode=ro", uri=True) as db:
        return {
            "quick_check": db.execute("PRAGMA quick_check").fetchone()[0],
            "migration_54": db.execute(
                "SELECT success FROM _sqlx_migrations WHERE version=54"
            ).fetchone(),
            "daybreak_column": any(
                row[1] == "daybreak_enabled"
                for row in db.execute("PRAGMA table_info(threads)")
            ),
        }


def startup(executable, home):
    env = dict(os.environ)
    env.update(
        CODEX_HOME=str(home),
        CODEX_SQLITE_HOME=str(home),
        CODEX_INTERNAL_APP_SERVER_REMOTE_CONTROL_DISABLED="1",
        OTEL_SDK_DISABLED="true",
    )
    for key in ("OPENAI_API_KEY", "CODEX_API_KEY", "CODEX_CLI_PATH"):
        env.pop(key, None)
    messages = queue.Queue()
    errors = deque(maxlen=12)
    process = subprocess.Popen(
        [str(executable), "-c", 'cli_auth_credentials_store="file"', "app-server"],
        cwd=home,
        env=env,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
        creationflags=subprocess.CREATE_NO_WINDOW,
    )

    def read_stdout():
        for line in process.stdout:
            try:
                messages.put(json.loads(line))
            except json.JSONDecodeError:
                messages.put({"invalid_json": line[:1000]})

    def read_stderr():
        for line in process.stderr:
            errors.append(line[:1000])

    threads = [
        threading.Thread(target=read_stdout, daemon=True),
        threading.Thread(target=read_stderr, daemon=True),
    ]
    for thread in threads:
        thread.start()
    try:
        process.stdin.write(
            json.dumps(
                {
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "clientInfo": {
                            "name": "codex-personal-upgrade-probe",
                            "version": "0.1.0",
                        }
                    },
                }
            )
            + "\n"
        )
        process.stdin.flush()
        deadline = time.monotonic() + 45
        response = None
        while time.monotonic() < deadline:
            if process.poll() is not None:
                raise RuntimeError(
                    f"app-server exited before initialize: {list(errors)}"
                )
            try:
                message = messages.get(timeout=0.2)
            except queue.Empty:
                continue
            if message.get("id") == 1:
                if "error" in message:
                    raise RuntimeError(f"initialize failed: {message['error']}")
                response = message["result"]
                break
        if response is None:
            raise TimeoutError(f"initialize timed out: {list(errors)}")
        if Path(response["codexHome"]).resolve() != home:
            raise RuntimeError("probe did not use its isolated CODEX_HOME")
        process.stdin.close()
        process.wait(timeout=45)
        if process.returncode != 0:
            raise RuntimeError(f"shutdown failed: {list(errors)}")
        return {"user_agent": response["userAgent"], "exit_code": process.returncode}
    finally:
        if process.poll() is None:
            process.kill()  # Only the probe's own child, never the running Desktop.
            process.wait(timeout=10)
        for thread in threads:
            thread.join(timeout=1)


parser = argparse.ArgumentParser()
parser.add_argument("--candidate", type=Path, required=True)
parser.add_argument("--previous", type=Path, required=True)
parser.add_argument("--source-db", type=Path, required=True)
parser.add_argument("--output", type=Path, required=True)
args = parser.parse_args()
source = args.source_db.resolve(strict=True)
candidate = args.candidate.resolve(strict=True)
previous = args.previous.resolve(strict=True)
output = args.output.resolve()
output.mkdir(parents=True, exist_ok=False)
saved = output / source.name
home = output / "isolated-home"
home.mkdir()
backup(source, saved)
backup(saved, home / source.name)
with saved.open("rb") as stream:
    saved_hash = hashlib.file_digest(stream, "sha256").hexdigest()
report = {"backup": str(saved), "backup_sha256": saved_hash, "before": schema(saved)}
report["candidate"] = startup(candidate, home)
report["after_candidate"] = schema(home / source.name)
report["previous_after_migration"] = startup(previous, home)
report["after_previous"] = schema(home / source.name)
assert report["after_candidate"]["quick_check"] == "ok"
assert report["after_candidate"]["migration_54"] == (1,)
assert report["after_candidate"]["daybreak_column"]
assert report["after_previous"]["quick_check"] == "ok"
(output / "receipt.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
print(json.dumps(report))
