# This is a bounded selector binder, not a full TOML validator. It recognizes
# explicit tables, dotted/quoted keys and single-line string selectors. It
# lexes other values only to preserve ownership across comments, containers and
# multiline strings. Inline/array-table representations of the selector's
# ancestors are refused rather than mistaken for a missing setting.
function Get-CodexConfigStatements {
    param([string]$Text, [string]$Description)

    if ($Text.Length -gt 4MB) {
        throw "Config binding exceeds its 4,194,304-character input limit in $Description."
    }
    $clean = $Text.ToCharArray()
    if ($clean.Length -gt 0 -and $clean[0] -eq [char]0xFEFF) { $clean[0] = ' ' }
    $stack = [System.Collections.Generic.List[char]]::new()
    $quote = [char]0
    $multi = $false
    $comment = $false
    $start = 0
    $line = 1
    $startLine = 1
    for ($i = 0; $i -lt $Text.Length; $i++) {
        $ch = $Text[$i]
        if ($comment) {
            if ($ch -eq "`n") { $comment = $false } else { $clean[$i] = ' '; continue }
        }
        if ($quote -ne [char]0) {
            if ($ch -eq "`n") {
                if (-not $multi) { throw "Unclosed single-line TOML string at line $line in $Description." }
                $line++
            }
            if ($quote -eq '"' -and $ch -eq '\') {
                $i++
                if ($i -ge $Text.Length) { throw "Incomplete TOML escape in $Description." }
                if (-not $multi -and ($Text[$i] -eq "`n" -or $Text[$i] -eq "`r")) {
                    throw "Escaped newline in a single-line TOML string in $Description."
                }
                if ($Text[$i] -eq "`n") { $line++ }
                continue
            }
            if ($ch -eq $quote) {
                if (-not $multi) { $quote = [char]0; continue }
                $run = 1
                while ($i + $run -lt $Text.Length -and $Text[$i + $run] -eq $quote) { $run++ }
                if ($run -ge 3) {
                    if ($run -gt 5) { throw "Unsupported TOML quote run at line $line in $Description." }
                    $quote = [char]0
                    $multi = $false
                }
                $i += $run - 1
            }
            continue
        }
        if ($ch -eq '#') { $comment = $true; $clean[$i] = ' '; continue }
        if ($ch -eq '"' -or $ch -eq "'") {
            $quote = $ch
            $multi = $i + 2 -lt $Text.Length -and $Text[$i + 1] -eq $ch -and $Text[$i + 2] -eq $ch
            if ($multi) { $i += 2 }
            continue
        }
        if ($ch -eq '[' -or $ch -eq '{') {
            if ($stack.Count -ge 64) { throw "TOML container nesting exceeds the binding limit in $Description." }
            $stack.Add($ch)
        } elseif ($ch -eq ']' -or $ch -eq '}') {
            $opening = if ($ch -eq ']') { '[' } else { '{' }
            if ($stack.Count -eq 0 -or $stack[$stack.Count - 1] -ne $opening) {
                throw "Unbalanced TOML delimiter at line $line in $Description."
            }
            $stack.RemoveAt($stack.Count - 1)
        }
        if ($ch -eq "`n") {
            if ($stack.Count -eq 0) {
                [pscustomobject]@{ Text = -join $clean[$start..$i]; Start = $start; Line = $startLine }
                $start = $i + 1
                $startLine = $line + 1
            }
            $line++
        }
    }
    if ($quote -ne [char]0 -or $stack.Count -ne 0) { throw "Unclosed TOML string or container in $Description." }
    if ($start -lt $Text.Length) {
        [pscustomobject]@{ Text = -join $clean[$start..($Text.Length - 1)]; Start = $start; Line = $startLine }
    }
}

function Read-CodexConfigString {
    param([string]$Text, [int]$Start, [string]$Description)

    $quote = $Text[$Start]
    if ($Start + 2 -lt $Text.Length -and $Text[$Start + 1] -eq $quote -and $Text[$Start + 2] -eq $quote) {
        throw "Multiline TOML keys/selectors are unsupported in $Description; use a single-line quoted selector."
    }
    $value = [System.Text.StringBuilder]::new()
    for ($i = $Start + 1; $i -lt $Text.Length; $i++) {
        $ch = $Text[$i]
        if ($ch -eq $quote) {
            return [pscustomobject]@{ Value = $value.ToString(); End = $i + 1 }
        }
        if ([int]$ch -lt 0x20 -and $ch -ne "`t" -or [int]$ch -eq 0x7F) {
            throw "Invalid control character in a TOML key/selector in $Description."
        }
        if ($quote -eq '"' -and $ch -eq '\') {
            $i++
            if ($i -ge $Text.Length) { throw "Incomplete TOML key/selector escape in $Description." }
            $escape = $Text[$i]
            $decoded = switch -CaseSensitive ($escape) {
                '"' { '"' }; '\' { '\' }; 'b' { [char]8 }; 't' { "`t" }
                'n' { "`n" }; 'f' { [char]12 }; 'r' { "`r" }
                'u' { $null }; 'U' { $null }
                default { throw "Unsupported TOML key/selector escape in $Description." }
            }
            if ($escape -ceq 'u' -or $escape -ceq 'U') {
                $digits = if ($escape -ceq 'u') { 4 } else { 8 }
                if ($i + $digits -ge $Text.Length) { throw "Incomplete TOML Unicode escape in $Description." }
                $hex = $Text.Substring($i + 1, $digits)
                if ($hex -notmatch '^[0-9A-Fa-f]+$') { throw "Invalid TOML Unicode escape in $Description." }
                $point = [Convert]::ToUInt32($hex, 16)
                if ($point -gt 0x10FFFF -or ($point -ge 0xD800 -and $point -le 0xDFFF)) {
                    throw "Invalid TOML Unicode scalar in $Description."
                }
                $decoded = [char]::ConvertFromUtf32([int]$point)
                $i += $digits
            }
            [void]$value.Append($decoded)
        } else { [void]$value.Append($ch) }
    }
    throw "Unclosed TOML key/selector string in $Description."
}

function Read-CodexConfigKeyPath {
    param([string]$Text, [int]$Start, [string]$Description)

    $parts = [System.Collections.Generic.List[string]]::new()
    $i = $Start
    while ($true) {
        while ($i -lt $Text.Length -and ($Text[$i] -eq ' ' -or $Text[$i] -eq "`t")) { $i++ }
        if ($i -ge $Text.Length) { throw "Incomplete TOML key in $Description." }
        if ($Text[$i] -eq '"' -or $Text[$i] -eq "'") {
            $key = Read-CodexConfigString -Text $Text -Start $i -Description $Description
            $parts.Add($key.Value)
            $i = $key.End
        } else {
            $key = [regex]::Match($Text.Substring($i), '^[A-Za-z0-9_-]+')
            if (-not $key.Success) { throw "Unsupported TOML key syntax in $Description." }
            $parts.Add($key.Value)
            $i += $key.Length
        }
        while ($i -lt $Text.Length -and ($Text[$i] -eq ' ' -or $Text[$i] -eq "`t")) { $i++ }
        if ($i -ge $Text.Length -or $Text[$i] -ne '.') { break }
        $i++
    }
    return [pscustomobject]@{ Parts = $parts.ToArray(); End = $i }
}

function Get-CodexCliConfigBinding {
    param([string]$Text, [string]$Description)

    $target = @('mcp_servers', 'node_repl', 'env', 'CODEX_CLI_PATH')
    $section = @()
    $headers = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    $dottedTables = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
    $binding = $null
    foreach ($statement in (Get-CodexConfigStatements -Text $Text -Description $Description)) {
        $body = $statement.Text
        $i = 0
        while ($i -lt $body.Length -and $body[$i] -match '[ \t\r\n]') { $i++ }
        if ($i -eq $body.Length) { continue }
        $at = "$Description (line $($statement.Line))"
        $header = $body[$i] -eq '['
        $arrayTable = $header -and $i + 1 -lt $body.Length -and $body[$i + 1] -eq '['
        if ($header) { $i += 1 + [int]$arrayTable }
        $key = Read-CodexConfigKeyPath -Text $body -Start $i -Description $at
        $path = @(if ($header) { @($key.Parts) } else { @($section) + @($key.Parts) })
        $shared = 0
        while ($shared -lt [Math]::Min($path.Count, $target.Count) -and $path[$shared] -ceq $target[$shared]) { $shared++ }
        $related = $shared -eq [Math]::Min($path.Count, $target.Count)
        # Only identities on the selector's path are used for duplicate checks;
        # joining arbitrary quoted keys would otherwise create false collisions.
        $identity = if ($related) { $path -join '.' } else { $null }
        $i = $key.End
        if ($header) {
            $closing = if ($arrayTable) { ']]' } else { ']' }
            if ($body.Substring($i).Trim([char[]]@(' ', "`t", "`r", "`n")) -cne $closing) {
                throw "Unsupported TOML table syntax in $at."
            }
            if ($related) {
                if ($arrayTable -or $path.Count -ge $target.Count) {
                    throw "Selector path uses an array/table representation in $at; require a scalar string at mcp_servers.node_repl.env.CODEX_CLI_PATH."
                }
                if (-not $headers.Add($identity) -or $dottedTables.Contains($identity)) {
                    throw "Duplicate or reopened selector ancestor table in $at; reconcile the TOML definitions before deployment."
                }
            }
            $section = $path
            continue
        }
        if ($i -ge $body.Length -or $body[$i] -ne '=') { throw "Expected a TOML assignment in $at." }
        if ($related -and $path.Count -ne $target.Count) {
            throw "Unsupported inline/scalar selector ancestor or nested selector in $at; use the explicit mcp_servers.node_repl.env table and a quoted CODEX_CLI_PATH."
        }
        # A dotted assignment defines its intermediate tables, even when the
        # assigned leaf is unrelated to CODEX_CLI_PATH.
        for ($n = $section.Count + 1; $n -lt $path.Count -and $n -lt $target.Count; $n++) {
            if ($shared -ge $n) {
                $parent = $path[0..($n - 1)] -join '.'
                if ($headers.Contains($parent)) {
                    throw "Dotted key redefines an explicit selector ancestor table in $at; reconcile the TOML definitions before deployment."
                }
                [void]$dottedTables.Add($parent)
            }
        }
        if (-not $related) { continue }
        if ($null -ne $binding) { throw "Duplicate CODEX_CLI_PATH binding in $at; reconcile the definitions before deployment." }
        $i++
        while ($i -lt $body.Length -and ($body[$i] -eq ' ' -or $body[$i] -eq "`t")) { $i++ }
        if ($i -ge $body.Length -or ($body[$i] -ne '"' -and $body[$i] -ne "'")) {
            throw "CODEX_CLI_PATH in $at must be a single-line quoted TOML string."
        }
        $value = Read-CodexConfigString -Text $body -Start $i -Description $at
        if ($body.Substring($value.End) -notmatch '^[ \t\r\n]*$') {
            throw "Unexpected content after the CODEX_CLI_PATH string in $at."
        }
        if ($value.Value -match '[\x00-\x1F\x7F]') { throw "CODEX_CLI_PATH contains a control character in $at." }
        $binding = [pscustomobject]@{
            Status = 'bound'; Value = $value.Value; ValueStart = $statement.Start + $i
            ValueLength = $value.End - $i; LineNumber = $statement.Line
            Section = 'mcp_servers.node_repl.env'
        }
    }
    if ($null -ne $binding) { return $binding }
    return [pscustomobject]@{
        Status = 'missing'; Value = $null; ValueStart = $null; ValueLength = $null
        LineNumber = $null; Section = 'mcp_servers.node_repl.env'
    }
}

function Get-CodexCliPathFromText {
    param([string]$Text, [string]$Description)

    return (Get-CodexCliConfigBinding -Text $Text -Description $Description).Value
}

function Get-CodexDevSha256Bytes {
    param([byte[]]$Bytes)

    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        return ([BitConverter]::ToString($sha.ComputeHash($Bytes))).Replace('-', '').ToLowerInvariant()
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

    if ($Entrypoint.Contains("'") -or $Entrypoint -match '[\x00-\x1F\x7F]') {
        throw "Cannot encode quotes or control characters in the CODEX_CLI_PATH literal."
    }
    $binding = Get-CodexCliConfigBinding -Text $Text -Description $Description
    if ($binding.Status -cne 'bound') {
        throw "Activation requires one existing mcp_servers.node_repl.env.CODEX_CLI_PATH string in $Description."
    }
    # Replace only the literal: spacing, quoted keys, comments, BOM and all
    # unrelated sections remain byte-for-byte when the caller re-encodes UTF-8.
    return $Text.Substring(0, $binding.ValueStart) + "'$Entrypoint'" +
        $Text.Substring($binding.ValueStart + $binding.ValueLength)
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
        try {
            if (Test-Path -LiteralPath $tempPath -PathType Leaf -ErrorAction Stop) {
                Remove-Item -LiteralPath $tempPath -ErrorAction Stop
            }
        } catch {
            try {
                Write-Warning "Config temporary-file cleanup incomplete; any remaining file retained at '$tempPath': $($_.Exception.Message)" -WarningAction Continue
            } catch {
                # A diagnostic failure must not replace the config operation's outcome.
            }
        }
    }
}
