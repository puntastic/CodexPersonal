function Add-CodexDevPathSegment {
    param([string]$Path)

    if ([string]::IsNullOrWhiteSpace($Path) -or -not (Test-Path -LiteralPath $Path -PathType Container)) {
        return
    }
    $fullPath = [System.IO.Path]::GetFullPath($Path)
    $present = @(@($env:PATH -split ";") | Where-Object {
        -not [string]::IsNullOrWhiteSpace($_) -and
        [System.IO.Path]::GetFullPath($_).TrimEnd("\") -ieq $fullPath.TrimEnd("\")
    })
    if ($present.Count -eq 0) {
        $env:PATH = "$fullPath;$env:PATH"
    }
}

function Get-CodexDevLocalToolRoot {
    return Join-Path $script:CodexDevRepositoryRoot ".tooling\repo-tools"
}

function Get-CodexDevHostTarget {
    switch ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture) {
        "X64" { return "x86_64-pc-windows-msvc" }
        "Arm64" { return "aarch64-pc-windows-msvc" }
        default { throw "Unsupported Windows architecture: $([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture)" }
    }
}

function Get-CodexDevMsvcArchitecture {
    param([string]$HostTarget = (Get-CodexDevHostTarget))

    switch ($HostTarget) {
        "x86_64-pc-windows-msvc" { return "x64" }
        "aarch64-pc-windows-msvc" { return "arm64" }
        default { throw "Unsupported MSVC host target: $HostTarget" }
    }
}

function Get-CodexDevMsvcRequiredComponent {
    param(
        [ValidateSet("x64", "arm64")]
        [string]$Architecture = (Get-CodexDevMsvcArchitecture)
    )

    switch ($Architecture) {
        "x64" { return "Microsoft.VisualStudio.Component.VC.Tools.x86.x64" }
        "arm64" { return "Microsoft.VisualStudio.Component.VC.Tools.ARM64" }
    }
}

function Get-CodexDevMsvcToolDirectory {
    param(
        [ValidateSet("x64", "arm64")]
        [string]$Architecture = (Get-CodexDevMsvcArchitecture),
        [string]$VcToolsInstallDir = $env:VCToolsInstallDir
    )

    if ([string]::IsNullOrWhiteSpace($VcToolsInstallDir)) {
        throw "VCToolsInstallDir is not set. Import a matching Visual Studio developer environment first."
    }

    $toolRoot = [System.IO.Path]::GetFullPath($VcToolsInstallDir).TrimEnd("\")
    $toolDirectory = [System.IO.Path]::GetFullPath(
        (Join-Path $toolRoot "bin\Host$Architecture\$Architecture")
    )
    $rootBoundary = "$toolRoot\"
    if (-not $toolDirectory.StartsWith($rootBoundary, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Resolved MSVC tool directory escapes VCToolsInstallDir: $toolDirectory"
    }
    return $toolDirectory
}

function Resolve-CodexDevMsvcToolset {
    param(
        [ValidateSet("x64", "arm64")]
        [string]$Architecture = (Get-CodexDevMsvcArchitecture),
        [string]$VcToolsInstallDir = $env:VCToolsInstallDir
    )

    $toolDirectory = Get-CodexDevMsvcToolDirectory `
        -Architecture $Architecture `
        -VcToolsInstallDir $VcToolsInstallDir
    $cl = Join-Path $toolDirectory "cl.exe"
    $link = Join-Path $toolDirectory "link.exe"
    foreach ($tool in @($cl, $link)) {
        if (-not (Test-Path -LiteralPath $tool -PathType Leaf)) {
            throw "The selected MSVC environment does not provide the expected tool: $tool"
        }
    }

    return [pscustomobject]@{
        Cl = [System.IO.Path]::GetFullPath($cl)
        Link = [System.IO.Path]::GetFullPath($link)
        ToolDirectory = $toolDirectory
    }
}

function Test-CodexDevMsvcEnvironment {
    param(
        [ValidateSet("x64", "arm64")]
        [string]$Architecture = (Get-CodexDevMsvcArchitecture)
    )

    if ([string]::IsNullOrWhiteSpace($env:VCToolsInstallDir) -or
        [string]::IsNullOrWhiteSpace($env:VSCMD_ARG_HOST_ARCH) -or
        [string]::IsNullOrWhiteSpace($env:VSCMD_ARG_TGT_ARCH) -or
        $env:VSCMD_ARG_HOST_ARCH -ine $Architecture -or
        $env:VSCMD_ARG_TGT_ARCH -ine $Architecture) {
        return $false
    }

    try {
        $null = Resolve-CodexDevMsvcToolset -Architecture $Architecture
        return $true
    } catch {
        return $false
    }
}

function Import-CodexDevEnvironmentRows {
    param([string[]]$Rows)

    $pathValue = $null
    foreach ($row in $Rows) {
        $separator = $row.IndexOf("=")
        if ($separator -lt 1) {
            continue
        }
        $name = $row.Substring(0, $separator)
        $value = $row.Substring($separator + 1)
        if ($name -ieq "Path") {
            $pathValue = $value
            continue
        }
        [Environment]::SetEnvironmentVariable($name, $value, "Process")
    }
    if ($null -eq $pathValue) {
        return $false
    }
    $env:PATH = $pathValue
    return $true
}

function Get-CodexDevWindowsExecutableTarget {
    param([string]$Path)

    $stream = [System.IO.File]::OpenRead($Path)
    $reader = [System.IO.BinaryReader]::new($stream)
    try {
        if ($stream.Length -lt 64 -or $reader.ReadUInt16() -ne 0x5A4D) {
            throw "Executable does not have a valid DOS header: $Path"
        }
        $stream.Position = 0x3C
        $peOffset = $reader.ReadUInt32()
        if ($peOffset -gt $stream.Length - 6) {
            throw "Executable has an invalid PE header offset: $Path"
        }
        $stream.Position = $peOffset
        if ($reader.ReadUInt32() -ne 0x00004550) {
            throw "Executable does not have a valid PE header: $Path"
        }
        switch ($reader.ReadUInt16()) {
            0x8664 { return "x86_64-pc-windows-msvc" }
            0xAA64 { return "aarch64-pc-windows-msvc" }
            default { throw "Executable uses an unsupported PE machine: $Path" }
        }
    } finally {
        $reader.Dispose()
        $stream.Dispose()
    }
}

function Import-CodexDevMsvcEnvironment {
    param(
        [ValidateSet("x64", "arm64")]
        [string]$Architecture = (Get-CodexDevMsvcArchitecture)
    )

    if (Test-CodexDevMsvcEnvironment -Architecture $Architecture) {
        return $env:VSINSTALLDIR
    }

    $vsDevCmd = $null
    $requiredComponent = Get-CodexDevMsvcRequiredComponent -Architecture $Architecture
    $vsWhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path -LiteralPath $vsWhere -PathType Leaf) {
        $installPath = @(& $vsWhere -latest -products * -requires $requiredComponent -property installationPath) |
            Select-Object -First 1
        if (-not [string]::IsNullOrWhiteSpace([string]$installPath)) {
            $candidate = Join-Path ([string]$installPath) "Common7\Tools\VsDevCmd.bat"
            if (Test-Path -LiteralPath $candidate -PathType Leaf) {
                $vsDevCmd = $candidate
            }
        }
    }
    if ($null -eq $vsDevCmd) {
        foreach ($edition in @("Community", "Professional", "Enterprise", "BuildTools")) {
            $candidate = Join-Path $env:ProgramFiles "Microsoft Visual Studio\2022\$edition\Common7\Tools\VsDevCmd.bat"
            if (Test-Path -LiteralPath $candidate -PathType Leaf) {
                $vsDevCmd = $candidate
                break
            }
        }
    }
    if ($null -eq $vsDevCmd) {
        throw "Visual Studio C++ build tools were not found. Run codex-rs/scripts/setup-windows.ps1 or install the MSVC workload."
    }

    $commandLine = "call `"$vsDevCmd`" -no_logo -arch=$Architecture -host_arch=$Architecture >nul && set"
    $environmentRows = @(& $env:ComSpec /d /s /c $commandLine)
    if ($LASTEXITCODE -ne 0) {
        throw "VsDevCmd failed with exit code $LASTEXITCODE`: $vsDevCmd"
    }

    if (-not (Import-CodexDevEnvironmentRows -Rows $environmentRows)) {
        throw "VsDevCmd did not return a Path environment row: $vsDevCmd"
    }
    if (-not (Test-CodexDevMsvcEnvironment -Architecture $Architecture)) {
        throw "VsDevCmd did not produce a complete $Architecture MSVC environment with cl.exe and link.exe under VCToolsInstallDir: $vsDevCmd"
    }
    return $vsDevCmd
}

function Resolve-CodexDevPython {
    param([string]$ExplicitPath)

    $python = Resolve-CodexDevFile -ExplicitPath $ExplicitPath -Candidates @(
        "C:\Python314\python.exe",
        "py.exe",
        "python.exe",
        "python"
    ) -Description "Python 3"
    $null = Invoke-CodexDevNative -FilePath $python -ArgumentList @(
        "-c", "import sys; raise SystemExit(0 if sys.version_info >= (3, 10) else 1)"
    ) -Capture
    return $python
}

function Resolve-CodexDevCargo {
    param([string]$ExplicitPath)

    return Resolve-CodexDevFile -ExplicitPath $ExplicitPath -Candidates @(
        (Join-Path $script:CodexDevRepositoryRoot ".tooling\cargo\bin\cargo.exe"),
        (Join-Path (Get-CodexDevUserProfile) ".cargo\bin\cargo.exe"),
        "cargo.exe",
        "cargo"
    ) -Description "Cargo"
}

function Resolve-CodexDevJust {
    return Resolve-CodexDevFile -Candidates @(
        (Join-Path $script:CodexDevRepositoryRoot ".tooling\cargo\bin\just.exe"),
        (Join-Path (Get-CodexDevUserProfile) ".cargo\bin\just.exe"),
        "just.exe",
        "just"
    ) -Description "just"
}

function Resolve-CodexDevUv {
    $localRoot = Get-CodexDevLocalToolRoot
    return Resolve-CodexDevFile -Candidates @(
        (Join-Path $localRoot "bin\uv.exe"),
        (Join-Path $localRoot "python\bin\uv.exe"),
        (Join-Path $localRoot "python\uv\uv.exe"),
        (Join-Path (Get-CodexDevUserProfile) ".local\bin\uv.exe"),
        "uv.exe",
        "uv"
    ) -Description "uv"
}

function Resolve-CodexDevDotslash {
    $localRoot = Get-CodexDevLocalToolRoot
    return Resolve-CodexDevFile -Candidates @(
        (Join-Path $localRoot "bin\dotslash.exe"),
        (Join-Path (Get-CodexDevUserProfile) ".cargo\bin\dotslash.exe"),
        "dotslash.exe",
        "dotslash"
    ) -Description "DotSlash"
}

function Resolve-CodexDevRipgrep {
    param([string]$ExplicitPath)

    $candidates = @()
    $installedBinRoot = Join-Path (Get-CodexDevLocalAppData) "OpenAI\Codex\bin"
    if (Test-Path -LiteralPath $installedBinRoot -PathType Container) {
        $candidates += @(Get-ChildItem -LiteralPath $installedBinRoot -Filter "rg.exe" -Recurse -File -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTime -Descending |
            Select-Object -ExpandProperty FullName)
    }
    if (-not [string]::IsNullOrWhiteSpace($env:CODEX_CLI_PATH)) {
        $packageRoot = Split-Path -Parent (Split-Path -Parent $env:CODEX_CLI_PATH)
        $candidates += Join-Path $packageRoot "codex-path\rg.exe"
    }
    $candidates += @("rg.exe", "rg")
    $ripgrep = Resolve-CodexDevFile -ExplicitPath $ExplicitPath -Candidates $candidates -Description "ripgrep"
    $actualTarget = Get-CodexDevWindowsExecutableTarget $ripgrep
    $expectedTarget = Get-CodexDevHostTarget
    if ($actualTarget -ne $expectedTarget) {
        throw "Ripgrep target '$actualTarget' does not match the host target '$expectedTarget': $ripgrep"
    }
    $null = Invoke-CodexDevNative -FilePath $ripgrep -ArgumentList @("--version") -Capture
    return $ripgrep
}

function Initialize-CodexDevEnvironment {
    param(
        [string]$CargoPath,
        [string]$CargoHome,
        [string]$RustupHome,
        [string]$PythonPath,
        [switch]$RequireJust
    )

    $cargo = Resolve-CodexDevCargo $CargoPath
    $python = Resolve-CodexDevPython $PythonPath
    $resolvedRustupHome = if (-not [string]::IsNullOrWhiteSpace($RustupHome)) {
        [System.IO.Path]::GetFullPath($RustupHome)
    } elseif (-not [string]::IsNullOrWhiteSpace($env:RUSTUP_HOME)) {
        $env:RUSTUP_HOME
    } elseif (Test-Path -LiteralPath (Join-Path $script:CodexDevRepositoryRoot ".tooling\rustup")) {
        Join-Path $script:CodexDevRepositoryRoot ".tooling\rustup"
    } else {
        Join-Path (Get-CodexDevUserProfile) ".rustup"
    }
    $resolvedCargoHome = if (-not [string]::IsNullOrWhiteSpace($CargoHome)) {
        [System.IO.Path]::GetFullPath($CargoHome)
    } elseif (-not [string]::IsNullOrWhiteSpace($env:CARGO_HOME)) {
        $env:CARGO_HOME
    } elseif (Test-Path -LiteralPath "C:\tmp\codex-cargo") {
        "C:\tmp\codex-cargo"
    } else {
        Join-Path (Get-CodexDevLocalAppData) "CodexPersonal\cargo"
    }

    $msvcArchitecture = Get-CodexDevMsvcArchitecture
    $vsDevCmd = Import-CodexDevMsvcEnvironment -Architecture $msvcArchitecture
    $msvcTools = Resolve-CodexDevMsvcToolset -Architecture $msvcArchitecture
    Add-CodexDevPathSegment $msvcTools.ToolDirectory
    $env:RUSTUP_HOME = $resolvedRustupHome
    $env:CARGO_HOME = $resolvedCargoHome
    $env:DOTSLASH_CACHE = Join-Path (Get-CodexDevLocalToolRoot) "cache\dotslash"
    $env:UV_CACHE_DIR = Join-Path (Get-CodexDevLocalToolRoot) "cache\uv"
    Add-CodexDevPathSegment (Join-Path (Get-CodexDevLocalToolRoot) "bin")
    Add-CodexDevPathSegment ([System.IO.Path]::GetDirectoryName($python))
    Add-CodexDevPathSegment ([System.IO.Path]::GetDirectoryName($cargo))
    $toolchainText = Get-Content -LiteralPath (Join-Path $script:CodexDevRepositoryRoot "codex-rs\rust-toolchain.toml") -Raw
    $channelMatch = [regex]::Match($toolchainText, '(?m)^channel\s*=\s*"(?<channel>[^"]+)"')
    if ($channelMatch.Success) {
        $toolchainBin = Join-Path $resolvedRustupHome "toolchains\$($channelMatch.Groups['channel'].Value)-$(Get-CodexDevHostTarget)\bin"
        if (Test-Path -LiteralPath (Join-Path $toolchainBin "rustc.exe") -PathType Leaf) {
            $env:RUSTC = Join-Path $toolchainBin "rustc.exe"
            $env:RUSTDOC = Join-Path $toolchainBin "rustdoc.exe"
            Add-CodexDevPathSegment $toolchainBin
        }
    }
    $rustc = Resolve-CodexDevFile -ExplicitPath $env:RUSTC -Candidates @(
        (Join-Path ([System.IO.Path]::GetDirectoryName($cargo)) "rustc.exe"),
        "rustc.exe",
        "rustc"
    ) -Description "rustc"
    $rustcVerbose = (Invoke-CodexDevNative -FilePath $rustc -ArgumentList @("-vV") -Capture) -join "`n"
    $rustcHostMatch = [regex]::Match($rustcVerbose, '(?m)^host:\s*(?<host>\S+)\s*$')
    if (-not $rustcHostMatch.Success -or $rustcHostMatch.Groups["host"].Value -ne (Get-CodexDevHostTarget)) {
        throw "rustc is not installed for the Windows Desktop host '$(Get-CodexDevHostTarget)'."
    }
    $cl = $msvcTools.Cl
    $link = $msvcTools.Link
    $just = if ($RequireJust) { Resolve-CodexDevJust } else { $null }
    $cargoVerbose = (Invoke-CodexDevNative -FilePath $cargo -ArgumentList @("-vV") -Capture) -join "`n"
    $cargoHostMatch = [regex]::Match($cargoVerbose, '(?m)^host:\s*(?<host>\S+)\s*$')
    if (-not $cargoHostMatch.Success) {
        throw "Cargo -vV did not report a host target."
    }
    $cargoHost = $cargoHostMatch.Groups["host"].Value
    if ($cargoHost -ne (Get-CodexDevHostTarget)) {
        throw "Cargo host '$cargoHost' does not match the Windows Desktop host '$(Get-CodexDevHostTarget)'."
    }
    $uv = try { Resolve-CodexDevUv } catch { $null }
    $dotslash = try { Resolve-CodexDevDotslash } catch { $null }
    if (-not [string]::IsNullOrWhiteSpace($uv)) {
        Add-CodexDevPathSegment ([System.IO.Path]::GetDirectoryName($uv))
    }
    if (-not [string]::IsNullOrWhiteSpace($dotslash)) {
        Add-CodexDevPathSegment ([System.IO.Path]::GetDirectoryName($dotslash))
    }

    return [pscustomobject]@{
        Cargo = $cargo
        CargoHost = $cargoHost
        CargoHome = $resolvedCargoHome
        RustupHome = $resolvedRustupHome
        Rustc = $rustc
        Python = $python
        Just = $just
        Uv = $uv
        Dotslash = $dotslash
        DotslashCache = $env:DOTSLASH_CACHE
        UvCache = $env:UV_CACHE_DIR
        VsDevCmd = $vsDevCmd
        Cl = $cl
        Link = $link
        HostTarget = Get-CodexDevHostTarget
    }
}
