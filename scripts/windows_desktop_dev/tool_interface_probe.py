"""Windows transport audition on an isolated, unauthenticated app-server.

No model requests, real threads, global configuration, or Desktop selection.
All arms use the native command/exec backend. This tests mechanics, not which
language a model generates best. Exact selected stream bytes stay in --output.
"""

import argparse
import base64
from collections import deque
import hashlib
import json
import os
from pathlib import Path
import queue
import shlex
import subprocess
import sys
import threading
import time


def child(mode, arguments):
    if mode == "argv":
        print(json.dumps(arguments, ensure_ascii=True))
    elif mode == "json-error":
        print('{"payload":"valid despite exit"}')
        print("diagnostic only", file=sys.stderr)
        return 7
    elif mode == "bad-json":
        print("not JSON")
    elif mode == "bytes":
        sys.stdout.buffer.write(bytes(range(256)) * 8)
        sys.stderr.buffer.write(b"stderr\x00\xff\n")
    elif mode == "flood":
        a = threading.Thread(target=lambda: sys.stdout.buffer.write(b"O" * 262144))
        b = threading.Thread(target=lambda: sys.stderr.buffer.write(b"E" * 262144))
        a.start()
        b.start()
        a.join()
        b.join()
    elif mode == "stdin":
        sys.stdout.buffer.write(b"READY\n")
        sys.stdout.buffer.flush()
        sys.stdout.buffer.write(sys.stdin.buffer.read())
    elif mode == "json-view":
        rows = json.loads(Path(arguments[0]).read_text(encoding="utf-8"))
        print(json.dumps([{"name": row["name"], "score": row["score"]}
                          for row in sorted(rows, key=lambda row: -row["score"]) if row["score"] >= 3]))
    return 0


class Rpc:
    def __init__(self, executable, home):
        home.mkdir()
        env = dict(os.environ)
        env.update(CODEX_HOME=str(home), CODEX_SQLITE_HOME=str(home),
                   CODEX_INTERNAL_APP_SERVER_REMOTE_CONTROL_DISABLED="1", OTEL_SDK_DISABLED="true")
        for key in ("OPENAI_API_KEY", "CODEX_API_KEY", "CODEX_CLI_PATH"):
            env.pop(key, None)
        self.messages, self.errors, self.pending, self.streams = queue.Queue(), deque(maxlen=12), {}, {}
        self.serial = 0
        self.process = subprocess.Popen(
            [str(executable), "-c", 'cli_auth_credentials_store="file"', "-c",
             f"sqlite_home={json.dumps(str(home))}", "app-server"],
            cwd=home, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, encoding="utf-8", creationflags=subprocess.CREATE_NO_WINDOW,
        )
        self.readers = [threading.Thread(target=self.read_stdout, daemon=True),
                        threading.Thread(target=self.read_stderr, daemon=True)]
        for reader in self.readers:
            reader.start()
        reply = self.wait(self.send("initialize", {"clientInfo": {"name": "coordinator-tool-probe", "version": "0.1.0"},
                                                   "capabilities": {"experimentalApi": True}}))
        if Path(reply["codexHome"]).resolve() != home.resolve():
            self.close()
            raise RuntimeError("isolated home mismatch")
        self.identity = reply["userAgent"]
        self.process.stdin.write(json.dumps({"method": "initialized"}) + "\n")
        self.process.stdin.flush()

    def read_stdout(self):
        for line in self.process.stdout:
            try:
                self.messages.put(json.loads(line))
            except json.JSONDecodeError:
                self.messages.put({"invalid_json": line[:1000]})

    def read_stderr(self):
        for line in self.process.stderr:
            self.errors.append(line[:1000])

    def send(self, method, params):
        self.serial += 1
        self.process.stdin.write(json.dumps({"id": self.serial, "method": method, "params": params}) + "\n")
        self.process.stdin.flush()
        return self.serial

    def pump(self):
        try:
            message = self.messages.get(timeout=.2)
        except queue.Empty:
            if self.process.poll() is not None:
                raise RuntimeError(f"probe server exited: {list(self.errors)}")
            return
        if "invalid_json" in message:
            raise RuntimeError(message)
        if "id" in message:
            self.pending[message["id"]] = message
        elif message.get("method") == "command/exec/outputDelta":
            p = message["params"]
            streams = self.streams.setdefault(p["processId"], {"stdout": bytearray(), "stderr": bytearray(), "capped": False})
            streams[p["stream"]].extend(base64.b64decode(p["deltaBase64"], validate=True))
            streams["capped"] |= p.get("capReached", False)

    def wait(self, identifier):
        deadline = time.monotonic() + 45
        while identifier not in self.pending and time.monotonic() < deadline:
            self.pump()
        if identifier not in self.pending:
            raise TimeoutError(f"RPC {identifier}: {list(self.errors)}")
        message = self.pending.pop(identifier)
        if "error" in message:
            raise RuntimeError(message["error"])
        return message["result"]

    def run(self, command, cwd, label, stdin=None):
        self.streams[label] = {"stdout": bytearray(), "stderr": bytearray(), "capped": False}
        start = time.monotonic()
        identifier = self.send("command/exec", {
            "command": command, "processId": label, "cwd": str(cwd),
            "streamStdoutStderr": True, "streamStdin": stdin is not None,
            "outputBytesCap": 1048576, "timeoutMs": 15000,
            "sandboxPolicy": {"type": "dangerFullAccess"},
            "env": {"PYTHONIOENCODING": "utf-8", "PYTHONUTF8": "1", "NO_COLOR": "1"},
        })
        if stdin is not None:
            deadline = time.monotonic() + 10
            while b"READY" not in self.streams[label]["stdout"] and time.monotonic() < deadline:
                self.pump()
                if identifier in self.pending:
                    final = self.wait(identifier)
                    raise RuntimeError(f"stdin fixture exited before READY: {final}")
            if b"READY" not in self.streams[label]["stdout"]:
                raise RuntimeError("stdin fixture never reached READY")
            self.wait(self.send("command/exec/write", {"processId": label,
                      "deltaBase64": base64.b64encode(stdin).decode("ascii"), "closeStdin": True}))
        final = self.wait(identifier)
        streams = self.streams.pop(label)
        return {"exit_code": final["exitCode"], "elapsed_seconds": time.monotonic() - start,
                "stdout": bytes(streams["stdout"]), "stderr": bytes(streams["stderr"]), "capped": streams["capped"]}

    def close(self):
        if self.process.poll() is None:
            self.process.stdin.close()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()  # Only this isolated probe server, never Desktop.
                self.process.wait(timeout=10)
        for reader in self.readers:
            reader.join(timeout=1)


def wrapped(arm, argv, paths):
    if arm == "direct":
        return argv
    if arm in ("powershell", "powershell-utf8"):
        quoted = " ".join("'" + x.replace("'", "''") + "'" for x in argv)
        prefix = "try { [Console]::OutputEncoding=[System.Text.Encoding]::UTF8 } catch {}\n" if arm == "powershell-utf8" else ""
        return [paths["powershell"], "-NoProfile", "-Command", prefix + "& " + quoted + "; exit $LASTEXITCODE"]
    if arm in ("bash", "bash-literal"):
        prefix = "export MSYS2_ARG_CONV_EXCL='*'; " if arm == "bash-literal" else ""
        return [paths["bash"], "--noprofile", "--norc", "-c", prefix + shlex.join(argv)]
    if arm == "nu":
        return [paths[arm], "--no-config-file", "-c",
                f"let executable = {json.dumps(argv[0], ensure_ascii=False)}; let args = {json.dumps(argv[1:], ensure_ascii=False)}; ^$executable ...$args"]
    raise ValueError(arm)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--codex", type=Path, required=True)
    parser.add_argument("--nu", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    paths = {"powershell": r"C:\Program Files\PowerShell\7\pwsh.exe", "bash": r"C:\Program Files\Git\bin\bash.exe", "nu": str(args.nu.resolve())}
    literal = ["two words", "", '"quoted"', "trailing\\", "single'quote", "%PATH%", "$HOME", "`", "*", "literal|pipe", "café 東京", "/not-a-path"]
    script = str(Path(__file__).resolve())
    data_file = out / "records.json"
    data_file.write_text(json.dumps([{"name": "low", "score": 1, "extra": "omit"},
                                    {"name": "café 東京", "score": 5, "extra": "omit"},
                                    {"name": "middle", "score": 3, "extra": "omit"}]), encoding="utf-8")
    selected_rows = [{"name": "café 東京", "score": 5}, {"name": "middle", "score": 3}]
    cases = [("argv", literal, 0, json.dumps(literal).encode() + b"\n", b""),
             ("json-error", [], 7, b'{"payload":"valid despite exit"}\n', b"diagnostic only\n"),
             ("bad-json", [], 0, b"not JSON\n", b""),
             ("bytes", [], 0, bytes(range(256)) * 8, b"stderr\x00\xff\n"),
             ("flood", [], 0, b"O" * 262144, b"E" * 262144),
             ("stdin", [], 0, b"READY\nhello stdin\x00\xff\n", b""),
             ("json-view", [str(data_file)], 0, json.dumps(selected_rows).encode(), b""),
             ("windows", [], 0, b'{"process":true,"service":true,"path":true,"acl":true}\n', b"")]
    results = []
    rpc = Rpc(args.codex.resolve(), out / "isolated-home")
    try:
        # An invalid RPC contract is an instrument failure, not 24 shell failures.
        preflight = rpc.run([sys.executable, "--version"], out, "preflight")
        if preflight["exit_code"] != 0 or not preflight["stdout"].startswith(b"Python "):
            raise RuntimeError("native RPC preflight failed")
        for arm in ("direct", "powershell", "powershell-utf8", "bash", "bash-literal", "nu"):
            for case, values, exit_code, stdout, stderr in cases:
                label = f"{arm}-{case}"
                command = wrapped(arm, [sys.executable, script, "--child", case, *values], paths)
                if case == "json-view" and arm == "nu":
                    command = [paths["nu"], "--no-config-file", "-c",
                               f"open {json.dumps(str(data_file))} | where score >= 3 | sort-by score --reverse | select name score | to json -r"]
                elif case == "json-view" and arm in ("powershell", "powershell-utf8"):
                    filename = str(data_file).replace("'", "''")
                    prefix = "try { [Console]::OutputEncoding=[System.Text.Encoding]::UTF8 } catch {}\n" if arm == "powershell-utf8" else ""
                    command = [paths["powershell"], "-NoProfile", "-Command",
                               prefix + f"$d = Get-Content -Raw -LiteralPath '{filename}' | ConvertFrom-Json; @($d | Where-Object score -GE 3 | Sort-Object score -Descending | Select-Object name,score) | ConvertTo-Json -Compress"]
                elif case == "windows":
                    inspect = "[ordered]@{process=((Get-Process -Id $PID).Id -eq $PID);service=((Get-Service -Name EventLog).Name -eq 'EventLog');path=(Test-Path -LiteralPath '.');acl=((Get-Acl -LiteralPath '.').Owner.Length -gt 0)} | ConvertTo-Json -Compress"
                    command = wrapped(arm, [paths["powershell"], "-NoProfile", "-Command", inspect], paths)
                row = {"arm": arm, "case": case, "command": command}
                try:
                    got = rpc.run(command, out, label, stdin=b"hello stdin\x00\xff\n" if case == "stdin" else None)
                    for stream in ("stdout", "stderr"):
                        data = got[stream]
                        (out / f"{label}.{stream}.bin").write_bytes(data)
                        row[stream] = {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
                    # Text newline translation is reported separately from byte corruption.
                    text_case = case in ("argv", "json-error", "bad-json", "json-view", "windows")
                    actual_out = got["stdout"].replace(b"\r\n", b"\n") if text_case else got["stdout"]
                    actual_err = got["stderr"].replace(b"\r\n", b"\n") if text_case else got["stderr"]
                    row.update({k: v for k, v in got.items() if k not in ("stdout", "stderr")})
                    try:
                        stdout_matches = json.loads(actual_out) == selected_rows if case == "json-view" else actual_out == stdout
                    except (UnicodeError, json.JSONDecodeError) as error:
                        row["payload_parse_error"] = str(error)
                        stdout_matches = False
                    row["checks"] = {"exit": got["exit_code"] == exit_code, "stdout": stdout_matches,
                                     "stderr": actual_err == stderr, "complete": not got["capped"]}
                    row["passed"] = all(row["checks"].values())
                except (RuntimeError, TimeoutError) as error:
                    row.update(passed=False, error=str(error))
                results.append(row)
                print(json.dumps({"arm": arm, "case": case, "passed": row["passed"], "checks": row.get("checks"), "error": row.get("error")}), flush=True)
    finally:
        rpc.close()
    report = {"schema": "tool-interface-probe-v1", "server": rpc.identity,
              "codex_sha256": hashlib.sha256(args.codex.read_bytes()).hexdigest(),
              "source_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "scope": "Deterministic native RPC transport fixtures, not model-generation or live-tool adoption evidence.", "results": results}
    (out / "report.json").write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8", newline="\n")


if __name__ == "__main__":
    if len(sys.argv) > 2 and sys.argv[1] == "--child":
        raise SystemExit(child(sys.argv[2], sys.argv[3:]))
    main()
