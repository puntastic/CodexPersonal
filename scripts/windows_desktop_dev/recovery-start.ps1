[CmdletBinding()]
param([switch]$Json, [switch]$ShowManualLaunch)
$ErrorActionPreference = 'Stop'
$kit = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'recovery-kit.json') -Raw | ConvertFrom-Json
if ($kit.schema_version -ne 1) { throw 'Unsupported recovery-kit manifest. Stop and ask for a current checked kit.' }
foreach ($source in $kit.copied_sources) {
    if ([IO.Path]::GetFileName($source.name) -cne $source.name) { throw 'Recovery-kit source name is invalid.' }
    $hash = (Get-FileHash -LiteralPath (Join-Path $PSScriptRoot $source.name) -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($hash -cne $source.sha256) { throw 'Recovery-kit source changed. Stop and check the kit before running it.' }
}
if ($ShowManualLaunch) {
    . (Join-Path $PSScriptRoot 'deployment_config.ps1')
    . (Join-Path $PSScriptRoot 'inspection.ps1')
    $package = Get-CodexDevInspectionPackage $kit.approved_package_path
    if ($package.status -ne 'observed' -or $package.fingerprint -cne $kit.package_fingerprint) {
        throw 'Approved package changed; no launch command offered.'
    }
    $actual = (Get-FileHash -LiteralPath $kit.approved_executable_path -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -cne $kit.executable_sha256) { throw 'Approved executable changed; no launch command offered.' }
    Write-Output 'This prints a command only. It does not start Codex or establish that a CLI session can resume your history.'
    Write-Output 'First quit the Desktop app and other Codex writers normally. Do not run two writers against the same home.'
    Write-Output 'Use only after choosing terminal fallback deliberately; review current evidence and do not relabel history modes.'
    Write-Output ("& '{0}'" -f ([string]$kit.approved_executable_path).Replace("'", "''"))
    return
}
& (Join-Path $PSScriptRoot 'inspect-environment.ps1') -ConfigPath $kit.config_path -DeploymentRoot $kit.deployment_root -Json:$Json
