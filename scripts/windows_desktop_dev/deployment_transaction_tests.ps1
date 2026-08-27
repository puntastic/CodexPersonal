Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

. (Join-Path $PSScriptRoot "common.ps1")
. (Join-Path $PSScriptRoot "environment.ps1")
. (Join-Path $PSScriptRoot "package.ps1")
. (Join-Path $PSScriptRoot "deployment_config.ps1")
. (Join-Path $PSScriptRoot "deployment_state.ps1")
. (Join-Path $PSScriptRoot "deployment_transaction.ps1")
. (Join-Path $PSScriptRoot "deployment.ps1")

$script:TransactionTestReceiptDetails = @()
$script:TransactionTestReceiptFailure = $null
$script:TransactionTestPackageInfoCalls = @()
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
    return & $script:TransactionTestRealPackageInfo `
        -PackageDirectory $PackageDirectory `
        -SkipSmoke
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
    $script:TransactionTestObservedDeployLock = $false
    $script:CodexDevDeploymentFaultInjector = {
        param($Stage, $Transaction)
        if ($Stage -eq "AfterConfig") {
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
    Assert-TransactionTestTrue `
        (-not (Test-Path -LiteralPath $caughtPaths.State)) `
        "Caught deployment failure left new state behind."
    Assert-TransactionTestTrue `
        (-not (Test-Path -LiteralPath $caughtPaths.Pending)) `
        "Caught deployment failure left a pending transaction behind."

    $interruptionCases = @(
        foreach ($action in @("Deploy", "Rollback")) {
            foreach ($stage in @("AfterPending", "AfterConfig", "AfterState")) {
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

        $expectedPendingStatus = if ($case.Stage -eq "AfterPending") {
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
                ($case.Stage -ne "AfterPending") `
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
        $expectedRecoveryStatus = if ($case.Stage -eq "AfterPending") {
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
    $null = Assert-TransactionTestThrows `
        -Pattern "not settled: drift" `
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

    Write-Host "windows_desktop_dev deployment transaction tests: PASS"
    exit 0
} finally {
    $script:CodexDevDeploymentFaultInjector = $null
    $script:TransactionTestReceiptFailure = $null
    $resolvedTestRoot = [System.IO.Path]::GetFullPath($testRoot)
    if ($resolvedTestRoot.StartsWith($tempBase, [System.StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item `
            -LiteralPath $resolvedTestRoot `
            -Recurse `
            -Force `
            -ErrorAction SilentlyContinue
    }
}
