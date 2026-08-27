function Install-CodexDevPackage {
    [CmdletBinding(SupportsShouldProcess = $true)]
    param(
        [string]$PackageDirectory,
        [string]$ConfigPath,
        [string]$DeploymentRoot,
        [switch]$SkipSmoke
    )

    $skipPreflightSmoke = $SkipSmoke -or $WhatIfPreference
    $package = Get-CodexDevPackageInfo -PackageDirectory $PackageDirectory -SkipSmoke:$skipPreflightSmoke
    $config = [System.IO.Path]::GetFullPath($ConfigPath)
    $root = [System.IO.Path]::GetFullPath($DeploymentRoot)
    $selection = Get-CodexDevPackageSelection $package
    $releaseId = "sha256-$($package.Fingerprint)"
    $releasePath = Join-Path $root "releases\$releaseId"
    $entrypoint = Join-Path $releasePath "bin\codex.exe"
    $planDeployment = Get-CodexDevDeploymentStatus `
        -ConfigPath $config `
        -DeploymentRoot $root
    $configuredForPlan = $planDeployment.ConfiguredEntrypoint
    $planBlockers = @(
        if ($planDeployment.Status -notin @("consistent", "unmanaged")) {
            "Deployment is not settled: $($planDeployment.Status). " +
            ($planDeployment.DriftReasons -join " ")
        }
        if (Test-Path -LiteralPath $releasePath -PathType Container) {
            try {
                $plannedRelease = Get-CodexDevPackageInfo `
                    -PackageDirectory $releasePath `
                    -SkipSmoke
                if ($plannedRelease.Fingerprint -ne $package.Fingerprint) {
                    "Existing release ID has a different package fingerprint: $releasePath"
                }
            } catch {
                "Existing release is not viable: $($_.Exception.Message)"
            }
        }
    )

    if (-not $PSCmdlet.ShouldProcess($config, "Stage $releaseId and select it for the next Codex Desktop restart")) {
        return [pscustomobject]@{
            Status = if ($planBlockers.Count -eq 0) { "planned" } else { "blocked" }
            SourcePackage = $package.PackageRoot
            ReleasePath = $releasePath
            ConfigPath = $config
            ConfiguredBefore = $configuredForPlan
            ConfiguredAfter = $entrypoint
            DeploymentStatus = $planDeployment.Status
            Blockers = $planBlockers
        }
    }

    $operation = Invoke-WithCodexDevDeploymentLock -DeploymentRoot $root -Body {
        Invoke-WithCodexDevConfigLock -ConfigPath $config -Body {
        $recovery = Repair-CodexDevInterruptedTransaction `
            -ConfigPath $config `
            -DeploymentRoot $root

        $deploymentStatus = Get-CodexDevDeploymentStatusUnlocked `
            -ConfigPath $config `
            -DeploymentRoot $root
        if ($deploymentStatus.Status -notin @("consistent", "unmanaged")) {
            throw (
                "Cannot deploy while config and deployment state are not settled: " +
                "$($deploymentStatus.Status). $($deploymentStatus.DriftReasons -join ' ')"
            )
        }

        New-Item -ItemType Directory -Path (Join-Path $root "releases") -Force | Out-Null
        if (-not (Test-Path -LiteralPath $releasePath -PathType Container)) {
            $staging = Join-Path $root (".staging-" + [guid]::NewGuid().ToString("N"))
            New-Item -ItemType Directory -Path $staging | Out-Null
            try {
                Get-ChildItem -LiteralPath $package.PackageRoot -Force |
                    Copy-Item -Destination $staging -Recurse -Force
                $staged = Get-CodexDevPackageInfo `
                    -PackageDirectory $staging `
                    -SkipSmoke:$SkipSmoke
                if ($staged.Fingerprint -ne $package.Fingerprint) {
                    throw "Staged package fingerprint changed during copy."
                }
                Move-Item -LiteralPath $staging -Destination $releasePath
            } finally {
                if (Test-Path -LiteralPath $staging) {
                    $resolvedStaging = [System.IO.Path]::GetFullPath($staging)
                    if ($resolvedStaging.StartsWith($root, [System.StringComparison]::OrdinalIgnoreCase)) {
                        Remove-Item `
                            -LiteralPath $resolvedStaging `
                            -Recurse `
                            -Force `
                            -ErrorAction SilentlyContinue
                    }
                }
            }
        } else {
            $existing = Get-CodexDevPackageInfo `
                -PackageDirectory $releasePath `
                -SkipSmoke:$SkipSmoke
            if ($existing.Fingerprint -ne $package.Fingerprint) {
                throw "Existing release ID has a different package fingerprint: $releasePath"
            }
        }

        $configTransition = Get-CodexCliConfigTransition `
            -ConfigPath $config `
            -Entrypoint $entrypoint
        $configuredBefore = $configTransition.ConfiguredBefore
        $paths = Get-CodexDevDeploymentPaths $root
        $stateRead = Read-CodexDevJsonFile $paths.State
        if ($null -ne $stateRead.Error) {
            throw "Deployment state could not be read: $($stateRead.Error)"
        }
        $stateBefore = $stateRead.Value
        $currentBefore = Get-CodexDevObjectProperty -Value $stateBefore -Name "Current"
        $currentBeforeEntrypoint = [string](
            Get-CodexDevObjectProperty -Value $currentBefore -Name "Entrypoint"
        )
        $currentSelection = Get-CodexDevObjectProperty `
            -Value $currentBefore `
            -Name "Selection"
        if (-not $stateRead.Exists -and
            (Test-CodexDevPathEqual -Left $configuredBefore -Right $entrypoint)) {
            $adoptedCurrent = [ordered]@{
                ReleaseId = $releaseId
                ReleasePath = $releasePath
                Entrypoint = $entrypoint
                Fingerprint = $package.Fingerprint
                SourcePackageRoot = $selection.SourcePackageRoot
                BuildId = $selection.BuildId
                ProvenanceStatus = $selection.ProvenanceStatus
                Selection = $selection
                SelectedAtUtc = [DateTime]::UtcNow.ToString("o")
            }
            $adoptedState = New-CodexDevDeploymentState `
                -StateBefore $null `
                -ConfigPath $config `
                -Current $adoptedCurrent `
                -Previous $null `
                -LastConfigBackup $null
            $configBeforeAdoption = Read-CodexCliConfigSnapshot $config
            if ($configBeforeAdoption.Sha256 -ne $configTransition.BeforeSha256) {
                throw "Config changed while state-only deployment adoption was being prepared."
            }
            Write-CodexDevJson -Path $paths.State -Value $adoptedState
            return [pscustomobject]@{
                Status = "adopted_existing_selection"
                Release = $adoptedCurrent
                Previous = $null
                ConfigPath = $config
                ConfiguredBefore = $configuredBefore
                ConfigBackup = $null
                RestartRequired = $env:CODEX_CLI_PATH -ne $entrypoint
                Recovery = $recovery
            }
        }
        if ((Test-CodexDevPathEqual -Left $currentBeforeEntrypoint -Right $entrypoint) -and
            (Test-CodexDevPathEqual -Left $configuredBefore -Right $entrypoint) -and
            (Test-CodexDevPackageSelectionEqual -Left $currentSelection -Right $selection)) {
            return [pscustomobject]@{
                Status = "already_selected"
                Release = $currentBefore
                Previous = Get-CodexDevObjectProperty -Value $stateBefore -Name "Previous"
                ConfigPath = $config
                ConfiguredBefore = if ($null -ne $recovery) {
                    $recovery.ConfiguredBefore
                } else {
                    $configuredBefore
                }
                ConfigBackup = if ($null -ne $recovery) {
                    $recovery.ConfigBackup
                } else {
                    [string](Get-CodexDevObjectProperty `
                        -Value $stateBefore `
                        -Name "LastConfigBackup")
                }
                RestartRequired = $env:CODEX_CLI_PATH -ne $entrypoint
                Recovery = $recovery
            }
        }

        if ((Test-CodexDevPathEqual -Left $currentBeforeEntrypoint -Right $entrypoint) -and
            (Test-CodexDevPathEqual -Left $configuredBefore -Right $entrypoint)) {
            $selectionOnlyCurrent = [ordered]@{
                ReleaseId = [string](Get-CodexDevObjectProperty -Value $currentBefore -Name "ReleaseId")
                ReleasePath = [string](Get-CodexDevObjectProperty -Value $currentBefore -Name "ReleasePath")
                Entrypoint = $entrypoint
                Fingerprint = $package.Fingerprint
                SourcePackageRoot = $selection.SourcePackageRoot
                BuildId = $selection.BuildId
                ProvenanceStatus = $selection.ProvenanceStatus
                Selection = $selection
                SelectedAtUtc = [DateTime]::UtcNow.ToString("o")
            }
            $selectionOnlyState = New-CodexDevDeploymentState `
                -StateBefore $stateBefore `
                -ConfigPath $config `
                -Current $selectionOnlyCurrent `
                -Previous (Get-CodexDevObjectProperty -Value $stateBefore -Name "Previous") `
                -LastConfigBackup ([string](Get-CodexDevObjectProperty `
                    -Value $stateBefore `
                    -Name "LastConfigBackup"))
            $configBeforeSelection = Read-CodexCliConfigSnapshot $config
            if ($configBeforeSelection.Sha256 -ne $configTransition.BeforeSha256) {
                throw "Config changed while the selected build occurrence was being recorded."
            }
            Write-CodexDevJson -Path $paths.State -Value $selectionOnlyState
            return [pscustomobject]@{
                Status = "selection_recorded"
                Release = $selectionOnlyCurrent
                Previous = $selectionOnlyState.Previous
                ConfigPath = $config
                ConfiguredBefore = $configuredBefore
                ConfigBackup = $selectionOnlyState.LastConfigBackup
                RestartRequired = $env:CODEX_CLI_PATH -ne $entrypoint
                Recovery = $recovery
            }
        }

        $previous = if ((Test-CodexDevPathEqual `
            -Left $currentBeforeEntrypoint `
            -Right $configuredBefore) -and $null -ne $currentBefore) {
            $currentBefore
        } elseif (-not [string]::IsNullOrWhiteSpace($configuredBefore)) {
            [pscustomobject]@{
                ReleaseId = "pre-lane"
                Entrypoint = $configuredBefore
                Fingerprint = $null
            }
        } else {
            $null
        }
        $backupDirectory = Join-Path $root "config-backups"
        New-Item -ItemType Directory -Path $backupDirectory -Force | Out-Null
        $backupPath = Join-Path $backupDirectory (
            "$([DateTime]::UtcNow.ToString('yyyyMMdd-HHmmssfff'))-$([guid]::NewGuid().ToString('N'))-config.toml"
        )
        [System.IO.File]::WriteAllBytes($backupPath, [byte[]]$configTransition.BeforeBytes)

        $current = [ordered]@{
            ReleaseId = $releaseId
            ReleasePath = $releasePath
            Entrypoint = $entrypoint
            Fingerprint = $package.Fingerprint
            SourcePackageRoot = $selection.SourcePackageRoot
            BuildId = $selection.BuildId
            ProvenanceStatus = $selection.ProvenanceStatus
            Selection = $selection
            SelectedAtUtc = [DateTime]::UtcNow.ToString("o")
        }
        $state = New-CodexDevDeploymentState `
            -StateBefore $stateBefore `
            -ConfigPath $config `
            -Current $current `
            -Previous $previous `
            -LastConfigBackup $backupPath
        Invoke-CodexDevDeploymentTransaction `
            -Action "Deploy" `
            -ConfigPath $config `
            -DeploymentRoot $root `
            -ConfiguredBefore $configuredBefore `
            -ConfiguredAfter $entrypoint `
            -ConfigTransition $configTransition `
            -ConfigBackup $backupPath `
            -StateBeforeExists ([bool]$stateRead.Exists) `
            -StateBefore $stateBefore `
            -StateAfter $state

        return [pscustomobject]@{
            Status = "selected_for_restart"
            Release = $current
            Previous = $previous
            ConfigPath = $config
            ConfiguredBefore = $configuredBefore
            ConfigBackup = $backupPath
            RestartRequired = $env:CODEX_CLI_PATH -ne $entrypoint
            Recovery = $recovery
        }
        }
    }

    if ($operation.Status -eq "already_selected" -and $null -eq $operation.Recovery) {
        return [pscustomobject]@{
            Status = $operation.Status
            Release = $operation.Release
            Previous = $operation.Previous
            ConfigPath = $config
            Receipt = $null
            ReceiptStatus = "not_written_already_selected"
            ReceiptError = $null
            RetryRequired = $false
            RetryGuidance = "No receipt is written for an idempotent already-selected Deploy."
            RestartRequired = $operation.RestartRequired
            Recovery = $operation.Recovery
        }
    }
    $receipt = $null
    $receiptError = $null
    try {
        $receipt = Write-CodexDevReceipt -Action "Deploy" -Details ([ordered]@{
            Package = $package
            Release = $operation.Release
            ConfigPath = $config
            ConfiguredBefore = $operation.ConfiguredBefore
            ConfigBackup = $operation.ConfigBackup
            LiveBefore = $env:CODEX_CLI_PATH
            Recovery = $operation.Recovery
            Selection = $selection
        })
    } catch {
        $receiptError = $_.Exception.Message
    }
    return [pscustomobject]@{
        Status = $operation.Status
        Release = $operation.Release
        Previous = $operation.Previous
        ConfigPath = $config
        Receipt = $receipt
        ReceiptStatus = if ($null -eq $receiptError) { "written" } else { "failed_after_success" }
        ReceiptError = $receiptError
        RetryRequired = $false
        RetryGuidance = if ($null -ne $receiptError) {
            "Do not retry Deploy; the selection succeeded and only receipt recording failed."
        } else {
            $null
        }
        RestartRequired = $operation.RestartRequired
        Recovery = $operation.Recovery
    }
}

function Invoke-CodexDevVerify {
    param(
        [string]$ConfigPath,
        [string]$DeploymentRoot = (Get-CodexDevDefaultDeploymentRoot)
    )

    $deployment = Get-CodexDevDeploymentStatus `
        -ConfigPath $ConfigPath `
        -DeploymentRoot $DeploymentRoot
    $configured = $deployment.ConfiguredEntrypoint
    if ($deployment.Status -ne "consistent") {
        return [pscustomobject]@{
            Status = $deployment.Status
            ConfigPath = $ConfigPath
            ConfiguredEntrypoint = $configured
            LiveEntrypoint = $env:CODEX_CLI_PATH
            Package = $null
            SelectedOccurrence = $null
            StagedReleaseProvenance = $null
            VerificationMode = "unsettled"
            ProofBoundary = "Deployment settlement is required before package verification."
            RestartRequired = $env:CODEX_CLI_PATH -ne $configured
            DeploymentStatus = $deployment.Status
            PendingDisposition = $deployment.PendingDisposition
            DriftReasons = @($deployment.DriftReasons)
            Deployment = $deployment
            DeploymentSettled = $false
        }
    }
    if ([string]::IsNullOrWhiteSpace($configured)) {
        throw "CODEX_CLI_PATH is not configured in $ConfigPath"
    }
    $configSnapshot = Read-CodexCliConfigSnapshot $ConfigPath
    if ($null -ne $configSnapshot.Error -or
        $configSnapshot.Sha256 -ne $deployment.ConfigSha256) {
        return [pscustomobject]@{
            Status = "drift"
            ConfigPath = $ConfigPath
            ConfiguredEntrypoint = $configured
            LiveEntrypoint = $env:CODEX_CLI_PATH
            Package = $null
            SelectedOccurrence = $null
            StagedReleaseProvenance = $null
            VerificationMode = "config_snapshot"
            ProofBoundary = "Config changed before package verification could begin."
            RestartRequired = $env:CODEX_CLI_PATH -ne $configured
            DeploymentStatus = "drift"
            PendingDisposition = $deployment.PendingDisposition
            DriftReasons = @("Config changed during deployment verification.")
            Deployment = $deployment
            DeploymentSettled = $false
        }
    }
    if (-not (Test-Path -LiteralPath $configured -PathType Leaf)) {
        throw "Configured Codex override does not exist: $configured"
    }
    $verificationDrift = [System.Collections.Generic.List[string]]::new()
    foreach ($reason in $deployment.DriftReasons) {
        $verificationDrift.Add([string]$reason)
    }
    $current = Get-CodexDevObjectProperty -Value $deployment.State -Name "Current"
    $selection = Get-CodexDevObjectProperty -Value $current -Name "Selection"
    $currentFingerprint = [string](
        Get-CodexDevObjectProperty -Value $current -Name "Fingerprint"
    )
    $currentReleasePath = [string](
        Get-CodexDevObjectProperty -Value $current -Name "ReleasePath"
    )
    $hasFingerprint = -not [string]::IsNullOrWhiteSpace($currentFingerprint)
    $hasReleasePath = -not [string]::IsNullOrWhiteSpace($currentReleasePath)
    if (-not $hasFingerprint -and -not $hasReleasePath) {
        $verifiedConfig = Read-CodexCliConfigSnapshot $ConfigPath
        if ($null -ne $verifiedConfig.Error -or
            $verifiedConfig.Sha256 -ne $deployment.ConfigSha256) {
            $verificationDrift.Add("Config changed during deployment verification.")
        }
        $effectiveDeploymentStatus = if ($verificationDrift.Count -gt 0) {
            "drift"
        } else {
            "consistent"
        }
        return [pscustomobject]@{
            Status = if ($effectiveDeploymentStatus -eq "consistent") {
                if ($env:CODEX_CLI_PATH -eq $configured) { "live" } else { "restart_required" }
            } else {
                $effectiveDeploymentStatus
            }
            ConfigPath = $ConfigPath
            ConfiguredEntrypoint = $configured
            LiveEntrypoint = $env:CODEX_CLI_PATH
            Package = $null
            SelectedOccurrence = $null
            StagedReleaseProvenance = $null
            VerificationMode = "pre_lane_selector_only"
            ProofBoundary = (
                "Selector existence only; pre-lane package integrity, PE targets, and provenance " +
                "are not attested."
            )
            RestartRequired = $env:CODEX_CLI_PATH -ne $configured
            DeploymentStatus = $effectiveDeploymentStatus
            PendingDisposition = $deployment.PendingDisposition
            DriftReasons = @($verificationDrift.ToArray())
            Deployment = $deployment
            DeploymentSettled = $effectiveDeploymentStatus -eq "consistent"
        }
    }
    if ($hasFingerprint -ne $hasReleasePath) {
        return [pscustomobject]@{
            Status = "drift"
            ConfigPath = $ConfigPath
            ConfiguredEntrypoint = $configured
            LiveEntrypoint = $env:CODEX_CLI_PATH
            Package = $null
            SelectedOccurrence = $selection
            StagedReleaseProvenance = $null
            VerificationMode = "managed_package"
            ProofBoundary = "Managed deployment state must record both ReleasePath and Fingerprint."
            RestartRequired = $env:CODEX_CLI_PATH -ne $configured
            DeploymentStatus = "drift"
            PendingDisposition = $deployment.PendingDisposition
            DriftReasons = @("Managed deployment state has incomplete package identity.")
            Deployment = $deployment
            DeploymentSettled = $false
        }
    }
    if (-not (Test-CodexDevPathEqual `
        -Left $configured `
        -Right (Join-Path $currentReleasePath "bin\codex.exe"))) {
        $verificationDrift.Add("Configured entrypoint is outside state.Current.ReleasePath.")
    }
    $package = Get-CodexDevPackageInfo $currentReleasePath
    $selectionFingerprint = [string](
        Get-CodexDevObjectProperty -Value $selection -Name "ArtifactFingerprint"
    )
    $currentBuildId = [string](
        Get-CodexDevObjectProperty -Value $current -Name "BuildId"
    )
    $selectionBuildId = [string](
        Get-CodexDevObjectProperty -Value $selection -Name "BuildId"
    )
    $currentSource = [string](
        Get-CodexDevObjectProperty -Value $current -Name "SourcePackageRoot"
    )
    $selectionSource = [string](
        Get-CodexDevObjectProperty -Value $selection -Name "SourcePackageRoot"
    )
    $currentProvenanceStatus = [string](
        Get-CodexDevObjectProperty -Value $current -Name "ProvenanceStatus"
    )
    $selectionProvenanceStatus = [string](
        Get-CodexDevObjectProperty -Value $selection -Name "ProvenanceStatus"
    )
    if ($currentFingerprint -ne $package.Fingerprint) {
        $verificationDrift.Add("Configured package fingerprint does not match state.Current.")
    }
    if ($null -eq $selection -or $selectionFingerprint -ne $package.Fingerprint) {
        $verificationDrift.Add("Selected build occurrence fingerprint does not match the configured package.")
    }
    if ($currentBuildId -ne $selectionBuildId) {
        $verificationDrift.Add("Selected build occurrence BuildId does not match state.Current.")
    }
    if (-not (Test-CodexDevPathEqual -Left $currentSource -Right $selectionSource)) {
        $verificationDrift.Add("Selected source package does not match state.Current.")
    }
    if ($currentProvenanceStatus -cne $selectionProvenanceStatus) {
        $verificationDrift.Add("Selected provenance status does not match state.Current.")
    }
    $verifiedConfig = Read-CodexCliConfigSnapshot $ConfigPath
    if ($null -ne $verifiedConfig.Error -or
        $verifiedConfig.Sha256 -ne $deployment.ConfigSha256) {
        $verificationDrift.Add("Config changed during deployment verification.")
    }
    $effectiveDeploymentStatus = if ($verificationDrift.Count -gt 0) {
        "drift"
    } else {
        "consistent"
    }
    $status = if ($effectiveDeploymentStatus -eq "consistent") {
        if ($env:CODEX_CLI_PATH -eq $configured) { "live" } else { "restart_required" }
    } else {
        $effectiveDeploymentStatus
    }
    return [pscustomobject]@{
        Status = $status
        ConfigPath = $ConfigPath
        ConfiguredEntrypoint = $configured
        LiveEntrypoint = $env:CODEX_CLI_PATH
        Package = $package
        SelectedOccurrence = $selection
        StagedReleaseProvenance = [pscustomobject]@{
            PackageRoot = $package.PackageRoot
            ProvenanceStatus = $package.ProvenanceStatus
            Provenance = $package.Provenance
        }
        VerificationMode = "managed_package"
        ProofBoundary = (
            "Configured package layout, host PE targets, artifact fingerprint, selected occurrence, " +
            "and executable smoke are verified."
        )
        RestartRequired = $env:CODEX_CLI_PATH -ne $configured
        DeploymentStatus = $effectiveDeploymentStatus
        PendingDisposition = $deployment.PendingDisposition
        DriftReasons = @($verificationDrift.ToArray())
        Deployment = $deployment
        DeploymentSettled = $effectiveDeploymentStatus -eq "consistent"
    }
}

function Get-CodexDevRollbackCandidateValidation {
    param(
        [AllowNull()][object]$Candidate,
        [switch]$SkipSmoke
    )

    $entrypoint = [string](
        Get-CodexDevObjectProperty -Value $Candidate -Name "Entrypoint"
    )
    $releasePath = [string](
        Get-CodexDevObjectProperty -Value $Candidate -Name "ReleasePath"
    )
    $fingerprint = [string](
        Get-CodexDevObjectProperty -Value $Candidate -Name "Fingerprint"
    )
    if ([string]::IsNullOrWhiteSpace($entrypoint)) {
        return [pscustomobject]@{
            Valid = $false
            Mode = "invalid"
            Package = $null
            Error = "Rollback candidate has no entrypoint."
        }
    }

    $hasReleasePath = -not [string]::IsNullOrWhiteSpace($releasePath)
    $hasFingerprint = -not [string]::IsNullOrWhiteSpace($fingerprint)
    if ($hasReleasePath -ne $hasFingerprint) {
        return [pscustomobject]@{
            Valid = $false
            Mode = "invalid"
            Package = $null
            Error = "Rollback candidate must record both ReleasePath and Fingerprint."
        }
    }
    if (-not $hasReleasePath) {
        return [pscustomobject]@{
            Valid = Test-Path -LiteralPath $entrypoint -PathType Leaf
            Mode = "pre_lane"
            Package = $null
            Error = if (Test-Path -LiteralPath $entrypoint -PathType Leaf) {
                $null
            } else {
                "Previous Desktop override no longer exists: $entrypoint"
            }
        }
    }

    $expectedEntrypoint = Join-Path $releasePath "bin\codex.exe"
    if (-not (Test-CodexDevPathEqual -Left $entrypoint -Right $expectedEntrypoint)) {
        return [pscustomobject]@{
            Valid = $false
            Mode = "managed"
            Package = $null
            Error = "Managed rollback entrypoint is outside its recorded release package."
        }
    }
    try {
        $package = Get-CodexDevPackageInfo `
            -PackageDirectory $releasePath `
            -SkipSmoke:$SkipSmoke
    } catch {
        return [pscustomobject]@{
            Valid = $false
            Mode = "managed"
            Package = $null
            Error = "Managed rollback package validation failed: $($_.Exception.Message)"
        }
    }
    if ([string]$package.Fingerprint -ne $fingerprint) {
        return [pscustomobject]@{
            Valid = $false
            Mode = "managed"
            Package = $package
            Error = "Managed rollback package fingerprint does not match deployment state."
        }
    }
    return [pscustomobject]@{
        Valid = $true
        Mode = "managed"
        Package = $package
        Error = $null
    }
}

function Get-CodexDevRollbackPlan {
    param(
        [object]$DeploymentStatus,
        [string]$ConfigPath
    )

    $blockers = [System.Collections.Generic.List[string]]::new()
    $state = $null
    $configured = $null
    $recoveryDisposition = "none"
    $recoveryCompletesRequest = $false
    $pendingTransactionId = $null
    $pendingStateAfter = $null
    $pendingConfigAfterSha256 = $null
    $pendingConfiguredBefore = $null
    $pendingConfigBackup = $null

    switch ($DeploymentStatus.Status) {
        "consistent" {
            $state = $DeploymentStatus.State
            $configured = $DeploymentStatus.ConfiguredEntrypoint
        }
        "pending_recoverable_before" {
            $pending = $DeploymentStatus.PendingTransaction
            $recoveryDisposition = "recover_before"
            $pendingTransactionId = [string]$pending.TransactionId
            $pendingStateAfter = $pending.StateAfter
            $pendingConfigAfterSha256 = [string]$pending.ConfigAfterSha256
            $pendingConfiguredBefore = [string]$pending.ConfiguredBefore
            $pendingConfigBackup = [string]$pending.ConfigBackup
            $configured = [string]$pending.ConfiguredBefore
            if ([bool]$pending.StateBeforeExists) {
                $state = $pending.StateBefore
            }
        }
        "pending_recoverable_after" {
            $pending = $DeploymentStatus.PendingTransaction
            $recoveryDisposition = "recover_after"
            $pendingTransactionId = [string]$pending.TransactionId
            $pendingStateAfter = $pending.StateAfter
            $pendingConfigAfterSha256 = [string]$pending.ConfigAfterSha256
            $pendingConfiguredBefore = [string]$pending.ConfiguredBefore
            $pendingConfigBackup = [string]$pending.ConfigBackup
            $configured = [string]$pending.ConfiguredAfter
            $state = $pending.StateAfter
            $recoveryCompletesRequest = [string]$pending.Action -eq "Rollback"
        }
        default {
            $blockers.Add(
                "Deployment is not settled: $($DeploymentStatus.Status). " +
                ($DeploymentStatus.DriftReasons -join " ")
            )
        }
    }

    if ($blockers.Count -eq 0 -and $null -eq $state) {
        $blockers.Add("No deployment state will exist after pending recovery.")
    }
    if ($blockers.Count -eq 0 -and
        -not (Test-CodexDevStateSchema -State $state -ConfigPath $ConfigPath)) {
        $blockers.Add("Deployment state after pending recovery is invalid.")
    }

    $current = Get-CodexDevObjectProperty -Value $state -Name "Current"
    $previous = Get-CodexDevObjectProperty -Value $state -Name "Previous"
    $currentEntrypoint = [string](
        Get-CodexDevObjectProperty -Value $current -Name "Entrypoint"
    )
    $previousEntrypoint = [string](
        Get-CodexDevObjectProperty -Value $previous -Name "Entrypoint"
    )
    if ($blockers.Count -eq 0 -and
        -not (Test-CodexDevPathEqual -Left $configured -Right $currentEntrypoint)) {
        $blockers.Add("Config and deployment state will not agree after pending recovery.")
    }
    if ($blockers.Count -eq 0 -and -not $recoveryCompletesRequest) {
        if ([string]::IsNullOrWhiteSpace($previousEntrypoint)) {
            $blockers.Add("No previous Desktop override is recorded.")
        } elseif (Test-CodexDevPathEqual -Left $currentEntrypoint -Right $previousEntrypoint) {
            $blockers.Add("The recorded current and previous Desktop overrides are identical.")
        }
    }
    $candidate = if ($recoveryCompletesRequest) { $current } else { $previous }
    $candidateValidation = $null
    if ($blockers.Count -eq 0) {
        $candidateValidation = Get-CodexDevRollbackCandidateValidation `
            -Candidate $candidate `
            -SkipSmoke
        if (-not $candidateValidation.Valid) {
            $blockers.Add([string]$candidateValidation.Error)
        }
    }

    return [pscustomobject]@{
        Status = if ($blockers.Count -gt 0) {
            "blocked"
        } elseif ($recoveryDisposition -ne "none") {
            "planned_with_recovery"
        } else {
            "planned"
        }
        ConfigPath = $ConfigPath
        ConfiguredBefore = $configured
        ConfiguredAfter = if ($recoveryCompletesRequest) {
            $configured
        } else {
            $previousEntrypoint
        }
        Current = $current
        Previous = $previous
        CandidateValidation = $candidateValidation
        DeploymentStatus = $DeploymentStatus.Status
        RecoveryDisposition = $recoveryDisposition
        RecoveryCompletesRequest = $recoveryCompletesRequest
        PendingTransactionId = $pendingTransactionId
        PendingStateAfter = $pendingStateAfter
        PendingConfigAfterSha256 = $pendingConfigAfterSha256
        PendingConfiguredBefore = $pendingConfiguredBefore
        PendingConfigBackup = $pendingConfigBackup
        Blockers = @($blockers.ToArray())
    }
}

function Invoke-CodexDevRollback {
    [CmdletBinding(SupportsShouldProcess = $true)]
    param(
        [string]$ConfigPath,
        [string]$DeploymentRoot
    )

    $config = [System.IO.Path]::GetFullPath($ConfigPath)
    $root = [System.IO.Path]::GetFullPath($DeploymentRoot)
    $paths = Get-CodexDevDeploymentPaths $root
    $planDeployment = Get-CodexDevDeploymentStatus `
        -ConfigPath $config `
        -DeploymentRoot $root
    $rollbackPlan = Get-CodexDevRollbackPlan `
        -DeploymentStatus $planDeployment `
        -ConfigPath $config
    if ($rollbackPlan.Status -eq "blocked") {
        if ($WhatIfPreference) {
            return $rollbackPlan
        }
        throw "Cannot roll back: $($rollbackPlan.Blockers -join ' ')"
    }
    if (-not $PSCmdlet.ShouldProcess($config, "Select previous Desktop override for the next restart")) {
        return $rollbackPlan
    }

    $operation = Invoke-WithCodexDevDeploymentLock -DeploymentRoot $root -Body {
        Invoke-WithCodexDevConfigLock -ConfigPath $config -Body {
        if ($rollbackPlan.RecoveryCompletesRequest) {
            $recoveryCandidate = Get-CodexDevRollbackCandidateValidation `
                -Candidate $rollbackPlan.Current
            if (-not $recoveryCandidate.Valid) {
                throw "Cannot complete interrupted Rollback: $($recoveryCandidate.Error)"
            }
        }
        $recovery = Repair-CodexDevInterruptedTransaction `
            -ConfigPath $config `
            -DeploymentRoot $root
        if ($null -ne $recovery -and
            $recovery.Action -eq "Rollback" -and
            $recovery.Status -eq "recovered_after") {
            return [pscustomobject]@{
                Status = "previous_selected_for_restart"
                ConfiguredBefore = $recovery.ConfiguredBefore
                ConfiguredEntrypoint = $recovery.ConfiguredAfter
                ConfigBackup = $recovery.ConfigBackup
                Recovery = $recovery
                RestartRequired = $env:CODEX_CLI_PATH -ne $recovery.ConfiguredAfter
            }
        }
        if ($null -eq $recovery -and
            $rollbackPlan.RecoveryCompletesRequest -and
            -not [string]::IsNullOrWhiteSpace($rollbackPlan.PendingTransactionId)) {
            $configAfterConcurrentRecovery = Read-CodexCliConfigSnapshot $config
            $stateAfterConcurrentRecovery = Read-CodexDevJsonFile $paths.State
            if ($configAfterConcurrentRecovery.Sha256 -eq $rollbackPlan.PendingConfigAfterSha256 -and
                (Test-CodexDevStateSnapshot `
                    -StateRead $stateAfterConcurrentRecovery `
                    -ExpectedExists $true `
                    -ExpectedValue $rollbackPlan.PendingStateAfter)) {
                $recovery = [pscustomobject]@{
                    Status = "already_recovered_after"
                    TransactionId = $rollbackPlan.PendingTransactionId
                    Action = "Rollback"
                    ConfiguredBefore = $rollbackPlan.PendingConfiguredBefore
                    ConfiguredAfter = $rollbackPlan.ConfiguredAfter
                    ConfigBackup = $rollbackPlan.PendingConfigBackup
                }
                return [pscustomobject]@{
                    Status = "previous_selected_for_restart"
                    ConfiguredBefore = $recovery.ConfiguredBefore
                    ConfiguredEntrypoint = $recovery.ConfiguredAfter
                    ConfigBackup = $recovery.ConfigBackup
                    Recovery = $recovery
                    RestartRequired = $env:CODEX_CLI_PATH -ne $recovery.ConfiguredAfter
                }
            }
        }
        $deploymentStatus = Get-CodexDevDeploymentStatusUnlocked `
            -ConfigPath $config `
            -DeploymentRoot $root
        if ($deploymentStatus.Status -ne "consistent") {
            throw (
                "Cannot roll back while config and deployment state are not settled: " +
                "$($deploymentStatus.Status). $($deploymentStatus.DriftReasons -join ' ')"
            )
        }
        $stateRead = Read-CodexDevJsonFile $paths.State
        if (-not $stateRead.Exists) {
            throw "No deployment state exists at $($paths.State)"
        }
        if ($null -ne $stateRead.Error) {
            throw "Deployment state could not be read: $($stateRead.Error)"
        }
        $state = $stateRead.Value
        $current = Get-CodexDevObjectProperty -Value $state -Name "Current"
        $previous = Get-CodexDevObjectProperty -Value $state -Name "Previous"
        $currentEntrypoint = [string](
            Get-CodexDevObjectProperty -Value $current -Name "Entrypoint"
        )
        $previousEntrypoint = [string](
            Get-CodexDevObjectProperty -Value $previous -Name "Entrypoint"
        )
        if ([string]::IsNullOrWhiteSpace($previousEntrypoint)) {
            throw "No previous Desktop override is recorded."
        }
        if (-not (Test-Path -LiteralPath $previousEntrypoint -PathType Leaf)) {
            throw "Previous Desktop override no longer exists: $previousEntrypoint"
        }
        $candidateValidation = Get-CodexDevRollbackCandidateValidation `
            -Candidate $previous
        if (-not $candidateValidation.Valid) {
            throw "Cannot roll back: $($candidateValidation.Error)"
        }

        $configTransition = Get-CodexCliConfigTransition `
            -ConfigPath $config `
            -Entrypoint $previousEntrypoint
        $configuredBefore = $configTransition.ConfiguredBefore
        if (-not (Test-CodexDevPathEqual -Left $configuredBefore -Right $currentEntrypoint)) {
            throw (
                "Cannot roll back while config and deployment state have drifted. " +
                "Configured: '$configuredBefore'; state current: '$currentEntrypoint'."
            )
        }
        $backupDirectory = Join-Path $root "config-backups"
        New-Item -ItemType Directory -Path $backupDirectory -Force | Out-Null
        $backupPath = Join-Path $backupDirectory (
            "$([DateTime]::UtcNow.ToString('yyyyMMdd-HHmmssfff'))-$([guid]::NewGuid().ToString('N'))-config.toml"
        )
        [System.IO.File]::WriteAllBytes($backupPath, [byte[]]$configTransition.BeforeBytes)
        $newState = New-CodexDevDeploymentState `
            -StateBefore $state `
            -ConfigPath $config `
            -Current $previous `
            -Previous $current `
            -LastConfigBackup $backupPath
        Invoke-CodexDevDeploymentTransaction `
            -Action "Rollback" `
            -ConfigPath $config `
            -DeploymentRoot $root `
            -ConfiguredBefore $configuredBefore `
            -ConfiguredAfter $previousEntrypoint `
            -ConfigTransition $configTransition `
            -ConfigBackup $backupPath `
            -StateBeforeExists $true `
            -StateBefore $state `
            -StateAfter $newState

        return [pscustomobject]@{
            Status = "previous_selected_for_restart"
            ConfiguredBefore = $configuredBefore
            ConfiguredEntrypoint = $previousEntrypoint
            ConfigBackup = $backupPath
            Recovery = $recovery
            RestartRequired = $env:CODEX_CLI_PATH -ne $previousEntrypoint
        }
        }
    }

    $receipt = $null
    $receiptError = $null
    try {
        $receipt = Write-CodexDevReceipt -Action "Rollback" -Details ([ordered]@{
            ConfigPath = $config
            ConfiguredBefore = $operation.ConfiguredBefore
            ConfiguredAfter = $operation.ConfiguredEntrypoint
            ConfigBackup = $operation.ConfigBackup
            LiveBefore = $env:CODEX_CLI_PATH
            Recovery = $operation.Recovery
        })
    } catch {
        $receiptError = $_.Exception.Message
    }
    return [pscustomobject]@{
        Status = $operation.Status
        ConfiguredEntrypoint = $operation.ConfiguredEntrypoint
        Receipt = $receipt
        ReceiptStatus = if ($null -eq $receiptError) { "written" } else { "failed_after_success" }
        ReceiptError = $receiptError
        RetryRequired = $false
        RetryGuidance = if ($null -ne $receiptError) {
            "Do not retry Rollback; the selection succeeded and only receipt recording failed."
        } else {
            $null
        }
        RestartRequired = $operation.RestartRequired
        Recovery = $operation.Recovery
    }
}
