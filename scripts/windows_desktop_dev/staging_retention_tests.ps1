# Entirely mocked deployment staging: no real package, config, selector or file writes.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'workflow.ps1')

$root = 'C:\mock-codex-staging'
$source = 'C:\mock-codex-candidate'
$fingerprint = 'a' * 64
function Require($condition, [string]$message) {
    if (-not $condition) { throw $message }
}
function Get-CodexDevPackageInfo {
    param($PackageDirectory, [switch]$SkipSmoke)
    $value = if ($script:Case -eq 'invalid-move') { '..\unexpected' } else { $fingerprint }
    if ($PackageDirectory -ne $source -and $script:Case -eq 'hash') { $value = 'b' * 64 }
    return [pscustomobject]@{ PackageRoot = $PackageDirectory; Fingerprint = $value }
}
function Get-CodexDevPackageSelection { param($Package) return [pscustomobject]@{} }
function Get-CodexDevDeploymentStatus {
    param($ConfigPath, $DeploymentRoot)
    return [pscustomobject]@{ Status = 'consistent'; ConfiguredEntrypoint = 'old'; PersistentEntrypoint = 'old'; DriftReasons = @() }
}
function Get-CodexDevDeploymentStatusUnlocked {
    param($ConfigPath, $DeploymentRoot)
    return Get-CodexDevDeploymentStatus $ConfigPath $DeploymentRoot
}
function Invoke-WithCodexDevDeploymentLock { param($DeploymentRoot, [scriptblock]$Body) & $Body }
function Invoke-WithCodexDevConfigLock { param($ConfigPath, [scriptblock]$Body) & $Body }
function Repair-CodexDevInterruptedTransaction { param($ConfigPath, $DeploymentRoot) return $null }
function Test-Path { param($LiteralPath, $PathType) return $false }
function New-Item {
    param($ItemType, $Path, [switch]$Force)
    Require ($ItemType -eq 'Directory') 'Unexpected item creation.'
    if ($Path -ne (Join-Path $root 'releases')) {
        Require ((Split-Path -Parent $Path) -eq $root) 'Staging creation escaped mock root.'
        Require ((Split-Path -Leaf $Path) -match '^\.staging-[0-9a-f]{32}$') 'Unexpected staging name.'
        $script:Staging = $Path
        $script:StagingExists = $true
    }
}
function Get-ChildItem {
    param($LiteralPath, [switch]$Force)
    Require ($LiteralPath -eq $source) 'Unexpected source enumeration.'
    return 'mock-package-file'
}
function Copy-Item {
    [CmdletBinding()]
    param([Parameter(ValueFromPipeline)]$InputObject, $Destination, [switch]$Recurse, [switch]$Force)
    process {
        Require ($Destination -eq $script:Staging) 'Unexpected copy destination.'
        if ($script:Case -eq 'copy') { throw 'synthetic copy failure' }
    }
}
function Move-Item {
    param($LiteralPath, $Destination, [switch]$Force)
    Require (-not $Force) 'Staging move must not force replacement.'
    Require ($LiteralPath -eq $script:Staging) 'Unexpected move source.'
    Require ($Destination -eq (Join-Path $root "releases\sha256-$fingerprint")) 'Unexpected move destination.'
    $script:Moves++
    if ($script:Case -eq 'move') { throw 'synthetic move failure' }
    $script:StagingExists = $false
}
function Remove-Item {
    param($LiteralPath, [switch]$Recurse, [switch]$Force)
    $script:Deletes++
    throw 'Unexpected deletion.'
}
function Write-Warning {
    [CmdletBinding()]
    param([string]$Message)
    $script:Warnings++
    Require ($Message.Contains($script:Staging)) 'Warning lost the retained path.'
    Require ($PSBoundParameters['WarningAction'] -eq 'Continue') 'Warning could mask the primary error.'
    if ($script:ThrowWarning) { throw 'synthetic diagnostic failure' }
}
function Get-CodexCliConfigTransition {
    param($ConfigPath, $Entrypoint)
    # Stop the successful-staging case before any real selection operation.
    throw 'selection-boundary-stop'
}

$cases = @{
    copy = 'synthetic copy failure'
    hash = 'Staged package fingerprint changed during copy.'
    move = 'synthetic move failure'
    'invalid-move' = 'Staging or release move target escaped its intended deployment directory.'
    success = 'selection-boundary-stop'
}
foreach ($preference in @('Continue', 'Stop')) {
    $WarningPreference = $preference
    foreach ($entry in $cases.GetEnumerator()) {
        foreach ($diagnosticFailure in @($false, $true)) {
            $script:Case = $entry.Key
            $script:ThrowWarning = $diagnosticFailure
            $script:Staging = $null
            $script:StagingExists = $false
            $script:Moves = 0
            $script:Deletes = 0
            $script:Warnings = 0
            $errorMessage = $null
            try {
                $null = Install-CodexDevPackage -PackageDirectory $source -ConfigPath "$root\config.toml" -DeploymentRoot $root -Confirm:$false
            } catch { $errorMessage = $_.Exception.Message }
            Require ($errorMessage -eq $entry.Value) "Primary failure was hidden: $errorMessage"
            Require ($script:Deletes -eq 0) 'Staging cleanup deleted files.'
            Require ($script:StagingExists -eq ($entry.Key -ne 'success')) 'Staging retention/move state differs.'
            Require ($script:Warnings -eq [int]($entry.Key -ne 'success')) 'Staging warning behavior differs.'
            Require ($script:Moves -eq [int]($entry.Key -in @('move', 'success'))) 'Move gate was bypassed.'
        }
    }
}
Write-Output 'Staging retention: 20 mocked cases passed; no filesystem/native/selector operations performed.'
