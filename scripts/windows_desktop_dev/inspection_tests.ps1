[CmdletBinding()]
param()
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'deployment_config.ps1')
. (Join-Path $PSScriptRoot 'inspection.ps1')
$script:Checks = 0
function Assert-InspectionEqual($Expected, $Actual, [string]$Message) {
    $script:Checks++
    if ($Expected -cne $Actual) { throw "$Message (expected '$Expected', got '$Actual')" }
}
function Assert-InspectionThrows([scriptblock]$Action, [string]$Message) {
    $script:Checks++
    try { & $Action | Out-Null } catch { return }
    throw $Message
}
function Write-InspectionFixture([string]$Path, [string]$Text) {
    $parent = Split-Path -Parent $Path
    New-Item -ItemType Directory -Path $parent -Force | Out-Null
    [IO.File]::WriteAllText($Path, $Text, [Text.UTF8Encoding]::new($false))
}
function Get-InspectionFixtureTree([string]$Root) {
    return (@(Get-ChildItem -LiteralPath $Root -Recurse -File | Sort-Object FullName | ForEach-Object {
        $_.FullName + '=' + (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
    }) -join "`n")
}
$fixture = Join-Path ([IO.Path]::GetTempPath()) ('codex-inspection-tests-' + [guid]::NewGuid().ToString('N'))
$savedProcesses = (Get-Command Get-CodexDevInspectionProcesses).ScriptBlock
$savedSelectors = (Get-Command Get-CodexDevInspectionSelectors).ScriptBlock
$savedAppx = (Get-Command Get-CodexDevInspectionAppxPackages).ScriptBlock
try {
    $config = Join-Path $fixture 'home\config.toml'
    $deploy = Join-Path $fixture 'deployment'
    $package = Join-Path $deploy 'releases\sha256-fixture'
    $exe = Join-Path $package 'bin\codex.exe'
    $desktop = Join-Path $fixture 'desktop'
    $manifest = '<Package><Identity Name="OpenAI.Codex" Version="1.2.3.4"/><Applications><Application Executable="app/ChatGPT.exe"/></Applications></Package>'
    Write-InspectionFixture (Join-Path $desktop 'AppxManifest.xml') $manifest
    Set-Item Function:Get-CodexDevInspectionProcesses $savedProcesses
    function Get-CimInstance {
        param($ClassName, $Filter, $Property, $ErrorAction)
        Assert-InspectionEqual $false ($Property -contains 'CommandLine') 'Process observation must not request command lines.'
        @(
            [pscustomobject]@{ ProcessId = 100; ParentProcessId = 1; ExecutablePath = (Join-Path $desktop 'app\ChatGPT.exe'); CreationDate = 'fixture' },
            [pscustomobject]@{ ProcessId = 101; ParentProcessId = 100; ExecutablePath = $exe; CreationDate = 'fixture' },
            [pscustomobject]@{ ProcessId = 200; ParentProcessId = 1; ExecutablePath = $exe; CreationDate = 'unrelated' }
        )
    }
    $running = Get-CodexDevInspectionProcesses (Get-CodexDevDesktopIdentity -DesktopInstallLocation $desktop)
    Assert-InspectionEqual 1 @($running.desktop).Count 'Desktop process must match manifest identity.'
    Assert-InspectionEqual 1 @($running.direct_cli_children).Count 'Unrelated same-name process must not become a Desktop child.'
    Assert-InspectionEqual 101 $running.direct_cli_children[0].pid 'Observed loaded child must be distinct from the selector.'
    Remove-Item Function:Get-CimInstance
    Write-InspectionFixture (Join-Path $desktop 'app\resources\app.asar') 'not executable fixture'
    Write-InspectionFixture $exe 'this is NOT an executable; inspection must not try running it'
    Write-InspectionFixture (Join-Path $package 'bin\codex-code-mode-host.exe') 'fixture code mode host'
    Write-InspectionFixture (Join-Path $package 'codex-package.json') '{"variant":"codex","version":"fixture"}'
    Write-InspectionFixture (Join-Path $package 'codex-dev-build.json') '{"private_unexported":"PROVENANCE-SECRET"}'
    Write-InspectionFixture $config ("[mcp_servers.node_repl.env]`nCODEX_CLI_PATH = '$exe'`n[credentials]`ntoken = 'CONFIG-SECRET'")
    Write-InspectionFixture (Join-Path $deploy 'state.json') (@{ Current = @{ Entrypoint = $exe; Fingerprint = 'ledger-claim' }; Previous = $null; Secret = 'LEDGER-SECRET' } | ConvertTo-Json)
    Write-InspectionFixture (Join-Path $deploy 'pending-transaction.json') '{"secret":"JOURNAL-SECRET"}'
    function Get-CodexDevInspectionSelectors {
        [pscustomobject]@{ user = [pscustomobject]@{ scope = 'User'; status = 'observed'; value = $exe }; process = [pscustomobject]@{ scope = 'test inherited'; value = 'other-reader.exe' } }
    }
    function Get-CodexDevInspectionProcesses {
        param($DesktopIdentity)
        [pscustomobject]@{ status = 'synthetic'; desktop = @(); direct_cli_children = @() }
    }
    $before = Get-InspectionFixtureTree $fixture
    $environmentBefore = [Environment]::GetEnvironmentVariable('CODEX_CLI_PATH', 'User')
    $observed = Get-CodexDevEnvironmentInspection -ConfigPath $config -DeploymentRoot $deploy -DesktopInstallLocation $desktop
    $json = $observed | ConvertTo-Json -Depth 12
    Assert-InspectionEqual 'no_change_observed' $observed.snapshot_status 'Stable fixture must be reported as observed, not compatible.'
    Assert-InspectionEqual 'explicit' $observed.desktop.source 'Fixture identity must not impersonate Appx discovery.'
    Assert-InspectionEqual '1.2.3.4' $observed.desktop.package_version 'Explicit manifest identity must be read.'
    Assert-InspectionEqual $exe $observed.config_mirror.value 'Section-bound mirror must be shown.'
    Assert-InspectionEqual 'other-reader.exe' $observed.selectors.process.value 'Inherited selector must remain distinct.'
    Assert-InspectionEqual 'observed' $observed.pending_journal.status 'Pending journal presence must be visible.'
    Assert-InspectionEqual 0 @($observed.mutations).Count 'Inspector must not claim mutation.'
    foreach ($secret in @('CONFIG-SECRET', 'LEDGER-SECRET', 'JOURNAL-SECRET', 'PROVENANCE-SECRET')) {
        Assert-InspectionEqual $false $json.Contains($secret) 'Secret-like fixture fields must not escape.'
    }
    Assert-InspectionEqual $before (Get-InspectionFixtureTree $fixture) 'Inspection must not write fixture bytes or lock files.'
    Assert-InspectionEqual $environmentBefore ([Environment]::GetEnvironmentVariable('CODEX_CLI_PATH', 'User')) 'Inspection must not change User selector.'
    Assert-InspectionEqual 1 @($observed.managed_packages).Count 'Managed selected package must be bounded and hashed.'
    $fingerprint = $observed.managed_packages[0].fingerprint
    # Independent SHA256 of ordinal relative-path/hash rows; hyphen and dot
    # have different culture order on Windows PowerShell 5.1 and PowerShell 7.
    Assert-InspectionEqual '8d18903df826a8765ed7c18cbfa5e787e79374fc54927c977fd913079ad9e5ec' $fingerprint 'Package fingerprint must be shell/culture independent.'
    Assert-InspectionEqual 'bin/codex-code-mode-host.exe' $observed.managed_packages[0].files[0].path 'Ordinal order must place the hyphenated companion before codex.exe.'
    Write-InspectionFixture (Join-Path $package 'codex-dev-build.json') '{"different":"provenance only"}'
    Assert-InspectionEqual $fingerprint (Get-CodexDevInspectionPackage $package).fingerprint 'Native provenance exclusion must match package fingerprint definition.'
    Write-InspectionFixture $config "[unrelated]`nCODEX_CLI_PATH = 'wrong-reader.exe'"
    $wrong = Get-CodexDevEnvironmentInspection -ConfigPath $config -DeploymentRoot $deploy -DesktopInstallLocation $desktop
    Assert-InspectionEqual 'missing' $wrong.config_mirror.status 'Wrong-section selector cannot become the mirror.'
    Write-InspectionFixture $config "[mcp_servers.node_repl.env]`nCODEX_CLI_PATH = 5"
    Assert-InspectionEqual 'unreadable_or_unsupported' (Get-CodexDevEnvironmentInspection -ConfigPath $config -DeploymentRoot $deploy -DesktopInstallLocation $desktop).config_mirror.status 'Malformed selector must be visible.'
    Write-InspectionFixture (Join-Path $deploy 'state.json') 'invalid json LEDGER-SECRET'
    Assert-InspectionEqual 'unreadable_or_unsupported' (Get-CodexDevEnvironmentInspection -ConfigPath $config -DeploymentRoot $deploy -DesktopInstallLocation $desktop).ledger.status 'Bad ledger must not look healthy.'
    function Get-CodexDevInspectionAppxPackages { [pscustomobject]@{ source = 'synthetic-appx'; packages = @() } }
    Assert-InspectionEqual 'missing' (Get-CodexDevDesktopIdentity).status 'No installed package is not an identity.'
    function Get-CodexDevInspectionAppxPackages { [pscustomobject]@{ source = 'synthetic-appx'; packages = @(1,2) } }
    Assert-InspectionEqual 'ambiguous' (Get-CodexDevDesktopIdentity).status 'Several Appx packages must not be guessed.'
    $script:SelectorReads = 0
    function Get-CodexDevInspectionSelectors {
        $script:SelectorReads++
        [pscustomobject]@{ user = [pscustomobject]@{ status = 'observed'; value = if ($script:SelectorReads -eq 1) { $exe } else { 'changed.exe' } }; process = [pscustomobject]@{ value = $null } }
    }
    Assert-InspectionEqual 'unstable' (Get-CodexDevEnvironmentInspection -ConfigPath $config -DeploymentRoot $deploy -DesktopInstallLocation $desktop).snapshot_status 'Changed selector must mark the whole snapshot unstable.'
    function Get-CodexDevInspectionSelectors { [pscustomobject]@{ user = [pscustomobject]@{ status = 'absent'; value = $null }; process = [pscustomobject]@{ value = $null } } }
    $missingRoot = Join-Path $fixture 'does-not-exist'
    $missing = Get-CodexDevEnvironmentInspection -ConfigPath (Join-Path $missingRoot 'config.toml') -DeploymentRoot $missingRoot -DesktopInstallLocation $desktop
    Assert-InspectionEqual 'missing' $missing.files_before[0].status 'Missing config must remain missing.'
    Assert-InspectionEqual $false (Test-Path -LiteralPath $missingRoot) 'Missing deployment root must not be created.'
    Write-InspectionFixture (Join-Path $desktop 'AppxManifest.xml') '<!DOCTYPE Package [<!ENTITY probe SYSTEM "file:///secret">]><Package>&probe;</Package>'
    Assert-InspectionEqual 'unavailable' (Get-CodexDevDesktopIdentity -DesktopInstallLocation $desktop).status 'External XML entities must be refused.'
    Write-InspectionFixture (Join-Path $desktop 'AppxManifest.xml') $manifest
    $kit = Join-Path $fixture 'recovery-kit'
    $args = @{ Destination = $kit; ApprovedPackagePath = $package; ExpectedPackageFingerprint = $fingerprint; ExpectedExecutableSha256 = (Get-CodexDevInspectionFile $exe).sha256; ConfigPath = $config; DeploymentRoot = $deploy; EvidencePath = @(Join-Path $deploy 'state.json') }
    $created = & (Join-Path $PSScriptRoot 'new-recovery-kit.ps1') @args
    Assert-InspectionEqual 'created' $created.status 'Kit must be built from exact approved bytes.'
    Assert-InspectionEqual 7 @(Get-ChildItem -LiteralPath $kit -File).Count 'Kit must copy only allowlisted modules plus manifest/launcher.'
    Assert-InspectionEqual 0 @(Get-ChildItem -LiteralPath $kit -Recurse -Filter '*.exe').Count 'Kit must not copy binaries.'
    Assert-InspectionEqual $false ((Get-ChildItem -LiteralPath $kit -File | Get-Content -Raw) -join "`n").Contains('LEDGER-SECRET') 'Evidence bodies must not be copied.'
    Assert-InspectionThrows { & (Join-Path $PSScriptRoot 'new-recovery-kit.ps1') @args } 'An existing kit must never be overwritten.'
    $args.Destination = Join-Path $fixture 'bad-kit'; $args.ExpectedExecutableSha256 = '0' * 64
    Assert-InspectionThrows { & (Join-Path $PSScriptRoot 'new-recovery-kit.ps1') @args } 'Hash mismatch must refuse build.'
    Assert-InspectionEqual $false (Test-Path -LiteralPath $args.Destination) 'Rejected build must not create output.'
    $manual = & (Join-Path $kit 'recovery-start.ps1') -ShowManualLaunch
    Assert-InspectionEqual $true (($manual -join "`n").Contains($exe)) 'Manual route must print the exact path without running the fake executable.'
    Write-InspectionFixture (Join-Path $package 'codex-package.json') '{"changed":"non-executable bytes"}'
    Assert-InspectionThrows { & (Join-Path $kit 'recovery-start.ps1') -ShowManualLaunch } 'Changed companion package bytes must refuse manual command.'
    Write-InspectionFixture (Join-Path $package 'codex-package.json') '{"variant":"codex","version":"fixture"}'
    Write-InspectionFixture $exe 'changed package bytes'
    Assert-InspectionThrows { & (Join-Path $kit 'recovery-start.ps1') -ShowManualLaunch } 'Changed executable must refuse manual command.'
    [pscustomobject]@{ status = 'passed'; checks = $script:Checks; powershell = $PSVersionTable.PSVersion.ToString(); limits = 'Synthetic file/selector cases; no live startup, resume, settings mutation or terminal fallback exercised.' } | ConvertTo-Json -Compress
} finally {
    Set-Item Function:Get-CodexDevInspectionProcesses $savedProcesses
    Set-Item Function:Get-CodexDevInspectionSelectors $savedSelectors
    Set-Item Function:Get-CodexDevInspectionAppxPackages $savedAppx
    $resolved = [IO.Path]::GetFullPath($fixture)
    $temp = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd([char[]]'\/') + '\'
    if ($resolved.StartsWith($temp, [StringComparison]::OrdinalIgnoreCase) -and
        (Split-Path -Leaf $resolved) -match '^codex-inspection-tests-[a-f0-9]{32}$' -and (Test-Path -LiteralPath $resolved)) {
        Remove-Item -LiteralPath $resolved -Recurse -ErrorAction Stop
    }
}
