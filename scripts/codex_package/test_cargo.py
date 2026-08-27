#!/usr/bin/env python3

import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import call
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from codex_package.cargo import build_source_binaries
from codex_package.cargo import cargo_build_command
from codex_package.cargo import cargo_profile_output_dir
from codex_package.cargo import parse_cargo_host
from codex_package.cargo import validate_native_host_target_neutrality
from codex_package.cargo import source_binaries_for_target
from codex_package.cargo import CODEX_RS_ROOT
from codex_package.targets import PACKAGE_VARIANTS
from codex_package.targets import TARGET_SPECS


class SourceBinariesForTargetTest(unittest.TestCase):
    def test_macos_package_with_prebuilt_entrypoint_builds_nothing(self) -> None:
        self.assertEqual(
            source_binaries_for_target(
                TARGET_SPECS["aarch64-apple-darwin"],
                PACKAGE_VARIANTS["codex"],
                build_entrypoint=False,
                build_code_mode_host=False,
                build_bwrap=False,
                build_codex_command_runner=False,
                build_codex_windows_sandbox_setup=False,
            ),
            [],
        )

    def test_linux_package_with_prebuilt_entrypoint_and_bwrap_builds_nothing(
        self,
    ) -> None:
        self.assertEqual(
            source_binaries_for_target(
                TARGET_SPECS["x86_64-unknown-linux-musl"],
                PACKAGE_VARIANTS["codex"],
                build_entrypoint=False,
                build_code_mode_host=False,
                build_bwrap=False,
                build_codex_command_runner=False,
                build_codex_windows_sandbox_setup=False,
            ),
            [],
        )

    def test_windows_package_with_prebuilt_entrypoint_and_helpers_builds_nothing(
        self,
    ) -> None:
        self.assertEqual(
            source_binaries_for_target(
                TARGET_SPECS["x86_64-pc-windows-msvc"],
                PACKAGE_VARIANTS["codex"],
                build_entrypoint=False,
                build_code_mode_host=False,
                build_bwrap=False,
                build_codex_command_runner=False,
                build_codex_windows_sandbox_setup=False,
            ),
            [],
        )

    def test_missing_windows_helpers_are_built(self) -> None:
        self.assertEqual(
            source_binaries_for_target(
                TARGET_SPECS["x86_64-pc-windows-msvc"],
                PACKAGE_VARIANTS["codex"],
                build_entrypoint=False,
                build_code_mode_host=False,
                build_bwrap=False,
                build_codex_command_runner=True,
                build_codex_windows_sandbox_setup=True,
            ),
            ["codex-command-runner", "codex-windows-sandbox-setup"],
        )

    def test_missing_code_mode_host_is_built_for_app_server(self) -> None:
        self.assertEqual(
            source_binaries_for_target(
                TARGET_SPECS["aarch64-apple-darwin"],
                PACKAGE_VARIANTS["codex-app-server"],
                build_entrypoint=False,
                build_code_mode_host=True,
                build_bwrap=False,
                build_codex_command_runner=False,
                build_codex_windows_sandbox_setup=False,
            ),
            ["codex-code-mode-host"],
        )

    def test_build_uses_prebuilt_windows_helpers_without_running_cargo(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            entrypoint = touch_file(root / "codex.exe")
            code_mode_host = touch_file(root / "codex-code-mode-host.exe")
            command_runner = touch_file(root / "codex-command-runner.exe")
            sandbox_setup = touch_file(root / "codex-windows-sandbox-setup.exe")

            outputs = build_source_binaries(
                TARGET_SPECS["x86_64-pc-windows-msvc"],
                PACKAGE_VARIANTS["codex"],
                cargo=str(root / "cargo-that-should-not-run"),
                profile="release",
                entrypoint_bin=entrypoint,
                code_mode_host_bin=code_mode_host,
                bwrap_bin=None,
                codex_command_runner_bin=command_runner,
                codex_windows_sandbox_setup_bin=sandbox_setup,
            )

        self.assertEqual(outputs.entrypoint_bin, entrypoint)
        self.assertEqual(outputs.code_mode_host_bin, code_mode_host)
        self.assertEqual(outputs.codex_command_runner_bin, command_runner)
        self.assertEqual(outputs.codex_windows_sandbox_setup_bin, sandbox_setup)


class CargoBuildCommandTest(unittest.TestCase):
    def test_cross_target_build_keeps_explicit_target(self) -> None:
        spec = TARGET_SPECS["aarch64-apple-darwin"]

        self.assertEqual(
            cargo_build_command(
                "cargo",
                spec,
                "release",
                ["codex", "codex-code-mode-host"],
                cargo_native_host=False,
            ),
            [
                "cargo",
                "build",
                "--target",
                "aarch64-apple-darwin",
                "--profile",
                "release",
                "--bin",
                "codex",
                "--bin",
                "codex-code-mode-host",
            ],
        )

    def test_native_host_build_omits_target(self) -> None:
        spec = TARGET_SPECS["x86_64-pc-windows-msvc"]

        self.assertEqual(
            cargo_build_command(
                "cargo",
                spec,
                "release",
                ["codex", "codex-command-runner"],
                cargo_native_host=True,
                cargo_locked=True,
            ),
            [
                "cargo",
                "build",
                "--locked",
                "--profile",
                "release",
                "--bin",
                "codex",
                "--bin",
                "codex-command-runner",
            ],
        )


class CargoHostTest(unittest.TestCase):
    def test_parses_host_from_verbose_version(self) -> None:
        self.assertEqual(
            parse_cargo_host(
                "cargo 1.91.0 (ea2d97820 2025-10-10)\n"
                "release: 1.91.0\n"
                "host: x86_64-pc-windows-msvc\n"
                "libgit2: 1.9.1 (sys:0.20.0 vendored)\n"
            ),
            "x86_64-pc-windows-msvc",
        )

    def test_rejects_missing_or_ambiguous_host(self) -> None:
        for verbose_version in (
            "cargo 1.91.0\nrelease: 1.91.0\n",
            "host: x86_64-pc-windows-msvc\nhost: aarch64-pc-windows-msvc\n",
        ):
            with self.subTest(verbose_version=verbose_version):
                with self.assertRaisesRegex(RuntimeError, "unique host"):
                    parse_cargo_host(verbose_version)


class CargoProfileOutputDirTest(unittest.TestCase):
    def test_cross_target_output_includes_target_triple(self) -> None:
        target_dir = Path("cargo-target")
        with patch(
            "codex_package.cargo.cargo_target_dir",
            return_value=target_dir,
        ):
            output_dir = cargo_profile_output_dir(
                TARGET_SPECS["aarch64-apple-darwin"],
                "dev",
            )

        self.assertEqual(
            output_dir,
            target_dir / "aarch64-apple-darwin" / "debug",
        )

    def test_native_host_output_omits_target_triple(self) -> None:
        target_dir = Path("cargo-target")
        with patch(
            "codex_package.cargo.cargo_target_dir",
            return_value=target_dir,
        ):
            output_dir = cargo_profile_output_dir(
                TARGET_SPECS["x86_64-pc-windows-msvc"],
                "release",
                cargo_native_host=True,
            )

        self.assertEqual(output_dir, target_dir / "release")


class NativeHostTargetNeutralityTest(unittest.TestCase):
    def test_rejects_cargo_home_build_target(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            cwd = root / "workspace" / "crate"
            cargo_home = root / "cargo-home"
            cwd.mkdir(parents=True)
            cargo_home.mkdir()
            (cargo_home / "config.toml").write_text(
                "[build]\ntarget = 'aarch64-pc-windows-msvc'\n",
                encoding="utf-8",
            )

            with self.assertRaisesRegex(
                RuntimeError, "build.target or build.target-dir"
            ):
                validate_native_host_target_neutrality(
                    cwd=cwd,
                    cargo_home=cargo_home,
                )

    def test_rejects_ancestor_target_dir(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            cwd = root / "workspace" / "crate"
            cargo_home = root / "cargo-home"
            config_dir = root / "workspace" / ".cargo"
            cwd.mkdir(parents=True)
            cargo_home.mkdir()
            config_dir.mkdir()
            (config_dir / "config.toml").write_text(
                "[build]\ntarget-dir = '../redirected-target'\n",
                encoding="utf-8",
            )

            with self.assertRaisesRegex(
                RuntimeError, "build.target or build.target-dir"
            ):
                validate_native_host_target_neutrality(
                    cwd=cwd,
                    cargo_home=cargo_home,
                )

    def test_allows_unrelated_ancestor_target_table(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            cwd = root / "workspace" / "crate"
            cargo_home = root / "cargo-home"
            config_dir = root / "workspace" / ".cargo"
            cwd.mkdir(parents=True)
            cargo_home.mkdir()
            config_dir.mkdir()
            (config_dir / "config.toml").write_text(
                "[target.x86_64-pc-windows-msvc]\nrunner = 'runner.exe'\n",
                encoding="utf-8",
            )

            validate_native_host_target_neutrality(
                cwd=cwd,
                cargo_home=cargo_home,
            )


class NativeHostBuildTest(unittest.TestCase):
    def test_matching_host_is_verified_before_native_build(self) -> None:
        spec = TARGET_SPECS["x86_64-pc-windows-msvc"]
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            code_mode_host = touch_file(root / "prebuilt" / "codex-code-mode-host.exe")
            command_runner = touch_file(root / "prebuilt" / "codex-command-runner.exe")
            sandbox_setup = touch_file(
                root / "prebuilt" / "codex-windows-sandbox-setup.exe"
            )
            built_entrypoint = root / "target" / "release" / "codex.exe"

            def run_cargo(
                command: list[str], **kwargs: object
            ) -> subprocess.CompletedProcess[str]:
                if command == ["test-cargo", "-vV"]:
                    return subprocess.CompletedProcess(
                        command,
                        0,
                        stdout="host: x86_64-pc-windows-msvc\n",
                    )
                touch_file(built_entrypoint)
                return subprocess.CompletedProcess(command, 0)

            with (
                patch.dict(
                    os.environ,
                    {"CARGO_BUILD_TARGET": "aarch64-pc-windows-msvc"},
                ),
                patch(
                    "codex_package.cargo.cargo_target_dir",
                    return_value=root / "target",
                ),
                patch(
                    "codex_package.cargo.resolve_codex_v8_cargo_env",
                    return_value={},
                ),
                patch(
                    "codex_package.cargo.validate_native_host_target_neutrality",
                ) as validate_target_neutrality,
                patch(
                    "codex_package.cargo.subprocess.run",
                    side_effect=run_cargo,
                ) as run,
                patch("builtins.print"),
            ):
                outputs = build_source_binaries(
                    spec,
                    PACKAGE_VARIANTS["codex"],
                    cargo="test-cargo",
                    profile="release",
                    entrypoint_bin=None,
                    code_mode_host_bin=code_mode_host,
                    bwrap_bin=None,
                    codex_command_runner_bin=command_runner,
                    codex_windows_sandbox_setup_bin=sandbox_setup,
                    cargo_native_host=True,
                    cargo_locked=True,
                )

        self.assertEqual(outputs.entrypoint_bin, built_entrypoint)
        validate_target_neutrality.assert_called_once_with()
        self.assertEqual(
            run.call_args_list[0],
            call(
                ["test-cargo", "-vV"],
                cwd=CODEX_RS_ROOT,
                check=True,
                capture_output=True,
                text=True,
            ),
        )
        self.assertEqual(
            run.call_args_list[1].args[0],
            [
                "test-cargo",
                "build",
                "--locked",
                "--profile",
                "release",
                "--bin",
                "codex",
            ],
        )
        self.assertNotIn("CARGO_BUILD_TARGET", run.call_args_list[1].kwargs["env"])

    def test_mismatched_host_stops_before_build(self) -> None:
        spec = TARGET_SPECS["x86_64-pc-windows-msvc"]
        with tempfile.TemporaryDirectory() as temp_dir:
            root = Path(temp_dir)
            code_mode_host = touch_file(root / "codex-code-mode-host.exe")
            command_runner = touch_file(root / "codex-command-runner.exe")
            sandbox_setup = touch_file(root / "codex-windows-sandbox-setup.exe")

            with patch(
                "codex_package.cargo.subprocess.run",
                return_value=subprocess.CompletedProcess(
                    ["test-cargo", "-vV"],
                    0,
                    stdout="host: aarch64-pc-windows-msvc\n",
                ),
            ) as run:
                with self.assertRaisesRegex(
                    RuntimeError,
                    "selected 'x86_64-pc-windows-msvc'.*reported "
                    "'aarch64-pc-windows-msvc'",
                ):
                    build_source_binaries(
                        spec,
                        PACKAGE_VARIANTS["codex"],
                        cargo="test-cargo",
                        profile="release",
                        entrypoint_bin=None,
                        code_mode_host_bin=code_mode_host,
                        bwrap_bin=None,
                        codex_command_runner_bin=command_runner,
                        codex_windows_sandbox_setup_bin=sandbox_setup,
                        cargo_native_host=True,
                    )

        self.assertEqual(run.call_count, 1)


def touch_file(path: Path) -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("", encoding="utf-8")
    return path.resolve()


if __name__ == "__main__":
    unittest.main()
