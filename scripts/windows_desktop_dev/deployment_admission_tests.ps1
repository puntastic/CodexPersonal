# Exercise the real admission and Deploy/Rollback control flow. External
# package/probe/selector/lock/transaction boundaries are deliberate doubles;
# only generated package, ledger and backup fixtures may touch the filesystem.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'workflow.ps1')

$script:Checks = 0
function Assert-AdmissionEqual {
    param($Expected, $Actual, [string]$Label)
    $script:Checks++
    if ($Expected -cne $Actual) { throw "$Label failed: expected <$Expected>, actual <$Actual>." }
}
function Assert-AdmissionRefused {
    param([scriptblock]$Action, [string]$Pattern)
    $script:Checks++
    try { $null = & $Action } catch {
        if ($_.Exception.Message -match $Pattern) { return }
        throw "Unexpected refusal: $($_.Exception.Message)"
    }
    throw "Expected refusal matching $Pattern."
}

$testRoot = Join-Path ([IO.Path]::GetTempPath()) ('codex-admission-tests-' + [guid]::NewGuid().ToString('N'))
$null = [IO.Directory]::CreateDirectory($testRoot)
$script:Root = $testRoot
$script:Events = [Collections.Generic.List[string]]::new()
$script:ChecksMode = 'qualified'
$script:SourcePackage = $null
$script:Old = $null
$script:Previous = $null
$script:State = $null
$script:Status = $null
$script:Persistent = $null
$script:ConfigSha = 'config-before'
$script:ConfigPath = $null
$script:DeploymentRoot = $null
$script:RepairCalls = 0
$script:TransactionCalls = 0
$script:ProbeCalls = 0
$script:BindingCalls = 0
$script:PackageCalls = 0
$script:FailFingerprintOnSecond = $false
$script:RefreshPlanUnderLock = $false
$script:RecoveryReturn = $null
$script:BindingPaths = [Collections.Generic.List[string]]::new()

function New-AdmissionPackage {
    param([string]$Path, [string]$Fingerprint)
    $null = [IO.Directory]::CreateDirectory((Join-Path $Path 'bin'))
    [IO.File]::WriteAllText((Join-Path $Path 'bin\codex.exe'), $Fingerprint)
    return [pscustomobject]@{
        ReleasePath = $Path; PackageRoot = $Path; Fingerprint = $Fingerprint
        Entrypoint = Join-Path $Path 'bin\codex.exe'; ReleaseId = "sha256-$Fingerprint"
    }
}
function Get-CodexDevPackageInfo {
    param([string]$PackageDirectory, [switch]$SkipSmoke)
    if (-not $SkipSmoke) { throw 'Unexpected executable smoke request.' }
    $script:PackageCalls++
    $fingerprint = [IO.File]::ReadAllText((Join-Path $PackageDirectory 'bin\codex.exe'))
    if ($script:FailFingerprintOnSecond -and $script:PackageCalls -eq 2) { $fingerprint = 'd' * 64 }
    return [pscustomobject]@{
        PackageRoot = $PackageDirectory; Entrypoint = Join-Path $PackageDirectory 'bin\codex.exe'
        Fingerprint = $fingerprint; Version = '0.999.0'; Provenance = $null; ProvenanceStatus = 'unavailable'
    }
}
function Read-CodexCliConfigSnapshot {
    param([string]$ConfigPath)
    if ($ConfigPath -cne $script:ConfigPath) { throw 'Unexpected config read.' }
    return [pscustomobject]@{ Exists = $true; Sha256 = $script:ConfigSha; Error = $null; ConfiguredEntrypoint = $script:Old.Entrypoint }
}
function Read-CodexDevPersistentSelector {
    return [pscustomobject]@{ Value = $script:Persistent; Error = $null; Scope = 'User-test-double' }
}
function Set-CodexDevPersistentSelector { throw 'No real selector mutation is allowed in admission tests.' }
function Set-CodexCliPathInConfig { throw 'No config mutation is allowed in admission tests.' }
function Get-CodexDevDeploymentStatus {
    param($ConfigPath, $DeploymentRoot)
    return $script:Status
}
function Get-CodexDevDeploymentStatusUnlocked {
    param($ConfigPath, $DeploymentRoot)
    return $script:Status
}
function Invoke-WithCodexDevDeploymentLock {
    param($DeploymentRoot, [scriptblock]$Body)
    if ($script:RefreshPlanUnderLock) {
        $script:RefreshPlanUnderLock = $false
        $script:State.Previous = $script:SourcePackage
        [IO.File]::WriteAllText((Get-CodexDevDeploymentPaths $script:DeploymentRoot).State, ($script:State | ConvertTo-Json -Depth 12))
    }
    & $Body
}
function Invoke-WithCodexDevConfigLock { param($ConfigPath, [scriptblock]$Body) & $Body }
function Invoke-CodexDevCompatibilityCheck {
    param([string]$PackageDirectory, [string]$PythonPath)
    $script:ProbeCalls++
    $script:Events.Add('probe')
    switch ($script:ChecksMode) {
        'config-drift' { $script:ConfigSha = 'changed-during-probe' }
        'selector-drift' { $script:Persistent = 'C:\different-reader.exe' }
        'state-drift' { [IO.File]::AppendAllText((Get-CodexDevDeploymentPaths $script:DeploymentRoot).State, ' ') }
        'pending-drift' { [IO.File]::WriteAllText((Get-CodexDevDeploymentPaths $script:DeploymentRoot).Pending, '{}') }
    }
    $compatible = $script:ChecksMode -notin @('unknown', 'failed') -and
        -not ($script:ChecksMode -eq 'recovery-only-unknown' -and $PackageDirectory -ceq $script:Old.ReleasePath)
    return [pscustomobject]@{
        Compatible = $compatible
        Status = if ($compatible) { 'qualified' } elseif ($script:ChecksMode -eq 'unknown') { 'unqualified' } else { 'failed' }
        Blockers = if ($compatible) { @() } else { @('Synthetic compatibility refusal') }
        ReportPath = 'synthetic-only'; Fingerprint = [IO.File]::ReadAllText((Join-Path $PackageDirectory 'bin\codex.exe'))
    }
}
function Assert-CodexDevCompatibilityBinding {
    param($Check, [string]$PackageDirectory)
    $script:BindingCalls++
    $script:BindingPaths.Add($PackageDirectory)
    $script:Events.Add('binding')
    if ($Check -is [array] -or -not $Check.Compatible) { throw 'Compatibility check is not one qualified object.' }
    if ([IO.File]::ReadAllText((Join-Path $PackageDirectory 'bin\codex.exe')) -cne $Check.Fingerprint) {
        throw 'Synthetic stale package binding.'
    }
    return $true # Production returns true; callers must suppress it.
}
function Repair-CodexDevInterruptedTransaction {
    param($ConfigPath, $DeploymentRoot)
    $script:Events.Add('repair')
    $script:RepairCalls++
    $script:Status.PendingExists = $false
    $script:Status.PendingDisposition = 'none'
    $script:Status.Status = 'consistent'
    return $script:RecoveryReturn
}
function Get-CodexCliConfigTransition {
    param([string]$ConfigPath, [string]$Entrypoint)
    $script:Events.Add('transition')
    return [pscustomobject]@{
        ConfigPath = $ConfigPath; ConfiguredBefore = $script:Old.Entrypoint; ConfiguredAfter = $Entrypoint
        BeforeSha256 = $script:ConfigSha; AfterSha256 = 'config-after'; BeforeBytes = [Text.Encoding]::UTF8.GetBytes('fixture before-image')
    }
}
function Invoke-CodexDevDeploymentTransaction {
    param($Action, $ConfigPath, $DeploymentRoot, $ConfiguredBefore, $ConfiguredAfter,
        $PersistentBefore, $PersistentAfter, $ConfigTransition, $ConfigBackup,
        $StateBeforeExists, $StateBefore, $StateAfter)
    $script:Events.Add('transaction')
    $script:TransactionCalls++
    if ($script:BindingCalls -lt 2) { throw 'Selection was reached without initial and final compatibility checks.' }
}
function Write-CodexDevReceipt { param($Action, $Details) return 'synthetic-receipt-no-write' }

function Reset-AdmissionFixture {
    $caseRoot = Join-Path $script:Root ([guid]::NewGuid().ToString('N'))
    $script:DeploymentRoot = Join-Path $caseRoot 'deployment'
    $null = [IO.Directory]::CreateDirectory($script:DeploymentRoot)
    $script:ConfigPath = Join-Path $caseRoot 'synthetic-config.toml'
    $script:SourcePackage = New-AdmissionPackage (Join-Path $caseRoot 'source') ('c' * 64)
    $script:Old = New-AdmissionPackage (Join-Path $script:DeploymentRoot ('releases\sha256-' + ('a' * 64))) ('a' * 64)
    $script:Previous = New-AdmissionPackage (Join-Path $script:DeploymentRoot ('releases\sha256-' + ('b' * 64))) ('b' * 64)
    $script:State = New-CodexDevDeploymentState -StateBefore $null -ConfigPath $script:ConfigPath -Current $script:Old -Previous $script:Previous -LastConfigBackup $null
    [IO.File]::WriteAllText((Get-CodexDevDeploymentPaths $script:DeploymentRoot).State, ($script:State | ConvertTo-Json -Depth 12))
    $script:Persistent = $script:Old.Entrypoint
    $script:ConfigSha = 'config-before'
    $script:Status = [pscustomobject]@{
        Status = 'consistent'; State = $script:State; ConfiguredEntrypoint = $script:Old.Entrypoint
        PersistentEntrypoint = $script:Persistent; DriftReasons = @(); PendingExists = $false
        PendingDisposition = 'none'; PendingTransaction = $null; ConfigSha256 = $script:ConfigSha
    }
    $script:RepairCalls = 0; $script:TransactionCalls = 0; $script:ProbeCalls = 0
    $script:BindingCalls = 0; $script:PackageCalls = 0; $script:ChecksMode = 'qualified'
    $script:FailFingerprintOnSecond = $false; $script:RecoveryReturn = $null
    $script:RefreshPlanUnderLock = $false
    $script:BindingPaths.Clear(); $script:Events.Clear()
}
function Set-AdmissionPending {
    param([ValidateSet('rollback', 'complete')][string]$Disposition)
    $after = New-CodexDevDeploymentState -StateBefore $script:State -ConfigPath $script:ConfigPath -Current $script:Previous -Previous $script:Old -LastConfigBackup $null
    $script:Status.PendingExists = $true
    $script:Status.PendingDisposition = $Disposition
    $script:Status.Status = if ($Disposition -eq 'rollback') { 'pending_recoverable_before' } else { 'pending_recoverable_after' }
    $script:Status.PendingTransaction = [pscustomobject]@{
        TransactionId = 'synthetic-pending'; Action = 'Rollback'; StateBefore = $script:State; StateAfter = $after
        StateBeforeExists = $true; PersistentBefore = $script:Old.Entrypoint; PersistentAfter = $script:Previous.Entrypoint
        ConfiguredBefore = $script:Old.Entrypoint; ConfiguredAfter = $script:Previous.Entrypoint
        ConfigAfterSha256 = 'config-after'; ConfigBackup = $null
    }
    if ($Disposition -eq 'complete') {
        $script:RecoveryReturn = [pscustomobject]@{
            Action = 'Rollback'; Status = 'recovered_after'; ConfiguredBefore = $script:Old.Entrypoint
            ConfiguredAfter = $script:Previous.Entrypoint; PersistentBefore = $script:Old.Entrypoint
            PersistentAfter = $script:Previous.Entrypoint; ConfigBackup = $null
        }
    }
}
function Assert-NoAdmissionMutation {
    Assert-AdmissionEqual 0 $script:RepairCalls 'No recovery mutation before admission'
    Assert-AdmissionEqual 0 $script:TransactionCalls 'No selection transaction before admission'
    $newRelease = Join-Path $script:DeploymentRoot ('releases\sha256-' + ('c' * 64))
    Assert-AdmissionEqual $false (Test-Path -LiteralPath $newRelease) 'Rejected source not staged'
}

try {
    Reset-AdmissionFixture
    foreach ($candidate in @($null, [pscustomobject]@{ Entrypoint = $script:Previous.Entrypoint },
        [pscustomobject]@{ ReleasePath = $script:Previous.ReleasePath; Fingerprint = 'invalid' })) {
        Assert-AdmissionRefused { Assert-CodexDevSelectionAdmission -Candidate $candidate } 'managed, hash-bound'
    }
    Assert-AdmissionEqual 0 $script:ProbeCalls 'Absent and pre-lane readers never execute'
    $wrong = [pscustomobject]@{ ReleasePath = $script:Previous.ReleasePath; Fingerprint = 'e' * 64 }
    Assert-AdmissionRefused { Assert-CodexDevSelectionAdmission -Candidate $wrong } 'recorded package fingerprint'
    Assert-AdmissionEqual 0 $script:ProbeCalls 'Mismatching fingerprint never executes'
    $wrongEntrypoint = [pscustomobject]@{
        ReleasePath = $script:Previous.ReleasePath; Fingerprint = $script:Previous.Fingerprint
        Entrypoint = $script:Old.Entrypoint
    }
    Assert-AdmissionRefused { Assert-CodexDevSelectionAdmission -Candidate $wrongEntrypoint } 'entrypoint'
    Assert-AdmissionEqual 0 $script:ProbeCalls 'Package and entrypoint mismatch never executes'
    $admitted = @(Assert-CodexDevSelectionAdmission -Candidate $script:Previous)
    Assert-AdmissionEqual 1 $admitted.Count 'Admission emits one object, not Assert Boolean plus receipt'
    Assert-AdmissionEqual 'qualified' $admitted[0].Status 'Typed admission result'

    foreach ($action in @('Deploy', 'Rollback')) {
        foreach ($mode in @('unknown', 'failed', 'config-drift', 'selector-drift', 'state-drift', 'pending-drift')) {
            Reset-AdmissionFixture
            $script:ChecksMode = $mode
            $pattern = if ($mode -in @('unknown', 'failed')) { 'compatibility was not established' } else { 'changed during compatibility probing' }
            Assert-AdmissionRefused {
                if ($action -eq 'Deploy') {
                    Install-CodexDevPackage -PackageDirectory $script:SourcePackage.PackageRoot -ConfigPath $script:ConfigPath -DeploymentRoot $script:DeploymentRoot -SkipSmoke -Confirm:$false
                } else { Invoke-CodexDevRollback -ConfigPath $script:ConfigPath -DeploymentRoot $script:DeploymentRoot -Confirm:$false }
            } $pattern
            Assert-NoAdmissionMutation
            Assert-AdmissionEqual 1 $script:ProbeCalls "$action still probes when SkipSmoke was passed where supported"
        }
    }
    Reset-AdmissionFixture
    $script:FailFingerprintOnSecond = $true
    Assert-AdmissionRefused { Install-CodexDevPackage -PackageDirectory $script:SourcePackage.PackageRoot -ConfigPath $script:ConfigPath -DeploymentRoot $script:DeploymentRoot -Confirm:$false } 'recorded package fingerprint'
    Assert-NoAdmissionMutation
    Assert-AdmissionEqual 0 $script:ProbeCalls 'Changed pre-probe package not executed'

    Reset-AdmissionFixture
    $script:State.Previous = [pscustomobject]@{ Entrypoint = $script:Previous.Entrypoint }
    Assert-AdmissionRefused { Invoke-CodexDevRollback -ConfigPath $script:ConfigPath -DeploymentRoot $script:DeploymentRoot -Confirm:$false } 'managed, hash-bound'
    Assert-NoAdmissionMutation
    Assert-AdmissionEqual 0 $script:ProbeCalls 'Pre-lane rollback never executes'

    foreach ($action in @('Deploy', 'Rollback')) {
        Reset-AdmissionFixture
        $script:ChecksMode = 'unknown'
        if ($action -eq 'Deploy') {
            $plan = Install-CodexDevPackage -PackageDirectory $script:SourcePackage.PackageRoot -ConfigPath $script:ConfigPath -DeploymentRoot $script:DeploymentRoot -WhatIf
        } else { $plan = Invoke-CodexDevRollback -ConfigPath $script:ConfigPath -DeploymentRoot $script:DeploymentRoot -WhatIf }
        Assert-AdmissionEqual 0 $script:ProbeCalls "$action WhatIf never probes"
        Assert-AdmissionEqual 0 $script:BindingCalls "$action WhatIf never asserts an unrun check"
        Assert-AdmissionEqual $true ($plan.Compatibility -like 'not_probed*') "$action WhatIf honestly labels compatibility"
        Assert-NoAdmissionMutation
    }

    Reset-AdmissionFixture
    $result = @(Install-CodexDevPackage -PackageDirectory $script:SourcePackage.PackageRoot -ConfigPath $script:ConfigPath -DeploymentRoot $script:DeploymentRoot -SkipSmoke -Confirm:$false)
    Assert-AdmissionEqual 1 $result.Count 'Deploy result has no leaked Boolean'
    Assert-AdmissionEqual 'selected_for_restart' $result[0].Status 'Qualified Deploy reaches transaction boundary'
    Assert-AdmissionEqual 1 $script:TransactionCalls 'Deploy transaction double reached once'
    Assert-AdmissionEqual 1 $script:ProbeCalls 'Deploy admits once'
    Assert-AdmissionEqual 2 $script:BindingCalls 'Deploy checks original and staged package'
    Assert-AdmissionEqual $result[0].Release.ReleasePath $script:BindingPaths[1] 'Final binding checks actual staged path'
    Assert-AdmissionEqual 'qualified' $result[0].Compatibility.Status 'Deploy carries one typed receipt'

    Reset-AdmissionFixture
    $result = @(Invoke-CodexDevRollback -ConfigPath $script:ConfigPath -DeploymentRoot $script:DeploymentRoot -Confirm:$false)
    Assert-AdmissionEqual 1 $result.Count 'Rollback result has no leaked Boolean'
    Assert-AdmissionEqual 'previous_selected_for_restart' $result[0].Status 'Qualified Rollback reaches transaction boundary'
    Assert-AdmissionEqual 1 $script:TransactionCalls 'Rollback transaction double reached once'
    Assert-AdmissionEqual 2 $script:BindingCalls 'Rollback checks initial and final candidate'

    Reset-AdmissionFixture
    $script:RefreshPlanUnderLock = $true
    $result = Invoke-CodexDevRollback -ConfigPath $script:ConfigPath -DeploymentRoot $script:DeploymentRoot -Confirm:$false
    Assert-AdmissionEqual $script:SourcePackage.ReleasePath $script:BindingPaths[0] 'Rollback admits the fresh under-lock target, not the stale preview'
    Assert-AdmissionEqual $script:SourcePackage.Entrypoint $result.ConfiguredEntrypoint 'Rollback selection follows its fresh admitted target'
    Assert-AdmissionEqual 1 $script:ProbeCalls 'Fresh under-lock target is the only probed reader'

    foreach ($disposition in @('rollback', 'complete')) {
        Reset-AdmissionFixture
        Set-AdmissionPending $disposition
        $null = Assert-CodexDevRecoveryAdmission -ConfigPath $script:ConfigPath -DeploymentRoot $script:DeploymentRoot
        $target = if ($disposition -eq 'rollback') { $script:Old.ReleasePath } else { $script:Previous.ReleasePath }
        Assert-AdmissionEqual $target $script:BindingPaths[0] "$disposition admits the exact restoration target"
        Assert-NoAdmissionMutation
        $script:Status.PendingTransaction.PersistentBefore = 'wrong-target'
        $script:Status.PendingTransaction.PersistentAfter = 'wrong-target'
        Assert-AdmissionRefused { Assert-CodexDevRecoveryAdmission -ConfigPath $script:ConfigPath -DeploymentRoot $script:DeploymentRoot } 'matching its selector'
        Assert-AdmissionEqual 1 $script:ProbeCalls 'Mismatching recovery selector adds no probe'

        Reset-AdmissionFixture
        Set-AdmissionPending $disposition
        if ($disposition -eq 'rollback') { $script:Status.PendingTransaction.ConfiguredBefore = 'wrong-config-mirror' }
        else { $script:Status.PendingTransaction.ConfiguredAfter = 'wrong-config-mirror' }
        Assert-AdmissionRefused { Assert-CodexDevRecoveryAdmission -ConfigPath $script:ConfigPath -DeploymentRoot $script:DeploymentRoot } 'matching|mirror|config'
        Assert-AdmissionEqual 0 $script:ProbeCalls 'Mismatching recovery config mirror never executes'

        Reset-AdmissionFixture
        Set-AdmissionPending $disposition
        $result = Invoke-CodexDevRollback -ConfigPath $script:ConfigPath -DeploymentRoot $script:DeploymentRoot -Confirm:$false
        Assert-AdmissionEqual 2 $script:ProbeCalls "$disposition checks requested and recovery readers"
        Assert-AdmissionEqual 'probe,binding,probe,binding,repair' (($script:Events | Select-Object -First 5) -join ',') "$disposition qualifies before recovery"
        Assert-AdmissionEqual 1 $script:RepairCalls "$disposition reaches recovery once"
        Assert-AdmissionEqual ([int]($disposition -eq 'rollback')) $script:TransactionCalls "$disposition avoids redundant completed rollback"

        Reset-AdmissionFixture
        Set-AdmissionPending $disposition
        if ($disposition -eq 'rollback') { $script:Status.PendingTransaction.PersistentBefore = 'wrong-target' }
        else { $script:Status.PendingTransaction.PersistentAfter = 'wrong-target' }
        Assert-AdmissionRefused { Install-CodexDevPackage -PackageDirectory $script:SourcePackage.PackageRoot -ConfigPath $script:ConfigPath -DeploymentRoot $script:DeploymentRoot -Confirm:$false } 'matching its selector'
        Assert-NoAdmissionMutation
    }
    Reset-AdmissionFixture
    Set-AdmissionPending 'rollback'
    $script:ChecksMode = 'recovery-only-unknown'
    Assert-AdmissionRefused { Invoke-CodexDevRollback -ConfigPath $script:ConfigPath -DeploymentRoot $script:DeploymentRoot -Confirm:$false } 'compatibility was not established'
    Assert-AdmissionEqual 2 $script:ProbeCalls 'Qualified requested reader does not bypass unknown recovery reader'
    Assert-NoAdmissionMutation
    Reset-AdmissionFixture
    $script:Status.PendingExists = $true
    $script:Status.PendingDisposition = 'ambiguous'
    Assert-AdmissionRefused { Assert-CodexDevRecoveryAdmission -ConfigPath $script:ConfigPath -DeploymentRoot $script:DeploymentRoot } 'cannot be safely recovered'
    Assert-AdmissionEqual 0 $script:ProbeCalls 'Ambiguous pending state is never probed or repaired'
} finally {
    $resolved = [IO.Path]::GetFullPath($testRoot)
    $parent = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd([char[]]'\/') + [IO.Path]::DirectorySeparatorChar
    if (-not $resolved.StartsWith($parent, [StringComparison]::OrdinalIgnoreCase) -or
        [IO.Path]::GetFileName($resolved) -notmatch '^codex-admission-tests-[0-9a-f]{32}$') {
        throw 'Admission test cleanup escaped its generated root.'
    }
    Remove-Item -LiteralPath $resolved -Recurse
}
Write-Output "deployment_admission_tests: $script:Checks checks passed (PowerShell $($PSVersionTable.PSVersion)); no live selectors/config or candidate binaries touched."
