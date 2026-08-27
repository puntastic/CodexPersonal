function Get-CodexDevDeploymentPaths {
    param([string]$DeploymentRoot)

    $root = [System.IO.Path]::GetFullPath($DeploymentRoot)
    return [pscustomobject]@{
        Root = $root
        Lock = Join-Path $root "deployment.lock"
        State = Join-Path $root "state.json"
        Pending = Join-Path $root "pending-transaction.json"
    }
}

function Invoke-WithCodexDevDeploymentLock {
    param(
        [string]$DeploymentRoot,
        [scriptblock]$Body,
        [int]$LockTimeoutMilliseconds = 15000
    )

    $paths = Get-CodexDevDeploymentPaths $DeploymentRoot
    New-Item `
        -ItemType Directory `
        -Path $paths.Root `
        -Force `
        -WhatIf:$false `
        -Confirm:$false | Out-Null
    $stopwatch = [System.Diagnostics.Stopwatch]::StartNew()
    $stream = $null
    while ($null -eq $stream) {
        try {
            $stream = [System.IO.File]::Open(
                $paths.Lock,
                [System.IO.FileMode]::OpenOrCreate,
                [System.IO.FileAccess]::ReadWrite,
                [System.IO.FileShare]::None
            )
        } catch [System.IO.IOException] {
            if ($stopwatch.ElapsedMilliseconds -ge $LockTimeoutMilliseconds) {
                $exception = [System.TimeoutException]::new(
                    "Timed out waiting for the Codex Desktop deployment lock: $($paths.Lock)",
                    $_.Exception
                )
                $exception.Data["CodexDevDeploymentLockTimeout"] = $true
                throw $exception
            }
            Start-Sleep -Milliseconds 50
        }
    }

    try {
        $metadata = [System.Text.Encoding]::UTF8.GetBytes(
            "pid=$PID acquired=$([DateTime]::UtcNow.ToString('o'))"
        )
        $stream.SetLength(0)
        $stream.Write($metadata, 0, $metadata.Length)
        $stream.Flush($true)
        return & $Body
    } finally {
        $stream.Dispose()
    }
}

function Invoke-WithCodexDevConfigLock {
    param(
        [string]$ConfigPath,
        [scriptblock]$Body,
        [int]$LockTimeoutMilliseconds = 15000
    )

    $config = [System.IO.Path]::GetFullPath($ConfigPath).ToLowerInvariant()
    $mutexName = "Local\CodexDevDeploymentConfig_$(Get-CodexDevSha256Text $config)"
    $mutex = [System.Threading.Mutex]::new($false, $mutexName)
    $acquired = $false
    try {
        try {
            $acquired = $mutex.WaitOne($LockTimeoutMilliseconds)
        } catch [System.Threading.AbandonedMutexException] {
            $acquired = $true
        }
        if (-not $acquired) {
            $exception = [System.TimeoutException]::new(
                "Timed out waiting for the Codex Desktop config lock: $ConfigPath"
            )
            $exception.Data["CodexDevDeploymentLockTimeout"] = $true
            throw $exception
        }
        return & $Body
    } finally {
        if ($acquired) {
            $mutex.ReleaseMutex()
        }
        $mutex.Dispose()
    }
}

function Read-CodexDevJsonFile {
    param([string]$Path)

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        return [pscustomobject]@{
            Exists = $false
            Value = $null
            Error = $null
        }
    }
    try {
        return [pscustomobject]@{
            Exists = $true
            Value = Get-Content -LiteralPath $Path -Raw | ConvertFrom-Json
            Error = $null
        }
    } catch {
        return [pscustomobject]@{
            Exists = $true
            Value = $null
            Error = $_.Exception.Message
        }
    }
}

function Get-CodexDevObjectProperty {
    param(
        [object]$Value,
        [string]$Name
    )

    if ($null -eq $Value) {
        return $null
    }
    $property = $Value.PSObject.Properties[$Name]
    if ($null -eq $property) {
        return $null
    }
    return $property.Value
}

function Test-CodexDevObjectProperty {
    param(
        [object]$Value,
        [string]$Name
    )

    return $null -ne $Value -and $null -ne $Value.PSObject.Properties[$Name]
}

function Test-CodexDevPathEqual {
    param(
        [AllowNull()][string]$Left,
        [AllowNull()][string]$Right
    )

    $leftMissing = [string]::IsNullOrWhiteSpace($Left)
    $rightMissing = [string]::IsNullOrWhiteSpace($Right)
    if ($leftMissing -or $rightMissing) {
        return $leftMissing -and $rightMissing
    }
    try {
        $leftPath = [System.IO.Path]::GetFullPath($Left).TrimEnd('\')
        $rightPath = [System.IO.Path]::GetFullPath($Right).TrimEnd('\')
        return [string]::Equals(
            $leftPath,
            $rightPath,
            [System.StringComparison]::OrdinalIgnoreCase
        )
    } catch {
        return [string]::Equals($Left, $Right, [System.StringComparison]::Ordinal)
    }
}

function Get-CodexDevPackageSelection {
    param([object]$Package)

    $buildId = [string](
        Get-CodexDevObjectProperty -Value $Package.Provenance -Name "BuildId"
    )
    $taskToken = [string](
        Get-CodexDevObjectProperty -Value $Package.Provenance -Name "TaskToken"
    )
    return [ordered]@{
        SourcePackageRoot = [string]$Package.PackageRoot
        ArtifactFingerprint = [string]$Package.Fingerprint
        ProvenanceStatus = [string]$Package.ProvenanceStatus
        BuildId = $buildId
        TaskToken = $taskToken
        Provenance = $Package.Provenance
    }
}

function Test-CodexDevPackageSelectionEqual {
    param(
        [AllowNull()][object]$Left,
        [AllowNull()][object]$Right
    )

    if ($null -eq $Left -or $null -eq $Right) {
        return $null -eq $Left -and $null -eq $Right
    }
    $leftJson = ConvertTo-Json -InputObject $Left -Depth 20 -Compress
    $rightJson = ConvertTo-Json -InputObject $Right -Depth 20 -Compress
    return $leftJson -ceq $rightJson
}

function Test-CodexDevStateSnapshot {
    param(
        [object]$StateRead,
        [bool]$ExpectedExists,
        [AllowNull()][object]$ExpectedValue
    )

    if ([bool]$StateRead.Exists -ne $ExpectedExists) {
        return $false
    }
    if (-not $ExpectedExists) {
        return $true
    }
    if ($null -ne $StateRead.Error) {
        return $false
    }
    $actualJson = ConvertTo-Json -InputObject $StateRead.Value -Depth 20 -Compress
    $expectedJson = ConvertTo-Json -InputObject $ExpectedValue -Depth 20 -Compress
    return $actualJson -ceq $expectedJson
}

function Test-CodexDevStateSchema {
    param(
        [AllowNull()][object]$State,
        [string]$ConfigPath
    )

    if ($null -eq $State) {
        return $false
    }
    foreach ($propertyName in @(
        "SchemaVersion",
        "ConfigPath",
        "Current",
        "Previous",
        "LastConfigBackup"
    )) {
        if (-not (Test-CodexDevObjectProperty -Value $State -Name $propertyName)) {
            return $false
        }
    }
    $schemaVersion = Get-CodexDevObjectProperty -Value $State -Name "SchemaVersion"
    $stateConfigPath = Get-CodexDevObjectProperty -Value $State -Name "ConfigPath"
    $current = Get-CodexDevObjectProperty -Value $State -Name "Current"
    $currentEntrypoint = Get-CodexDevObjectProperty -Value $current -Name "Entrypoint"
    $previous = Get-CodexDevObjectProperty -Value $State -Name "Previous"
    $previousEntrypoint = Get-CodexDevObjectProperty -Value $previous -Name "Entrypoint"
    $lastConfigBackup = Get-CodexDevObjectProperty -Value $State -Name "LastConfigBackup"
    $numericSchema = $schemaVersion -is [int] -or $schemaVersion -is [long]
    return $numericSchema -and [long]$schemaVersion -eq 1 -and
        $stateConfigPath -is [string] -and
        -not [string]::IsNullOrWhiteSpace([string]$stateConfigPath) -and
        (Test-CodexDevPathEqual -Left ([string]$stateConfigPath) -Right $ConfigPath) -and
        $null -ne $current -and
        $currentEntrypoint -is [string] -and
        -not [string]::IsNullOrWhiteSpace([string]$currentEntrypoint) -and
        ($null -eq $previous -or
            ($previousEntrypoint -is [string] -and
                -not [string]::IsNullOrWhiteSpace([string]$previousEntrypoint))) -and
        ($null -eq $lastConfigBackup -or $lastConfigBackup -is [string])
}

function Get-CodexDevDeploymentStatusUnlocked {
    param(
        [string]$ConfigPath,
        [string]$DeploymentRoot
    )

    $config = [System.IO.Path]::GetFullPath($ConfigPath)
    $paths = Get-CodexDevDeploymentPaths $DeploymentRoot
    $configRead = Read-CodexCliConfigSnapshot $config
    $configError = $configRead.Error
    $configured = $configRead.ConfiguredEntrypoint
    $stateRead = Read-CodexDevJsonFile $paths.State
    $pendingRead = Read-CodexDevJsonFile $paths.Pending
    $driftReasons = [System.Collections.Generic.List[string]]::new()
    if ($null -ne $configError) {
        $driftReasons.Add("Config could not be read: $configError")
    }
    if ($null -ne $stateRead.Error) {
        $driftReasons.Add("Deployment state could not be read: $($stateRead.Error)")
    }

    $stateCurrent = Get-CodexDevObjectProperty -Value $stateRead.Value -Name "Current"
    $stateCurrentEntrypoint = [string](
        Get-CodexDevObjectProperty -Value $stateCurrent -Name "Entrypoint"
    )
    $stateConfigPath = [string](
        Get-CodexDevObjectProperty -Value $stateRead.Value -Name "ConfigPath"
    )
    $stateSchemaValid = -not $stateRead.Exists -or
        ($null -eq $stateRead.Error -and
            (Test-CodexDevStateSchema -State $stateRead.Value -ConfigPath $config))
    $pending = $pendingRead.Value
    $pendingDisposition = "none"
    $status = $null

    if ($pendingRead.Exists) {
        if ($null -ne $pendingRead.Error) {
            $pendingDisposition = "invalid_pending"
            $status = "pending_invalid"
            $driftReasons.Add("Pending transaction could not be read: $($pendingRead.Error)")
        } else {
            $requiredProperties = @(
                "SchemaVersion",
                "TransactionId",
                "Action",
                "ConfigPath",
                "ConfiguredBefore",
                "ConfiguredAfter",
                "ConfigBeforeSha256",
                "ConfigAfterSha256",
                "ConfigBackup",
                "StateBeforeExists",
                "StateBefore",
                "StateAfter"
            )
            $missingProperties = @(
                $requiredProperties | Where-Object {
                    -not (Test-CodexDevObjectProperty -Value $pending -Name $_)
                }
            )
            if ($missingProperties.Count -gt 0) {
                $pendingDisposition = "invalid_pending"
                $status = "pending_invalid"
                $driftReasons.Add(
                    "Pending transaction is missing: $($missingProperties -join ', ')"
                )
            } elseif (($pending.SchemaVersion -isnot [int] -and
                $pending.SchemaVersion -isnot [long]) -or
                [long]$pending.SchemaVersion -ne 1 -or
                [string]$pending.Action -notin @("Deploy", "Rollback") -or
                [string]::IsNullOrWhiteSpace([string]$pending.TransactionId) -or
                [string]::IsNullOrWhiteSpace([string]$pending.ConfiguredAfter) -or
                [string]$pending.ConfigBeforeSha256 -notmatch '^[0-9a-f]{64}$' -or
                [string]$pending.ConfigAfterSha256 -notmatch '^[0-9a-f]{64}$' -or
                [string]$pending.ConfigBeforeSha256 -eq [string]$pending.ConfigAfterSha256 -or
                (Test-CodexDevPathEqual `
                    -Left ([string]$pending.ConfiguredBefore) `
                    -Right ([string]$pending.ConfiguredAfter)) -or
                [string]::IsNullOrWhiteSpace([string]$pending.ConfigBackup) -or
                $pending.StateBeforeExists -isnot [bool] -or
                ($pending.StateBeforeExists -eq $false -and $null -ne $pending.StateBefore) -or
                ($pending.StateBeforeExists -eq $true -and $null -eq $pending.StateBefore) -or
                $null -eq $pending.StateAfter) {
                $pendingDisposition = "invalid_pending"
                $status = "pending_invalid"
                $driftReasons.Add("Pending transaction has an unsupported schema or invalid fields.")
            } elseif (-not (Test-CodexDevPathEqual -Left $config -Right ([string]$pending.ConfigPath))) {
                $pendingDisposition = "config_path_mismatch"
                $status = "pending_drift"
                $driftReasons.Add(
                    "Pending transaction targets a different config path: $($pending.ConfigPath)"
                )
            } elseif ($null -ne $configError) {
                $pendingDisposition = "ambiguous_config"
                $status = "pending_drift"
                $driftReasons.Add("Pending transaction cannot classify the current config.")
            } else {
                $stateAfterCurrent = Get-CodexDevObjectProperty `
                    -Value $pending.StateAfter `
                    -Name "Current"
                $stateAfterEntrypoint = [string](
                    Get-CodexDevObjectProperty -Value $stateAfterCurrent -Name "Entrypoint"
                )
                $stateBeforeValid = -not [bool]$pending.StateBeforeExists -or
                    (Test-CodexDevStateSchema `
                        -State $pending.StateBefore `
                        -ConfigPath $config)
                $stateAfterValid = Test-CodexDevStateSchema `
                    -State $pending.StateAfter `
                    -ConfigPath $config
                if (-not $stateBeforeValid -or -not $stateAfterValid -or
                    -not (Test-CodexDevPathEqual `
                        -Left $stateAfterEntrypoint `
                        -Right ([string]$pending.ConfiguredAfter))) {
                    $pendingDisposition = "invalid_pending"
                    $status = "pending_invalid"
                    $driftReasons.Add("Pending transaction state snapshots are invalid.")
                } else {
                $configMatchesBefore = $configRead.Sha256 -eq [string]$pending.ConfigBeforeSha256
                $configMatchesAfter = $configRead.Sha256 -eq [string]$pending.ConfigAfterSha256
                $stateMatchesBefore = Test-CodexDevStateSnapshot `
                    -StateRead $stateRead `
                    -ExpectedExists ([bool]$pending.StateBeforeExists) `
                    -ExpectedValue $pending.StateBefore
                $stateMatchesAfter = Test-CodexDevStateSnapshot `
                    -StateRead $stateRead `
                    -ExpectedExists $true `
                    -ExpectedValue $pending.StateAfter

                if ($configMatchesBefore -eq $configMatchesAfter) {
                    $pendingDisposition = "ambiguous_config"
                    $status = "pending_drift"
                    $driftReasons.Add(
                        "Configured entrypoint is not uniquely the pending Before or After value."
                    )
                } elseif (-not ($stateMatchesBefore -or $stateMatchesAfter)) {
                    $pendingDisposition = "ambiguous_state"
                    $status = "pending_drift"
                    $driftReasons.Add(
                        "Deployment state is neither the pending Before nor After snapshot."
                    )
                } elseif ($configMatchesBefore) {
                    $pendingDisposition = "rollback"
                    $status = "pending_recoverable_before"
                    $driftReasons.Add("Interrupted transaction is recoverable to Before.")
                } else {
                    $pendingDisposition = "complete"
                    $status = "pending_recoverable_after"
                    $driftReasons.Add("Interrupted transaction is recoverable to After.")
                }
                }
            }
        }
    } elseif ($null -ne $configError) {
        $status = "config_invalid"
    } elseif ($null -ne $stateRead.Error -or -not $stateSchemaValid) {
        $status = "state_invalid"
        if ($null -eq $stateRead.Error) {
            $driftReasons.Add("Deployment state schema or config binding is invalid.")
        }
    } elseif (-not $stateRead.Exists) {
        $status = if ([string]::IsNullOrWhiteSpace($configured)) {
            "unconfigured"
        } else {
            "unmanaged"
        }
    } elseif ([string]::IsNullOrWhiteSpace($stateCurrentEntrypoint)) {
        $status = "state_invalid"
        $driftReasons.Add("Deployment state has no Current.Entrypoint.")
    } elseif (-not [string]::IsNullOrWhiteSpace($stateConfigPath) -and
        -not (Test-CodexDevPathEqual -Left $config -Right $stateConfigPath)) {
        $status = "drift"
        $driftReasons.Add("Deployment state is bound to a different config path.")
    } elseif (Test-CodexDevPathEqual -Left $configured -Right $stateCurrentEntrypoint) {
        $status = "consistent"
    } else {
        $status = "drift"
        $driftReasons.Add(
            "Configured entrypoint does not match deployment state Current.Entrypoint."
        )
    }

    return [pscustomobject]@{
        Status = $status
        ConfigPath = $config
        DeploymentRoot = $paths.Root
        StatePath = $paths.State
        PendingPath = $paths.Pending
        ConfiguredEntrypoint = $configured
        StateCurrentEntrypoint = $stateCurrentEntrypoint
        StateExists = [bool]$stateRead.Exists
        PendingExists = [bool]$pendingRead.Exists
        PendingDisposition = $pendingDisposition
        PendingTransaction = $pending
        ConfigSha256 = $configRead.Sha256
        State = $stateRead.Value
        DriftReasons = @($driftReasons.ToArray())
    }
}

function Get-CodexDevDeploymentStatus {
    param(
        [string]$ConfigPath,
        [string]$DeploymentRoot = (Get-CodexDevDefaultDeploymentRoot)
    )

    $paths = Get-CodexDevDeploymentPaths $DeploymentRoot
    if (-not (Test-Path -LiteralPath $paths.Root -PathType Container)) {
        return Invoke-WithCodexDevConfigLock `
            -ConfigPath $ConfigPath `
            -LockTimeoutMilliseconds 1000 `
            -Body {
                Get-CodexDevDeploymentStatusUnlocked `
                    -ConfigPath $ConfigPath `
                    -DeploymentRoot $paths.Root
            }
    }
    try {
        return Invoke-WithCodexDevDeploymentLock `
            -DeploymentRoot $paths.Root `
            -LockTimeoutMilliseconds 1000 `
            -Body {
                Invoke-WithCodexDevConfigLock `
                    -ConfigPath $ConfigPath `
                    -LockTimeoutMilliseconds 1000 `
                    -Body {
                        Get-CodexDevDeploymentStatusUnlocked `
                            -ConfigPath $ConfigPath `
                            -DeploymentRoot $paths.Root
                    }
            }
    } catch {
        if ($_.Exception.Data["CodexDevDeploymentLockTimeout"] -ne $true) {
            throw
        }
        return [pscustomobject]@{
            Status = "locked"
            ConfigPath = [System.IO.Path]::GetFullPath($ConfigPath)
            DeploymentRoot = $paths.Root
            StatePath = $paths.State
            PendingPath = $paths.Pending
            ConfiguredEntrypoint = $null
            StateCurrentEntrypoint = $null
            StateExists = $null
            PendingExists = $null
            PendingDisposition = "unknown_while_locked"
            PendingTransaction = $null
            DriftReasons = @($_.Exception.Message)
        }
    }
}

function New-CodexDevDeploymentState {
    param(
        [AllowNull()][object]$StateBefore,
        [string]$ConfigPath,
        [object]$Current,
        [AllowNull()][object]$Previous,
        [AllowNull()][string]$LastConfigBackup
    )

    $state = if ($null -eq $StateBefore) {
        [pscustomobject][ordered]@{}
    } else {
        ConvertFrom-Json (ConvertTo-Json -InputObject $StateBefore -Depth 20)
    }
    foreach ($property in ([ordered]@{
        SchemaVersion = 1
        ConfigPath = $ConfigPath
        Current = $Current
        Previous = $Previous
        LastConfigBackup = $LastConfigBackup
    }).GetEnumerator()) {
        $state | Add-Member `
            -MemberType NoteProperty `
            -Name $property.Key `
            -Value $property.Value `
            -Force
    }
    return $state
}
