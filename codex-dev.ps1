<#
.SYNOPSIS
Runs the personal-fork Windows Desktop development lane.

.DESCRIPTION
This is the stable operator entrypoint for local Codex CLI/Desktop work. It
discovers the existing Rust/MSVC/Python environment, delegates builds to the
canonical package builder, keeps package activation reversible, and reports
source, staged, configured, and live state separately.

.EXAMPLE
.\codex-dev.ps1 -Action Doctor

.EXAMPLE
.\codex-dev.ps1 -Action Setup

.EXAMPLE
.\codex-dev.ps1 -Action Just -JustArguments @("test", "-p", "codex-state")

.EXAMPLE
.\codex-dev.ps1 -Action Build -CargoProfile dev-small

.EXAMPLE
.\codex-dev.ps1 -Action Deploy -PackageDirectory .\codex-rs\target\desktop-dev\packages\example

.EXAMPLE
.\codex-dev.ps1 -Action Verify

.EXAMPLE
.\codex-dev.ps1 -Action Rollback -WhatIf
#>

[CmdletBinding()]
param(
    [Parameter(Position = 0)]
    [ValidateSet("Help", "Doctor", "Setup", "Just", "Build", "Deploy", "Verify", "Rollback", "SelfTest")]
    [string]$Action = "Help",

    [ValidateSet("dev-small", "release")]
    [string]$CargoProfile = "dev-small",

    [string]$PackageDirectory,
    [string[]]$JustArguments = @(),
    [string]$CargoPath,
    [string]$CargoHome,
    [string]$RustupHome,
    [string]$PythonPath,
    [string]$RipgrepPath,
    [string]$ConfigPath,
    [string]$DeploymentRoot,
    [switch]$WhatIf,
    [switch]$Json
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$workflow = Join-Path $PSScriptRoot "scripts\windows_desktop_dev\workflow.ps1"
if (-not (Test-Path -LiteralPath $workflow -PathType Leaf)) {
    throw "Desktop development workflow is missing: $workflow"
}
. $workflow

$parameters = @{
    Action = $Action
    CargoProfile = $CargoProfile
    JustArguments = $JustArguments
    WhatIf = $WhatIf
    Json = $Json
}
foreach ($entry in @{
    PackageDirectory = $PackageDirectory
    CargoPath = $CargoPath
    CargoHome = $CargoHome
    RustupHome = $RustupHome
    PythonPath = $PythonPath
    RipgrepPath = $RipgrepPath
    ConfigPath = $ConfigPath
    DeploymentRoot = $DeploymentRoot
}.GetEnumerator()) {
    if (-not [string]::IsNullOrWhiteSpace([string]$entry.Value)) {
        $parameters[$entry.Key] = $entry.Value
    }
}

Invoke-CodexDesktopDev @parameters
