function Get-CodexDevLastPackagePointerPath {
    param([string]$TaskToken = (Get-CodexDevTaskToken))

    return Join-Path $script:CodexDevRepositoryRoot "codex-rs\target\desktop-dev\last-package.$TaskToken.json"
}

function Test-CodexDevSourceStateEqual {
    param(
        [object]$Before,
        [object]$After
    )

    return [string]$Before.Head -eq [string]$After.Head -and
        [string]$Before.Branch -eq [string]$After.Branch -and
        [string]$Before.GitVisibleSourceFingerprint -eq [string]$After.GitVisibleSourceFingerprint
}

function Set-CodexDevLastPackagePointer {
    param(
        [string]$PointerPath,
        [object]$Pointer
    )

    New-Item -ItemType Directory -Path (Split-Path -Parent $PointerPath) -Force | Out-Null
    $lockPath = "$PointerPath.lock"
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    $lock = $null
    while ($null -eq $lock) {
        try {
            $lock = [System.IO.File]::Open(
                $lockPath,
                [System.IO.FileMode]::OpenOrCreate,
                [System.IO.FileAccess]::ReadWrite,
                [System.IO.FileShare]::None
            )
        } catch [System.IO.IOException] {
            if ([DateTime]::UtcNow -ge $deadline) {
                throw "Timed out waiting to update task package pointer: $PointerPath"
            }
            Start-Sleep -Milliseconds 100
        }
    }
    try {
        if (Test-Path -LiteralPath $PointerPath -PathType Leaf) {
            $existing = Get-Content -LiteralPath $PointerPath -Raw | ConvertFrom-Json
            if (-not [string]::IsNullOrWhiteSpace([string]$existing.StartedAtUtc) -and
                [DateTime]::Parse([string]$existing.StartedAtUtc).ToUniversalTime() -gt
                [DateTime]::Parse([string]$Pointer.StartedAtUtc).ToUniversalTime()) {
                return $false
            }
        }
        Write-CodexDevJson -Path $PointerPath -Value $Pointer
        return $true
    } finally {
        $lock.Dispose()
    }
}

function Complete-CodexDevBuildPointer {
    param(
        [bool]$SourceStable,
        [string]$PointerPath,
        [object]$Pointer
    )

    if (-not $SourceStable) {
        return [pscustomobject]@{ Advanced = $false; Error = $null }
    }
    try {
        return [pscustomobject]@{
            Advanced = Set-CodexDevLastPackagePointer `
                -PointerPath $PointerPath `
                -Pointer $Pointer
            Error = $null
        }
    } catch {
        return [pscustomobject]@{
            Advanced = $false
            Error = $_.Exception.Message
        }
    }
}

function Assert-CodexDevPackageExecutableTargets {
    param(
        [string]$PackageRoot,
        [string]$ExpectedTarget = (Get-CodexDevHostTarget)
    )

    foreach ($executable in @(Get-ChildItem -LiteralPath $PackageRoot -Recurse -File -Filter "*.exe")) {
        $relativePath = [System.IO.Path]::GetRelativePath($PackageRoot, $executable.FullName)
        $executablePath = $executable.FullName
        $actualTarget = Get-CodexDevWindowsExecutableTarget $executablePath
        if ($actualTarget -ne $ExpectedTarget) {
            throw "Package executable '$relativePath' targets '$actualTarget'; expected '$ExpectedTarget'."
        }
    }
}

function Get-CodexDevPackageInfo {
    param(
        [string]$PackageDirectory,
        [switch]$SkipSmoke
    )

    $packageRoot = (Resolve-Path -LiteralPath $PackageDirectory -ErrorAction Stop).Path
    foreach ($relativePath in $script:CodexDevExpectedPackageFiles) {
        $path = Join-Path $packageRoot $relativePath
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            throw "Package is incomplete; missing $relativePath in $packageRoot"
        }
    }
    $manifest = Get-Content -LiteralPath (Join-Path $packageRoot "codex-package.json") -Raw |
        ConvertFrom-Json
    $expectedTarget = Get-CodexDevHostTarget
    if ([string]$manifest.target -ne $expectedTarget -or [string]$manifest.variant -ne "codex") {
        throw "Package manifest is not a native Codex package: target=$($manifest.target), variant=$($manifest.variant)"
    }
    Assert-CodexDevPackageExecutableTargets -PackageRoot $packageRoot -ExpectedTarget $expectedTarget

    $provenancePath = Join-Path $packageRoot "codex-dev-build.json"
    $artifactFiles = @(Get-ChildItem -LiteralPath $packageRoot -Recurse -File | Where-Object {
        -not [string]::Equals(
            $_.FullName,
            $provenancePath,
            [System.StringComparison]::OrdinalIgnoreCase
        )
    } | Sort-Object FullName)
    $hashRows = foreach ($artifactFile in $artifactFiles) {
        $relativePath = [System.IO.Path]::GetRelativePath($packageRoot, $artifactFile.FullName)
        [pscustomobject]@{
            Path = $relativePath.Replace("\", "/")
            Sha256 = (Get-FileHash -LiteralPath $artifactFile.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
            Bytes = $artifactFile.Length
        }
    }
    $fingerprintMaterial = ($hashRows | ForEach-Object { "$($_.Path)=$($_.Sha256)" }) -join "`n"
    $fingerprint = Get-CodexDevSha256Text $fingerprintMaterial
    $provenance = $null
    $provenanceStatus = "unavailable"
    if (Test-Path -LiteralPath $provenancePath -PathType Leaf) {
        $provenance = Get-Content -LiteralPath $provenancePath -Raw | ConvertFrom-Json
        if ([int]$provenance.SchemaVersion -ne 1) {
            throw "Unsupported package provenance schema in ${provenancePath}: $($provenance.SchemaVersion)"
        }
        if ([string]$provenance.ArtifactFingerprint -ne $fingerprint) {
            throw "Package provenance does not match the package fingerprint: $packageRoot"
        }
        $provenanceStatus = [string]$provenance.Status
        if ($provenanceStatus -notin @("stable", "source_changed_during_build")) {
            throw "Unsupported package provenance status '$provenanceStatus' in $provenancePath"
        }
    }
    $versionOutput = $null
    if (-not $SkipSmoke) {
        $smokeBase = [System.IO.Path]::GetFullPath(
            (Join-Path $script:CodexDevRepositoryRoot "codex-rs\target\desktop-dev\smoke")
        )
        New-Item -ItemType Directory -Path $smokeBase -Force | Out-Null
        $smokeHome = Join-Path $smokeBase ([guid]::NewGuid().ToString("N"))
        New-Item -ItemType Directory -Path $smokeHome | Out-Null
        $oldCodexHome = $env:CODEX_HOME
        $oldSqliteHome = $env:CODEX_SQLITE_HOME
        try {
            $env:CODEX_HOME = $smokeHome
            $env:CODEX_SQLITE_HOME = $smokeHome
            $versionOutput = (Invoke-CodexDevNative -FilePath (Join-Path $packageRoot "bin\codex.exe") -ArgumentList @("--version") -Capture) -join "`n"
        } finally {
            if ($null -eq $oldCodexHome) { Remove-Item Env:CODEX_HOME -ErrorAction SilentlyContinue } else { $env:CODEX_HOME = $oldCodexHome }
            if ($null -eq $oldSqliteHome) { Remove-Item Env:CODEX_SQLITE_HOME -ErrorAction SilentlyContinue } else { $env:CODEX_SQLITE_HOME = $oldSqliteHome }
            $resolvedSmoke = [System.IO.Path]::GetFullPath($smokeHome)
            if ($resolvedSmoke.StartsWith($smokeBase, [System.StringComparison]::OrdinalIgnoreCase)) {
                Remove-Item -LiteralPath $resolvedSmoke -Recurse -Force -ErrorAction SilentlyContinue
            }
        }
    }
    return [pscustomobject]@{
        PackageRoot = $packageRoot
        Entrypoint = Join-Path $packageRoot "bin\codex.exe"
        Target = [string]$manifest.target
        Version = [string]$manifest.version
        Fingerprint = $fingerprint
        Files = @($hashRows)
        VersionOutput = $versionOutput
        ProvenanceStatus = $provenanceStatus
        Provenance = $provenance
    }
}

function Invoke-CodexDevBuild {
    param(
        [string]$CargoProfile,
        [string]$PackageDirectory,
        [string]$CargoPath,
        [string]$CargoHome,
        [string]$RustupHome,
        [string]$PythonPath,
        [string]$RipgrepPath
    )

    $environment = Initialize-CodexDevEnvironment -CargoPath $CargoPath -CargoHome $CargoHome -RustupHome $RustupHome -PythonPath $PythonPath
    $sourceBefore = Get-CodexDevGitState
    $taskToken = Get-CodexDevTaskToken
    $buildId = [guid]::NewGuid().ToString("N")
    $startedAtUtc = [DateTime]::UtcNow.ToString("o")
    $pointerPath = Get-CodexDevLastPackagePointerPath -TaskToken $taskToken
    $output = if (-not [string]::IsNullOrWhiteSpace($PackageDirectory)) {
        [System.IO.Path]::GetFullPath($PackageDirectory)
    } else {
        $dirtySuffix = if ($sourceBefore.Dirty) { "-dirty" } else { "" }
        $stamp = [DateTime]::UtcNow.ToString("yyyyMMdd-HHmmssfff")
        $name = "$($sourceBefore.ShortHead)$dirtySuffix-$CargoProfile-$stamp-$($buildId.Substring(0, 12))"
        Join-Path $script:CodexDevRepositoryRoot "codex-rs\target\desktop-dev\packages\$name"
    }
    if (Test-Path -LiteralPath $output) {
        throw "Build output already exists; choose a new package directory: $output"
    }
    $rg = Resolve-CodexDevRipgrep $RipgrepPath
    $builder = Join-Path $script:CodexDevRepositoryRoot "scripts\build_codex_package.py"
    $oldRepoRoot = $env:CODEX_REPO_ROOT
    try {
        $env:CODEX_REPO_ROOT = $script:CodexDevRepositoryRoot
        Invoke-CodexDevNative -FilePath $environment.Python -ArgumentList @(
            $builder,
            "--variant", "codex",
            "--target", $environment.HostTarget,
            "--cargo-native-host",
            "--cargo-locked",
            "--cargo-profile", $CargoProfile,
            "--cargo", $environment.Cargo,
            "--rg-bin", $rg,
            "--package-dir", $output
        )
    } finally {
        if ($null -eq $oldRepoRoot) { Remove-Item Env:CODEX_REPO_ROOT -ErrorAction SilentlyContinue } else { $env:CODEX_REPO_ROOT = $oldRepoRoot }
    }
    $package = Get-CodexDevPackageInfo $output
    $sourceAfter = Get-CodexDevGitState
    $sourceStable = Test-CodexDevSourceStateEqual -Before $sourceBefore -After $sourceAfter
    $provenanceStatus = if ($sourceStable) { "stable" } else { "source_changed_during_build" }
    $provenance = [ordered]@{
        SchemaVersion = 1
        Status = $provenanceStatus
        BuildId = $buildId
        TaskToken = $taskToken
        Repository = $script:CodexDevRepositoryRoot
        StartedAtUtc = $startedAtUtc
        CompletedAtUtc = [DateTime]::UtcNow.ToString("o")
        CargoProfile = $CargoProfile
        CargoLocked = $true
        ArtifactFingerprint = $package.Fingerprint
        SourceBefore = $sourceBefore
        SourceAfter = $sourceAfter
    }
    Write-CodexDevJson -Path (Join-Path $output "codex-dev-build.json") -Value $provenance
    $package = Get-CodexDevPackageInfo $output
    $receipt = $null
    $receiptError = $null
    try {
        $receipt = Write-CodexDevReceipt -Action "Build" -Details ([ordered]@{
            CargoProfile = $CargoProfile
            Environment = $environment
            Package = $package
            SourceAfter = $sourceAfter
            SourceStable = $sourceStable
        }) -Source $sourceBefore
    } catch {
        $receiptError = $_.Exception.Message
    }
    $pointer = [ordered]@{
        SchemaVersion = 1
        Status = if ($sourceStable) { "ready" } else { "source_changed_during_build" }
        BuildId = $buildId
        TaskToken = $taskToken
        PackageRoot = $package.PackageRoot
        ArtifactFingerprint = $package.Fingerprint
        ProvenanceStatus = $package.ProvenanceStatus
        Receipt = $receipt
        ReceiptError = $receiptError
        StartedAtUtc = $startedAtUtc
        CompletedAtUtc = [DateTime]::UtcNow.ToString("o")
    }
    $pointerResult = Complete-CodexDevBuildPointer `
        -SourceStable $sourceStable `
        -PointerPath $pointerPath `
        -Pointer $pointer
    $pointerAdvanced = [bool]$pointerResult.Advanced
    $pointerError = $pointerResult.Error
    return [pscustomobject]@{
        Status = if (-not $sourceStable) {
            "source_changed_during_build"
        } elseif ($null -ne $pointerError) {
            "built_pointer_failed"
        } elseif (-not $pointerAdvanced) {
            "built_pointer_superseded"
        } elseif ($null -ne $receiptError) {
            "built_receipt_failed"
        } else {
            "built"
        }
        Package = $package
        Receipt = $receipt
        ReceiptError = $receiptError
        LastPackagePointer = $pointerPath
        PointerAdvanced = $pointerAdvanced
        PointerError = $pointerError
        Next = if ($sourceStable) {
            ".\codex-dev.ps1 -Action Deploy -PackageDirectory '$($package.PackageRoot)'"
        } else {
            ".\codex-dev.ps1 -Action Build -CargoProfile $CargoProfile"
        }
    }
}

function Resolve-CodexDevPackageDirectory {
    param([string]$PackageDirectory)

    if (-not [string]::IsNullOrWhiteSpace($PackageDirectory)) {
        return (Resolve-Path -LiteralPath $PackageDirectory).Path
    }
    $taskToken = Get-CodexDevTaskToken
    $pointerPath = Get-CodexDevLastPackagePointerPath -TaskToken $taskToken
    if (-not (Test-Path -LiteralPath $pointerPath -PathType Leaf)) {
        throw "No package was supplied and task '$taskToken' has no recorded build. Run Build or pass -PackageDirectory."
    }
    $pointer = Get-Content -LiteralPath $pointerPath -Raw | ConvertFrom-Json
    if ([int]$pointer.SchemaVersion -ne 1 -or [string]$pointer.Status -ne "ready") {
        throw "Task '$taskToken' has no deployable implicit build; pointer status is '$($pointer.Status)'. Pass an explicit -PackageDirectory to inspect or deploy a retained package."
    }
    $package = Get-CodexDevPackageInfo -PackageDirectory ([string]$pointer.PackageRoot) -SkipSmoke
    if ($package.Fingerprint -ne [string]$pointer.ArtifactFingerprint) {
        throw "Task '$taskToken' package pointer no longer matches its recorded artifact fingerprint."
    }
    if ($package.ProvenanceStatus -ne "stable" -or [string]$pointer.ProvenanceStatus -ne "stable") {
        throw "Task '$taskToken' implicit package does not have stable build provenance. Pass -PackageDirectory explicitly if intentional."
    }
    if ([string]$package.Provenance.BuildId -ne [string]$pointer.BuildId -or
        [string]$package.Provenance.TaskToken -ne [string]$pointer.TaskToken -or
        [string]$pointer.TaskToken -ne $taskToken) {
        throw "Task '$taskToken' package pointer does not match its package provenance identity."
    }
    return $package.PackageRoot
}
