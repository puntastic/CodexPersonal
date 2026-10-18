Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

. (Join-Path $PSScriptRoot "common.ps1")

function Assert-GitStateTest {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw $Message }
}

function Reset-GitStateFixture {
    $script:FixtureHead = "0123456789abcdef0123456789abcdef01234567"
    $script:FixtureHeadAfter = $script:FixtureHead
    $script:HeadReads = 0
    $script:BranchRows = @()
    $script:StatusRows = @()
    $script:FailureCommand = $null
    $script:EmptyHead = $false
    $script:EmptyShortHead = $false
}

# Zero emitted rows, not a literal $null, reproduce the native capture's
# detached-branch result. All Git commands are mocked; no Git or filesystem
# mutation, subprocess, fixture directory, or user configuration is used.
function Invoke-CodexDevGit {
    param([string[]]$ArgumentList)
    $command = $ArgumentList -join " "
    if ($command -eq $script:FailureCommand) { throw "mock git failure: $command" }
    switch ($command) {
        "rev-parse HEAD" {
            $script:HeadReads++
            if ($script:EmptyHead) { return @() }
            $value = if ($script:HeadReads -eq 1) { $script:FixtureHead } else { $script:FixtureHeadAfter }
            return " $value "
        }
        "rev-parse --short=12 HEAD" {
            if ($script:EmptyShortHead) { return @() }
            return " $($script:FixtureHead.Substring(0, 12)) "
        }
        "branch --show-current" { return $script:BranchRows }
        "status --porcelain=v1 --untracked-files=normal" { return $script:StatusRows }
        "ls-files --others --exclude-standard -z --" { return @() }
        default {
            if ($command -eq "diff --name-only --no-renames -z $script:FixtureHead --" -or
                $command -eq "diff --raw --no-renames -z $script:FixtureHead --") {
                return @()
            }
            throw "Unexpected mocked Git command: $command"
        }
    }
}

function Assert-GitStateFailure {
    param([string]$ExpectedMessage)
    $caught = $null
    try { $null = Get-CodexDevGitState } catch { $caught = $_ }
    Assert-GitStateTest ($null -ne $caught) "Invalid Git state was accepted."
    if (-not [string]::IsNullOrEmpty($ExpectedMessage)) {
        Assert-GitStateTest ($caught.Exception.Message.Contains($ExpectedMessage)) "Git failure was swallowed or replaced."
    }
}

Reset-GitStateFixture
$originalFailure = $null
try {
    $null = ([string](Invoke-CodexDevGit @("branch", "--show-current") | Select-Object -First 1)).Trim()
} catch { $originalFailure = $_ }
Assert-GitStateTest ($null -ne $originalFailure -and $originalFailure.FullyQualifiedErrorId -eq "InvokeMethodOnNull") "Original zero-output cast/Trim failure was not reproduced."
Assert-GitStateTest (([string]$null).Trim() -eq "") "Literal null control no longer distinguishes the pipeline shape."

$detached = Get-CodexDevGitState
Assert-GitStateTest ($detached.Branch -is [string] -and $detached.Branch -ceq "") "Detached HEAD did not produce an empty branch string."
Assert-GitStateTest ($detached.Head -ceq $script:FixtureHead -and $detached.ShortHead -ceq $script:FixtureHead.Substring(0, 12)) "HEAD fields changed or were not trimmed."
Assert-GitStateTest (-not $detached.Dirty -and $detached.Status.Count -eq 0 -and $script:HeadReads -eq 2) "Clean detached state or final HEAD read changed."
Assert-GitStateTest ($detached.GitVisibleSourceFingerprint -match '^[0-9a-f]{64}$') "Detached baseline lost its source fingerprint."

Reset-GitStateFixture
$script:BranchRows = @("  codex/named-fixture  ")
$named = Get-CodexDevGitState
Assert-GitStateTest ($named.Branch -ceq "codex/named-fixture" -and $named.Head -ceq $detached.Head) "Named branch behavior changed."
Assert-GitStateTest ($named.GitVisibleSourceFingerprint -ceq $detached.GitVisibleSourceFingerprint) "Branch normalization changed identical source fingerprint inputs."

Reset-GitStateFixture
$script:StatusRows = @(" M tracked-file.txt", "?? fixture-untracked.txt")
$dirty = Get-CodexDevGitState
Assert-GitStateTest ($dirty.Dirty -and $dirty.Status.Count -eq 2 -and $dirty.Status[0] -ceq " M tracked-file.txt") "Porcelain dirty-state rows were changed."

foreach ($command in @("rev-parse HEAD", "rev-parse --short=12 HEAD", "branch --show-current", "status --porcelain=v1 --untracked-files=normal")) {
    Reset-GitStateFixture
    $script:FailureCommand = $command
    Assert-GitStateFailure "mock git failure: $command"
}
Reset-GitStateFixture
$script:FailureCommand = "diff --name-only --no-renames -z $script:FixtureHead --"
Assert-GitStateFailure "mock git failure: $script:FailureCommand"

Reset-GitStateFixture
$script:EmptyHead = $true
Assert-GitStateFailure ""
Reset-GitStateFixture
$script:EmptyShortHead = $true
Assert-GitStateFailure ""

Reset-GitStateFixture
$script:FixtureHeadAfter = "fedcba9876543210fedcba9876543210fedcba98"
Assert-GitStateFailure "Git HEAD changed while source state was being captured:"
Assert-GitStateTest ($script:HeadReads -eq 2) "Source-stability guard did not perform both HEAD reads."

Write-Output "Git-state checks passed; original zero-output failure reproduced on PowerShell $($PSVersionTable.PSVersion). All Git calls mocked."
