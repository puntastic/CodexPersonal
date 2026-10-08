"""Windows-only, task-authorized app-server startup rehearsal on a primary DB copy.

The source opens read-only. A new output directory keeps an untouched backup and
an isolated trial home. Neither package is selected for Desktop by this probe.
The schema assertions target an upgrade from migrations55–57 through58. Optional
companion copies preserve history/memory recovery without claiming that startup
exercised every lazy store or a model turn. No test resumes a real source rollout.
Green DB startup does not prove that an older reader can replay new V2
retained_source metadata: dropping an unknown field before hash validation is
a separate source-derived conditional compatibility risk, not exercised here.
"""

import argparse
from collections import deque
from contextlib import closing
import hashlib
import json
import os
from pathlib import Path
import queue
import sqlite3
import subprocess
import threading
import time


ARCHIVE_SORT_KEYS = ("created_at", "updated_at", "recency_at")


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def file_hash(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def backup_fingerprint(path):
    return {
        file.name: file_hash(file)
        for suffix in ("", "-wal", "-shm")
        if (file := path.with_name(path.name + suffix)).exists()
    }


def quote_identifier(value):
    return '"' + value.replace('"', '""') + '"'


def json_value(value):
    return {"blob_hex": value.hex()} if isinstance(value, bytes) else value


def table_evidence(db, table):
    """Hash logical rows, not SQLite pages; disclose no row contents."""
    info = db.execute(f"PRAGMA table_info({quote_identifier(table)})").fetchall()
    columns = [row[1] for row in info]
    primary_key = [row[1] for row in sorted(info, key=lambda row: row[5]) if row[5]]
    ordering = ", ".join(quote_identifier(name) for name in primary_key or columns)
    digest = hashlib.sha256(json.dumps(columns).encode("utf-8"))
    count = 0
    for row in db.execute(
        f"SELECT * FROM {quote_identifier(table)} ORDER BY {ordering}"
    ):
        digest.update(json.dumps([json_value(value) for value in row]).encode("utf-8"))
        digest.update(b"\n")
        count += 1
    return {"columns": columns, "rows": count, "sha256": digest.hexdigest()}


def backup(source, target):
    deadline = time.monotonic() + 30

    def progress(_status, _remaining, _total):
        if time.monotonic() > deadline:
            raise TimeoutError("online SQLite backup exceeded 30 seconds")

    with closing(sqlite3.connect(source.as_uri() + "?mode=ro", uri=True)) as src:
        with closing(sqlite3.connect(target)) as dst:
            src.backup(dst, pages=512, progress=progress, sleep=0.05)


def schema(path):
    with closing(sqlite3.connect(path.as_uri() + "?mode=ro", uri=True)) as db:
        db.execute("PRAGMA query_only=ON")
        tables = {
            row[0]
            for row in db.execute("SELECT name FROM sqlite_master WHERE type='table'")
        }
        result = {
            "quick_check": "\n".join(
                row[0] for row in db.execute("PRAGMA quick_check")
            ),
            "file_identity": [path.stat().st_dev, path.stat().st_ino],
        }
        ledger = db.execute("SELECT * FROM _sqlx_migrations ORDER BY version")
        fields = [column[0] for column in ledger.description]
        result["migration_ledger"] = [
            dict(zip(fields, (json_value(value) for value in row))) for row in ledger
        ]
        result["applied_migrations"] = [
            row["version"] for row in result["migration_ledger"] if row["success"] == 1
        ]
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
            result["row_evidence"] = {
                name: table_evidence(db, name)
                for name in ("threads", table)
                if name in tables
            }
            result["archive_indexes"] = {}
            indexes = {row[1]: row for row in db.execute("PRAGMA index_list(threads)")}
            for key in ARCHIVE_SORT_KEYS:
                name = f"idx_threads_archive_{key}_ms"
                if name not in indexes:
                    continue
                result["archive_indexes"][name] = {
                    "sql": db.execute(
                        "SELECT sql FROM sqlite_master WHERE type='index' AND name=?",
                        (name,),
                    ).fetchone()[0],
                    "partial": indexes[name][4],
                    "unique": indexes[name][2],
                    "keys": [
                        {"column": row[2], "descending": row[3], "collation": row[4]}
                        for row in db.execute(
                            f"PRAGMA index_xinfo({quote_identifier(name)})"
                        )
                        if row[5]
                    ],
                }
        return result


def validate_readback(before, after, *, primary=False):
    require(after["quick_check"] == "ok", "copied database quick_check failed")
    require(
        before["file_identity"] == after["file_identity"],
        "copied database was replaced",
    )
    old = {row["version"]: row for row in before["migration_ledger"]}
    new = {row["version"]: row for row in after["migration_ledger"]}
    require(
        all(new.get(version) == row for version, row in old.items()),
        "existing migration ledger rows changed or disappeared",
    )
    require(
        all(row["success"] == 1 for row in new.values()),
        "unsuccessful migration ledger row",
    )
    if not primary:
        return
    require(set(new) - set(old) == {58} - set(old), "unexpected added state migrations")
    require(
        {55, 56, 57, 58} <= set(after["applied_migrations"]),
        "required state migrations missing",
    )
    require(
        after["creator_columns"] and after["attachments_table"],
        "required state schema missing",
    )
    require(
        before["row_evidence"] == after["row_evidence"],
        "thread or attachment evidence changed",
    )
    for key in ARCHIVE_SORT_KEYS:
        name = f"idx_threads_archive_{key}_ms"
        actual = after["archive_indexes"].get(name)
        require(actual is not None, f"archive index missing: {name}")
        expected_sql = f"CREATE INDEX {name} ON threads(archived, {key}_ms DESC, id DESC) WHERE archived = 1"
        normalized = lambda sql: "".join(sql.split()).rstrip(";").lower()
        require(
            normalized(actual["sql"]) == normalized(expected_sql),
            f"unexpected SQL for {name}",
        )
        require(
            actual["partial"] == 1 and actual["unique"] == 0,
            f"unexpected index kind: {name}",
        )
        require(
            actual["keys"]
            == [
                {"column": "archived", "descending": 0, "collation": "BINARY"},
                {"column": f"{key}_ms", "descending": 1, "collation": "BINARY"},
                {"column": "id", "descending": 1, "collation": "BINARY"},
            ],
            f"unexpected index column order or direction: {name}",
        )


def recovery_backups(home):
    # These are the two recovery roots used by state/runtime/recovery.rs. Only
    # inspect this new trial's roots, never the original CODEX_HOME.
    roots = (home / "db-backups", home.with_name(home.name + ".db-backups"))
    return [str(path.relative_to(home.parent)) for path in roots if path.exists()]


def list_metadata(request, *, check_attachments):
    # DB-only listing avoids scanning/repairing original rollout paths retained
    # in the copied state. Never resume, read, or turn/start those threads.
    listed = request(2, "thread/list", {"limit": 1, "useStateDbOnly": True})
    rows = listed["data"]
    require(
        isinstance(rows, list) and len(rows) <= 1,
        "unexpected bounded thread-list result",
    )
    attachment_count = None
    archived_counts = {}
    if check_attachments and rows:
        attachments = request(
            3, "thread/attachment/list", {"threadId": rows[0]["id"], "limit": 1}
        )["data"]
        require(
            isinstance(attachments, list) and len(attachments) <= 1,
            "unexpected bounded attachment-list result",
        )
        attachment_count = len(attachments)
    for identifier, key in enumerate(ARCHIVE_SORT_KEYS, start=4):
        archived = request(
            identifier,
            "thread/list",
            {
                "limit": 2,
                "useStateDbOnly": True,
                "archived": True,
                "modelProviders": [],
                "sortKey": key,
                "sortDirection": "desc",
            },
        )["data"]
        require(
            isinstance(archived, list) and len(archived) <= 2,
            f"unexpected bounded archive-list result for {key}",
        )
        archived_counts[key] = len(archived)
    return {
        "db_only_threads_returned": len(rows),
        "attachment_rows_returned": attachment_count,
        "db_only_archived_threads_returned": archived_counts,
    }


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
                    "version": "0.3.0",
                },
                "capabilities": {"experimentalApi": True},
            },
        )
        if Path(response["codexHome"]).resolve() != home:
            raise RuntimeError("probe did not use its isolated CODEX_HOME")
        process.stdin.write(json.dumps({"method": "initialized"}) + "\n")
        process.stdin.flush()
        metadata = list_metadata(request, check_attachments=check_attachments)
        process.stdin.close()
        process.wait(timeout=45)
        if process.returncode != 0:
            raise RuntimeError(f"shutdown failed: {list(errors)}")
        return {
            "user_agent": response["userAgent"],
            "exit_code": process.returncode,
            **metadata,
            "scope": "initialize, DB-only thread metadata/archived sorts, and candidate attachment listing; no turn or real rollout resumed",
        }
    finally:
        if process.poll() is None:
            process.kill()  # Only the probe's own child, never the running Desktop.
            process.wait(timeout=10)
        for thread in threads:
            thread.join(timeout=1)


def run_probe(args):
    source = args.source_db.resolve(strict=True)
    candidate = args.candidate.resolve(strict=True)
    previous = args.previous.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    home = output / "isolated-home"
    home.mkdir()
    databases = {}
    for path in [source, *args.source_companion_db]:
        path = path.resolve(strict=True)
        if path.name in databases:
            raise ValueError("duplicate database name")
        saved = output / path.name
        backup(path, saved)
        backup(saved, home / path.name)
        before = schema(home / path.name)
        require(before["quick_check"] == "ok", "input database copy failed quick_check")
        databases[path.name] = {
            "backup": str(saved),
            "backup_sha256": file_hash(saved),
            "backup_files_sha256": backup_fingerprint(saved),
            "before": before,
        }
    report = databases.pop(source.name)
    report["companions"] = databases
    report["scope_limit"] = (
        "DB startup/schema/metadata only; no model turn or real rollout resumed. "
        "This does not establish old-reader replay of new V2 retained_source metadata: "
        "unknown-field dropping before hash validation is a separate source-derived conditional risk. "
        "Companion checks cover integrity, file identity, and existing migration rows; "
        "they do not require logs or other companion rows to remain unchanged."
    )
    require(
        {55, 56, 57} <= set(report["before"]["applied_migrations"]),
        "probe requires the previously qualified migrations55–57 baseline",
    )
    require(not recovery_backups(home), "recovery backup exists before startup")
    try:
        for startup_key, readback_key, executable, check_attachments in (
            ("candidate", "after_candidate", candidate, True),
            ("previous_after_migration", "after_previous", previous, False),
            (
                "candidate_after_previous",
                "after_candidate_after_previous",
                candidate,
                True,
            ),
        ):
            startup_error = None
            try:
                report[startup_key] = startup(
                    executable, home, check_attachments=check_attachments
                )
            except Exception as error:
                startup_error = error
                report[startup_key] = {"failure": str(error)}
            found = recovery_backups(home)
            report[startup_key]["recovery_backups"] = found
            # A failed process still earns a readback attempt for each copy.
            for name, detail in [(source.name, report), *databases.items()]:
                try:
                    detail[readback_key] = schema(home / name)
                except Exception as error:
                    detail[readback_key] = {"failure": str(error)}
                    if startup_error is None:
                        startup_error = error
            if startup_error is not None:
                raise startup_error
            require(not found, "startup created a recovery backup/rebuilt database")
            validate_readback(report["before"], report[readback_key], primary=True)
            for detail in databases.values():
                validate_readback(detail["before"], detail[readback_key])
            # After the first migration, the full ledger (including migration58)
            # must survive both subsequent startups without additions or edits.
            if readback_key != "after_candidate":
                for detail in [report, *databases.values()]:
                    require(
                        detail[readback_key]["migration_ledger"]
                        == detail["after_candidate"]["migration_ledger"],
                        "migration ledger changed after initial candidate startup",
                    )
        report["passed"] = True
    except Exception as error:
        report["passed"] = False
        report["failure"] = str(error)
        raise
    finally:
        # Check the untouched backups even on a failed rehearsal, and write the
        # final readbacks before returning a successful receipt.
        for detail in [report, *databases.values()]:
            saved = Path(detail["backup"])
            detail["backup_files_sha256_final"] = backup_fingerprint(saved)
            detail["backup_sha256_final"] = detail["backup_files_sha256_final"].get(
                saved.name
            )
        untouched = all(
            detail["backup_files_sha256_final"] == detail["backup_files_sha256"]
            for detail in [report, *databases.values()]
        )
        report["untouched_backups_verified"] = untouched
        if not untouched:
            report["passed"] = False
            report["failure"] = "untouched backup changed"
        (output / "receipt.json").write_text(
            json.dumps(report, indent=2), encoding="utf-8"
        )
        require(untouched, "untouched backup changed")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--previous", type=Path, required=True)
    parser.add_argument("--source-db", type=Path, required=True)
    parser.add_argument("--source-companion-db", type=Path, action="append", default=[])
    parser.add_argument("--output", type=Path, required=True)
    print(json.dumps(run_probe(parser.parse_args())))


if __name__ == "__main__":
    main()
