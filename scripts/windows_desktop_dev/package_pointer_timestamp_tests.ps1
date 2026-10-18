Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

. (Join-Path $PSScriptRoot "package.ps1")

function Assert-PointerTimestampTest {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw $Message }
}

function Assert-PointerTimestampRejected {
    param([object]$Value)
    $rejected = $false
    try { $null = ConvertTo-CodexDevPointerTimestamp $Value } catch { $rejected = $true }
    Assert-PointerTimestampTest $rejected "An ambiguous or malformed timestamp was accepted."
}

# The actual lock is exercised only in this newly owned GUID directory.
# Pointer reads/writes are mocked: no task pointer or package is accessed.
$testBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd([char[]]"\/")
$testRoot = Join-Path $testBase ([guid]::NewGuid().ToString("N"))
if (Test-Path -LiteralPath $testRoot) { throw "Timestamp fixture directory already exists." }
New-Item -ItemType Directory -Path $testRoot | Out-Null
$script:FixturePointerPath = Join-Path $testRoot "mock-pointer.json"
$script:PointerExists = $true
$script:ExistingPointerJson = $null
$script:PointerWrites = @()

function Test-Path {
    param([string]$LiteralPath, [string]$PathType)
    Assert-PointerTimestampTest ($LiteralPath -eq $script:FixturePointerPath) "Unexpected pointer existence check."
    return $script:PointerExists
}
function Get-Content {
    param([string]$LiteralPath, [switch]$Raw)
    Assert-PointerTimestampTest ($LiteralPath -eq $script:FixturePointerPath -and $Raw) "Unexpected pointer read."
    return $script:ExistingPointerJson
}
function Write-CodexDevJson {
    param([string]$Path, [object]$Value)
    Assert-PointerTimestampTest ($Path -eq $script:FixturePointerPath) "Unexpected pointer write."
    $script:PointerWrites += $Value
}

function Test-PointerTimestampUpdate {
    param([object]$Existing, [object]$Incoming, [bool]$ExpectedAdvance, [switch]$Invalid)
    $script:PointerExists = $true
    $script:ExistingPointerJson = @{ StartedAtUtc = $Existing } | ConvertTo-Json -Compress
    $script:PointerWrites = @()
    $result = Complete-CodexDevBuildPointer -SourceStable $true -PointerPath $script:FixturePointerPath -Pointer ([pscustomobject]@{ StartedAtUtc = $Incoming })
    Assert-PointerTimestampTest ($result.Advanced -eq $ExpectedAdvance) "Pointer newer-wins/tie behavior changed."
    Assert-PointerTimestampTest ($script:PointerWrites.Count -eq [int]$ExpectedAdvance) "An unqualified pointer was written."
    Assert-PointerTimestampTest (($null -ne $result.Error) -eq [bool]$Invalid) "Invalid comparison did not fail closed."
    $lock = [IO.File]::Open("$script:FixturePointerPath.lock", [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    $lock.Dispose()
}

$oldCulture = [Globalization.CultureInfo]::CurrentCulture
$oldUiCulture = [Globalization.CultureInfo]::CurrentUICulture
try {
    $raw = "2026-10-16T14:13:06.7161810Z"
    $offsetRaw = "2026-10-17T00:13:06.7161810+10:00"
    $utc = [DateTime]::ParseExact($raw, "o", [Globalization.CultureInfo]::InvariantCulture, [Globalization.DateTimeStyles]::RoundtripKind)
    $offset = [DateTimeOffset]::Parse($offsetRaw, [Globalization.CultureInfo]::InvariantCulture)
    foreach ($culture in @("en-AU", "en-US", "fr-FR")) {
        [Globalization.CultureInfo]::CurrentCulture = [Globalization.CultureInfo]::GetCultureInfo($culture)
        [Globalization.CultureInfo]::CurrentUICulture = [Globalization.CultureInfo]::GetCultureInfo($culture)
        $typed = ('{"StartedAtUtc":"' + $raw + '"}' | ConvertFrom-Json).StartedAtUtc
        $typedOffset = ('{"StartedAtUtc":"' + $offsetRaw + '"}' | ConvertFrom-Json).StartedAtUtc
        Assert-PointerTimestampTest ($typed -is [DateTime] -and $typed.Kind -eq [DateTimeKind]::Utc) "UTC JSON did not reproduce the typed DateTime input."
        if ($culture -eq "en-AU") {
            $legacyFailed = $false
            try { $null = [DateTime]::Parse([string]$typed).ToUniversalTime() } catch { $legacyFailed = $true }
            Assert-PointerTimestampTest $legacyFailed "The original en-AU string-cast/Parse failure was not reproduced."
        }
        foreach ($value in @($raw, $offsetRaw, $utc, $typed, $typedOffset, $offset, $utc.ToLocalTime())) {
            $converted = ConvertTo-CodexDevPointerTimestamp $value
            Assert-PointerTimestampTest ($converted -is [DateTimeOffset] -and $converted.Offset -eq [TimeSpan]::Zero -and $converted.UtcDateTime.Ticks -eq $utc.Ticks) "Timestamp lost its instant or fractional precision."
        }
        Assert-PointerTimestampTest ((ConvertTo-CodexDevPointerTimestamp "2026-10-16T14:13:06.7161811Z").UtcDateTime.Ticks -eq $utc.Ticks + 1) "One-tick precision was lost."
        foreach ($rawShort in @("2026-10-16T14:13:06Z", "2026-10-16T14:13:06.1Z")) {
            $expected = [DateTimeOffset]::Parse($rawShort, [Globalization.CultureInfo]::InvariantCulture)
            Assert-PointerTimestampTest ((ConvertTo-CodexDevPointerTimestamp $rawShort) -eq $expected) "Explicit-zone short ISO input failed."
        }
        foreach ($invalid in @($null, "", "10/16/2026 14:13:06", "2026-10-16T14:13:06", "2026-10-16T14:13:06.71618101Z", "2026-99-16T14:13:06Z", 123, [DateTime]::SpecifyKind($utc, [DateTimeKind]::Unspecified))) {
            Assert-PointerTimestampRejected $invalid
        }
        Test-PointerTimestampUpdate $raw "2026-10-16T14:13:06.7161811Z" $true
        Test-PointerTimestampUpdate $raw "2026-10-16T14:13:06.7161809Z" $false
        Test-PointerTimestampUpdate $raw $typed $true
        Test-PointerTimestampUpdate $offsetRaw $raw $true
        Test-PointerTimestampUpdate $raw $offset $true
        Test-PointerTimestampUpdate "malformed" $raw $false -Invalid
        Test-PointerTimestampUpdate "2026-10-16T14:13:06.71618101Z" $raw $false -Invalid
        Test-PointerTimestampUpdate "2026-10-16T14:13:06" $raw $false -Invalid
        Test-PointerTimestampUpdate $raw "malformed" $false -Invalid
        Test-PointerTimestampUpdate "" $raw $false -Invalid
        Test-PointerTimestampUpdate $null $raw $false -Invalid
        $script:PointerExists = $false
        $script:PointerWrites = @()
        $result = Complete-CodexDevBuildPointer -SourceStable $true -PointerPath $script:FixturePointerPath -Pointer ([pscustomobject]@{ StartedAtUtc = "zoneless" })
        Assert-PointerTimestampTest (-not $result.Advanced -and $null -ne $result.Error -and $script:PointerWrites.Count -eq 0) "Invalid first pointer was written."
        $result = Complete-CodexDevBuildPointer -SourceStable $false -PointerPath $script:FixturePointerPath -Pointer ([pscustomobject]@{ StartedAtUtc = "invalid but source unstable" })
        Assert-PointerTimestampTest (-not $result.Advanced -and $null -eq $result.Error -and $script:PointerWrites.Count -eq 0) "Source-stable guard changed."
        $result = Complete-CodexDevBuildPointer -SourceStable $true -PointerPath $script:FixturePointerPath -Pointer ([pscustomobject]@{ StartedAtUtc = $raw })
        Assert-PointerTimestampTest ($result.Advanced -and $null -eq $result.Error -and $script:PointerWrites.Count -eq 1) "A valid first pointer was not written."
        foreach ($invalidJson in @("{malformed", "{}")) {
            $script:PointerExists = $true
            $script:ExistingPointerJson = $invalidJson
            $script:PointerWrites = @()
            $result = Complete-CodexDevBuildPointer -SourceStable $true -PointerPath $script:FixturePointerPath -Pointer ([pscustomobject]@{ StartedAtUtc = $raw })
            Assert-PointerTimestampTest (-not $result.Advanced -and $null -ne $result.Error -and $script:PointerWrites.Count -eq 0) "Malformed existing pointer data was overwritten."
        }
        Write-Output "Pointer timestamp checks passed for $culture."
    }
} finally {
    [Globalization.CultureInfo]::CurrentCulture = $oldCulture
    [Globalization.CultureInfo]::CurrentUICulture = $oldUiCulture
    Remove-CodexDevSmokeHome -SmokeBase $testBase -SmokeHome $testRoot
}
