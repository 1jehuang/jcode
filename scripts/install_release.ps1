<#
.SYNOPSIS
    Install the locally-built jcode release into the immutable version store
    and point the launcher at the `current` channel (Windows equivalent of
    scripts/install_release.sh).

.DESCRIPTION
    Builds with `cargo build --profile <profile>` (default: release-lto;
    --fast switches to release), copies the resulting jcode.exe into
    %LOCALAPPDATA%\jcode\builds\versions\<hash>\jcode.exe, refreshes the
    stable + current channel files, writes the matching version markers,
    and points %LOCALAPPDATA%\jcode\bin\jcode.exe at the current channel.

    Windows symlink creation requires elevation or Developer Mode, so this
    script always copies. The platform module handles overwriting a
    running jcode.exe via the rename-aside trick implemented in
    src/platform.rs::replace_executable_atomic.

.PARAMETER Fast
    Build with the `release` profile (no LTO) instead of `release-lto`.

.PARAMETER InstallDir
    Override the launcher install directory. Default:
    $env:LOCALAPPDATA\jcode\bin

.EXAMPLE
    pwsh scripts/install_release.ps1
    pwsh scripts/install_release.ps1 -Fast
#>
[CmdletBinding()]
param(
    [switch]$Fast,
    [string]$InstallDir
)

$ErrorActionPreference = 'Stop'

$repoRoot = & git rev-parse --show-toplevel 2>$null
if ($LASTEXITCODE -ne 0 -or -not $repoRoot) {
    $repoRoot = (Get-Location).Path
}

$profile = if ($env:JCODE_RELEASE_PROFILE) { $env:JCODE_RELEASE_PROFILE } else { 'release-lto' }
if ($Fast) { $profile = 'release' }

switch ($profile) {
    'release-lto' { Write-Host 'Building with LTO (this takes a few minutes)...' }
    'release'     { Write-Host 'Building fast release profile (no LTO)...' }
    default {
        Write-Error "Unsupported profile: $profile (expected: release or release-lto)"
        exit 1
    }
}

& cargo build --profile $profile --manifest-path (Join-Path $repoRoot 'Cargo.toml')
if ($LASTEXITCODE -ne 0) {
    Write-Error "cargo build failed with exit code $LASTEXITCODE"
    exit $LASTEXITCODE
}

$binSource = Join-Path $repoRoot "target/$profile/jcode.exe"
if (-not (Test-Path -LiteralPath $binSource -PathType Leaf)) {
    Write-Error "Release binary not found: $binSource"
    exit 1
}

# Resolve a short hash for the version directory; mirror install_release.sh.
$hash = ''
try {
    $sha = & git -C $repoRoot rev-parse --short HEAD 2>$null
    if ($LASTEXITCODE -eq 0 -and $sha) {
        $hash = $sha.Trim()
        $dirty = & git -C $repoRoot status --porcelain 2>$null
        if ($LASTEXITCODE -eq 0 -and $dirty) {
            $hash = "$hash-dirty"
        }
    }
} catch {
    $hash = ''
}
if (-not $hash) {
    $hash = (Get-Date).ToString('yyyyMMddHHmmss')
}

$jcodeHome = if ($env:JCODE_HOME) { $env:JCODE_HOME } else { Join-Path $env:LOCALAPPDATA 'jcode' }
$buildsDir = Join-Path $jcodeHome 'builds'
$versionDir = Join-Path $buildsDir "versions/$hash"
$null = New-Item -ItemType Directory -Force -Path $versionDir

# replace_executable_atomic-equivalent: rename any in-use .exe aside, then copy.
# Cheap PowerShell version of the platform module's logic.
function Update-JcodeExe {
    param(
        [Parameter(Mandatory)] [string]$Source,
        [Parameter(Mandatory)] [string]$Destination
    )
    if (Test-Path -LiteralPath $Destination -PathType Leaf) {
        $stem = [System.IO.Path]::GetFileName($Destination)
        $parent = [System.IO.Path]::GetDirectoryName($Destination)
        $nanos = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
        $aside = Join-Path $parent ".$stem.$PID-$nanos.old"
        try {
            Move-Item -LiteralPath $Destination -Destination $aside -Force -ErrorAction Stop
        } catch {
            # If rename fails the destination probably is not actually in use;
            # try a direct copy/overwrite as the fallback.
        }
    }
    Copy-Item -LiteralPath $Source -Destination $Destination -Force
}

$versionedExe = Join-Path $versionDir 'jcode.exe'
Update-JcodeExe -Source $binSource -Destination $versionedExe

$stableDir = Join-Path $buildsDir 'stable'
$null = New-Item -ItemType Directory -Force -Path $stableDir
Update-JcodeExe -Source $versionedExe -Destination (Join-Path $stableDir 'jcode.exe')
Set-Content -LiteralPath (Join-Path $buildsDir 'stable-version') -Value $hash -NoNewline:$false

$currentDir = Join-Path $buildsDir 'current'
$null = New-Item -ItemType Directory -Force -Path $currentDir
Update-JcodeExe -Source $versionedExe -Destination (Join-Path $currentDir 'jcode.exe')
Set-Content -LiteralPath (Join-Path $buildsDir 'current-version') -Value $hash -NoNewline:$false

if (-not $InstallDir) {
    $InstallDir = if ($env:JCODE_INSTALL_DIR) { $env:JCODE_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'jcode/bin' }
}
$null = New-Item -ItemType Directory -Force -Path $InstallDir
$launcher = Join-Path $InstallDir 'jcode.exe'
Update-JcodeExe -Source (Join-Path $currentDir 'jcode.exe') -Destination $launcher

# Best-effort sweep of stale .old sidecars (matches startup cleanup).
foreach ($dir in @($stableDir, $currentDir, $versionDir, $InstallDir)) {
    Get-ChildItem -LiteralPath $dir -Filter '*.old' -Force -ErrorAction SilentlyContinue |
        Where-Object { $_.Name.StartsWith('.') } |
        ForEach-Object { Remove-Item -LiteralPath $_.FullName -Force -ErrorAction SilentlyContinue }
}

Write-Host ""
Write-Host "Installed:        $versionedExe"
Write-Host "Stable channel:   $(Join-Path $stableDir 'jcode.exe')"
Write-Host "Current channel:  $(Join-Path $currentDir 'jcode.exe')"
Write-Host "Launcher:         $launcher"

$pathDirs = ($env:PATH -split [System.IO.Path]::PathSeparator) | Where-Object { $_ }
if ($pathDirs -notcontains $InstallDir) {
    Write-Host ""
    Write-Host "Tip: add $InstallDir to PATH (User scope) so 'jcode' resolves in new shells."
    Write-Host "     Example: setx PATH ""%PATH%;$InstallDir"""
}
