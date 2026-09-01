Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

. (Join-Path $PSScriptRoot "environment.ps1")
. (Join-Path $PSScriptRoot "tooling.ps1")

function Assert-CodexDevEnvironmentTestEqual {
    param(
        [object]$Expected,
        [object]$Actual,
        [string]$Message
    )

    if ($Expected -ne $Actual) {
        throw "$Message`nExpected: $Expected`nActual:   $Actual"
    }
}

$testRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("codex-msvc-environment-tests-" + [guid]::NewGuid().ToString("N"))
$savedEnvironment = @{
    PATH = $env:PATH
    VCToolsInstallDir = $env:VCToolsInstallDir
    VSCMD_ARG_HOST_ARCH = $env:VSCMD_ARG_HOST_ARCH
    VSCMD_ARG_TGT_ARCH = $env:VSCMD_ARG_TGT_ARCH
    VSINSTALLDIR = $env:VSINSTALLDIR
    RUSTY_V8_ARCHIVE = $env:RUSTY_V8_ARCHIVE
    RUSTY_V8_SRC_BINDING_PATH = $env:RUSTY_V8_SRC_BINDING_PATH
    V8_FROM_SOURCE = $env:V8_FROM_SOURCE
    CODEX_REPO_ROOT = $env:CODEX_REPO_ROOT
    BAZELISK_HOME = $env:BAZELISK_HOME
    CODEX_BAZEL_OUTPUT_USER_ROOT = $env:CODEX_BAZEL_OUTPUT_USER_ROOT
}
$testVariableName = "CODEX_DEV_ENVIRONMENT_ROW_TEST"
$savedTestVariable = [Environment]::GetEnvironmentVariable($testVariableName, "Process")

try {
    New-Item -ItemType Directory -Path $testRoot | Out-Null

    Assert-CodexDevEnvironmentTestEqual `
        "Microsoft.VisualStudio.Component.VC.Tools.x86.x64" `
        (Get-CodexDevMsvcRequiredComponent -Architecture x64) `
        "x64 selected the wrong Visual Studio component."
    Assert-CodexDevEnvironmentTestEqual `
        "Microsoft.VisualStudio.Component.VC.Tools.ARM64" `
        (Get-CodexDevMsvcRequiredComponent -Architecture arm64) `
        "ARM64 selected the wrong Visual Studio component."

    $bazeliskX64 = Get-CodexDevBazeliskAsset -HostTarget "x86_64-pc-windows-msvc"
    Assert-CodexDevEnvironmentTestEqual "1.28.1" $bazeliskX64.Version "The Bazelisk version drifted from CI."
    Assert-CodexDevEnvironmentTestEqual `
        "bazelisk-windows-amd64.exe" `
        $bazeliskX64.FileName `
        "The x64 Bazelisk asset was not selected."
    Assert-CodexDevEnvironmentTestEqual `
        "b9d65a1f7c2d7af885a96a4fd5aa36b40fb41816d30944390569eef908bdc954" `
        $bazeliskX64.Sha256 `
        "The x64 Bazelisk release digest changed."
    $repositoryRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
    $bazelCiSetup = Get-Content -LiteralPath (Join-Path $repositoryRoot ".github\actions\setup-bazel-ci\action.yml") -Raw
    if ($bazelCiSetup -notmatch '(?m)^\s*bazelisk-version:\s*1\.28\.1\s*$') {
        throw "The Windows development lane Bazelisk pin no longer matches setup-bazel-ci."
    }

    $vcToolsRoot = Join-Path $testRoot "VC\Tools\MSVC\14.99.99999"
    $msvcBin = Join-Path $vcToolsRoot "bin\Hostx64\x64"
    $unrelatedBin = Join-Path $testRoot "unrelated"
    New-Item -ItemType Directory -Path $msvcBin -Force | Out-Null
    New-Item -ItemType Directory -Path $unrelatedBin -Force | Out-Null
    foreach ($path in @(
        (Join-Path $msvcBin "cl.exe"),
        (Join-Path $msvcBin "link.exe"),
        (Join-Path $unrelatedBin "link.exe")
    )) {
        [System.IO.File]::WriteAllBytes($path, [byte[]](0))
    }

    $env:VCToolsInstallDir = $vcToolsRoot
    $env:VSCMD_ARG_HOST_ARCH = "x64"
    $env:VSCMD_ARG_TGT_ARCH = "x64"
    $env:VSINSTALLDIR = Join-Path $testRoot "Visual Studio"
    $env:PATH = "$unrelatedBin;$($savedEnvironment.PATH)"

    if (-not (Test-CodexDevMsvcEnvironment -Architecture x64)) {
        throw "A complete, architecture-matched MSVC environment was rejected."
    }
    $toolset = Resolve-CodexDevMsvcToolset -Architecture x64
    Assert-CodexDevEnvironmentTestEqual `
        ([System.IO.Path]::GetFullPath((Join-Path $msvcBin "link.exe"))) `
        $toolset.Link `
        "MSVC resolution accepted an unrelated link.exe from PATH."
    Assert-CodexDevEnvironmentTestEqual `
        $env:VSINSTALLDIR `
        (Import-CodexDevMsvcEnvironment -Architecture x64) `
        "A valid existing MSVC environment was not reused."
    Add-CodexDevPathSegment $toolset.ToolDirectory
    Assert-CodexDevEnvironmentTestEqual `
        ([System.IO.Path]::GetFullPath($msvcBin)) `
        ([System.IO.Path]::GetFullPath(($env:PATH -split ";")[0])) `
        "A reused MSVC environment did not restore its exact tool directory to PATH."

    $env:VSCMD_ARG_TGT_ARCH = "arm64"
    if (Test-CodexDevMsvcEnvironment -Architecture x64) {
        throw "A target-architecture-mismatched MSVC environment was accepted."
    }
    $env:VSCMD_ARG_TGT_ARCH = "x64"

    Remove-Item -LiteralPath (Join-Path $msvcBin "link.exe") -Force
    if (Test-CodexDevMsvcEnvironment -Architecture x64) {
        throw "An unrelated link.exe on PATH masked a missing MSVC linker."
    }

    $importedPath = Join-Path $testRoot "case-insensitive-path"
    $imported = Import-CodexDevEnvironmentRows -Rows @(
        "$testVariableName=present",
        "pAtH=$importedPath"
    )
    if (-not $imported) {
        throw "A mixed-case Path row was not recognized."
    }
    Assert-CodexDevEnvironmentTestEqual $importedPath $env:PATH "A mixed-case Path row was not imported."
    Assert-CodexDevEnvironmentTestEqual "present" ([Environment]::GetEnvironmentVariable($testVariableName, "Process")) "A non-Path environment row was not imported."

    $stalePath = Join-Path $testRoot "stale-path"
    $developerPath = Join-Path $testRoot "developer-path"
    $imported = Import-CodexDevEnvironmentRows -Rows @(
        "PATH=$developerPath",
        "Path=$stalePath"
    )
    if (-not $imported) {
        throw "Duplicate PATH rows were not recognized."
    }
    Assert-CodexDevEnvironmentTestEqual `
        $developerPath `
        $env:PATH `
        "A stale mixed-case Path row displaced the canonical developer PATH."

    $fakePython = Join-Path $testRoot "python\python.exe"
    New-Item -ItemType Directory -Path (Split-Path -Parent $fakePython) -Force | Out-Null
    [System.IO.File]::WriteAllBytes($fakePython, [byte[]](0))
    $pythonShim = Initialize-CodexDevPython3Shim `
        -Python $fakePython `
        -BinRoot (Join-Path $testRoot "python-shim")
    $expectedShim = "@echo off`r`n`"$([System.IO.Path]::GetFullPath($fakePython))`" %*`r`n"
    Assert-CodexDevEnvironmentTestEqual `
        $expectedShim `
        (Get-Content -LiteralPath $pythonShim -Raw) `
        "The Python 3 command shim did not preserve the selected interpreter."

    $fakeArchive = Join-Path $testRoot "v8\rusty_v8.lib.gz"
    $fakeBinding = Join-Path $testRoot "v8\src_binding.rs"
    New-Item -ItemType Directory -Path (Split-Path -Parent $fakeArchive) -Force | Out-Null
    [System.IO.File]::WriteAllBytes($fakeArchive, [byte[]](1))
    [System.IO.File]::WriteAllBytes($fakeBinding, [byte[]](2))
    $realRustyV8Resolver = (Get-Command Resolve-CodexDevRustyV8CargoEnvironment).ScriptBlock
    try {
        function Resolve-CodexDevRustyV8CargoEnvironment {
            param([string]$Python, [string]$HostTarget, [string]$CacheRoot)
            return @{
                RUSTY_V8_ARCHIVE = $fakeArchive
                RUSTY_V8_SRC_BINDING_PATH = $fakeBinding
            }
        }
        Remove-Item Env:RUSTY_V8_ARCHIVE -ErrorAction SilentlyContinue
        Remove-Item Env:RUSTY_V8_SRC_BINDING_PATH -ErrorAction SilentlyContinue
        Enable-CodexDevRustyV8CargoEnvironment -Python "fake-python" -HostTarget "x86_64-pc-windows-msvc"
        Assert-CodexDevEnvironmentTestEqual `
            ([System.IO.Path]::GetFullPath($fakeArchive)) `
            $env:RUSTY_V8_ARCHIVE `
            "Rusty V8 archive resolution did not reach Cargo."
        Assert-CodexDevEnvironmentTestEqual `
            ([System.IO.Path]::GetFullPath($fakeBinding)) `
            $env:RUSTY_V8_SRC_BINDING_PATH `
            "Rusty V8 binding resolution did not reach Cargo."

        function Resolve-CodexDevRustyV8CargoEnvironment {
            param([string]$Python, [string]$HostTarget, [string]$CacheRoot)
            return @{}
        }
        $env:RUSTY_V8_ARCHIVE = "preserved-archive"
        $env:RUSTY_V8_SRC_BINDING_PATH = "preserved-binding"
        Enable-CodexDevRustyV8CargoEnvironment -Python "fake-python" -HostTarget "x86_64-pc-windows-msvc"
        Assert-CodexDevEnvironmentTestEqual "preserved-archive" $env:RUSTY_V8_ARCHIVE "A complete caller V8 override was replaced."
        Assert-CodexDevEnvironmentTestEqual "preserved-binding" $env:RUSTY_V8_SRC_BINDING_PATH "A complete caller V8 override was replaced."
    } finally {
        Set-Item -LiteralPath Function:\Resolve-CodexDevRustyV8CargoEnvironment -Value $realRustyV8Resolver
    }

    Write-Host "windows_desktop_dev environment tests: PASS"
} finally {
    foreach ($name in $savedEnvironment.Keys) {
        [Environment]::SetEnvironmentVariable($name, $savedEnvironment[$name], "Process")
    }
    [Environment]::SetEnvironmentVariable($testVariableName, $savedTestVariable, "Process")
    if (Test-Path -LiteralPath $testRoot) {
        Remove-Item -LiteralPath $testRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
