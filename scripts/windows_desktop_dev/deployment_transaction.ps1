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
        [object]$ConfigTransition,
        [string]$ConfigBackup,
        [bool]$StateBeforeExists,
        [AllowNull()][object]$StateBefore,
        [object]$StateAfter
    )

    if (Test-CodexDevPathEqual -Left $ConfiguredBefore -Right $ConfiguredAfter) {
        throw "Journaled deployment transitions require distinct Before and After selectors."
    }
    if ([string]$ConfigTransition.BeforeSha256 -eq [string]$ConfigTransition.AfterSha256) {
        throw "Journaled deployment transitions require distinct Before and After config images."
    }
    $paths = Get-CodexDevDeploymentPaths $DeploymentRoot
    $transaction = [ordered]@{
        SchemaVersion = 1
        TransactionId = [guid]::NewGuid().ToString("N")
        Action = $Action
        CreatedAtUtc = [DateTime]::UtcNow.ToString("o")
        ConfigPath = [System.IO.Path]::GetFullPath($ConfigPath)
        ConfiguredBefore = $ConfiguredBefore
        ConfiguredAfter = $ConfiguredAfter
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
        Set-CodexCliPathInConfig `
            -ConfigPath $ConfigPath `
            -Entrypoint $ConfiguredAfter `
            -Transition $ConfigTransition
        Invoke-CodexDevDeploymentFault -Stage "AfterConfig" -Transaction $transaction
        Write-CodexDevJson -Path $paths.State -Value $StateAfter
        Invoke-CodexDevDeploymentFault -Stage "AfterState" -Transaction $transaction
        Remove-Item -LiteralPath $paths.Pending -Force
    } catch {
        $originalError = $_
        if ($originalError.Exception.Data["CodexDevDeploymentInterrupted"] -eq $true) {
            throw $originalError
        }

        $restoreErrors = [System.Collections.Generic.List[string]]::new()
        $configNow = Read-CodexCliConfigSnapshot $ConfigPath
        $stateNow = Read-CodexDevJsonFile $paths.State
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
        if (-not ($stateIsBefore -or $stateIsAfter)) {
            $restoreErrors.Add("state is neither the transaction Before nor After snapshot")
        }
        if ($restoreErrors.Count -eq 0 -and $configIsAfter) {
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
            $restoredState = Read-CodexDevJsonFile $paths.State
            if ($restoredConfig.Sha256 -eq [string]$ConfigTransition.BeforeSha256 -and
                (Test-CodexDevStateSnapshot `
                    -StateRead $restoredState `
                    -ExpectedExists $StateBeforeExists `
                    -ExpectedValue $StateBefore)) {
                Remove-Item -LiteralPath $paths.Pending -Force
                throw $originalError
            }
            $restoreErrors.Add("restored files did not verify as the transaction Before state")
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
    $configNow = Read-CodexCliConfigSnapshot $ConfigPath
    if ($null -ne $configNow.Error -or $configNow.Sha256 -ne $expectedConfigSha256) {
        throw "Config changed while interrupted deployment recovery was being prepared."
    }

    $paths = Get-CodexDevDeploymentPaths $DeploymentRoot
    if ($status.PendingDisposition -eq "rollback") {
        Restore-CodexDevStateSnapshot `
            -StatePath $paths.State `
            -StateExists ([bool]$pending.StateBeforeExists) `
            -StateValue $pending.StateBefore
        $expectedStateExists = [bool]$pending.StateBeforeExists
        $expectedState = $pending.StateBefore
        $recoveryStatus = "recovered_before"
    } else {
        Write-CodexDevJson -Path $paths.State -Value $pending.StateAfter
        $expectedStateExists = $true
        $expectedState = $pending.StateAfter
        $recoveryStatus = "recovered_after"
    }
    $verifiedConfig = Read-CodexCliConfigSnapshot $ConfigPath
    $verifiedState = Read-CodexDevJsonFile $paths.State
    if ($verifiedConfig.Sha256 -ne $expectedConfigSha256 -or
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
        ConfigBackup = [string]$pending.ConfigBackup
    }
}
