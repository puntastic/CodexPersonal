"""Windows-only, task-authorized app-server startup rehearsal on a primary DB copy.

The source opens read-only. A new output directory keeps an untouched backup and
an isolated trial home. Neither package is selected for Desktop by this probe.
The schema assertions target 0.157.1's state migrations55–57. Optional companion
copies preserve history/memory recovery without claiming that startup exercised
every lazy store or a model turn. No test resumes a real source rollout.
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
        db.execute("PRAGMA query_only=ON")
        tables = {
            row[0]
            for row in db.execute("SELECT name FROM sqlite_master WHERE type='table'")
        }
        result = {
            "quick_check": db.execute("PRAGMA quick_check").fetchone()[0],
            "applied_migrations": [
                row[0]
                for row in db.execute(
                    "SELECT version FROM _sqlx_migrations WHERE success=1 ORDER BY version"
                )
            ],
        }
        if "threads" in tables:
            columns = {row[1] for row in db.execute("PRAGMA table_info(threads)")}
            result["creator_columns"] = {
                "creator_user_id",
                "creator_account_id",
            } <= columns
            source = json.dumps(
                {"subagent": {"other": "guardian"}}, separators=(",", ":")
            )
            result["guardian_projection"] = db.execute(
                "SELECT count(*), coalesce(sum(length(title)),0), "
                "coalesce(sum(length(first_user_message)),0) FROM threads WHERE source=?",
                (source,),
            ).fetchone()
            result["attachments_table"] = "thread_attachments" in tables
            table = (
                "thread_attachments"
                if result["attachments_table"]
                else "thread_artifacts"
            )
            result["attachment_rows"] = (
                db.execute(f"SELECT count(*) FROM {table}").fetchone()[0]
                if table in tables
                else None
            )
        return result


def startup(executable, home, *, check_attachments=False):
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
        [
            str(executable),
            "-c",
            'cli_auth_credentials_store="file"',
            "-c",
            f"sqlite_home={json.dumps(str(home))}",
            "app-server",
        ],
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

        def request(identifier, method, params):
            process.stdin.write(
                json.dumps({"id": identifier, "method": method, "params": params})
                + "\n"
            )
            process.stdin.flush()
            deadline = time.monotonic() + 45
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise RuntimeError(
                        f"app-server exited during {method}: {list(errors)}"
                    )
                try:
                    message = messages.get(timeout=0.2)
                except queue.Empty:
                    continue
                if message.get("id") == identifier:
                    if "error" in message:
                        raise RuntimeError(f"{method} failed: {message['error']}")
                    return message["result"]
            raise TimeoutError(f"{method} timed out: {list(errors)}")

        response = request(
            1,
            "initialize",
            {
                "clientInfo": {
                    "name": "codex-personal-upgrade-probe",
                    "version": "0.2.0",
                },
                "capabilities": {"experimentalApi": True},
            },
        )
        if Path(response["codexHome"]).resolve() != home:
            raise RuntimeError("probe did not use its isolated CODEX_HOME")
        process.stdin.write(json.dumps({"method": "initialized"}) + "\n")
        process.stdin.flush()
        # DB-only listing explicitly avoids scanning/repairing original rollouts
        # whose paths survive in the copied state. Never resume those threads.
        listed = request(2, "thread/list", {"limit": 1, "useStateDbOnly": True})
        rows = listed["data"]
        if not isinstance(rows, list) or len(rows) > 1:
            raise RuntimeError("unexpected bounded thread-list result")
        attachment_count = None
        if check_attachments and rows:
            attachments = request(
                3, "thread/attachment/list", {"threadId": rows[0]["id"], "limit": 1}
            )
            attachment_count = len(attachments["data"])
        process.stdin.close()
        process.wait(timeout=45)
        if process.returncode != 0:
            raise RuntimeError(f"shutdown failed: {list(errors)}")
        return {
            "user_agent": response["userAgent"],
            "exit_code": process.returncode,
            "db_only_threads_returned": len(rows),
            "attachment_rows_returned": attachment_count,
            "scope": "initialize, DB-only thread metadata, and candidate attachment listing; no turn or real rollout resumed",
        }
    finally:
        if process.poll() is None:
            process.kill()  # Only the probe's own child, never the running Desktop.
            process.wait(timeout=10)
        for thread in threads:
            thread.join(timeout=1)


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--candidate", type=Path, required=True)
parser.add_argument("--previous", type=Path, required=True)
parser.add_argument("--source-db", type=Path, required=True)
parser.add_argument("--source-companion-db", type=Path, action="append", default=[])
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
companions = {}
for path in args.source_companion_db:
    companion = path.resolve(strict=True)
    if companion.name == source.name or companion.name in companions:
        raise ValueError("duplicate companion database name")
    preserved = output / companion.name
    backup(companion, preserved)
    backup(preserved, home / companion.name)
    with preserved.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    companions[companion.name] = {
        "backup": str(preserved),
        "backup_sha256": digest,
        "before": schema(preserved),
    }
with saved.open("rb") as stream:
    saved_hash = hashlib.file_digest(stream, "sha256").hexdigest()
report = {
    "backup": str(saved),
    "backup_sha256": saved_hash,
    "before": schema(saved),
    "companions": companions,
}
report["candidate"] = startup(candidate, home, check_attachments=True)
report["after_candidate"] = schema(home / source.name)
for name, detail in companions.items():
    detail["after_candidate"] = schema(home / name)
report["previous_after_migration"] = startup(previous, home)
report["after_previous"] = schema(home / source.name)
for name, detail in companions.items():
    detail["after_previous"] = schema(home / name)
    assert detail["after_candidate"]["quick_check"] == "ok"
    assert detail["after_previous"]["quick_check"] == "ok"
assert report["after_candidate"]["quick_check"] == "ok"
assert {55, 56, 57} <= set(report["after_candidate"]["applied_migrations"])
assert report["after_candidate"]["creator_columns"]
assert report["after_candidate"]["attachments_table"]
assert (
    report["after_candidate"]["attachment_rows"] == report["before"]["attachment_rows"]
)
assert report["after_candidate"]["guardian_projection"][2] == 0
assert report["after_previous"]["quick_check"] == "ok"
report["candidate_after_previous"] = startup(candidate, home, check_attachments=True)
(output / "receipt.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
print(json.dumps(report))
