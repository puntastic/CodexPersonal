[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$ConfigPath,
    [Parameter(Mandatory = $true)][string]$DeploymentRoot,
    [string]$DesktopInstallLocation,
    [switch]$Json
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'deployment_config.ps1')
. (Join-Path $PSScriptRoot 'inspection.ps1')
$report = Get-CodexDevEnvironmentInspection -ConfigPath $ConfigPath -DeploymentRoot $DeploymentRoot -DesktopInstallLocation $DesktopInstallLocation
if ($Json) { $report | ConvertTo-Json -Depth 12; return }
Write-Output 'Desktop recovery inspection - no settings changed'
Write-Output ("Snapshot: {0}; Desktop identity: {1} ({2})" -f $report.snapshot_status, $report.desktop.status, $report.desktop.source)
Write-Output ("User selector: {0}" -f $report.selectors.user.value)
Write-Output ("This PowerShell process inherited: {0}" -f $report.selectors.process.value)
Write-Output ("node_repl mirror ({0}): {1}" -f $report.config_mirror.status, $report.config_mirror.value)
Write-Output ("Ledger current ({0}): {1}" -f $report.ledger.status, $report.ledger.current_entrypoint)
Write-Output ("Pending journal: {0}" -f $report.pending_journal.status)
Write-Output ("Loaded direct CLI children observed: {0}; process reading: {1}" -f @($report.running.direct_cli_children).Count, $report.running.status)
foreach ($child in $report.running.direct_cli_children) {
    Write-Output ("  PID {0}: {1}; SHA256 {2}" -f $child.pid, $child.executable.path, $child.executable.sha256)
}
Write-Output 'This does not establish startup, resume or history integrity. Use -Json for bounded evidence details.'
if ($report.snapshot_status -eq 'unstable') { Write-Output 'State changed during inspection. Stop; wait for the other writer, then inspect again. Nothing was repaired.' }
