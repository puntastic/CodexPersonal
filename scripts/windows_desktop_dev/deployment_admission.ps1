# Admission is separate from the transaction engine: compensation restores the
# exact before-images of an operation, never invents a different fallback.
function Get-CodexDevAdmissionSnapshot {
    param([string]$ConfigPath, [string]$DeploymentRoot)
    $config = Read-CodexCliConfigSnapshot $ConfigPath
    if ($null -ne $config.Error) { throw 'Cannot bind deployment admission to an unreadable config.' }
    $selector = Read-CodexDevPersistentSelector
    if ($null -ne $selector.Error) { throw 'Cannot bind deployment admission to an unreadable User selector.' }
    $paths = Get-CodexDevDeploymentPaths $DeploymentRoot
    $hashes = foreach ($path in @($paths.State, $paths.Pending)) {
        if (Test-Path -LiteralPath $path -PathType Leaf) {
            (Get-FileHash -LiteralPath $path -Algorithm SHA256 -ErrorAction Stop).Hash
        } elseif (Test-Path -LiteralPath $path) { throw 'Deployment state path is not an ordinary file.' }
        else { 'absent' }
    }
    return ([ordered]@{ Config = $config.Sha256; UserSelector = $selector.Value; State = $hashes[0]; Pending = $hashes[1] } | ConvertTo-Json -Compress)
}

function Assert-CodexDevAdmissionSnapshot {
    param([string]$Snapshot, [string]$ConfigPath, [string]$DeploymentRoot)
    if ($Snapshot -cne (Get-CodexDevAdmissionSnapshot -ConfigPath $ConfigPath -DeploymentRoot $DeploymentRoot)) {
        throw 'Config, User selector or deployment ledger changed during compatibility probing. No new selection was made; inspect and retry only after settlement.'
    }
}

function Assert-CodexDevSelectionAdmission {
    param([object]$Candidate, [string]$PythonPath)
    $root = [string](Get-CodexDevObjectProperty -Value $Candidate -Name 'ReleasePath')
    if ([string]::IsNullOrWhiteSpace($root)) {
        $root = [string](Get-CodexDevObjectProperty -Value $Candidate -Name 'PackageRoot')
    }
    $fingerprint = [string](Get-CodexDevObjectProperty -Value $Candidate -Name 'Fingerprint')
    if ([string]::IsNullOrWhiteSpace($root) -or $fingerprint -notmatch '^[0-9a-f]{64}$') {
        throw 'Compatibility admission requires a managed, hash-bound package. A pre-lane or absent reader needs an explicit recovery appraisal; no fallback was selected.'
    }
    $actual = Get-CodexDevPackageInfo -PackageDirectory $root -SkipSmoke
    if ($actual.Fingerprint -cne $fingerprint) { throw 'Admission candidate differs from its recorded package fingerprint.' }
    $entrypoint = [string](Get-CodexDevObjectProperty -Value $Candidate -Name 'Entrypoint')
    if (-not (Test-CodexDevPathEqual -Left $entrypoint -Right $actual.Entrypoint)) {
        throw 'Admission selector does not name the validated package entrypoint; no recovery or selection was performed.'
    }
    $check = Invoke-CodexDevCompatibilityCheck -PackageDirectory $root -PythonPath $PythonPath
    if (-not $check.Compatible) {
        throw "Desktop/history compatibility was not established: $($check.Status). $($check.Blockers -join ' ') Report: $($check.ReportPath)"
    }
    $null = Assert-CodexDevCompatibilityBinding -Check $check -PackageDirectory $root
    return $check
}

function Assert-CodexDevRecoveryAdmission {
    param([string]$ConfigPath, [string]$DeploymentRoot, [string]$PythonPath)
    $status = Get-CodexDevDeploymentStatusUnlocked -ConfigPath $ConfigPath -DeploymentRoot $DeploymentRoot
    if (-not $status.PendingExists) { return }
    if ($status.PendingDisposition -notin @('rollback', 'complete')) {
        throw 'Pending deployment cannot be safely recovered automatically. Inspect the retained journal; no selection was changed.'
    }
    $pending = $status.PendingTransaction
    $state = if ($status.PendingDisposition -eq 'rollback') { $pending.StateBefore } else { $pending.StateAfter }
    $candidate = Get-CodexDevObjectProperty -Value $state -Name 'Current'
    $expected = if ($status.PendingDisposition -eq 'rollback') { $pending.PersistentBefore } else { $pending.PersistentAfter }
    $mirror = if ($status.PendingDisposition -eq 'rollback') { $pending.ConfiguredBefore } else { $pending.ConfiguredAfter }
    $entrypoint = [string](Get-CodexDevObjectProperty -Value $candidate -Name 'Entrypoint')
    if (-not (Test-CodexDevSelectorExact -Left $entrypoint -Right $expected) -or
        -not (Test-CodexDevSelectorExact -Left $entrypoint -Right $mirror)) {
        throw 'Pending recovery does not name one managed reader matching its selector. Preserve the journal for explicit recovery.'
    }
    $null = Assert-CodexDevSelectionAdmission -Candidate $candidate -PythonPath $PythonPath
}
