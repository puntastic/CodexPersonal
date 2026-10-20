"""Harmless wrapper fixtures only: no Rust, sandbox, accounts, config or kills."""

from contextlib import redirect_stdout, redirect_stderr
import ctypes
from ctypes import wintypes
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import uuid

import capture_native_command as capture


CHILD = """
import os, pathlib, sys, time
print('descendant-start', flush=True)
time.sleep(3)
print('descendant-end', flush=True)
print('descendant-stderr-end', file=sys.stderr, flush=True)
pathlib.Path(sys.argv[1]).write_text('done')
os._exit(0)
"""
PARENT = """
import os, pathlib, subprocess, sys
child = subprocess.Popen([sys.executable, '-c', sys.argv[2], sys.argv[1] + '/done'])
pathlib.Path(sys.argv[1], 'child-pid').write_text(str(child.pid))
print('root-stdout', flush=True)
print('root-stderr', file=sys.stderr, flush=True)
os._exit(7)
"""


class CaptureTests(unittest.TestCase):
    def setUp(self):
        self.base = Path(tempfile.gettempdir()).resolve()
        self.root = self.base / ("codex-native-capture-test-" + str(uuid.uuid4()))
        self.root.mkdir()
        self.safe_to_remove = True

    def tearDown(self):
        self.assertEqual(self.root.resolve().parent, self.base)
        self.assertFalse(self.root.is_symlink())
        if self.safe_to_remove:
            shutil.rmtree(self.root)
        else:
            print(
                f"Retained active/unverified fixture tree: {self.root}", file=sys.stderr
            )

    def invoke(self, argv, name="capture"):
        directory = self.root / name
        process = subprocess.Popen(
            [
                sys.executable,
                str(Path(capture.__file__).resolve()),
                "--output-dir",
                str(directory),
                "--",
                *argv,
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        # communicate(timeout) does not kill; every fixture self-terminates.
        try:
            stdout, stderr = process.communicate(timeout=15)
        except subprocess.TimeoutExpired:
            self.safe_to_remove = False
            raise
        return process.returncode, directory, stdout, stderr

    def test_literal_arguments_and_byte_exact_streams(self):
        args = [
            "",
            "with spaces",
            'quote"inside',
            "trail\\",
            "a&b|c",
            "%PATH%",
            "雪",
            "$HOME",
            "--option",
        ]
        script = "import json,sys;sys.stdout.buffer.write(json.dumps(sys.argv[1:]).encode());sys.stderr.buffer.write(bytes([0,255,13,10]))"
        code, directory, _, stderr = self.invoke([sys.executable, "-c", script, *args])
        self.assertEqual(code, 0)
        self.assertEqual(json.loads((directory / "stdout.log").read_bytes()), args)
        self.assertEqual((directory / "stderr.log").read_bytes(), b"\x00\xff\r\n")
        self.assertEqual(stderr, b"")

    def test_summary_is_byte_bounded_and_escapes_control_lines(self):
        suffix = (
            b"\n::error::not a workflow command\n"
            b"##[error]not an annotation\n"
            b"embedded ##[warning]not an annotation\n"
            b"##[add-mask]not a mask\n\x00\x1b\xff"
        )
        raw = b"x" * 100000 + suffix
        # Keep the Windows command line itself below its limit.
        script = f"import sys;sys.stdout.buffer.write(b'x'*100000+{suffix!r})"
        code, directory, stdout, _ = self.invoke([sys.executable, "-c", script])
        self.assertEqual(code, 0)
        self.assertEqual((directory / "stdout.log").read_bytes(), raw)
        self.assertLess(len(stdout), capture.SUMMARY_BYTES_PER_STREAM * 12 + 1000)
        self.assertFalse(any(line.startswith(b"::") for line in stdout.splitlines()))
        self.assertNotIn(b"##[", stdout)
        summary = next(
            line
            for line in stdout.splitlines()
            if line.startswith(b"capture stdout.log ")
        )
        self.assertEqual(
            json.loads(summary.split(b" tail=", 1)[1]),
            raw[-capture.SUMMARY_BYTES_PER_STREAM :].decode("utf-8", errors="replace"),
        )
        self.assertNotIn(b"\x00", stdout)
        self.assertNotIn(b"\x1b", stdout)

    def test_error_console_paths_escape_actions_commands(self):
        message = "##[error]not an annotation\n::warning::not an annotation"
        console = io.StringIO()
        with (
            patch.object(capture.shutil, "which", side_effect=OSError(message)),
            redirect_stderr(console),
        ):
            code = capture.capture([sys.executable], self.root / "startup", self.root)
        self.assertEqual(code, 1)
        self.assertNotIn("##[", console.getvalue())
        self.assertEqual(
            json.loads(console.getvalue().split("capture failed: ", 1)[1]), message
        )
        receipt = json.loads((self.root / "startup" / "native-result.json").read_text())
        self.assertEqual(receipt["error"], message)

        console = io.StringIO()
        with (
            patch.object(capture, "capture", side_effect=OSError(message)),
            patch.object(
                sys,
                "argv",
                [
                    "capture",
                    "--output-dir",
                    str(self.root / "preflight"),
                    "--",
                    sys.executable,
                ],
            ),
            redirect_stderr(console),
        ):
            code = capture.main()
        self.assertEqual(code, 1)
        self.assertNotIn("##[", console.getvalue())
        self.assertEqual(
            json.loads(console.getvalue().split("capture preflight failed: ", 1)[1]),
            message,
        )

    def test_native_exit_codes_are_preserved_in_cli_and_receipt(self):
        codes = [0, 7, 0xC0000005, 0xFFFFFFFF] if os.name == "nt" else [0, 7]
        for native_code in codes:
            with self.subTest(native_code=native_code):
                signed = native_code if native_code < 2**31 else native_code - 2**32
                code, directory, _, _ = self.invoke(
                    [sys.executable, "-c", f"import os;os._exit({signed})"],
                    str(native_code),
                )
                self.assertEqual(code, native_code)
                receipt = json.loads((directory / "native-result.json").read_text())
                self.assertEqual(
                    (receipt["status"], receipt["root_exit_code"]),
                    ("root-exited", native_code),
                )
                phase = json.loads((directory / "capture-phase.json").read_text())
                self.assertEqual(
                    (phase["phase"], phase["status"]), ("capture-complete", "complete")
                )

    def test_startup_failure_does_not_invent_native_exit(self):
        code, directory, _, _ = self.invoke([str(self.root / "missing.exe")])
        self.assertNotEqual(code, 0)
        receipt = json.loads((directory / "native-result.json").read_text())
        self.assertEqual(
            (receipt["status"], receipt["root_pid"], receipt["root_exit_code"]),
            ("startup-failed", None, None),
        )
        self.assertIn("error", receipt)
        self.assertEqual(
            json.loads((directory / "capture-phase.json").read_text())["status"],
            "failed",
        )

    @unittest.skipUnless(os.name == "nt", "Windows batch-file shell edge")
    def test_batch_target_is_rejected(self):
        code, directory, _, _ = self.invoke([str(self.root / "not-native.cmd"), "&"])
        self.assertNotEqual(code, 0)
        self.assertIn(
            "Batch files",
            json.loads((directory / "native-result.json").read_text())["error"],
        )

    def test_existing_directory_is_not_overwritten(self):
        directory = self.root / "capture"
        directory.mkdir()
        marker = directory / "stdout.log"
        marker.write_bytes(b"previous evidence")
        code, _, _, _ = self.invoke(
            [sys.executable, "-c", "raise AssertionError('must not run')"]
        )
        self.assertNotEqual(code, 0)
        self.assertEqual(marker.read_bytes(), b"previous evidence")
        self.assertFalse((directory / "native-result.json").exists())

    def test_later_summary_failure_preserves_immutable_native_result(self):
        for native_code in (0, 7):
            directory = self.root / str(native_code)
            saved = []

            def fail_after_receipt(path):
                saved.append((path / "native-result.json").read_bytes())
                raise OSError("synthetic console/tail failure")

            with (
                patch.object(capture, "emit_summary", fail_after_receipt),
                redirect_stdout(io.StringIO()),
                redirect_stderr(io.StringIO()),
            ):
                code = capture.capture(
                    [sys.executable, "-c", f"raise SystemExit({native_code})"],
                    directory,
                    self.root,
                )
            self.assertEqual(code, native_code or 1)
            self.assertEqual((directory / "native-result.json").read_bytes(), saved[0])
            phase = json.loads((directory / "capture-phase.json").read_text())
            self.assertEqual(
                (phase["phase"], phase["status"]), ("console-summary", "failed")
            )

    def test_phase_write_failure_after_exit_preserves_native_result(self):
        directory = self.root / "phase-failure"
        save_json = capture.save_json
        saved = []

        def fail_summary_phase(path, name, value):
            if name == "capture-phase.json" and value["phase"] == "console-summary":
                saved.append((path / "native-result.json").read_bytes())
                raise OSError("synthetic phase-write failure")
            save_json(path, name, value)

        with (
            patch.object(capture, "save_json", fail_summary_phase),
            redirect_stderr(io.StringIO()),
        ):
            code = capture.capture(
                [sys.executable, "-c", "raise SystemExit(7)"], directory, self.root
            )
        self.assertEqual(code, 7)
        self.assertEqual((directory / "native-result.json").read_bytes(), saved[0])
        # An interrupted/stale running phase must not be accepted as capture success.
        phase = json.loads((directory / "capture-phase.json").read_text())
        self.assertEqual(
            (phase["phase"], phase["status"]), ("native-start-and-wait", "running")
        )

    def test_closed_console_streams_preserve_native_result(self):
        for native_code in (0, 7):
            directory = self.root / f"closed-console-{native_code}"
            closed_stream = io.StringIO()
            closed_stream.close()
            with redirect_stdout(closed_stream), redirect_stderr(closed_stream):
                code = capture.capture(
                    [sys.executable, "-c", f"raise SystemExit({native_code})"],
                    directory,
                    self.root,
                )
            self.assertEqual(code, native_code or 1)
            receipt = json.loads((directory / "native-result.json").read_text())
            self.assertEqual(
                (receipt["status"], receipt["root_exit_code"]),
                ("root-exited", native_code),
            )
            phase = json.loads((directory / "capture-phase.json").read_text())
            self.assertEqual(
                (phase["phase"], phase["status"]), ("console-summary", "failed")
            )

    @unittest.skipUnless(os.name == "nt", "Windows inherited-handle fixture")
    def test_root_receipt_precedes_descendant_exit_and_late_output(self):
        self.safe_to_remove = False
        wrapper_return_observed = False
        try:
            code, directory, _, _ = self.invoke(
                [sys.executable, "-c", PARENT, str(self.root), CHILD]
            )
            wrapper_return_observed = True
            done_at_return = (self.root / "done").exists()
            receipt = json.loads((directory / "native-result.json").read_text())
            before = (directory / "stdout.log").read_bytes()
        finally:
            # Wait on the known fixture process handle; never kill by PID.
            pid_file = self.root / "child-pid"
            if pid_file.exists():
                kernel = ctypes.WinDLL("kernel32", use_last_error=True)
                kernel.OpenProcess.argtypes = [
                    wintypes.DWORD,
                    wintypes.BOOL,
                    wintypes.DWORD,
                ]
                kernel.OpenProcess.restype = wintypes.HANDLE
                kernel.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
                kernel.WaitForSingleObject.restype = wintypes.DWORD
                kernel.CloseHandle.argtypes = [wintypes.HANDLE]
                handle = kernel.OpenProcess(
                    0x00100000, False, int(pid_file.read_text())
                )
                if handle:
                    try:
                        descendant_exit_observed = (
                            kernel.WaitForSingleObject(handle, 10000) == 0
                        )
                        self.safe_to_remove = (
                            wrapper_return_observed and descendant_exit_observed
                        )
                    finally:
                        kernel.CloseHandle(handle)
                else:
                    self.safe_to_remove = (
                        wrapper_return_observed
                        and ctypes.get_last_error() == 87
                        and (self.root / "done").exists()
                    )
        self.assertTrue(
            self.safe_to_remove,
            "Wrapper or descendant exit unverified; no cleanup attempted",
        )
        self.assertEqual(code, 7)
        self.assertFalse(done_at_return)
        self.assertEqual(
            (
                receipt["root_exit_code"],
                receipt["descendant_completion"],
                receipt["log_completeness"],
            ),
            (7, "not-observed", "not-established"),
        )
        self.assertNotIn(b"descendant-end", before)
        self.assertIn(b"descendant-end", (directory / "stdout.log").read_bytes())
        self.assertIn(b"descendant-stderr-end", (directory / "stderr.log").read_bytes())

    @unittest.skipUnless(os.name == "nt", "Windows inherited-handle fixture")
    def test_descendant_exit_does_not_erase_wrapper_timeout(self):
        (self.root / "child-pid").write_text("123")
        try:
            with (
                patch.object(
                    self, "invoke", side_effect=subprocess.TimeoutExpired("fixture", 15)
                ),
                patch.object(ctypes, "WinDLL") as win_dll,
            ):
                kernel = win_dll.return_value
                kernel.OpenProcess.return_value = 456
                kernel.WaitForSingleObject.return_value = 0
                with self.assertRaises(subprocess.TimeoutExpired):
                    self.test_root_receipt_precedes_descendant_exit_and_late_output()
                kernel.WaitForSingleObject.assert_called_once_with(456, 10000)
                self.assertFalse(self.safe_to_remove)
        finally:
            # All process creation/waits were mocked; only this owned PID fixture exists.
            self.safe_to_remove = True


if __name__ == "__main__":
    unittest.main()
