$script:CodexDevPersistentSelectorReader = $null
$script:CodexDevPersistentSelectorWriter = $null

function Set-CodexDevPersistentSelectorTestAdapter {
    param(
        [scriptblock]$Reader,
        [scriptblock]$Writer
    )

    if ($null -eq $Reader -or $null -eq $Writer) {
        throw "Persistent-selector test adapters require both Reader and Writer."
    }
    $script:CodexDevPersistentSelectorReader = $Reader
    $script:CodexDevPersistentSelectorWriter = $Writer
}

function Clear-CodexDevPersistentSelectorTestAdapter {
    $script:CodexDevPersistentSelectorReader = $null
    $script:CodexDevPersistentSelectorWriter = $null
}

function Read-CodexDevPersistentSelector {
    try {
        $value = if ($null -ne $script:CodexDevPersistentSelectorReader) {
            & $script:CodexDevPersistentSelectorReader -Name "CODEX_CLI_PATH"
        } else {
            [System.Environment]::GetEnvironmentVariable(
                "CODEX_CLI_PATH",
                [System.EnvironmentVariableTarget]::User
            )
        }
        return [pscustomobject]@{
            Value = if ($null -eq $value) { $null } else { [string]$value }
            Error = $null
            Scope = "User"
        }
    } catch {
        return [pscustomobject]@{
            Value = $null
            Error = $_.Exception.Message
            Scope = "User"
        }
    }
}

function Get-CodexDevPersistentSelector {
    $read = Read-CodexDevPersistentSelector
    if ($null -ne $read.Error) {
        throw "User-scope CODEX_CLI_PATH could not be read: $($read.Error)"
    }
    return $read.Value
}

function Test-CodexDevSelectorExact {
    param(
        [AllowNull()][string]$Left,
        [AllowNull()][string]$Right
    )

    if ($null -eq $Left -or $null -eq $Right) {
        return $null -eq $Left -and $null -eq $Right
    }
    return [string]::Equals($Left, $Right, [System.StringComparison]::Ordinal)
}

function Set-CodexDevPersistentSelector {
    param([AllowNull()][string]$Entrypoint)

    if ($null -ne $script:CodexDevPersistentSelectorWriter) {
        $null = & $script:CodexDevPersistentSelectorWriter `
            -Name "CODEX_CLI_PATH" `
            -Value $Entrypoint
    } else {
        [System.Environment]::SetEnvironmentVariable(
            "CODEX_CLI_PATH",
            $Entrypoint,
            [System.EnvironmentVariableTarget]::User
        )
    }

    $readback = Get-CodexDevPersistentSelector
    if (-not (Test-CodexDevSelectorExact -Left $readback -Right $Entrypoint)) {
        throw (
            "User-scope CODEX_CLI_PATH did not read back exactly after update. " +
            "Expected '$Entrypoint'; read '$readback'."
        )
    }
    return $readback
}

function Test-CodexDevRestartRequired {
    param(
        [AllowNull()][string]$ProcessLiveEntrypoint,
        [AllowNull()][string]$PersistentEntrypoint
    )

    return -not (Test-CodexDevSelectorExact `
        -Left $ProcessLiveEntrypoint `
        -Right $PersistentEntrypoint)
}
