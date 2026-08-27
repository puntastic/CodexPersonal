function Get-CodexCliPathFromText {
    param(
        [string]$Text,
        [string]$Description
    )

    $matches = [regex]::Matches(
        $Text,
        '(?m)^[ \t]*CODEX_CLI_PATH[ \t]*=[ \t]*(?<value>[^\r\n]+)[ \t]*\r?$'
    )
    if ($matches.Count -eq 0) {
        return $null
    }
    if ($matches.Count -ne 1) {
        throw "Expected one CODEX_CLI_PATH setting in $Description; found $($matches.Count)."
    }
    $literal = $matches[0].Groups["value"].Value.Trim()
    if ($literal.Length -ge 2 -and $literal[0] -eq "'" -and $literal[$literal.Length - 1] -eq "'") {
        return $literal.Substring(1, $literal.Length - 2)
    }
    if ($literal.Length -ge 2 -and $literal[0] -eq '"' -and $literal[$literal.Length - 1] -eq '"') {
        return $literal.Substring(1, $literal.Length - 2).Replace('\\', '\')
    }
    throw "CODEX_CLI_PATH in $Description is not a quoted TOML string."
}

function Get-CodexDevSha256Bytes {
    param([byte[]]$Bytes)

    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        return ([Convert]::ToHexString($sha.ComputeHash($Bytes))).ToLowerInvariant()
    } finally {
        $sha.Dispose()
    }
}

function Read-CodexCliConfigSnapshot {
    param([string]$ConfigPath)

    $config = [System.IO.Path]::GetFullPath($ConfigPath)
    if (-not (Test-Path -LiteralPath $config -PathType Leaf)) {
        return [pscustomobject]@{
            Exists = $false
            Path = $config
            Bytes = $null
            Text = $null
            Sha256 = $null
            ConfiguredEntrypoint = $null
            Error = $null
        }
    }
    try {
        $bytes = [System.IO.File]::ReadAllBytes($config)
        $text = [System.Text.UTF8Encoding]::new($false, $true).GetString($bytes)
        return [pscustomobject]@{
            Exists = $true
            Path = $config
            Bytes = $bytes
            Text = $text
            Sha256 = Get-CodexDevSha256Bytes $bytes
            ConfiguredEntrypoint = Get-CodexCliPathFromText `
                -Text $text `
                -Description $config
            Error = $null
        }
    } catch {
        return [pscustomobject]@{
            Exists = $true
            Path = $config
            Bytes = $null
            Text = $null
            Sha256 = $null
            ConfiguredEntrypoint = $null
            Error = $_.Exception.Message
        }
    }
}

function Get-CodexCliPathFromConfig {
    param([string]$ConfigPath)

    $snapshot = Read-CodexCliConfigSnapshot $ConfigPath
    if ($null -ne $snapshot.Error) {
        throw $snapshot.Error
    }
    return $snapshot.ConfiguredEntrypoint
}

function Get-CodexCliPathUpdatedText {
    param(
        [string]$Text,
        [string]$Entrypoint,
        [string]$Description
    )

    if ($Entrypoint.Contains("'")) {
        throw "Cannot encode a single quote in the CODEX_CLI_PATH literal: $Entrypoint"
    }
    $pattern = '(?m)^[ \t]*CODEX_CLI_PATH[ \t]*=[^\r\n]*(?<cr>\r?)$'
    $matches = [regex]::Matches($Text, $pattern)
    if ($matches.Count -ne 1) {
        throw "Activation requires exactly one existing CODEX_CLI_PATH setting in $Description; found $($matches.Count)."
    }
    $replacement = "CODEX_CLI_PATH = '$Entrypoint'"
    return [regex]::Replace($Text, $pattern, [System.Text.RegularExpressions.MatchEvaluator]{
        param($match)
        $replacement + $match.Groups["cr"].Value
    }, 1)
}

function Get-CodexCliConfigTransition {
    param(
        [string]$ConfigPath,
        [string]$Entrypoint
    )

    $snapshot = Read-CodexCliConfigSnapshot $ConfigPath
    if (-not $snapshot.Exists) {
        throw "Config does not exist: $($snapshot.Path)"
    }
    if ($null -ne $snapshot.Error) {
        throw $snapshot.Error
    }
    $afterText = Get-CodexCliPathUpdatedText `
        -Text $snapshot.Text `
        -Entrypoint $Entrypoint `
        -Description $snapshot.Path
    $afterBytes = [System.Text.UTF8Encoding]::new($false).GetBytes($afterText)
    return [pscustomobject]@{
        ConfigPath = $snapshot.Path
        ConfiguredBefore = $snapshot.ConfiguredEntrypoint
        ConfiguredAfter = $Entrypoint
        BeforeBytes = $snapshot.Bytes
        BeforeSha256 = $snapshot.Sha256
        AfterBytes = $afterBytes
        AfterSha256 = Get-CodexDevSha256Bytes $afterBytes
    }
}

function Set-CodexCliPathInConfig {
    param(
        [string]$ConfigPath,
        [string]$Entrypoint,
        [AllowNull()][object]$Transition
    )

    if ($null -eq $Transition) {
        $Transition = Get-CodexCliConfigTransition `
            -ConfigPath $ConfigPath `
            -Entrypoint $Entrypoint
    }
    if (-not (Test-CodexDevPathEqual -Left $ConfigPath -Right ([string]$Transition.ConfigPath)) -or
        -not (Test-CodexDevPathEqual -Left $Entrypoint -Right ([string]$Transition.ConfiguredAfter))) {
        throw "Config transition does not match the requested config path and entrypoint."
    }
    $current = Read-CodexCliConfigSnapshot $ConfigPath
    if ($null -ne $current.Error -or $current.Sha256 -ne [string]$Transition.BeforeSha256) {
        throw "Config changed after the deployment transaction was prepared: $ConfigPath"
    }
    $tempPath = "$ConfigPath.tmp.$([guid]::NewGuid().ToString('N'))"
    try {
        [System.IO.File]::WriteAllBytes($tempPath, [byte[]]$Transition.AfterBytes)
        $tempHash = Get-CodexDevSha256Bytes ([System.IO.File]::ReadAllBytes($tempPath))
        if ($tempHash -ne [string]$Transition.AfterSha256) {
            throw "Prepared config bytes changed before commit."
        }
        Move-Item -LiteralPath $tempPath -Destination $ConfigPath -Force
    } finally {
        Remove-Item -LiteralPath $tempPath -Force -ErrorAction SilentlyContinue
    }
}
