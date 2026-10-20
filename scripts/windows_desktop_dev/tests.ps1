[CmdletBinding()]
param([switch]$BuildPointerOnly)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

. (Join-Path $PSScriptRoot "workflow.ps1")

function Assert-CodexDevEqual {
    param(
        [object]$Expected,
        [object]$Actual,
        [string]$Message
    )
    if ($Expected -ne $Actual) {
        throw "$Message`nExpected: $Expected`nActual:   $Actual"
    }
}

function Assert-CodexDevThrows {
    param(
        [scriptblock]$Action,
        [string]$Message
    )

    try {
        & $Action
    } catch {
        return
    }
    throw $Message
}

# This section is entirely mocked: no build, package execution, filesystem
# write, current-task identity change, or live selector/configuration read.
$buildMockNames = @(
    "Initialize-CodexDevEnvironment", "Get-CodexDevGitState", "Get-CodexDevTaskToken",
    "Get-CodexDevLastPackagePointerPath", "Resolve-CodexDevRipgrep", "Invoke-CodexDevNative",
    "Get-CodexDevPackageInfo", "Write-CodexDevJson", "Write-CodexDevReceipt",
    "Complete-CodexDevBuildPointer", "Set-CodexDevLastPackagePointer",
    "Get-CodexDevDoctorReport", "Invoke-CodexDevToolSetup", "Resolve-CodexDevPackageDirectory",
    "Install-CodexDevPackage", "Invoke-CodexDevVerify", "Invoke-CodexDevRollback"
)
$savedBuildMocks = @{}
foreach ($name in $buildMockNames) { $savedBuildMocks[$name] = (Get-Command $name).ScriptBlock }
$mockRoot = Join-Path ([IO.Path]::GetTempPath()) ("codex-build-pointer-mock-" + [guid]::NewGuid().ToString("N"))
try {
    function Initialize-CodexDevEnvironment {
        param($CargoPath, $CargoHome, $RustupHome, $PythonPath)
        return [pscustomobject]@{ Python = "mock-python"; Cargo = "mock-cargo"; HostTarget = "x86_64-pc-windows-msvc" }
    }
    function Get-CodexDevGitState {
        $script:BuildPointerCase.SourceReads++
        $changed = -not $script:BuildPointerCase.Stable -and $script:BuildPointerCase.SourceReads -gt 1
        return [pscustomobject]@{
            Head = if ($changed) { "changed" } else { "before" }; ShortHead = "before"; Branch = "mock"
            GitVisibleSourceFingerprint = if ($changed) { "changed" } else { "same" }; Dirty = $false
        }
    }
    function Get-CodexDevTaskToken { return "mock-task-identity" }
    function Get-CodexDevLastPackagePointerPath {
        param($TaskToken)
        $script:BuildPointerCase.PointerPathCalls++
        if ($script:BuildPointerCase.Skip) { throw "Skipped build requested a pointer path." }
        return Join-Path $mockRoot "last-package.json"
    }
    function Test-Path {
        param([string]$LiteralPath)
        Assert-CodexDevEqual (Join-Path $mockRoot "package") $LiteralPath "Unexpected filesystem existence check."
        return $false
    }
    function Resolve-CodexDevRipgrep { param($ExplicitPath) return "mock-rg" }
    function Invoke-CodexDevNative {
        param($FilePath, $ArgumentList)
        $script:BuildPointerCase.NativeCalls++
        if ($script:BuildPointerCase.Fault -eq "build") { throw "synthetic build failure" }
        Assert-CodexDevEqual $true ($ArgumentList -contains "--cargo-locked") "Build lost its locked flag."
        Assert-CodexDevEqual $true ($ArgumentList -contains "--cargo-native-host") "Build lost native-host mode."
    }
    function Get-CodexDevPackageInfo {
        param($PackageDirectory, [switch]$SkipSmoke)
        $script:BuildPointerCase.PackageChecks++
        Assert-CodexDevEqual $false ([bool]$SkipSmoke) "Pointer opt-out skipped package smoke validation."
        if ($script:BuildPointerCase.Fault -eq "validation") { throw "synthetic validation failure" }
        return [pscustomobject]@{
            PackageRoot = $PackageDirectory; Fingerprint = "mock-artifact-fingerprint"
            ProvenanceStatus = if ($null -eq $script:BuildPointerCase.Provenance) { "unavailable" } else { $script:BuildPointerCase.Provenance.Status }
            Provenance = $script:BuildPointerCase.Provenance
        }
    }
    function Write-CodexDevJson {
        param($Path, $Value)
        Assert-CodexDevEqual (Join-Path $mockRoot "package/codex-dev-build.json") $Path "Unexpected JSON write target."
        if ($script:BuildPointerCase.Fault -eq "provenance") { throw "synthetic provenance failure" }
        $script:BuildPointerCase.Provenance = $Value
    }
    function Write-CodexDevReceipt {
        param($Action, $Details, $Source)
        Assert-CodexDevEqual "Build" $Action "Unexpected receipt action."
        $script:BuildPointerCase.ReceiptCalls++
        $script:BuildPointerCase.ReceiptDetails = $Details
        if ($script:BuildPointerCase.Fault -eq "receipt") { throw "synthetic receipt failure" }
        return Join-Path $mockRoot "build-receipt.json"
    }
    function Complete-CodexDevBuildPointer {
        param($SourceStable, $PointerPath, $Pointer)
        $script:BuildPointerCase.PointerCompletionCalls++
        if ($script:BuildPointerCase.Skip) { throw "Skipped build called pointer completion." }
        $original = $savedBuildMocks["Complete-CodexDevBuildPointer"]
        return & $original -SourceStable $SourceStable -PointerPath $PointerPath -Pointer $Pointer
    }
    function Set-CodexDevLastPackagePointer {
        param($PointerPath, $Pointer)
        $script:BuildPointerCase.PointerWriterCalls++
        if ($script:BuildPointerCase.PointerMode -eq "failure") { throw "synthetic pointer failure" }
        return $script:BuildPointerCase.PointerMode -eq "advance"
    }
    function Get-CodexDevDoctorReport { throw "Unexpected Doctor action." }
    function Invoke-CodexDevToolSetup { throw "Unexpected Setup action." }
    function Resolve-CodexDevPackageDirectory { throw "Unexpected package resolution." }
    function Install-CodexDevPackage { throw "Unexpected Deploy action." }
    function Invoke-CodexDevVerify { throw "Unexpected Verify action." }
    function Invoke-CodexDevRollback { throw "Unexpected Rollback action." }

    $cases = @(
        @{ Name = "default"; Skip = $false; Stable = $true; Fault = ""; PointerMode = "advance"; Status = "built"; Writes = 1 },
        @{ Name = "unselected"; Skip = $true; Stable = $true; Fault = ""; PointerMode = "advance"; Status = "built_unselected"; Writes = 0 },
        @{ Name = "default-unstable"; Skip = $false; Stable = $false; Fault = ""; PointerMode = "advance"; Status = "source_changed_during_build"; Writes = 0 },
        @{ Name = "unselected-unstable"; Skip = $true; Stable = $false; Fault = ""; PointerMode = "advance"; Status = "source_changed_during_build"; Writes = 0 },
        @{ Name = "default-receipt-failure"; Skip = $false; Stable = $true; Fault = "receipt"; PointerMode = "advance"; Status = "built_receipt_failed"; Writes = 1 },
        @{ Name = "unselected-receipt-failure"; Skip = $true; Stable = $true; Fault = "receipt"; PointerMode = "advance"; Status = "built_receipt_failed"; Writes = 0 },
        @{ Name = "default-pointer-failure"; Skip = $false; Stable = $true; Fault = ""; PointerMode = "failure"; Status = "built_pointer_failed"; Writes = 1 },
        @{ Name = "default-superseded"; Skip = $false; Stable = $true; Fault = ""; PointerMode = "superseded"; Status = "built_pointer_superseded"; Writes = 1 },
        @{ Name = "unselected-build-failure"; Skip = $true; Stable = $true; Fault = "build"; PointerMode = "advance"; Status = "throws"; Writes = 0 },
        @{ Name = "unselected-validation-failure"; Skip = $true; Stable = $true; Fault = "validation"; PointerMode = "advance"; Status = "throws"; Writes = 0 },
        @{ Name = "unselected-provenance-failure"; Skip = $true; Stable = $true; Fault = "provenance"; PointerMode = "advance"; Status = "throws"; Writes = 0 }
    )
    $mockParameters = @{
        CargoProfile = "dev-small"; PackageDirectory = Join-Path $mockRoot "package"
        ConfigPath = Join-Path $mockRoot "unused-config.toml"; DeploymentRoot = Join-Path $mockRoot "unused-deployments"; Json = $true
    }
    foreach ($case in $cases) {
        $script:BuildPointerCase = $case.Clone()
        foreach ($counter in @("SourceReads", "NativeCalls", "PackageChecks", "ReceiptCalls", "PointerPathCalls", "PointerCompletionCalls", "PointerWriterCalls")) { $script:BuildPointerCase[$counter] = 0 }
        $script:BuildPointerCase.Provenance = $null
        $script:BuildPointerCase.ReceiptDetails = $null
        $result = $null
        $problem = $null
        try { $result = Invoke-CodexDesktopDev -Action Build @mockParameters -SkipLatestPackagePointer:$case.Skip | ConvertFrom-Json } catch { $problem = $_.Exception.Message }
        if ($case.Status -eq "throws") {
            Assert-CodexDevEqual "synthetic $($case.Fault) failure" $problem "Build failure was hidden."
            Assert-CodexDevEqual 0 $script:BuildPointerCase.ReceiptCalls "Failed build/validation/provenance wrote a receipt."
        } else {
            Assert-CodexDevEqual $null $problem "Mocked build failed unexpectedly."
            Assert-CodexDevEqual $case.Status $result.Status "Build disposition changed or masked failure."
            Assert-CodexDevEqual 2 $script:BuildPointerCase.PackageChecks "Build omitted package validation."
            Assert-CodexDevEqual 1 $script:BuildPointerCase.ReceiptCalls "Build omitted its receipt."
            Assert-CodexDevEqual $case.Stable $script:BuildPointerCase.ReceiptDetails.SourceStable "Receipt source stability changed."
            Assert-CodexDevEqual $case.Skip $script:BuildPointerCase.ReceiptDetails.SkipLatestPackagePointer "Receipt lost the pointer policy."
            Assert-CodexDevEqual $case.Skip $result.LatestPackagePointerSkipped "Router did not forward the opt-out."
            $expectedProvenance = if ($case.Stable) { "stable" } else { "source_changed_during_build" }
            Assert-CodexDevEqual $expectedProvenance $script:BuildPointerCase.Provenance.Status "Provenance was weakened."
            if ($case.Skip) {
                Assert-CodexDevEqual $false $result.PointerAdvanced "Unselected result claimed pointer advancement."
                Assert-CodexDevEqual $null $result.PointerError "Deliberate non-selection became a pointer error."
                Assert-CodexDevEqual $null $result.LastPackagePointer "Unselected result returned a pointer path."
                Assert-CodexDevEqual $null $result.Next "Unselected result suggested automatic deployment."
            } else {
                Assert-CodexDevEqual ($case.Stable -and $case.PointerMode -eq "advance") $result.PointerAdvanced "Default pointer advancement changed."
                Assert-CodexDevEqual (Join-Path $mockRoot "last-package.json") $result.LastPackagePointer "Default pointer path changed."
                $expectedPointerError = if ($case.PointerMode -eq "failure") { "synthetic pointer failure" } else { $null }
                Assert-CodexDevEqual $expectedPointerError $result.PointerError "Default pointer failure detail changed."
                $expectedNext = if ($case.Stable) {
                    ".\codex-dev.ps1 -Action Deploy -PackageDirectory '$(Join-Path $mockRoot 'package')'"
                } else { ".\codex-dev.ps1 -Action Build -CargoProfile dev-small" }
                Assert-CodexDevEqual $expectedNext $result.Next "Default next-action hint changed."
            }
            if ($case.Fault -eq "receipt") { Assert-CodexDevEqual "synthetic receipt failure" $result.ReceiptError "Receipt failure detail was lost." }
        }
        Assert-CodexDevEqual $case.Writes $script:BuildPointerCase.PointerWriterCalls "Pointer writer call count changed."
        if ($case.Skip) {
            Assert-CodexDevEqual 0 $script:BuildPointerCase.PointerPathCalls "Skipped build resolved the task pointer path."
            Assert-CodexDevEqual 0 $script:BuildPointerCase.PointerCompletionCalls "Skipped build called pointer completion."
        }
        Write-Output "Build pointer mock passed: $($case.Name)"
    }
    foreach ($action in @("Help", "Doctor", "Setup", "Just", "Deploy", "Verify", "Rollback", "SelfTest")) {
        $problem = $null
        try { $null = Invoke-CodexDesktopDev -Action $action @mockParameters -SkipLatestPackagePointer } catch { $problem = $_.Exception.Message }
        Assert-CodexDevEqual "-SkipLatestPackagePointer is valid only for Build; no action was started." $problem "Opt-out was accepted outside Build."
    }
    $nativeCalls = $script:BuildPointerCase.NativeCalls
    $problem = $null
    try { $null = Invoke-CodexDesktopDev -Action Build @mockParameters -SkipLatestPackagePointer -WhatIf } catch { $problem = $_.Exception.Message }
    Assert-CodexDevEqual "Build does not support -WhatIf; no build or package changes were started. Omit -WhatIf only when you intend to build." $problem "Build preview refusal changed."
    Assert-CodexDevEqual $nativeCalls $script:BuildPointerCase.NativeCalls "Build preview started a build."
    Write-Output "Build pointer router checks passed: eight non-Build actions and Build -WhatIf."
} finally {
    foreach ($name in $buildMockNames) { Set-Item -LiteralPath "Function:\$name" -Value $savedBuildMocks[$name] }
    Remove-Item -LiteralPath Function:\Test-Path
    $script:BuildPointerCase = $null
}
if ($BuildPointerOnly) {
    Write-Output "windows_desktop_dev Build pointer mocks: PASS"
    exit 0
}

$script:SelfTestRealUserSelectorBefore = [System.Environment]::GetEnvironmentVariable(
    "CODEX_CLI_PATH",
    [System.EnvironmentVariableTarget]::User
)
$script:SelfTestPersistentSelector = $null
Set-CodexDevPersistentSelectorTestAdapter `
    -Reader {
        param($Name)
        return $script:SelfTestPersistentSelector
    } `
    -Writer {
        param($Name, $Value)
        $script:SelfTestPersistentSelector = $Value
    }

function New-FakeCodexPackage {
    param(
        [string]$Path,
        [string]$Marker
    )

    New-Item -ItemType Directory -Path $Path | Out-Null
    foreach ($relativePath in $script:CodexDevExpectedPackageFiles) {
        $file = Join-Path $Path $relativePath
        New-Item -ItemType Directory -Path (Split-Path -Parent $file) -Force | Out-Null
        if ($relativePath -eq "codex-package.json") {
            [System.IO.File]::WriteAllText(
                $file,
                (@{
                    layoutVersion = 1
                    version = "0.0.0"
                    target = Get-CodexDevHostTarget
                    variant = "codex"
                    entrypoint = "bin/codex.exe"
                    resourcesDir = "codex-resources"
                    pathDir = "codex-path"
                } | ConvertTo-Json),
                [System.Text.UTF8Encoding]::new($false)
            )
        } else {
            Write-FakeCodexPe `
                -Path $file `
                -Target (Get-CodexDevHostTarget) `
                -Marker "$Marker/$relativePath"
        }
    }
}

function Write-FakeCodexPe {
    param(
        [string]$Path,
        [ValidateSet("x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc")]
        [string]$Target,
        [string]$Marker = ""
    )

    $markerBytes = [System.Text.Encoding]::UTF8.GetBytes($Marker)
    $bytes = [byte[]]::new([Math]::Max(128, 0x50 + $markerBytes.Length))
    $bytes[0] = 0x4D
    $bytes[1] = 0x5A
    [BitConverter]::GetBytes([uint32]0x40).CopyTo($bytes, 0x3C)
    [BitConverter]::GetBytes([uint32]0x00004550).CopyTo($bytes, 0x40)
    $machine = if ($Target -eq "x86_64-pc-windows-msvc") { [uint16]0x8664 } else { [uint16]0xAA64 }
    [BitConverter]::GetBytes($machine).CopyTo($bytes, 0x44)
    $markerBytes.CopyTo($bytes, 0x50)
    New-Item -ItemType Directory -Path (Split-Path -Parent $Path) -Force | Out-Null
    [System.IO.File]::WriteAllBytes($Path, $bytes)
}

$tempBase = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath())
$testRoot = Join-Path $tempBase ("codex-desktop-dev-tests-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $testRoot | Out-Null

try {
    $realBuild = (Get-Command Invoke-CodexDevBuild).ScriptBlock
    $script:SelfTestBuildCalls = 0
    try {
        function Invoke-CodexDevBuild {
            param(
                [string]$CargoProfile,
                [string]$PackageDirectory,
                [string]$CargoPath,
                [string]$CargoHome,
                [string]$RustupHome,
                [string]$PythonPath,
                [string]$RipgrepPath,
                [string]$RemoteControlAppServerVersion,
                [switch]$SkipLatestPackagePointer
            )

            $script:SelfTestBuildCalls++
            return [pscustomobject]@{
                Status = "mock_build"
                CargoProfile = $CargoProfile
                PackageDirectory = $PackageDirectory
            }
        }

        $buildPreviewError = $null
        try {
            Invoke-CodexDesktopDev -Action Build -WhatIf -Json
        } catch {
            $buildPreviewError = $_.Exception.Message
        }
        Assert-CodexDevEqual `
            "Build does not support -WhatIf; no build or package changes were started. Omit -WhatIf only when you intend to build." `
            $buildPreviewError `
            "Build -WhatIf did not explicitly refuse the unsupported preview."
        Assert-CodexDevEqual 0 $script:SelfTestBuildCalls "Build -WhatIf invoked the build implementation."

        $buildParameters = @{
            Action = "Build"
            CargoProfile = "dev-small"
            PackageDirectory = Join-Path $testRoot "unused-build-output"
            Json = $true
        }
        $buildResult = Invoke-CodexDesktopDev @buildParameters | ConvertFrom-Json
        Assert-CodexDevEqual 1 $script:SelfTestBuildCalls "Build without -WhatIf did not invoke the build implementation once."
        Assert-CodexDevEqual "mock_build" $buildResult.Status "Build did not return the build implementation's result."
        Assert-CodexDevEqual $buildParameters.CargoProfile $buildResult.CargoProfile "Build did not forward the selected Cargo profile."
        Assert-CodexDevEqual $buildParameters.PackageDirectory $buildResult.PackageDirectory "Build did not forward the selected package directory."

        $null = Invoke-CodexDesktopDev @buildParameters -WhatIf:$false
        Assert-CodexDevEqual 2 $script:SelfTestBuildCalls "Build with -WhatIf:`$false did not invoke the build implementation once."
    } finally {
        Set-Item -LiteralPath Function:\Invoke-CodexDevBuild -Value $realBuild
    }

    $capturedLines = @(Invoke-CodexDevNative -FilePath $env:ComSpec -ArgumentList @(
        "/d", "/s", "/c", "echo capture-one&echo capture-two"
    ) -WorkingDirectory $testRoot -Capture)
    Assert-CodexDevEqual 2 $capturedLines.Count "Native capture did not preserve line boundaries."
    Assert-CodexDevEqual "capture-one" $capturedLines[0] "Native capture changed stdout."
    Assert-CodexDevEqual "capture-two" $capturedLines[1] "Native capture changed stdout."
    $childPath = @(Invoke-CodexDevNative -FilePath $env:ComSpec -ArgumentList @(
        "/d", "/s", "/c", "echo %PATH%"
    ) -WorkingDirectory $testRoot -Capture)
    Assert-CodexDevEqual $env:PATH $childPath[0] "Native launch did not normalize duplicate Windows PATH keys."
    Assert-CodexDevThrows {
        Resolve-CodexDevFile -ExplicitPath (Join-Path $testRoot "missing-explicit.exe") -Candidates @($env:ComSpec) -Description "test executable"
    } "A missing explicit tool path silently fell back to another executable."

    $oldThreadId = $env:CODEX_THREAD_ID
    try {
        $env:CODEX_THREAD_ID = "self-test-one"
        $threadOnePointer = Get-CodexDevLastPackagePointerPath
        $env:CODEX_THREAD_ID = "self-test-two"
        $threadTwoPointer = Get-CodexDevLastPackagePointerPath
        Remove-Item Env:CODEX_THREAD_ID -ErrorAction SilentlyContinue
        $manualPointer = Get-CodexDevLastPackagePointerPath
    } finally {
        if ($null -eq $oldThreadId) {
            Remove-Item Env:CODEX_THREAD_ID -ErrorAction SilentlyContinue
        } else {
            $env:CODEX_THREAD_ID = $oldThreadId
        }
    }
    if ($threadOnePointer -eq $threadTwoPointer -or $threadOnePointer -eq $manualPointer) {
        throw "Task-scoped package pointers collided."
    }
    $brokenPointerPath = Join-Path $testRoot "broken-pointer\last-package.json"
    New-Item -ItemType Directory -Path (Split-Path -Parent $brokenPointerPath) -Force | Out-Null
    [System.IO.File]::WriteAllText(
        $brokenPointerPath,
        "not-json",
        [System.Text.UTF8Encoding]::new($false)
    )
    $pointerFailure = Complete-CodexDevBuildPointer `
        -SourceStable $true `
        -PointerPath $brokenPointerPath `
        -Pointer ([ordered]@{ StartedAtUtc = [DateTime]::UtcNow.ToString("o") })
    Assert-CodexDevEqual $false $pointerFailure.Advanced "A broken pointer was reported as advanced."
    if ([string]::IsNullOrWhiteSpace([string]$pointerFailure.Error)) {
        throw "A broken pointer did not return a non-transactional PointerError."
    }

    $pePackage = Join-Path $testRoot "pe-package"
    $hostTarget = Get-CodexDevHostTarget
    foreach ($relativePath in $script:CodexDevExpectedPackageFiles) {
        if ($relativePath.EndsWith(".exe", [System.StringComparison]::OrdinalIgnoreCase)) {
            Write-FakeCodexPe -Path (Join-Path $pePackage $relativePath) -Target $hostTarget
        }
    }
    Assert-CodexDevPackageExecutableTargets -PackageRoot $pePackage -ExpectedTarget $hostTarget
    $otherTarget = if ($hostTarget -eq "x86_64-pc-windows-msvc") {
        "aarch64-pc-windows-msvc"
    } else {
        "x86_64-pc-windows-msvc"
    }
    Write-FakeCodexPe -Path (Join-Path $pePackage "codex-resources\codex-command-runner.exe") -Target $otherTarget
    Assert-CodexDevThrows {
        Assert-CodexDevPackageExecutableTargets -PackageRoot $pePackage -ExpectedTarget $hostTarget
    } "A non-entrypoint helper with the wrong PE architecture passed package validation."

    $alternateUv = Join-Path $testRoot "tool-layout\python\uv\uv.exe"
    New-Item -ItemType Directory -Path (Split-Path -Parent $alternateUv) -Force | Out-Null
    [System.IO.File]::WriteAllText($alternateUv, "uv", [System.Text.UTF8Encoding]::new($false))
    Assert-CodexDevEqual $alternateUv (Resolve-CodexDevInstalledUvForExposure (Join-Path $testRoot "tool-layout")) "Setup did not recognize the alternate uv wheel layout."

    $originalRepositoryRoot = $script:CodexDevRepositoryRoot
    try {
        $script:CodexDevRepositoryRoot = Join-Path $testRoot "broken-tool-repository"
        $brokenToolRoot = Get-CodexDevLocalToolRoot
        New-Item -ItemType Directory -Path (Join-Path $brokenToolRoot "bin") -Force | Out-Null
        foreach ($toolName in @("uv.exe", "dotslash.exe")) {
            [System.IO.File]::WriteAllBytes(
                (Join-Path $brokenToolRoot "bin\$toolName"),
                [byte[]](0)
            )
        }
        $brokenToolStatus = Get-CodexDevToolStatus
        Assert-CodexDevEqual $false $brokenToolStatus.FormatReady "Corrupt local formatter helpers reported ready."
        if ([string]::IsNullOrWhiteSpace([string]$brokenToolStatus.UvError) -or
            [string]::IsNullOrWhiteSpace([string]$brokenToolStatus.DotslashError)) {
            throw "Corrupt local formatter helpers did not report exact probe errors."
        }
    } finally {
        $script:CodexDevRepositoryRoot = $originalRepositoryRoot
    }

    $config = Join-Path $testRoot "config.toml"
    $oldEntrypoint = Join-Path $testRoot "old\bin\codex.exe"
    New-Item -ItemType Directory -Path (Split-Path -Parent $oldEntrypoint) -Force | Out-Null
    [System.IO.File]::WriteAllText($oldEntrypoint, "old", [System.Text.UTF8Encoding]::new($false))
    [System.IO.File]::WriteAllText(
        $config,
        "[mcp_servers.node_repl.env]`r`nKEEP_ME = 'yes'`r`nCODEX_CLI_PATH = '$oldEntrypoint'`r`n",
        [System.Text.UTF8Encoding]::new($false)
    )
    $script:SelfTestPersistentSelector = $oldEntrypoint

    $rewriteTarget = Join-Path $testRoot "rewrite\bin\codex.exe"
    Set-CodexCliPathInConfig -ConfigPath $config -Entrypoint $rewriteTarget
    Assert-CodexDevEqual $rewriteTarget (Get-CodexCliPathFromConfig $config) "Config rewrite did not select the requested entrypoint."
    if (-not (Select-String -LiteralPath $config -SimpleMatch "KEEP_ME = 'yes'" -Quiet)) {
        throw "Config rewrite damaged an unrelated setting."
    }
    Set-CodexCliPathInConfig -ConfigPath $config -Entrypoint $oldEntrypoint

    $packageOne = Join-Path $testRoot "package-one"
    $packageTwo = Join-Path $testRoot "package-two"
    New-FakeCodexPackage -Path $packageOne -Marker "one"
    New-FakeCodexPackage -Path $packageTwo -Marker "two"
    $infoOne = Get-CodexDevPackageInfo -PackageDirectory $packageOne -SkipSmoke
    $infoTwo = Get-CodexDevPackageInfo -PackageDirectory $packageTwo -SkipSmoke
    if ($infoOne.Fingerprint -eq $infoTwo.Fingerprint) {
        throw "Distinct packages received the same fingerprint."
    }

    $literalPackage = Join-Path $testRoot "package-[literal]"
    New-FakeCodexPackage -Path $literalPackage -Marker "literal"
    $literalInfo = Get-CodexDevPackageInfo -PackageDirectory $literalPackage -SkipSmoke
    Assert-CodexDevEqual $literalPackage $literalInfo.PackageRoot "A legal bracketed package path was treated as a wildcard."

    $provenance = [ordered]@{
        SchemaVersion = 1
        Status = "stable"
        BuildId = "self-test-build"
        TaskToken = "pointer-owner"
        ArtifactFingerprint = $infoOne.Fingerprint
    }
    Write-CodexDevJson -Path (Join-Path $packageOne "codex-dev-build.json") -Value $provenance
    $provenancedInfo = Get-CodexDevPackageInfo -PackageDirectory $packageOne -SkipSmoke
    Assert-CodexDevEqual "stable" $provenancedInfo.ProvenanceStatus "Package provenance was not loaded."

    $packageThree = Join-Path $testRoot "package-three"
    New-FakeCodexPackage -Path $packageThree -Marker "three"
    $infoThree = Get-CodexDevPackageInfo -PackageDirectory $packageThree -SkipSmoke
    Write-CodexDevJson -Path (Join-Path $packageThree "codex-dev-build.json") -Value ([ordered]@{
        SchemaVersion = 1
        Status = "stable"
        ArtifactFingerprint = $infoThree.Fingerprint
    })
    [System.IO.File]::AppendAllText(
        (Join-Path $packageThree "bin\codex-code-mode-host.exe"),
        "tampered",
        [System.Text.UTF8Encoding]::new($false)
    )
    Assert-CodexDevThrows {
        Get-CodexDevPackageInfo -PackageDirectory $packageThree -SkipSmoke
    } "A package mutation did not invalidate its provenance sidecar."

    $packageFour = Join-Path $testRoot "package-four"
    New-FakeCodexPackage -Path $packageFour -Marker "four"
    $infoFourBefore = Get-CodexDevPackageInfo -PackageDirectory $packageFour -SkipSmoke
    [System.IO.File]::WriteAllText(
        (Join-Path $packageFour "unexpected-runtime-file.dat"),
        "artifact identity must include me",
        [System.Text.UTF8Encoding]::new($false)
    )
    $infoFourAfter = Get-CodexDevPackageInfo -PackageDirectory $packageFour -SkipSmoke
    if ($infoFourBefore.Fingerprint -eq $infoFourAfter.Fingerprint) {
        throw "An unexpected packaged file did not change artifact identity."
    }

    $originalRepositoryRoot = $script:CodexDevRepositoryRoot
    $oldThreadId = $env:CODEX_THREAD_ID
    try {
        $script:CodexDevRepositoryRoot = $testRoot
        $env:CODEX_THREAD_ID = "pointer-owner"
        $pointerPath = Get-CodexDevLastPackagePointerPath
        Write-CodexDevJson -Path $pointerPath -Value ([ordered]@{
            SchemaVersion = 1
            Status = "ready"
            BuildId = "self-test-build"
            TaskToken = "pointer-owner"
            PackageRoot = $packageOne
            ArtifactFingerprint = $provenancedInfo.Fingerprint
            ProvenanceStatus = "stable"
        })
        Assert-CodexDevEqual $packageOne (Resolve-CodexDevPackageDirectory) "The owning task could not resolve its package pointer."
        $env:CODEX_THREAD_ID = "different-task"
        Assert-CodexDevThrows {
            Resolve-CodexDevPackageDirectory
        } "A different task fell back to another task's package pointer."
    } finally {
        $script:CodexDevRepositoryRoot = $originalRepositoryRoot
        if ($null -eq $oldThreadId) {
            Remove-Item Env:CODEX_THREAD_ID -ErrorAction SilentlyContinue
        } else {
            $env:CODEX_THREAD_ID = $oldThreadId
        }
    }

    $script:CodexDevSelfTestRealPackageInfo = (Get-Command Get-CodexDevPackageInfo).ScriptBlock
    function Get-CodexDevPackageInfo {
        param(
            [string]$PackageDirectory,
            [switch]$SkipSmoke
        )

        return & $script:CodexDevSelfTestRealPackageInfo `
            -PackageDirectory $PackageDirectory `
            -SkipSmoke
    }
    function Write-CodexDevReceipt {
        param(
            [string]$Action,
            [object]$Details,
            [object]$Source
        )

        return Join-Path $testRoot "receipt-$Action-$([guid]::NewGuid().ToString('N')).json"
    }

    $deployRoot = Join-Path $testRoot "deployments"
    $planned = Install-CodexDevPackage -PackageDirectory $packageOne -ConfigPath $config -DeploymentRoot $deployRoot -SkipSmoke -WhatIf
    Assert-CodexDevEqual "planned" $planned.Status "WhatIf did not return a plan."
    Assert-CodexDevEqual $oldEntrypoint (Get-CodexCliPathFromConfig $config) "WhatIf changed config."

    $first = Install-CodexDevPackage -PackageDirectory $packageOne -ConfigPath $config -DeploymentRoot $deployRoot -SkipSmoke
    Assert-CodexDevEqual "selected_for_restart" $first.Status "First deploy did not settle."
    Assert-CodexDevEqual $first.Release.Entrypoint (Get-CodexCliPathFromConfig $config) "First deploy did not update config."
    Assert-CodexDevEqual $first.Release.Entrypoint (Get-CodexDevPersistentSelector) "First deploy did not update the persistent selector."
    $firstAgain = Install-CodexDevPackage -PackageDirectory $packageOne -ConfigPath $config -DeploymentRoot $deployRoot -SkipSmoke
    Assert-CodexDevEqual "already_selected" $firstAgain.Status "Repeated deploy was not idempotent."
    Assert-CodexDevEqual $oldEntrypoint $firstAgain.Previous.Entrypoint "Repeated deploy discarded the rollback entrypoint."

    $second = Install-CodexDevPackage -PackageDirectory $packageTwo -ConfigPath $config -DeploymentRoot $deployRoot -SkipSmoke
    Assert-CodexDevEqual $second.Release.Entrypoint (Get-CodexCliPathFromConfig $config) "Second deploy did not update config."
    Assert-CodexDevEqual $second.Release.Entrypoint (Get-CodexDevPersistentSelector) "Second deploy did not update the persistent selector."

    $rolledBack = Invoke-CodexDevRollback -ConfigPath $config -DeploymentRoot $deployRoot
    Assert-CodexDevEqual "previous_selected_for_restart" $rolledBack.Status "Rollback did not settle."
    Assert-CodexDevEqual $first.Release.Entrypoint (Get-CodexCliPathFromConfig $config) "Rollback did not restore the first release."
    Assert-CodexDevEqual $first.Release.Entrypoint (Get-CodexDevPersistentSelector) "Rollback did not restore the persistent selector."

    $script:SelfTestIndependentDoctorConfigReads = 0
    function Get-CodexCliPathFromConfig {
        param([string]$ConfigPath)

        $script:SelfTestIndependentDoctorConfigReads++
        throw "Doctor must use the deployment snapshot instead of rereading config."
    }
    function Initialize-CodexDevEnvironment {
        param(
            [string]$CargoPath,
            [string]$CargoHome,
            [string]$RustupHome,
            [string]$PythonPath
        )

        return [pscustomobject]@{}
    }
    function Resolve-CodexDevRipgrep {
        param([string]$ExplicitPath)

        return "test-rg.exe"
    }
    function Get-CodexDevToolStatus {
        return [pscustomobject]@{
            FormatReady = $true
            Just = "test-just.exe"
            JustError = $null
            BazelReady = $true
        }
    }
    function Get-CodexDevGitState {
        return [pscustomobject]@{ Status = "test" }
    }
    function Get-CodexDevStorageInventory {
        param([string]$DeploymentRoot)

        return [pscustomobject]@{ Root = $DeploymentRoot }
    }
    $doctor = Get-CodexDevDoctorReport `
        -CargoPath "test-cargo.exe" `
        -CargoHome (Join-Path $testRoot "cargo-home") `
        -RustupHome (Join-Path $testRoot "rustup-home") `
        -PythonPath "test-python.exe" `
        -RipgrepPath "test-rg.exe" `
        -ConfigPath $config `
        -DeploymentRoot $deployRoot
    Assert-CodexDevEqual `
        0 `
        $script:SelfTestIndependentDoctorConfigReads `
        "Doctor mixed an independent config read into its deployment snapshot."
    Assert-CodexDevEqual `
        $doctor.Deployment.ConfiguredEntrypoint `
        $doctor.ConfiguredEntrypoint `
        "Doctor did not report the configured selector from its locked deployment snapshot."

    Assert-CodexDevEqual `
        $script:SelfTestRealUserSelectorBefore `
        ([System.Environment]::GetEnvironmentVariable(
            "CODEX_CLI_PATH",
            [System.EnvironmentVariableTarget]::User
        )) `
        "SelfTest mutated the real User-scope CODEX_CLI_PATH."

    Write-Host "windows_desktop_dev tests: PASS"
    exit 0
} finally {
    Clear-CodexDevPersistentSelectorTestAdapter
    $resolvedTestRoot = [System.IO.Path]::GetFullPath($testRoot)
    if ($resolvedTestRoot.StartsWith($tempBase, [System.StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $resolvedTestRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
