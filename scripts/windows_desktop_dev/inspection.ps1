# Read-only by construction: no deployment, lock, Doctor, toolchain or package execution.
# Keep this module loadable in Windows PowerShell 5.1, including outside the repository.
function Get-CodexDevInspectionFile {
    param([string]$Path)
    $result = [ordered]@{ path = $Path; status = 'missing'; sha256 = $null; bytes = $null }
    try {
        $item = Get-Item -LiteralPath $Path -ErrorAction Stop
        if ($item.PSIsContainer) { $result.status = 'not_file' }
        elseif (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { $result.status = 'reparse_point' }
        else {
            $result.sha256 = (Get-FileHash -LiteralPath $Path -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant()
            $result.bytes = $item.Length
            $result.status = 'observed'
        }
    } catch [System.Management.Automation.ItemNotFoundException] { }
    catch { $result.status = 'unavailable' }
    return [pscustomobject]$result
}

function Read-CodexDevInspectionText {
    param([string]$Path, [long]$MaximumBytes = 4194304)
    $item = Get-Item -LiteralPath $Path -ErrorAction Stop
    if ($item.PSIsContainer -or $item.Length -gt $MaximumBytes -or
        ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw 'Inspection refused a non-file, reparse point or oversized text input.'
    }
    return [System.Text.UTF8Encoding]::new($false, $true).GetString([IO.File]::ReadAllBytes($Path)).TrimStart([char]0xFEFF)
}

function Get-CodexDevInspectionProperty {
    param($Value, [string]$Name)
    if ($null -ne $Value -and $null -ne $Value.PSObject.Properties[$Name]) {
        return $Value.PSObject.Properties[$Name].Value
    }
    return $null
}

function Get-CodexDevInspectionAppxPackages {
    try {
        return [pscustomobject]@{ source = 'appx'; packages = @(Get-AppxPackage -Name OpenAI.Codex -ErrorAction Stop) }
    } catch {
        # Appx cannot load in some PowerShell 7 hosts. Use only the built-in reader,
        # with a fixed command and bounded wait; never execute a selected Codex binary.
        $reader = Join-Path $env:WINDIR 'System32\WindowsPowerShell\v1.0\powershell.exe'
        if (-not (Test-Path -LiteralPath $reader -PathType Leaf)) { throw 'Appx metadata reader unavailable.' }
        $start = New-Object System.Diagnostics.ProcessStartInfo
        $start.FileName = $reader
        $start.Arguments = '-NoLogo -NoProfile -NonInteractive -Command "$ErrorActionPreference=''Stop''; @(Get-AppxPackage -Name OpenAI.Codex | Select-Object PackageFullName,Version,InstallLocation) | ConvertTo-Json -Compress"'
        $start.UseShellExecute = $false
        $start.CreateNoWindow = $true
        $start.RedirectStandardOutput = $true
        $start.RedirectStandardError = $true
        $process = New-Object System.Diagnostics.Process
        $process.StartInfo = $start
        try {
            if (-not $process.Start()) { throw 'Appx metadata reader did not start.' }
            $stdout = $process.StandardOutput.ReadToEndAsync()
            $stderr = $process.StandardError.ReadToEndAsync()
            if (-not $process.WaitForExit(10000)) {
                $process.Kill()
                throw 'Appx metadata reader timed out.'
            }
            if ($process.ExitCode -ne 0) { throw 'Appx metadata reader failed.' }
            $text = $stdout.GetAwaiter().GetResult()
            if ($text.Length -gt 65536) { throw 'Appx metadata result exceeded its bound.' }
            $packages = if ([string]::IsNullOrWhiteSpace($text)) { @() } else { @($text | ConvertFrom-Json) }
            return [pscustomobject]@{ source = 'appx-windows-powershell'; packages = $packages }
        } finally { $process.Dispose() }
    }
}

function Get-CodexDevDesktopIdentity {
    param([string]$DesktopInstallLocation)
    $result = [ordered]@{
        status = 'unavailable'; source = 'appx'; package_full_name = $null
        package_version = $null; install_location = $null; desktop_executables = @()
        app_asar = $null; manifest = $null; notes = @()
    }
    try {
        if ([string]::IsNullOrWhiteSpace($DesktopInstallLocation)) {
            $read = Get-CodexDevInspectionAppxPackages
            $result.source = $read.source
            $packages = @($read.packages)
            if ($packages.Count -eq 0) { $result.status = 'missing'; return [pscustomobject]$result }
            if ($packages.Count -ne 1) { $result.status = 'ambiguous'; return [pscustomobject]$result }
            $result.package_full_name = [string]$packages[0].PackageFullName
            $result.package_version = [string]$packages[0].Version
            $DesktopInstallLocation = [string]$packages[0].InstallLocation
        } else { $result.source = 'explicit' }
        $root = [IO.Path]::GetFullPath($DesktopInstallLocation).TrimEnd([char[]]'\/')
        $result.install_location = $root
        $manifestPath = Join-Path $root 'AppxManifest.xml'
        $result.manifest = Get-CodexDevInspectionFile $manifestPath
        $settings = New-Object System.Xml.XmlReaderSettings
        $settings.DtdProcessing = [System.Xml.DtdProcessing]::Prohibit
        $settings.XmlResolver = $null
        $reader = [System.Xml.XmlReader]::Create([IO.StringReader]::new((Read-CodexDevInspectionText $manifestPath)), $settings)
        try { $xml = New-Object System.Xml.XmlDocument; $xml.XmlResolver = $null; $xml.Load($reader) }
        finally { $reader.Dispose() }
        $identity = $xml.SelectSingleNode('/*[local-name()="Package"]/*[local-name()="Identity"]')
        if ($null -eq $identity -or $identity.GetAttribute('Name') -ne 'OpenAI.Codex') { throw 'Unexpected package identity.' }
        if ($result.source -eq 'explicit') { $result.package_version = $identity.GetAttribute('Version') }
        elseif ($identity.GetAttribute('Version') -ne $result.package_version) { throw 'Appx and manifest version disagree.' }
        $result.desktop_executables = @($xml.SelectNodes('//*[local-name()="Application"]') | ForEach-Object {
            $relative = $_.GetAttribute('Executable')
            if (-not [string]::IsNullOrWhiteSpace($relative)) {
                $full = [IO.Path]::GetFullPath((Join-Path $root $relative))
                if (-not $full.StartsWith($root + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
                    throw 'Manifest executable escapes its package.'
                }
                $full
            }
        })
        $result.app_asar = Get-CodexDevInspectionFile (Join-Path $root 'app\resources\app.asar')
        $result.status = if ($result.app_asar.status -eq 'observed') { 'observed' } else { 'incomplete' }
    } catch { $result.notes += 'Desktop identity could not be established from the selected metadata; no fallback identity was invented.' }
    return [pscustomobject]$result
}

function Get-CodexDevInspectionPackage {
    param([string]$PackagePath)
    $root = [IO.Path]::GetFullPath($PackagePath).TrimEnd([char[]]'\/')
    $result = [ordered]@{ path = $root; status = 'unavailable'; fingerprint = $null; files = @(); native_provenance = $null }
    try {
        # Traverse explicitly so a junction is never followed and a large/unrelated
        # directory does not become an accidental recursive corpus scan.
        $queue = New-Object 'System.Collections.Generic.Queue[string]'
        $queue.Enqueue($root)
        $files = @(); $directories = 0
        while ($queue.Count -gt 0) {
            $directory = $queue.Dequeue(); $directories++
            $item = Get-Item -LiteralPath $directory -ErrorAction Stop
            if (-not $item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 -or $directories -gt 64) {
                throw 'Package directory is not a bounded regular directory tree.'
            }
            foreach ($item in @(Get-ChildItem -LiteralPath $directory -Force -ErrorAction Stop)) {
                if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw 'Package contains a reparse point.' }
                if ($item.PSIsContainer) { $queue.Enqueue($item.FullName) }
                else { $files += $item; if ($files.Count -gt 128) { throw 'Package file count exceeds inspection bound.' } }
            }
        }
        # Culture sorting differs between .NET Framework and modern .NET (notably
        # hyphen versus dot). Package identity must not depend on the shell host.
        $sortedPaths = [string[]]@($files | ForEach-Object { $_.FullName })
        [Array]::Sort($sortedPaths, [StringComparer]::Ordinal)
        $result.files = @($sortedPaths | ForEach-Object {
            $relative = $_.Substring($root.Length + 1).Replace('\', '/')
            if ($relative -ne 'codex-dev-build.json') {
                $observation = Get-CodexDevInspectionFile $_
                if ($observation.status -ne 'observed') { throw 'Package file unavailable.' }
                [pscustomobject]@{ path = $relative; sha256 = $observation.sha256; bytes = $observation.bytes }
            }
        })
        $material = ($result.files | ForEach-Object { "$($_.path)=$($_.sha256)" }) -join "`n"
        $result.fingerprint = Get-CodexDevSha256Bytes ([Text.Encoding]::UTF8.GetBytes($material))
        $result.native_provenance = Get-CodexDevInspectionFile (Join-Path $root 'codex-dev-build.json')
        $result.status = 'observed'
    } catch { $result.status = 'unavailable'; $result.fingerprint = $null }
    return [pscustomobject]$result
}

function Get-CodexDevInspectionSelectors {
    $user = [ordered]@{ scope = 'User'; status = 'unavailable'; value = $null }
    try {
        $user.value = [Environment]::GetEnvironmentVariable('CODEX_CLI_PATH', 'User')
        $user.status = if ([string]::IsNullOrWhiteSpace($user.value)) { 'absent' } else { 'observed' }
    } catch { }
    return [pscustomobject]@{
        user = [pscustomobject]$user
        process = [pscustomobject]@{
            scope = 'inspection-process inherited environment, not Desktop process memory'
            value = [Environment]::GetEnvironmentVariable('CODEX_CLI_PATH', 'Process')
        }
    }
}

function Get-CodexDevInspectionProcesses {
    param($DesktopIdentity)
    try {
        $rows = @(Get-CimInstance Win32_Process -Filter "Name='Codex.exe' OR Name='ChatGPT.exe'" -Property ProcessId,ParentProcessId,ExecutablePath,CreationDate -ErrorAction Stop)
        $desktop = @($rows | Where-Object { $DesktopIdentity.desktop_executables -contains $_.ExecutablePath })
        $children = @($rows | Where-Object { @($desktop | ForEach-Object { $_.ProcessId }) -contains $_.ParentProcessId -and
            $DesktopIdentity.desktop_executables -notcontains $_.ExecutablePath })
        return [pscustomobject]@{
            status = 'observed'; observed_at_utc = [DateTime]::UtcNow.ToString('o')
            desktop = @($desktop | ForEach-Object { [pscustomobject]@{ pid = $_.ProcessId; path = $_.ExecutablePath; created = $_.CreationDate } })
            direct_cli_children = @($children | ForEach-Object { [pscustomobject]@{
                pid = $_.ProcessId; parent_pid = $_.ParentProcessId; created = $_.CreationDate
                executable = Get-CodexDevInspectionFile $_.ExecutablePath
            } })
            limit = 'Direct named children of manifest-bound Desktop processes only; no command lines or session payloads read. An empty set is not proof of no other runtime.'
        }
    } catch { return [pscustomobject]@{ status = 'unavailable'; desktop = @(); direct_cli_children = @(); limit = 'Process observation unavailable; selectors do not prove a loaded binary.' } }
}

function Get-CodexDevEnvironmentInspection {
    param(
        [Parameter(Mandatory = $true)][string]$ConfigPath,
        [Parameter(Mandatory = $true)][string]$DeploymentRoot,
        [string]$DesktopInstallLocation
    )
    $config = [IO.Path]::GetFullPath($ConfigPath)
    $root = [IO.Path]::GetFullPath($DeploymentRoot).TrimEnd([char[]]'\/')
    $started = [DateTime]::UtcNow.ToString('o')
    $selectors = Get-CodexDevInspectionSelectors
    $paths = @($config, (Join-Path $root 'state.json'), (Join-Path $root 'pending-transaction.json'))
    $before = @($paths | ForEach-Object { Get-CodexDevInspectionFile $_ })
    $mirror = [ordered]@{ status = 'missing'; value = $null; section = 'mcp_servers.node_repl.env'; line = $null }
    if ($before[0].status -eq 'observed') {
        try {
            $binding = Get-CodexCliConfigBinding -Text (Read-CodexDevInspectionText $config) -Description 'selected config'
            $mirror.status = $binding.Status; $mirror.value = $binding.Value; $mirror.line = $binding.LineNumber
        } catch { $mirror.status = 'unreadable_or_unsupported'; }
    } elseif ($before[0].status -ne 'missing') { $mirror.status = $before[0].status }
    $ledger = [ordered]@{ status = $before[1].status; current_entrypoint = $null; previous_entrypoint = $null; claimed_fingerprint = $null }
    if ($before[1].status -eq 'observed') {
        try {
            $state = Read-CodexDevInspectionText $paths[1] 1048576 | ConvertFrom-Json
            $current = Get-CodexDevInspectionProperty $state 'Current'
            $previous = Get-CodexDevInspectionProperty $state 'Previous'
            $ledger.current_entrypoint = Get-CodexDevInspectionProperty $current 'Entrypoint'
            $ledger.previous_entrypoint = Get-CodexDevInspectionProperty $previous 'Entrypoint'
            $ledger.claimed_fingerprint = Get-CodexDevInspectionProperty $current 'Fingerprint'
        } catch { $ledger.status = 'unreadable_or_unsupported' }
    }
    $desktop = Get-CodexDevDesktopIdentity -DesktopInstallLocation $DesktopInstallLocation
    $executables = @(@($selectors.user.value, $selectors.process.value, $mirror.value, $ledger.current_entrypoint) |
        Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | Select-Object -Unique | ForEach-Object { Get-CodexDevInspectionFile $_ })
    $packages = @($executables | Where-Object {
        $_.path.StartsWith($root + '\releases\', [StringComparison]::OrdinalIgnoreCase) -and $_.path.EndsWith('\bin\codex.exe', [StringComparison]::OrdinalIgnoreCase)
    } | ForEach-Object { Get-CodexDevInspectionPackage (Split-Path -Parent (Split-Path -Parent $_.path)) })
    $processes = Get-CodexDevInspectionProcesses $desktop
    $after = @($paths | ForEach-Object { Get-CodexDevInspectionFile $_ })
    $endSelectors = Get-CodexDevInspectionSelectors
    $endDesktop = Get-CodexDevDesktopIdentity -DesktopInstallLocation $DesktopInstallLocation
    $changed = New-Object 'System.Collections.Generic.List[string]'
    if (($before | ConvertTo-Json -Depth 5 -Compress) -cne ($after | ConvertTo-Json -Depth 5 -Compress)) { $changed.Add('config, ledger or journal') }
    if (($selectors | ConvertTo-Json -Depth 5 -Compress) -cne ($endSelectors | ConvertTo-Json -Depth 5 -Compress)) { $changed.Add('selectors') }
    if (($desktop | ConvertTo-Json -Depth 5 -Compress) -cne ($endDesktop | ConvertTo-Json -Depth 5 -Compress)) { $changed.Add('Desktop identity') }
    foreach ($file in $executables) {
        if ($file.sha256 -ne (Get-CodexDevInspectionFile $file.path).sha256) { $changed.Add('executable bytes') }
    }
    foreach ($package in $packages) {
        if ($package.fingerprint -ne (Get-CodexDevInspectionPackage $package.path).fingerprint) { $changed.Add('package bytes') }
    }
    return [pscustomobject]@{
        schema_version = 1; action = 'inspect'; mutations = @(); started_at_utc = $started; finished_at_utc = [DateTime]::UtcNow.ToString('o')
        snapshot_status = if ($changed.Count -eq 0) { 'no_change_observed' } else { 'unstable' }
        changed_during_read = @($changed); files_before = $before; files_after = $after
        selectors = $selectors; config_mirror = [pscustomobject]$mirror; ledger = [pscustomobject]$ledger
        pending_journal = $before[2]; desktop = $desktop; executable_files = $executables; managed_packages = $packages; running = $processes
        limits = @('Read-only observations, not a compatibility verdict or lock-protected transaction.',
            'No binary was executed; no auth files, histories, session payloads or complete environment were inspected.',
            'Config mirror belongs to node_repl; it is not the Desktop User selector.',
            'Current/Previous ledger claims are not rollback approval. An absent previous selector is not a stock-reader recovery plan.')
    }
}
