#Requires -Version 5.1
<#
.SYNOPSIS
  Installs sct — the local-first SNOMED CT toolchain — on Windows.

.DESCRIPTION
  Downloads the latest prebuilt sct.exe from GitHub Releases, verifies its
  SHA-256 checksum, and installs it to $env:LOCALAPPDATA\sct\bin by default.
  Prompts to add the install directory to the user PATH if it is not already
  present.

.PARAMETER InstallDir
  Override install directory. Default: $env:LOCALAPPDATA\sct\bin

.PARAMETER Version
  Install a specific version tag, e.g. v0.3.9. Default: latest.

.EXAMPLE
  # One-liner install (PowerShell):
  iwr -useb https://raw.githubusercontent.com/pacharanero/sct/main/install.ps1 | iex

.EXAMPLE
  # Install a specific version to a custom directory:
  $env:SCT_VERSION = 'v0.3.9'
  $env:SCT_INSTALL_DIR = 'C:\tools\sct'
  iwr -useb https://raw.githubusercontent.com/pacharanero/sct/main/install.ps1 | iex
#>

[CmdletBinding()]
param(
    [string]$InstallDir = $env:SCT_INSTALL_DIR,
    [string]$Version    = $env:SCT_VERSION
)

$ErrorActionPreference = 'Stop'
$ProgressPreference    = 'SilentlyContinue'  # Speeds up Invoke-WebRequest

$Repo = 'pacharanero/sct'

if ([string]::IsNullOrWhiteSpace($InstallDir)) {
    $InstallDir = Join-Path $env:LOCALAPPDATA 'sct\bin'
}

function Info($msg) { Write-Host $msg }
function Die($msg)  { Write-Host "error: $msg" -ForegroundColor Red; exit 1 }

# --- Architecture ---------------------------------------------------------
$arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
if ($arch -ne 'X64') {
    Die "unsupported Windows architecture: $arch (only x86_64 is currently supported)"
}
$target = 'windows-x86_64'

# --- Latest version -------------------------------------------------------
if ([string]::IsNullOrWhiteSpace($Version)) {
    Info 'Looking up latest sct release...'
    try {
        $release = Invoke-RestMethod -UseBasicParsing `
            -Uri "https://api.github.com/repos/$Repo/releases/latest"
        $Version = $release.tag_name
    } catch {
        Die "could not fetch latest release tag: $_"
    }
}
if ([string]::IsNullOrWhiteSpace($Version)) {
    Die 'could not determine latest version'
}
Info "Installing sct $Version for $target"

# --- Download -------------------------------------------------------------
$archive       = "sct-$target.zip"
$url           = "https://github.com/$Repo/releases/download/$Version/$archive"
$checksumsUrl  = "https://github.com/$Repo/releases/download/$Version/SHA256SUMS"

$tmpDir = Join-Path ([System.IO.Path]::GetTempPath()) ("sct-install-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $tmpDir | Out-Null

try {
    $zipPath       = Join-Path $tmpDir $archive
    $checksumsPath = Join-Path $tmpDir 'SHA256SUMS'

    Info "Downloading $archive..."
    Invoke-WebRequest -UseBasicParsing -Uri $url          -OutFile $zipPath
    Invoke-WebRequest -UseBasicParsing -Uri $checksumsUrl -OutFile $checksumsPath

    # --- Verify SHA-256 ---------------------------------------------------
    Info 'Verifying SHA-256 checksum...'
    $expected = $null
    foreach ($line in Get-Content $checksumsPath) {
        $parts = $line -split '\s+', 2
        if ($parts.Count -eq 2 -and $parts[1].Trim() -eq $archive) {
            $expected = $parts[0].ToLowerInvariant()
            break
        }
    }
    if (-not $expected) { Die "checksum for $archive not found in SHA256SUMS" }

    $actual = (Get-FileHash -Algorithm SHA256 -Path $zipPath).Hash.ToLowerInvariant()
    if ($expected -ne $actual) {
        Die "checksum mismatch:`n  expected: $expected`n  got:      $actual"
    }
    Info 'Checksum OK'

    # --- Extract and install ---------------------------------------------
    Info 'Extracting...'
    $extractDir = Join-Path $tmpDir 'extract'
    Expand-Archive -Path $zipPath -DestinationPath $extractDir -Force

    $exe = Join-Path $extractDir 'sct.exe'
    if (-not (Test-Path $exe)) { Die 'sct.exe not found in archive' }

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Move-Item -Force -Path $exe -Destination (Join-Path $InstallDir 'sct.exe')

    Info ''
    Info "sct installed to $InstallDir\sct.exe"
    Info ''
} finally {
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $tmpDir
}

# --- PATH -----------------------------------------------------------------
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (-not ($userPath -split ';' | Where-Object { $_ -and (Resolve-Path -ErrorAction SilentlyContinue $_).Path -eq (Resolve-Path $InstallDir).Path })) {
    Info "$InstallDir is not on your user PATH."
    $reply = Read-Host 'Add it now? [Y/n]'
    if ([string]::IsNullOrWhiteSpace($reply) -or $reply -match '^[Yy]') {
        $newPath = if ([string]::IsNullOrEmpty($userPath)) { $InstallDir } else { "$userPath;$InstallDir" }
        [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
        Info 'PATH updated. Open a new terminal for the change to take effect.'
    } else {
        Info 'Skipped. To add it manually later, run:'
        Info "  [Environment]::SetEnvironmentVariable('Path', `"`$env:Path;$InstallDir`", 'User')"
    }
}

try {
    & (Join-Path $InstallDir 'sct.exe') --version
} catch {
    # ignore — user may need to open a new shell first
}
