Set-StrictMode -Version Latest

. (Join-Path $PSScriptRoot "common.ps1")
. (Join-Path $PSScriptRoot "environment.ps1")
. (Join-Path $PSScriptRoot "tooling.ps1")
. (Join-Path $PSScriptRoot "package.ps1")
. (Join-Path $PSScriptRoot "deployment_config.ps1")
. (Join-Path $PSScriptRoot "deployment_state.ps1")
. (Join-Path $PSScriptRoot "deployment_transaction.ps1")
. (Join-Path $PSScriptRoot "deployment.ps1")

function Measure-CodexDevStorageRoot {
    param([string]$Path)

    $root = [System.IO.Path]::GetFullPath($Path)
    if (-not (Test-Path -LiteralPath $root -PathType Container)) {
        return [pscustomobject]@{
            Root = $root
            Exists = $false
            DirectoryCount = 0
            FileCount = 0
            Bytes = 0
            GiB = 0
        }
    }
    $files = @(Get-ChildItem -LiteralPath $root -Recurse -File -ErrorAction Stop)
    $bytes = [long](($files | Measure-Object -Property Length -Sum).Sum)
    return [pscustomobject]@{
        Root = $root
        Exists = $true
        DirectoryCount = @(Get-ChildItem -LiteralPath $root -Directory).Count
        FileCount = $files.Count
        Bytes = $bytes
        GiB = [Math]::Round($bytes / 1GB, 3)
    }
}

function Get-CodexDevStorageInventory {
    param([string]$DeploymentRoot)

    $localRoot = Join-Path $script:CodexDevRepositoryRoot "codex-rs\target\desktop-dev"
    return [pscustomobject]@{
        Packages = Measure-CodexDevStorageRoot (Join-Path $localRoot "packages")
        Receipts = Measure-CodexDevStorageRoot (Join-Path $localRoot "receipts")
        Releases = Measure-CodexDevStorageRoot (Join-Path $DeploymentRoot "releases")
        ConfigBackups = Measure-CodexDevStorageRoot (Join-Path $DeploymentRoot "config-backups")
    }
}

function Get-CodexDevDoctorReport {
    param(
        [string]$CargoPath,
        [string]$CargoHome,
        [string]$RustupHome,
        [string]$PythonPath,
        [string]$RipgrepPath,
        [string]$ConfigPath,
        [string]$DeploymentRoot
    )

    $environment = try {
        Initialize-CodexDevEnvironment -CargoPath $CargoPath -CargoHome $CargoHome -RustupHome $RustupHome -PythonPath $PythonPath
    } catch {
        [pscustomobject]@{ Error = $_.Exception.Message }
    }
    $rgError = $null
    $rg = try {
        Resolve-CodexDevRipgrep $RipgrepPath
    } catch {
        $rgError = $_.Exception.Message
        $null
    }
    $tooling = Get-CodexDevToolStatus
    $configured = try { Get-CodexCliPathFromConfig $ConfigPath } catch { "ERROR: $($_.Exception.Message)" }
    $deployment = Get-CodexDevDeploymentStatus `
        -ConfigPath $ConfigPath `
        -DeploymentRoot $DeploymentRoot
    $configuredExists = -not [string]::IsNullOrWhiteSpace($configured) -and
        -not $configured.StartsWith("ERROR:") -and
        (Test-Path -LiteralPath $configured -PathType Leaf)
    $environmentReady = $null -eq $environment.PSObject.Properties["Error"]
    $capabilities = [pscustomobject]@{
        BuildReady = $environmentReady -and -not [string]::IsNullOrWhiteSpace($rg)
        FormatReady = $environmentReady -and $tooling.FormatReady
        JustReady = $environmentReady -and
            -not [string]::IsNullOrWhiteSpace($tooling.Just) -and
            [string]::IsNullOrWhiteSpace($tooling.JustError)
        DeployReady = $configuredExists -and $deployment.Status -in @("consistent", "unmanaged")
    }
    $driveName = [System.IO.Path]::GetPathRoot($script:CodexDevRepositoryRoot).TrimEnd("\").TrimEnd(":")
    $repoDrive = Get-PSDrive -Name $driveName -ErrorAction SilentlyContinue
    return [pscustomobject]@{
        Status = if ($capabilities.BuildReady -and $capabilities.FormatReady -and $capabilities.DeployReady) {
            "ready"
        } elseif ($capabilities.BuildReady -and $capabilities.DeployReady -and -not $capabilities.FormatReady) {
            "needs_tooling"
        } elseif ($capabilities.BuildReady -or $capabilities.FormatReady -or $capabilities.DeployReady) {
            "partial"
        } else {
            "needs_setup"
        }
        Repository = $script:CodexDevRepositoryRoot
        Source = Get-CodexDevGitState
        Environment = $environment
        Tooling = $tooling
        Capabilities = $capabilities
        Ripgrep = $rg
        RipgrepError = $rgError
        ConfigPath = $ConfigPath
        ConfiguredEntrypoint = $configured
        ConfiguredEntrypointExists = $configuredExists
        LiveEntrypoint = $env:CODEX_CLI_PATH
        RestartRequired = $configuredExists -and $env:CODEX_CLI_PATH -ne $configured
        DeploymentRoot = $DeploymentRoot
        Deployment = $deployment
        Storage = Get-CodexDevStorageInventory -DeploymentRoot $DeploymentRoot
        RepositoryDriveFreeGiB = if ($null -ne $repoDrive) { [Math]::Round($repoDrive.Free / 1GB, 2) } else { $null }
    }
}

function Invoke-CodexDesktopDev {
    [CmdletBinding()]
    param(
        [string]$Action,
        [string]$CargoProfile,
        [string]$PackageDirectory,
        [string[]]$JustArguments,
        [string]$CargoPath,
        [string]$CargoHome,
        [string]$RustupHome,
        [string]$PythonPath,
        [string]$RipgrepPath,
        [string]$ConfigPath = (Get-CodexDevDefaultConfigPath),
        [string]$DeploymentRoot = (Get-CodexDevDefaultDeploymentRoot),
        [switch]$WhatIf,
        [switch]$Json
    )

    if ([System.Environment]::OSVersion.Platform -ne [System.PlatformID]::Win32NT) {
        throw "The Desktop development lane currently supports Windows only."
    }
    switch ($Action) {
        "Help" {
            Write-CodexDevResult -Json:$Json -Value @"
CodexPersonal Windows Desktop lane

  .\codex-dev.ps1 -Action Doctor
  .\codex-dev.ps1 -Action Setup
  .\codex-dev.ps1 -Action Just -JustArguments @('test','-p','codex-state')
  .\codex-dev.ps1 -Action Build -CargoProfile dev-small
  .\codex-dev.ps1 -Action Deploy [-PackageDirectory <package>]
  # Restart Codex Desktop, then:
  .\codex-dev.ps1 -Action Verify
  .\codex-dev.ps1 -Action Rollback [-WhatIf]
  .\codex-dev.ps1 -Action SelfTest

Setup installs missing formatter helpers into the ignored local tool cache.
Build defaults to a fresh, ignored package directory, records its hashes and
source provenance, and advances only this task's last-stable package pointer.
Deploy stages an immutable copy, serializes config/state changes through a
recoverable transaction, changes only the existing CODEX_CLI_PATH setting for
the next restart, and records the prior entrypoint and selected build.
Rollback selects that prior entrypoint; it does not delete packages or data.
"@
        }
        "Doctor" {
            $parameters = @{
                CargoPath = $CargoPath
                CargoHome = $CargoHome
                RustupHome = $RustupHome
                PythonPath = $PythonPath
                RipgrepPath = $RipgrepPath
                ConfigPath = $ConfigPath
                DeploymentRoot = $DeploymentRoot
            }
            Write-CodexDevResult -Json:$Json -Value (Get-CodexDevDoctorReport @parameters)
        }
        "Setup" {
            $parameters = @{
                CargoPath = $CargoPath
                CargoHome = $CargoHome
                RustupHome = $RustupHome
                PythonPath = $PythonPath
                WhatIf = $WhatIf
            }
            Write-CodexDevResult -Json:$Json -Value (Invoke-CodexDevToolSetup @parameters)
        }
        "Just" {
            if ($JustArguments.Count -eq 0) { throw "Pass one or more -JustArguments." }
            $environment = Initialize-CodexDevEnvironment -CargoPath $CargoPath -CargoHome $CargoHome -RustupHome $RustupHome -PythonPath $PythonPath -RequireJust
            Invoke-CodexDevNative -FilePath $environment.Just -ArgumentList $JustArguments
        }
        "Build" {
            $parameters = @{
                CargoProfile = $CargoProfile
                PackageDirectory = $PackageDirectory
                CargoPath = $CargoPath
                CargoHome = $CargoHome
                RustupHome = $RustupHome
                PythonPath = $PythonPath
                RipgrepPath = $RipgrepPath
            }
            Write-CodexDevResult -Json:$Json -Value (Invoke-CodexDevBuild @parameters)
        }
        "Deploy" {
            $resolvedPackage = Resolve-CodexDevPackageDirectory $PackageDirectory
            $parameters = @{
                PackageDirectory = $resolvedPackage
                ConfigPath = $ConfigPath
                DeploymentRoot = $DeploymentRoot
                WhatIf = $WhatIf
            }
            Write-CodexDevResult -Json:$Json -Value (Install-CodexDevPackage @parameters)
        }
        "Verify" {
            Write-CodexDevResult -Json:$Json -Value (Invoke-CodexDevVerify `
                -ConfigPath $ConfigPath `
                -DeploymentRoot $DeploymentRoot)
        }
        "Rollback" {
            $parameters = @{
                ConfigPath = $ConfigPath
                DeploymentRoot = $DeploymentRoot
                WhatIf = $WhatIf
            }
            Write-CodexDevResult -Json:$Json -Value (Invoke-CodexDevRollback @parameters)
        }
        "SelfTest" {
            $pwsh = Join-Path $PSHOME "pwsh.exe"
            foreach ($testName in @(
                "environment_tests.ps1",
                "deployment_transaction_tests.ps1",
                "tests.ps1"
            )) {
                $testScript = Join-Path $PSScriptRoot $testName
                Invoke-CodexDevNative -FilePath $pwsh -ArgumentList @(
                    "-NoLogo", "-NoProfile", "-File", $testScript
                )
            }
        }
        default { throw "Unknown action: $Action" }
    }
}
