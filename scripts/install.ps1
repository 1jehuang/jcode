<#
.SYNOPSIS
    Install kcode on Windows.
.DESCRIPTION
    Downloads the latest kcode release and installs it to %LOCALAPPDATA%\kcode\bin.

    One-liner install:
      irm https://kcode.sh/install.ps1 | iex

    Or download and run (allows parameters):
      & ([scriptblock]::Create((irm https://kcode.sh/install.ps1)))
.PARAMETER InstallDir
    Override the installation directory (default: $env:LOCALAPPDATA\kcode\bin)
.PARAMETER Version
    Override the version tag to install. Required when using a local artifact path.
.PARAMETER ArtifactExePath
    Use a local kcode.exe artifact instead of downloading from GitHub.
.PARAMETER ArtifactTgzPath
    Use a local kcode .tar.gz artifact instead of downloading from GitHub.
.PARAMETER BuildFromSource
    If no prebuilt release asset is available, explicitly allow a source build.
    Source builds require Git, Rust, and the Visual Studio C++ Build Tools.
.PARAMETER ConfigureAlacritty
    Install Alacritty through winget when it is not already available.
.PARAMETER ConfigureHotkey
    Configure the optional global launch hotkey.
.PARAMETER SkipAlacrittySetup
    Deprecated compatibility switch. Alacritty setup is opt-in by default.
.PARAMETER SkipHotkeySetup
    Deprecated compatibility switch. Hotkey setup is opt-in by default.
#>
param(
    [string]$InstallDir,
    [string]$Version,
    [string]$ArtifactExePath,
    [string]$ArtifactTgzPath,
    [switch]$BuildFromSource,
    [switch]$ConfigureAlacritty,
    [switch]$ConfigureHotkey,
    [switch]$SkipAlacrittySetup,
    [switch]$SkipHotkeySetup
)

$ErrorActionPreference = 'Stop'

function Assert-KcodeOwnedPath([string]$ProductPath) {
    if (-not $ProductPath) { return }
    $probe = [System.IO.Path]::GetFullPath($ProductPath)
    if ($probe -match '(?i)(^|[\\/])(?:\.jcode[^\\/]*|jcode(?:[.-][^\\/]*)?)(?:[\\/]|$)') {
        throw 'KCode cannot install into jcode storage or binary directories.'
    }
    while ($probe) {
        if (Test-Path -LiteralPath $probe) {
            $entry = Get-Item -LiteralPath $probe -Force
            if (($entry.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw 'KCode installer does not follow reparse points in product paths.'
            }
        }
        $parent = Split-Path -Parent $probe
        if (-not $parent -or $parent -eq $probe) { break }
        $probe = $parent
    }
}

# No maintained KCode channel exists for this branch. Fail before network,
# directory creation, executable probing, or PATH changes.
if (-not $ArtifactExePath -and -not $ArtifactTgzPath) {
    throw 'KCode release downloads are disabled. Build KooshaPari/KCode and provide a local artifact with -Version.'
}
if (-not $Version) {
    throw 'KCode local artifact installs require an explicit -Version.'
}
foreach ($productPath in @($InstallDir, $env:KCODE_HOME)) {
    Assert-KcodeOwnedPath $productPath
}

if ($PSVersionTable.PSVersion.Major -lt 5) {
    Write-Host "error: PowerShell 5.1 or later is required" -ForegroundColor Red
    exit 1
}

$Repo = "KooshaPari/KCode"
$ReleaseMetadataBase = if ($env:KCODE_RELEASE_METADATA_BASE) {
    $env:KCODE_RELEASE_METADATA_BASE.TrimEnd('/')
} else {
    "https://github.com/KooshaPari/KCode/releases"
}

if (-not $InstallDir) {
    $localAppData = if ($env:LOCALAPPDATA) { $env:LOCALAPPDATA } else { [Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData) }
    if (-not $localAppData -and $env:USERPROFILE) { $localAppData = Join-Path $env:USERPROFILE "AppData\Local" }
    $InstallDir = if ($env:KCODE_HOME) { Join-Path $env:KCODE_HOME "bin" } else { Join-Path $localAppData "kcode\bin" }
}

$KcodeHome = if ($env:KCODE_HOME) {
    $env:KCODE_HOME
} elseif ($env:USERPROFILE) {
    Join-Path $env:USERPROFILE ".kcode"
} else {
    Join-Path ([Environment]::GetFolderPath("UserProfile")) ".kcode"
}

Assert-KcodeOwnedPath $InstallDir
Assert-KcodeOwnedPath $KcodeHome
$HotkeyDir = Join-Path $KcodeHome "hotkey"
$SetupHintsPath = Join-Path $KcodeHome "setup_hints.json"

function Write-Info($msg) { Write-Host $msg -ForegroundColor Blue }
function Write-Err($msg) { throw "error: $msg" }
function Write-Warn($msg) { Write-Host "warning: $msg" -ForegroundColor Yellow }

function ConvertFrom-KcodeWebContent($Content) {
    if ($null -eq $Content) { return "" }

    # Windows PowerShell 5.1 returns Byte[] for some text responses when the
    # server uses application/octet-stream (including kcode.sh metadata and
    # GitHub release checksum manifests). Casting Byte[] directly to [string]
    # produces a space-separated list of decimal bytes instead of the text.
    if ($Content -is [byte[]]) {
        return [System.Text.Encoding]::UTF8.GetString($Content)
    }

    return [string]$Content
}

function Test-KcodeReleaseTag([string]$Tag) {
    return [bool]($Tag -match '^v[0-9]+\.[0-9]+\.[0-9]+(?:[+.-][A-Za-z0-9.-]+)?$')
}

function Resolve-KcodeReleaseTagFromUri([string]$Uri) {
    if (-not $Uri) { return $null }
    if ($Uri -match '/releases/tag/([^/?#]+)') {
        return [Uri]::UnescapeDataString($Matches[1])
    }
    return $null
}

function Get-LatestKcodeReleaseTag {
    # Avoid api.github.com here. Its unauthenticated limit is only 60 requests
    # per public IP per hour, so installs are unreliable behind shared NAT/VPNs.
    $metadataTag = $null
    try {
        $metadataResponse = Invoke-WebRequest -UseBasicParsing -Uri "$ReleaseMetadataBase/latest/version"
        $candidate = (ConvertFrom-KcodeWebContent -Content $metadataResponse.Content).Trim()
        if (Test-KcodeReleaseTag $candidate) { $metadataTag = $candidate }
    } catch {}

    try {
        $response = Invoke-WebRequest -UseBasicParsing -Method Head -Uri "https://github.com/$Repo/releases/latest"
        $baseResponse = $response.BaseResponse
        $resolvedUri = $null

        if ($baseResponse) {
            $responseUriProperty = $baseResponse.PSObject.Properties['ResponseUri']
            if ($responseUriProperty -and $responseUriProperty.Value) {
                $resolvedUri = [string]$responseUriProperty.Value
            }

            if (-not $resolvedUri) {
                $requestMessageProperty = $baseResponse.PSObject.Properties['RequestMessage']
                if ($requestMessageProperty -and $requestMessageProperty.Value) {
                    $resolvedUri = [string]$requestMessageProperty.Value.RequestUri
                }
            }
        }

        $tag = Resolve-KcodeReleaseTagFromUri $resolvedUri
        if ($tag) { return $tag }
    } catch {
        if (-not $metadataTag) {
            Write-Err "Failed to determine latest version: $_"
        }
    }

    if ($metadataTag) {
        Write-Warn "GitHub release lookup unavailable; using cached kcode.sh metadata ($metadataTag)."
        return $metadataTag
    }
    Write-Err "Failed to determine latest version"
}

function Get-KcodeReleaseDownloadBases([string]$ReleaseTag) {
    $bases = New-Object System.Collections.Generic.List[string]
    try {
        $response = Invoke-WebRequest -UseBasicParsing -Uri "$ReleaseMetadataBase/$ReleaseTag/download-bases"
        foreach ($line in ((ConvertFrom-KcodeWebContent -Content $response.Content) -split "`r?`n")) {
            $candidate = $line.Trim().TrimEnd('/')
            if ($candidate -match '^https://\S+$' -and -not $bases.Contains($candidate)) {
                $bases.Add($candidate)
            }
        }
    } catch {}

    $githubBase = "https://github.com/$Repo/releases/download/$ReleaseTag"
    if (-not $bases.Contains($githubBase)) { $bases.Add($githubBase) }
    return $bases.ToArray()
}

function Get-KcodeSha256FromManifest {
    param(
        [Parameter(Mandatory = $true)][string]$ManifestText,
        [Parameter(Mandatory = $true)][string]$AssetName
    )

    foreach ($line in ($ManifestText -split "`r?`n")) {
        if ($line -match '^\s*([0-9a-fA-F]{64})\s+\*?(.+?)\s*$') {
            $candidateName = [System.IO.Path]::GetFileName($Matches[2])
            if ($candidateName -eq $AssetName) {
                return $Matches[1].ToLowerInvariant()
            }
        }
    }

    return $null
}

function Get-ReleaseChecksum([string]$ReleaseTag, [string]$AssetName) {
    $lastError = $null
    foreach ($checksumUrl in @(
        "$ReleaseMetadataBase/$ReleaseTag/SHA256SUMS",
        "https://github.com/$Repo/releases/download/$ReleaseTag/SHA256SUMS"
    )) {
        try {
            $response = Invoke-WebRequest -UseBasicParsing -Uri $checksumUrl
            $expected = Get-KcodeSha256FromManifest -ManifestText (ConvertFrom-KcodeWebContent -Content $response.Content) -AssetName $AssetName
            if ($expected) { return $expected }
        } catch {
            $lastError = $_
        }
    }

    if ($lastError) {
        Write-Err "Could not download SHA256SUMS for $ReleaseTag. Refusing to install an unverified download: $lastError"
    }
    Write-Err "SHA256SUMS for $ReleaseTag does not list $AssetName"
}

function Assert-KcodeFileChecksum([string]$FilePath, [string]$ExpectedSha256, [string]$AssetName) {
    try {
        $actual = (Get-FileHash -LiteralPath $FilePath -Algorithm SHA256).Hash.ToLowerInvariant()
    } catch {
        Write-Err "Could not calculate SHA256 for ${AssetName}: $_"
    }

    if ($actual -ne $ExpectedSha256) {
        Remove-Item -LiteralPath $FilePath -Force -ErrorAction SilentlyContinue
        Write-Err "SHA256 verification failed for $AssetName (expected $ExpectedSha256, got $actual)"
    }

    Write-Info "Verified SHA256: $AssetName"
    return $actual
}

function Get-KcodeLocalAppDataDir {
    if ($env:LOCALAPPDATA) {
        return $env:LOCALAPPDATA
    }

    $localAppData = [Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)
    if ($localAppData) {
        return $localAppData
    }

    if ($env:USERPROFILE) {
        return (Join-Path $env:USERPROFILE "AppData\Local")
    }

    return (Join-Path ([Environment]::GetFolderPath("UserProfile")) "AppData\Local")
}

function Get-DefaultKcodeInstallDir {
    return (Join-Path (Get-KcodeLocalAppDataDir) "kcode\bin")
}

function ConvertTo-KcodePathKey([string]$PathValue) {
    if (-not $PathValue) {
        return ""
    }

    $clean = [Environment]::ExpandEnvironmentVariables($PathValue.Trim().Trim('"'))
    if (-not $clean) {
        return ""
    }

    try {
        $clean = [System.IO.Path]::GetFullPath($clean)
    } catch {
    }


    $clean = $clean.TrimEnd([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)
    return $clean.ToUpperInvariant()
}

function Split-KcodePathList([string]$PathValue) {
    if (-not $PathValue) {
        return @()
    }

    $entries = @()
    foreach ($entry in ($PathValue -split ';')) {
        $clean = $entry.Trim().Trim('"')
        if ($clean) {
            $entries += $clean
        }
    }
    return $entries
}

function Join-KcodePathList([string[]]$Entries) {
    if (-not $Entries -or $Entries.Count -eq 0) {
        return ""
    }

    return ($Entries -join ';')
}

function Get-KcodeManagedPathKeys([string]$InstallDir) {
    $keys = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
    foreach ($candidate in @($InstallDir, (Get-DefaultKcodeInstallDir))) {
        $key = ConvertTo-KcodePathKey $candidate
        if ($key) {
            [void]$keys.Add($key)
        }
    }
    return $keys
}

function Resolve-KcodePathUpdate {
    param(
        [Parameter(Mandatory = $true)][string]$InstallDir,
        [AllowNull()][string]$CurrentPath,
        [switch]$RemoveOnly
    )

    $managedKeys = Get-KcodeManagedPathKeys -InstallDir $InstallDir
    $nextEntries = @()
    $removedManaged = 0

    foreach ($entry in (Split-KcodePathList $CurrentPath)) {
        $key = ConvertTo-KcodePathKey $entry
        if (-not $key) {
            continue
        }

        if ($managedKeys.Contains($key)) {
            $removedManaged += 1
            continue
        }

        $nextEntries += $entry
    }

    if (-not $RemoveOnly) {
        $nextEntries = @($InstallDir) + $nextEntries
    }

    $nextPath = Join-KcodePathList $nextEntries
    $changed = ($nextPath -ne ([string]$CurrentPath))

    return [pscustomobject]@{
        Path = $nextPath
        Changed = $changed
        RemovedManagedEntries = $removedManaged
        RemovedDuplicateEntries = 0
        AddedLauncherEntry = (-not $RemoveOnly)
        InstallDir = $InstallDir
    }
}

function Send-KcodeEnvironmentChangedBroadcast {
    if ($env:KCODE_DISABLE_ENV_BROADCAST -eq "1") {
        return $false
    }

    if (-not ("Kcode.EnvironmentBroadcast" -as [type])) {
        Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
namespace Kcode {
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
    [Kcode.EnvironmentBroadcast]::SendMessageTimeout([IntPtr]0xffff, 0x001A, [UIntPtr]::Zero, "Environment", 0x0002, 5000, [ref]$result) | Out-Null
    return $true
}

function Set-KcodeUserPath {
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

    $update = Resolve-KcodePathUpdate -InstallDir $InstallDir -CurrentPath $CurrentPath
    $broadcasted = $false

    if ($update.Changed) {
        if ($SetUserPathAction) {
            & $SetUserPathAction $update.Path
        } else {
            [Environment]::SetEnvironmentVariable("Path", $update.Path, "User")
        }

        if ($Broadcast) {
            if ($BroadcastAction) {
                & $BroadcastAction | Out-Null
            } else {
                Send-KcodeEnvironmentChangedBroadcast | Out-Null
            }
            $broadcasted = $true
        }
    }

    $update | Add-Member -NotePropertyName Broadcasted -NotePropertyValue $broadcasted
    return $update
}

function Remove-KcodeUserPath {
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

    $update = Resolve-KcodePathUpdate -InstallDir $InstallDir -CurrentPath $CurrentPath -RemoveOnly
    $broadcasted = $false

    if ($update.Changed) {
        if ($SetUserPathAction) {
            & $SetUserPathAction $update.Path
        } else {
            [Environment]::SetEnvironmentVariable("Path", $update.Path, "User")
        }

        if ($Broadcast) {
            if ($BroadcastAction) {
                & $BroadcastAction | Out-Null
            } else {
                Send-KcodeEnvironmentChangedBroadcast | Out-Null
            }
            $broadcasted = $true
        }
    }

    $update | Add-Member -NotePropertyName Broadcasted -NotePropertyValue $broadcasted
    return $update
}

function Set-KcodeProcessPath([string]$InstallDir) {
    $update = Resolve-KcodePathUpdate -InstallDir $InstallDir -CurrentPath $env:Path
    $env:Path = $update.Path
    return $update
}

function Remove-KcodeStaleLauncherBackups {
    param(
        [Parameter(Mandatory = $true)][string]$LauncherDir
    )

    Get-ChildItem -LiteralPath $LauncherDir -Filter '.kcode-launcher-old-*.exe' -File -Force -ErrorAction SilentlyContinue |
        Remove-Item -Force -ErrorAction SilentlyContinue
}

function Install-KcodeLauncher {
    param(
        [Parameter(Mandatory = $true)][string]$SourcePath,
        [Parameter(Mandatory = $true)][string]$LauncherPath
    )

    $launcherDir = Split-Path -Parent $LauncherPath
    New-Item -ItemType Directory -Path $launcherDir -Force | Out-Null

    $operationId = [guid]::NewGuid().ToString('N')
    $tempLauncher = Join-Path $launcherDir (".kcode-launcher-{0}.tmp.exe" -f $operationId)
    $oldLauncher = Join-Path $launcherDir (".kcode-launcher-old-{0}.exe" -f $operationId)
    $movedExistingLauncher = $false
    try {
        Copy-Item -Path $SourcePath -Destination $tempLauncher -Force
        if (Test-Path -LiteralPath $LauncherPath) {
            # Windows will not overwrite a loaded executable, but it does allow
            # the directory entry to be renamed while the process keeps running
            # from its existing file handle. Move the old launcher aside first,
            # then atomically put the new binary at the stable PATH location.
            Move-Item -LiteralPath $LauncherPath -Destination $oldLauncher
            $movedExistingLauncher = $true
        }

        try {
            Move-Item -LiteralPath $tempLauncher -Destination $LauncherPath
        } catch {
            if ($movedExistingLauncher -and -not (Test-Path -LiteralPath $LauncherPath)) {
                Move-Item -LiteralPath $oldLauncher -Destination $LauncherPath
                $movedExistingLauncher = $false
            }
            throw
        }

        if ($movedExistingLauncher) {
            # Removal succeeds immediately for an idle launcher. If an older
            # kcode process still has the renamed executable loaded, Windows
            # keeps it until that process exits and the next install cleans it.
            Remove-Item -LiteralPath $oldLauncher -Force -ErrorAction SilentlyContinue
        }

        # Only prune backups after the stable path contains the new launcher.
        # Doing this before replacement could delete another concurrent
        # installer's rollback file during its short rename window.
        Remove-KcodeStaleLauncherBackups -LauncherDir $launcherDir
    } finally {
        Remove-Item -LiteralPath $tempLauncher -Force -ErrorAction SilentlyContinue
    }

    return $LauncherPath
}

function Resolve-OptionalPath([string]$PathValue) {
    if (-not $PathValue) {
        return $null
    }

    try {
        return (Resolve-Path -LiteralPath $PathValue -ErrorAction Stop).Path
    } catch {
        Write-Err "Provided path does not exist: $PathValue"
    }
}

function Stop-ProcessTree([int]$ProcessId) {
    try {
        Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
            Where-Object { $_.ParentProcessId -eq $ProcessId } |
            ForEach-Object { Stop-ProcessTree -ProcessId $_.ProcessId }
    } catch {}

    try {
        Stop-Process -Id $ProcessId -Force -ErrorAction SilentlyContinue
    } catch {}
}

function Invoke-ProcessWithTimeout {
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [string[]]$ArgumentList = @(),
        [Parameter(Mandatory = $true)][int]$TimeoutSeconds,
        [Parameter(Mandatory = $true)][string]$FriendlyName,
        [switch]$CaptureOutput
    )

    $startParams = @{
        FilePath = $FilePath
        ArgumentList = $ArgumentList
        PassThru = $true
        NoNewWindow = $true
    }

    $stdoutPath = $null
    $stderrPath = $null
    if ($CaptureOutput) {
        $stdoutPath = Join-Path $env:TEMP ("kcode-{0}-{1}-stdout.log" -f $FriendlyName, [guid]::NewGuid().ToString('N'))
        $stderrPath = Join-Path $env:TEMP ("kcode-{0}-{1}-stderr.log" -f $FriendlyName, [guid]::NewGuid().ToString('N'))
        $startParams.RedirectStandardOutput = $stdoutPath
        $startParams.RedirectStandardError = $stderrPath
    }

    $process = Start-Process @startParams
    # Wait-Process did not gain -Timeout until newer PowerShell releases. Use
    # the underlying .NET Process API so the documented PowerShell 5.1 minimum
    # is real rather than only passing on PowerShell 7.
    $timedOut = -not $process.WaitForExit($TimeoutSeconds * 1000)
    if ($timedOut) {
        Stop-ProcessTree -ProcessId $process.Id
        return [pscustomobject]@{
            TimedOut = $true
            ExitCode = $null
            StdoutPath = $stdoutPath
            StderrPath = $stderrPath
        }
    }

    # Ensure redirected streams have finished flushing before callers inspect
    # their files, then refresh ExitCode from the completed process.
    $process.WaitForExit()
    $process.Refresh()
    return [pscustomobject]@{
        TimedOut = $false
        ExitCode = $process.ExitCode
        StdoutPath = $stdoutPath
        StderrPath = $stderrPath
    }
}

function Write-LogTail([string]$Path, [string]$Label) {
    if (-not $Path -or -not (Test-Path $Path)) {
        return
    }

    $lines = Get-Content -Path $Path -Tail 40 -ErrorAction SilentlyContinue
    if ($lines -and $lines.Count -gt 0) {
        Write-Warn "$Label (last 40 lines):"
        $lines | ForEach-Object { Write-Host $_ }
    }
}

function Test-CommandExists([string]$CommandName) {
    return [bool](Get-Command $CommandName -ErrorAction SilentlyContinue)
}

function Test-AlacrittyInstalled {
    return [bool](Find-AlacrittyPath)
}

function Find-AlacrittyPath {
    $candidates = @(
        "C:\Program Files\Alacritty\alacritty.exe",
        "C:\Program Files (x86)\Alacritty\alacritty.exe"
    )

    if ($env:LOCALAPPDATA) {
        $candidates += (Join-Path $env:LOCALAPPDATA "Microsoft\WinGet\Links\alacritty.exe")
    }

    foreach ($candidate in $candidates) {
        if ($candidate -and (Test-Path $candidate)) {
            return $candidate
        }
    }

    try {
        $command = Get-Command alacritty -ErrorAction Stop
        if ($command -and $command.Source) {
            return $command.Source
        }
    } catch {}

    return $null
}

function Install-Alacritty {
    if (Test-AlacrittyInstalled) {
        Write-Info "Alacritty is already installed"
        return $true
    }

    if (-not (Test-CommandExists "winget")) {
        Write-Warn "winget was not found, so Alacritty could not be installed automatically"
        Write-Warn "Install App Installer / winget from Microsoft, then run: winget install -e --id Alacritty.Alacritty"
        return $false
    }

    Write-Info "Installing Alacritty..."
    $wingetArgs = @(
        "install",
        "-e",
        "--id", "Alacritty.Alacritty",
        "--accept-source-agreements",
        "--accept-package-agreements",
        "--disable-interactivity"
    )

    $wingetResult = Invoke-ProcessWithTimeout -FilePath "winget" -ArgumentList $wingetArgs -TimeoutSeconds 180 -FriendlyName "winget-install"
    if ($wingetResult.TimedOut) {
        Write-Warn "Alacritty install timed out after 180 seconds; skipping automatic setup"
        return $false
    }

    if ($wingetResult.ExitCode -ne 0) {
        Write-Warn "Alacritty install failed (winget exit code: $($wingetResult.ExitCode))"
        return $false
    }

    $alacrittyPath = Find-AlacrittyPath
    if (-not $alacrittyPath) {
        Write-Warn "Alacritty install finished, but alacritty.exe was not found on PATH yet"
        return $false
    }

    Write-Info "Alacritty installed: $alacrittyPath"
    return $true
}

function Stop-KcodeHotkeyListeners {
    try {
        Get-CimInstance Win32_Process -Filter "Name = 'powershell.exe' OR Name = 'pwsh.exe'" -ErrorAction SilentlyContinue |
            Where-Object { $_.CommandLine -like '*kcode-hotkey*' } |
            ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
    } catch {}

    try {
        $currentPid = $PID
        Get-CimInstance Win32_Process -Filter "Name = 'kcode.exe'" -ErrorAction SilentlyContinue |
            Where-Object { $_.ProcessId -ne $currentPid -and $_.CommandLine -like '*--listen-windows-hotkey*' } |
            ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
    } catch {}
}

function ConvertFrom-KcodeVersionOutput([string]$Output) {
    if (-not $Output) {
        return $null
    }

    # A genuinely fresh profile may print the one-time telemetry notice before
    # the version. When output is captured by PowerShell, terminal control
    # sequences can also leave the final `kcode v...` on the same logical line.
    if ($Output -match '(?i)\bkcode\s+v?([0-9][0-9A-Za-z.+-]*)') {
        return "v$($Matches[1])"
    }

    return $null
}

function Get-KcodeVersionFromBinary([string]$BinaryPath) {
    if (-not $BinaryPath -or -not (Test-Path -LiteralPath $BinaryPath)) {
        return $null
    }

    $previousErrorActionPreference = $ErrorActionPreference
    try {
        # Fresh profiles emit the one-time telemetry notice on stderr. Under
        # Windows PowerShell with ErrorActionPreference=Stop, native stderr is
        # promoted to a terminating NativeCommandError even when the process
        # succeeds. Capture both streams without letting that notice abort the
        # version probe.
        $ErrorActionPreference = 'Continue'
        $output = (& $BinaryPath --version 2>&1 | Out-String).Trim()
        $exitCode = $LASTEXITCODE
        if ($exitCode -ne 0) {
            return $null
        }
        return (ConvertFrom-KcodeVersionOutput $output)
    } catch {
        return $null
    } finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }
}

function Assert-KcodeBinaryCandidate {
    param(
        [Parameter(Mandatory = $true)][string]$BinaryPath,
        [Parameter(Mandatory = $true)][string]$ExpectedVersion
    )

    if ($env:KCODE_INSTALL_SKIP_BINARY_VALIDATION -eq "1") {
        return $null
    }

    $reportedVersion = Get-KcodeVersionFromBinary $BinaryPath
    if (-not $reportedVersion) {
        Write-Err "Downloaded kcode binary could not run '--version'. It may be corrupt, quarantined by antivirus, or built for the wrong architecture."
    }

    $expectedNumber = $ExpectedVersion.TrimStart('v')
    if ($reportedVersion.TrimStart('v') -ne $expectedNumber) {
        Write-Err "Downloaded binary reports $reportedVersion, but the installer requested $ExpectedVersion"
    }

    Write-Info "Validated kcode binary: $reportedVersion"
    return $reportedVersion
}

function Test-KcodeMsvcBuildToolsAvailable {
    if (Get-Command link.exe -ErrorAction SilentlyContinue) {
        return $true
    }

    $programFilesX86 = [Environment]::GetFolderPath([Environment+SpecialFolder]::ProgramFilesX86)
    if (-not $programFilesX86) {
        return $false
    }

    $vswhere = Join-Path $programFilesX86 "Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path -LiteralPath $vswhere)) {
        return $false
    }

    try {
        $linkPath = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -find 'VC\Tools\MSVC\**\bin\Hostx64\x64\link.exe' 2>$null | Select-Object -First 1
        return [bool]$linkPath
    } catch {
        return $false
    }
}

function Assert-KcodeSourceBuildPrerequisites {
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
        Write-Err "Git is required for -BuildFromSource. Install it with: winget install -e --id Git.Git"
    }
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue) -or -not (Get-Command rustc -ErrorAction SilentlyContinue)) {
        Write-Err "Rust is required for -BuildFromSource. Install it from https://rustup.rs, then open a new PowerShell window."
    }

    $rustHost = ""
    try {
        $rustHost = (& rustc -vV 2>$null | Select-String '^host:' | Select-Object -First 1).ToString()
    } catch {}

    if ($rustHost -match 'pc-windows-msvc' -and -not (Test-KcodeMsvcBuildToolsAvailable)) {
        Write-Err "The MSVC linker (link.exe) was not found. Install Visual Studio 2022 Build Tools with the 'Desktop development with C++' workload, then open a new PowerShell window before using -BuildFromSource."
    }
}

function Set-SetupHintsState([bool]$AlacrittyConfigured, [bool]$HotkeyConfigured) {
    New-Item -ItemType Directory -Path $KcodeHome -Force | Out-Null

    $state = @{
        launch_count = 0
        hotkey_configured = $HotkeyConfigured
        hotkey_dismissed = $HotkeyConfigured
        alacritty_configured = $AlacrittyConfigured
        alacritty_dismissed = $AlacrittyConfigured
        desktop_shortcut_created = $false
        mac_ghostty_guided = $false
        mac_ghostty_dismissed = $false
    }

    if (Test-Path $SetupHintsPath) {
        try {
            $existing = Get-Content $SetupHintsPath -Raw | ConvertFrom-Json -ErrorAction Stop
            foreach ($property in $existing.PSObject.Properties) {
                $state[$property.Name] = $property.Value
            }
        } catch {
            Write-Warn "Could not read existing setup hints state; overwriting it"
        }
    }

    if ($AlacrittyConfigured) {
        $state.alacritty_configured = $true
        $state.alacritty_dismissed = $true
    }

    if ($HotkeyConfigured) {
        $state.hotkey_configured = $true
        $state.hotkey_dismissed = $true
    }

    $state | ConvertTo-Json | Set-Content -Path $SetupHintsPath -Encoding UTF8
}

function Get-KcodeHotkeyShortcutScript([string]$StartupShortcutPath, [string]$KcodeExePath) {
    $escapedShortcutPath = $StartupShortcutPath.Replace("'", "''")
    $escapedExePath = $KcodeExePath.Replace("'", "''")
    $listenerArguments = "-NoProfile -ExecutionPolicy RemoteSigned -WindowStyle Hidden -Command `"& '$escapedExePath' setup-hotkey --listen-windows-hotkey`""
    $escapedListenerArguments = $listenerArguments.Replace("'", "''")
    $shortcutLines = @(
        '$ErrorActionPreference = ''Stop''',
        '$shell = New-Object -ComObject WScript.Shell',
        "`$shortcut = `$shell.CreateShortcut('$escapedShortcutPath')",
        "`$shortcut.TargetPath = 'powershell.exe'",
        "`$shortcut.Arguments = '$escapedListenerArguments'",
        "`$shortcut.Description = 'kcode global launch hotkey listener'",
        '$shortcut.WindowStyle = 7',
        '$shortcut.Save()',
        "Write-Output 'OK'"
    )
    return ($shortcutLines -join "`r`n")
}

function Install-KcodeHotkey([string]$KcodeExePath) {
    New-Item -ItemType Directory -Path $HotkeyDir -Force | Out-Null
    $skipProcessLifecycle = (
        $env:KCODE_WINDOWS_SETUP_SKIP_EXTERNALS -eq "1" -or
        $env:KCODE_WINDOWS_SETUP_SKIP_PROCESS_LIFECYCLE -eq "1"
    )
    if (-not $skipProcessLifecycle) {
        Stop-KcodeHotkeyListeners
    }

    # Upgrade cleanup: v0.47 and earlier wrote a generated PowerShell listener.
    # The first-party listener now lives in kcode.exe itself and is launched via
    # `kcode setup-hotkey --listen-windows-hotkey` from a login shortcut.
    Remove-Item -Path (Join-Path $HotkeyDir "kcode-hotkey.ps1") -Force -ErrorAction SilentlyContinue
    Remove-Item -Path (Join-Path $HotkeyDir "kcode-hotkey-launcher.vbs") -Force -ErrorAction SilentlyContinue
    $startupDir = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Startup"
    New-Item -ItemType Directory -Path $startupDir -Force | Out-Null
    $startupShortcutPath = Join-Path $startupDir "kcode-hotkey.lnk"
    $shortcutScript = Get-KcodeHotkeyShortcutScript -StartupShortcutPath $startupShortcutPath -KcodeExePath $KcodeExePath

    if ($env:KCODE_WINDOWS_SETUP_SKIP_EXTERNALS -eq "1") {
        Set-Content -Path (Join-Path $HotkeyDir "kcode-hotkey-shortcut.ps1") -Value $shortcutScript -Encoding UTF8
        Write-Info "Configured Alt+; and the Copilot key to launch kcode"
        return $true
    }

    $shortcutOutput = & powershell -NoProfile -Command $shortcutScript
    if ($LASTEXITCODE -ne 0 -or -not ($shortcutOutput -match 'OK')) {
        Write-Warn "Created hotkey files, but could not create the Startup shortcut"
        return $false
    }

    $escapedExePath = $KcodeExePath.Replace("'", "''")
    $launchHotkeyCommand = "Start-Process -FilePath '$escapedExePath' -ArgumentList @('setup-hotkey', '--listen-windows-hotkey') -WindowStyle Hidden"
    if (-not $skipProcessLifecycle) {
        & powershell -NoProfile -ExecutionPolicy RemoteSigned -WindowStyle Hidden -Command $launchHotkeyCommand | Out-Null
        if ($LASTEXITCODE -ne 0) {
            Write-Warn "Hotkey will start on next login, but could not be launched immediately"
        }
    }

    Write-Info "Configured Alt+; and the Copilot key to launch kcode"
    return $true
}
function Resolve-KcodeWindowsArtifact([string[]]$ArchitectureCandidates) {
    $sawX64 = $false

    foreach ($arch in @($ArchitectureCandidates)) {
        if (-not $arch) { continue }
        switch -Regex ($arch.Trim()) {
            '^(Arm64|ARM64|AARCH64|aarch64)$' { return "kcode-windows-aarch64" }
            '^(X64|AMD64|x86_64)$' { $sawX64 = $true }
        }
    }

    if ($sawX64) { return "kcode-windows-x86_64" }
    return $null
}

function Get-KcodeWindowsArtifact {
    $candidates = @()

    try {
        $runtimeArch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
        if ($runtimeArch) { $candidates += [string]$runtimeArch }
    } catch {}

    foreach ($envArch in @($env:PROCESSOR_ARCHITECTURE, $env:PROCESSOR_ARCHITEW6432)) {
        if ($envArch) { $candidates += [string]$envArch }
    }

    $artifact = Resolve-KcodeWindowsArtifact $candidates
    if ($artifact) { return $artifact }

    $displayArch = if ($candidates.Count -gt 0) { $candidates -join ", " } else { "<unknown>" }
    Write-Err "Unsupported architecture: $displayArch (supported: x86_64, ARM64)"
}

function Invoke-KcodeInstall {
$Artifact = Get-KcodeWindowsArtifact

$ResolvedArtifactExePath = Resolve-OptionalPath $ArtifactExePath
$ResolvedArtifactTgzPath = Resolve-OptionalPath $ArtifactTgzPath

if ($ResolvedArtifactExePath -and $ResolvedArtifactTgzPath) {
    Write-Err "Provide only one of -ArtifactExePath or -ArtifactTgzPath"
}

if (-not $Version) {
    if ($ResolvedArtifactExePath) {
        $Version = Get-KcodeVersionFromBinary $ResolvedArtifactExePath
        if (-not $Version) {
            Write-Err "Could not detect a kcode version from '$ResolvedArtifactExePath'. Pass -Version explicitly if this is a trusted local build."
        }
        Write-Info "Detected local artifact version: $Version"
    } elseif ($ResolvedArtifactTgzPath) {
        Write-Err "-Version is required when using -ArtifactTgzPath"
    } else {
        Write-Info "Fetching latest release..."
        $Version = Get-LatestKcodeReleaseTag
    }
}

if (-not $Version) { Write-Err "Failed to determine latest version" }

$VersionNum = $Version.TrimStart('v')
$DownloadBases = @() # Local artifacts only: no release metadata or network lookup.

$BuildsDir = if ($env:KCODE_HOME) { Join-Path $env:KCODE_HOME "builds" } else { Join-Path (Get-KcodeLocalAppDataDir) "kcode\builds" }
Assert-KcodeOwnedPath $BuildsDir
$StableDir = Join-Path $BuildsDir "stable"
$VersionDir = Join-Path $BuildsDir "versions\$VersionNum"
$LauncherPath = Join-Path $InstallDir "kcode.exe"

$Existing = ""
if (Test-Path $LauncherPath) {
    try { $Existing = & $LauncherPath --version 2>$null | Select-Object -First 1 } catch {}
}

if ($Existing) {
    if ($Existing -match [regex]::Escape($VersionNum)) {
        Write-Info "kcode $Version is already installed - reinstalling"
    } else {
        Write-Info "Updating kcode $Existing -> $Version"
    }
} else {
    Write-Info "Installing kcode $Version"
}
Write-Info "  launcher: $LauncherPath"

foreach ($d in @($InstallDir, $StableDir, $VersionDir)) {
    if (-not (Test-Path $d)) { New-Item -ItemType Directory -Path $d -Force | Out-Null }
}

$TempDir = Join-Path $env:TEMP "kcode-install-$(Get-Random)"
New-Item -ItemType Directory -Path $TempDir -Force | Out-Null

try {
$DownloadMode = ""
$DownloadPath = Join-Path $TempDir "kcode.download"
$DownloadedAssetName = $null

if ($ResolvedArtifactExePath) {
    Write-Info "Using local artifact exe: $ResolvedArtifactExePath"
    Copy-Item -Path $ResolvedArtifactExePath -Destination $DownloadPath -Force
    $DownloadMode = "bin"
} elseif ($ResolvedArtifactTgzPath) {
    Write-Info "Using local artifact archive: $ResolvedArtifactTgzPath"
    Copy-Item -Path $ResolvedArtifactTgzPath -Destination $DownloadPath -Force
    $DownloadMode = "tar"
} else {
    foreach ($candidate in @(
        @{ Name = "$Artifact.exe"; Mode = "bin" },
        @{ Name = "$Artifact.tar.gz"; Mode = "tar" }
    )) {
        foreach ($base in $DownloadBases) {
            try {
                Write-Info "Downloading $($candidate.Name) from $base..."
                Invoke-WebRequest -UseBasicParsing -Uri "$base/$($candidate.Name)" -OutFile $DownloadPath
                $DownloadMode = $candidate.Mode
                $DownloadedAssetName = $candidate.Name
                break
            } catch {}
        }
        if ($DownloadMode) { break }
    }
}

if (-not $ResolvedArtifactExePath -and -not $ResolvedArtifactTgzPath -and $DownloadMode) {
    $downloadedAssetName = if ($DownloadMode -eq "bin") { "$Artifact.exe" } else { "$Artifact.tar.gz" }
    $expectedSha256 = Get-ReleaseChecksum -ReleaseTag $Version -AssetName $downloadedAssetName
    Assert-KcodeFileChecksum -FilePath $DownloadPath -ExpectedSha256 $expectedSha256 -AssetName $downloadedAssetName | Out-Null
}

$DestBin = Join-Path $VersionDir "kcode.exe"

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
    if (-not $BuildFromSource) {
        $releaseUrl = "https://github.com/$Repo/releases/tag/$Version"
        Write-Err "No prebuilt $Artifact asset was found in $Version. Check $releaseUrl or rerun the downloaded script with -BuildFromSource. The installer will not start a long source build automatically."
    }

    Write-Info "No prebuilt asset found for $Artifact in $Version; -BuildFromSource was requested"
    Assert-KcodeSourceBuildPrerequisites

    $SrcDir = Join-Path $TempDir "kcode-src"
    Write-Info "Cloning $Repo at $Version..."
    $gitCloneResult = Invoke-ProcessWithTimeout -FilePath "git" -ArgumentList @(
        "clone",
        "--depth", "1",
        "--branch", $Version,
        "https://github.com/$Repo.git",
        $SrcDir
    ) -TimeoutSeconds 600 -FriendlyName "git-clone" -CaptureOutput
    if ($gitCloneResult.TimedOut) {
        Write-LogTail -Path $gitCloneResult.StdoutPath -Label "git stdout"
        Write-LogTail -Path $gitCloneResult.StderrPath -Label "git stderr"
        Write-Err "git clone timed out after 600 seconds"
    }
    if ($gitCloneResult.ExitCode -ne 0) {
        Write-LogTail -Path $gitCloneResult.StdoutPath -Label "git stdout"
        Write-LogTail -Path $gitCloneResult.StderrPath -Label "git stderr"
        Write-Err "Failed to clone $Repo at $Version (exit code: $($gitCloneResult.ExitCode))"
    }

    Write-Info "Building kcode from source (this can take several minutes)..."
    $cargoResult = Invoke-ProcessWithTimeout -FilePath "cargo" -ArgumentList @(
        "build", "--release", "--locked", "-p", "kcode", "--bin", "kcode",
        "--manifest-path", (Join-Path $SrcDir "Cargo.toml")
    ) -TimeoutSeconds 1800 -FriendlyName "cargo-build" -CaptureOutput
    if ($cargoResult.TimedOut) {
        Write-LogTail -Path $cargoResult.StdoutPath -Label "cargo stdout"
        Write-LogTail -Path $cargoResult.StderrPath -Label "cargo stderr"
        Write-Err "cargo build timed out after 1800 seconds"
    }
    if ($cargoResult.ExitCode -ne 0) {
        Write-LogTail -Path $cargoResult.StdoutPath -Label "cargo stdout"
        Write-LogTail -Path $cargoResult.StderrPath -Label "cargo stderr"
        Write-Err "cargo build failed (exit code: $($cargoResult.ExitCode))"
    }

    $BuiltBin = Join-Path $SrcDir "target\release\kcode.exe"
    if (-not (Test-Path $BuiltBin)) { Write-Err "Built binary not found at $BuiltBin" }
    Copy-Item -Path $BuiltBin -Destination $DestBin -Force
}

Assert-KcodeBinaryCandidate -BinaryPath $DestBin -ExpectedVersion $Version | Out-Null

$StableBin = Join-Path $StableDir "kcode.exe"
Copy-Item -Path $DestBin -Destination $StableBin -Force
Set-Content -Path (Join-Path $BuildsDir "stable-version") -Value $VersionNum
Install-KcodeLauncher -SourcePath $StableBin -LauncherPath $LauncherPath | Out-Null
} finally {
    Get-Item -LiteralPath $TempDir -ErrorAction SilentlyContinue |
        Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
}

# Gracefully reload any running background server onto the freshly installed
# binary (issue #291). `server reload` only reloads a genuinely-older daemon,
# hands its live sessions to the new process, and is a no-op when nothing is
# running, so it is safe to call unconditionally. Best-effort: never fail the
# install over it.
if ($env:KCODE_SKIP_SERVER_RELOAD -ne "1") {
    try {
        & $LauncherPath server reload 2>$null | Out-Null
    } catch {
    }
}

$userPathUpdate = Set-KcodeUserPath -InstallDir $InstallDir
if ($userPathUpdate.Changed) {
    Write-Info "Updated user PATH with $InstallDir"
    if ($userPathUpdate.RemovedManagedEntries -gt 0 -or $userPathUpdate.RemovedDuplicateEntries -gt 0) {
        Write-Info "  removed $($userPathUpdate.RemovedManagedEntries) stale kcode PATH entr$(if ($userPathUpdate.RemovedManagedEntries -eq 1) { 'y' } else { 'ies' }) and $($userPathUpdate.RemovedDuplicateEntries) duplicate entr$(if ($userPathUpdate.RemovedDuplicateEntries -eq 1) { 'y' } else { 'ies' })"
    }
} else {
    Write-Info "User PATH already contains $InstallDir"
}

Set-KcodeProcessPath -InstallDir $InstallDir | Out-Null

$installedAlacritty = $false
$configuredHotkey = $false
$shouldSetupAlacritty = [bool]($ConfigureAlacritty -and -not $SkipAlacrittySetup)
$shouldSetupHotkey = [bool]($ConfigureHotkey -and -not $SkipHotkeySetup)

if ($ConfigureAlacritty -and $SkipAlacrittySetup) {
    Write-Warn "Both -ConfigureAlacritty and -SkipAlacrittySetup were provided; skipping Alacritty setup"
}
if ($ConfigureHotkey -and $SkipHotkeySetup) {
    Write-Warn "Both -ConfigureHotkey and -SkipHotkeySetup were provided; skipping hotkey setup"
}

if ($shouldSetupAlacritty) {
    $installedAlacritty = Install-Alacritty
} else {
    $installedAlacritty = Test-AlacrittyInstalled
    Write-Info "Optional Alacritty setup not requested"
}

if ($shouldSetupHotkey) {
    $configuredHotkey = Install-KcodeHotkey -KcodeExePath $LauncherPath
} else {
    Write-Info "Optional global hotkey setup not requested"
}

Set-SetupHintsState -AlacrittyConfigured:(Test-AlacrittyInstalled) -HotkeyConfigured:$configuredHotkey

Write-Host ""
Write-Info "kcode $Version installed successfully!"
Write-Host ""

if (Test-AlacrittyInstalled) {
    $alacrittyPath = Find-AlacrittyPath
    if ($alacrittyPath) {
        Write-Info "Alacritty ready: $alacrittyPath"
    }
}

if ($configuredHotkey) {
    Write-Info "Global launch keys ready: Alt+; and the Copilot key open kcode"
    Write-Host ""
} elseif (-not $ConfigureHotkey) {
    Write-Info "Optional: run 'kcode setup-hotkey' to configure global launch hotkeys and terminal preferences."
    Write-Host ""
}

if (Get-Command kcode -ErrorAction SilentlyContinue) {
    Write-Info "Run 'kcode' to get started."
} else {
    Write-Host "  Open a new terminal window, then run:"
    Write-Host ""
    Write-Host "    kcode" -ForegroundColor Green
}
}

if ($env:KCODE_INSTALL_PS1_IMPORT_ONLY -ne "1") {
    Invoke-KcodeInstall
}
