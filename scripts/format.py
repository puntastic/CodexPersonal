#!/usr/bin/env python3
"""Format repository sources or check that they are already formatted."""

import argparse
import json
import os
import shlex
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor, as_completed
from dataclasses import dataclass
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]


@dataclass(frozen=True)
class Command:
    args: tuple[str, ...]
    cwd: Path = REPO_ROOT


@dataclass(frozen=True)
class FormatterGroup:
    name: str
    commands: tuple[Command, ...]


@dataclass(frozen=True)
class FormatterResult:
    name: str
    output: str
    returncode: int


def just_formatter_group(*, check: bool) -> FormatterGroup:
    args = ["just", "--unstable", "--fmt"]
    if check:
        args.append("--check")
    return FormatterGroup("Just", (Command(tuple(args)),))


def rust_formatter_group(*, check: bool) -> FormatterGroup:
    options = ["--", "--config", "imports_granularity=Item"]
    if check:
        options.append("--check")
    cwd = REPO_ROOT / "codex-rs"
    if sys.platform != "win32":
        return FormatterGroup("Rust", (Command(("cargo", "fmt", *options), cwd),))

    metadata = json.loads(
        subprocess.check_output(
            ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"],
            cwd=cwd,
        )
    )
    commands = tuple(
        Command(
            (
                "cargo",
                "fmt",
                *(arg for name in batch for arg in ("-p", name)),
                *options,
            ),
            cwd,
        )
        for batch in windows_rust_package_batches(metadata)
    )
    return FormatterGroup("Rust", commands)


def windows_rust_package_batches(
    metadata: dict, budget: int = 16_000
) -> list[list[str]]:
    """Bound rustfmt's expanded file argv, not just the short cargo invocation.

    CreateProcess limits the command line to 32K UTF-16 units. Leave substantial
    room for cargo's additional flags and the executable path; one package stays
    atomic so cargo retains ownership of target selection and formatting rules.
    """
    members = set(metadata["workspace_members"])
    batches = []
    batch = []
    size = 0
    for package in metadata["packages"]:
        if package["id"] not in members:
            continue
        cost = sum(
            len(target["src_path"].encode("utf-16-le")) // 2 + 3
            for target in package["targets"]
        )
        if cost > budget:
            raise RuntimeError(
                f"Rustfmt targets for {package['name']} exceed Windows argv budget"
            )
        if batch and size + cost > budget:
            batches.append(batch)
            batch = []
            size = 0
        batch.append(package["name"])
        size += cost
    if batch:
        batches.append(batch)
    if not batches:
        raise RuntimeError("Cargo metadata contained no workspace packages to format")
    return batches


def buildifier_formatter_group(*, check: bool) -> FormatterGroup:
    repository_files = subprocess.check_output(
        ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
        cwd=REPO_ROOT,
    ).split(b"\0")
    buildifier_files: list[str] = []
    for encoded_path in repository_files:
        if not encoded_path:
            continue
        path = Path(os.fsdecode(encoded_path))
        name = path.name
        if (
            name in {"BUILD", "WORKSPACE", "MODULE.bazel"}
            or name.startswith(("BUILD.", "WORKSPACE."))
            or name.endswith((".BUILD.bazel", ".MODULE.bazel", ".bzl", ".sky"))
            or ".bzl." in name
            or ".sky." in name
        ):
            buildifier_files.append(path.as_posix())
    buildifier_files.sort()

    # Invoke DotSlash explicitly because Windows does not honor shebangs.
    buildifier_args = [
        "dotslash",
        str(REPO_ROOT / "tools" / "buildifier"),
        "-mode=check" if check else "-mode=fix",
        "-lint=off",
        *buildifier_files,
    ]
    return FormatterGroup("Bazel/Starlark", (Command(tuple(buildifier_args)),))


def python_sdk_formatter_group(*, check: bool) -> FormatterGroup:
    # Each `--project` retains its local dependency and Ruff configuration context.
    uv_run_args = [
        "uv",
        "run",
        "--frozen",
        "--project",
        "sdk/python",
        "--only-group",
        "format",
    ]
    format_args = [
        *uv_run_args,
        "ruff",
        "format",
    ]
    if check:
        format_args.append("--check")
        # `ruff check --diff` reports lint-driven rewrites without changing files.
        # It is the check-mode counterpart of `--fix --fix-only`, not a full lint gate.
        lint_args = ["ruff", "check", "--diff"]
    else:
        # Ruff's lint fixer and formatter are separate passes: the first applies
        # fixable lint rewrites, while the second formats source layout.
        lint_args = ["ruff", "check", "--fix", "--fix-only"]

    return FormatterGroup(
        "Python SDK",
        (
            Command((*uv_run_args, *lint_args, "sdk/python")),
            Command((*format_args, "sdk/python")),
        ),
    )


def python_scripts_formatter_group(*, check: bool) -> FormatterGroup:
    args = ["uv", "run", "--frozen", "--project", "scripts", "ruff", "format"]
    if check:
        args.append("--check")
    args.append(".")
    return FormatterGroup("Python scripts", (Command(tuple(args)),))


def formatter_groups(*, check: bool) -> tuple[FormatterGroup, ...]:
    return (
        just_formatter_group(check=check),
        rust_formatter_group(check=check),
        buildifier_formatter_group(check=check),
        python_sdk_formatter_group(check=check),
        python_scripts_formatter_group(check=check),
    )


def run_formatter_group(group: FormatterGroup) -> FormatterResult:
    """Run one formatter group sequentially and return its buffered output."""
    for command in group.commands:
        try:
            process = subprocess.run(
                command.args,
                cwd=command.cwd,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                check=False,
            )
        except OSError as error:
            output = f"$ {shlex.join(command.args)}\n{error}\n"
            return FormatterResult(group.name, output, 1)

        if process.returncode != 0:
            output = f"$ {shlex.join(command.args)}\n{process.stdout}"
            if process.stdout and not process.stdout.endswith("\n"):
                output += "\n"
            return FormatterResult(group.name, output, process.returncode)

    return FormatterResult(group.name, "", 0)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--check",
        action="store_true",
        help="check formatting without modifying files",
    )
    args = parser.parse_args()
    groups = formatter_groups(check=args.check)

    failures: list[str] = []
    with ThreadPoolExecutor(max_workers=len(groups)) as executor:
        futures = [executor.submit(run_formatter_group, group) for group in groups]
        for future in as_completed(futures):
            result = future.result()
            if result.returncode != 0:
                failures.append(result.name)
                print(f"==> {result.name} formatter failed", file=sys.stderr)
                print(result.output, end="", file=sys.stderr)

    if failures:
        print(f"Formatting failed: {', '.join(failures)}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
