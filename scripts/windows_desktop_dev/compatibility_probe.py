"""Bounded local startup-contract probes; never select a package or use live history.

The caller owns full package validation and final transaction binding. Reader
qualification is exact recorded evidence, not a declaration inferred from --version.
Only the fixed startup command is executed, in new file-only synthetic homes.
"""

import argparse
import ctypes
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import threading
import time
import uuid


MAX_STREAM_BYTES = 262144
FILE_AUTH = "cli_auth_credentials_store = 'file'\n"


def windows_system_directory():
    """Match the loader's known-folder lookup, not overridable ProgramData env."""
    if os.name != "nt":
        raise ValueError("this startup probe requires the Windows policy resolver")
    folder = ctypes.create_string_buffer(uuid.UUID("62ab5d82-fdc1-4dc3-a9dd-070d1d495d97").bytes_le)
    pointer = ctypes.c_void_p()
    get_folder = ctypes.windll.shell32.SHGetKnownFolderPath
    get_folder.argtypes = [ctypes.c_void_p, ctypes.c_uint32, ctypes.c_void_p,
                          ctypes.POINTER(ctypes.c_void_p)]
    get_folder.restype = ctypes.c_long
    if get_folder(ctypes.byref(folder), 0, None, ctypes.byref(pointer)) != 0 or not pointer.value:
        raise ValueError("machine policy location could not be established; no fixture executed")
    try:
        return Path(ctypes.wstring_at(pointer)) / "OpenAI" / "Codex"
    finally:
        free = ctypes.windll.ole32.CoTaskMemFree
        free.argtypes = [ctypes.c_void_p]
        free(pointer)


def assert_no_machine_policy(directory=None):
    """A fresh home is insufficient if enforced machine policy selects keyring auth.

    Do not bypass that policy. This isolated fixture route is unavailable until a
    separate policy-aware qualification is designed. No policy contents are read.
    """
    directory = windows_system_directory() if directory is None else Path(directory)
    checked = []
    for name in ("config.toml", "requirements.toml"):
        path = directory / name
        try:
            path.stat()
        except FileNotFoundError:
            parent = path.parent
            while True:
                try:
                    parent.stat()
                    if not parent.is_dir():
                        raise ValueError("machine policy ancestor is not a directory")
                    break
                except FileNotFoundError:
                    if parent == parent.parent:
                        raise ValueError("machine policy location is unavailable")
                    parent = parent.parent
            checked.append(str(path))
        else:
            raise ValueError("machine-wide Codex configuration/policy is present; isolated probe unavailable")
    return checked


def digest_file(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def reject_duplicates(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON member")
        result[key] = value
    return result


def strict_json(text):
    def invalid_constant(value):
        raise ValueError("non-finite JSON number")

    return json.loads(text, object_pairs_hook=reject_duplicates,
                      parse_constant=invalid_constant)


def fixture_environment(home):
    env = {name: os.environ[name] for name in
           ("PATH", "SystemRoot", "WINDIR", "COMSPEC", "PATHEXT")
           if name in os.environ}
    for name in ("sqlite", "tmp", "appdata", "localappdata"):
        (home / name).mkdir()
    env.update({
        "HOME": str(home), "USERPROFILE": str(home),
        "APPDATA": str(home / "appdata"),
        "LOCALAPPDATA": str(home / "localappdata"),
        "XDG_CONFIG_HOME": str(home / "appdata"),
        "XDG_CACHE_HOME": str(home / "localappdata"),
        "CODEX_HOME": str(home), "CODEX_SQLITE_HOME": str(home / "sqlite"),
        "TEMP": str(home / "tmp"), "TMP": str(home / "tmp"),
        "TMPDIR": str(home / "tmp"),
        "NO_PROXY": "127.0.0.1,localhost", "no_proxy": "127.0.0.1,localhost",
    })
    return env


def bounded_capture(argv, home, env, timeout=20, byte_limit=MAX_STREAM_BYTES):
    """Bound memory and root-process lifetime; a missing EOF cannot earn success."""
    buffers = [bytearray(), bytearray()]
    counts = [0, 0]
    overflow = threading.Event()
    reader_errors = []

    def read_stream(stream, index):
        try:
            while True:
                chunk = stream.read1(4096)
                if not chunk:
                    return
                counts[index] += len(chunk)
                room = max(0, byte_limit - len(buffers[index]))
                buffers[index].extend(chunk[:room])
                if counts[index] > byte_limit:
                    overflow.set()
        except (OSError, ValueError):
            reader_errors.append(index)

    flags = subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0
    proc = subprocess.Popen(argv, cwd=home, env=env, shell=False,
                            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, creationflags=flags)
    threads = [threading.Thread(target=read_stream, args=(stream, index), daemon=True)
               for index, stream in enumerate((proc.stdout, proc.stderr))]
    for thread in threads:
        thread.start()
    deadline = time.monotonic() + timeout
    stopped_for = None
    while proc.poll() is None:
        if overflow.is_set() or time.monotonic() >= deadline:
            stopped_for = "output_limit" if overflow.is_set() else "timeout"
            # This is the exact fixture process we created, not a Desktop/Cargo PID.
            proc.terminate()
            try:
                proc.wait(timeout=2)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=2)
            break
        time.sleep(0.01)
    for thread in threads:
        thread.join(timeout=1)
    complete = not any(thread.is_alive() for thread in threads)
    if complete:
        proc.stdout.close()
        proc.stderr.close()
    if overflow.is_set():
        stopped_for = "output_limit"
    return {"exit_code": proc.returncode, "stopped_for": stopped_for,
            "streams_complete": complete, "reader_errors": reader_errors,
            "stdout_bytes": counts[0], "stderr_bytes": counts[1],
            "stdout": bytes(buffers[0]).decode("utf-8", errors="strict"),
            "stderr": bytes(buffers[1]).decode("utf-8", errors="replace")}


def validate_response(capture, kind, version):
    if capture["stopped_for"] or not capture["streams_complete"] or capture["reader_errors"]:
        raise ValueError("bounded capture did not complete")
    if kind != "malformed_config" and capture["exit_code"] != 0:
        raise ValueError(f"startup command failed with exit {capture['exit_code']}; bounded diagnostics retained in the fixture")
    document = strict_json(capture["stdout"])
    if not isinstance(document, dict):
        raise ValueError("startup response is not one JSON object")
    if kind == "malformed_config":
        error = document.get("error")
        if capture["exit_code"] == 0 or not isinstance(error, dict):
            raise ValueError("malformed configuration did not produce a failing error envelope")
        if type(error.get("code")) not in (int, float) or not isinstance(error.get("message"), str):
            raise ValueError("error envelope lacks typed code/message")
        data = error.get("data")
        if "configRequirements" in document or not isinstance(data, dict) or data.get("reason") != "configLoad":
            raise ValueError("configuration error was not preserved as configLoad")
    else:
        if capture["exit_code"] != 0 or "error" in document:
            raise ValueError("startup command failed")
        if document.get("appServerVersion") != version:
            raise ValueError("startup version disagrees with package manifest")
        config = document.get("configRequirements")
        if not isinstance(config, dict) or "requirements" not in config:
            raise ValueError("startup response lacks configRequirements.requirements")
        requirements = config["requirements"]
        if requirements is not None and not isinstance(requirements, dict):
            raise ValueError("requirements has an invalid type")
        if kind == "local_policy" and (
                not isinstance(requirements, dict) or
                requirements.get("allowedLoginMethods") != ["api"]):
            raise ValueError("fixture login policy was not carried through the real loader")
    return {"response_shape": "valid", "error_path_checked": kind == "malformed_config"}


def check_fixture(home, original):
    if (home / "config.toml").read_text(encoding="utf-8") != original:
        raise ValueError("fixture configuration changed")
    if any((home / name).exists() for name in (
            "sessions", "archived_sessions", "app-server-control", "app-server-daemon",
            "auth.json")):
        raise ValueError("unexpected backend or credential artifact in fixture")
    if any((home / "sqlite").iterdir()) or any(".sqlite" in path.name for path in home.iterdir()):
        raise ValueError("unexpected database artifact in fixture")


def verify_input_bindings(request):
    candidate = request["candidate"]
    desktop = request["desktop"]
    if digest_file(candidate["executable"]) != candidate["executable_sha256"]:
        raise ValueError("candidate executable changed")
    if digest_file(desktop["app_asar"]["path"]) != desktop["app_asar"]["sha256"]:
        raise ValueError("Desktop archive changed")


def validate_request(request):
    if not isinstance(request, dict) or set(request) != {
            "schema_version", "candidate", "desktop", "required_history_formats"}:
        raise ValueError("request must contain only the declared compatibility fields")
    candidate, desktop = request["candidate"], request["desktop"]
    if not isinstance(candidate, dict) or set(candidate) != {
            "executable", "executable_sha256", "package_fingerprint", "version"}:
        raise ValueError("invalid candidate binding")
    if not isinstance(desktop, dict) or set(desktop) != {"package_version", "app_asar"}:
        raise ValueError("invalid Desktop binding")
    if not isinstance(desktop["app_asar"], dict) or set(desktop["app_asar"]) != {"path", "sha256"}:
        raise ValueError("invalid Desktop archive binding")
    for value in (candidate["executable_sha256"], candidate["package_fingerprint"],
                  desktop["app_asar"]["sha256"]):
        if not isinstance(value, str) or not re.fullmatch(r"[0-9a-f]{64}", value):
            raise ValueError("binding hashes must be lowercase SHA256")
    for value in (candidate["executable"], candidate["version"],
                  desktop["package_version"], desktop["app_asar"]["path"]):
        if not isinstance(value, str) or not value.strip():
            raise ValueError("binding paths and versions must be nonempty strings")
    for value in (candidate["executable"], desktop["app_asar"]["path"]):
        if not Path(value).is_absolute() or Path(value) != Path(value).resolve():
            raise ValueError("bound executable and Desktop archive paths must be absolute and canonical")


def preflight(request, registry_path, scratch_root, capture=bounded_capture):
    started = datetime.now(timezone.utc).isoformat()
    registry_path = Path(registry_path).resolve()
    if registry_path.stat().st_size > 262144:
        raise ValueError("qualification registry exceeds the supported size")
    registry_bytes = registry_path.read_bytes()
    if len(registry_bytes) > 262144:
        raise ValueError("qualification registry exceeds the supported size")
    registry = strict_json(registry_bytes.decode("utf-8-sig"))
    result = {"schema_version": 1, "status": "unqualified", "compatible": False,
              "started_at_utc": started, "bindings": None,
              "registry_sha256": hashlib.sha256(registry_bytes).hexdigest(), "checks": [], "blockers": [],
              "proof_boundary": "Local startup contract plus exact recorded reader evidence; not authenticated policy, full codec testing, live resume, network isolation or adoption."}
    try:
        validate_request(request)
        result["bindings"] = request
        if (not isinstance(registry, dict) or type(registry.get("schema_version")) is not int
                or registry["schema_version"] != 1 or type(request.get("schema_version")) is not int
                or request["schema_version"] != 1):
            raise ValueError("unsupported compatibility schema")
        for key in ("desktop_contracts", "qualified_readers"):
            if not isinstance(registry.get(key), list) or any(not isinstance(item, dict) for item in registry[key]):
                raise ValueError("invalid qualification registry collection")
        desktop, candidate = request["desktop"], request["candidate"]
        contracts = [item for item in registry["desktop_contracts"]
                     if item["package_version"] == desktop["package_version"] and
                     item["app_asar_sha256"] == desktop["app_asar"]["sha256"]]
        readers = [item for item in registry["qualified_readers"]
                   if item["executable_sha256"] == candidate["executable_sha256"] and
                   item["package_fingerprint"] == candidate["package_fingerprint"]]
        if len(contracts) != 1:
            raise ValueError("Desktop contract is unknown or ambiguous; review its actual startup consumer")
        if len(readers) != 1:
            raise ValueError("exact candidate has no unique qualified-reader record; qualification is not inferred from its version")
        required = request.get("required_history_formats")
        if required != ["paginated_refs_v1"]:
            raise ValueError("this project gate requires paginated_refs_v1; no silent requirement downgrade")
        if not set(required).issubset(readers[0]["history_formats"]):
            raise ValueError("candidate qualification does not cover required history")
        verify_input_bindings(request)
        result["absent_machine_policy_paths"] = assert_no_machine_policy()
        result["contract_id"] = contracts[0]["id"]
        result["reader_evidence"] = readers[0]
        root = Path(scratch_root).resolve()
        root.mkdir(parents=True, exist_ok=True)
        run = root / ("compatibility-" + uuid.uuid4().hex)
        run.mkdir()
        result["fixture_root"] = str(run)
        client = json.dumps({"name": "codex_desktop", "title": "Codex Desktop",
                             "version": contracts[0]["client_version"]}, separators=(",", ":"))
        base_argv = [candidate["executable"], "app-server", "--analytics-default-enabled",
                     "--print-startup-config", "--startup-client-info", client]
        fixtures = (
            ("startup", FILE_AUTH + "[features.cue_activation]\nenabled = true\nmode = 'advisory'\n", ["--strict-config"]),
            ("local_policy", FILE_AUTH + "forced_login_method = 'api'\n", []),
            ("malformed_config", FILE_AUTH + "invalid = [\n", []),
        )
        for kind, config, extra in fixtures:
            verify_input_bindings(request)
            if assert_no_machine_policy() != result["absent_machine_policy_paths"]:
                raise ValueError("machine policy location changed during the probe")
            home = run / kind
            home.mkdir()
            (home / "config.toml").write_text(config, encoding="utf-8")
            env = fixture_environment(home)
            check = {"name": kind, "passed": False, "attempted": True}
            result["checks"].append(check)
            observation = capture(base_argv + extra, home, env)
            check.update({"exit_code": observation["exit_code"],
                          "stopped_for": observation["stopped_for"],
                          "stdout_bytes": observation["stdout_bytes"],
                          "stderr_bytes": observation["stderr_bytes"],
                          "stdout_sha256": hashlib.sha256(observation["stdout"].encode()).hexdigest()})
            try:
                check.update(validate_response(observation, kind, candidate["version"]))
                check_fixture(home, config)
                check["passed"] = True
            except (ValueError, KeyError, TypeError) as error:
                check["error"] = str(error)
                # Synthetic output only; bounded diagnostics stay in the private fixture.
                (home / "stdout.txt").write_text(observation["stdout"], encoding="utf-8")
                (home / "stderr.txt").write_text(observation["stderr"], encoding="utf-8")
                raise
        verify_input_bindings(request)
        if assert_no_machine_policy() != result["absent_machine_policy_paths"]:
            raise ValueError("machine policy location changed during the probe")
        if digest_file(registry_path) != result["registry_sha256"]:
            raise ValueError("qualification registry changed during the probe")
        result.update(status="qualified", compatible=True)
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        result["blockers"].append(str(error))
        if result["checks"]:
            result["status"] = "failed"
    result["completed_at_utc"] = datetime.now(timezone.utc).isoformat()
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--request", required=True, type=Path)
    parser.add_argument("--registry", required=True, type=Path)
    parser.add_argument("--scratch-root", required=True, type=Path)
    args = parser.parse_args()
    try:
        if args.request.stat().st_size > 65536:
            raise ValueError("compatibility request exceeds the supported size")
        request = strict_json(args.request.read_text(encoding="utf-8-sig"))
        result = preflight(request, args.registry, args.scratch_root)
    except (OSError, ValueError, KeyError, TypeError) as error:
        result = {"schema_version": 1, "status": "unqualified", "compatible": False,
                  "blockers": [str(error)], "checks": []}
    print(json.dumps(result, ensure_ascii=True))
    return 0 if result["compatible"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
