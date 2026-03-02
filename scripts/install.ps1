<#
.SYNOPSIS
    Install jcode on Windows.
.DESCRIPTION
    Downloads the latest jcode release and installs it to %LOCALAPPDATA%\jcode\bin.

    One-liner install:
      irm https://raw.githubusercontent.com/1jehuang/jcode/master/scripts/install.ps1 | iex

    Or download and run (allows parameters):
      & ([scriptblock]::Create((irm https://raw.githubusercontent.com/1jehuang/jcode/master/scripts/install.ps1)))
.PARAMETER InstallDir
    Override the installation directory (default: $env:LOCALAPPDATA\jcode\bin)
#>
param(
    [string]$InstallDir
)

$ErrorActionPreference = 'Stop'

if ($PSVersionTable.PSVersion.Major -lt 5) {
    Write-Host "error: PowerShell 5.1 or later is required" -ForegroundColor Red
    exit 1
}

$Repo = "1jehuang/jcode"
$Artifact = "jcode-windows-x86_64"

if (-not $InstallDir) {
    $InstallDir = Join-Path $env:LOCALAPPDATA "jcode\bin"
}

function Write-Info($msg) { Write-Host $msg -ForegroundColor Blue }
function Write-Err($msg) { Write-Host "error: $msg" -ForegroundColor Red; exit 1 }

$Arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
if ($Arch -ne 'X64') {
    Write-Err "Unsupported architecture: $Arch (only x86_64 supported)"
}

Write-Info "Fetching latest release..."
try {
    $Release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest"
    $Version = $Release.tag_name
} catch {
    Write-Err "Failed to determine latest version: $_"
}

if (-not $Version) { Write-Err "Failed to determine latest version" }

$VersionNum = $Version.TrimStart('v')
$TgzUrl = "https://github.com/$Repo/releases/download/$Version/$Artifact.tar.gz"
$ExeUrl = "https://github.com/$Repo/releases/download/$Version/$Artifact.exe"

$BuildsDir = Join-Path $env:LOCALAPPDATA "jcode\builds"
$StableDir = Join-Path $BuildsDir "stable"
$VersionDir = Join-Path $BuildsDir "versions\$VersionNum"
$LauncherPath = Join-Path $InstallDir "jcode.exe"

$Existing = ""
if (Test-Path $LauncherPath) {
    try { $Existing = & $LauncherPath --version 2>$null | Select-Object -First 1 } catch {}
}

if ($Existing) {
    if ($Existing -match [regex]::Escape($VersionNum)) {
        Write-Info "jcode $Version is already installed - reinstalling"
    } else {
        Write-Info "Updating jcode $Existing -> $Version"
    }
} else {
    Write-Info "Installing jcode $Version"
}
Write-Info "  launcher: $LauncherPath"

foreach ($d in @($InstallDir, $StableDir, $VersionDir)) {
    if (-not (Test-Path $d)) { New-Item -ItemType Directory -Path $d -Force | Out-Null }
}

$TempDir = Join-Path $env:TEMP "jcode-install-$(Get-Random)"
New-Item -ItemType Directory -Path $TempDir -Force | Out-Null

$DownloadMode = ""
$DownloadPath = Join-Path $TempDir "jcode.download"

try {
    Write-Info "Downloading $Artifact.tar.gz..."
    Invoke-WebRequest -Uri $TgzUrl -OutFile $DownloadPath -UseBasicParsing
    $DownloadMode = "tar"
} catch {
    try {
        Write-Info "Trying direct binary download..."
        Invoke-WebRequest -Uri $ExeUrl -OutFile $DownloadPath -UseBasicParsing
        $DownloadMode = "bin"
    } catch {
        $DownloadMode = ""
    }
}

$DestBin = Join-Path $VersionDir "jcode.exe"

if ($DownloadMode -eq "tar") {
    Write-Info "Extracting..."
    tar xzf $DownloadPath -C $TempDir 2>$null
    $SrcBin = Join-Path $TempDir "$Artifact.exe"
    if (-not (Test-Path $SrcBin)) {
        Write-Err "Downloaded archive did not contain expected binary: $Artifact.exe"
    }
    Move-Item -Path $SrcBin -Destination $DestBin -Force
} elseif ($DownloadMode -eq "bin") {
    Move-Item -Path $DownloadPath -Destination $DestBin -Force
} else {
    Write-Info "No prebuilt asset found for $Artifact in $Version; building from source..."
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) { Write-Err "git is required to build from source" }
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { Write-Err "cargo is required to build from source" }

    $SrcDir = Join-Path $TempDir "jcode-src"
    git clone --depth 1 --branch $Version "https://github.com/$Repo.git" $SrcDir
    if ($LASTEXITCODE -ne 0) { Write-Err "Failed to clone $Repo at $Version" }

    cargo build --release --manifest-path (Join-Path $SrcDir "Cargo.toml")
    if ($LASTEXITCODE -ne 0) { Write-Err "cargo build failed" }

    $BuiltBin = Join-Path $SrcDir "target\release\jcode.exe"
    if (-not (Test-Path $BuiltBin)) { Write-Err "Built binary not found at $BuiltBin" }
    Copy-Item -Path $BuiltBin -Destination $DestBin -Force
}

Copy-Item -Path $DestBin -Destination (Join-Path $StableDir "jcode.exe") -Force
Set-Content -Path (Join-Path $BuildsDir "stable-version") -Value $VersionNum
Copy-Item -Path (Join-Path $StableDir "jcode.exe") -Destination $LauncherPath -Force

Remove-Item -Path $TempDir -Recurse -Force -ErrorAction SilentlyContinue

$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($UserPath -notlike "*$InstallDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$InstallDir;$UserPath", "User")
    Write-Info "Added $InstallDir to user PATH"
}

$env:Path = "$InstallDir;$env:Path"

Write-Host ""
Write-Info "jcode $Version installed successfully!"
Write-Host ""

if (Get-Command jcode -ErrorAction SilentlyContinue) {
    Write-Info "Run 'jcode' to get started."
} else {
    Write-Host "  Open a new terminal window, then run:"
    Write-Host ""
    Write-Host "    jcode" -ForegroundColor Green
}
