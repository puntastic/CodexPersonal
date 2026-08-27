Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

. (Join-Path $PSScriptRoot "environment.ps1")

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
