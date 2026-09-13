#!/usr/bin/env python3

import hashlib
import io
import json
import os
import sys
import tempfile
import unittest
from collections.abc import Iterator
from contextlib import contextmanager, redirect_stdout
from pathlib import Path
from unittest.mock import MagicMock, call, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from codex_package import v8
from codex_package.targets import TARGET_SPECS, TargetSpec
from codex_package.v8 import RustyV8ArtifactPair
from codex_package.v8 import fetch_codex_v8_artifacts
from codex_package.v8 import main
from codex_package.v8 import resolve_codex_v8_cargo_env


VERSION = "150.4.0"
PROFILE = "ptrcomp_sandbox_release"
RELEASE_URL = f"https://github.com/openai/codex/releases/download/rusty-v8-v{VERSION}"
WINDOWS_MANIFESTS = {
    "aarch64-pc-windows-msvc": {
        f"rusty_v8_{PROFILE}_aarch64-pc-windows-msvc.lib.gz": (
            "54722842af36b74248c403ff531254efac6ff65d281198bab0c6350fc1188ad4"
        ),
        f"src_binding_{PROFILE}_aarch64-pc-windows-msvc.rs": (
            "dabf78ba1faac127660db9862b1d0354175c71b8db2d4fcb5bacbd9c93576b16"
        ),
    },
    "x86_64-pc-windows-msvc": {
        f"rusty_v8_{PROFILE}_x86_64-pc-windows-msvc.lib.gz": (
            "732ec5da4243aa166799780c8519a5eea6f32f6e47657a323342794dc3c239d6"
        ),
        f"src_binding_{PROFILE}_x86_64-pc-windows-msvc.rs": (
            "dabf78ba1faac127660db9862b1d0354175c71b8db2d4fcb5bacbd9c93576b16"
        ),
    },
}


class ResolveCodexV8CargoEnvTest(unittest.TestCase):
    def test_complete_overrides_remain_authoritative(self) -> None:
        environ = {
            "RUSTY_V8_ARCHIVE": "archive",
            "RUSTY_V8_SRC_BINDING_PATH": "binding",
        }
        with patch("codex_package.v8.fetch_codex_v8_artifacts") as fetch:
            cargo_env = resolve_codex_v8_cargo_env(
                TARGET_SPECS["x86_64-pc-windows-msvc"],
                environ=environ,
            )

        self.assertEqual(cargo_env, {})
        fetch.assert_not_called()

    def test_partial_override_is_rejected(self) -> None:
        for name in ("RUSTY_V8_ARCHIVE", "RUSTY_V8_SRC_BINDING_PATH"):
            with self.subTest(name=name):
                with self.assertRaisesRegex(RuntimeError, "set together"):
                    resolve_codex_v8_cargo_env(
                        TARGET_SPECS["x86_64-pc-windows-msvc"],
                        environ={name: "artifact"},
                    )

    def test_v8_from_source_skips_artifact_resolution(self) -> None:
        for value in ("true", "1", "yes"):
            with self.subTest(value=value):
                with patch("codex_package.v8.fetch_codex_v8_artifacts") as fetch:
                    cargo_env = resolve_codex_v8_cargo_env(
                        TARGET_SPECS["x86_64-pc-windows-msvc"],
                        environ={"V8_FROM_SOURCE": value},
                    )

                self.assertEqual(cargo_env, {})
                fetch.assert_not_called()


class FetchCodexV8ArtifactsTest(unittest.TestCase):
    def test_windows_msvc_names_and_release_manifest_checksums(self) -> None:
        for target, manifest in WINDOWS_MANIFESTS.items():
            with (
                self.subTest(target=target),
                tempfile.TemporaryDirectory() as temp_dir,
            ):
                cache_root = Path(temp_dir)
                manifest_name = f"rusty_v8_{PROFILE}_{target}.sha256"

                def download(url: str, dest: Path) -> None:
                    self.assertEqual(url, f"{RELEASE_URL}/{manifest_name}")
                    dest.parent.mkdir(parents=True, exist_ok=True)
                    dest.write_text(
                        "".join(
                            f"{digest}  {name}\n" for name, digest in manifest.items()
                        ),
                        encoding="utf-8",
                    )

                with (
                    patch("codex_package.v8.download_file", side_effect=download),
                    patch("codex_package.v8.ensure_valid_artifact") as ensure,
                ):
                    artifacts = fetch_codex_v8_artifacts(
                        TARGET_SPECS[target],
                        version=VERSION,
                        cache_root=cache_root,
                    )

                expected_paths = {
                    name: cache_root / f"rusty-v8-{VERSION}-{target}" / name
                    for name in manifest
                }
                self.assertEqual(
                    artifacts,
                    RustyV8ArtifactPair(
                        archive=expected_paths[f"rusty_v8_{PROFILE}_{target}.lib.gz"],
                        binding=expected_paths[f"src_binding_{PROFILE}_{target}.rs"],
                    ),
                )
                self.assertEqual(
                    ensure.call_args_list,
                    [
                        call(
                            path,
                            manifest[name],
                            f"{RELEASE_URL}/{name}",
                        )
                        for name, path in expected_paths.items()
                    ],
                )

    def test_valid_cached_pair_is_reused_without_download(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            cache_root = Path(temp_dir)
            target = "x86_64-pc-windows-msvc"
            artifacts = write_cached_pair(cache_root, target)

            with (
                patch("codex_package.v8.REPO_ROOT", cache_root),
                patch("codex_package.v8.download_file") as download,
            ):
                actual = fetch_codex_v8_artifacts(
                    TARGET_SPECS[target],
                    version=VERSION,
                    cache_root=cache_root,
                )

        self.assertEqual(actual, artifacts)
        download.assert_not_called()

    def test_invalid_cache_requires_fetch_and_fails_closed_offline(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            cache_root = Path(temp_dir)
            target = "x86_64-pc-windows-msvc"
            artifacts = write_cached_pair(cache_root, target)
            artifacts.archive.write_bytes(b"corrupt")

            with (
                patch("codex_package.v8.REPO_ROOT", cache_root),
                patch(
                    "codex_package.v8.download_file", side_effect=OSError("offline")
                ) as download,
            ):
                with self.assertRaisesRegex(OSError, "offline"):
                    fetch_codex_v8_artifacts(
                        TARGET_SPECS[target],
                        version=VERSION,
                        cache_root=cache_root,
                    )

        self.assertEqual(
            download.call_args_list,
            [
                call(
                    f"{RELEASE_URL}/rusty_v8_{PROFILE}_{target}.sha256",
                    cache_root
                    / f"rusty-v8-{VERSION}-{target}"
                    / f"rusty_v8_{PROFILE}_{target}.sha256",
                )
            ],
        )

    def test_invalid_cached_manifest_requires_fetch_and_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            cache_root = Path(temp_dir)
            target = "x86_64-pc-windows-msvc"
            artifacts = write_cached_pair(cache_root, target)
            manifest = artifacts.archive.parent / f"rusty_v8_{PROFILE}_{target}.sha256"
            manifest.write_text("not a checksum\n", encoding="utf-8")

            with (
                patch("codex_package.v8.REPO_ROOT", cache_root),
                patch(
                    "codex_package.v8.download_file", side_effect=OSError("offline")
                ) as download,
            ):
                with self.assertRaisesRegex(OSError, "offline"):
                    fetch_codex_v8_artifacts(
                        TARGET_SPECS[target],
                        version=VERSION,
                        cache_root=cache_root,
                    )

        self.assertEqual(
            download.call_args_list,
            [call(f"{RELEASE_URL}/{manifest.name}", manifest)],
        )


class MainTest(unittest.TestCase):
    def test_cli_emits_absolute_cargo_environment_json(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            cache_root = Path(temp_dir).resolve()
            target = "x86_64-pc-windows-msvc"
            artifacts = write_cached_pair(cache_root, target)
            stdout = io.StringIO()

            with (
                patch("codex_package.v8.REPO_ROOT", cache_root),
                patch(
                    "codex_package.v8.resolved_v8_crate_version", return_value=VERSION
                ),
                patch("codex_package.v8.download_file") as download,
                patch.dict(os.environ, {}, clear=True),
                redirect_stdout(stdout),
            ):
                exit_code = main(
                    [
                        "--target",
                        target,
                        "--cache-root",
                        str(cache_root),
                    ]
                )

        self.assertEqual(exit_code, 0)
        self.assertEqual(
            json.loads(stdout.getvalue()),
            {
                "RUSTY_V8_ARCHIVE": str(artifacts.archive.resolve()),
                "RUSTY_V8_SRC_BINDING_PATH": str(artifacts.binding.resolve()),
            },
        )
        download.assert_not_called()

    def test_cli_emits_empty_json_for_v8_from_source(self) -> None:
        stdout = io.StringIO()
        with (
            patch.dict(os.environ, {"V8_FROM_SOURCE": "1"}, clear=True),
            patch("codex_package.v8.fetch_codex_v8_artifacts") as fetch,
            redirect_stdout(stdout),
        ):
            exit_code = main(
                [
                    "--target",
                    "x86_64-pc-windows-msvc",
                    "--cache-root",
                    "cache",
                ]
            )

        self.assertEqual(exit_code, 0)
        self.assertEqual(json.loads(stdout.getvalue()), {})
        fetch.assert_not_called()

    def test_cli_emits_empty_json_for_complete_caller_overrides(self) -> None:
        stdout = io.StringIO()
        environ = {
            "RUSTY_V8_ARCHIVE": "caller-archive",
            "RUSTY_V8_SRC_BINDING_PATH": "caller-binding",
        }
        with (
            patch.dict(os.environ, environ, clear=True),
            patch("codex_package.v8.fetch_codex_v8_artifacts") as fetch,
            redirect_stdout(stdout),
        ):
            exit_code = main(
                [
                    "--target",
                    "x86_64-pc-windows-msvc",
                    "--cache-root",
                    "cache",
                ]
            )

        self.assertEqual(exit_code, 0)
        self.assertEqual(json.loads(stdout.getvalue()), {})
        fetch.assert_not_called()

    def test_cli_rejects_partial_caller_override(self) -> None:
        stdout = io.StringIO()
        with (
            patch.dict(
                os.environ,
                {"RUSTY_V8_ARCHIVE": "caller-archive"},
                clear=True,
            ),
            redirect_stdout(stdout),
        ):
            with self.assertRaisesRegex(RuntimeError, "set together"):
                main(
                    [
                        "--target",
                        "x86_64-pc-windows-msvc",
                        "--cache-root",
                        "cache",
                    ]
                )

        self.assertEqual(stdout.getvalue(), "")


def write_cached_pair(cache_root: Path, target: str) -> RustyV8ArtifactPair:
    cache_dir = cache_root / f"rusty-v8-{VERSION}-{target}"
    cache_dir.mkdir(parents=True)
    archive = cache_dir / f"rusty_v8_{PROFILE}_{target}.lib.gz"
    binding = cache_dir / f"src_binding_{PROFILE}_{target}.rs"
    archive.write_bytes(b"archive")
    binding.write_bytes(b"binding")
    checksums = cache_dir / f"rusty_v8_{PROFILE}_{target}.sha256"
    checksums.write_text(
        "".join(
            f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n"
            for path in (archive, binding)
        ),
        encoding="utf-8",
    )
    pins = cache_root / "third_party/v8/rusty_v8_150_4_0_release_manifests.sha256"
    pins.parent.mkdir(parents=True)
    pins.write_text(
        f"{hashlib.sha256(checksums.read_bytes()).hexdigest()}  {checksums.name}\n",
        encoding="utf-8",
    )
    return RustyV8ArtifactPair(archive=archive, binding=binding)


class TrustedCodexV8ArtifactsTest(unittest.TestCase):
    version = "150.4.0"

    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)

    @contextmanager
    def release(
        self,
        target: str,
        *,
        line_ending: bytes = b"\n",
        trusted_digest: str | None = None,
        trusted_name: str | None = None,
        create_pins: bool = True,
    ) -> Iterator[tuple[TargetSpec, MagicMock, str]]:
        spec = TARGET_SPECS[target]
        profile = v8.V8_ARTIFACT_PROFILE
        archive_name = (
            f"rusty_v8_{profile}_{target}.lib.gz"
            if spec.is_windows
            else f"librusty_v8_{profile}_{target}.a.gz"
        )
        binding_name = f"src_binding_{profile}_{target}.rs"
        manifest_name = f"rusty_v8_{profile}_{target}.sha256"
        archive = b"trusted V8 archive"
        binding = b"trusted V8 binding"
        manifest = (
            line_ending.join(
                (
                    f"{hashlib.sha256(archive).hexdigest()}  {archive_name}".encode(),
                    f"{hashlib.sha256(binding).hexdigest()}  {binding_name}".encode(),
                )
            )
            + line_ending
        )
        payloads = {
            manifest_name: manifest,
            archive_name: archive,
            binding_name: binding,
        }

        if create_pins:
            pins = (
                self.root / "third_party/v8/rusty_v8_150_4_0_release_manifests.sha256"
            )
            pins.parent.mkdir(parents=True)
            digest = trusted_digest or hashlib.sha256(manifest).hexdigest()
            name = trusted_name or manifest_name
            pins.write_bytes(f"{digest}  {name}".encode() + line_ending)

        def download(_url: str, destination: Path) -> None:
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(payloads[destination.name])

        with (
            patch.object(v8, "REPO_ROOT", self.root),
            patch.object(v8, "download_file", side_effect=download) as download_file,
        ):
            yield spec, download_file, manifest_name

    def test_fetches_artifacts_after_authenticating_manifest(self) -> None:
        with self.release("x86_64-unknown-linux-gnu") as (
            spec,
            download,
            manifest_name,
        ):
            artifacts = v8.fetch_codex_v8_artifacts(
                spec, version=self.version, cache_root=self.root / "cache"
            )

            self.assertEqual(artifacts.archive.read_bytes(), b"trusted V8 archive")
            self.assertEqual(artifacts.binding.read_bytes(), b"trusted V8 binding")
            self.assertEqual(download.call_args_list[0].args[1].name, manifest_name)
            self.assertEqual(download.call_count, 3)

    def test_authenticates_windows_manifest_with_crlf(self) -> None:
        with self.release("x86_64-pc-windows-msvc", line_ending=b"\r\n") as (
            spec,
            download,
            _manifest_name,
        ):
            artifacts = v8.fetch_codex_v8_artifacts(
                spec, version=self.version, cache_root=self.root / "cache"
            )

            self.assertEqual(artifacts.archive.read_bytes(), b"trusted V8 archive")
            self.assertEqual(artifacts.binding.read_bytes(), b"trusted V8 binding")
            self.assertEqual(download.call_count, 3)

    def test_self_consistent_cached_pair_still_requires_trusted_manifest(self) -> None:
        with self.release("x86_64-pc-windows-msvc") as (spec, download, manifest_name):
            cache_root = self.root / "cache"
            artifacts = v8.fetch_codex_v8_artifacts(
                spec, version=self.version, cache_root=cache_root
            )
            artifacts.archive.write_bytes(b"untrusted replacement")
            manifest = artifacts.archive.parent / manifest_name
            manifest.write_bytes(
                b"".join(
                    f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n".encode()
                    for path in (artifacts.archive, artifacts.binding)
                )
            )
            download.reset_mock()
            download.side_effect = OSError("offline")

            with self.assertRaisesRegex(OSError, "offline"):
                v8.fetch_codex_v8_artifacts(
                    spec, version=self.version, cache_root=cache_root
                )
            download.assert_called_once()

    def test_rejects_tampered_manifest_before_downloading_artifacts(self) -> None:
        with self.release("x86_64-unknown-linux-gnu", trusted_digest="0" * 64) as (
            spec,
            download,
            manifest_name,
        ):
            with self.assertRaisesRegex(
                RuntimeError, "does not match its trusted SHA-256"
            ):
                v8.fetch_codex_v8_artifacts(
                    spec, version=self.version, cache_root=self.root / "cache"
                )

            download.assert_called_once()
            self.assertEqual(download.call_args.args[1].name, manifest_name)

    def test_rejects_missing_manifest_pin_before_downloading_artifacts(self) -> None:
        with self.release(
            "x86_64-unknown-linux-gnu", trusted_name="another-target.sha256"
        ) as (spec, download, manifest_name):
            with self.assertRaisesRegex(RuntimeError, "has no trusted SHA-256"):
                v8.fetch_codex_v8_artifacts(
                    spec, version=self.version, cache_root=self.root / "cache"
                )

            download.assert_called_once()
            self.assertEqual(download.call_args.args[1].name, manifest_name)

    def test_rejects_missing_pin_file_before_downloading_artifacts(self) -> None:
        with self.release("x86_64-unknown-linux-gnu", create_pins=False) as (
            spec,
            download,
            manifest_name,
        ):
            with self.assertRaises(FileNotFoundError):
                v8.fetch_codex_v8_artifacts(
                    spec, version=self.version, cache_root=self.root / "cache"
                )

            download.assert_called_once()
            self.assertEqual(download.call_args.args[1].name, manifest_name)


if __name__ == "__main__":
    unittest.main()
