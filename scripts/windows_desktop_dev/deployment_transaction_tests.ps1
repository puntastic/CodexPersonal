Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

. (Join-Path $PSScriptRoot "common.ps1")
. (Join-Path $PSScriptRoot "environment.ps1")
. (Join-Path $PSScriptRoot "package.ps1")
. (Join-Path $PSScriptRoot "deployment_config.ps1")
. (Join-Path $PSScriptRoot "deployment_selector.ps1")
. (Join-Path $PSScriptRoot "deployment_state.ps1")
. (Join-Path $PSScriptRoot "deployment_transaction.ps1")
. (Join-Path $PSScriptRoot "deployment.ps1")

$script:TransactionTestRealUserSelectorBefore = [System.Environment]::GetEnvironmentVariable(
    "CODEX_CLI_PATH",
    [System.EnvironmentVariableTarget]::User
)
$script:TransactionTestPersistentSelector = $null
$script:TransactionTestPersistentWrites = 0
Set-CodexDevPersistentSelectorTestAdapter `
    -Reader {
        param($Name)
        return $script:TransactionTestPersistentSelector
    } `
    -Writer {
        param($Name, $Value)
        $script:TransactionTestPersistentSelector = $Value
        $script:TransactionTestPersistentWrites++
    }

$script:TransactionTestReceiptDetails = @()
$script:TransactionTestReceiptFailure = $null
$script:TransactionTestPackageInfoCalls = @()
$script:TransactionTestPackageInfoHook = $null
$script:TransactionTestRealPackageInfo = (Get-Command Get-CodexDevPackageInfo).ScriptBlock

function Get-CodexDevPackageInfo {
    param(
        [string]$PackageDirectory,
        [switch]$SkipSmoke
    )

    $script:TransactionTestPackageInfoCalls += ,([pscustomobject]@{
        PackageDirectory = $PackageDirectory
        SkipSmokeRequested = [bool]$SkipSmoke
    })
    $result = & $script:TransactionTestRealPackageInfo `
        -PackageDirectory $PackageDirectory `
        -SkipSmoke
    if ($null -ne $script:TransactionTestPackageInfoHook) {
        $null = & $script:TransactionTestPackageInfoHook `
            -PackageDirectory $PackageDirectory `
            -Package $result
    }
    return $result
}

function Write-CodexDevReceipt {
    param(
        [string]$Action,
        [object]$Details,
        [object]$Source
    )

    if ($null -ne $script:TransactionTestReceiptFailure) {
        throw [string]$script:TransactionTestReceiptFailure
    }
    $script:TransactionTestReceiptDetails += ,$Details
    return "transaction-test-$Action-$($script:TransactionTestReceiptDetails.Count).json"
}

function Assert-TransactionTestEqual {
    param(
        [object]$Expected,
        [object]$Actual,
        [string]$Message
    )

    if ($Expected -ne $Actual) {
        throw "$Message`nExpected: $Expected`nActual:   $Actual"
    }
}

function Assert-TransactionTestTrue {
    param(
        [bool]$Condition,
        [string]$Message
    )

    if (-not $Condition) {
        throw $Message
    }
}

function Assert-TransactionTestThrows {
    param(
        [scriptblock]$Body,
        [string]$Pattern,
        [string]$Message
    )

    try {
        & $Body
    } catch {
        if ($_.Exception.Message -notmatch $Pattern) {
            throw "$Message`nUnexpected error: $($_.Exception.Message)"
        }
        return $_
    }
    throw "$Message`nExpected an exception matching: $Pattern"
}

function Write-TransactionTestFakePe {
    param(
        [string]$Path,
        [ValidateSet("x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc")]
        [string]$Target,
        [string]$Marker
    )

    $markerBytes = [System.Text.Encoding]::UTF8.GetBytes($Marker)
    $bytes = [byte[]]::new([Math]::Max(128, 0x50 + $markerBytes.Length))
    $bytes[0] = 0x4D
    $bytes[1] = 0x5A
    [BitConverter]::GetBytes([uint32]0x40).CopyTo($bytes, 0x3C)
    [BitConverter]::GetBytes([uint32]0x00004550).CopyTo($bytes, 0x40)
    $machine = if ($Target -eq "x86_64-pc-windows-msvc") {
        [uint16]0x8664
    } else {
        [uint16]0xAA64
    }
    [BitConverter]::GetBytes($machine).CopyTo($bytes, 0x44)
    $markerBytes.CopyTo($bytes, 0x50)
    [System.IO.File]::WriteAllBytes($Path, $bytes)
}

function New-TransactionTestPackage {
    param(
        [string]$Path,
        [string]$Marker,
        [string]$BuildId
    )

    New-Item -ItemType Directory -Path $Path | Out-Null
    foreach ($relativePath in $script:CodexDevExpectedPackageFiles) {
        $file = Join-Path $Path $relativePath
        New-Item -ItemType Directory -Path (Split-Path -Parent $file) -Force | Out-Null
        if ($relativePath -eq "codex-package.json") {
            Write-CodexDevJson -Path $file -Value ([ordered]@{
                layoutVersion = 1
                version = "0.0.0"
                target = Get-CodexDevHostTarget
                variant = "codex"
                entrypoint = "bin/codex.exe"
                resourcesDir = "codex-resources"
                pathDir = "codex-path"
            })
        } else {
            Write-TransactionTestFakePe `
                -Path $file `
                -Target (Get-CodexDevHostTarget) `
                -Marker "$Marker/$relativePath"
        }
    }
    $package = Get-CodexDevPackageInfo -PackageDirectory $Path -SkipSmoke
    if (-not [string]::IsNullOrWhiteSpace($BuildId)) {
        Write-CodexDevJson -Path (Join-Path $Path "codex-dev-build.json") -Value ([ordered]@{
            SchemaVersion = 1
            Status = "stable"
            BuildId = $BuildId
            TaskToken = "transaction-tests"
            ArtifactFingerprint = $package.Fingerprint
        })
    }
    return [System.IO.Path]::GetFullPath($Path)
}

function New-TransactionTestFixture {
    param(
        [string]$Root,
        [string]$Marker = "package",
        [string]$BuildId = "build-one"
    )

    New-Item -ItemType Directory -Path $Root | Out-Null
    $config = Join-Path $Root "config.toml"
    $oldEntrypoint = Join-Path $Root "installed\bin\codex.exe"
    New-Item -ItemType Directory -Path (Split-Path -Parent $oldEntrypoint) -Force | Out-Null
    [System.IO.File]::WriteAllText(
        $oldEntrypoint,
        "installed",
        [System.Text.UTF8Encoding]::new($false)
    )
    $script:TransactionTestPersistentSelector = $oldEntrypoint
    [System.IO.File]::WriteAllText(
        $config,
        "KEEP_ME = 'yes'`r`nCODEX_CLI_PATH = '$oldEntrypoint'`r`n",
        [System.Text.UTF8Encoding]::new($false)
    )
    $package = New-TransactionTestPackage `
        -Path (Join-Path $Root "package") `
        -Marker $Marker `
        -BuildId $BuildId
    return [pscustomobject]@{
        Root = $Root
        Config = $config
        OldEntrypoint = $oldEntrypoint
        Package = $package
        DeploymentRoot = Join-Path $Root "deployments"
    }
}

function Test-TransactionExclusiveLockHeld {
    param([string]$LockPath)

    $probe = $null
    try {
        $probe = [System.IO.File]::Open(
            $LockPath,
            [System.IO.FileMode]::OpenOrCreate,
            [System.IO.FileAccess]::ReadWrite,
            [System.IO.FileShare]::None
        )
        return $false
    } catch [System.IO.IOException] {
        return $true
    } finally {
        if ($null -ne $probe) {
            $probe.Dispose()
        }
    }
}

function Get-TransactionStateJson {
    param([string]$StatePath)

    $state = Get-Content -LiteralPath $StatePath -Raw | ConvertFrom-Json
    return ConvertTo-Json -InputObject $state -Depth 20 -Compress
}

function Get-TransactionReleaseIdentityJson {
    param([AllowNull()][object]$Release)

    if ($null -eq $Release) {
        return "null"
    }
    $identity = [ordered]@{}
    foreach ($name in @(
        "ReleaseId",
        "ReleasePath",
        "Entrypoint",
        "Fingerprint",
        "SourcePackageRoot",
        "BuildId",
        "ProvenanceStatus",
        "Selection"
    )) {
        $identity[$name] = Get-CodexDevObjectProperty -Value $Release -Name $name
    }
    return ConvertTo-Json -InputObject $identity -Depth 20 -Compress
}

function Invoke-TransactionVerifySkippingSmoke {
    param(
        [string]$ConfigPath,
        [string]$DeploymentRoot
    )

    return Invoke-CodexDevVerify `
        -ConfigPath $ConfigPath `
        -DeploymentRoot $DeploymentRoot
}

$tempBase = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
$testRoot = Join-Path $tempBase (
    "codex-desktop-deployment-transaction-tests-" + [guid]::NewGuid().ToString("N")
)
New-Item -ItemType Directory -Path $testRoot | Out-Null

try {
    $lockRoot = Join-Path $testRoot "exclusive-lock"
    $lockPaths = Get-CodexDevDeploymentPaths $lockRoot
    New-Item -ItemType Directory -Path $lockPaths.Root | Out-Null
    $heldLock = [System.IO.File]::Open(
        $lockPaths.Lock,
        [System.IO.FileMode]::OpenOrCreate,
        [System.IO.FileAccess]::ReadWrite,
        [System.IO.FileShare]::None
    )
    try {
        $null = Assert-TransactionTestThrows -Pattern "Timed out waiting" -Message (
            "A second deployment operation acquired the shared root lock."
        ) -Body {
            Invoke-WithCodexDevDeploymentLock `
                -DeploymentRoot $lockRoot `
                -LockTimeoutMilliseconds 100 `
                -Body { throw "lock body should not run" }
        }
        $lockedStatus = Get-CodexDevDeploymentStatus `
            -ConfigPath (Join-Path $lockRoot "config.toml") `
            -DeploymentRoot $lockRoot
        Assert-TransactionTestEqual `
            "locked" `
            $lockedStatus.Status `
            "Status threw or hid active deployment lock contention."
        Assert-TransactionTestEqual `
            "unknown_while_locked" `
            $lockedStatus.PendingDisposition `
            "Locked status claimed a pending disposition without reading it."
    } finally {
        $heldLock.Dispose()
    }
    $script:TransactionTestReacquiredStaleLock = $false
    Invoke-WithCodexDevDeploymentLock `
        -DeploymentRoot $lockRoot `
        -LockTimeoutMilliseconds 100 `
        -Body { $script:TransactionTestReacquiredStaleLock = $true }
    Assert-TransactionTestTrue `
        $script:TransactionTestReacquiredStaleLock `
        "A stale lock file was mistaken for active lock ownership."

    $caught = New-TransactionTestFixture -Root (Join-Path $testRoot "caught-failure")
    $caughtPaths = Get-CodexDevDeploymentPaths $caught.DeploymentRoot
    $caughtConfigBefore = Get-Content -LiteralPath $caught.Config -Raw
    $caughtPersistentBefore = Get-CodexDevPersistentSelector
    $script:TransactionTestObservedDeployLock = $false
    $script:CodexDevDeploymentFaultInjector = {
        param($Stage, $Transaction)
        if ($Stage -eq "AfterPersistentSelector") {
            $script:TransactionTestObservedDeployLock = Test-TransactionExclusiveLockHeld `
                (Get-CodexDevDeploymentPaths $caught.DeploymentRoot).Lock
            throw "injected caught deploy failure"
        }
    }
    $null = Assert-TransactionTestThrows `
        -Pattern "injected caught deploy failure" `
        -Message "Caught deployment failure was not surfaced." `
        -Body {
            Install-CodexDevPackage `
                -PackageDirectory $caught.Package `
                -ConfigPath $caught.Config `
                -DeploymentRoot $caught.DeploymentRoot `
                -SkipSmoke
        }
    $script:CodexDevDeploymentFaultInjector = $null
    Assert-TransactionTestTrue `
        $script:TransactionTestObservedDeployLock `
        "Deploy did not hold the exclusive root lock during config mutation."
    Assert-TransactionTestEqual `
        $caughtConfigBefore `
        (Get-Content -LiteralPath $caught.Config -Raw) `
        "Caught deployment failure did not restore config exactly."
    Assert-TransactionTestEqual `
        $caughtPersistentBefore `
        (Get-CodexDevPersistentSelector) `
        "Caught deployment failure did not restore the persistent selector exactly."
    Assert-TransactionTestTrue `
        (-not (Test-Path -LiteralPath $caughtPaths.State)) `
        "Caught deployment failure left new state behind."
    Assert-TransactionTestTrue `
        (-not (Test-Path -LiteralPath $caughtPaths.Pending)) `
        "Caught deployment failure left a pending transaction behind."

    $interruptionCases = @(
        foreach ($action in @("Deploy", "Rollback")) {
            foreach ($stage in @(
                "AfterPending",
                "AfterConfig",
                "AfterPersistentSelector",
                "AfterState"
            )) {
                [pscustomobject]@{ Action = $action; Stage = $stage }
            }
        }
    )
    foreach ($case in $interruptionCases) {
        $caseName = "$($case.Action)-$($case.Stage)"
        $fixture = New-TransactionTestFixture `
            -Root (Join-Path $testRoot "interruption-$caseName")
        $paths = Get-CodexDevDeploymentPaths $fixture.DeploymentRoot
        $expectedSelectedEntrypoint = $null
        $expectedCurrent = $null
        $expectedPrevious = $null
        if ($case.Action -eq "Rollback") {
            $packageTwo = New-TransactionTestPackage `
                -Path (Join-Path $fixture.Root "package-two") `
                -Marker "$caseName-package-two" `
                -BuildId "$caseName-build-two"
            $firstSelection = Install-CodexDevPackage `
                -PackageDirectory $fixture.Package `
                -ConfigPath $fixture.Config `
                -DeploymentRoot $fixture.DeploymentRoot `
                -SkipSmoke
            $null = Install-CodexDevPackage `
                -PackageDirectory $packageTwo `
                -ConfigPath $fixture.Config `
                -DeploymentRoot $fixture.DeploymentRoot `
                -SkipSmoke
            $expectedSelectedEntrypoint = $firstSelection.Release.Entrypoint
        }

        $script:TransactionTestInterruptStage = $case.Stage
        $script:CodexDevDeploymentFaultInjector = {
            param($Stage, $Transaction)
            if ($Stage -eq $script:TransactionTestInterruptStage) {
                return "Interrupt"
            }
        }
        try {
            if ($case.Action -eq "Deploy") {
                $null = Assert-TransactionTestThrows `
                    -Pattern "Simulated interruption" `
                    -Message "$caseName interruption was not simulated." `
                    -Body {
                        Install-CodexDevPackage `
                            -PackageDirectory $fixture.Package `
                            -ConfigPath $fixture.Config `
                            -DeploymentRoot $fixture.DeploymentRoot `
                            -SkipSmoke
                    }
            } else {
                $null = Assert-TransactionTestThrows `
                    -Pattern "Simulated interruption" `
                    -Message "$caseName interruption was not simulated." `
                    -Body {
                        Invoke-CodexDevRollback `
                            -ConfigPath $fixture.Config `
                            -DeploymentRoot $fixture.DeploymentRoot
                    }
            }
        } finally {
            $script:CodexDevDeploymentFaultInjector = $null
            Remove-Variable `
                -Scope Script `
                -Name TransactionTestInterruptStage `
                -ErrorAction SilentlyContinue
        }

        $expectedPendingStatus = if ($case.Stage -in @("AfterPending", "AfterConfig")) {
            "pending_recoverable_before"
        } else {
            "pending_recoverable_after"
        }
        $pendingStatus = Get-CodexDevDeploymentStatus `
            -ConfigPath $fixture.Config `
            -DeploymentRoot $fixture.DeploymentRoot
        Assert-TransactionTestEqual `
            $expectedPendingStatus `
            $pendingStatus.Status `
            "$caseName pending transaction classification was wrong."
        $expectedCurrent = $pendingStatus.PendingTransaction.StateAfter.Current
        $expectedPrevious = $pendingStatus.PendingTransaction.StateAfter.Previous
        $verifyPending = Invoke-CodexDevVerify `
            -ConfigPath $fixture.Config `
            -DeploymentRoot $fixture.DeploymentRoot
        Assert-TransactionTestEqual `
            $expectedPendingStatus `
            $verifyPending.Status `
            "$caseName Verify hid the pending transaction."
        Assert-TransactionTestTrue `
            ($null -eq $verifyPending.Package) `
            "$caseName Verify inspected a package before settling pending state."

        $whatIfConfigBefore = Get-Content -LiteralPath $fixture.Config -Raw
        $whatIfStateBefore = if (Test-Path -LiteralPath $paths.State -PathType Leaf) {
            Get-Content -LiteralPath $paths.State -Raw
        } else {
            $null
        }
        $whatIfPendingBefore = Get-Content -LiteralPath $paths.Pending -Raw
        $whatIfRollback = if ($case.Action -eq "Rollback") {
            Invoke-CodexDevRollback `
                -ConfigPath $fixture.Config `
                -DeploymentRoot $fixture.DeploymentRoot `
                -WhatIf
        } else {
            $null
        }
        if ($null -ne $whatIfRollback) {
            Assert-TransactionTestEqual `
                "planned_with_recovery" `
                $whatIfRollback.Status `
                "$caseName WhatIf did not expose its recovery step."
            Assert-TransactionTestEqual `
                ($case.Stage -in @("AfterPersistentSelector", "AfterState")) `
                ([bool]$whatIfRollback.RecoveryCompletesRequest) `
                "$caseName WhatIf misreported whether recovery completes Rollback."
            Assert-TransactionTestEqual `
                $whatIfConfigBefore `
                (Get-Content -LiteralPath $fixture.Config -Raw) `
                "$caseName WhatIf mutated config."
            Assert-TransactionTestEqual `
                $whatIfStateBefore `
                (Get-Content -LiteralPath $paths.State -Raw) `
                "$caseName WhatIf mutated state."
            Assert-TransactionTestEqual `
                $whatIfPendingBefore `
                (Get-Content -LiteralPath $paths.Pending -Raw) `
                "$caseName WhatIf mutated the pending journal."
        }

        $recovered = if ($case.Action -eq "Deploy") {
            Install-CodexDevPackage `
                -PackageDirectory $fixture.Package `
                -ConfigPath $fixture.Config `
                -DeploymentRoot $fixture.DeploymentRoot `
                -SkipSmoke
        } else {
            Invoke-CodexDevRollback `
                -ConfigPath $fixture.Config `
                -DeploymentRoot $fixture.DeploymentRoot
        }
        $expectedRecoveryStatus = if ($case.Stage -in @("AfterPending", "AfterConfig")) {
            "recovered_before"
        } else {
            "recovered_after"
        }
        Assert-TransactionTestEqual `
            $expectedRecoveryStatus `
            $recovered.Recovery.Status `
            "$caseName did not settle to the expected snapshot."
        if ($case.Action -eq "Rollback") {
            Assert-TransactionTestEqual `
                $expectedSelectedEntrypoint `
                (Get-CodexCliPathFromConfig $fixture.Config) `
                "$caseName recovery toggled Rollback twice."
        }
        Assert-TransactionTestTrue `
            (-not (Test-Path -LiteralPath $paths.Pending)) `
            "$caseName recovery retained its pending journal."
        Assert-TransactionTestEqual `
            "consistent" `
            (Get-CodexDevDeploymentStatus `
                -ConfigPath $fixture.Config `
                -DeploymentRoot $fixture.DeploymentRoot).Status `
            "$caseName recovery did not settle config and state."
        $settledState = Get-Content -LiteralPath $paths.State -Raw | ConvertFrom-Json
        Assert-TransactionTestEqual `
            (Get-TransactionReleaseIdentityJson $expectedCurrent) `
            (Get-TransactionReleaseIdentityJson $settledState.Current) `
            "$caseName recovery did not preserve exact Current identity."
        Assert-TransactionTestEqual `
            (ConvertTo-Json -InputObject $expectedPrevious -Depth 20 -Compress) `
            (ConvertTo-Json -InputObject $settledState.Previous -Depth 20 -Compress) `
            "$caseName recovery did not preserve exact Previous identity."
        Assert-TransactionTestTrue `
            ((Get-Content -LiteralPath $fixture.Config -Raw) -match "KEEP_ME = 'yes'") `
            "$caseName recovery did not preserve unrelated config content."
    }

    $compensation = New-TransactionTestFixture `
        -Root (Join-Path $testRoot "interrupted-compensation")
    $script:CodexDevDeploymentFaultInjector = {
        param($Stage, $Transaction)
        if ($Stage -eq "AfterPersistentSelector") { return "Interrupt" }
    }
    $null = Assert-TransactionTestThrows `
        -Pattern "Simulated interruption" `
        -Message "Compensation recovery fixture did not retain a transaction." `
        -Body {
            Install-CodexDevPackage `
                -PackageDirectory $compensation.Package `
                -ConfigPath $compensation.Config `
                -DeploymentRoot $compensation.DeploymentRoot `
                -SkipSmoke
        }
    $script:CodexDevDeploymentFaultInjector = $null
    $compensationPending = (Read-CodexDevJsonFile (
        Get-CodexDevDeploymentPaths $compensation.DeploymentRoot
    ).Pending).Value
    Restore-CodexDevConfigBackup `
        -ConfigPath $compensation.Config `
        -BackupPath $compensationPending.ConfigBackup `
        -ExpectedCurrentSha256 $compensationPending.ConfigAfterSha256 `
        -ExpectedBackupSha256 $compensationPending.ConfigBeforeSha256
    $compensationStatus = Get-CodexDevDeploymentStatus `
        -ConfigPath $compensation.Config `
        -DeploymentRoot $compensation.DeploymentRoot
    Assert-TransactionTestEqual `
        "pending_recoverable_before" `
        $compensationStatus.Status `
        "Interrupted failure compensation was not recoverable to Before."
    $compensationRecovered = Install-CodexDevPackage `
        -PackageDirectory $compensation.Package `
        -ConfigPath $compensation.Config `
        -DeploymentRoot $compensation.DeploymentRoot `
        -SkipSmoke
    Assert-TransactionTestEqual `
        "recovered_before" `
        $compensationRecovered.Recovery.Status `
        "Interrupted failure compensation did not finish restoring Before."

    $ambiguous = New-TransactionTestFixture -Root (Join-Path $testRoot "ambiguous")
    $ambiguousPaths = Get-CodexDevDeploymentPaths $ambiguous.DeploymentRoot
    $script:CodexDevDeploymentFaultInjector = {
        param($Stage, $Transaction)
        if ($Stage -eq "AfterPending") { return "Interrupt" }
    }
    $null = Assert-TransactionTestThrows -Pattern "Simulated interruption" -Message (
        "Ambiguous recovery fixture did not retain a pending transaction."
    ) -Body {
        Install-CodexDevPackage `
            -PackageDirectory $ambiguous.Package `
            -ConfigPath $ambiguous.Config `
            -DeploymentRoot $ambiguous.DeploymentRoot `
            -SkipSmoke
    }
    $script:CodexDevDeploymentFaultInjector = $null
    $externalEntrypoint = Join-Path $ambiguous.Root "external\bin\codex.exe"
    Set-CodexCliPathInConfig -ConfigPath $ambiguous.Config -Entrypoint $externalEntrypoint
    $ambiguousStatus = Get-CodexDevDeploymentStatus `
        -ConfigPath $ambiguous.Config `
        -DeploymentRoot $ambiguous.DeploymentRoot
    Assert-TransactionTestEqual `
        "pending_drift" `
        $ambiguousStatus.Status `
        "Ambiguous config was not exposed as pending drift."
    $null = Assert-TransactionTestThrows `
        -Pattern "cannot be recovered automatically" `
        -Message "Deploy overwrote ambiguous pending drift." `
        -Body {
            Install-CodexDevPackage `
                -PackageDirectory $ambiguous.Package `
                -ConfigPath $ambiguous.Config `
                -DeploymentRoot $ambiguous.DeploymentRoot `
                -SkipSmoke
        }
    Assert-TransactionTestTrue `
        (Test-Path -LiteralPath $ambiguousPaths.Pending -PathType Leaf) `
        "Ambiguous pending transaction was discarded."

    $malformedState = New-TransactionTestFixture `
        -Root (Join-Path $testRoot "malformed-state")
    $malformedStatePaths = Get-CodexDevDeploymentPaths $malformedState.DeploymentRoot
    Write-CodexDevJson -Path $malformedStatePaths.State -Value ([ordered]@{
        SchemaVersion = 1
        ConfigPath = $malformedState.Config
        Current = "bad"
        Previous = $null
        LastConfigBackup = $null
    })
    $malformedStateBefore = Get-Content -LiteralPath $malformedStatePaths.State -Raw
    $malformedStateStatus = Get-CodexDevDeploymentStatus `
        -ConfigPath $malformedState.Config `
        -DeploymentRoot $malformedState.DeploymentRoot
    Assert-TransactionTestEqual `
        "state_invalid" `
        $malformedStateStatus.Status `
        "Valid JSON with malformed nested state escaped schema classification."
    Assert-TransactionTestEqual `
        $malformedStateBefore `
        (Get-Content -LiteralPath $malformedStatePaths.State -Raw) `
        "State classification mutated malformed state."

    $malformedPending = New-TransactionTestFixture `
        -Root (Join-Path $testRoot "malformed-pending")
    $malformedPendingPaths = Get-CodexDevDeploymentPaths $malformedPending.DeploymentRoot
    $script:CodexDevDeploymentFaultInjector = {
        param($Stage, $Transaction)
        if ($Stage -eq "AfterPending") { return "Interrupt" }
    }
    try {
        $null = Assert-TransactionTestThrows `
            -Pattern "Simulated interruption" `
            -Message "Malformed pending fixture did not retain a journal." `
            -Body {
                Install-CodexDevPackage `
                    -PackageDirectory $malformedPending.Package `
                    -ConfigPath $malformedPending.Config `
                    -DeploymentRoot $malformedPending.DeploymentRoot `
                    -SkipSmoke
            }
    } finally {
        $script:CodexDevDeploymentFaultInjector = $null
    }
    $pendingValue = Get-Content -LiteralPath $malformedPendingPaths.Pending -Raw |
        ConvertFrom-Json
    $pendingValue.StateBeforeExists = "false"
    Write-CodexDevJson -Path $malformedPendingPaths.Pending -Value $pendingValue
    Assert-TransactionTestEqual `
        "pending_invalid" `
        (Get-CodexDevDeploymentStatus `
            -ConfigPath $malformedPending.Config `
            -DeploymentRoot $malformedPending.DeploymentRoot).Status `
        "String StateBeforeExists was coerced into a valid Boolean."
    $pendingValue.StateBeforeExists = $false
    $pendingValue.StateAfter.Current = "bad"
    Write-CodexDevJson -Path $malformedPendingPaths.Pending -Value $pendingValue
    $malformedPendingBefore = Get-Content -LiteralPath $malformedPendingPaths.Pending -Raw
    Assert-TransactionTestEqual `
        "pending_invalid" `
        (Get-CodexDevDeploymentStatus `
            -ConfigPath $malformedPending.Config `
            -DeploymentRoot $malformedPending.DeploymentRoot).Status `
        "Malformed nested pending state escaped schema classification."
    Assert-TransactionTestEqual `
        $malformedPendingBefore `
        (Get-Content -LiteralPath $malformedPendingPaths.Pending -Raw) `
        "Pending classification mutated malformed valid JSON."

    $thirdImage = New-TransactionTestFixture `
        -Root (Join-Path $testRoot "third-config-image")
    $thirdImagePaths = Get-CodexDevDeploymentPaths $thirdImage.DeploymentRoot
    $thirdImageText = (Get-Content -LiteralPath $thirdImage.Config -Raw).Replace(
        "KEEP_ME = 'yes'",
        "KEEP_ME = 'external-edit'"
    )
    $script:TransactionTestThirdConfigPath = $thirdImage.Config
    $script:TransactionTestThirdConfigText = $thirdImageText
    $script:CodexDevDeploymentFaultInjector = {
        param($Stage, $Transaction)
        if ($Stage -eq "AfterPending") {
            [System.IO.File]::WriteAllText(
                $script:TransactionTestThirdConfigPath,
                $script:TransactionTestThirdConfigText,
                [System.Text.UTF8Encoding]::new($false)
            )
            throw "injected unrelated config edit"
        }
    }
    try {
        $null = Assert-TransactionTestThrows `
            -Pattern "restoration was incomplete" `
            -Message "Caught failure overwrote a third config image." `
            -Body {
                Install-CodexDevPackage `
                    -PackageDirectory $thirdImage.Package `
                    -ConfigPath $thirdImage.Config `
                    -DeploymentRoot $thirdImage.DeploymentRoot `
                    -SkipSmoke
            }
    } finally {
        $script:CodexDevDeploymentFaultInjector = $null
        Remove-Variable -Scope Script -Name TransactionTestThirdConfigPath
        Remove-Variable -Scope Script -Name TransactionTestThirdConfigText
    }
    Assert-TransactionTestEqual `
        $thirdImageText `
        (Get-Content -LiteralPath $thirdImage.Config -Raw) `
        "Caught restoration overwrote an unrelated config edit."
    Assert-TransactionTestTrue `
        (Test-Path -LiteralPath $thirdImagePaths.Pending -PathType Leaf) `
        "Caught restoration discarded its journal after config drift."
    Assert-TransactionTestEqual `
        "pending_drift" `
        (Get-CodexDevDeploymentStatus `
            -ConfigPath $thirdImage.Config `
            -DeploymentRoot $thirdImage.DeploymentRoot).Status `
        "Third config image was not exposed as pending drift."

    $thirdState = New-TransactionTestFixture `
        -Root (Join-Path $testRoot "third-state-snapshot")
    $thirdStatePaths = Get-CodexDevDeploymentPaths $thirdState.DeploymentRoot
    $script:CodexDevDeploymentFaultInjector = {
        param($Stage, $Transaction)
        if ($Stage -eq "AfterPending") { return "Interrupt" }
    }
    try {
        $null = Assert-TransactionTestThrows `
            -Pattern "Simulated interruption" `
            -Message "Third-state fixture did not retain a journal." `
            -Body {
                Install-CodexDevPackage `
                    -PackageDirectory $thirdState.Package `
                    -ConfigPath $thirdState.Config `
                    -DeploymentRoot $thirdState.DeploymentRoot `
                    -SkipSmoke
            }
    } finally {
        $script:CodexDevDeploymentFaultInjector = $null
    }
    Write-CodexDevJson -Path $thirdStatePaths.State -Value ([ordered]@{
        SchemaVersion = 1
        ConfigPath = $thirdState.Config
        Current = [ordered]@{
            ReleaseId = "external-state"
            Entrypoint = $thirdState.OldEntrypoint
            Fingerprint = $null
        }
        Previous = $null
        LastConfigBackup = $null
        ExternalSentinel = "preserve-me"
    })
    $thirdStateConfigBefore = Get-Content -LiteralPath $thirdState.Config -Raw
    $thirdStateStateBefore = Get-Content -LiteralPath $thirdStatePaths.State -Raw
    $thirdStatePendingBefore = Get-Content -LiteralPath $thirdStatePaths.Pending -Raw
    $thirdStateStatus = Get-CodexDevDeploymentStatus `
        -ConfigPath $thirdState.Config `
        -DeploymentRoot $thirdState.DeploymentRoot
    Assert-TransactionTestEqual `
        "pending_drift" `
        $thirdStateStatus.Status `
        "Third state snapshot was not exposed as pending drift."
    Assert-TransactionTestEqual `
        "ambiguous_state" `
        $thirdStateStatus.PendingDisposition `
        "Third state snapshot was not classified independently of config."
    $null = Assert-TransactionTestThrows `
        -Pattern "cannot be recovered automatically" `
        -Message "Deploy overwrote a third state snapshot." `
        -Body {
            Install-CodexDevPackage `
                -PackageDirectory $thirdState.Package `
                -ConfigPath $thirdState.Config `
                -DeploymentRoot $thirdState.DeploymentRoot `
                -SkipSmoke
        }
    $null = Assert-TransactionTestThrows `
        -Pattern "Cannot roll back" `
        -Message "Rollback overwrote a third state snapshot." `
        -Body {
            Invoke-CodexDevRollback `
                -ConfigPath $thirdState.Config `
                -DeploymentRoot $thirdState.DeploymentRoot
        }
    Assert-TransactionTestEqual `
        $thirdStateConfigBefore `
        (Get-Content -LiteralPath $thirdState.Config -Raw) `
        "Drift refusal mutated config around a third state snapshot."
    Assert-TransactionTestEqual `
        $thirdStateStateBefore `
        (Get-Content -LiteralPath $thirdStatePaths.State -Raw) `
        "Drift refusal mutated the third state snapshot."
    Assert-TransactionTestEqual `
        $thirdStatePendingBefore `
        (Get-Content -LiteralPath $thirdStatePaths.Pending -Raw) `
        "Drift refusal mutated the pending journal."

    $drift = New-TransactionTestFixture -Root (Join-Path $testRoot "ordinary-drift")
    $driftFirst = Install-CodexDevPackage `
        -PackageDirectory $drift.Package `
        -ConfigPath $drift.Config `
        -DeploymentRoot $drift.DeploymentRoot `
        -SkipSmoke
    $driftSecondPackage = New-TransactionTestPackage `
        -Path (Join-Path $drift.Root "package-two") `
        -Marker "different" `
        -BuildId "drift-build-two"
    $driftPlan = Install-CodexDevPackage `
        -PackageDirectory $driftSecondPackage `
        -ConfigPath $drift.Config `
        -DeploymentRoot $drift.DeploymentRoot `
        -SkipSmoke `
        -WhatIf
    $driftCandidate = Join-Path $driftPlan.ReleasePath "bin\codex.exe"
    Set-CodexCliPathInConfig -ConfigPath $drift.Config -Entrypoint $driftCandidate
    $oneSidedStatus = Get-CodexDevDeploymentStatus `
        -ConfigPath $drift.Config `
        -DeploymentRoot $drift.DeploymentRoot
    Assert-TransactionTestEqual `
        "config_mirror_mismatch" `
        $oneSidedStatus.Status `
        "One-sided config drift was not exposed separately."
    $null = Assert-TransactionTestThrows `
        -Pattern "not settled: config_mirror_mismatch" `
        -Message "Deploy journaled ConfiguredBefore equal to candidate while state drifted." `
        -Body {
            Install-CodexDevPackage `
                -PackageDirectory $driftSecondPackage `
                -ConfigPath $drift.Config `
                -DeploymentRoot $drift.DeploymentRoot `
                -SkipSmoke
        }
    Assert-TransactionTestTrue `
        (-not (Test-Path -LiteralPath (
            Get-CodexDevDeploymentPaths $drift.DeploymentRoot
        ).Pending)) `
        "Ordinary drift created an unrecoverable pending transaction."

    $configOnlyMirror = New-TransactionTestFixture `
        -Root (Join-Path $testRoot "config-only-mirror")
    $null = Set-CodexDevPersistentSelector -Entrypoint $null
    $configOnlyMirrorDeploy = Install-CodexDevPackage `
        -PackageDirectory $configOnlyMirror.Package `
        -ConfigPath $configOnlyMirror.Config `
        -DeploymentRoot $configOnlyMirror.DeploymentRoot `
        -SkipSmoke
    Assert-TransactionTestTrue `
        ($null -eq $configOnlyMirrorDeploy.Previous) `
        "Deploy promoted a config-only mirror into the authoritative rollback candidate."
    $configOnlyMirrorState = Read-CodexDevJsonFile (
        Get-CodexDevDeploymentPaths $configOnlyMirror.DeploymentRoot
    ).State
    Assert-TransactionTestTrue `
        ($null -eq $configOnlyMirrorState.Value.Previous) `
        "Deployment state retained a config-only mirror as Previous."
    Assert-TransactionTestEqual `
        $configOnlyMirrorDeploy.Release.Entrypoint `
        (Get-CodexDevPersistentSelector) `
        "Deploy did not establish the requested release as the persistent selector."
    $configOnlyMirrorRollback = Invoke-CodexDevRollback `
        -ConfigPath $configOnlyMirror.Config `
        -DeploymentRoot $configOnlyMirror.DeploymentRoot `
        -WhatIf
    Assert-TransactionTestEqual `
        "blocked" `
        $configOnlyMirrorRollback.Status `
        "Rollback treated a stale config-only mirror as an authoritative previous selection."

    $externalRollback = New-TransactionTestFixture `
        -Root (Join-Path $testRoot "external-rollback-adoption")
    $externalRollbackDeploy = Install-CodexDevPackage `
        -PackageDirectory $externalRollback.Package `
        -ConfigPath $externalRollback.Config `
        -DeploymentRoot $externalRollback.DeploymentRoot `
        -SkipSmoke
    Set-CodexCliPathInConfig `
        -ConfigPath $externalRollback.Config `
        -Entrypoint $externalRollback.OldEntrypoint
    $null = Set-CodexDevPersistentSelector `
        -Entrypoint $externalRollback.OldEntrypoint
    $externalRollbackPaths = Get-CodexDevDeploymentPaths $externalRollback.DeploymentRoot
    $externalRollbackConfigBefore = Get-Content -LiteralPath $externalRollback.Config -Raw
    $externalRollbackStateBefore = Get-TransactionStateJson $externalRollbackPaths.State
    $externalRollbackStatus = Get-CodexDevDeploymentStatus `
        -ConfigPath $externalRollback.Config `
        -DeploymentRoot $externalRollback.DeploymentRoot
    Assert-TransactionTestEqual `
        "previous_configured_state_stale" `
        $externalRollbackStatus.Status `
        "A config matching recorded Previous was classified as arbitrary drift."
    $externalRollbackPlan = Invoke-CodexDevRollback `
        -ConfigPath $externalRollback.Config `
        -DeploymentRoot $externalRollback.DeploymentRoot `
        -WhatIf
    Assert-TransactionTestEqual `
        "planned_with_recovery" `
        $externalRollbackPlan.Status `
        "Rollback WhatIf did not plan adoption of an external previous selection."
    Assert-TransactionTestEqual `
        "settle_configured_previous" `
        $externalRollbackPlan.RecoveryDisposition `
        "Rollback WhatIf returned the wrong external-selection disposition."
    Assert-TransactionTestEqual `
        $externalRollbackConfigBefore `
        (Get-Content -LiteralPath $externalRollback.Config -Raw) `
        "External rollback WhatIf mutated config."
    Assert-TransactionTestEqual `
        $externalRollbackStateBefore `
        (Get-TransactionStateJson $externalRollbackPaths.State) `
        "External rollback WhatIf mutated deployment state."
    $externalRollbackAdopted = Invoke-CodexDevRollback `
        -ConfigPath $externalRollback.Config `
        -DeploymentRoot $externalRollback.DeploymentRoot
    Assert-TransactionTestEqual `
        "configured_previous_settled" `
        $externalRollbackAdopted.Status `
        "Rollback did not adopt the already-selected previous entrypoint."
    Assert-TransactionTestEqual `
        $externalRollbackConfigBefore `
        (Get-Content -LiteralPath $externalRollback.Config -Raw) `
        "External rollback adoption rewrote config."
    $externalRollbackSettled = Get-CodexDevDeploymentStatus `
        -ConfigPath $externalRollback.Config `
        -DeploymentRoot $externalRollback.DeploymentRoot
    Assert-TransactionTestEqual `
        "consistent" `
        $externalRollbackSettled.Status `
        "External rollback adoption did not settle deployment state."
    Assert-TransactionTestEqual `
        $externalRollback.OldEntrypoint `
        $externalRollbackSettled.State.Current.Entrypoint `
        "External rollback adoption did not promote the observed selector to Current."
    Assert-TransactionTestEqual `
        $externalRollbackDeploy.Release.Entrypoint `
        $externalRollbackSettled.State.Previous.Entrypoint `
        "External rollback adoption did not retain the displaced release as Previous."
    Assert-TransactionTestTrue `
        (-not (Test-Path -LiteralPath $externalRollbackPaths.Pending)) `
        "External rollback adoption created a pending config transaction."

    $interruptedSettlement = New-TransactionTestFixture `
        -Root (Join-Path $testRoot "interrupted-previous-settlement")
    $interruptedSettlementDeploy = Install-CodexDevPackage `
        -PackageDirectory $interruptedSettlement.Package `
        -ConfigPath $interruptedSettlement.Config `
        -DeploymentRoot $interruptedSettlement.DeploymentRoot `
        -SkipSmoke
    Set-CodexCliPathInConfig `
        -ConfigPath $interruptedSettlement.Config `
        -Entrypoint $interruptedSettlement.OldEntrypoint
    $null = Set-CodexDevPersistentSelector `
        -Entrypoint $interruptedSettlement.OldEntrypoint
    $interruptedSettlementPaths = Get-CodexDevDeploymentPaths `
        $interruptedSettlement.DeploymentRoot
    $script:CodexDevDeploymentFaultInjector = {
        param($Stage, $Transaction)
        if ($Stage -eq "AfterState") { return "Interrupt" }
    }
    $null = Assert-TransactionTestThrows `
        -Pattern "Simulated interruption" `
        -Message "Configured-Previous settlement did not retain its AfterState journal." `
        -Body {
            Invoke-CodexDevRollback `
                -ConfigPath $interruptedSettlement.Config `
                -DeploymentRoot $interruptedSettlement.DeploymentRoot
        }
    $script:CodexDevDeploymentFaultInjector = $null
    $interruptedSettlementStatus = Get-CodexDevDeploymentStatus `
        -ConfigPath $interruptedSettlement.Config `
        -DeploymentRoot $interruptedSettlement.DeploymentRoot
    Assert-TransactionTestEqual `
        "pending_recoverable_after" `
        $interruptedSettlementStatus.Status `
        "AfterState settlement interruption was not recoverable to its completed Rollback."
    Assert-TransactionTestEqual `
        "Rollback" `
        ([string]$interruptedSettlementStatus.PendingTransaction.Action) `
        "State-only settlement journal lost the owning Rollback action."
    $interruptedSettlementRetry = Invoke-CodexDevRollback `
        -ConfigPath $interruptedSettlement.Config `
        -DeploymentRoot $interruptedSettlement.DeploymentRoot
    Assert-TransactionTestEqual `
        "recovered_after" `
        $interruptedSettlementRetry.Recovery.Status `
        "Retry did not complete the interrupted state-only Rollback settlement."
    $interruptedSettlementFinal = Read-CodexDevJsonFile $interruptedSettlementPaths.State
    Assert-TransactionTestEqual `
        $interruptedSettlement.OldEntrypoint `
        $interruptedSettlementFinal.Value.Current.Entrypoint `
        "Retry toggled the settled Previous entrypoint back out of Current."
    Assert-TransactionTestEqual `
        $interruptedSettlementDeploy.Release.Entrypoint `
        $interruptedSettlementFinal.Value.Previous.Entrypoint `
        "Retry did not retain the displaced managed release as Previous."
    Assert-TransactionTestTrue `
        (-not (Test-Path -LiteralPath $interruptedSettlementPaths.Pending)) `
        "Retry left the completed settlement transaction pending."

    $stalePlanRace = New-TransactionTestFixture `
        -Root (Join-Path $testRoot "stale-plan-settlement-race")
    $stalePlanRaceDeploy = Install-CodexDevPackage `
        -PackageDirectory $stalePlanRace.Package `
        -ConfigPath $stalePlanRace.Config `
        -DeploymentRoot $stalePlanRace.DeploymentRoot `
        -SkipSmoke
    Set-CodexCliPathInConfig `
        -ConfigPath $stalePlanRace.Config `
        -Entrypoint $stalePlanRace.OldEntrypoint
    $null = Set-CodexDevPersistentSelector `
        -Entrypoint $stalePlanRace.OldEntrypoint
    $stalePlanRacePaths = Get-CodexDevDeploymentPaths $stalePlanRace.DeploymentRoot
    $stalePlanRaceStatus = Get-CodexDevDeploymentStatus `
        -ConfigPath $stalePlanRace.Config `
        -DeploymentRoot $stalePlanRace.DeploymentRoot
    $script:TransactionTestStaleSettlementPlan = Get-CodexDevRollbackPlan `
        -DeploymentStatus $stalePlanRaceStatus `
        -ConfigPath $stalePlanRace.Config
    $script:TransactionTestOriginalDeploymentLock = (
        Get-Command Invoke-WithCodexDevDeploymentLock
    ).ScriptBlock
    $script:TransactionTestDeploymentLockCalls = 0
    $script:TransactionTestRaceConfig = $stalePlanRace.Config
    $script:TransactionTestRaceRoot = $stalePlanRace.DeploymentRoot
    Set-Item -LiteralPath Function:\Invoke-WithCodexDevDeploymentLock -Value {
        param(
            [string]$DeploymentRoot,
            [scriptblock]$Body,
            [int]$LockTimeoutMilliseconds = 15000
        )

        $script:TransactionTestDeploymentLockCalls++
        if ($script:TransactionTestDeploymentLockCalls -eq 2) {
            $script:CodexDevDeploymentFaultInjector = {
                param($Stage, $Transaction)
                if ($Stage -eq "AfterState") { return "Interrupt" }
            }
            try {
                & $script:TransactionTestOriginalDeploymentLock `
                    -DeploymentRoot $script:TransactionTestRaceRoot `
                    -LockTimeoutMilliseconds $LockTimeoutMilliseconds `
                    -Body {
                        Invoke-WithCodexDevConfigLock `
                            -ConfigPath $script:TransactionTestRaceConfig `
                            -Body {
                                Complete-CodexDevConfiguredPreviousSettlement `
                                    -Action "Rollback" `
                                    -SettlementPlan $script:TransactionTestStaleSettlementPlan.ConfiguredPreviousSettlement `
                                    -ConfigPath $script:TransactionTestRaceConfig `
                                    -DeploymentRoot $script:TransactionTestRaceRoot
                            }
                    }
            } catch {
                if ($_.Exception.Message -notmatch "Simulated interruption") {
                    throw
                }
            } finally {
                $script:CodexDevDeploymentFaultInjector = $null
            }
        }
        return & $script:TransactionTestOriginalDeploymentLock `
            -DeploymentRoot $DeploymentRoot `
            -Body $Body `
            -LockTimeoutMilliseconds $LockTimeoutMilliseconds
    }
    try {
        $stalePlanRaceResult = Invoke-CodexDevRollback `
            -ConfigPath $stalePlanRace.Config `
            -DeploymentRoot $stalePlanRace.DeploymentRoot
    } finally {
        Set-Item `
            -LiteralPath Function:\Invoke-WithCodexDevDeploymentLock `
            -Value $script:TransactionTestOriginalDeploymentLock
        $script:CodexDevDeploymentFaultInjector = $null
    }
    Assert-TransactionTestEqual `
        "previous_selected_for_restart" `
        $stalePlanRaceResult.Status `
        "Stale rollback plan did not complete the winner's journaled settlement."
    Assert-TransactionTestEqual `
        "recovered_after" `
        $stalePlanRaceResult.Recovery.Status `
        "Stale rollback plan bypassed the winner's recoverable-after journal."
    Assert-TransactionTestTrue `
        (-not (Test-Path -LiteralPath $stalePlanRacePaths.Pending)) `
        "Stale rollback plan claimed success while the winner's journal remained pending."
    $stalePlanRaceFinal = Read-CodexDevJsonFile $stalePlanRacePaths.State
    Assert-TransactionTestEqual `
        $stalePlanRace.OldEntrypoint `
        $stalePlanRaceFinal.Value.Current.Entrypoint `
        "Stale rollback plan toggled the winner's settled Previous back out of Current."
    Assert-TransactionTestEqual `
        $stalePlanRaceDeploy.Release.Entrypoint `
        $stalePlanRaceFinal.Value.Previous.Entrypoint `
        "Stale rollback plan lost the displaced managed release."

    $managedSettlement = New-TransactionTestFixture `
        -Root (Join-Path $testRoot "managed-previous-settlement")
    $managedSettlementPackageTwo = New-TransactionTestPackage `
        -Path (Join-Path $managedSettlement.Root "package-two") `
        -Marker "managed-settlement-two" `
        -BuildId "managed-settlement-build-two"
    $managedSettlementFirst = Install-CodexDevPackage `
        -PackageDirectory $managedSettlement.Package `
        -ConfigPath $managedSettlement.Config `
        -DeploymentRoot $managedSettlement.DeploymentRoot `
        -SkipSmoke
    $managedSettlementSecond = Install-CodexDevPackage `
        -PackageDirectory $managedSettlementPackageTwo `
        -ConfigPath $managedSettlement.Config `
        -DeploymentRoot $managedSettlement.DeploymentRoot `
        -SkipSmoke
    $managedSettlementPaths = Get-CodexDevDeploymentPaths $managedSettlement.DeploymentRoot
    $managedSettlementStateBefore = Read-CodexDevJsonFile $managedSettlementPaths.State
    $managedSettlementCurrentBefore = $managedSettlementStateBefore.Value.Current
    $managedSettlementPreviousBefore = $managedSettlementStateBefore.Value.Previous
    $managedSettlementStateBefore.Value | Add-Member `
        -MemberType NoteProperty `
        -Name "TestSentinel" `
        -Value "retain-me" `
        -Force
    Write-CodexDevJson `
        -Path $managedSettlementPaths.State `
        -Value $managedSettlementStateBefore.Value
    $managedSettlementLastBackup = $managedSettlementStateBefore.Value.LastConfigBackup
    $managedSettlementBackupCount = @(
        Get-ChildItem `
            -LiteralPath (Join-Path $managedSettlement.DeploymentRoot "config-backups") `
            -File
    ).Count
    Set-CodexCliPathInConfig `
        -ConfigPath $managedSettlement.Config `
        -Entrypoint $managedSettlementPreviousBefore.Entrypoint
    $null = Set-CodexDevPersistentSelector `
        -Entrypoint $managedSettlementPreviousBefore.Entrypoint
    $managedSettlementConfigBefore = Get-Content -LiteralPath $managedSettlement.Config -Raw
    $managedSettlementResult = Invoke-CodexDevRollback `
        -ConfigPath $managedSettlement.Config `
        -DeploymentRoot $managedSettlement.DeploymentRoot
    Assert-TransactionTestEqual `
        "configured_previous_settled" `
        $managedSettlementResult.Status `
        "Managed configured-Previous settlement did not complete."
    Assert-TransactionTestEqual `
        $managedSettlementConfigBefore `
        (Get-Content -LiteralPath $managedSettlement.Config -Raw) `
        "Managed configured-Previous settlement rewrote config."
    $managedSettlementStateAfter = Read-CodexDevJsonFile $managedSettlementPaths.State
    Assert-TransactionTestEqual `
        (Get-TransactionReleaseIdentityJson $managedSettlementPreviousBefore) `
        (Get-TransactionReleaseIdentityJson $managedSettlementStateAfter.Value.Current) `
        "Managed configured-Previous settlement did not promote Previous exactly."
    Assert-TransactionTestEqual `
        (Get-TransactionReleaseIdentityJson $managedSettlementCurrentBefore) `
        (Get-TransactionReleaseIdentityJson $managedSettlementStateAfter.Value.Previous) `
        "Managed configured-Previous settlement did not retain displaced Current exactly."
    Assert-TransactionTestEqual `
        $managedSettlementLastBackup `
        $managedSettlementStateAfter.Value.LastConfigBackup `
        "Managed configured-Previous settlement changed the retained backup pointer."
    Assert-TransactionTestEqual `
        "retain-me" `
        $managedSettlementStateAfter.Value.TestSentinel `
        "Managed configured-Previous settlement discarded an unknown state property."
    Assert-TransactionTestEqual `
        $managedSettlementBackupCount `
        @(
            Get-ChildItem `
                -LiteralPath (Join-Path $managedSettlement.DeploymentRoot "config-backups") `
                -File
        ).Count `
        "Managed configured-Previous settlement created a config backup."
    $managedSettlementNextRollback = Invoke-CodexDevRollback `
        -ConfigPath $managedSettlement.Config `
        -DeploymentRoot $managedSettlement.DeploymentRoot `
        -WhatIf
    Assert-TransactionTestEqual `
        "planned" `
        $managedSettlementNextRollback.Status `
        "Managed settlement did not preserve a usable rollback candidate."
    Assert-TransactionTestEqual `
        $managedSettlementCurrentBefore.Entrypoint `
        $managedSettlementNextRollback.ConfiguredAfter `
        "Managed settlement did not make displaced Current the next rollback candidate."

    $deploySettlement = New-TransactionTestFixture `
        -Root (Join-Path $testRoot "deploy-with-previous-settlement")
    $deploySettlementFirst = Install-CodexDevPackage `
        -PackageDirectory $deploySettlement.Package `
        -ConfigPath $deploySettlement.Config `
        -DeploymentRoot $deploySettlement.DeploymentRoot `
        -SkipSmoke
    Set-CodexCliPathInConfig `
        -ConfigPath $deploySettlement.Config `
        -Entrypoint $deploySettlement.OldEntrypoint
    $null = Set-CodexDevPersistentSelector `
        -Entrypoint $deploySettlement.OldEntrypoint
    $deploySettlementPlan = Install-CodexDevPackage `
        -PackageDirectory $deploySettlement.Package `
        -ConfigPath $deploySettlement.Config `
        -DeploymentRoot $deploySettlement.DeploymentRoot `
        -SkipSmoke `
        -WhatIf
    Assert-TransactionTestEqual `
        "planned_with_recovery" `
        $deploySettlementPlan.Status `
        "Deploy WhatIf did not include configured-Previous settlement."
    Assert-TransactionTestEqual `
        "settle_configured_previous" `
        $deploySettlementPlan.RecoveryDisposition `
        "Deploy WhatIf returned the wrong settlement disposition."
    $deploySettlementResult = Install-CodexDevPackage `
        -PackageDirectory $deploySettlement.Package `
        -ConfigPath $deploySettlement.Config `
        -DeploymentRoot $deploySettlement.DeploymentRoot `
        -SkipSmoke
    Assert-TransactionTestEqual `
        "selected_for_restart" `
        $deploySettlementResult.Status `
        "Deploy did not continue after configured-Previous settlement."
    Assert-TransactionTestEqual `
        "configured_previous_settled" `
        $deploySettlementResult.Recovery.Status `
        "Deploy did not report its configured-Previous settlement."
    Assert-TransactionTestEqual `
        $deploySettlementFirst.Release.Entrypoint `
        (Get-CodexCliPathFromConfig $deploySettlement.Config) `
        "Deploy did not select the requested release after settlement."
    Assert-TransactionTestEqual `
        $deploySettlementFirst.Release.Entrypoint `
        (Get-CodexDevPersistentSelector) `
        "Deploy did not reselect the requested release in the persistent selector."
    Assert-TransactionTestEqual `
        "consistent" `
        (Get-CodexDevDeploymentStatus `
            -ConfigPath $deploySettlement.Config `
            -DeploymentRoot $deploySettlement.DeploymentRoot).Status `
        "Deploy did not leave state consistent after settlement."
    $processSelectorBeforeRegression = $env:CODEX_CLI_PATH
    try {
        $env:CODEX_CLI_PATH = $deploySettlementFirst.Release.Entrypoint
        $deploySettlementVerify = Invoke-TransactionVerifySkippingSmoke `
            -ConfigPath $deploySettlement.Config `
            -DeploymentRoot $deploySettlement.DeploymentRoot
        Assert-TransactionTestEqual `
            "live" `
            $deploySettlementVerify.Status `
            "Verify did not become live after simulating the next process startup snapshot."
        Assert-TransactionTestEqual `
            $deploySettlementFirst.Release.Entrypoint `
            $deploySettlementVerify.PersistentEntrypoint `
            "Verify did not expose the persistent next-launch selector."
        Assert-TransactionTestEqual `
            $deploySettlementFirst.Release.Entrypoint `
            $deploySettlementVerify.ConfiguredEntrypoint `
            "Verify did not expose the config mirror separately."
        Assert-TransactionTestEqual `
            $deploySettlementFirst.Release.Entrypoint `
            $deploySettlementVerify.ProcessLiveEntrypoint `
            "Verify did not expose the process-live startup snapshot separately."
        $verifyStateDriftPaths = Get-CodexDevDeploymentPaths `
            $deploySettlement.DeploymentRoot
        $script:TransactionTestPackageInfoHook = {
            param($PackageDirectory, $Package)
            if (Test-CodexDevPathEqual `
                -Left $PackageDirectory `
                -Right $deploySettlementFirst.Release.ReleasePath) {
                $script:TransactionTestPackageInfoHook = $null
                $stateDuringVerify = Read-CodexDevJsonFile $verifyStateDriftPaths.State
                $stateDuringVerify.Value | Add-Member `
                    -MemberType NoteProperty `
                    -Name "VerificationRaceSentinel" `
                    -Value "changed-after-package-read" `
                    -Force
                Write-CodexDevJson `
                    -Path $verifyStateDriftPaths.State `
                    -Value $stateDuringVerify.Value
            }
        }
        try {
            $verifyStateDrift = Invoke-TransactionVerifySkippingSmoke `
                -ConfigPath $deploySettlement.Config `
                -DeploymentRoot $deploySettlement.DeploymentRoot
        } finally {
            $script:TransactionTestPackageInfoHook = $null
        }
        Assert-TransactionTestEqual `
            "drift" `
            $verifyStateDrift.Status `
            "Verify ignored deployment-state drift after package verification began."
        Assert-TransactionTestTrue `
            ($verifyStateDrift.DriftReasons -contains (
                "Deployment state changed during deployment verification."
            )) `
            "Verify did not report its failed closing deployment-state attestation."
    } finally {
        if ($null -eq $processSelectorBeforeRegression) {
            Remove-Item Env:CODEX_CLI_PATH -ErrorAction SilentlyContinue
        } else {
            $env:CODEX_CLI_PATH = $processSelectorBeforeRegression
        }
    }

    $deploySettledCandidate = New-TransactionTestFixture `
        -Root (Join-Path $testRoot "deploy-settled-candidate")
    $deploySettledCandidatePackageTwo = New-TransactionTestPackage `
        -Path (Join-Path $deploySettledCandidate.Root "package-two") `
        -Marker "deploy-settled-candidate-two" `
        -BuildId "deploy-settled-candidate-build-two"
    $deploySettledCandidateFirst = Install-CodexDevPackage `
        -PackageDirectory $deploySettledCandidate.Package `
        -ConfigPath $deploySettledCandidate.Config `
        -DeploymentRoot $deploySettledCandidate.DeploymentRoot `
        -SkipSmoke
    $null = Install-CodexDevPackage `
        -PackageDirectory $deploySettledCandidatePackageTwo `
        -ConfigPath $deploySettledCandidate.Config `
        -DeploymentRoot $deploySettledCandidate.DeploymentRoot `
        -SkipSmoke
    Set-CodexCliPathInConfig `
        -ConfigPath $deploySettledCandidate.Config `
        -Entrypoint $deploySettledCandidateFirst.Release.Entrypoint
    $null = Set-CodexDevPersistentSelector `
        -Entrypoint $deploySettledCandidateFirst.Release.Entrypoint
    $deploySettledCandidateResult = Install-CodexDevPackage `
        -PackageDirectory $deploySettledCandidate.Package `
        -ConfigPath $deploySettledCandidate.Config `
        -DeploymentRoot $deploySettledCandidate.DeploymentRoot `
        -SkipSmoke
    Assert-TransactionTestEqual `
        "already_selected" `
        $deploySettledCandidateResult.Status `
        "Deploy did not recognize the candidate selected by state settlement."
    Assert-TransactionTestEqual `
        "configured_previous_settled" `
        $deploySettledCandidateResult.Recovery.Status `
        "Already-selected Deploy did not retain its settlement evidence."
    Assert-TransactionTestEqual `
        "written" `
        $deploySettledCandidateResult.ReceiptStatus `
        "Already-selected Deploy with settlement did not write a recovery receipt."

    $adoption = New-TransactionTestFixture -Root (Join-Path $testRoot "state-only-adoption")
    $adoptionPlan = Install-CodexDevPackage `
        -PackageDirectory $adoption.Package `
        -ConfigPath $adoption.Config `
        -DeploymentRoot $adoption.DeploymentRoot `
        -SkipSmoke `
        -WhatIf
    $adoptionCandidate = Join-Path $adoptionPlan.ReleasePath "bin\codex.exe"
    Set-CodexCliPathInConfig `
        -ConfigPath $adoption.Config `
        -Entrypoint $adoptionCandidate
    $null = Set-CodexDevPersistentSelector -Entrypoint $adoptionCandidate
    $adopted = Install-CodexDevPackage `
        -PackageDirectory $adoption.Package `
        -ConfigPath $adoption.Config `
        -DeploymentRoot $adoption.DeploymentRoot `
        -SkipSmoke
    Assert-TransactionTestEqual `
        "adopted_existing_selection" `
        $adopted.Status `
        "Unmanaged config already at the candidate was not handled as state-only adoption."
    Assert-TransactionTestEqual `
        "consistent" `
        (Get-CodexDevDeploymentStatus `
            -ConfigPath $adoption.Config `
            -DeploymentRoot $adoption.DeploymentRoot).Status `
        "State-only adoption did not settle deployment state."
    Assert-TransactionTestTrue `
        (-not (Test-Path -LiteralPath (
            Get-CodexDevDeploymentPaths $adoption.DeploymentRoot
        ).Pending)) `
        "State-only adoption created an equal-selector pending transaction."

    $rollback = New-TransactionTestFixture -Root (Join-Path $testRoot "rollback")
    $rollbackPackageTwo = New-TransactionTestPackage `
        -Path (Join-Path $rollback.Root "package-two") `
        -Marker "package-two" `
        -BuildId "rollback-build-two"
    $rollbackFirst = Install-CodexDevPackage `
        -PackageDirectory $rollback.Package `
        -ConfigPath $rollback.Config `
        -DeploymentRoot $rollback.DeploymentRoot `
        -SkipSmoke
    $rollbackSecond = Install-CodexDevPackage `
        -PackageDirectory $rollbackPackageTwo `
        -ConfigPath $rollback.Config `
        -DeploymentRoot $rollback.DeploymentRoot `
        -SkipSmoke
    $rollbackPaths = Get-CodexDevDeploymentPaths $rollback.DeploymentRoot
    $rollbackConfigBefore = Get-Content -LiteralPath $rollback.Config -Raw
    $rollbackStateBefore = Get-TransactionStateJson $rollbackPaths.State
    $script:TransactionTestObservedRollbackLock = $false
    $script:CodexDevDeploymentFaultInjector = {
        param($Stage, $Transaction)
        if ($Stage -eq "AfterConfig") {
            $script:TransactionTestObservedRollbackLock = Test-TransactionExclusiveLockHeld `
                (Get-CodexDevDeploymentPaths $rollback.DeploymentRoot).Lock
            throw "injected caught rollback failure"
        }
    }
    $null = Assert-TransactionTestThrows `
        -Pattern "injected caught rollback failure" `
        -Message "Caught rollback failure was not surfaced." `
        -Body {
            Invoke-CodexDevRollback `
                -ConfigPath $rollback.Config `
                -DeploymentRoot $rollback.DeploymentRoot
        }
    $script:CodexDevDeploymentFaultInjector = $null
    Assert-TransactionTestTrue `
        $script:TransactionTestObservedRollbackLock `
        "Rollback did not hold the shared exclusive root lock."
    Assert-TransactionTestEqual `
        $rollbackConfigBefore `
        (Get-Content -LiteralPath $rollback.Config -Raw) `
        "Caught rollback failure did not restore config exactly."
    Assert-TransactionTestEqual `
        $rollbackStateBefore `
        (Get-TransactionStateJson $rollbackPaths.State) `
        "Caught rollback failure did not restore state."
    Assert-TransactionTestTrue `
        (-not (Test-Path -LiteralPath $rollbackPaths.Pending)) `
        "Caught rollback failure retained its pending transaction."
    $rolledBack = Invoke-CodexDevRollback `
        -ConfigPath $rollback.Config `
        -DeploymentRoot $rollback.DeploymentRoot
    Assert-TransactionTestEqual `
        $rollbackFirst.Release.Entrypoint `
        $rolledBack.ConfiguredEntrypoint `
        "Rollback did not select the prior release after restoration."
    Assert-TransactionTestTrue `
        (@($script:TransactionTestPackageInfoCalls | Where-Object {
            (Test-CodexDevPathEqual `
                -Left $_.PackageDirectory `
                -Right $rollbackFirst.Release.ReleasePath) -and
            -not $_.SkipSmokeRequested
        }).Count -gt 0) `
        "Managed Rollback did not request full package smoke before mutation."

    $tamperedRollback = New-TransactionTestFixture `
        -Root (Join-Path $testRoot "tampered-rollback-candidate")
    $tamperedPackageTwo = New-TransactionTestPackage `
        -Path (Join-Path $tamperedRollback.Root "package-two") `
        -Marker "tampered-package-two" `
        -BuildId "tampered-build-two"
    $tamperedFirst = Install-CodexDevPackage `
        -PackageDirectory $tamperedRollback.Package `
        -ConfigPath $tamperedRollback.Config `
        -DeploymentRoot $tamperedRollback.DeploymentRoot `
        -SkipSmoke
    $null = Install-CodexDevPackage `
        -PackageDirectory $tamperedPackageTwo `
        -ConfigPath $tamperedRollback.Config `
        -DeploymentRoot $tamperedRollback.DeploymentRoot `
        -SkipSmoke
    $tamperedPaths = Get-CodexDevDeploymentPaths $tamperedRollback.DeploymentRoot
    $tamperedNonEntrypoint = Join-Path `
        $tamperedFirst.Release.ReleasePath `
        "codex-path\rg.exe"
    $tamperedBytes = [System.IO.File]::ReadAllBytes($tamperedNonEntrypoint)
    $tamperedBytes[0x50] = $tamperedBytes[0x50] -bxor 0x01
    [System.IO.File]::WriteAllBytes($tamperedNonEntrypoint, $tamperedBytes)
    $tamperedConfigBefore = Get-Content -LiteralPath $tamperedRollback.Config -Raw
    $tamperedStateBefore = Get-TransactionStateJson $tamperedPaths.State
    $tamperedPlan = Invoke-CodexDevRollback `
        -ConfigPath $tamperedRollback.Config `
        -DeploymentRoot $tamperedRollback.DeploymentRoot `
        -WhatIf
    Assert-TransactionTestEqual `
        "blocked" `
        $tamperedPlan.Status `
        "Rollback WhatIf accepted a tampered non-entrypoint package file."
    Assert-TransactionTestTrue `
        (($tamperedPlan.Blockers -join " ") -match "fingerprint") `
        "Rollback WhatIf did not identify managed package fingerprint drift."
    $null = Assert-TransactionTestThrows `
        -Pattern "fingerprint" `
        -Message "Rollback selected a tampered managed package." `
        -Body {
            Invoke-CodexDevRollback `
                -ConfigPath $tamperedRollback.Config `
                -DeploymentRoot $tamperedRollback.DeploymentRoot
        }
    Assert-TransactionTestEqual `
        $tamperedConfigBefore `
        (Get-Content -LiteralPath $tamperedRollback.Config -Raw) `
        "Blocked tampered Rollback mutated config."
    Assert-TransactionTestEqual `
        $tamperedStateBefore `
        (Get-TransactionStateJson $tamperedPaths.State) `
        "Blocked tampered Rollback mutated state."
    Assert-TransactionTestTrue `
        (-not (Test-Path -LiteralPath $tamperedPaths.Pending)) `
        "Blocked tampered Rollback created a pending journal."

    $preLane = New-TransactionTestFixture -Root (Join-Path $testRoot "pre-lane-verify")
    $null = Install-CodexDevPackage `
        -PackageDirectory $preLane.Package `
        -ConfigPath $preLane.Config `
        -DeploymentRoot $preLane.DeploymentRoot `
        -SkipSmoke
    $preLaneRollback = Invoke-CodexDevRollback `
        -ConfigPath $preLane.Config `
        -DeploymentRoot $preLane.DeploymentRoot
    Assert-TransactionTestEqual `
        $preLane.OldEntrypoint `
        $preLaneRollback.ConfiguredEntrypoint `
        "Rollback did not restore the standalone pre-lane selector."
    $packageCallsBeforePreLaneVerify = $script:TransactionTestPackageInfoCalls.Count
    $preLaneVerify = Invoke-CodexDevVerify `
        -ConfigPath $preLane.Config `
        -DeploymentRoot $preLane.DeploymentRoot
    Assert-TransactionTestEqual `
        "consistent" `
        $preLaneVerify.DeploymentStatus `
        "Verify rejected consistent pre-lane selector state."
    Assert-TransactionTestEqual `
        "pre_lane_selector_only" `
        $preLaneVerify.VerificationMode `
        "Verify did not report its selector-only pre-lane proof boundary."
    Assert-TransactionTestTrue `
        ($null -eq $preLaneVerify.Package) `
        "Pre-lane Verify incorrectly treated a standalone exe as a canonical package."
    Assert-TransactionTestEqual `
        $packageCallsBeforePreLaneVerify `
        $script:TransactionTestPackageInfoCalls.Count `
        "Pre-lane Verify attempted canonical package validation."

    $deployReceiptFailure = New-TransactionTestFixture `
        -Root (Join-Path $testRoot "deploy-receipt-failure")
    $script:TransactionTestReceiptFailure = "injected deploy receipt failure"
    try {
        $deployWithoutReceipt = Install-CodexDevPackage `
            -PackageDirectory $deployReceiptFailure.Package `
            -ConfigPath $deployReceiptFailure.Config `
            -DeploymentRoot $deployReceiptFailure.DeploymentRoot `
            -SkipSmoke
    } finally {
        $script:TransactionTestReceiptFailure = $null
    }
    Assert-TransactionTestEqual `
        "selected_for_restart" `
        $deployWithoutReceipt.Status `
        "Receipt failure changed successful Deploy status."
    Assert-TransactionTestEqual `
        "failed_after_success" `
        $deployWithoutReceipt.ReceiptStatus `
        "Deploy did not distinguish post-success receipt failure."
    Assert-TransactionTestTrue `
        ($deployWithoutReceipt.ReceiptError -match "injected deploy receipt failure") `
        "Deploy did not return its receipt error."
    Assert-TransactionTestTrue `
        (-not $deployWithoutReceipt.RetryRequired) `
        "Deploy receipt failure incorrectly requested a selection retry."
    $alreadySelectedAfterReceiptFailure = Install-CodexDevPackage `
        -PackageDirectory $deployReceiptFailure.Package `
        -ConfigPath $deployReceiptFailure.Config `
        -DeploymentRoot $deployReceiptFailure.DeploymentRoot `
        -SkipSmoke
    Assert-TransactionTestEqual `
        "already_selected" `
        $alreadySelectedAfterReceiptFailure.Status `
        "Deploy retry after receipt failure repeated the selection."
    Assert-TransactionTestEqual `
        "not_written_already_selected" `
        $alreadySelectedAfterReceiptFailure.ReceiptStatus `
        "Already-selected Deploy receipt semantics were not explicit."

    $rollbackReceiptFailure = New-TransactionTestFixture `
        -Root (Join-Path $testRoot "rollback-receipt-failure")
    $rollbackReceiptPackageTwo = New-TransactionTestPackage `
        -Path (Join-Path $rollbackReceiptFailure.Root "package-two") `
        -Marker "rollback-receipt-package-two" `
        -BuildId "rollback-receipt-build-two"
    $rollbackReceiptFirst = Install-CodexDevPackage `
        -PackageDirectory $rollbackReceiptFailure.Package `
        -ConfigPath $rollbackReceiptFailure.Config `
        -DeploymentRoot $rollbackReceiptFailure.DeploymentRoot `
        -SkipSmoke
    $null = Install-CodexDevPackage `
        -PackageDirectory $rollbackReceiptPackageTwo `
        -ConfigPath $rollbackReceiptFailure.Config `
        -DeploymentRoot $rollbackReceiptFailure.DeploymentRoot `
        -SkipSmoke
    $script:TransactionTestReceiptFailure = "injected rollback receipt failure"
    try {
        $rollbackWithoutReceipt = Invoke-CodexDevRollback `
            -ConfigPath $rollbackReceiptFailure.Config `
            -DeploymentRoot $rollbackReceiptFailure.DeploymentRoot
    } finally {
        $script:TransactionTestReceiptFailure = $null
    }
    Assert-TransactionTestEqual `
        "previous_selected_for_restart" `
        $rollbackWithoutReceipt.Status `
        "Receipt failure changed successful Rollback status."
    Assert-TransactionTestEqual `
        "failed_after_success" `
        $rollbackWithoutReceipt.ReceiptStatus `
        "Rollback did not distinguish post-success receipt failure."
    Assert-TransactionTestTrue `
        ($rollbackWithoutReceipt.ReceiptError -match "injected rollback receipt failure") `
        "Rollback did not return its receipt error."
    Assert-TransactionTestEqual `
        $rollbackReceiptFirst.Release.Entrypoint `
        (Get-CodexCliPathFromConfig $rollbackReceiptFailure.Config) `
        "Rollback receipt failure obscured a successful selection."

    $occurrenceRoot = Join-Path $testRoot "occurrence"
    $occurrence = New-TransactionTestFixture `
        -Root $occurrenceRoot `
        -Marker "identical-artifacts" `
        -BuildId "build-occurrence-one"
    $occurrencePackageTwo = New-TransactionTestPackage `
        -Path (Join-Path $occurrenceRoot "same-artifact-new-build") `
        -Marker "identical-artifacts" `
        -BuildId "build-occurrence-two"
    $occurrenceFirst = Install-CodexDevPackage `
        -PackageDirectory $occurrence.Package `
        -ConfigPath $occurrence.Config `
        -DeploymentRoot $occurrence.DeploymentRoot `
        -SkipSmoke
    $occurrenceSecond = Install-CodexDevPackage `
        -PackageDirectory $occurrencePackageTwo `
        -ConfigPath $occurrence.Config `
        -DeploymentRoot $occurrence.DeploymentRoot `
        -SkipSmoke
    Assert-TransactionTestEqual `
        "selection_recorded" `
        $occurrenceSecond.Status `
        "Identical artifacts from a new build occurrence were treated as the old selection."
    Assert-TransactionTestEqual `
        $occurrenceFirst.Release.ReleasePath `
        $occurrenceSecond.Release.ReleasePath `
        "Artifact-identical packages did not deduplicate to one immutable release."
    $occurrenceState = Get-Content `
        -LiteralPath (Get-CodexDevDeploymentPaths $occurrence.DeploymentRoot).State `
        -Raw | ConvertFrom-Json
    Assert-TransactionTestEqual `
        "build-occurrence-two" `
        ([string]$occurrenceState.Current.BuildId) `
        "Deployment state lost the selected source package BuildId."
    Assert-TransactionTestEqual `
        $occurrencePackageTwo `
        ([string]$occurrenceState.Current.SourcePackageRoot) `
        "Deployment state points at staged first-writer provenance instead of the selected source package."
    Assert-TransactionTestEqual `
        "build-occurrence-two" `
        ([string]$script:TransactionTestReceiptDetails[-1].Selection.BuildId) `
        "Deployment receipt lost the selected source package BuildId."
    $stagedProvenance = Get-Content `
        -LiteralPath (Join-Path $occurrenceSecond.Release.ReleasePath "codex-dev-build.json") `
        -Raw | ConvertFrom-Json
    Assert-TransactionTestEqual `
        "build-occurrence-one" `
        ([string]$stagedProvenance.BuildId) `
        "Regression fixture no longer exercises a first-writer staged sidecar."
    $occurrenceVerify = Invoke-TransactionVerifySkippingSmoke `
        -ConfigPath $occurrence.Config `
        -DeploymentRoot $occurrence.DeploymentRoot
    Assert-TransactionTestEqual `
        "consistent" `
        $occurrenceVerify.DeploymentStatus `
        "Verify confused staged first-writer provenance with the selected occurrence."
    Assert-TransactionTestEqual `
        "build-occurrence-two" `
        ([string]$occurrenceVerify.SelectedOccurrence.BuildId) `
        "Verify did not report the selected source occurrence."
    Assert-TransactionTestEqual `
        "build-occurrence-one" `
        ([string]$occurrenceVerify.StagedReleaseProvenance.Provenance.BuildId) `
        "Verify did not report staged first-writer provenance separately."

    Assert-TransactionTestEqual `
        $script:TransactionTestRealUserSelectorBefore `
        ([System.Environment]::GetEnvironmentVariable(
            "CODEX_CLI_PATH",
            [System.EnvironmentVariableTarget]::User
        )) `
        "Transaction tests mutated the real User-scope CODEX_CLI_PATH."
    Assert-TransactionTestTrue `
        ($script:TransactionTestPersistentWrites -gt 0) `
        "Transaction tests did not exercise the persistent-selector write seam."
    Write-Host "windows_desktop_dev deployment transaction tests: PASS"
    exit 0
} finally {
    $script:CodexDevDeploymentFaultInjector = $null
    $script:TransactionTestReceiptFailure = $null
    $script:TransactionTestPackageInfoHook = $null
    Clear-CodexDevPersistentSelectorTestAdapter
    $resolvedTestRoot = [System.IO.Path]::GetFullPath($testRoot)
    if ($resolvedTestRoot.StartsWith($tempBase, [System.StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item `
            -LiteralPath $resolvedTestRoot `
            -Recurse `
            -Force `
            -ErrorAction SilentlyContinue
    }
}
