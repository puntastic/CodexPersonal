Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$WarningPreference = "Stop"

. (Join-Path $PSScriptRoot "package.ps1")

function Assert-SmokeCleanupTest {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw $Message }
}

# Every filesystem mutation and native call used below is mocked. These paths
# are string fixtures only: no directories, binaries, or recursive deletes run.
$script:RemoveCalls = @()
$script:RemovalFailure = $false
$script:SmokeFailure = $false
$script:ResolvedHomeOverride = $null
$script:ReparsePath = $null
$script:CodexDevRepositoryRoot = "C:\mock-smoke-cleanup\repo"
$script:CodexDevExpectedPackageFiles = @("codex-package.json")
$script:PackageRoot = "C:\mock-smoke-cleanup\package"

function Get-Item {
    [CmdletBinding()]
    param([string]$LiteralPath)
    $fullName = if ($null -ne $script:ResolvedHomeOverride -and $LiteralPath -eq $script:SmokeHomeFixture) {
        $script:ResolvedHomeOverride
    } else { $LiteralPath }
    [pscustomobject]@{
        FullName = $fullName
        PSIsContainer = $true
        Attributes = if ($LiteralPath -eq $script:ReparsePath) {
            [System.IO.FileAttributes]::Directory -bor [System.IO.FileAttributes]::ReparsePoint
        } else { [System.IO.FileAttributes]::Directory }
    }
}

function Remove-Item {
    [CmdletBinding()]
    param(
        [Parameter(Position = 0)][string]$Path,
        [string]$LiteralPath,
        [switch]$Recurse,
        [switch]$Force
    )
    if (-not [string]::IsNullOrEmpty($Path)) { throw "Unexpected nonliteral removal: $Path" }
    $script:RemoveCalls += @{
        LiteralPath = $LiteralPath
        Recurse = [bool]$Recurse
        ForceBound = $PSBoundParameters.ContainsKey("Force")
        ErrorAction = [string]$PSBoundParameters["ErrorAction"]
    }
    if ($script:RemovalFailure) { throw "mock cleanup access denied" }
}

function Resolve-Path {
    [CmdletBinding()]
    param([string]$LiteralPath)
    [pscustomobject]@{ Path = $script:PackageRoot }
}
function Test-Path {
    param([string]$LiteralPath, [string]$PathType)
    return -not $LiteralPath.EndsWith("codex-dev-build.json")
}
function Get-Content {
    param([string]$LiteralPath, [switch]$Raw)
    '{"target":"x86_64-pc-windows-msvc","variant":"codex","version":"mock-version"}'
}
function Get-ChildItem {
    param([string]$LiteralPath, [switch]$Recurse, [switch]$File)
    @()
}
function New-Item {
    param([string]$ItemType, [string]$Path, [switch]$Force)
    [pscustomobject]@{ FullName = $Path }
}
function Get-CodexDevHostTarget { "x86_64-pc-windows-msvc" }
function Assert-CodexDevPackageExecutableTargets {
    param([string]$PackageRoot, [string]$ExpectedTarget)
}
function Get-CodexDevSha256Text { param([string]$Text) "mock-fingerprint" }
function Invoke-CodexDevNative {
    param([string]$FilePath, [string[]]$ArgumentList, [switch]$Capture)
    Assert-SmokeCleanupTest ($FilePath -eq (Join-Path $script:PackageRoot "bin\codex.exe")) "Smoke executable changed."
    Assert-SmokeCleanupTest ($ArgumentList.Count -eq 1 -and $ArgumentList[0] -eq "--version" -and $Capture) "Smoke arguments changed."
    Assert-SmokeCleanupTest ($env:CODEX_HOME -eq $env:CODEX_SQLITE_HOME) "Smoke homes differ."
    Assert-SmokeCleanupTest ($env:CODEX_HOME -match '\\smoke\\[0-9a-f]{32}$') "Smoke home is not a fresh GUID child."
    if ($script:SmokeFailure) { throw "original mock smoke failure" }
    "codex mock-version"
}

$base = "C:\mock-smoke-cleanup\owned-smoke"
$script:SmokeHomeFixture = Join-Path $base "0123456789abcdef0123456789abcdef"
$normalOutput = @(Remove-CodexDevSmokeHome -SmokeBase $base -SmokeHome $script:SmokeHomeFixture)
Assert-SmokeCleanupTest ($normalOutput.Count -eq 0 -and $script:RemoveCalls.Count -eq 1) "Normal cleanup changed the return stream or did not remove once."
$call = $script:RemoveCalls[0]
Assert-SmokeCleanupTest ($call.LiteralPath -ceq $script:SmokeHomeFixture -and $call.Recurse -and -not $call.ForceBound -and $call.ErrorAction -eq "Stop") "Cleanup was not exact-literal, recursive, non-forcing and checked."

foreach ($refused in @(
    $base,
    "$base-sibling\child",
    (Join-Path $base "..\outside"),
    (Join-Path $base "junction\0123456789abcdef0123456789abcdef"),
    (Join-Path $base "not-a-generated-smoke-home")
)) {
    $script:RemoveCalls = @()
    $warnings = @(& { Remove-CodexDevSmokeHome -SmokeBase $base -SmokeHome $refused } 3>&1)
    Assert-SmokeCleanupTest ($script:RemoveCalls.Count -eq 0 -and $warnings.Count -eq 1 -and $warnings[0] -is [System.Management.Automation.WarningRecord]) "Unsafe target was not refused with a nonthrowing warning."
}
$script:ResolvedHomeOverride = "$base-sibling\resolved"
$warnings = @(& { Remove-CodexDevSmokeHome -SmokeBase $base -SmokeHome $script:SmokeHomeFixture } 3>&1)
Assert-SmokeCleanupTest ($script:RemoveCalls.Count -eq 0 -and $warnings.Count -eq 1) "Resolved escape was not refused."
$script:ResolvedHomeOverride = $null
$script:ReparsePath = $script:SmokeHomeFixture
$warnings = @(& { Remove-CodexDevSmokeHome -SmokeBase $base -SmokeHome $script:SmokeHomeFixture } 3>&1)
Assert-SmokeCleanupTest ($script:RemoveCalls.Count -eq 0 -and $warnings.Count -eq 1) "Reparse target was not refused."
$script:ReparsePath = $null
$script:RemovalFailure = $true
$warnings = @(& { Remove-CodexDevSmokeHome -SmokeBase $base -SmokeHome $script:SmokeHomeFixture } 3>&1)
Assert-SmokeCleanupTest ($warnings.Count -eq 1 -and [string]$warnings[0] -like '*mock cleanup access denied*') "Failed cleanup was not visibly reported."

$oldCodexHome = $env:CODEX_HOME
$oldSqliteHome = $env:CODEX_SQLITE_HOME
try {
    $env:CODEX_HOME = "original-codex-home"
    $env:CODEX_SQLITE_HOME = "original-sqlite-home"
    $script:SmokeFailure = $true
    $caught = $null
    try { Get-CodexDevPackageInfo -PackageDirectory $script:PackageRoot | Out-Null } catch { $caught = $_.Exception.Message }
    Assert-SmokeCleanupTest ($caught -eq "original mock smoke failure") "Cleanup masked the original smoke exception."
    Assert-SmokeCleanupTest ($env:CODEX_HOME -eq "original-codex-home" -and $env:CODEX_SQLITE_HOME -eq "original-sqlite-home") "Smoke failure did not restore the environment."
    $script:SmokeFailure = $false
    $result = Get-CodexDevPackageInfo -PackageDirectory $script:PackageRoot
    Assert-SmokeCleanupTest ($result.VersionOutput -eq "codex mock-version" -and $result.Fingerprint -eq "mock-fingerprint") "Cleanup failure changed the package return contract."
    Assert-SmokeCleanupTest ($env:CODEX_HOME -eq "original-codex-home" -and $env:CODEX_SQLITE_HOME -eq "original-sqlite-home") "Smoke success did not restore the environment."
} finally {
    [Environment]::SetEnvironmentVariable("CODEX_HOME", $oldCodexHome, "Process")
    [Environment]::SetEnvironmentVariable("CODEX_SQLITE_HOME", $oldSqliteHome, "Process")
}

Write-Output "Smoke cleanup checks passed (mocked filesystem and native execution only)."
