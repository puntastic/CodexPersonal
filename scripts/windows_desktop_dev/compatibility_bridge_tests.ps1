Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'inspection.ps1')
. (Join-Path $PSScriptRoot 'compatibility.ps1')

$script:Checks = 0
function Assert-BridgeEqual {
    param($Expected, $Actual, [string]$Label)
    $script:Checks++
    if ($Expected -cne $Actual) { throw "$Label failed: expected <$Expected>, actual <$Actual>." }
}
function Assert-BridgeRefused {
    param([scriptblock]$Action, [string]$Pattern)
    $script:Checks++
    try { $null = & $Action } catch {
        if ($_.Exception.Message -match $Pattern) { return }
        throw "Unexpected refusal: $($_.Exception.Message)"
    }
    throw "Expected refusal matching $Pattern."
}

$testRoot = Join-Path ([IO.Path]::GetTempPath()) ('codex bridge tests-' + [guid]::NewGuid().ToString('N'))
$null = [IO.Directory]::CreateDirectory($testRoot)
$script:FixtureRoot = $testRoot
$script:FakeFingerprint = 'a' * 64
$script:FakeVersion = '0.999.0'
$script:DesktopVersion = '26.999.0.0'
$script:DesktopStatus = 'observed'
$script:Mode = 'qualified'
$script:PackageCalls = 0
$script:ProbeCalls = 0
$script:SourceDrift = $false
$script:PolicyUnavailable = $false
$script:RealProcess = (Get-Command Invoke-CodexDevCompatibilityProcess).ScriptBlock
$script:RealHash = (Get-Command Get-CodexDevCompatibilityFileHash).ScriptBlock
$script:RealPolicyState = (Get-Command Get-CodexDevMachinePolicyPathState).ScriptBlock
$envBefore = @{}
foreach ($name in @('CODEX_HOME', 'CODEX_SQLITE_HOME', 'CODEX_CLI_PATH', 'PATH', 'TEMP')) {
    $envBefore[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}

function Get-CodexDevPackageInfo {
    param([string]$PackageDirectory, [switch]$SkipSmoke)
    if (-not $SkipSmoke) { throw 'Bridge tried to run a package smoke check.' }
    $script:PackageCalls++
    return [pscustomobject]@{
        PackageRoot = [IO.Path]::GetFullPath($PackageDirectory)
        Entrypoint = Join-Path $PackageDirectory 'codex.exe'
        Fingerprint = $script:FakeFingerprint; Version = $script:FakeVersion
    }
}
function Get-CodexDevDesktopIdentity {
    param([string]$DesktopInstallLocation)
    $source = if ([string]::IsNullOrWhiteSpace($DesktopInstallLocation)) { 'appx' } else { 'explicit' }
    return [pscustomobject]@{
        status = $script:DesktopStatus; source = $source; package_full_name = 'OpenAI.Codex_fixture'
        package_version = $script:DesktopVersion; install_location = Join-Path $script:FixtureRoot 'desktop'
        app_asar = Get-CodexDevInspectionFile (Join-Path $script:FixtureRoot 'app.asar')
        manifest = Get-CodexDevInspectionFile (Join-Path $script:FixtureRoot 'AppxManifest.xml')
    }
}
function Get-CodexDevCompatibilityFileHash {
    param([string]$Path)
    if ($script:SourceDrift -and [IO.Path]::GetFileName($Path) -ceq 'compatibility_probe.py') { return 'f' * 64 }
    return & $script:RealHash -Path $Path
}
function Initialize-CodexDevEnvironment { throw 'Forbidden initialization.' }
function Invoke-CodexDevDoctor { throw 'Forbidden Doctor call.' }
function Invoke-CodexDevGit { throw 'Forbidden Git call.' }
function Get-CodexDevMachinePolicyDirectory { return Join-Path $script:FixtureRoot 'machine-policy' }
function Get-CodexDevMachinePolicyPathState {
    param([string]$Path)
    if ($script:PolicyUnavailable) { return 'unavailable' }
    return & $script:RealPolicyState -Path $Path
}
function Invoke-CodexDevCompatibilityProcess {
    param([string]$FilePath, [string[]]$ArgumentList, [string]$WorkingDirectory)
    $script:ProbeCalls++
    Assert-BridgeEqual '-I' $ArgumentList[0] 'Isolated Python startup'
    Assert-BridgeEqual '-B' $ArgumentList[1] 'No Python bytecode write'
    Assert-BridgeEqual '-S' $ArgumentList[2] 'No Python site initialization'
    Assert-BridgeEqual 'compatibility_probe.py' ([IO.Path]::GetFileName($ArgumentList[3])) 'Fixed probe source'
    Assert-BridgeEqual '--request' $ArgumentList[4] 'Literal request argument'
    Assert-BridgeEqual '--registry' $ArgumentList[6] 'Literal registry argument'
    Assert-BridgeEqual '--scratch-root' $ArgumentList[8] 'Literal scratch argument'
    Assert-BridgeEqual $WorkingDirectory $ArgumentList[9] 'Owned scratch working directory'
    $request = [IO.File]::ReadAllText($ArgumentList[5]) | ConvertFrom-Json
    Assert-BridgeEqual 'paginated_refs_v1' $request.required_history_formats[0] 'History requirement preserved'
    $result = [ordered]@{
        schema_version = 1; status = 'qualified'; compatible = $true; bindings = $request
        registry_sha256 = Get-CodexDevCompatibilityFileHash $ArgumentList[7]
        absent_machine_policy_paths = @(
            (Join-Path (Get-CodexDevMachinePolicyDirectory) 'config.toml'),
            (Join-Path (Get-CodexDevMachinePolicyDirectory) 'requirements.toml')
        )
        blockers = @(); checks = @([ordered]@{ name = 'mock startup'; passed = $true })
    }
    $exitCode = 0
    $stopped = $null
    switch ($script:Mode) {
        'unqualified' { $result.status = 'unqualified'; $result.compatible = $false; $result.blockers = @('Unknown synthetic identity'); $exitCode = 1 }
        'exit_mismatch' { $exitCode = 1 }
        'false_zero' { $result.status = 'failed'; $result.compatible = $false }
        'bool_string' { $result.compatible = 'true' }
        'schema_string' { $result.schema_version = '1' }
        'wrong_binding' { $result.bindings.candidate.version = 'wrong' }
        'changed_package' { $script:FakeFingerprint = 'b' * 64 }
        'timeout' { $stopped = 'timeout' }
        'report_write_failure' { $null = [IO.Directory]::CreateDirectory((Join-Path $WorkingDirectory 'compatibility-report.json')) }
        'wrong_machine_policy' { $result.absent_machine_policy_paths[0] = Join-Path $script:FixtureRoot 'different-config.toml' }
    }
    return [pscustomobject]@{
        ExitCode = $exitCode; Stdout = ($result | ConvertTo-Json -Depth 16 -Compress)
        Stderr = ''; StoppedFor = $stopped; StreamsComplete = $true
    }
}

try {
    foreach ($name in @('codex.exe', 'python.exe', 'app.asar', 'AppxManifest.xml')) {
        [IO.File]::WriteAllText((Join-Path $testRoot $name), "fixture-$name", [Text.UTF8Encoding]::new($false))
    }
    $python = Join-Path $testRoot 'python.exe'
    $scratch = Join-Path $testRoot 'scratch'
    $invoke = @{ PackageDirectory = $testRoot; PythonPath = $python; ScratchRoot = $scratch }
    $check = Invoke-CodexDevCompatibilityCheck @invoke
    if (-not $check.Compatible) { throw "Synthetic qualification failed: $($check.Blockers -join '; ')" }
    Assert-BridgeEqual 'qualified' $check.Status 'Qualified return'
    Assert-BridgeEqual $true $check.Compatible 'Compatible return'
    Assert-BridgeEqual 2 $script:PackageCalls 'Package checked before and after'
    Assert-BridgeEqual $true (Test-Path -LiteralPath $check.ReportPath) 'Retained report'
    Assert-BridgeEqual $true (Assert-CodexDevCompatibilityBinding -Check $check -PackageDirectory $testRoot) 'Fresh binding accepted'
    $policyDirectory = Get-CodexDevMachinePolicyDirectory
    $null = [IO.Directory]::CreateDirectory($policyDirectory)
    foreach ($name in @('config.toml', 'requirements.toml')) {
        $policy = Join-Path $policyDirectory $name
        [IO.File]::WriteAllText($policy, 'synthetic policy contents must not be read')
        Assert-BridgeRefused { Assert-CodexDevCompatibilityBinding -Check $check -PackageDirectory $testRoot } 'policy is present or unreadable'
        Remove-Item -LiteralPath $policy
    }
    $script:PolicyUnavailable = $true
    Assert-BridgeRefused { Assert-CodexDevCompatibilityBinding -Check $check -PackageDirectory $testRoot } 'policy is present or unreadable'
    $script:PolicyUnavailable = $false
    Assert-BridgeEqual $true (Assert-CodexDevCompatibilityBinding -Check $check -PackageDirectory $testRoot) 'Absence re-established without policy content reads'
    $notDirectory = Join-Path $testRoot 'policy-parent-file'
    [IO.File]::WriteAllText($notDirectory, 'fixture')
    Assert-BridgeEqual 'unavailable' (Get-CodexDevMachinePolicyPathState (Join-Path $notDirectory 'config.toml')) 'Non-directory policy ancestor is not absence'

    $copy = Join-Path $testRoot 'staged package'
    $null = [IO.Directory]::CreateDirectory($copy)
    [IO.File]::Copy((Join-Path $testRoot 'codex.exe'), (Join-Path $copy 'codex.exe'))
    Assert-BridgeEqual $true (Assert-CodexDevCompatibilityBinding -Check $check -PackageDirectory $copy) 'Identical staged package accepted'
    [IO.File]::WriteAllText((Join-Path $copy 'codex.exe'), 'changed binary')
    Assert-BridgeRefused { Assert-CodexDevCompatibilityBinding -Check $check -PackageDirectory $copy } 'ExecutableSha256'

    $script:FakeFingerprint = 'c' * 64
    Assert-BridgeRefused { Assert-CodexDevCompatibilityBinding -Check $check -PackageDirectory $testRoot } 'PackageFingerprint'
    $script:FakeFingerprint = 'a' * 64
    $script:DesktopVersion = '26.999.1.0'
    Assert-BridgeRefused { Assert-CodexDevCompatibilityBinding -Check $check -PackageDirectory $testRoot } 'DesktopVersion'
    $script:DesktopVersion = '26.999.0.0'
    $script:SourceDrift = $true
    Assert-BridgeRefused { Assert-CodexDevCompatibilityBinding -Check $check -PackageDirectory $testRoot } 'ProbeSha256'
    $script:SourceDrift = $false

    $altered = ($check | ConvertTo-Json -Depth 24) | ConvertFrom-Json
    $altered.Bindings.PackageVersion = 'unrecorded'
    Assert-BridgeRefused { Assert-CodexDevCompatibilityBinding -Check $altered -PackageDirectory $testRoot } 'differs from its retained receipt'
    [IO.File]::AppendAllText($check.RequestPath, ' ')
    Assert-BridgeRefused { Assert-CodexDevCompatibilityBinding -Check $check -PackageDirectory $testRoot } 'receipt/request bytes changed'

    $explicit = Invoke-CodexDevCompatibilityCheck @invoke -DesktopInstallLocation (Join-Path $testRoot 'desktop')
    Assert-BridgeEqual $true $explicit.Compatible 'Explicit location permits diagnostic check'
    Assert-BridgeRefused { Assert-CodexDevCompatibilityBinding -Check $explicit -PackageDirectory $testRoot } 'diagnostic only'
    foreach ($mode in @('exit_mismatch', 'false_zero', 'bool_string', 'schema_string', 'wrong_binding', 'timeout', 'changed_package', 'report_write_failure', 'wrong_machine_policy')) {
        $script:Mode = $mode
        $failed = Invoke-CodexDevCompatibilityCheck @invoke
        Assert-BridgeEqual $false $failed.Compatible "$mode cannot qualify"
        Assert-BridgeEqual 'failed' $failed.Status "$mode returns typed failure"
        $script:FakeFingerprint = 'a' * 64
    }
    $script:Mode = 'unqualified'
    $unknown = Invoke-CodexDevCompatibilityCheck @invoke
    Assert-BridgeEqual $false $unknown.Compatible 'Unknown reader refused'
    Assert-BridgeEqual 'unqualified' $unknown.Status 'Unknown reader remains unqualified'
    $script:Mode = 'qualified'
    $script:DesktopStatus = 'ambiguous'
    $beforeCalls = $script:ProbeCalls
    $missing = Invoke-CodexDevCompatibilityCheck @invoke
    Assert-BridgeEqual 'failed' $missing.Status 'Ambiguous Desktop refused'
    Assert-BridgeEqual $beforeCalls $script:ProbeCalls 'Ambiguous Desktop never launches'
    $script:DesktopStatus = 'observed'
    $missingPython = Invoke-CodexDevCompatibilityCheck -PackageDirectory $testRoot -PythonPath (Join-Path $testRoot 'absent.exe') -ScratchRoot $scratch
    Assert-BridgeEqual $false $missingPython.Compatible 'Unavailable explicit Python refused without install'

    # Exercise the real bounded runner, not the mock, with an inert PowerShell
    # fixture. No candidate binary or live Codex config/session is executed.
    $argumentScript = Join-Path $testRoot 'argv fixture.ps1'
    [IO.File]::WriteAllText($argumentScript, 'param([string]$Value) [Console]::Write($Value)', [Text.UTF8Encoding]::new($false))
    $shell = Join-Path $env:WINDIR 'System32\WindowsPowerShell\v1.0\powershell.exe'
    $literal = 'a path\with "quoted" parts & symbols\'
    $capture = & $script:RealProcess -FilePath $shell -WorkingDirectory $testRoot -ArgumentList @('-NoLogo', '-NoProfile', '-NonInteractive', '-File', $argumentScript, $literal)
    Assert-BridgeEqual $literal $capture.Stdout 'Literal Windows argv round trip'
    Assert-BridgeEqual $true $capture.StreamsComplete 'Real native streams complete'
    $capture = & $script:RealProcess -FilePath $shell -WorkingDirectory $testRoot -MaximumCharacters 100 -ArgumentList @('-NoLogo', '-NoProfile', '-NonInteractive', '-Command', "[Console]::Write(('x' * 10000))")
    Assert-BridgeEqual 'output_limit' $capture.StoppedFor 'Real bounded output limit'
    $capture = & $script:RealProcess -FilePath $shell -WorkingDirectory $testRoot -TimeoutSeconds 1 -ArgumentList @('-NoLogo', '-NoProfile', '-NonInteractive', '-Command', 'Start-Sleep -Seconds 5')
    Assert-BridgeEqual 'timeout' $capture.StoppedFor 'Real bounded process lifetime'
    foreach ($name in $envBefore.Keys) {
        Assert-BridgeEqual $envBefore[$name] ([Environment]::GetEnvironmentVariable($name, 'Process')) "Environment preserved: $name"
    }
} finally {
    # Every entry is generated under this exact unique directory. Verify the
    # resolved absolute root before removing only the synthetic test fixture.
    $resolved = [IO.Path]::GetFullPath($testRoot)
    $parent = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd([char[]]'\/') + [IO.Path]::DirectorySeparatorChar
    if (-not $resolved.StartsWith($parent, [StringComparison]::OrdinalIgnoreCase) -or
        [IO.Path]::GetFileName($resolved) -notmatch '^codex bridge tests-[0-9a-f]{32}$') {
        throw 'Test cleanup root did not match the owned temporary directory.'
    }
    Remove-Item -LiteralPath $resolved -Recurse
}
Write-Output "compatibility_bridge_tests: $script:Checks checks passed (PowerShell $($PSVersionTable.PSVersion))."
