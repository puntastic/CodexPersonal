$ErrorActionPreference = "Stop"

$startupArguments = @()
if (-not [string]::IsNullOrWhiteSpace($env:CODEX_BAZEL_OUTPUT_USER_ROOT)) {
    $startupArguments += "--output_user_root=$env:CODEX_BAZEL_OUTPUT_USER_ROOT"
}

& $env:BAZEL_REAL @startupArguments @args
exit $LASTEXITCODE
