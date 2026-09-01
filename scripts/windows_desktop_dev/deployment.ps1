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
    $persistentForPlan = $planDeployment.PersistentEntrypoint
    $configuredPreviousSettlement = if (
        $planDeployment.Status -eq "previous_configured_state_stale"
    ) {
        Get-CodexDevConfiguredPreviousSettlementPlan `
            -DeploymentStatus $planDeployment `
            -ConfigPath $config
    } else {
        $null
    }
    $planBlockers = @(
        if ($planDeployment.Status -notin @(
            "consistent",
            "unmanaged",
            "previous_configured_state_stale"
        )) {
            "Deployment is not settled: $($planDeployment.Status). " +
            ($planDeployment.DriftReasons -join " ")
        }
        if ($null -ne $configuredPreviousSettlement -and
            $configuredPreviousSettlement.Status -eq "blocked") {
            foreach ($settlementBlocker in $configuredPreviousSettlement.Blockers) {
                [string]$settlementBlocker
            }
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
            Status = if ($planBlockers.Count -gt 0) {
                "blocked"
            } elseif ($null -ne $configuredPreviousSettlement) {
                "planned_with_recovery"
            } else {
                "planned"
            }
            SourcePackage = $package.PackageRoot
            ReleasePath = $releasePath
            ConfigPath = $config
            ConfiguredBefore = $configuredForPlan
            ConfiguredAfter = $entrypoint
            PersistentBefore = $persistentForPlan
            PersistentAfter = $entrypoint
            DeploymentStatus = $planDeployment.Status
            RecoveryDisposition = if ($null -ne $configuredPreviousSettlement) {
                "settle_configured_previous"
            } else {
                "none"
            }
            ConfiguredPreviousSettlement = $configuredPreviousSettlement
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
        if ($deploymentStatus.Status -eq "previous_configured_state_stale") {
            $freshSettlementPlan = Get-CodexDevConfiguredPreviousSettlementPlan `
                -DeploymentStatus $deploymentStatus `
                -ConfigPath $config
            if ($freshSettlementPlan.Status -eq "blocked") {
                throw (
                    "Cannot deploy after settling the configured Previous entrypoint: " +
                    ($freshSettlementPlan.Blockers -join " ")
                )
            }
            $recovery = Complete-CodexDevConfiguredPreviousSettlement `
                -Action "Deploy" `
                -SettlementPlan $freshSettlementPlan `
                -ConfigPath $config `
                -DeploymentRoot $root
            $deploymentStatus = Get-CodexDevDeploymentStatusUnlocked `
                -ConfigPath $config `
                -DeploymentRoot $root
        }
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
        $persistentBefore = Get-CodexDevPersistentSelector
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
            (Test-CodexDevPathEqual -Left $configuredBefore -Right $entrypoint) -and
            (Test-CodexDevPathEqual -Left $persistentBefore -Right $entrypoint)) {
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
            Invoke-CodexDevDeploymentTransaction `
                -Action "Deploy" `
                -ConfigPath $config `
                -DeploymentRoot $root `
                -ConfiguredBefore $configuredBefore `
                -ConfiguredAfter $entrypoint `
                -PersistentBefore $persistentBefore `
                -PersistentAfter $entrypoint `
                -ConfigTransition $configTransition `
                -ConfigBackup $null `
                -StateBeforeExists $false `
                -StateBefore $null `
                -StateAfter $adoptedState
            return [pscustomobject]@{
                Status = "adopted_existing_selection"
                Release = $adoptedCurrent
                Previous = $null
                ConfigPath = $config
                ConfiguredBefore = $configuredBefore
                PersistentBefore = $persistentBefore
                PersistentEntrypoint = $entrypoint
                ConfigBackup = $null
                RestartRequired = Test-CodexDevRestartRequired `
                    -ProcessLiveEntrypoint $env:CODEX_CLI_PATH `
                    -PersistentEntrypoint $entrypoint
                Recovery = $recovery
            }
        }
        if ((Test-CodexDevPathEqual -Left $currentBeforeEntrypoint -Right $entrypoint) -and
            (Test-CodexDevPathEqual -Left $configuredBefore -Right $entrypoint) -and
            (Test-CodexDevPathEqual -Left $persistentBefore -Right $entrypoint) -and
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
                PersistentBefore = $persistentBefore
                PersistentEntrypoint = $entrypoint
                ConfigBackup = if ($null -ne $recovery) {
                    $recovery.ConfigBackup
                } else {
                    [string](Get-CodexDevObjectProperty `
                        -Value $stateBefore `
                        -Name "LastConfigBackup")
                }
                RestartRequired = Test-CodexDevRestartRequired `
                    -ProcessLiveEntrypoint $env:CODEX_CLI_PATH `
                    -PersistentEntrypoint $entrypoint
                Recovery = $recovery
            }
        }

        if ((Test-CodexDevPathEqual -Left $currentBeforeEntrypoint -Right $entrypoint) -and
            (Test-CodexDevPathEqual -Left $configuredBefore -Right $entrypoint) -and
            (Test-CodexDevPathEqual -Left $persistentBefore -Right $entrypoint)) {
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
            Invoke-CodexDevDeploymentTransaction `
                -Action "Deploy" `
                -ConfigPath $config `
                -DeploymentRoot $root `
                -ConfiguredBefore $configuredBefore `
                -ConfiguredAfter $entrypoint `
                -PersistentBefore $persistentBefore `
                -PersistentAfter $entrypoint `
                -ConfigTransition $configTransition `
                -ConfigBackup $null `
                -StateBeforeExists $true `
                -StateBefore $stateBefore `
                -StateAfter $selectionOnlyState
            return [pscustomobject]@{
                Status = "selection_recorded"
                Release = $selectionOnlyCurrent
                Previous = $selectionOnlyState.Previous
                ConfigPath = $config
                ConfiguredBefore = $configuredBefore
                PersistentBefore = $persistentBefore
                PersistentEntrypoint = $entrypoint
                ConfigBackup = $selectionOnlyState.LastConfigBackup
                RestartRequired = Test-CodexDevRestartRequired `
                    -ProcessLiveEntrypoint $env:CODEX_CLI_PATH `
                    -PersistentEntrypoint $entrypoint
                Recovery = $recovery
            }
        }

        $previous = if ((Test-CodexDevPathEqual `
            -Left $currentBeforeEntrypoint `
            -Right $configuredBefore) -and
            (Test-CodexDevPathEqual -Left $currentBeforeEntrypoint -Right $persistentBefore) -and
            $null -ne $currentBefore) {
            $currentBefore
        } elseif (-not [string]::IsNullOrWhiteSpace($persistentBefore)) {
            [pscustomobject]@{
                ReleaseId = "pre-lane"
                Entrypoint = $persistentBefore
                Fingerprint = $null
            }
        } else {
            $null
        }
        $backupPath = $null
        if ($configTransition.BeforeSha256 -ne $configTransition.AfterSha256) {
            $backupDirectory = Join-Path $root "config-backups"
            New-Item -ItemType Directory -Path $backupDirectory -Force | Out-Null
            $backupPath = Join-Path $backupDirectory (
                "$([DateTime]::UtcNow.ToString('yyyyMMdd-HHmmssfff'))-$([guid]::NewGuid().ToString('N'))-config.toml"
            )
            [System.IO.File]::WriteAllBytes($backupPath, [byte[]]$configTransition.BeforeBytes)
        }

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
            -PersistentBefore $persistentBefore `
            -PersistentAfter $entrypoint `
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
            PersistentBefore = $persistentBefore
            PersistentEntrypoint = $entrypoint
            ConfigBackup = $backupPath
            RestartRequired = Test-CodexDevRestartRequired `
                -ProcessLiveEntrypoint $env:CODEX_CLI_PATH `
                -PersistentEntrypoint $entrypoint
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
            PersistentEntrypoint = $operation.PersistentEntrypoint
            ConfiguredEntrypoint = $operation.Release.Entrypoint
            ProcessLiveEntrypoint = $env:CODEX_CLI_PATH
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
            ConfiguredAfter = $operation.Release.Entrypoint
            PersistentBefore = $operation.PersistentBefore
            PersistentAfter = $operation.PersistentEntrypoint
            ConfigBackup = $operation.ConfigBackup
            ProcessLiveAtOperation = $env:CODEX_CLI_PATH
            ProcessLiveProofBoundary = (
                "Inherited process selector snapshot; it does not attest the loaded Desktop binary."
            )
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
        PersistentEntrypoint = $operation.PersistentEntrypoint
        ConfiguredEntrypoint = $operation.Release.Entrypoint
        ProcessLiveEntrypoint = $env:CODEX_CLI_PATH
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

function Get-CodexDevClosingVerificationSnapshot {
    param(
        [string]$ConfigPath,
        [string]$DeploymentRoot,
        [object]$InitialDeployment,
        [System.Collections.Generic.List[string]]$VerificationDrift
    )

    $verifiedDeployment = Get-CodexDevDeploymentStatus `
        -ConfigPath $ConfigPath `
        -DeploymentRoot $DeploymentRoot
    if ($verifiedDeployment.Status -ne "consistent") {
        $VerificationDrift.Add(
            "Deployment planes were not settled at the closing verification read: " +
            "$($verifiedDeployment.Status)."
        )
    }
    if ($verifiedDeployment.ConfigSha256 -ne $InitialDeployment.ConfigSha256) {
        $VerificationDrift.Add("Config changed during deployment verification.")
    }
    if (-not (Test-CodexDevSelectorExact `
            -Left $verifiedDeployment.PersistentEntrypoint `
            -Right $InitialDeployment.PersistentEntrypoint)) {
        $VerificationDrift.Add("Persistent selector changed during deployment verification.")
    }
    if (-not (Test-CodexDevSelectorExact `
            -Left $verifiedDeployment.ConfiguredEntrypoint `
            -Right $InitialDeployment.ConfiguredEntrypoint)) {
        $VerificationDrift.Add("Configured selector changed during deployment verification.")
    }
    $verifiedState = [pscustomobject]@{
        Exists = [bool]$verifiedDeployment.StateExists
        Value = Get-CodexDevObjectProperty -Value $verifiedDeployment -Name "State"
        Error = $null
    }
    if (-not (Test-CodexDevStateSnapshot `
            -StateRead $verifiedState `
            -ExpectedExists ([bool]$InitialDeployment.StateExists) `
            -ExpectedValue $InitialDeployment.State)) {
        $VerificationDrift.Add("Deployment state changed during deployment verification.")
    }
    return $verifiedDeployment
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
    $persistent = $deployment.PersistentEntrypoint
    $processLive = $env:CODEX_CLI_PATH
    if ($deployment.Status -ne "consistent") {
        return [pscustomobject]@{
            Status = $deployment.Status
            ConfigPath = $ConfigPath
            PersistentEntrypoint = $persistent
            ConfiguredEntrypoint = $configured
            ProcessLiveEntrypoint = $processLive
            LiveEntrypoint = $processLive
            Package = $null
            SelectedOccurrence = $null
            StagedReleaseProvenance = $null
            VerificationMode = "unsettled"
            ProofBoundary = (
                "Deployment settlement is required before package verification. ProcessLiveEntrypoint " +
                "is only this process's inherited selector snapshot."
            )
            RestartRequired = Test-CodexDevRestartRequired `
                -ProcessLiveEntrypoint $processLive `
                -PersistentEntrypoint $persistent
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
    $persistentSnapshot = Read-CodexDevPersistentSelector
    if ($null -ne $configSnapshot.Error -or
        $configSnapshot.Sha256 -ne $deployment.ConfigSha256 -or
        $null -ne $persistentSnapshot.Error -or
        -not (Test-CodexDevSelectorExact `
            -Left $persistentSnapshot.Value `
            -Right $persistent)) {
        return [pscustomobject]@{
            Status = "drift"
            ConfigPath = $ConfigPath
            PersistentEntrypoint = $persistent
            ConfiguredEntrypoint = $configured
            ProcessLiveEntrypoint = $processLive
            LiveEntrypoint = $processLive
            Package = $null
            SelectedOccurrence = $null
            StagedReleaseProvenance = $null
            VerificationMode = "durable_selector_snapshot"
            ProofBoundary = (
                "Persistent selector or config mirror changed before package verification could begin."
            )
            RestartRequired = Test-CodexDevRestartRequired `
                -ProcessLiveEntrypoint $processLive `
                -PersistentEntrypoint $persistent
            DeploymentStatus = "drift"
            PendingDisposition = $deployment.PendingDisposition
            DriftReasons = @("Persistent selector or config mirror changed during verification.")
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
        $verifiedDeployment = Get-CodexDevClosingVerificationSnapshot `
            -ConfigPath $ConfigPath `
            -DeploymentRoot $DeploymentRoot `
            -InitialDeployment $deployment `
            -VerificationDrift $verificationDrift
        $effectiveDeploymentStatus = if ($verificationDrift.Count -gt 0) {
            "drift"
        } else {
            "consistent"
        }
        return [pscustomobject]@{
            Status = if ($effectiveDeploymentStatus -eq "consistent") {
                if (-not (Test-CodexDevRestartRequired `
                    -ProcessLiveEntrypoint $processLive `
                    -PersistentEntrypoint $persistent)) { "live" } else { "restart_required" }
            } else {
                $effectiveDeploymentStatus
            }
            ConfigPath = $ConfigPath
            PersistentEntrypoint = $persistent
            ConfiguredEntrypoint = $configured
            ProcessLiveEntrypoint = $processLive
            LiveEntrypoint = $processLive
            Package = $null
            SelectedOccurrence = $null
            StagedReleaseProvenance = $null
            VerificationMode = "pre_lane_selector_only"
            ProofBoundary = (
                "Selector existence only; pre-lane package integrity, PE targets, and provenance " +
                "are not attested. ProcessLiveEntrypoint is an inherited selector snapshot, not " +
                "attestation of the loaded Desktop binary."
            )
            RestartRequired = Test-CodexDevRestartRequired `
                -ProcessLiveEntrypoint $processLive `
                -PersistentEntrypoint $persistent
            DeploymentStatus = $effectiveDeploymentStatus
            PendingDisposition = $verifiedDeployment.PendingDisposition
            DriftReasons = @($verificationDrift.ToArray())
            Deployment = $verifiedDeployment
            DeploymentSettled = $effectiveDeploymentStatus -eq "consistent"
        }
    }
    if ($hasFingerprint -ne $hasReleasePath) {
        return [pscustomobject]@{
            Status = "drift"
            ConfigPath = $ConfigPath
            PersistentEntrypoint = $persistent
            ConfiguredEntrypoint = $configured
            ProcessLiveEntrypoint = $processLive
            LiveEntrypoint = $processLive
            Package = $null
            SelectedOccurrence = $selection
            StagedReleaseProvenance = $null
            VerificationMode = "managed_package"
            ProofBoundary = (
                "Managed deployment state must record both ReleasePath and Fingerprint. " +
                "ProcessLiveEntrypoint remains only an inherited selector snapshot."
            )
            RestartRequired = Test-CodexDevRestartRequired `
                -ProcessLiveEntrypoint $processLive `
                -PersistentEntrypoint $persistent
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
    $verifiedDeployment = Get-CodexDevClosingVerificationSnapshot `
        -ConfigPath $ConfigPath `
        -DeploymentRoot $DeploymentRoot `
        -InitialDeployment $deployment `
        -VerificationDrift $verificationDrift
    $effectiveDeploymentStatus = if ($verificationDrift.Count -gt 0) {
        "drift"
    } else {
        "consistent"
    }
    $status = if ($effectiveDeploymentStatus -eq "consistent") {
        if (-not (Test-CodexDevRestartRequired `
            -ProcessLiveEntrypoint $processLive `
            -PersistentEntrypoint $persistent)) { "live" } else { "restart_required" }
    } else {
        $effectiveDeploymentStatus
    }
    return [pscustomobject]@{
        Status = $status
        ConfigPath = $ConfigPath
        PersistentEntrypoint = $persistent
        ConfiguredEntrypoint = $configured
        ProcessLiveEntrypoint = $processLive
        LiveEntrypoint = $processLive
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
            "and executable smoke are verified. ProcessLiveEntrypoint equality proves only that " +
            "this process inherited the selected path; it does not attest the loaded Desktop binary."
        )
        RestartRequired = Test-CodexDevRestartRequired `
            -ProcessLiveEntrypoint $processLive `
            -PersistentEntrypoint $persistent
        DeploymentStatus = $effectiveDeploymentStatus
        PendingDisposition = $verifiedDeployment.PendingDisposition
        DriftReasons = @($verificationDrift.ToArray())
        Deployment = $verifiedDeployment
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

function Get-CodexDevConfiguredPreviousSettlementPlan {
    param(
        [object]$DeploymentStatus,
        [string]$ConfigPath
    )

    $blockers = [System.Collections.Generic.List[string]]::new()
    if ($DeploymentStatus.Status -ne "previous_configured_state_stale") {
        $blockers.Add(
            "Deployment does not have the recorded Previous entrypoint configured: " +
            "$($DeploymentStatus.Status)."
        )
    }
    $stateBefore = $DeploymentStatus.State
    $currentBefore = Get-CodexDevObjectProperty -Value $stateBefore -Name "Current"
    $previousBefore = Get-CodexDevObjectProperty -Value $stateBefore -Name "Previous"
    $lastConfigBackup = Get-CodexDevObjectProperty `
        -Value $stateBefore `
        -Name "LastConfigBackup"
    $stateAfter = if ($blockers.Count -eq 0) {
        New-CodexDevDeploymentState `
            -StateBefore $stateBefore `
            -ConfigPath $ConfigPath `
            -Current $previousBefore `
            -Previous $currentBefore `
            -LastConfigBackup $lastConfigBackup
    } else {
        $null
    }
    if ($blockers.Count -eq 0 -and
        -not (Test-CodexDevStateSchema -State $stateAfter -ConfigPath $ConfigPath)) {
        $blockers.Add("Projected deployment state is invalid.")
    }
    $configured = $DeploymentStatus.ConfiguredEntrypoint
    $persistent = $DeploymentStatus.PersistentEntrypoint
    $projectedCurrent = Get-CodexDevObjectProperty -Value $stateAfter -Name "Current"
    $projectedEntrypoint = [string](
        Get-CodexDevObjectProperty -Value $projectedCurrent -Name "Entrypoint"
    )
    if ($blockers.Count -eq 0 -and
        -not (Test-CodexDevPathEqual -Left $configured -Right $projectedEntrypoint)) {
        $blockers.Add("Projected deployment state does not match the configured entrypoint.")
    }
    if ($blockers.Count -eq 0 -and
        -not (Test-CodexDevPathEqual -Left $persistent -Right $projectedEntrypoint)) {
        $blockers.Add("Projected deployment state does not match the persistent selector.")
    }
    return [pscustomobject]@{
        Status = if ($blockers.Count -eq 0) { "ready" } else { "blocked" }
        ConfigPath = $ConfigPath
        ConfiguredEntrypoint = $configured
        PersistentEntrypoint = $persistent
        ConfigSha256 = $DeploymentStatus.ConfigSha256
        StateBefore = $stateBefore
        StateAfter = $stateAfter
        Blockers = @($blockers.ToArray())
    }
}

function Complete-CodexDevConfiguredPreviousSettlement {
    param(
        [ValidateSet("Deploy", "Rollback")]
        [string]$Action,
        [object]$SettlementPlan,
        [string]$ConfigPath,
        [string]$DeploymentRoot
    )

    if ($SettlementPlan.Status -ne "ready") {
        throw "Cannot settle the configured Previous entrypoint: $($SettlementPlan.Blockers -join ' ')"
    }
    $paths = Get-CodexDevDeploymentPaths $DeploymentRoot
    $configBeforeSettlement = Read-CodexCliConfigSnapshot $ConfigPath
    $persistentBeforeSettlement = Get-CodexDevPersistentSelector
    $stateBeforeSettlement = Read-CodexDevJsonFile $paths.State
    $alreadySettled = $configBeforeSettlement.Sha256 -eq $SettlementPlan.ConfigSha256 -and
        (Test-CodexDevSelectorExact `
            -Left $persistentBeforeSettlement `
            -Right $SettlementPlan.PersistentEntrypoint) -and
        (Test-CodexDevStateSnapshot `
            -StateRead $stateBeforeSettlement `
            -ExpectedExists $true `
            -ExpectedValue $SettlementPlan.StateAfter)
    if ($alreadySettled) {
        return [pscustomobject]@{
            Status = "configured_previous_already_settled"
            Action = "StateSettlement"
            ConfiguredBefore = $SettlementPlan.ConfiguredEntrypoint
            ConfiguredAfter = $SettlementPlan.ConfiguredEntrypoint
            ConfiguredEntrypoint = $SettlementPlan.ConfiguredEntrypoint
            PersistentBefore = $SettlementPlan.PersistentEntrypoint
            PersistentAfter = $SettlementPlan.PersistentEntrypoint
            PersistentEntrypoint = $SettlementPlan.PersistentEntrypoint
            ConfigBackup = $null
            RetainedLastConfigBackup = Get-CodexDevObjectProperty `
                -Value $SettlementPlan.StateAfter `
                -Name "LastConfigBackup"
        }
    }
    $settlementStatus = Get-CodexDevDeploymentStatusUnlocked `
        -ConfigPath $ConfigPath `
        -DeploymentRoot $DeploymentRoot
    if ($settlementStatus.Status -ne "previous_configured_state_stale") {
        throw (
            "Cannot settle the configured Previous entrypoint because deployment state changed: " +
            "$($settlementStatus.Status). $($settlementStatus.DriftReasons -join ' ')"
        )
    }
    if ($configBeforeSettlement.Sha256 -ne $SettlementPlan.ConfigSha256 -or
        -not (Test-CodexDevSelectorExact `
            -Left $persistentBeforeSettlement `
            -Right $SettlementPlan.PersistentEntrypoint) -or
        -not (Test-CodexDevStateSnapshot `
            -StateRead $stateBeforeSettlement `
            -ExpectedExists $true `
            -ExpectedValue $SettlementPlan.StateBefore)) {
        throw "Config or deployment state changed while previous-state settlement was prepared."
    }
    $configTransition = Get-CodexCliConfigTransition `
        -ConfigPath $ConfigPath `
        -Entrypoint $SettlementPlan.ConfiguredEntrypoint
    if ($configTransition.BeforeSha256 -ne $SettlementPlan.ConfigSha256 -or
        $configTransition.AfterSha256 -ne $SettlementPlan.ConfigSha256) {
        throw "Config changed before previous-state settlement could be journaled."
    }
    Invoke-CodexDevDeploymentTransaction `
        -Action $Action `
        -ConfigPath $ConfigPath `
        -DeploymentRoot $DeploymentRoot `
        -ConfiguredBefore $SettlementPlan.ConfiguredEntrypoint `
        -ConfiguredAfter $SettlementPlan.ConfiguredEntrypoint `
        -PersistentBefore $SettlementPlan.PersistentEntrypoint `
        -PersistentAfter $SettlementPlan.PersistentEntrypoint `
        -ConfigTransition $configTransition `
        -ConfigBackup $null `
        -StateBeforeExists $true `
        -StateBefore $SettlementPlan.StateBefore `
        -StateAfter $SettlementPlan.StateAfter

    return [pscustomobject]@{
        Status = "configured_previous_settled"
        Action = "StateSettlement"
        ConfiguredBefore = $SettlementPlan.ConfiguredEntrypoint
        ConfiguredAfter = $SettlementPlan.ConfiguredEntrypoint
        ConfiguredEntrypoint = $SettlementPlan.ConfiguredEntrypoint
        PersistentBefore = $SettlementPlan.PersistentEntrypoint
        PersistentAfter = $SettlementPlan.PersistentEntrypoint
        PersistentEntrypoint = $SettlementPlan.PersistentEntrypoint
        ConfigBackup = $null
        RetainedLastConfigBackup = Get-CodexDevObjectProperty `
            -Value $SettlementPlan.StateAfter `
            -Name "LastConfigBackup"
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
    $persistent = $null
    $recoveryDisposition = "none"
    $recoveryCompletesRequest = $false
    $pendingTransactionId = $null
    $pendingStateAfter = $null
    $pendingConfigAfterSha256 = $null
    $pendingConfiguredBefore = $null
    $pendingPersistentBefore = $null
    $pendingConfigBackup = $null
    $configuredPreviousSettlement = $null

    switch ($DeploymentStatus.Status) {
        "consistent" {
            $state = $DeploymentStatus.State
            $configured = $DeploymentStatus.ConfiguredEntrypoint
            $persistent = $DeploymentStatus.PersistentEntrypoint
        }
        "pending_recoverable_before" {
            $pending = $DeploymentStatus.PendingTransaction
            $recoveryDisposition = "recover_before"
            $pendingTransactionId = [string]$pending.TransactionId
            $pendingStateAfter = $pending.StateAfter
            $pendingConfigAfterSha256 = [string]$pending.ConfigAfterSha256
            $pendingConfiguredBefore = [string]$pending.ConfiguredBefore
            $pendingPersistentBefore = $pending.PersistentBefore
            $pendingConfigBackup = [string]$pending.ConfigBackup
            $configured = [string]$pending.ConfiguredBefore
            $persistent = $pending.PersistentBefore
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
            $pendingPersistentBefore = $pending.PersistentBefore
            $pendingConfigBackup = [string]$pending.ConfigBackup
            $configured = [string]$pending.ConfiguredAfter
            $persistent = [string]$pending.PersistentAfter
            $state = $pending.StateAfter
            $recoveryCompletesRequest = [string]$pending.Action -eq "Rollback"
        }
        "previous_configured_state_stale" {
            $configuredPreviousSettlement = Get-CodexDevConfiguredPreviousSettlementPlan `
                -DeploymentStatus $DeploymentStatus `
                -ConfigPath $ConfigPath
            if ($configuredPreviousSettlement.Status -eq "blocked") {
                foreach ($settlementBlocker in $configuredPreviousSettlement.Blockers) {
                    $blockers.Add([string]$settlementBlocker)
                }
            } else {
                $state = $configuredPreviousSettlement.StateAfter
                $configured = $DeploymentStatus.ConfiguredEntrypoint
                $persistent = $DeploymentStatus.PersistentEntrypoint
                $recoveryDisposition = "settle_configured_previous"
                $recoveryCompletesRequest = $true
            }
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
    if ($blockers.Count -eq 0 -and
        -not (Test-CodexDevPathEqual -Left $persistent -Right $currentEntrypoint)) {
        $blockers.Add("Persistent selector and deployment state will not agree after recovery.")
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
        PersistentBefore = $persistent
        PersistentAfter = if ($recoveryCompletesRequest) {
            $persistent
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
        PendingPersistentBefore = $pendingPersistentBefore
        PendingConfigBackup = $pendingConfigBackup
        ConfiguredPreviousSettlement = $configuredPreviousSettlement
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
                PersistentBefore = $recovery.PersistentBefore
                PersistentEntrypoint = $recovery.PersistentAfter
                ConfigBackup = $recovery.ConfigBackup
                Recovery = $recovery
                RestartRequired = Test-CodexDevRestartRequired `
                    -ProcessLiveEntrypoint $env:CODEX_CLI_PATH `
                    -PersistentEntrypoint $recovery.PersistentAfter
            }
        }
        if ($rollbackPlan.RecoveryDisposition -eq "settle_configured_previous") {
            $settlementCandidate = Get-CodexDevRollbackCandidateValidation `
                -Candidate $rollbackPlan.Current
            if (-not $settlementCandidate.Valid) {
                throw "Cannot settle Rollback to configured Previous: $($settlementCandidate.Error)"
            }
            $recovery = Complete-CodexDevConfiguredPreviousSettlement `
                -Action "Rollback" `
                -SettlementPlan $rollbackPlan.ConfiguredPreviousSettlement `
                -ConfigPath $config `
                -DeploymentRoot $root
            return [pscustomobject]@{
                Status = "configured_previous_settled"
                ConfiguredBefore = $rollbackPlan.ConfiguredBefore
                ConfiguredEntrypoint = $rollbackPlan.ConfiguredAfter
                PersistentBefore = $rollbackPlan.PersistentBefore
                PersistentEntrypoint = $rollbackPlan.PersistentAfter
                ConfigBackup = $null
                Recovery = $recovery
                RestartRequired = Test-CodexDevRestartRequired `
                    -ProcessLiveEntrypoint $env:CODEX_CLI_PATH `
                    -PersistentEntrypoint $rollbackPlan.PersistentAfter
            }
        }
        if ($null -eq $recovery -and
            $rollbackPlan.RecoveryCompletesRequest -and
            -not [string]::IsNullOrWhiteSpace($rollbackPlan.PendingTransactionId)) {
            $configAfterConcurrentRecovery = Read-CodexCliConfigSnapshot $config
            $persistentAfterConcurrentRecovery = Get-CodexDevPersistentSelector
            $stateAfterConcurrentRecovery = Read-CodexDevJsonFile $paths.State
            if ($configAfterConcurrentRecovery.Sha256 -eq $rollbackPlan.PendingConfigAfterSha256 -and
                (Test-CodexDevSelectorExact `
                    -Left $persistentAfterConcurrentRecovery `
                    -Right $rollbackPlan.PersistentAfter) -and
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
                    PersistentBefore = $rollbackPlan.PendingPersistentBefore
                    PersistentAfter = $rollbackPlan.PersistentAfter
                    ConfigBackup = $rollbackPlan.PendingConfigBackup
                }
                return [pscustomobject]@{
                    Status = "previous_selected_for_restart"
                    ConfiguredBefore = $recovery.ConfiguredBefore
                    ConfiguredEntrypoint = $recovery.ConfiguredAfter
                    PersistentBefore = $recovery.PersistentBefore
                    PersistentEntrypoint = $recovery.PersistentAfter
                    ConfigBackup = $recovery.ConfigBackup
                    Recovery = $recovery
                    RestartRequired = Test-CodexDevRestartRequired `
                        -ProcessLiveEntrypoint $env:CODEX_CLI_PATH `
                        -PersistentEntrypoint $recovery.PersistentAfter
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
        $persistentBefore = Get-CodexDevPersistentSelector
        if (-not (Test-CodexDevPathEqual -Left $configuredBefore -Right $currentEntrypoint) -or
            -not (Test-CodexDevPathEqual -Left $persistentBefore -Right $currentEntrypoint)) {
            throw (
                "Cannot roll back while persistent selector, config mirror, and state have drifted. " +
                "Persistent: '$persistentBefore'; configured: '$configuredBefore'; " +
                "state current: '$currentEntrypoint'."
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
            -PersistentBefore $persistentBefore `
            -PersistentAfter $previousEntrypoint `
            -ConfigTransition $configTransition `
            -ConfigBackup $backupPath `
            -StateBeforeExists $true `
            -StateBefore $state `
            -StateAfter $newState

        return [pscustomobject]@{
            Status = "previous_selected_for_restart"
            ConfiguredBefore = $configuredBefore
            ConfiguredEntrypoint = $previousEntrypoint
            PersistentBefore = $persistentBefore
            PersistentEntrypoint = $previousEntrypoint
            ConfigBackup = $backupPath
            Recovery = $recovery
            RestartRequired = Test-CodexDevRestartRequired `
                -ProcessLiveEntrypoint $env:CODEX_CLI_PATH `
                -PersistentEntrypoint $previousEntrypoint
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
            PersistentBefore = $operation.PersistentBefore
            PersistentAfter = $operation.PersistentEntrypoint
            ConfigBackup = $operation.ConfigBackup
            ProcessLiveAtOperation = $env:CODEX_CLI_PATH
            ProcessLiveProofBoundary = (
                "Inherited process selector snapshot; it does not attest the loaded Desktop binary."
            )
            Recovery = $operation.Recovery
        })
    } catch {
        $receiptError = $_.Exception.Message
    }
    return [pscustomobject]@{
        Status = $operation.Status
        PersistentEntrypoint = $operation.PersistentEntrypoint
        ConfiguredEntrypoint = $operation.ConfiguredEntrypoint
        ProcessLiveEntrypoint = $env:CODEX_CLI_PATH
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
