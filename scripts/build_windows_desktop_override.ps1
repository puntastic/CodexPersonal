<#
.SYNOPSIS
Builds a canonical Windows package for the Codex Desktop CLI override.

.DESCRIPTION
Builds the four native release binaries together under codex-rs/target/release
so an existing native Cargo cache is reused. It then provides those binaries to
scripts/build_codex_package.py, which owns the canonical layout, manifest,
package validation, and refusal to replace a non-empty output directory.

.PARAMETER OutputDirectory
New package output directory. The wrapper never passes the builder's --force
option.

.PARAMETER CargoPath
Explicit path to the Cargo executable for the active Rust/MSVC environment.

.PARAMETER RipgrepPath
Optional explicit path to a native rg.exe. When omitted, the active rg.exe is
accepted only when it comes from the Codex Desktop installation under
%LOCALAPPDATA%\OpenAI\Codex\bin.

.PARAMETER PythonPath
Python executable or path used to run the canonical package builder.

.EXAMPLE
pwsh ./scripts/build_windows_desktop_override.ps1 `
    -OutputDirectory ./dist/codex-desktop-override `
    -CargoPath ./path/to/cargo.exe
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$OutputDirectory,

    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$CargoPath,

    [string]$RipgrepPath,

    [ValidateNotNullOrEmpty()]
    [string]$PythonPath = "py.exe"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

function Resolve-RequiredFile {
    param(
        [string]$Path,
        [string]$Description
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "$Description is not a file: $Path"
    }

    return (Resolve-Path -LiteralPath $Path).Path
}

function Resolve-ApplicationPath {
    param(
        [string]$Command,
        [string]$Description
    )

    if (Test-Path -LiteralPath $Command) {
        return Resolve-RequiredFile -Path $Command -Description $Description
    }

    $application = Get-Command -Name $Command -CommandType Application -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if ($null -eq $application -or [string]::IsNullOrWhiteSpace($application.Source)) {
        throw "Could not find $Description application '$Command'."
    }

    return $application.Source
}

function Get-CargoHostTarget {
    param(
        [string]$CargoExecutable
    )

    $versionLines = @(& $CargoExecutable -vV)
    if ($LASTEXITCODE -ne 0) {
        throw "Cargo version query failed with exit code $LASTEXITCODE."
    }

    $hostLine = $versionLines | Where-Object { $_ -match "^host:\s*(.+)$" } | Select-Object -First 1
    $hostMatch = [regex]::Match([string]$hostLine, "^host:\s*(.+)$")
    if (-not $hostMatch.Success) {
        throw "Cargo -vV did not report a host target."
    }

    $target = $hostMatch.Groups[1].Value.Trim()
    if ($target -notin @("x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc")) {
        throw "Cargo host '$target' is not a supported native Windows MSVC target."
    }

    return $target
}

function Get-WindowsExecutableTarget {
    param(
        [string]$Path
    )

    $stream = [System.IO.File]::OpenRead($Path)
    $reader = [System.IO.BinaryReader]::new($stream)
    try {
        if ($stream.Length -lt 64 -or $reader.ReadUInt16() -ne 0x5A4D) {
            throw "Executable does not have a valid DOS header: $Path"
        }

        $stream.Position = 0x3C
        $peOffset = $reader.ReadUInt32()
        if ($peOffset -gt $stream.Length - 6) {
            throw "Executable has an invalid PE header offset: $Path"
        }

        $stream.Position = $peOffset
        if ($reader.ReadUInt32() -ne 0x00004550) {
            throw "Executable does not have a valid PE header: $Path"
        }

        $machine = $reader.ReadUInt16()
        if ($machine -eq 0x8664) {
            return "x86_64-pc-windows-msvc"
        }
        if ($machine -eq 0xAA64) {
            return "aarch64-pc-windows-msvc"
        }

        throw "Executable uses unsupported PE machine 0x$($machine.ToString('X4')): $Path"
    } finally {
        $reader.Dispose()
        $stream.Dispose()
    }
}

function Resolve-CodexV8Environment {
    param(
        [string]$PythonExecutable,
        [string]$RepositoryRoot,
        [string]$Target
    )

    $resolver = "import json, sys; from scripts.codex_package.targets import TARGET_SPECS; from scripts.codex_package.v8 import resolve_codex_v8_cargo_env; print(json.dumps(resolve_codex_v8_cargo_env(TARGET_SPECS[sys.argv[1]])))"
    $hadRepoRoot = Test-Path Env:CODEX_REPO_ROOT
    $previousRepoRoot = $env:CODEX_REPO_ROOT
    $locationPushed = $false
    try {
        $env:CODEX_REPO_ROOT = $RepositoryRoot
        Push-Location $RepositoryRoot
        $locationPushed = $true
        $json = @(& $PythonExecutable -c $resolver $Target)
        if ($LASTEXITCODE -ne 0) {
            throw "Codex V8 artifact resolution failed with exit code $LASTEXITCODE."
        }
        return (($json -join [Environment]::NewLine) | ConvertFrom-Json)
    } finally {
        if ($locationPushed) {
            Pop-Location
        }
        if ($hadRepoRoot) {
            $env:CODEX_REPO_ROOT = $previousRepoRoot
        } else {
            Remove-Item Env:CODEX_REPO_ROOT -ErrorAction SilentlyContinue
        }
    }
}

function Resolve-InstalledCodexRipgrep {
    if ([string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
        throw "LOCALAPPDATA is unavailable; pass -RipgrepPath explicitly."
    }

    $application = Get-Command -Name "rg.exe" -CommandType Application -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if ($null -eq $application -or [string]::IsNullOrWhiteSpace($application.Source)) {
        throw "Could not find the installed Codex rg.exe; pass -RipgrepPath explicitly."
    }

    $candidate = [System.IO.Path]::GetFullPath($application.Source)
    $codexBinRoot = [System.IO.Path]::GetFullPath(
        (Join-Path $env:LOCALAPPDATA "OpenAI\Codex\bin")
    )
    $separator = [System.IO.Path]::DirectorySeparatorChar.ToString()
    $codexBinPrefix = if ($codexBinRoot.EndsWith($separator)) {
        $codexBinRoot
    } else {
        "$codexBinRoot$separator"
    }
    if (-not $candidate.StartsWith(
        $codexBinPrefix,
        [System.StringComparison]::OrdinalIgnoreCase
    )) {
        throw "Active rg.exe is not from the Codex installation: $candidate. Pass -RipgrepPath explicitly."
    }

    return Resolve-RequiredFile -Path $candidate -Description "installed Codex ripgrep"
}

if ([System.Environment]::OSVersion.Platform -ne [System.PlatformID]::Win32NT) {
    throw "This Windows Desktop package wrapper must run on Windows."
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$cargoWorkspaceManifest = Join-Path $repoRoot "codex-rs\Cargo.toml"
$nativeTargetDirectory = Join-Path $repoRoot "codex-rs\target"
$releaseDirectory = Join-Path $nativeTargetDirectory "release"
$outputFullPath = [System.IO.Path]::GetFullPath($OutputDirectory)
if (Test-Path -LiteralPath $outputFullPath) {
    throw "Output already exists; refusing to build or replace it: $outputFullPath"
}
$builderResolution = @{
    Path = Join-Path $PSScriptRoot "build_codex_package.py"
    Description = "canonical package builder"
}
$builderPath = Resolve-RequiredFile @builderResolution
$cargoExecutable = Resolve-RequiredFile -Path $CargoPath -Description "Cargo executable"
$pythonExecutable = Resolve-ApplicationPath -Command $PythonPath -Description "Python"
$target = Get-CargoHostTarget -CargoExecutable $cargoExecutable
$v8Environment = Resolve-CodexV8Environment `
    -PythonExecutable $pythonExecutable `
    -RepositoryRoot $repoRoot `
    -Target $target
$rgExecutable = if ([string]::IsNullOrWhiteSpace($RipgrepPath)) {
    Resolve-InstalledCodexRipgrep
} else {
    Resolve-RequiredFile -Path $RipgrepPath -Description "ripgrep executable"
}
$rgTarget = Get-WindowsExecutableTarget -Path $rgExecutable
if ($rgTarget -ne $target) {
    throw "Ripgrep target '$rgTarget' does not match Cargo target '$target': $rgExecutable"
}

$null = & $rgExecutable --version
if ($LASTEXITCODE -ne 0) {
    throw "Ripgrep validation failed with exit code $LASTEXITCODE."
}

$nativeBuildArguments = @(
    "build",
    "--manifest-path", $cargoWorkspaceManifest,
    "--target-dir", $nativeTargetDirectory,
    "--release",
    "--locked",
    "--bin", "codex",
    "--bin", "codex-code-mode-host",
    "--bin", "codex-command-runner",
    "--bin", "codex-windows-sandbox-setup"
)

Write-Host "==> Building native Windows release binaries in $releaseDirectory"
$hadCargoBuildTarget = Test-Path Env:CARGO_BUILD_TARGET
$previousCargoBuildTarget = $env:CARGO_BUILD_TARGET
$hadPython = Test-Path Env:PYTHON
$previousPython = $env:PYTHON
$hadRustyV8Archive = Test-Path Env:RUSTY_V8_ARCHIVE
$previousRustyV8Archive = $env:RUSTY_V8_ARCHIVE
$hadRustyV8Binding = Test-Path Env:RUSTY_V8_SRC_BINDING_PATH
$previousRustyV8Binding = $env:RUSTY_V8_SRC_BINDING_PATH
$nativeBuildLocationPushed = $false
try {
    Remove-Item Env:CARGO_BUILD_TARGET -ErrorAction SilentlyContinue
    $env:PYTHON = $pythonExecutable
    if ($null -ne $v8Environment.PSObject.Properties["RUSTY_V8_ARCHIVE"]) {
        $env:RUSTY_V8_ARCHIVE = [string]$v8Environment.RUSTY_V8_ARCHIVE
        $env:RUSTY_V8_SRC_BINDING_PATH = [string]$v8Environment.RUSTY_V8_SRC_BINDING_PATH
    }
    Push-Location $repoRoot
    $nativeBuildLocationPushed = $true
    & $cargoExecutable @nativeBuildArguments
    if ($LASTEXITCODE -ne 0) {
        throw "Native Cargo release build failed with exit code $LASTEXITCODE."
    }
} finally {
    if ($nativeBuildLocationPushed) {
        Pop-Location
    }
    if ($hadCargoBuildTarget) {
        $env:CARGO_BUILD_TARGET = $previousCargoBuildTarget
    } else {
        Remove-Item Env:CARGO_BUILD_TARGET -ErrorAction SilentlyContinue
    }
    if ($hadPython) {
        $env:PYTHON = $previousPython
    } else {
        Remove-Item Env:PYTHON -ErrorAction SilentlyContinue
    }
    if ($hadRustyV8Archive) {
        $env:RUSTY_V8_ARCHIVE = $previousRustyV8Archive
    } else {
        Remove-Item Env:RUSTY_V8_ARCHIVE -ErrorAction SilentlyContinue
    }
    if ($hadRustyV8Binding) {
        $env:RUSTY_V8_SRC_BINDING_PATH = $previousRustyV8Binding
    } else {
        Remove-Item Env:RUSTY_V8_SRC_BINDING_PATH -ErrorAction SilentlyContinue
    }
}

$builderArguments = @(
    $builderPath,
    "--variant", "codex",
    "--target", $target,
    "--cargo-profile", "release",
    "--cargo", $cargoExecutable,
    "--rg-bin", $rgExecutable,
    "--entrypoint-bin", (Join-Path $releaseDirectory "codex.exe"),
    "--code-mode-host-bin", (Join-Path $releaseDirectory "codex-code-mode-host.exe"),
    "--codex-command-runner-bin", (Join-Path $releaseDirectory "codex-command-runner.exe"),
    "--codex-windows-sandbox-setup-bin", (Join-Path $releaseDirectory "codex-windows-sandbox-setup.exe"),
    "--package-dir", $outputFullPath
)

$hadRepoRoot = Test-Path Env:CODEX_REPO_ROOT
$previousRepoRoot = $env:CODEX_REPO_ROOT
$builderLocationPushed = $false
try {
    $env:CODEX_REPO_ROOT = $repoRoot
    Write-Host "==> Staging canonical Codex Desktop override package for $target"
    Push-Location $repoRoot
    $builderLocationPushed = $true
    & $pythonExecutable @builderArguments
    if ($LASTEXITCODE -ne 0) {
        throw "Canonical package builder failed with exit code $LASTEXITCODE."
    }
} finally {
    if ($builderLocationPushed) {
        Pop-Location
    }
    if ($hadRepoRoot) {
        $env:CODEX_REPO_ROOT = $previousRepoRoot
    } else {
        Remove-Item Env:CODEX_REPO_ROOT -ErrorAction SilentlyContinue
    }
}

Write-Host "==> Desktop override package is ready: $outputFullPath"
