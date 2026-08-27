function Get-CodexDevToolStatus {
    $justError = $null
    $justVersion = $null
    $just = try { Resolve-CodexDevJust } catch { $justError = $_.Exception.Message; $null }
    if (-not [string]::IsNullOrWhiteSpace($just)) {
        try {
            $justVersion = (Invoke-CodexDevNative -FilePath $just -ArgumentList @("--version") -Capture) -join "`n"
        } catch {
            $justError = $_.Exception.Message
        }
    }
    $uvError = $null
    $uvVersion = $null
    $uv = try { Resolve-CodexDevUv } catch { $uvError = $_.Exception.Message; $null }
    if (-not [string]::IsNullOrWhiteSpace($uv)) {
        try {
            $uvVersion = (Invoke-CodexDevNative -FilePath $uv -ArgumentList @("--version") -Capture) -join "`n"
        } catch {
            $uvError = $_.Exception.Message
        }
    }
    $dotslashError = $null
    $dotslashVersion = $null
    $dotslash = try { Resolve-CodexDevDotslash } catch { $dotslashError = $_.Exception.Message; $null }
    if (-not [string]::IsNullOrWhiteSpace($dotslash)) {
        try {
            $dotslashVersion = (Invoke-CodexDevNative -FilePath $dotslash -ArgumentList @("--version") -Capture) -join "`n"
        } catch {
            $dotslashError = $_.Exception.Message
        }
    }
    return [pscustomobject]@{
        Root = Get-CodexDevLocalToolRoot
        Just = $just
        JustVersion = $justVersion
        JustError = $justError
        Uv = $uv
        UvVersion = $uvVersion
        UvError = $uvError
        Dotslash = $dotslash
        DotslashVersion = $dotslashVersion
        DotslashError = $dotslashError
        FormatReady = [string]::IsNullOrWhiteSpace($justError) -and
            [string]::IsNullOrWhiteSpace($uvError) -and
            [string]::IsNullOrWhiteSpace($dotslashError) -and
            -not [string]::IsNullOrWhiteSpace($just) -and
            -not [string]::IsNullOrWhiteSpace($uv) -and
            -not [string]::IsNullOrWhiteSpace($dotslash)
    }
}

function Resolve-CodexDevInstalledUvForExposure {
    param([string]$Root)

    return @(
        (Join-Path $Root "python\bin\uv.exe"),
        (Join-Path $Root "python\uv\uv.exe")
    ) | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
}

function Invoke-CodexDevToolSetup {
    [CmdletBinding(SupportsShouldProcess = $true)]
    param(
        [string]$CargoPath,
        [string]$CargoHome,
        [string]$RustupHome,
        [string]$PythonPath
    )

    $environment = Initialize-CodexDevEnvironment -CargoPath $CargoPath -CargoHome $CargoHome -RustupHome $RustupHome -PythonPath $PythonPath
    $root = Get-CodexDevLocalToolRoot
    $before = Get-CodexDevToolStatus
    $planned = @()

    if ([string]::IsNullOrWhiteSpace($before.Dotslash) -or
        -not [string]::IsNullOrWhiteSpace($before.DotslashError)) {
        $planned += if ([string]::IsNullOrWhiteSpace($before.Dotslash)) { "dotslash" } else { "dotslash-repair" }
        if ($PSCmdlet.ShouldProcess($root, "Install DotSlash with Cargo")) {
            New-Item -ItemType Directory -Path $root -Force | Out-Null
            $dotslashArguments = @("install", "--locked")
            if (-not [string]::IsNullOrWhiteSpace($before.DotslashError)) {
                $dotslashArguments += "--force"
            }
            $dotslashArguments += @("--root", $root, "dotslash")
            Invoke-CodexDevNative -FilePath $environment.Cargo -ArgumentList $dotslashArguments
        }
    }
    if ([string]::IsNullOrWhiteSpace($before.Uv) -or
        -not [string]::IsNullOrWhiteSpace($before.UvError)) {
        $planned += if ([string]::IsNullOrWhiteSpace($before.Uv)) { "uv" } else { "uv-repair" }
        if ($PSCmdlet.ShouldProcess($root, "Install uv with Python pip")) {
            $pythonTools = Join-Path $root "python"
            New-Item -ItemType Directory -Path $pythonTools -Force | Out-Null
            Invoke-CodexDevNative -FilePath $environment.Python -ArgumentList @(
                "-m", "pip", "install",
                "--disable-pip-version-check",
                "--no-compile",
                "--upgrade",
                "--target", $pythonTools,
                "uv"
            )
        }
    }

    $installedUv = Resolve-CodexDevInstalledUvForExposure $root
    $localUv = Join-Path $root "bin\uv.exe"
    if (-not [string]::IsNullOrWhiteSpace($installedUv) -and
        (-not (Test-Path -LiteralPath $localUv -PathType Leaf) -or
        -not [string]::IsNullOrWhiteSpace($before.UvError))) {
        $planned += "uv-exposure"
        if ($PSCmdlet.ShouldProcess($localUv, "Expose the locally installed uv binary")) {
            New-Item -ItemType Directory -Path (Split-Path -Parent $localUv) -Force | Out-Null
            Copy-Item -LiteralPath $installedUv -Destination $localUv -Force
        }
    }
    $pythonTools = Join-Path $root "python"
    if ((Test-Path -LiteralPath $localUv -PathType Leaf) -and
        (Test-Path -LiteralPath $pythonTools -PathType Container)) {
        $planned += "uv-tree-cleanup"
        if ($PSCmdlet.ShouldProcess($pythonTools, "Remove the redundant uv installation tree")) {
            $resolvedPythonTools = [System.IO.Path]::GetFullPath($pythonTools)
            if (-not $resolvedPythonTools.StartsWith($root, [System.StringComparison]::OrdinalIgnoreCase)) {
                throw "Refusing to remove a tool directory outside the local tool root: $resolvedPythonTools"
            }
            Remove-Item -LiteralPath $resolvedPythonTools -Recurse -Force
        }
    }

    Add-CodexDevPathSegment (Join-Path $root "bin")
    Add-CodexDevPathSegment (Join-Path $root "python\bin")
    $after = Get-CodexDevToolStatus
    $problems = @(
        foreach ($tool in @(
            @{ Name = "just"; Path = $after.Just; Error = $after.JustError },
            @{ Name = "uv"; Path = $after.Uv; Error = $after.UvError },
            @{ Name = "dotslash"; Path = $after.Dotslash; Error = $after.DotslashError }
        )) {
            if ([string]::IsNullOrWhiteSpace([string]$tool.Path)) {
                "$($tool.Name) missing"
            } elseif (-not [string]::IsNullOrWhiteSpace([string]$tool.Error)) {
                "$($tool.Name) invalid: $($tool.Error)"
            }
        }
    )
    if (-not $WhatIfPreference -and -not $after.FormatReady) {
        throw "Local tool setup finished without a complete format toolchain. $($problems -join ' | ')"
    }
    $versions = [ordered]@{
        Just = $after.JustVersion
        Uv = $after.UvVersion
        Dotslash = $after.DotslashVersion
    }
    $receipt = $null
    $receiptError = $null
    if (-not $WhatIfPreference -and @($planned).Count -gt 0) {
        try {
            $receipt = Write-CodexDevReceipt -Action "Setup" -Details ([ordered]@{
                Planned = $planned
                Tooling = $after
                Versions = $versions
            })
        } catch {
            $receiptError = $_.Exception.Message
        }
    }
    return [pscustomobject]@{
        Status = if ($after.FormatReady -and @($planned).Count -eq 0) {
            "already_ready"
        } elseif ($WhatIfPreference -and @($planned).Count -gt 0) {
            "planned"
        } elseif ($WhatIfPreference) {
            "needs_prerequisite"
        } else {
            "ready"
        }
        Planned = $planned
        Problems = $problems
        Before = $before
        After = $after
        Versions = $versions
        Receipt = $receipt
        ReceiptError = $receiptError
    }
}
