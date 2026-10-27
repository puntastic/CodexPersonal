[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Destination,
    [Parameter(Mandatory = $true)][string]$ApprovedPackagePath,
    [Parameter(Mandatory = $true)][ValidatePattern('^[a-f0-9]{64}$')][string]$ExpectedPackageFingerprint,
    [Parameter(Mandatory = $true)][ValidatePattern('^[a-f0-9]{64}$')][string]$ExpectedExecutableSha256,
    [Parameter(Mandatory = $true)][string]$ConfigPath,
    [Parameter(Mandatory = $true)][string]$DeploymentRoot,
    [string[]]$EvidencePath = @()
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'deployment_config.ps1')
. (Join-Path $PSScriptRoot 'inspection.ps1')
$destinationPath = [IO.Path]::GetFullPath($Destination).TrimEnd([char[]]'\/')
if (Test-Path -LiteralPath $destinationPath) { throw 'Choose a new recovery-kit folder; existing content is never overwritten.' }
$parent = Get-Item -LiteralPath (Split-Path -Parent $destinationPath) -ErrorAction Stop
if (-not $parent.PSIsContainer) { throw 'Recovery-kit parent must be an existing directory.' }
$ancestor = $parent
while ($null -ne $ancestor) {
    if (($ancestor.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw 'Recovery-kit destination must not traverse a reparse point.' }
    $ancestor = $ancestor.Parent
}
foreach ($cache in @((Join-Path $env:LOCALAPPDATA 'OpenAI'), (Join-Path $env:LOCALAPPDATA 'Packages'), $DeploymentRoot)) {
    $boundary = [IO.Path]::GetFullPath($cache).TrimEnd([char[]]'\/')
    if ($destinationPath.Equals($boundary, [StringComparison]::OrdinalIgnoreCase) -or
        $destinationPath.StartsWith($boundary + '\', [StringComparison]::OrdinalIgnoreCase)) {
        throw 'The recovery kit must live outside application caches and deployment storage.'
    }
}
$package = Get-CodexDevInspectionPackage $ApprovedPackagePath
$entrypoint = Join-Path $package.path 'bin\codex.exe'
$exe = Get-CodexDevInspectionFile $entrypoint
if ($package.status -ne 'observed' -or $package.fingerprint -cne $ExpectedPackageFingerprint -or
    $exe.status -ne 'observed' -or $exe.sha256 -cne $ExpectedExecutableSha256) {
    throw 'Approved package or executable does not match the supplied exact evidence; no kit was created.'
}
$evidence = @($EvidencePath | ForEach-Object {
    $observed = Get-CodexDevInspectionFile ([IO.Path]::GetFullPath($_))
    if ($observed.status -ne 'observed') { throw 'An evidence anchor is unavailable; no kit was created.' }
    $observed
})
$sourceNames = @('inspection.ps1', 'deployment_config.ps1', 'inspect-environment.ps1', 'recovery-start.ps1', 'recovery-start-here.md')
$sources = @($sourceNames | ForEach-Object {
    $source = Get-CodexDevInspectionFile (Join-Path $PSScriptRoot $_)
    if ($source.status -ne 'observed') { throw 'A recovery-kit source is unavailable; no kit was created.' }
    [pscustomobject]@{ name = $_; sha256 = $source.sha256; source_path = $source.path }
})
# All authority/evidence validation precedes this first write. No config, history,
# credential, evidence-body or binary copies enter the portable kit.
New-Item -ItemType Directory -Path $destinationPath -ErrorAction Stop | Out-Null
foreach ($source in $sources) {
    Copy-Item -LiteralPath $source.source_path -Destination (Join-Path $destinationPath $source.name) -ErrorAction Stop
    if ((Get-CodexDevInspectionFile (Join-Path $destinationPath $source.name)).sha256 -cne $source.sha256) { throw 'Recovery-kit copy failed verification; partial folder retained for inspection.' }
}
$manifest = [ordered]@{
    schema_version = 1; created_at_utc = [DateTime]::UtcNow.ToString('o')
    purpose = 'Read-only inspection first. No automatic selector change, repair, launch or resume.'
    config_path = [IO.Path]::GetFullPath($ConfigPath); deployment_root = [IO.Path]::GetFullPath($DeploymentRoot)
    approved_package_path = $package.path; package_fingerprint = $package.fingerprint
    approved_executable_path = $entrypoint; executable_sha256 = $exe.sha256
    evidence_anchors = $evidence; copied_sources = $sources
    limits = @('The package remains at its original path; this is not an offline binary backup.',
        'Hashes bind this kit to observed bytes, not future compatibility or recovery success.',
        'Evidence anchors are paths and hashes only. Auth, config contents and session histories were not copied.')
}
[IO.File]::WriteAllText((Join-Path $destinationPath 'recovery-kit.json'), ($manifest | ConvertTo-Json -Depth 8), [Text.UTF8Encoding]::new($false))
$launcher = '@echo off' + "`r`n" + 'powershell.exe -NoLogo -NoProfile -NoExit -File "%~dp0recovery-start.ps1"' + "`r`n"
[IO.File]::WriteAllText((Join-Path $destinationPath 'Start here.cmd'), $launcher, [Text.UTF8Encoding]::new($false))
$readback = Get-Content -LiteralPath (Join-Path $destinationPath 'recovery-kit.json') -Raw | ConvertFrom-Json
if ($readback.executable_sha256 -cne $ExpectedExecutableSha256) { throw 'Recovery-kit manifest readback failed; partial folder retained.' }
[pscustomobject]@{ status = 'created'; destination = $destinationPath; manifest = Get-CodexDevInspectionFile (Join-Path $destinationPath 'recovery-kit.json'); copied_files = $sources.Count; binary_copied = $false }
