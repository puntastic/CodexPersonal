# Package qualification bridge. Dot-sourcing this file has no external effect.
# Existing package/inspection helpers must already be loaded by the caller.
$script:CodexDevCompatibilityBridgePath = $PSCommandPath

function ConvertTo-CodexDevCompatibilityArgument {
    param([AllowEmptyString()][string]$Value)
    # Windows CRT quoting for ProcessStartInfo.Arguments on .NET Framework.
    # Never pass these arguments through cmd.exe or another shell.
    $quoted = [Text.StringBuilder]::new('"')
    $slashes = 0
    foreach ($character in $Value.ToCharArray()) {
        if ($character -eq '\') { $slashes++; continue }
        if ($character -eq '"') {
            [void]$quoted.Append(('\' * (2 * $slashes + 1)))
        } else { [void]$quoted.Append(('\' * $slashes)) }
        [void]$quoted.Append($character)
        $slashes = 0
    }
    [void]$quoted.Append(('\' * (2 * $slashes)))
    [void]$quoted.Append('"')
    return $quoted.ToString()
}

function Invoke-CodexDevCompatibilityProcess {
    param(
        [string]$FilePath, [string[]]$ArgumentList, [string]$WorkingDirectory,
        [int]$TimeoutSeconds = 120, [int]$MaximumCharacters = 262144
    )
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $FilePath
    $start.WorkingDirectory = $WorkingDirectory
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardInput = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.StandardOutputEncoding = [Text.UTF8Encoding]::new($false, $true)
    $start.StandardErrorEncoding = [Text.UTF8Encoding]::new($false, $true)
    if ($null -ne $start.PSObject.Properties['ArgumentList']) {
        foreach ($argument in $ArgumentList) { $start.ArgumentList.Add($argument) }
    } else {
        $start.Arguments = ($ArgumentList | ForEach-Object { ConvertTo-CodexDevCompatibilityArgument $_ }) -join ' '
    }
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $start
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $stoppedFor = $null
    $complete = $false
    try {
        if (-not $process.Start()) { throw 'Compatibility probe process did not start.' }
        $process.StandardInput.Close()
        $streams = @($process.StandardOutput, $process.StandardError)
        $buffers = @([char[]]::new(4096), [char[]]::new(4096))
        $texts = @([Text.StringBuilder]::new(), [Text.StringBuilder]::new())
        $tasks = @($streams[0].ReadAsync($buffers[0], 0, 4096), $streams[1].ReadAsync($buffers[1], 0, 4096))
        $done = @($false, $false)
        $exitedAt = $null
        while ($true) {
            for ($index = 0; $index -lt 2; $index++) {
                if (-not $done[$index] -and $tasks[$index].IsCompleted) {
                    $count = $tasks[$index].GetAwaiter().GetResult()
                    if ($texts[$index].Length + $count -gt $MaximumCharacters) {
                        $stoppedFor = 'output_limit'
                        break
                    }
                    if ($count -eq 0) { $done[$index] = $true }
                    else {
                        [void]$texts[$index].Append($buffers[$index], 0, $count)
                        $tasks[$index] = $streams[$index].ReadAsync($buffers[$index], 0, 4096)
                    }
                }
            }
            if ($null -ne $stoppedFor) { break }
            if ($process.HasExited) {
                if ($done[0] -and $done[1]) { $complete = $true; break }
                if ($null -eq $exitedAt) { $exitedAt = $timer.Elapsed.TotalSeconds }
                if ($timer.Elapsed.TotalSeconds - $exitedAt -gt 2) { $stoppedFor = 'streams_incomplete'; break }
            }
            if ($timer.Elapsed.TotalSeconds -ge $TimeoutSeconds) { $stoppedFor = 'timeout'; break }
            [Threading.Thread]::Sleep(10)
        }
        if (-not $process.HasExited) {
            # Only the exact fixture process this bridge created. This does not
            # claim cleanup of arbitrary descendants on Windows PowerShell 5.1.
            $process.Kill()
            if (-not $process.WaitForExit(2000)) { throw 'Compatibility probe root process did not stop.' }
        }
        return [pscustomobject]@{
            ExitCode = $process.ExitCode; Stdout = $texts[0].ToString(); Stderr = $texts[1].ToString()
            StoppedFor = $stoppedFor; StreamsComplete = $complete
        }
    } finally {
        if ($null -ne $process -and $null -ne $process.StartInfo) {
            try { if (-not $process.HasExited) { $process.Kill(); [void]$process.WaitForExit(2000) } } catch { }
        }
        $timer.Stop()
        $process.Dispose()
    }
}

function Get-CodexDevCompatibilityFileHash {
    param([string]$Path)
    $file = Get-CodexDevInspectionFile -Path $Path
    if ($file.status -cne 'observed') { throw "Compatibility binding cannot read a regular file: $Path" }
    return [string]$file.sha256
}

function Get-CodexDevMachinePolicyDirectory {
    # Resolve the Windows known folder, never the overridable ProgramData env.
    $common = [Environment]::GetFolderPath([Environment+SpecialFolder]::CommonApplicationData)
    if ([string]::IsNullOrWhiteSpace($common) -or -not [IO.Path]::IsPathRooted($common)) {
        throw 'Machine policy location could not be established.'
    }
    return Join-Path $common 'OpenAI\Codex'
}

function Get-CodexDevMachinePolicyPathState {
    param([string]$Path)
    $probe = $Path
    $leaf = $true
    while (-not [string]::IsNullOrWhiteSpace($probe)) {
        try {
            $attributes = [IO.File]::GetAttributes($probe)
            if ($leaf) { return 'present' }
            if (($attributes -band [IO.FileAttributes]::Directory) -eq 0 -or
                ($attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { return 'unavailable' }
            return 'absent'
        } catch [IO.FileNotFoundException] { }
        catch [IO.DirectoryNotFoundException] { }
        catch { return 'unavailable' }
        $leaf = $false
        $probe = [IO.Path]::GetDirectoryName($probe)
    }
    return 'unavailable'
}

function Assert-CodexDevMachinePolicyAbsent {
    param([object[]]$RecordedPaths)
    $directory = Get-CodexDevMachinePolicyDirectory
    $expected = @((Join-Path $directory 'config.toml'), (Join-Path $directory 'requirements.toml'))
    if ($RecordedPaths.Count -ne 2) { throw 'Compatibility receipt lacks the two required machine-policy absence observations.' }
    for ($i = 0; $i -lt $expected.Count; $i++) {
        if ($RecordedPaths[$i] -isnot [string] -or -not [IO.Path]::IsPathRooted($RecordedPaths[$i]) -or
            -not [string]::Equals([IO.Path]::GetFullPath($RecordedPaths[$i]), [IO.Path]::GetFullPath($expected[$i]), [StringComparison]::OrdinalIgnoreCase)) {
            throw 'Compatibility receipt names a different machine policy location; run a new check.'
        }
        if ((Get-CodexDevMachinePolicyPathState $expected[$i]) -cne 'absent') {
            throw 'Machine-wide Codex configuration/policy is present or unreadable; isolated compatibility admission is unavailable. No policy contents were read.'
        }
    }
}

function Get-CodexDevCompatibilityBindings {
    param([string]$PackageDirectory, [string]$DesktopInstallLocation)
    $package = Get-CodexDevPackageInfo -PackageDirectory $PackageDirectory -SkipSmoke
    $desktop = Get-CodexDevDesktopIdentity -DesktopInstallLocation $DesktopInstallLocation
    if ($desktop.status -cne 'observed' -or $desktop.app_asar.status -cne 'observed' -or
        $desktop.manifest.status -cne 'observed' -or [string]::IsNullOrWhiteSpace($desktop.package_version)) {
        throw 'Current Desktop identity is missing, ambiguous or incomplete; no compatibility identity was invented.'
    }
    return [pscustomobject][ordered]@{
        PackageDirectory = [IO.Path]::GetFullPath($package.PackageRoot)
        PackageFingerprint = $package.Fingerprint
        Executable = [IO.Path]::GetFullPath($package.Entrypoint)
        ExecutableSha256 = Get-CodexDevCompatibilityFileHash $package.Entrypoint
        PackageVersion = $package.Version
        DesktopSource = $desktop.source
        DesktopPackageFullName = $desktop.package_full_name
        DesktopVersion = $desktop.package_version
        DesktopInstallLocation = $desktop.install_location
        DesktopArchivePath = $desktop.app_asar.path
        DesktopArchiveSha256 = $desktop.app_asar.sha256
        DesktopManifestSha256 = $desktop.manifest.sha256
        RegistrySha256 = Get-CodexDevCompatibilityFileHash (Join-Path $PSScriptRoot 'compatibility_profiles.json')
        ProbeSha256 = Get-CodexDevCompatibilityFileHash (Join-Path $PSScriptRoot 'compatibility_probe.py')
        BridgeSha256 = Get-CodexDevCompatibilityFileHash $script:CodexDevCompatibilityBridgePath
    }
}

function Assert-CodexDevCompatibilityFieldsEqual {
    param($Expected, $Actual, [switch]$AllowPackageRelocation)
    foreach ($property in $Actual.PSObject.Properties) {
        if ($AllowPackageRelocation -and $property.Name -cin @('PackageDirectory', 'Executable')) { continue }
        $previous = Get-CodexDevInspectionProperty -Value $Expected -Name $property.Name
        if ($previous -cne $property.Value) {
            throw "Compatibility binding changed: $($property.Name). This compatibility check did not select the candidate; inspect any completed recovery before checking again."
        }
    }
}

function New-CodexDevCompatibilityScratch {
    param([string]$ScratchRoot)
    if ([string]::IsNullOrWhiteSpace($ScratchRoot)) {
        $ScratchRoot = Join-Path ([IO.Path]::GetTempPath()) 'codex-desktop-compatibility'
    }
    $root = [IO.Path]::GetFullPath($ScratchRoot)
    $check = $root
    while (-not [string]::IsNullOrWhiteSpace($check)) {
        if (Test-Path -LiteralPath $check) {
            $item = Get-Item -LiteralPath $check -Force -ErrorAction Stop
            if (-not $item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw 'Compatibility scratch must be a regular directory path without reparse points.'
            }
        }
        $check = [IO.Path]::GetDirectoryName($check.TrimEnd([char[]]'\/'))
    }
    $null = [IO.Directory]::CreateDirectory($root)
    $run = Join-Path $root ('bridge-' + [guid]::NewGuid().ToString('N'))
    if (Test-Path -LiteralPath $run) { throw 'Compatibility scratch collision; no existing directory will be reused.' }
    $null = [IO.Directory]::CreateDirectory($run)
    return $run
}

function Invoke-CodexDevCompatibilityCheck {
    param(
        [Parameter(Mandatory = $true)][string]$PackageDirectory,
        [string]$PythonPath, [string]$DesktopInstallLocation, [string]$ScratchRoot
    )
    $check = [pscustomobject][ordered]@{
        SchemaVersion = 1; Status = 'unqualified'; Compatible = $false; Bindings = $null
        ReportPath = $null; ReportSha256 = $null; RequestPath = $null; RequestSha256 = $null
        Interpreter = $null; InterpreterSha256 = $null; Result = $null; Blockers = @()
    }
    try {
        $check.Bindings = Get-CodexDevCompatibilityBindings -PackageDirectory $PackageDirectory -DesktopInstallLocation $DesktopInstallLocation
        if ([string]::IsNullOrWhiteSpace($PythonPath)) {
            $python = @(Get-Command python.exe, python3.exe -CommandType Application -ErrorAction SilentlyContinue |
                Where-Object { $_.Source -notmatch '\\WindowsApps\\' } | Select-Object -First 1)
            if ($python.Count -eq 0) { throw 'Python is unavailable; provide -PythonPath. Nothing was installed.' }
            $PythonPath = $python[0].Source
        }
        $check.Interpreter = (Get-Item -LiteralPath $PythonPath -ErrorAction Stop).FullName
        $check.InterpreterSha256 = Get-CodexDevCompatibilityFileHash $check.Interpreter
        $run = New-CodexDevCompatibilityScratch -ScratchRoot $ScratchRoot
        $check.ReportPath = Join-Path $run 'compatibility-report.json'
        $check.RequestPath = Join-Path $run 'request.json'
        $binding = $check.Bindings
        $request = [ordered]@{
            schema_version = 1
            candidate = [ordered]@{
                executable = $binding.Executable; executable_sha256 = $binding.ExecutableSha256
                package_fingerprint = $binding.PackageFingerprint; version = $binding.PackageVersion
            }
            desktop = [ordered]@{
                package_version = $binding.DesktopVersion
                app_asar = [ordered]@{ path = $binding.DesktopArchivePath; sha256 = $binding.DesktopArchiveSha256 }
            }
            required_history_formats = @('paginated_refs_v1')
        }
        $requestJson = $request | ConvertTo-Json -Depth 12 -Compress
        if ([Text.Encoding]::UTF8.GetByteCount($requestJson) -gt 65536) {
            throw 'Compatibility request exceeds its supported size; no probe was started.'
        }
        [IO.File]::WriteAllText($check.RequestPath, $requestJson, [Text.UTF8Encoding]::new($false))
        $check.RequestSha256 = Get-CodexDevCompatibilityFileHash $check.RequestPath
        $observation = Invoke-CodexDevCompatibilityProcess -FilePath $check.Interpreter -WorkingDirectory $run -ArgumentList @(
            '-I', '-B', '-S', (Join-Path $PSScriptRoot 'compatibility_probe.py'),
            '--request', $check.RequestPath, '--registry', (Join-Path $PSScriptRoot 'compatibility_profiles.json'),
            '--scratch-root', $run
        )
        if ($null -ne $observation.StoppedFor -or -not $observation.StreamsComplete -or
            $observation.Stdout.Length -gt 262144 -or $observation.Stderr.Length -gt 262144) {
            throw 'Compatibility probe exceeded its output/time bound or did not close its streams; no successful check is available.'
        }
        $result = $observation.Stdout | ConvertFrom-Json -ErrorAction Stop
        if ($result -isnot [pscustomobject] -or
            ($result.schema_version -isnot [int] -and $result.schema_version -isnot [long]) -or
            $result.schema_version -ne 1 -or
            $result.compatible -isnot [bool] -or $result.status -cnotin @('qualified', 'unqualified', 'failed')) {
            throw 'Compatibility probe returned an unsupported result shape.'
        }
        if (($result.compatible -and ($result.status -cne 'qualified' -or $observation.ExitCode -ne 0)) -or
            (-not $result.compatible -and ($result.status -ceq 'qualified' -or $observation.ExitCode -ne 1))) {
            throw 'Compatibility probe exit code and result disagree.'
        }
        if ($result.compatible) {
            if (($result.bindings | ConvertTo-Json -Depth 12 -Compress) -cne $requestJson -or
                $result.registry_sha256 -cne $binding.RegistrySha256) {
                throw 'Compatibility probe response is not bound to this request and registry.'
            }
            $after = Get-CodexDevCompatibilityBindings -PackageDirectory $PackageDirectory -DesktopInstallLocation $DesktopInstallLocation
            Assert-CodexDevCompatibilityFieldsEqual -Expected $binding -Actual $after
            if ((Get-CodexDevCompatibilityFileHash $check.Interpreter) -cne $check.InterpreterSha256 -or
                (Get-CodexDevCompatibilityFileHash $check.RequestPath) -cne $check.RequestSha256) {
                throw 'Compatibility interpreter or request changed during the check.'
            }
            Assert-CodexDevMachinePolicyAbsent -RecordedPaths @($result.absent_machine_policy_paths)
        }
        $check.Result = $result
        $check.Status = $result.status
        $check.Compatible = $result.compatible
        $check.Blockers = @($result.blockers)
    } catch {
        $check.Status = 'failed'
        $check.Compatible = $false
        $check.Blockers = @($_.Exception.Message)
    }
    if ($null -ne $check.ReportPath) {
        try {
            $bytes = [Text.UTF8Encoding]::new($false).GetBytes(($check | ConvertTo-Json -Depth 24))
            if ($bytes.Length -gt 1048576) { throw 'Compatibility report exceeds its supported size.' }
            [IO.File]::WriteAllBytes($check.ReportPath, $bytes)
            $check.ReportSha256 = Get-CodexDevCompatibilityFileHash $check.ReportPath
        } catch {
            $check.Status = 'failed'
            $check.Compatible = $false
            $check.Blockers += 'Compatibility receipt could not be retained and hashed; no admission is available.'
        }
    }
    return $check
}

function Assert-CodexDevCompatibilityBinding {
    param([Parameter(Mandatory = $true)]$Check, [Parameter(Mandatory = $true)][string]$PackageDirectory)
    if ($Check.SchemaVersion -ne 1 -or $Check.Compatible -isnot [bool] -or -not $Check.Compatible -or
        $Check.Status -cne 'qualified' -or $null -eq $Check.Result -or -not $Check.Result.compatible) {
        throw 'A successful typed compatibility check is required before selection.'
    }
    if ($Check.Bindings.DesktopSource -cnotin @('appx', 'appx-windows-powershell')) {
        throw 'An explicit Desktop path is diagnostic only; deployment requires a check of the current registered Desktop.'
    }
    if ((Get-CodexDevCompatibilityFileHash $Check.ReportPath) -cne $Check.ReportSha256 -or
        (Get-CodexDevCompatibilityFileHash $Check.RequestPath) -cne $Check.RequestSha256) {
        throw 'Compatibility receipt/request bytes changed; run a new check.'
    }
    $recorded = (Read-CodexDevInspectionText -Path $Check.ReportPath -MaximumBytes 1048576) | ConvertFrom-Json
    $supplied = ($Check | ConvertTo-Json -Depth 24) | ConvertFrom-Json
    $supplied.ReportSha256 = $null
    if (($recorded | ConvertTo-Json -Depth 24 -Compress) -cne ($supplied | ConvertTo-Json -Depth 24 -Compress)) {
        throw 'Compatibility check object differs from its retained receipt; run a new check.'
    }
    $current = Get-CodexDevCompatibilityBindings -PackageDirectory $PackageDirectory
    if ($current.DesktopSource -cnotin @('appx', 'appx-windows-powershell')) {
        throw 'Current registered Desktop identity could not be established.'
    }
    Assert-CodexDevCompatibilityFieldsEqual -Expected $Check.Bindings -Actual $current -AllowPackageRelocation
    if ((Get-CodexDevCompatibilityFileHash $Check.Interpreter) -cne $Check.InterpreterSha256) {
        throw 'Compatibility interpreter changed; run a new check.'
    }
    Assert-CodexDevMachinePolicyAbsent -RecordedPaths @($Check.Result.absent_machine_policy_paths)
    return $true
}
