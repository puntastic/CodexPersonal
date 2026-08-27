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
    $firstAgain = Install-CodexDevPackage -PackageDirectory $packageOne -ConfigPath $config -DeploymentRoot $deployRoot -SkipSmoke
    Assert-CodexDevEqual "already_selected" $firstAgain.Status "Repeated deploy was not idempotent."
    Assert-CodexDevEqual $oldEntrypoint $firstAgain.Previous.Entrypoint "Repeated deploy discarded the rollback entrypoint."

    $second = Install-CodexDevPackage -PackageDirectory $packageTwo -ConfigPath $config -DeploymentRoot $deployRoot -SkipSmoke
    Assert-CodexDevEqual $second.Release.Entrypoint (Get-CodexCliPathFromConfig $config) "Second deploy did not update config."

    $rolledBack = Invoke-CodexDevRollback -ConfigPath $config -DeploymentRoot $deployRoot
    Assert-CodexDevEqual "previous_selected_for_restart" $rolledBack.Status "Rollback did not settle."
    Assert-CodexDevEqual $first.Release.Entrypoint (Get-CodexCliPathFromConfig $config) "Rollback did not restore the first release."

    Write-Host "windows_desktop_dev tests: PASS"
    exit 0
} finally {
    $resolvedTestRoot = [System.IO.Path]::GetFullPath($testRoot)
    if ($resolvedTestRoot.StartsWith($tempBase, [System.StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $resolvedTestRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
