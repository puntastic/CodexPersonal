Set-StrictMode -Version Latest

$script:CodexDevRepositoryRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$script:CodexDevExpectedPackageFiles = @(
    "codex-package.json",
    "bin\codex.exe",
    "bin\codex-code-mode-host.exe",
    "codex-path\rg.exe",
    "codex-resources\codex-command-runner.exe",
    "codex-resources\codex-windows-sandbox-setup.exe"
)

function Get-CodexDevUserProfile {
    if (-not [string]::IsNullOrWhiteSpace($env:USERPROFILE)) {
        return [System.IO.Path]::GetFullPath($env:USERPROFILE)
    }
    if (-not [string]::IsNullOrWhiteSpace($HOME)) {
        return [System.IO.Path]::GetFullPath($HOME)
    }
    return [Environment]::GetFolderPath("UserProfile")
}

function Get-CodexDevLocalAppData {
    if (-not [string]::IsNullOrWhiteSpace($env:LOCALAPPDATA)) {
        return [System.IO.Path]::GetFullPath($env:LOCALAPPDATA)
    }
    return [Environment]::GetFolderPath("LocalApplicationData")
}

function Get-CodexDevDefaultConfigPath {
    return Join-Path (Get-CodexDevUserProfile) ".codex\config.toml"
}

function Get-CodexDevDefaultDeploymentRoot {
    return Join-Path (Get-CodexDevLocalAppData) "OpenAI\Codex\dev-overrides"
}

function Resolve-CodexDevFile {
    param(
        [string]$ExplicitPath,
        [string[]]$Candidates,
        [string]$Description
    )

    if (-not [string]::IsNullOrWhiteSpace($ExplicitPath)) {
        if (Test-Path -LiteralPath $ExplicitPath -PathType Leaf -ErrorAction SilentlyContinue) {
            return (Resolve-Path -LiteralPath $ExplicitPath).Path
        }
        $explicitApplication = Get-Command -Name $ExplicitPath -CommandType Application -ErrorAction SilentlyContinue |
            Select-Object -First 1
        if ($null -ne $explicitApplication -and
            -not [string]::IsNullOrWhiteSpace($explicitApplication.Source)) {
            return $explicitApplication.Source
        }
        throw "The explicit $Description could not be resolved: $ExplicitPath"
    }
    foreach ($candidate in $Candidates) {
        if ([string]::IsNullOrWhiteSpace($candidate)) {
            continue
        }
        if (Test-Path -LiteralPath $candidate -PathType Leaf -ErrorAction SilentlyContinue) {
            return (Resolve-Path -LiteralPath $candidate).Path
        }
        $application = Get-Command -Name $candidate -CommandType Application -ErrorAction SilentlyContinue |
            Select-Object -First 1
        if ($null -ne $application -and -not [string]::IsNullOrWhiteSpace($application.Source)) {
            return $application.Source
        }
    }
    throw "Could not find $Description. Checked: $($Candidates -join ', ')"
}

function Get-CodexDevTaskToken {
    $rawToken = if ([string]::IsNullOrWhiteSpace($env:CODEX_THREAD_ID)) {
        "manual"
    } else {
        $env:CODEX_THREAD_ID.Trim()
    }
    $safeToken = $rawToken -replace '[^A-Za-z0-9._-]', '-'
    if ([string]::IsNullOrWhiteSpace($safeToken)) {
        return "manual"
    }
    if ($safeToken.Length -le 64) {
        return $safeToken
    }
    $suffix = (Get-CodexDevSha256Text $rawToken).Substring(0, 12)
    return "$($safeToken.Substring(0, 48))-$suffix"
}

function Invoke-CodexDevNative {
    param(
        [string]$FilePath,
        [string[]]$ArgumentList = @(),
        [string]$WorkingDirectory = $script:CodexDevRepositoryRoot,
        [switch]$Capture
    )

    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $FilePath
    $startInfo.WorkingDirectory = $WorkingDirectory
    $startInfo.UseShellExecute = $false
    foreach ($argument in $ArgumentList) {
        $startInfo.ArgumentList.Add($argument)
    }
    foreach ($pathKey in @($startInfo.Environment.Keys | Where-Object { $_ -ieq "Path" })) {
        $null = $startInfo.Environment.Remove($pathKey)
    }
    $startInfo.Environment["PATH"] = $env:PATH
    if ($Capture) {
        $startInfo.RedirectStandardOutput = $true
        $startInfo.RedirectStandardError = $true
    }

    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    try {
        if (-not $process.Start()) {
            throw "Could not start native command: $FilePath"
        }
        $stdoutTask = if ($Capture) { $process.StandardOutput.ReadToEndAsync() } else { $null }
        $stderrTask = if ($Capture) { $process.StandardError.ReadToEndAsync() } else { $null }
        $process.WaitForExit()
        $output = @()
        if ($Capture) {
            $stdout = $stdoutTask.GetAwaiter().GetResult()
            $stderr = $stderrTask.GetAwaiter().GetResult()
            $output = @($stdout, $stderr) |
                Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
                ForEach-Object { @($_ -split "`r?`n") } |
                Where-Object { -not [string]::IsNullOrWhiteSpace($_) }
        }
        $exitCode = $process.ExitCode
        if ($exitCode -ne 0) {
            $tail = if ($Capture) { ($output | Select-Object -Last 20) -join [Environment]::NewLine } else { "" }
            throw "Command failed with exit code $exitCode`: $FilePath $($ArgumentList -join ' ')$([Environment]::NewLine)$tail"
        }
        return $output
    } finally {
        $process.Dispose()
    }
}

function Invoke-CodexDevGit {
    param([string[]]$ArgumentList)

    $git = Resolve-CodexDevFile -Candidates @("git.exe", "git") -Description "Git"
    $gitArguments = @(
        "-c", "core.excludesfile=", "-C", $script:CodexDevRepositoryRoot
    ) + $ArgumentList
    return Invoke-CodexDevNative -FilePath $git -ArgumentList $gitArguments -Capture
}

function Get-CodexDevGitState {
    $head = ([string](Invoke-CodexDevGit @("rev-parse", "HEAD") | Select-Object -First 1)).Trim()
    $shortHead = ([string](Invoke-CodexDevGit @("rev-parse", "--short=12", "HEAD") | Select-Object -First 1)).Trim()
    $branch = ([string](Invoke-CodexDevGit @("branch", "--show-current") | Select-Object -First 1)).Trim()
    $status = @(Invoke-CodexDevGit @("status", "--porcelain=v1", "--untracked-files=normal"))
    $fingerprint = Get-CodexDevGitVisibleSourceFingerprint -Head $head
    $headAfter = ([string](Invoke-CodexDevGit @("rev-parse", "HEAD") | Select-Object -First 1)).Trim()
    if ($headAfter -ne $head) {
        throw "Git HEAD changed while source state was being captured: $head -> $headAfter"
    }
    return [pscustomobject]@{
        Head = $head
        ShortHead = $shortHead
        Branch = $branch
        Dirty = $status.Count -gt 0
        Status = @($status | ForEach-Object { [string]$_ })
        GitVisibleSourceFingerprint = $fingerprint
    }
}

function Get-CodexDevGitVisibleSourceFingerprint {
    param([string]$Head)

    $changed = (Invoke-CodexDevGit @(
        "diff", "--name-only", "--no-renames", "-z", $Head, "--"
    )) -join "`n"
    $rawDiff = (Invoke-CodexDevGit @(
        "diff", "--raw", "--no-renames", "-z", $Head, "--"
    )) -join "`n"
    $untracked = (Invoke-CodexDevGit @(
        "ls-files", "--others", "--exclude-standard", "-z", "--"
    )) -join "`n"
    $paths = @($changed, $untracked) |
        Where-Object { -not [string]::IsNullOrEmpty($_) } |
        ForEach-Object {
            $_.Split(
                [char[]]@([char]0),
                [System.StringSplitOptions]::RemoveEmptyEntries
            )
        } |
        Sort-Object -Unique
    $rows = foreach ($relativePath in $paths) {
        $fullPath = Join-Path $script:CodexDevRepositoryRoot $relativePath
        if (Test-Path -LiteralPath $fullPath -PathType Leaf) {
            "$relativePath=file:$((Get-FileHash -LiteralPath $fullPath -Algorithm SHA256).Hash.ToLowerInvariant())"
        } elseif (Test-Path -LiteralPath $fullPath -PathType Container) {
            "$relativePath=directory"
        } else {
            "$relativePath=missing"
        }
    }
    return Get-CodexDevSha256Text ((@($Head, "raw=$rawDiff") + @($rows)) -join "`n")
}

function Get-CodexDevSha256Text {
    param([string]$Text)

    $bytes = [System.Text.Encoding]::UTF8.GetBytes($Text)
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        return ([Convert]::ToHexString($sha.ComputeHash($bytes))).ToLowerInvariant()
    } finally {
        $sha.Dispose()
    }
}

function Write-CodexDevJson {
    param(
        [string]$Path,
        [object]$Value
    )

    $directory = Split-Path -Parent $Path
    New-Item -ItemType Directory -Path $directory -Force | Out-Null
    $tempPath = "$Path.tmp.$([guid]::NewGuid().ToString('N'))"
    [System.IO.File]::WriteAllText(
        $tempPath,
        ($Value | ConvertTo-Json -Depth 12),
        [System.Text.UTF8Encoding]::new($false)
    )
    Move-Item -LiteralPath $tempPath -Destination $Path -Force
}

function Write-CodexDevReceipt {
    param(
        [string]$Action,
        [object]$Details,
        [object]$Source
    )

    if ($null -eq $Source) {
        $Source = Get-CodexDevGitState
    }

    $receipt = [ordered]@{
        SchemaVersion = 1
        Action = $Action
        TimestampUtc = [DateTime]::UtcNow.ToString("o")
        Repository = $script:CodexDevRepositoryRoot
        Source = $Source
        Details = $Details
    }
    $stamp = [DateTime]::UtcNow.ToString("yyyyMMdd-HHmmssfff")
    $taskToken = Get-CodexDevTaskToken
    $unique = [guid]::NewGuid().ToString("N").Substring(0, 12)
    $receiptName = "$stamp-$taskToken-$unique-$($Action.ToLowerInvariant()).json"
    $receiptPath = Join-Path $script:CodexDevRepositoryRoot "codex-rs\target\desktop-dev\receipts\$receiptName"
    Write-CodexDevJson -Path $receiptPath -Value $receipt
    return $receiptPath
}

function Write-CodexDevResult {
    param(
        [object]$Value,
        [switch]$Json
    )

    if ($Json) {
        $Value | ConvertTo-Json -Depth 12
    } elseif ($Value -is [string]) {
        Write-Host $Value
    } else {
        $Value | Format-List | Out-String -Width 220 | Write-Host
    }
}
