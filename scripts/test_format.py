import json
import unittest
from unittest.mock import patch

import format as formatter


def package(name, paths):
    return {"id": name, "name": name, "targets": [{"src_path": path} for path in paths]}


class WindowsRustFormattingTest(unittest.TestCase):
    def test_batches_expanded_targets_without_omitting_or_duplicating_members(self):
        metadata = {
            "workspace_members": ["a", "b", "c"],
            "packages": [
                package("a", ["a" * 10]),
                package("dependency", ["x" * 100]),
                package("b", ["b" * 10]),
                package("c", ["c" * 10]),
            ],
        }
        self.assertEqual(
            formatter.windows_rust_package_batches(metadata, budget=26),
            [["a", "b"], ["c"]],
        )

    def test_utf16_width_and_all_targets_count_toward_limit(self):
        metadata = {
            "workspace_members": ["a", "b"],
            "packages": [package("a", ["😀", "a"]), package("b", ["b"])],
        }
        self.assertEqual(
            formatter.windows_rust_package_batches(metadata, budget=12), [["a"], ["b"]]
        )

    def test_an_oversized_single_package_fails_without_silently_skipping_it(self):
        metadata = {"workspace_members": ["a"], "packages": [package("a", ["a" * 40])]}
        with self.assertRaisesRegex(RuntimeError, "a exceed Windows argv budget"):
            formatter.windows_rust_package_batches(metadata, budget=20)

    def test_windows_keeps_check_mode_and_cargo_package_selection(self):
        metadata = {"workspace_members": ["a"], "packages": [package("a", ["a.rs"])]}
        with (
            patch.object(formatter.sys, "platform", "win32"),
            patch.object(
                formatter.subprocess,
                "check_output",
                return_value=json.dumps(metadata).encode(),
            ),
        ):
            result = formatter.rust_formatter_group(check=True)
        self.assertEqual(
            result.commands[0].args,
            (
                "cargo",
                "fmt",
                "-p",
                "a",
                "--",
                "--config",
                "imports_granularity=Item",
                "--check",
            ),
        )

    def test_other_hosts_keep_the_original_single_invocation(self):
        with (
            patch.object(formatter.sys, "platform", "linux"),
            patch.object(formatter.subprocess, "check_output") as metadata,
        ):
            result = formatter.rust_formatter_group(check=False)
        metadata.assert_not_called()
        self.assertEqual(
            result.commands[0].args,
            ("cargo", "fmt", "--", "--config", "imports_granularity=Item"),
        )


if __name__ == "__main__":
    unittest.main()
