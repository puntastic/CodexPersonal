<#
.SYNOPSIS
Compatibility entrypoint for building a Windows Desktop override package.

.DESCRIPTION
The canonical operator surface is now codex-dev.ps1. This shim preserves the
previous command path while routing toolchain discovery, native-host packaging,
validation, provenance receipts, and output handling through the shared lane.

.PARAMETER OutputDirectory
Fresh package output directory.

.PARAMETER CargoPath
Optional Cargo executable. The development lane discovers the configured local
toolchain when this is omitted.

.PARAMETER RipgrepPath
Optional native ripgrep executable.

.PARAMETER PythonPath
Optional Python 3 executable.

.PARAMETER CargoProfile
Cargo profile for the source build. Defaults to release for compatibility with
the previous wrapper.
#>

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$OutputDirectory,

    [string]$CargoPath,
    [string]$RipgrepPath,
    [string]$PythonPath,

    [ValidateSet("dev-small", "release")]
    [string]$CargoProfile = "release"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$entrypoint = Join-Path $repoRoot "codex-dev.ps1"
$parameters = @{
    Action = "Build"
    PackageDirectory = $OutputDirectory
    CargoProfile = $CargoProfile
}
foreach ($entry in @{
    CargoPath = $CargoPath
    RipgrepPath = $RipgrepPath
    PythonPath = $PythonPath
}.GetEnumerator()) {
    if (-not [string]::IsNullOrWhiteSpace([string]$entry.Value)) {
        $parameters[$entry.Key] = $entry.Value
    }
}

& $entrypoint @parameters
