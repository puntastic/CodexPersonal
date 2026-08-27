"""Cargo builds for source-built Codex package artifacts."""

import os
import subprocess
from dataclasses import dataclass
from pathlib import Path

from .targets import REPO_ROOT
from .targets import PackageVariant
from .targets import TargetSpec
from .v8 import resolve_codex_v8_cargo_env


CODEX_RS_ROOT = REPO_ROOT / "codex-rs"


@dataclass(frozen=True)
class SourceBuildOutputs:
    entrypoint_bin: Path
    code_mode_host_bin: Path
    bwrap_bin: Path | None
    codex_command_runner_bin: Path | None
    codex_windows_sandbox_setup_bin: Path | None


def build_source_binaries(
    spec: TargetSpec,
    variant: PackageVariant,
    *,
    cargo: str,
    profile: str,
    entrypoint_bin: Path | None,
    code_mode_host_bin: Path | None,
    bwrap_bin: Path | None,
    codex_command_runner_bin: Path | None,
    codex_windows_sandbox_setup_bin: Path | None,
    cargo_native_host: bool = False,
    cargo_locked: bool = False,
) -> SourceBuildOutputs:
    validate_prebuilt_resource_inputs(
        spec,
        bwrap_bin=bwrap_bin,
        codex_command_runner_bin=codex_command_runner_bin,
        codex_windows_sandbox_setup_bin=codex_windows_sandbox_setup_bin,
    )
    binaries = source_binaries_for_target(
        spec,
        variant,
        build_entrypoint=entrypoint_bin is None,
        build_code_mode_host=code_mode_host_bin is None,
        build_bwrap=spec.is_linux and bwrap_bin is None,
        build_codex_command_runner=spec.is_windows and codex_command_runner_bin is None,
        build_codex_windows_sandbox_setup=spec.is_windows
        and codex_windows_sandbox_setup_bin is None,
    )
    if binaries:
        if cargo_native_host:
            validate_cargo_native_host(cargo, spec.target)
            validate_native_host_target_neutrality()

        cmd = cargo_build_command(
            cargo,
            spec,
            profile,
            binaries,
            cargo_native_host=cargo_native_host,
            cargo_locked=cargo_locked,
        )

        cargo_env = None
        if entrypoint_bin is None or code_mode_host_bin is None:
            codex_v8_env = resolve_codex_v8_cargo_env(spec)
            if codex_v8_env:
                cargo_env = {**os.environ, **codex_v8_env}
        if cargo_native_host:
            cargo_env = dict(os.environ if cargo_env is None else cargo_env)
            cargo_env.pop("CARGO_BUILD_TARGET", None)

        print("+", " ".join(cmd))
        subprocess.run(
            cmd,
            cwd=CODEX_RS_ROOT,
            check=True,
            env=cargo_env,
        )

    output_dir = cargo_profile_output_dir(
        spec,
        profile,
        cargo_native_host=cargo_native_host,
    )
    outputs = SourceBuildOutputs(
        entrypoint_bin=resolve_output_path(
            entrypoint_bin,
            output_dir / variant.entrypoint_name(spec),
        ),
        code_mode_host_bin=(
            code_mode_host_bin.resolve()
            if code_mode_host_bin is not None
            else output_dir / f"codex-code-mode-host{spec.exe_suffix}"
        ),
        bwrap_bin=resolve_output_path(
            bwrap_bin,
            output_dir / "bwrap" if spec.is_linux else None,
        ),
        codex_command_runner_bin=resolve_output_path(
            codex_command_runner_bin,
            output_dir / "codex-command-runner.exe" if spec.is_windows else None,
        ),
        codex_windows_sandbox_setup_bin=resolve_output_path(
            codex_windows_sandbox_setup_bin,
            output_dir / "codex-windows-sandbox-setup.exe" if spec.is_windows else None,
        ),
    )
    validate_source_outputs(outputs)
    return outputs


def cargo_build_command(
    cargo: str,
    spec: TargetSpec,
    profile: str,
    binaries: list[str],
    *,
    cargo_native_host: bool,
    cargo_locked: bool = False,
) -> list[str]:
    cmd = [cargo, "build"]
    if not cargo_native_host:
        cmd.extend(["--target", spec.target])
    if cargo_locked:
        cmd.append("--locked")
    cmd.extend(["--profile", profile])
    for binary in binaries:
        cmd.extend(["--bin", binary])
    return cmd


def validate_cargo_native_host(cargo: str, package_target: str) -> None:
    result = subprocess.run(
        [cargo, "-vV"],
        cwd=CODEX_RS_ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    cargo_host = parse_cargo_host(result.stdout)
    if cargo_host != package_target:
        raise RuntimeError(
            "--cargo-native-host requires the selected package target to match "
            f"Cargo's host exactly; selected {package_target!r}, but "
            f"`{cargo} -vV` reported {cargo_host!r}."
        )


def parse_cargo_host(verbose_version: str) -> str:
    hosts = []
    for line in verbose_version.splitlines():
        key, separator, value = line.partition(":")
        if separator and key.strip() == "host" and value.strip():
            hosts.append(value.strip())

    if len(hosts) != 1:
        raise RuntimeError("Could not determine a unique host from `cargo -vV` output.")
    return hosts[0]


def validate_native_host_target_neutrality(
    *,
    cwd: Path = CODEX_RS_ROOT,
    cargo_home: Path | None = None,
) -> None:
    configured_layout_files = [
        path
        for path in cargo_config_paths(cwd=cwd, cargo_home=cargo_home)
        if cargo_config_declares_native_output_override(path)
    ]
    if configured_layout_files:
        rendered = ", ".join(str(path) for path in configured_layout_files)
        raise RuntimeError(
            "--cargo-native-host requires Cargo's native target/<profile> output "
            "layout, but build.target or build.target-dir is configured in: "
            f"{rendered}. Remove that override for this build or use the ordinary "
            "explicit-target package mode."
        )


def cargo_config_paths(*, cwd: Path, cargo_home: Path | None) -> list[Path]:
    candidates = []
    resolved_cwd = cwd.resolve()
    for directory in (resolved_cwd, *resolved_cwd.parents):
        candidates.extend(
            [
                directory / ".cargo" / "config.toml",
                directory / ".cargo" / "config",
            ]
        )

    effective_cargo_home = cargo_home
    if effective_cargo_home is None:
        configured_home = os.environ.get("CARGO_HOME")
        effective_cargo_home = (
            Path(configured_home) if configured_home else Path.home() / ".cargo"
        )
    candidates.extend(
        [effective_cargo_home / "config.toml", effective_cargo_home / "config"]
    )

    unique = []
    seen = set()
    for candidate in candidates:
        resolved = candidate.resolve()
        if resolved not in seen:
            seen.add(resolved)
            unique.append(resolved)
    return unique


def cargo_config_declares_native_output_override(path: Path) -> bool:
    if not path.is_file():
        return False

    in_build_table = False
    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = strip_toml_comment(raw_line).strip()
        if not line:
            continue
        if line.startswith("["):
            in_build_table = False
            if line.endswith("]") and not line.startswith("[["):
                table_name = unquote_toml_key(line[1:-1].strip())
                in_build_table = table_name == "build"
            continue
        if in_build_table and any(
            toml_key_assignment(line, key) for key in ("target", "target-dir")
        ):
            return True
        if any(
            toml_dotted_key_assignment(line, "build", key)
            for key in ("target", "target-dir")
        ):
            return True
        if toml_key_assignment(line, "build"):
            _, value = line.split("=", 1)
            compact = "".join(value.split())
            if compact.startswith("{") and any(
                marker in compact
                for marker in (
                    "target=",
                    '"target"=',
                    "'target'=",
                    "target-dir=",
                    '"target-dir"=',
                    "'target-dir'=",
                )
            ):
                return True
    return False


def strip_toml_comment(line: str) -> str:
    in_single_quote = False
    in_double_quote = False
    escaped = False
    for index, character in enumerate(line):
        if in_double_quote and character == "\\" and not escaped:
            escaped = True
            continue
        if character == '"' and not in_single_quote and not escaped:
            in_double_quote = not in_double_quote
        elif character == "'" and not in_double_quote:
            in_single_quote = not in_single_quote
        elif character == "#" and not in_single_quote and not in_double_quote:
            return line[:index]
        escaped = False
    return line


def toml_key_assignment(line: str, key: str) -> bool:
    left, separator, _ = line.partition("=")
    if not separator:
        return False
    return left.strip() in {key, f'"{key}"', f"'{key}'"}


def toml_dotted_key_assignment(line: str, table: str, key: str) -> bool:
    left, separator, _ = line.partition("=")
    if not separator:
        return False
    components = [unquote_toml_key(component.strip()) for component in left.split(".")]
    return components == [table, key]


def unquote_toml_key(value: str) -> str:
    if len(value) >= 2 and value[0] == value[-1] and value[0] in {'"', "'"}:
        return value[1:-1]
    return value


def source_binaries_for_target(
    spec: TargetSpec,
    variant: PackageVariant,
    *,
    build_entrypoint: bool,
    build_code_mode_host: bool,
    build_bwrap: bool,
    build_codex_command_runner: bool,
    build_codex_windows_sandbox_setup: bool,
) -> list[str]:
    binaries = []
    if build_entrypoint:
        binaries.append(variant.cargo_bin)
    if build_code_mode_host:
        binaries.append("codex-code-mode-host")
    if build_bwrap:
        binaries.append("bwrap")
    if build_codex_command_runner:
        binaries.append("codex-command-runner")
    if build_codex_windows_sandbox_setup:
        binaries.append("codex-windows-sandbox-setup")
    return binaries


def validate_prebuilt_resource_inputs(
    spec: TargetSpec,
    *,
    bwrap_bin: Path | None,
    codex_command_runner_bin: Path | None,
    codex_windows_sandbox_setup_bin: Path | None,
) -> None:
    if bwrap_bin is not None and not spec.is_linux:
        raise RuntimeError("--bwrap-bin is only supported for Linux targets.")
    if codex_command_runner_bin is not None and not spec.is_windows:
        raise RuntimeError(
            "--codex-command-runner-bin is only supported for Windows targets."
        )
    if codex_windows_sandbox_setup_bin is not None and not spec.is_windows:
        raise RuntimeError(
            "--codex-windows-sandbox-setup-bin is only supported for Windows targets."
        )


def resolve_output_path(
    explicit_path: Path | None, default_path: Path | None
) -> Path | None:
    if explicit_path is not None:
        return explicit_path.resolve()

    return default_path


def cargo_profile_output_dir(
    spec: TargetSpec,
    profile: str,
    *,
    cargo_native_host: bool = False,
) -> Path:
    target_dir = cargo_target_dir()
    if cargo_native_host:
        return target_dir / cargo_profile_dirname(profile)
    return target_dir / spec.target / cargo_profile_dirname(profile)


def cargo_target_dir() -> Path:
    target_dir = os.environ.get("CARGO_TARGET_DIR")
    if target_dir is None:
        return CODEX_RS_ROOT / "target"

    path = Path(target_dir)
    if path.is_absolute():
        return path

    return CODEX_RS_ROOT / path


def cargo_profile_dirname(profile: str) -> str:
    if profile == "dev":
        return "debug"
    if profile == "release":
        return "release"
    return profile


def validate_source_outputs(outputs: SourceBuildOutputs) -> None:
    for path in [
        outputs.entrypoint_bin,
        outputs.code_mode_host_bin,
        outputs.bwrap_bin,
        outputs.codex_command_runner_bin,
        outputs.codex_windows_sandbox_setup_bin,
    ]:
        if path is not None and not path.is_file():
            raise RuntimeError(f"cargo build did not produce expected binary: {path}")
