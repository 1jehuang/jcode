<#
.SYNOPSIS
    Uninstall jcode on Windows.
.DESCRIPTION
    Removes the per-user launcher at %LOCALAPPDATA%\jcode\bin\jcode.exe,
    installed build binaries, and the jcode launcher directory from the user PATH.
    By default user data under %USERPROFILE%\.jcode is kept.

    One-liner uninstall:
      irm https://raw.githubusercontent.com/1jehuang/jcode/master/scripts/uninstall.ps1 | iex
.PARAMETER InstallDir
    Override the launcher directory (default: $env:LOCALAPPDATA\jcode\bin)
.PARAMETER Purge
    Also delete user data in $env:JCODE_HOME or %USERPROFILE%\.jcode.
.PARAMETER DryRun
    Print what would be removed without deleting anything.
.PARAMETER Yes
    Skip the confirmation prompt.
#>
param(
    [string]$InstallDir,
    [switch]$Purge,
    [switch]$DryRun,
    [switch]$Yes
)

$ErrorActionPreference = 'Stop'

function Write-Info($msg) { Write-Host $msg -ForegroundColor Blue }
function Write-Err($msg) { throw "error: $msg" }
function Write-Warn($msg) { Write-Host "warning: $msg" -ForegroundColor Yellow }

function Get-JcodeLocalAppDataDir {
    if ($env:LOCALAPPDATA) { return $env:LOCALAPPDATA }

    $localAppData = [Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)
    if ($localAppData) { return $localAppData }

    if ($env:USERPROFILE) { return (Join-Path $env:USERPROFILE "AppData\Local") }
    return (Join-Path ([Environment]::GetFolderPath("UserProfile")) "AppData\Local")
}

function Get-DefaultJcodeInstallDir {
    return (Join-Path (Get-JcodeLocalAppDataDir) "jcode\bin")
}

function Get-JcodeRoamingAppDataDir {
    if ($env:APPDATA) { return $env:APPDATA }

    $appData = [Environment]::GetFolderPath([Environment+SpecialFolder]::ApplicationData)
    if ($appData) { return $appData }

    if ($env:USERPROFILE) { return (Join-Path $env:USERPROFILE "AppData\Roaming") }
    return (Join-Path ([Environment]::GetFolderPath("UserProfile")) "AppData\Roaming")
}

function Get-JcodeStartupShortcutPath {
    return (Join-Path (Get-JcodeRoamingAppDataDir) "Microsoft\Windows\Start Menu\Programs\Startup\jcode-hotkey.lnk")
}

function Get-JcodeHotkeyArtifactPaths([string]$UserDataDir) {
    $hotkeyDir = Join-Path $UserDataDir "hotkey"
    return @(
        (Join-Path $hotkeyDir "jcode-hotkey.ps1"),
        (Join-Path $hotkeyDir "jcode-hotkey-launcher.vbs"),
        (Join-Path $hotkeyDir "jcode-hotkey-shortcut.ps1")
    )
}

function Clear-JcodeHotkeySetupState([string]$UserDataDir) {
    $setupHintsPath = Join-Path $UserDataDir "setup_hints.json"
    if (-not (Test-Path -LiteralPath $setupHintsPath)) { return }

    try {
        $state = Get-Content -LiteralPath $setupHintsPath -Raw | ConvertFrom-Json -ErrorAction Stop
        foreach ($property in @(
            @{ Name = "hotkey_configured"; Value = $false },
            @{ Name = "hotkey_dismissed"; Value = $true }
        )) {
            if ($state.PSObject.Properties.Name -contains $property.Name) {
                $state.($property.Name) = $property.Value
            } else {
                $state | Add-Member -NotePropertyName $property.Name -NotePropertyValue $property.Value
            }
        }
        $state | ConvertTo-Json | Set-Content -LiteralPath $setupHintsPath -Encoding UTF8
    } catch {
        Write-Warn "Could not update hotkey setup state in $setupHintsPath"
    }
}

function ConvertTo-JcodePathKey([string]$PathValue) {
    if (-not $PathValue) { return "" }
    $clean = [Environment]::ExpandEnvironmentVariables($PathValue.Trim().Trim('"'))
    if (-not $clean) { return "" }
    try { $clean = [System.IO.Path]::GetFullPath($clean) } catch {}
    $clean = $clean.TrimEnd([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)
    return $clean.ToUpperInvariant()
}

function Test-JcodeSafePurgePath([string]$PathValue) {
    $pathKey = ConvertTo-JcodePathKey $PathValue
    if (-not $pathKey) { return $false }

    try {
        $fullPath = [System.IO.Path]::GetFullPath([Environment]::ExpandEnvironmentVariables($PathValue.Trim().Trim('"')))
        $rootKey = ConvertTo-JcodePathKey ([System.IO.Path]::GetPathRoot($fullPath))
        $leafName = [System.IO.Path]::GetFileName($fullPath.TrimEnd(
            [System.IO.Path]::DirectorySeparatorChar,
            [System.IO.Path]::AltDirectorySeparatorChar
        ))
    } catch {
        return $false
    }

    if ($pathKey -eq $rootKey -or $leafName -notmatch '(?i)^\.?jcode(?:[-_ ].*)?$') {
        return $false
    }

    $separator = [string][System.IO.Path]::DirectorySeparatorChar
    foreach ($protectedPath in @(
        $env:USERPROFILE,
        $env:HOME,
        $env:LOCALAPPDATA,
        $env:APPDATA,
        [Environment]::GetFolderPath([Environment+SpecialFolder]::UserProfile)
    )) {
        $protectedKey = ConvertTo-JcodePathKey $protectedPath
        if (-not $protectedKey) { continue }
        if ($pathKey -eq $protectedKey -or $protectedKey.StartsWith($pathKey + $separator, [System.StringComparison]::OrdinalIgnoreCase)) {
            return $false
        }
    }

    return $true
}

function Test-JcodeManagedExecutablePath([string]$ExecutablePath, [string]$LauncherPath, [string]$BuildsDir) {
    $executableKey = ConvertTo-JcodePathKey $ExecutablePath
    $launcherKey = ConvertTo-JcodePathKey $LauncherPath
    $buildsKey = ConvertTo-JcodePathKey $BuildsDir
    if (-not $executableKey) { return $false }
    if ($launcherKey -and $executableKey -eq $launcherKey) { return $true }

    # A live upgrade may rename the loaded stable launcher before replacing it.
    # Treat only that tightly-scoped backup pattern in the launcher directory as
    # managed so uninstall can stop and remove it without touching other tools.
    $launcherDirKey = ConvertTo-JcodePathKey (Split-Path -Parent $LauncherPath)
    $executableDirKey = ConvertTo-JcodePathKey (Split-Path -Parent $ExecutablePath)
    $executableName = Split-Path -Leaf $ExecutablePath
    if ($launcherDirKey -and $executableDirKey -eq $launcherDirKey -and $executableName -like '.jcode-launcher-old-*.exe') {
        return $true
    }

    $separator = [string][System.IO.Path]::DirectorySeparatorChar
    return [bool]($buildsKey -and $executableKey.StartsWith($buildsKey + $separator, [System.StringComparison]::OrdinalIgnoreCase))
}

function Split-JcodePathList([string]$PathValue) {
    if (-not $PathValue) { return @() }
    $entries = @()
    foreach ($entry in ($PathValue -split ';')) {
        $clean = $entry.Trim().Trim('"')
        if ($clean) { $entries += $clean }
    }
    return $entries
}

function Join-JcodePathList([string[]]$Entries) {
    if (-not $Entries -or $Entries.Count -eq 0) { return "" }
    return ($Entries -join ';')
}

function Get-JcodeManagedPathKeys([string]$InstallDir) {
    $keys = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
    foreach ($candidate in @($InstallDir, (Get-DefaultJcodeInstallDir))) {
        $key = ConvertTo-JcodePathKey $candidate
        if ($key) { [void]$keys.Add($key) }
    }
    return $keys
}

function Resolve-JcodePathRemoval {
    param(
        [Parameter(Mandatory = $true)][string]$InstallDir,
        [AllowNull()][string]$CurrentPath
    )

    $managedKeys = Get-JcodeManagedPathKeys -InstallDir $InstallDir
    $nextEntries = @()
    $removedManaged = 0

    foreach ($entry in (Split-JcodePathList $CurrentPath)) {
        $key = ConvertTo-JcodePathKey $entry
        if (-not $key) { continue }
        if ($managedKeys.Contains($key)) {
            $removedManaged += 1
            continue
        }
        $nextEntries += $entry
    }

    $nextPath = Join-JcodePathList $nextEntries
    return [pscustomobject]@{
        Path = $nextPath
        Changed = ($nextPath -ne ([string]$CurrentPath))
        RemovedManagedEntries = $removedManaged
        InstallDir = $InstallDir
    }
}

function Send-JcodeEnvironmentChangedBroadcast {
    if ($env:JCODE_DISABLE_ENV_BROADCAST -eq "1") { return $false }
    if (-not ("Jcode.EnvironmentBroadcast" -as [type])) {
        Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
namespace Jcode {
    public static class EnvironmentBroadcast {
        [DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)]
        public static extern IntPtr SendMessageTimeout(
            IntPtr hWnd,
            UInt32 Msg,
            UIntPtr wParam,
            string lParam,
            UInt32 fuFlags,
            UInt32 uTimeout,
            out UIntPtr lpdwResult);
    }
}
"@
    }
    $result = [UIntPtr]::Zero
    [Jcode.EnvironmentBroadcast]::SendMessageTimeout([IntPtr]0xffff, 0x001A, [UIntPtr]::Zero, "Environment", 0x0002, 5000, [ref]$result) | Out-Null
    return $true
}

function Remove-JcodeUserPath {
    param(
        [Parameter(Mandatory = $true)][string]$InstallDir,
        [AllowNull()][string]$CurrentPath,
        [scriptblock]$SetUserPathAction,
        [scriptblock]$BroadcastAction,
        [bool]$Broadcast = $true
    )

    if (-not $PSBoundParameters.ContainsKey('CurrentPath')) {
        $CurrentPath = [Environment]::GetEnvironmentVariable("Path", "User")
    }

    $update = Resolve-JcodePathRemoval -InstallDir $InstallDir -CurrentPath $CurrentPath
    $broadcasted = $false
    if ($update.Changed) {
        if ($SetUserPathAction) {
            & $SetUserPathAction $update.Path
        } else {
            [Environment]::SetEnvironmentVariable("Path", $update.Path, "User")
        }

        if ($Broadcast) {
            if ($BroadcastAction) { & $BroadcastAction | Out-Null } else { Send-JcodeEnvironmentChangedBroadcast | Out-Null }
            $broadcasted = $true
        }
    }
    $update | Add-Member -NotePropertyName Broadcasted -NotePropertyValue $broadcasted
    return $update
}


function Invoke-JcodeUninstall {
    param(
        [string]$InstallDir,
        [switch]$Purge,
        [switch]$DryRun,
        [switch]$Yes
    )

if (-not $InstallDir) { $InstallDir = Get-DefaultJcodeInstallDir }

$localJcodeRoot = Join-Path (Get-JcodeLocalAppDataDir) "jcode"
$launcherPath = Join-Path $InstallDir "jcode.exe"
$buildsDir = Join-Path $localJcodeRoot "builds"
$userDataDir = if ($env:JCODE_HOME) {
    $env:JCODE_HOME
} elseif ($env:USERPROFILE) {
    Join-Path $env:USERPROFILE ".jcode"
} else {
    Join-Path ([Environment]::GetFolderPath("UserProfile")) ".jcode"
}
$startupShortcutPath = Get-JcodeStartupShortcutPath
$hotkeyArtifactPaths = @(Get-JcodeHotkeyArtifactPaths -UserDataDir $userDataDir)
$launcherBackupPaths = if (Test-Path -LiteralPath $InstallDir) {
    @(Get-ChildItem -LiteralPath $InstallDir -Filter '.jcode-launcher-old-*.exe' -File -Force -ErrorAction SilentlyContinue |
        ForEach-Object { $_.FullName })
} else {
    @()
}
if ($Purge -and -not (Test-JcodeSafePurgePath $userDataDir)) {
    Write-Err "Refusing to purge unsafe JCODE_HOME path '$userDataDir'. Use a dedicated .jcode or jcode-* directory."
}

$targets = @()
if (Test-Path -LiteralPath $launcherPath) { $targets += "$launcherPath (launcher)" }
foreach ($path in $launcherBackupPaths) { $targets += "$path (previous live-upgrade launcher)" }
if (Test-Path -LiteralPath $buildsDir) { $targets += "$buildsDir (installed binaries)" }
if (Test-Path -LiteralPath $startupShortcutPath) { $targets += "$startupShortcutPath (launch-hotkey startup shortcut)" }
foreach ($path in $hotkeyArtifactPaths) {
    if (Test-Path -LiteralPath $path) { $targets += "$path (launch-hotkey artifact)" }
}
if ($Purge -and (Test-Path -LiteralPath $userDataDir)) { $targets += "$userDataDir (user data)" }

$userPathPreview = Resolve-JcodePathRemoval -InstallDir $InstallDir -CurrentPath ([Environment]::GetEnvironmentVariable("Path", "User"))
if ($userPathPreview.RemovedManagedEntries -gt 0) {
    $targets += "$InstallDir (user PATH entry)"
}

if ($targets.Count -eq 0) {
    Write-Info "Nothing to uninstall: no jcode installation found."
    return 0
}

Write-Info "The following will be removed:"
foreach ($target in $targets) { Write-Host "  - $target" }
if (-not $Purge) {
    Write-Warn "User data in $userDataDir is kept. Run with -Purge for a full wipe."
}

if ($DryRun) {
    Write-Info "Dry run: nothing was deleted."
    return 0
}

if (-not $Yes) {
    $reply = Read-Host "Proceed? [y/N]"
    if ($reply -notin @("y", "Y", "yes", "YES")) {
        Write-Info "Aborted."
        return 1
    }
}

try {
    $managedProcessIds = @(Get-CimInstance Win32_Process -Filter "Name = 'jcode.exe'" -ErrorAction SilentlyContinue |
        Where-Object { Test-JcodeManagedExecutablePath -ExecutablePath $_.ExecutablePath -LauncherPath $launcherPath -BuildsDir $buildsDir } |
        ForEach-Object { $_.ProcessId })
    foreach ($processId in $managedProcessIds) {
        $process = Get-Process -Id $processId -ErrorAction SilentlyContinue
        Stop-Process -Id $processId -Force -ErrorAction SilentlyContinue
        if ($process) {
            try { [void]$process.WaitForExit(10000) } catch {}
        }
    }
} catch {}

if (Test-Path -LiteralPath $startupShortcutPath) {
    Remove-Item -LiteralPath $startupShortcutPath -Force
    Write-Info "Removed $startupShortcutPath"
}

foreach ($path in $hotkeyArtifactPaths) {
    if (Test-Path -LiteralPath $path) {
        Remove-Item -LiteralPath $path -Force
        Write-Info "Removed $path"
    }
}
if (-not $Purge) {
    $hotkeyDir = Join-Path $userDataDir "hotkey"
    if (Test-Path -LiteralPath $hotkeyDir) {
        Remove-Item -LiteralPath $hotkeyDir -Force -ErrorAction SilentlyContinue
    }
    Clear-JcodeHotkeySetupState -UserDataDir $userDataDir
}

if (Test-Path -LiteralPath $launcherPath) {
    Remove-Item -LiteralPath $launcherPath -Force
    Write-Info "Removed $launcherPath"
}

foreach ($path in $launcherBackupPaths) {
    if (Test-Path -LiteralPath $path) {
        Remove-Item -LiteralPath $path -Force
        Write-Info "Removed $path"
    }
}

if (Test-Path -LiteralPath $InstallDir) {
    try { Remove-Item -LiteralPath $InstallDir -Force -ErrorAction SilentlyContinue } catch {}
}

if ($Purge) {
    foreach ($path in @($localJcodeRoot, $userDataDir)) {
        if ($path -and (Test-Path -LiteralPath $path)) {
            Remove-Item -LiteralPath $path -Recurse -Force
            Write-Info "Removed $path"
        }
    }
} elseif (Test-Path -LiteralPath $buildsDir) {
    Remove-Item -LiteralPath $buildsDir -Recurse -Force
    Write-Info "Removed $buildsDir"
}

$pathUpdate = Remove-JcodeUserPath -InstallDir $InstallDir
if ($pathUpdate.Changed) {
    Write-Info "Removed $($pathUpdate.RemovedManagedEntries) jcode entr$(if ($pathUpdate.RemovedManagedEntries -eq 1) { 'y' } else { 'ies' }) from user PATH"
}

Write-Info "jcode uninstalled."
Write-Info "Reinstall with: irm https://jcode.sh/install.ps1 | iex"


    return 0
}

if ($env:JCODE_UNINSTALL_PS1_IMPORT_ONLY -ne "1") {
    $exitCode = Invoke-JcodeUninstall -InstallDir $InstallDir -Purge:$Purge -DryRun:$DryRun -Yes:$Yes
    if ($null -ne $exitCode -and [int]$exitCode -ne 0) {
        if ($MyInvocation.MyCommand.Path) { exit ([int]$exitCode) }
        $global:LASTEXITCODE = [int]$exitCode
    }
}
