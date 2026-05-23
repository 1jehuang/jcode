#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Install or update CarpAI (Code Analysis & Refactoring Platform for AI)

.DESCRIPTION
    Downloads and installs the latest CarpAI release from GitHub.
    Supports version specification, custom install paths, and offline installation.

.PARAMETER Version
    Specific version to install (e.g., "v0.5.0"). Defaults to latest stable.

.PARAMETER InstallDir
    Custom installation directory. Defaults to $env:LOCALAPPDATA\carpai

.PARAMETER Offline
    Use offline mode - install from local package instead of downloading.

.PARAMETER PackagePath
    Path to local .zip package (used with -Offline).

.PARAMETER Verbose
    Enable verbose output for debugging.

.EXAMPLE
    .\install.ps1
    Install latest stable version

.EXAMPLE
    .\install.ps1 -Version v0.5.0
    Install specific version

.EXAMPLE
    .\install.ps1 -InstallDir D:\Tools\CarpAI
    Install to custom directory

.EXAMPLE
    .\install.ps1 -Offline -PackagePath C:\Downloads\carpai-v0.5.0.zip
    Install from local package (offline mode)
#>

[CmdletBinding()]
param(
    [string]$Version = "",
    [string]$InstallDir = "",
    [switch]$Offline,
    [string]$PackagePath = "",
    [switch]$Verbose
)

# Colors for output
$RED = "`e[31m"
$GREEN = "`e[32m"
$YELLOW = "`e[33m"
$BLUE = "`e[34m"
$NC = "`e[0m" # No Color

function Write-Success {
    param([string]$Message)
    Write-Host "$GREEN✓ $Message$NC"
}

function Write-Error-Custom {
    param([string]$Message)
    Write-Host "$RED✗ $Message$NC"
}

function Write-Info {
    param([string]$Message)
    Write-Host "$BLUEℹ $Message$NC"
}

function Write-Warning-Custom {
    param([string]$Message)
    Write-Host "$YELLOW⚠ $Message$NC"
}

# Check if running as administrator
function Test-Administrator {
    $currentUser = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
    return $currentUser.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

# Check system requirements
function Test-SystemRequirements {
    Write-Info "Checking system requirements..."

    # Check Windows version (Windows 10 or later recommended)
    $osVersion = [Environment]::OSVersion.Version.Major
    if ($osVersion -lt 10) {
        Write-Warning-Custom "Windows 10 or later is recommended. Current version: $osVersion"
    }

    # Check PowerShell version
    $psVersion = $PSVersionTable.PSVersion.Major
    if ($psVersion -lt 5) {
        Write-Error-Custom "PowerShell 5.0 or later required. Current version: $psVersion"
        exit 1
    }

    # Check available disk space (minimum 500MB)
    $drive = Split-Path $env:TEMP -Qualifier
    $diskSpace = (Get-Volume -DriveLetter $drive.Substring(0,1)).SizeRemaining / 1GB
    if ($diskSpace -lt 0.5) {
        Write-Error-Custom "Insufficient disk space. Need at least 500MB. Available: $([math]::Round($diskSpace * 1024))MB"
        exit 1
    }

    Write-Success "System requirements met"
}

# Check dependencies
function Test-Dependencies {
    Write-Info "Checking dependencies..."

    $missingDeps = @()

    # Check Git (optional but recommended)
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
        $missingDeps += "Git (recommended for version control integration)"
    }

    # Check Docker Desktop (optional for containerized deployment)
    if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
        Write-Warning-Custom "Docker not found (optional for containerized mode)"
    }

    if ($missingDeps.Count -gt 0) {
        Write-Warning-Custom "Missing optional dependencies:"
        foreach ($dep in $missingDeps) {
            Write-Host "  - $dep"
        }
    } else {
        Write-Success "All dependencies satisfied"
    }
}

# Get latest release version from GitHub
function Get-LatestVersion {
    try {
        $releasesUrl = "https://api.github.com/repos/codecargo/carpai/releases/latest"
        $response = Invoke-RestMethod -Uri $releasesUrl -Headers @{
            "Accept" = "application/vnd.github.v3+json"
            "User-Agent" = "CarpAI-Installer"
        }
        return $response.tag_name
    } catch {
        Write-Error-Custom "Failed to fetch latest version: $_"
        exit 1
    }
}

# Download release package
function Download-Release {
    param([string]$version)

    $downloadUrl = "https://github.com/codecargo/carpai/releases/download/$version/carpai-$version-windows-x86_64.zip"
    $tempFile = Join-Path $env:TEMP "carpai-$version.zip"

    Write-Info "Downloading CarpAI $version..."
    Write-Info "URL: $downloadUrl"

    try {
        Invoke-WebRequest -Uri $downloadUrl -OutFile $tempFile -ProgressAction SilentlyContinue
        Write-Success "Download completed: $tempFile"
        return $tempFile
    } catch {
        Write-Error-Custom "Download failed: $_"
        exit 1
    }
}

# Extract package to install directory
function Install-Package {
    param([string]$packagePath, [string]$installDir)

    Write-Info "Installing to: $installDir"

    # Create install directory if it doesn't exist
    if (-not (Test-Path $installDir)) {
        New-Item -ItemType Directory -Path $installDir -Force | Out-Null
    }

    # Extract zip file
    try {
        Expand-Archive -Path $packagePath -DestinationPath $installDir -Force
        Write-Success "Package extracted"
    } catch {
        Write-Error-Custom "Extraction failed: $_"
        exit 1
    }

    # Verify installation
    $exePath = Join-Path $installDir "carpai.exe"
    if (-not (Test-Path $exePath)) {
        Write-Error-Custom "Installation verification failed: carpai.exe not found"
        exit 1
    }

    Write-Success "Installation verified: $exePath"
}

# Add to PATH
function Add-ToPath {
    param([string]$installDir)

    Write-Info "Adding CarpAI to user PATH..."

    # Get current user PATH
    $userPath = [Environment]::GetEnvironmentVariable("PATH", "User")
    $paths = $userPath -split ";"

    # Check if already in PATH
    if ($paths -contains $installDir) {
        Write-Info "CarpAI already in PATH"
        return
    }

    # Add to PATH
    $newPath = $userPath.TrimEnd(";") + ";$installDir"
    [Environment]::SetEnvironmentVariable("PATH", $newPath, "User")

    # Update current session PATH
    $env:PATH = $env:PATH + ";$installDir"

    Write-Success "Added to PATH: $installDir"
    Write-Info "Please restart your terminal or run: `$env:PATH = `$env:PATH + `";$installDir`""
}

# Create default configuration
function Create-DefaultConfig {
    param([string]$installDir)

    $configDir = Join-Path $env:USERPROFILE ".carpai"
    $configFile = Join-Path $configDir "config.toml"

    if (Test-Path $configFile) {
        Write-Info "Configuration already exists: $configFile"
        return
    }

    Write-Info "Creating default configuration..."

    # Create config directory
    if (-not (Test-Path $configDir)) {
        New-Item -ItemType Directory -Path $configDir -Force | Out-Null
    }

    # Write default config
    $defaultConfig = @"
# CarpAI Configuration File
# Location: ~/.carpai/config.toml

[general]
workspace_root = "."
log_level = "info"
max_context_tokens = 8192

[agent]
auto_mcp_discovery = true
cross_file_planning = true
semantic_refactoring = true

[mcp]
enabled_servers = ["github", "jira", "slack"]

[offline]
mode = "auto"  # auto, online, offline
cache_size_mb = 512
vector_index_type = "hnsw"

[kubernetes]
namespace = "carpai"
replicas = 3

[database]
url = "postgresql://carpai:password@localhost:5432/carpai"
pool_size = 10
"@

    Set-Content -Path $configFile -Value $defaultConfig -Encoding UTF8
    Write-Success "Configuration created: $configFile"
}

# Create start menu shortcut
function Create-Shortcut {
    param([string]$installDir)

    $startMenuPath = [Environment]::GetFolderPath("StartMenu")
    $shortcutPath = Join-Path $startMenuPath "Programs\CarpAI.lnk"

    $exePath = Join-Path $installDir "carpai.exe"
    $iconPath = Join-Path $installDir "assets\carpai.ico"

    try {
        $WshShell = New-Object -ComObject WScript.Shell
        $shortcut = $WshShell.CreateShortcut($shortcutPath)
        $shortcut.TargetPath = $exePath
        $shortcut.WorkingDirectory = $installDir
        $shortcut.Description = "CarpAI - Code Analysis & Refactoring Platform"
        if (Test-Path $iconPath) {
            $shortcut.IconLocation = $iconPath
        }
        $shortcut.Save()
        Write-Success "Start menu shortcut created"
    } catch {
        Write-Warning-Custom "Failed to create shortcut: $_"
    }
}

# Display post-installation information
function Show-PostInstallInfo {
    param([string]$installDir, [string]$version)

    Write-Host ""
    Write-Host "$GREEN╔═══════════════════════════════════════════════════════════╗$NC"
    Write-Host "$GREEN║         CarpAI Installation Completed Successfully!       ║$NC"
    Write-Host "$GREEN╚═══════════════════════════════════════════════════════════╝$NC"
    Write-Host ""
    Write-Host "  Version:      $version"
    Write-Host "  Install Dir:  $installDir"
    Write-Host "  Config File:  $env:USERPROFILE\.carpai\config.toml"
    Write-Host ""
    Write-Host "$BLUE Quick Start:$NC"
    Write-Host "  1. Restart your terminal or reload PATH"
    Write-Host "  2. Run: carpai --version"
    Write-Host "  3. Run: carpai init <workspace>"
    Write-Host "  4. Run: carpai analyze"
    Write-Host ""
    Write-Host "$BLUE Documentation:$NC"
    Write-Host "  https://docs.codecargo.io/carpai"
    Write-Host ""
    Write-Host "$BLUE Support:$NC"
    Write-Host "  GitHub Issues: https://github.com/codecargo/carpai/issues"
    Write-Host "  Discord: https://discord.gg/codecargo"
    Write-Host ""
}

# Main installation flow
function Main {
    Write-Host ""
    Write-Host "$BLUE╔═══════════════════════════════════════════════════════════╗$NC"
    Write-Host "$BLUE║              CarpAI Installer v1.0                        ║$NC"
    Write-Host "$BLUE╚═══════════════════════════════════════════════════════════╝$NC"
    Write-Host ""

    # Step 1: System checks
    Test-SystemRequirements
    Test-Dependencies

    # Step 2: Determine version
    if ([string]::IsNullOrEmpty($Version)) {
        Write-Info "Fetching latest stable version..."
        $Version = Get-LatestVersion
        Write-Success "Latest version: $Version"
    } else {
        Write-Info "Using specified version: $Version"
    }

    # Step 3: Determine install directory
    if ([string]::IsNullOrEmpty($InstallDir)) {
        $InstallDir = Join-Path $env:LOCALAPPDATA "carpai\$Version"
    }

    # Step 4: Get package (download or local)
    $packagePath = ""
    if ($Offline) {
        if ([string]::IsNullOrEmpty($PackagePath)) {
            Write-Error-Custom "Offline mode requires -PackagePath parameter"
            exit 1
        }
        if (-not (Test-Path $PackagePath)) {
            Write-Error-Custom "Package not found: $PackagePath"
            exit 1
        }
        $packagePath = $PackagePath
        Write-Info "Using offline package: $packagePath"
    } else {
        $packagePath = Download-Release $Version
    }

    # Step 5: Install
    Install-Package $packagePath $InstallDir

    # Step 6: Configure PATH
    Add-ToPath $InstallDir

    # Step 7: Create configuration
    Create-DefaultConfig

    # Step 8: Create shortcut
    Create-Shortcut $InstallDir

    # Step 9: Post-install info
    Show-PostInstallInfo $InstallDir $Version
}

# Execute main function
try {
    Main
} catch {
    Write-Error-Custom "Installation failed: $_"
    exit 1
}
