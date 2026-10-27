Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'deployment_config.ps1')

$script:Checks = 0
function Assert-BindingEqual {
    param([AllowNull()][object]$Expected, [AllowNull()][object]$Actual, [string]$Label)
    $script:Checks++
    if ($Expected -cne $Actual) { throw "$Label failed. Expected <$Expected>, got <$Actual>." }
}
function Assert-BindingRefused {
    param([string]$Text, [string]$Pattern, [string]$Label)
    $script:Checks++
    try { $null = Get-CodexCliConfigBinding -Text $Text -Description 'synthetic fixture' }
    catch {
        if ($_.Exception.Message -match $Pattern) { return }
        throw "$Label failed with an unexpected diagnostic: $($_.Exception.Message)"
    }
    throw "$Label was accepted."
}

$header = '[mcp_servers.node_repl.env]'
$selector = "CODEX_CLI_PATH = 'C:\old\codex.exe'"
$valid = "$header`n$selector`n"
$binding = Get-CodexCliConfigBinding -Text $valid -Description 'synthetic fixture'
Assert-BindingEqual 'bound' $binding.Status 'Explicit table status'
Assert-BindingEqual 'C:\old\codex.exe' $binding.Value 'Literal path'
Assert-BindingEqual 2 $binding.LineNumber 'Selector line'
Assert-BindingEqual "'C:\old\codex.exe'" $valid.Substring($binding.ValueStart, $binding.ValueLength) 'Exact source span'
Assert-BindingEqual $null (Get-CodexCliPathFromText -Text '' -Description 'empty') 'Empty input'

foreach ($wrong in @(
    $selector,
    "[other]`n$selector",
    "[mcp_servers.other.env]`n$selector",
    "[MCP_SERVERS.node_repl.env]`n$selector",
    "[mcp_servers.node_repl.env]`ncodex_cli_path='wrong-case'",
    "['mcp_servers.node_repl.env']`n$selector",
    "# $header`n# $selector",
    "[mcp_servers.node_repl.env.child]`n$selector"
)) {
    Assert-BindingEqual $null (Get-CodexCliPathFromText -Text $wrong -Description 'wrong owner') 'Unrelated assignment is not selected'
    $script:Checks++
    try { $null = Get-CodexCliPathUpdatedText -Text $wrong -Entrypoint 'C:\new.exe' -Description 'wrong owner' }
    catch {
        if ($_.Exception.Message -match 'requires one existing mcp_servers') { continue }
        throw
    }
    throw 'Writer changed an unrelated setting.'
}

foreach ($form in @(
    '[ "mcp_servers" . ''node_repl'' . env ] # owner',
    '[mcp_servers."node_repl".env]',
    '[mcp_servers.node_repl."e\u006Ev"]'
)) {
    Assert-BindingEqual 'C:\old\codex.exe' (Get-CodexCliPathFromText -Text "$form`n$selector # retained" -Description 'quoted header') 'Quoted keys'
}
foreach ($form in @(
    'mcp_servers.node_repl.env.CODEX_CLI_PATH',
    '"mcp_servers".node_repl.''env''."CODEX_CLI_PATH"',
    'mcp_servers.node_repl.env."CODEX_CLI_PAT\u0048"'
)) {
    Assert-BindingEqual 'C:\old\codex.exe' (Get-CodexCliPathFromText -Text "$form = 'C:\old\codex.exe'" -Description 'dotted key') 'Dotted root selector'
}
Assert-BindingEqual 'C:\old\codex.exe' (Get-CodexCliPathFromText -Text "[mcp_servers.node_repl]`nenv.CODEX_CLI_PATH = 'C:\old\codex.exe'" -Description 'relative dotted key') 'Ancestor-relative selector'
Assert-BindingEqual 'C:\old\codex.exe' (Get-CodexCliPathFromText -Text "$header`nCODEX_CLI_PATH = `"C:\\old\\codex.exe`" # escaped" -Description 'basic string') 'Basic escapes'
Assert-BindingEqual 'C:\old\codex.exe' (Get-CodexCliPathFromText -Text "$header`nCODEX_CLI_PATH = `"C:\u005Cold\U0000005Ccodex.exe`"" -Description 'Unicode string') 'Unicode escapes'
Assert-BindingEqual 'C:\#path]\codex.exe' (Get-CodexCliPathFromText -Text "$header`nCODEX_CLI_PATH = 'C:\#path]\codex.exe' # comment" -Description 'literal punctuation') 'Comment syntax inside string'

$multiline = @'
instructions = """
[mcp_servers.node_repl.env]
CODEX_CLI_PATH = 'C:\fake.exe'
"""
other = '''
[mcp_servers.node_repl.env]
CODEX_CLI_PATH = 'C:\fake2.exe'
'''
array = [
  "[mcp_servers.node_repl.env]", # comment
  { name = "CODEX_CLI_PATH", value = "decoy" }
]
[mcp_servers.node_repl.env]
CODEX_CLI_PATH = 'C:\real.exe'
'@
Assert-BindingEqual 'C:\real.exe' (Get-CodexCliPathFromText -Text $multiline -Description 'multiline decoys') 'Multiline and container decoys'
$literalEnd = "note = '''ends with a quote''''`n$valid"
Assert-BindingEqual 'C:\old\codex.exe' (Get-CodexCliPathFromText -Text $literalEnd -Description 'four quote terminator') 'Valid four-quote terminator'
Assert-BindingEqual 'C:\real.exe' (Get-CodexCliPathFromText -Text "$multiline`n[unrelated]`nCODEX_CLI_PATH='C:\unrelated.exe'" -Description 'multiple consumers') 'Unrelated same-name setting'

foreach ($case in @(
    @{ Text = "$valid$selector"; Error = 'Duplicate CODEX'; Label = 'Duplicate selector' },
    @{ Text = "$valid$header"; Error = 'Duplicate or reopened'; Label = 'Reopened target table' },
    @{ Text = "$valid[mcp_servers.node_repl.env.child]`nX=1`n$header"; Error = 'Duplicate or reopened'; Label = 'Reopened after child' },
    @{ Text = "mcp_servers.node_repl.env.OTHER='x'`n$valid"; Error = 'Duplicate or reopened'; Label = 'Dotted table redefinition' },
    @{ Text = "$valid[mcp_servers.node_repl]`nenv.OTHER='x'"; Error = 'Dotted key redefines'; Label = 'Explicit table redefinition through unrelated dotted leaf' },
    @{ Text = "[mcp_servers]`n[mcp_servers]`n$valid"; Error = 'Duplicate or reopened'; Label = 'Repeated ancestor' },
    @{ Text = "[[mcp_servers.node_repl.env]]`n$selector"; Error = 'array/table'; Label = 'Target array table' },
    @{ Text = "[[mcp_servers]]`nnode_repl.env.CODEX_CLI_PATH='x'"; Error = 'array/table'; Label = 'Array ancestor' },
    @{ Text = 'mcp_servers = { node_repl = { env = { CODEX_CLI_PATH = "x" } } }'; Error = 'inline/scalar'; Label = 'Inline ancestor' },
    @{ Text = "[mcp_servers.node_repl]`nenv = { CODEX_CLI_PATH = 'x' }"; Error = 'inline/scalar'; Label = 'Inline env' },
    @{ Text = "$header`nCODEX_CLI_PATH.child = 'x'"; Error = 'inline/scalar'; Label = 'Nested selector' },
    @{ Text = '[mcp_servers.node_repl.env.CODEX_CLI_PATH]'; Error = 'array/table'; Label = 'Selector table' },
    @{ Text = "$header`nCODEX_CLI_PATH = 3"; Error = 'single-line quoted'; Label = 'Wrong value type' },
    @{ Text = "$header`nCODEX_CLI_PATH = '''multi'''"; Error = 'Multiline'; Label = 'Multiline selector' },
    @{ Text = "$header`nCODEX_CLI_PATH = 'x' extra"; Error = 'Unexpected content'; Label = 'Trailing content' },
    @{ Text = "$header`nCODEX_CLI_PATH = `"C:\wrong\path`""; Error = 'Unsupported TOML'; Label = 'Invalid escape' },
    @{ Text = "$header`nCODEX_CLI_PATH = `"C:\uD800`""; Error = 'Invalid TOML Unicode'; Label = 'Surrogate escape' },
    @{ Text = "$header`nCODEX_CLI_PATH = `"C:\U00110000`""; Error = 'Invalid TOML Unicode'; Label = 'Out-of-range escape' },
    @{ Text = "$header`nCODEX_CLI_PATH = `"C:\nfile`""; Error = 'control character'; Label = 'Escaped control in path' },
    @{ Text = 'note = """unterminated'; Error = 'Unclosed TOML'; Label = 'Unclosed multiline' },
    @{ Text = "note = `"bad\`ncontinuation`"`n$valid"; Error = 'Escaped newline'; Label = 'Single-line escaped newline' },
    @{ Text = 'note = ['; Error = 'Unclosed TOML'; Label = 'Unclosed array' },
    @{ Text = 'note = ]'; Error = 'Unbalanced'; Label = 'Unexpected closer' },
    @{ Text = "[mcp_servers.node_repl.env`n$selector"; Error = 'Unclosed TOML'; Label = 'Unclosed header' }
)) {
    Assert-BindingRefused -Text $case.Text -Pattern $case.Error -Label $case.Label
}

# Writers retain every original byte outside the selected literal, including
# UTF-8 BOM, mixed line endings, indentation, quoted key and inline comment.
$before = [string][char]0xFEFF + "# head`r`n[other]`nCODEX_CLI_PATH='leave me'`r`n$header # section`r`n  `"CODEX_CLI_PATH`"  =  `"C:\\old\\codex.exe`"  # keep comment`r`nTAIL = 'yes'"
$expected = $before.Replace('"C:\\old\\codex.exe"', "'C:\new\codex.exe'")
Assert-BindingEqual $expected (Get-CodexCliPathUpdatedText -Text $before -Entrypoint 'C:\new\codex.exe' -Description 'writer fixture') 'Byte-preserving writer'
Assert-BindingEqual 'C:\new\codex.exe' (Get-CodexCliPathFromText -Text $expected -Description 'round trip') 'Written value binds again'
Assert-BindingEqual 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855' (Get-CodexDevSha256Bytes ([byte[]]@())) 'Baseline SHA256'

$testRoot = Join-Path ([IO.Path]::GetTempPath()) ('codex-selector-binding-' + [guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $testRoot
try {
    $path = Join-Path $testRoot 'config.toml'
    $bytes = [Text.UTF8Encoding]::new($false).GetBytes($before)
    [IO.File]::WriteAllBytes($path, $bytes)
    $transition = Get-CodexCliConfigTransition -ConfigPath $path -Entrypoint 'C:\new\codex.exe'
    Assert-BindingEqual $before ([Text.UTF8Encoding]::new($false).GetString([IO.File]::ReadAllBytes($path))) 'Transition preparation never writes'
    Assert-BindingEqual $expected ([Text.UTF8Encoding]::new($false).GetString($transition.AfterBytes)) 'Transition exact after bytes'
    Assert-BindingEqual (Get-CodexDevSha256Bytes $bytes) $transition.BeforeSha256 'Transition before hash'
    [IO.File]::WriteAllText($path, '[wrong]' + "`n" + $selector, [Text.UTF8Encoding]::new($false))
    Assert-BindingEqual $null (Read-CodexCliConfigSnapshot $path).ConfiguredEntrypoint 'Snapshot does not bind wrong owner'
    [IO.File]::WriteAllBytes($path, [byte[]]@(0xFF, 0xFF))
    $snapshot = Read-CodexCliConfigSnapshot $path
    Assert-BindingEqual $true ($null -ne $snapshot.Error) 'Invalid UTF-8 is not missing selector'
} finally {
    # The test creates this exact unique directory and removes only its files.
    Remove-Item -LiteralPath (Join-Path $testRoot 'config.toml') -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $testRoot
}

Write-Output "deployment_config_binding_tests: $script:Checks checks passed (PowerShell $($PSVersionTable.PSVersion))."
