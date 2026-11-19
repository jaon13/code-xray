<#
.SYNOPSIS
One-step Windows installer for CodeXray (pre-built Release zip or source build).
Usage:
  irm https://raw.githubusercontent.com/jaon13/code-xray/main/install.ps1 | iex
#>
param(
    [string]$InstallDir = $(if ($env:CODEXRAY_HOME) { $env:CODEXRAY_HOME } else { Join-Path $HOME ".agents\code-xray" }),
    [string]$Repo = "jaon13/code-xray",
    [switch]$FromSource
)

$ErrorActionPreference = "Stop"

$pyCmd = Get-Command python -ErrorAction SilentlyContinue
if (-not $pyCmd) { $pyCmd = Get-Command python3 -ErrorAction SilentlyContinue }
if (-not $pyCmd) {
    throw "Python 3.10+ is required to install CodeXray."
}

$arch = if ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq [System.Runtime.InteropServices.Architecture]::Arm64) {
    "arm64"
} else {
    "amd64"
}
$assetName = "CodeXray-windows-$arch.zip"
$installedFromRelease = $false

New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null

if (-not $FromSource -and $env:CODEXRAY_FROM_SOURCE -ne "1") {
    try {
        $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -Headers @{ "User-Agent" = "code-xray-installer" }
        $asset = $release.assets | Where-Object { $_.name -eq $assetName } | Select-Object -First 1
        if ($asset) {
            $tmpZip = Join-Path ([System.IO.Path]::GetTempPath()) "code-xray-$PID.zip"
            Write-Host "[code-xray] Downloading prebuilt release ($assetName) ..."
            Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $tmpZip
            Expand-Archive -Path $tmpZip -DestinationPath $InstallDir -Force
            Remove-Item $tmpZip -Force -ErrorAction SilentlyContinue
            & $pyCmd.Source (Join-Path $InstallDir "scripts\publish_tools.py") --skip-rust --skip-dotnet --register-mcp --install-wrappers
            if ($LASTEXITCODE -ne 0) { throw "publish_tools.py failed ($LASTEXITCODE)" }
            $installedFromRelease = $true
        }
    } catch {
        Write-Host "[code-xray] Prebuilt release unavailable ($($_.Exception.Message)); falling back to source install."
    }
}

if (-not $installedFromRelease) {
    $gitCmd = Get-Command git -ErrorAction SilentlyContinue
    if ($gitCmd) {
        if (Test-Path (Join-Path $InstallDir ".git")) {
            Write-Host "[code-xray] Updating $InstallDir ..."
            git -C $InstallDir pull --ff-only
        } else {
            if (Test-Path $InstallDir) {
                Get-ChildItem $InstallDir -Force | Remove-Item -Recurse -Force
            }
            Write-Host "[code-xray] Cloning https://github.com/$Repo.git -> $InstallDir ..."
            git clone --depth 1 "https://github.com/$Repo.git" $InstallDir
        }
    } else {
        $tmpZip = Join-Path ([System.IO.Path]::GetTempPath()) "code-xray-src-$PID.zip"
        $tmpExtract = Join-Path ([System.IO.Path]::GetTempPath()) "code-xray-src-$PID"
        Invoke-WebRequest -Uri "https://github.com/$Repo/archive/refs/heads/main.zip" -OutFile $tmpZip
        Expand-Archive -Path $tmpZip -DestinationPath $tmpExtract -Force
        $inner = Get-ChildItem $tmpExtract -Directory | Select-Object -First 1
        Copy-Item (Join-Path $inner.FullName "*") $InstallDir -Recurse -Force
        Remove-Item $tmpZip, $tmpExtract -Recurse -Force -ErrorAction SilentlyContinue
    }

    if (-not (Get-Command cargo -ErrorAction SilentlyContinue) -or -not (Get-Command dotnet -ErrorAction SilentlyContinue)) {
        throw "Building from source requires Rust (cargo) and .NET 10 SDK (dotnet). Install them or download a tagged GitHub Release."
    }

    & $pyCmd.Source (Join-Path $InstallDir "scripts\publish_tools.py") --skip-tests --register-mcp --install-wrappers
    if ($LASTEXITCODE -ne 0) { throw "publish_tools.py failed ($LASTEXITCODE)" }
}

Write-Host "[code-xray] Installed at $InstallDir"
Write-Host "[code-xray] Restart Antigravity / Cursor to reload the 'code-xray' MCP server."
