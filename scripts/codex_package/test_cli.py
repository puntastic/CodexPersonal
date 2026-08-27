#!/usr/bin/env python3

import argparse
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from codex_package.cli import parse_args
from codex_package.cli import parse_package_version


class PackageVersionTest(unittest.TestCase):
    def test_accepts_release_prerelease_and_build_versions(self) -> None:
        for version in (
            "0.0.0",
            "1.2.3",
            "0.0.0-internal.deadbeef",
            "1.2.3-alpha.1+build.01",
            "18446744073709551615.0.0",
        ):
            with self.subTest(version=version):
                self.assertEqual(parse_package_version(version), version)

    def test_rejects_versions_the_runtime_cannot_parse(self) -> None:
        for version in (
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "v1.2.3",
            "01.2.3",
            "1.02.3",
            "1.2.03",
            "1.2.3-",
            "1.2.3-alpha..1",
            "1.2.3-01",
            "1.2.3+",
            "1.2.3+build..1",
            "18446744073709551616.0.0",
        ):
            with self.subTest(version=version):
                with self.assertRaises(argparse.ArgumentTypeError):
                    parse_package_version(version)


class CargoNativeHostOptionTest(unittest.TestCase):
    def test_parses_native_host_build_option(self) -> None:
        with patch(
            "sys.argv",
            [
                "build_codex_package.py",
                "--target",
                "x86_64-pc-windows-msvc",
                "--cargo-native-host",
                "--cargo-locked",
            ],
        ):
            args = parse_args()

        self.assertEqual(
            (args.target, args.cargo_native_host, args.cargo_locked),
            ("x86_64-pc-windows-msvc", True, True),
        )


if __name__ == "__main__":
    unittest.main()
