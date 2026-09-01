$script:CodexDevDeploymentFaultInjector = $null

function Restore-CodexDevConfigBackup {
    param(
        [string]$ConfigPath,
        [string]$BackupPath,
        [string]$ExpectedCurrentSha256,
        [string]$ExpectedBackupSha256
    )

    if (-not (Test-Path -LiteralPath $BackupPath -PathType Leaf)) {
        throw "Config backup does not exist: $BackupPath"
    }
    $current = Read-CodexCliConfigSnapshot $ConfigPath
    if ($null -ne $current.Error -or $current.Sha256 -ne $ExpectedCurrentSha256) {
        throw "Config no longer matches the transaction image selected for restoration."
    }
    $backupBytes = [System.IO.File]::ReadAllBytes($BackupPath)
    if ((Get-CodexDevSha256Bytes $backupBytes) -ne $ExpectedBackupSha256) {
        throw "Config backup does not match the pending Before image."
    }
    $tempPath = "$ConfigPath.restore.$([guid]::NewGuid().ToString('N'))"
    try {
        [System.IO.File]::WriteAllBytes($tempPath, $backupBytes)
        Move-Item -LiteralPath $tempPath -Destination $ConfigPath -Force
    } finally {
        Remove-Item -LiteralPath $tempPath -Force -ErrorAction SilentlyContinue
    }
}

function Restore-CodexDevStateSnapshot {
    param(
        [string]$StatePath,
        [bool]$StateExists,
        [AllowNull()][object]$StateValue
    )

    if ($StateExists) {
        Write-CodexDevJson -Path $StatePath -Value $StateValue
    } else {
        Remove-Item -LiteralPath $StatePath -Force -ErrorAction SilentlyContinue
    }
}

function Restore-CodexDevPersistentSelector {
    param(
        [AllowNull()][string]$ExpectedCurrent,
        [AllowNull()][string]$SelectorBefore
    )

    $current = Get-CodexDevPersistentSelector
    if (-not (Test-CodexDevSelectorExact -Left $current -Right $ExpectedCurrent)) {
        throw "Persistent selector no longer matches the transaction image selected for restoration."
    }
    $null = Set-CodexDevPersistentSelector -Entrypoint $SelectorBefore
}

function Invoke-CodexDevDeploymentFault {
    param(
        [string]$Stage,
        [object]$Transaction
    )

    if ($null -eq $script:CodexDevDeploymentFaultInjector) {
        return
    }
    $result = @(& $script:CodexDevDeploymentFaultInjector -Stage $Stage -Transaction $Transaction)
    if ($result.Count -gt 0 -and [string]$result[-1] -eq "Interrupt") {
        $exception = [System.InvalidOperationException]::new(
            "Simulated interruption at deployment transaction stage $Stage."
        )
        $exception.Data["CodexDevDeploymentInterrupted"] = $true
        throw $exception
    }
}

function Invoke-CodexDevDeploymentTransaction {
    param(
        [string]$Action,
        [string]$ConfigPath,
        [string]$DeploymentRoot,
        [AllowNull()][string]$ConfiguredBefore,
        [string]$ConfiguredAfter,
        [AllowNull()][string]$PersistentBefore,
        [string]$PersistentAfter,
        [object]$ConfigTransition,
        [AllowNull()][string]$ConfigBackup,
        [bool]$StateBeforeExists,
        [AllowNull()][object]$StateBefore,
        [object]$StateAfter
    )

    if (-not (Test-CodexDevPathEqual -Left $ConfiguredAfter -Right $PersistentAfter)) {
        throw "Journaled deployment transitions require matching persistent and config After selectors."
    }
    $configChanged = [string]$ConfigTransition.BeforeSha256 -ne
        [string]$ConfigTransition.AfterSha256
    $persistentChanged = -not (Test-CodexDevSelectorExact `
        -Left $PersistentBefore `
        -Right $PersistentAfter)
    if ($configChanged -and [string]::IsNullOrWhiteSpace($ConfigBackup)) {
        throw "A changed config image requires an exact Before backup."
    }
    $paths = Get-CodexDevDeploymentPaths $DeploymentRoot
    $configAtPrepare = Read-CodexCliConfigSnapshot $ConfigPath
    $persistentAtPrepare = Get-CodexDevPersistentSelector
    $stateAtPrepare = Read-CodexDevJsonFile $paths.State
    if ($null -ne $configAtPrepare.Error -or
        $configAtPrepare.Sha256 -ne [string]$ConfigTransition.BeforeSha256 -or
        -not (Test-CodexDevSelectorExact `
            -Left $persistentAtPrepare `
            -Right $PersistentBefore) -or
        -not (Test-CodexDevStateSnapshot `
            -StateRead $stateAtPrepare `
            -ExpectedExists $StateBeforeExists `
            -ExpectedValue $StateBefore)) {
        throw "Persistent selector, config mirror, or state changed before the transaction was journaled."
    }
    $transaction = [ordered]@{
        SchemaVersion = 2
        TransactionId = [guid]::NewGuid().ToString("N")
        Action = $Action
        CreatedAtUtc = [DateTime]::UtcNow.ToString("o")
        ConfigPath = [System.IO.Path]::GetFullPath($ConfigPath)
        ConfiguredBefore = $ConfiguredBefore
        ConfiguredAfter = $ConfiguredAfter
        PersistentBefore = $PersistentBefore
        PersistentAfter = $PersistentAfter
        ConfigBeforeSha256 = [string]$ConfigTransition.BeforeSha256
        ConfigAfterSha256 = [string]$ConfigTransition.AfterSha256
        ConfigBackup = $ConfigBackup
        StateBeforeExists = $StateBeforeExists
        StateBefore = $StateBefore
        StateAfter = $StateAfter
    }
    Write-CodexDevJson -Path $paths.Pending -Value $transaction

    try {
        Invoke-CodexDevDeploymentFault -Stage "AfterPending" -Transaction $transaction
        if ($configChanged) {
            Set-CodexCliPathInConfig `
                -ConfigPath $ConfigPath `
                -Entrypoint $ConfiguredAfter `
                -Transition $ConfigTransition
        }
        Invoke-CodexDevDeploymentFault -Stage "AfterConfig" -Transaction $transaction
        if ($persistentChanged) {
            $null = Set-CodexDevPersistentSelector -Entrypoint $PersistentAfter
        }
        Invoke-CodexDevDeploymentFault `
            -Stage "AfterPersistentSelector" `
            -Transaction $transaction
        Write-CodexDevJson -Path $paths.State -Value $StateAfter
        Invoke-CodexDevDeploymentFault -Stage "AfterState" -Transaction $transaction

        $finalConfig = Read-CodexCliConfigSnapshot $ConfigPath
        $finalPersistent = Get-CodexDevPersistentSelector
        $finalState = Read-CodexDevJsonFile $paths.State
        $finalConfigMatches = $null -eq $finalConfig.Error -and
            $finalConfig.Sha256 -eq [string]$ConfigTransition.AfterSha256
        $finalPersistentMatches = Test-CodexDevSelectorExact `
                -Left $finalPersistent `
                -Right $PersistentAfter
        $finalStateMatches = Test-CodexDevStateSnapshot `
                -StateRead $finalState `
                -ExpectedExists $true `
                -ExpectedValue $StateAfter
        if (-not $finalConfigMatches -or
            -not $finalPersistentMatches -or
            -not $finalStateMatches) {
            throw (
                "Deployment transaction final readback did not prove the persistent selector, " +
                "config mirror, and state After snapshot " +
                "(config=$finalConfigMatches persistent=$finalPersistentMatches state=$finalStateMatches)."
            )
        }
        Remove-Item -LiteralPath $paths.Pending -Force
    } catch {
        $originalError = $_
        if ($originalError.Exception.Data["CodexDevDeploymentInterrupted"] -eq $true) {
            throw $originalError
        }

        $restoreErrors = [System.Collections.Generic.List[string]]::new()
        $persistentNow = Read-CodexDevPersistentSelector
        $configNow = Read-CodexCliConfigSnapshot $ConfigPath
        $stateNow = Read-CodexDevJsonFile $paths.State
        $persistentIsBefore = $null -eq $persistentNow.Error -and
            (Test-CodexDevSelectorExact `
                -Left $persistentNow.Value `
                -Right $PersistentBefore)
        $persistentIsAfter = $null -eq $persistentNow.Error -and
            (Test-CodexDevSelectorExact `
                -Left $persistentNow.Value `
                -Right $PersistentAfter)
        $configIsBefore = $null -eq $configNow.Error -and
            $configNow.Sha256 -eq [string]$ConfigTransition.BeforeSha256
        $configIsAfter = $null -eq $configNow.Error -and
            $configNow.Sha256 -eq [string]$ConfigTransition.AfterSha256
        $stateIsBefore = Test-CodexDevStateSnapshot `
            -StateRead $stateNow `
            -ExpectedExists $StateBeforeExists `
            -ExpectedValue $StateBefore
        $stateIsAfter = Test-CodexDevStateSnapshot `
            -StateRead $stateNow `
            -ExpectedExists $true `
            -ExpectedValue $StateAfter
        if (-not ($configIsBefore -or $configIsAfter)) {
            $restoreErrors.Add("config is neither the transaction Before nor After image")
        }
        if (-not ($persistentIsBefore -or $persistentIsAfter)) {
            $restoreErrors.Add(
                "persistent selector is neither the transaction Before nor After value"
            )
        }
        if (-not ($stateIsBefore -or $stateIsAfter)) {
            $restoreErrors.Add("state is neither the transaction Before nor After snapshot")
        }
        if ($restoreErrors.Count -eq 0 -and $configChanged -and $configIsAfter) {
            try {
                Restore-CodexDevConfigBackup `
                    -ConfigPath $ConfigPath `
                    -BackupPath $ConfigBackup `
                    -ExpectedCurrentSha256 ([string]$ConfigTransition.AfterSha256) `
                    -ExpectedBackupSha256 ([string]$ConfigTransition.BeforeSha256)
            } catch {
                $restoreErrors.Add("config: $($_.Exception.Message)")
            }
        }
        if ($restoreErrors.Count -eq 0) {
            if ($persistentChanged -and $persistentIsAfter) {
                try {
                    Restore-CodexDevPersistentSelector `
                        -ExpectedCurrent $PersistentAfter `
                        -SelectorBefore $PersistentBefore
                } catch {
                    $restoreErrors.Add("persistent selector: $($_.Exception.Message)")
                }
            }
        }
        if ($restoreErrors.Count -eq 0) {
            try {
                Restore-CodexDevStateSnapshot `
                    -StatePath $paths.State `
                    -StateExists $StateBeforeExists `
                    -StateValue $StateBefore
            } catch {
                $restoreErrors.Add("state: $($_.Exception.Message)")
            }
        }
        if ($restoreErrors.Count -eq 0) {
            $restoredConfig = Read-CodexCliConfigSnapshot $ConfigPath
            $restoredPersistent = Read-CodexDevPersistentSelector
            $restoredState = Read-CodexDevJsonFile $paths.State
            if ($restoredConfig.Sha256 -eq [string]$ConfigTransition.BeforeSha256 -and
                $null -eq $restoredPersistent.Error -and
                (Test-CodexDevSelectorExact `
                    -Left $restoredPersistent.Value `
                    -Right $PersistentBefore) -and
                (Test-CodexDevStateSnapshot `
                    -StateRead $restoredState `
                    -ExpectedExists $StateBeforeExists `
                    -ExpectedValue $StateBefore)) {
                Remove-Item -LiteralPath $paths.Pending -Force
                throw $originalError
            }
            $restoreErrors.Add(
                "restored selector, config, and state did not verify as transaction Before"
            )
        }
        throw (
            "Deployment transaction failed and restoration was incomplete. " +
            "Original error: $($originalError.Exception.Message). " +
            "Restoration errors: $($restoreErrors -join '; '). " +
            "Pending transaction retained at $($paths.Pending)."
        )
    }
}

function Repair-CodexDevInterruptedTransaction {
    param(
        [string]$ConfigPath,
        [string]$DeploymentRoot
    )

    $status = Get-CodexDevDeploymentStatusUnlocked `
        -ConfigPath $ConfigPath `
        -DeploymentRoot $DeploymentRoot
    if (-not $status.PendingExists) {
        return $null
    }
    if ($status.PendingDisposition -notin @("rollback", "complete")) {
        throw (
            "Pending deployment transaction cannot be recovered automatically: " +
            "$($status.PendingDisposition). $($status.DriftReasons -join ' ')"
        )
    }

    $pending = $status.PendingTransaction
    $expectedConfigSha256 = if ($status.PendingDisposition -eq "rollback") {
        [string]$pending.ConfigBeforeSha256
    } else {
        [string]$pending.ConfigAfterSha256
    }
    $expectedPersistent = if ($status.PendingDisposition -eq "rollback") {
        $pending.PersistentBefore
    } else {
        [string]$pending.PersistentAfter
    }
    $configNow = Read-CodexCliConfigSnapshot $ConfigPath
    $persistentNow = Get-CodexDevPersistentSelector
    if ($null -ne $configNow.Error) {
        throw "Config changed while interrupted deployment recovery was being prepared."
    }

    $paths = Get-CodexDevDeploymentPaths $DeploymentRoot
    if ($status.PendingDisposition -eq "rollback") {
        if ($configNow.Sha256 -eq [string]$pending.ConfigAfterSha256 -and
            [string]$pending.ConfigBeforeSha256 -ne [string]$pending.ConfigAfterSha256) {
            Restore-CodexDevConfigBackup `
                -ConfigPath $ConfigPath `
                -BackupPath ([string]$pending.ConfigBackup) `
                -ExpectedCurrentSha256 ([string]$pending.ConfigAfterSha256) `
                -ExpectedBackupSha256 ([string]$pending.ConfigBeforeSha256)
        } elseif ($configNow.Sha256 -ne [string]$pending.ConfigBeforeSha256) {
            throw "Config changed while interrupted deployment recovery was being prepared."
        }
        if (-not (Test-CodexDevSelectorExact `
            -Left $persistentNow `
            -Right $pending.PersistentBefore)) {
            Restore-CodexDevPersistentSelector `
                -ExpectedCurrent ([string]$pending.PersistentAfter) `
                -SelectorBefore $pending.PersistentBefore
        }
        Restore-CodexDevStateSnapshot `
            -StatePath $paths.State `
            -StateExists ([bool]$pending.StateBeforeExists) `
            -StateValue $pending.StateBefore
        $expectedStateExists = [bool]$pending.StateBeforeExists
        $expectedState = $pending.StateBefore
        $recoveryStatus = "recovered_before"
    } else {
        if ($configNow.Sha256 -ne [string]$pending.ConfigAfterSha256 -or
            -not (Test-CodexDevSelectorExact `
                -Left $persistentNow `
                -Right ([string]$pending.PersistentAfter))) {
            throw "Durable selectors changed while interrupted deployment recovery was prepared."
        }
        Write-CodexDevJson -Path $paths.State -Value $pending.StateAfter
        $expectedStateExists = $true
        $expectedState = $pending.StateAfter
        $recoveryStatus = "recovered_after"
    }
    $verifiedConfig = Read-CodexCliConfigSnapshot $ConfigPath
    $verifiedPersistent = Read-CodexDevPersistentSelector
    $verifiedState = Read-CodexDevJsonFile $paths.State
    if ($verifiedConfig.Sha256 -ne $expectedConfigSha256 -or
        $null -ne $verifiedPersistent.Error -or
        -not (Test-CodexDevSelectorExact `
            -Left $verifiedPersistent.Value `
            -Right $expectedPersistent) -or
        -not (Test-CodexDevStateSnapshot `
            -StateRead $verifiedState `
            -ExpectedExists $expectedStateExists `
            -ExpectedValue $expectedState)) {
        throw "Interrupted deployment recovery did not verify; pending transaction was retained."
    }
    Remove-Item -LiteralPath $paths.Pending -Force
    return [pscustomobject]@{
        Status = $recoveryStatus
        TransactionId = [string]$pending.TransactionId
        Action = [string]$pending.Action
        ConfiguredBefore = [string]$pending.ConfiguredBefore
        ConfiguredAfter = [string]$pending.ConfiguredAfter
        PersistentBefore = $pending.PersistentBefore
        PersistentAfter = [string]$pending.PersistentAfter
        ConfigBackup = [string]$pending.ConfigBackup
    }
}
