"""Capture one native CI command, recording root exit before any log/report work.

Use a fresh --output-dir and put literal command arguments after --. Python's
stdlib is the only dependency. Logs are raw files, not redirected pipes: waiting
for the root does not wait for descendant EOF. Descendant exit and final log
completeness are deliberately NOT established. The caller owns JUnit validation,
artifact retention, cancellation and the overall job timeout.

Require native-result.json AND capture-phase.json status=complete; native exit 0
alone is not capture/report success. Console summaries have a byte bound, not a
wall-clock guarantee against a blocked console sink. No live log following occurs.
"""

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys


SUMMARY_BYTES_PER_STREAM = 4096


def utc_now():
    return datetime.now(timezone.utc).isoformat()


def save_json(directory, name, value):
    temporary = directory / (name + ".tmp")
    temporary.write_text(
        json.dumps(value, indent=2, ensure_ascii=True) + "\n", encoding="utf-8"
    )
    temporary.replace(directory / name)


def console_json(value):
    # The legacy Actions parser recognizes ##[ anywhere, even after a prefix.
    # Escaping hashes keeps this valid JSON without changing stored raw evidence.
    return json.dumps(value, ensure_ascii=True).replace("#", r"\u0023")


def emit_summary(directory):
    for name in ("stdout.log", "stderr.log"):
        with (directory / name).open("rb") as stream:
            size = os.fstat(stream.fileno()).st_size
            stream.seek(max(0, size - SUMMARY_BYTES_PER_STREAM))
            tail = stream.read(min(size, SUMMARY_BYTES_PER_STREAM))
        # Escape controls/newlines and legacy Actions commands, then prefix the line.
        print(
            f"capture {name} snapshot_bytes={size} tail="
            + console_json(tail.decode("utf-8", errors="replace"))
        )
    sys.stdout.flush()


def capture(argv, directory, cwd):
    directory = Path(directory).resolve()
    directory.mkdir()  # Never truncate or mistake evidence from a previous run.
    phase = {
        "schema_version": 1,
        "phase": "native-start-and-wait",
        "status": "running",
    }
    result = {
        "schema_version": 1,
        "argv": argv,
        "cwd": str(Path(cwd).resolve()),
        "started_utc": utc_now(),
        "status": "not-started",
        "root_pid": None,
        "root_exit_code": None,
        "root_exit_observed_utc": None,
        "stdout": "stdout.log",
        "stderr": "stderr.log",
        "descendant_completion": "not-observed",
        "log_completeness": "not-established",
    }
    exit_code = None
    try:
        save_json(directory, "capture-phase.json", phase)
        with (
            (directory / "stdout.log").open("xb") as stdout,
            (directory / "stderr.log").open("xb") as stderr,
        ):
            try:
                executable = shutil.which(argv[0]) or argv[0]
                if os.name == "nt" and Path(executable).suffix.lower() in (
                    ".bat",
                    ".cmd",
                ):
                    raise ValueError(
                        "Batch files are not literal native commands; pass a native executable"
                    )
                process = subprocess.Popen(
                    [executable, *argv[1:]],
                    cwd=cwd,
                    stdin=subprocess.DEVNULL,
                    stdout=stdout,
                    stderr=stderr,
                    shell=False,
                )
            except (OSError, ValueError) as error:
                result.update(status="startup-failed", error=str(error)[:2000])
                save_json(directory, "native-result.json", result)
                raise
            result["root_pid"] = process.pid
            exit_code = process.wait()
            result.update(
                status="root-exited",
                root_exit_code=exit_code,
                root_exit_observed_utc=utc_now(),
            )
            # This is the first potentially failing operation after the root wait.
            # Do not overwrite this immutable result during later reporting.
            save_json(directory, "native-result.json", result)
        phase.update(phase="console-summary", status="running")
        save_json(directory, "capture-phase.json", phase)
        print(
            f"capture root-exited exit_code={exit_code}; descendant/log completeness unknown"
        )
        emit_summary(directory)
        phase.update(phase="capture-complete", status="complete")
        save_json(directory, "capture-phase.json", phase)
    except Exception as error:
        phase.update(status="failed", error=str(error)[:2000])
        try:
            save_json(directory, "capture-phase.json", phase)
        except OSError:
            pass  # A missing/stale phase is failure too; do not invent a receipt.
        try:
            print(
                "capture failed: " + console_json(str(error)[:500]),
                file=sys.stderr,
                flush=True,
            )
        except (OSError, ValueError):
            pass
        # Never launder a native failure. A successful root plus wrapper failure
        # must also fail, with the original zero still preserved in its receipt.
        return exit_code if exit_code not in (None, 0) else 1
    return exit_code


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", required=True, type=Path)
    parser.add_argument("--cwd", default=Path.cwd(), type=Path)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    argv = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not argv:
        parser.error(
            "a native executable and its literal arguments are required after --"
        )
    try:
        return capture(argv, args.output_dir, args.cwd)
    except OSError as error:
        print(
            "capture preflight failed: " + console_json(str(error)[:500]),
            file=sys.stderr,
            flush=True,
        )
        return 1


if __name__ == "__main__":
    code = main()
    # All owned writes/flushes have already completed. Bypass interpreter cleanup
    # so a late console flush cannot replace a native status with Python's 120.
    # Windows _exit takes signed int, while the native receipt retains the DWORD.
    if os.name == "nt" and code >= 2**31:
        code -= 2**32
    os._exit(code)
