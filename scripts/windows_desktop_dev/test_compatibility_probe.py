"""Synthetic contract/guard tests; no live profile, Desktop or package selection."""

from copy import deepcopy
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location(
    "compatibility_probe", Path(__file__).with_name("compatibility_probe.py"))
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)


def result(text, code=0):
    return {"exit_code": code, "stopped_for": None, "streams_complete": True,
            "reader_errors": [], "stdout_bytes": len(text.encode()),
            "stderr_bytes": 0, "stdout": text, "stderr": ""}


class ContractTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="codex-compat-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        machine = patch.object(probe, "windows_system_directory", return_value=self.root / "machine")
        machine.start()
        self.addCleanup(machine.stop)
        self.exe = self.root / "candidate.exe"
        self.asar = self.root / "app.asar"
        self.exe.write_bytes(b"fixture executable identity; never executed")
        self.asar.write_bytes(b"fixture desktop identity")
        self.request = {
            "schema_version": 1,
            "candidate": {"executable": str(self.exe),
                          "executable_sha256": probe.digest_file(self.exe),
                          "package_fingerprint": "a" * 64, "version": "0.163.0"},
            "desktop": {"package_version": "fixture",
                        "app_asar": {"path": str(self.asar),
                                     "sha256": probe.digest_file(self.asar)}},
            "required_history_formats": ["paginated_refs_v1"],
        }
        self.registry = {
            "schema_version": 1,
            "desktop_contracts": [{"id": "fixture-contract", "package_version": "fixture",
                                   "app_asar_sha256": probe.digest_file(self.asar),
                                   "client_version": "fixture-client"}],
            "qualified_readers": [{"executable_sha256": probe.digest_file(self.exe),
                                   "package_fingerprint": "a" * 64,
                                   "history_formats": ["paginated_refs_v1"]}],
        }
        self.registry_path = self.root / "registry.json"
        self.write_registry()
        self.calls = []

    def write_registry(self):
        self.registry_path.write_text(json.dumps(self.registry), encoding="utf-8")

    def fake_capture(self, argv, home, env):
        self.calls.append((argv, home, env))
        self.assertNotIn("--startup-sign-out", argv)
        self.assertEqual(str(home), env["CODEX_HOME"])
        config = (home / "config.toml").read_text(encoding="utf-8")
        if "invalid = [" in config:
            return result(json.dumps({"error": {"code": -32600, "message": "fixture error",
                                                "data": {"reason": "configLoad"}}}), 1)
        requirements = {"allowedLoginMethods": ["api"]} if "forced_login_method" in config else None
        return result(json.dumps({"appServerVersion": "0.163.0",
                                  "configRequirements": {"requirements": requirements}}))

    def run_probe(self, capture=None, request=None):
        return probe.preflight(request or self.request, self.registry_path,
                               self.root / "scratch", capture or self.fake_capture)

    def test_success_checks_real_contract_shape_policy_and_error_path(self):
        r = self.run_probe()
        self.assertTrue(r["compatible"])
        self.assertEqual(["startup", "local_policy", "malformed_config"],
                         [c["name"] for c in r["checks"]])
        self.assertEqual(3, len(self.calls))

    def test_relative_paths_are_refused_before_execution(self):
        for location, key in (("candidate", "executable"), ("desktop", "app_asar")):
            request = deepcopy(self.request)
            if location == "candidate":
                request[location][key] = "candidate.exe"
            else:
                request[location][key]["path"] = "app.asar"
            self.assertFalse(self.run_probe(request=request)["compatible"])
        self.assertEqual([], self.calls)

    def test_malformed_registry_shapes_are_structured_refusals(self):
        for registry in ([], None, 3, {"schema_version": 1, "desktop_contracts": [None]}):
            self.registry = registry
            self.write_registry()
            self.assertFalse(self.run_probe()["compatible"])
        self.assertEqual([], self.calls)

    def test_malformed_error_data_is_refused(self):
        for data in (None, [], "configLoad", 3):
            with self.assertRaises(ValueError):
                probe.validate_response(result(json.dumps({"error": {
                    "code": -32600, "message": "bad config", "data": data}}), 1),
                    "malformed_config", "0.163.0")

    def test_machine_policy_is_not_ignored_or_read(self):
        machine = self.root / "machine"
        machine.mkdir()
        (machine / "requirements.toml").write_text("private policy contents", encoding="utf-8")
        r = self.run_probe()
        self.assertFalse(r["compatible"])
        self.assertNotIn("private policy contents", json.dumps(r))
        self.assertEqual([], self.calls)

    def test_machine_policy_appearance_stops_next_fixture(self):
        def capture(argv, home, env):
            r = self.fake_capture(argv, home, env)
            machine = self.root / "machine"
            machine.mkdir()
            (machine / "config.toml").write_text("new machine policy", encoding="utf-8")
            return r
        self.assertFalse(self.run_probe(capture)["compatible"])
        self.assertEqual(1, len(self.calls))

    def test_unknown_desktop_never_executes_or_creates_scratch(self):
        self.registry["desktop_contracts"] = []
        self.write_registry()
        self.assertFalse(self.run_probe()["compatible"])
        self.assertEqual([], self.calls)
        self.assertFalse((self.root / "scratch").exists())

    def test_version_alone_does_not_qualify_an_unknown_reader(self):
        self.registry["qualified_readers"] = []
        self.write_registry()
        self.assertFalse(self.run_probe()["compatible"])
        self.assertEqual([], self.calls)

    def test_required_history_cannot_be_downgraded(self):
        for required in ([], ["legacy"], None):
            request = deepcopy(self.request)
            request["required_history_formats"] = required
            self.assertFalse(self.run_probe(request=request)["compatible"])
        self.assertEqual([], self.calls)

    def test_duplicate_contract_or_reader_is_not_a_choice(self):
        for key in ("desktop_contracts", "qualified_readers"):
            saved = deepcopy(self.registry[key])
            self.registry[key] *= 2
            self.write_registry()
            self.assertFalse(self.run_probe()["compatible"])
            self.registry[key] = saved
        self.assertEqual([], self.calls)

    def test_hash_drift_before_first_call_is_refused(self):
        self.exe.write_bytes(b"changed")
        self.assertFalse(self.run_probe()["compatible"])
        self.assertEqual([], self.calls)

    def test_hash_drift_between_calls_stops_before_next_execution(self):
        def capture(argv, home, env):
            r = self.fake_capture(argv, home, env)
            self.exe.write_bytes(b"changed after first call")
            return r
        self.assertFalse(self.run_probe(capture)["compatible"])
        self.assertEqual(1, len(self.calls))

    def test_registry_drift_does_not_publish_qualified(self):
        def capture(argv, home, env):
            r = self.fake_capture(argv, home, env)
            self.registry_path.write_text("{}", encoding="utf-8")
            return r
        self.assertFalse(self.run_probe(capture)["compatible"])

    def test_unknown_input_is_not_echoed_as_diagnostic_data(self):
        request = deepcopy(self.request)
        request["unrelated_secret"] = "not for this result"
        r = self.run_probe(request=request)
        self.assertIsNone(r["bindings"])
        self.assertNotIn("not for this result", json.dumps(r))

    def test_bad_responses_never_pass(self):
        for text in ("{}", "[]", "null", "{} trailing", "{}\n{}",
                     '{"x":1,"x":2}', '{"x":NaN}',
                     '{"appServerVersion":"0.163.0","configRequirements":{}}'):
            with self.subTest(text=text):
                r = self.run_probe(lambda *_: result(text))
                self.assertFalse(r["compatible"])

    def test_fake_null_policy_does_not_satisfy_policy_fixture(self):
        def capture(argv, home, env):
            self.fake_capture(argv, home, env)
            return result('{"appServerVersion":"0.163.0","configRequirements":{"requirements":null}}')
        r = self.run_probe(capture)
        self.assertFalse(r["compatible"])
        self.assertEqual("local_policy", r["checks"][-1]["name"])

    def test_configuration_mutation_and_backend_artifact_are_failures(self):
        for mutation in ("config", "sessions"):
            def capture(argv, home, env):
                r = self.fake_capture(argv, home, env)
                if mutation == "config":
                    (home / "config.toml").write_text("changed", encoding="utf-8")
                else:
                    (home / "sessions").mkdir()
                return r
            self.assertFalse(self.run_probe(capture)["compatible"])

    def test_inherited_credentials_and_endpoints_are_not_copied(self):
        previous = os.environ.get("OPENAI_API_KEY")
        try:
            os.environ["OPENAI_API_KEY"] = "test-value-not-a-credential"
            self.run_probe()
            self.assertNotIn("OPENAI_API_KEY", self.calls[0][2])
        finally:
            if previous is None:
                os.environ.pop("OPENAI_API_KEY", None)
            else:
                os.environ["OPENAI_API_KEY"] = previous


class CaptureTests(unittest.TestCase):
    def test_timeout_and_output_bound(self):
        with tempfile.TemporaryDirectory(prefix="codex-capture-test-") as directory:
            root = Path(directory)
            env = {k: v for k, v in os.environ.items() if k in ("PATH", "SystemRoot", "WINDIR")}
            r = probe.bounded_capture([sys.executable, "-c", "import time;time.sleep(10)"],
                                      root, env, timeout=0.05)
            self.assertEqual("timeout", r["stopped_for"])
            r = probe.bounded_capture([sys.executable, "-c", "print('x'*1000000)"],
                                      root, env, byte_limit=1024)
            self.assertEqual("output_limit", r["stopped_for"])
            self.assertLessEqual(len(r["stdout"].encode()), 1024)

    def test_incomplete_stream_cannot_pass(self):
        r = result('{"appServerVersion":"0.163.0","configRequirements":{"requirements":null}}')
        r["streams_complete"] = False
        with self.assertRaises(ValueError):
            probe.validate_response(r, "startup", "0.163.0")


if __name__ == "__main__":
    unittest.main()
