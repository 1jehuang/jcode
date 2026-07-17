param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$installScript = Join-Path $repoRoot 'scripts\install.ps1'
$uninstallScript = Join-Path $repoRoot 'scripts\uninstall.ps1'
$testRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("jcode-windows-setup-eval-{0}" -f ([guid]::NewGuid().ToString('N')))

$envNames = @(
    'LOCALAPPDATA',
    'APPDATA',
    'USERPROFILE',
    'JCODE_HOME',
    'TEMP',
    'TMP',
    'PATH',
    'JCODE_INSTALL_PS1_IMPORT_ONLY',
    'JCODE_UNINSTALL_PS1_IMPORT_ONLY',
    'JCODE_SKIP_SERVER_RELOAD',
    'JCODE_INSTALL_SKIP_BINARY_VALIDATION',
    'JCODE_DISABLE_ENV_BROADCAST',
    'JCODE_WINDOWS_SETUP_SKIP_EXTERNALS'
)
$originalEnv = @{}
foreach ($name in $envNames) {
    $originalEnv[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}

function Restore-TestEnvironment {
    foreach ($name in $script:envNames) {
        $value = $script:originalEnv[$name]
        if ($null -eq $value) {
            [Environment]::SetEnvironmentVariable($name, $null, 'Process')
        } else {
            [Environment]::SetEnvironmentVariable($name, $value, 'Process')
        }
    }
}

function Assert-True($Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}

function Assert-False($Condition, [string]$Message) {
    if ($Condition) { throw $Message }
}

function Assert-Equal($Expected, $Actual, [string]$Message) {
    if ($Expected -ne $Actual) {
        throw "$Message`nExpected: $Expected`nActual:   $Actual"
    }
}

function Assert-Contains([string]$Haystack, [string]$Needle, [string]$Message) {
    if (-not $Haystack.Contains($Needle)) {
        throw "$Message`nMissing: $Needle"
    }
}

function Assert-NotContains([string]$Haystack, [string]$Needle, [string]$Message) {
    if ($Haystack.Contains($Needle)) {
        throw "$Message`nUnexpected: $Needle"
    }
}

function Assert-PathExists([string]$Path, [string]$Message) {
    Assert-True (Test-Path -LiteralPath $Path) $Message
}

function Assert-PathMissing([string]$Path, [string]$Message) {
    Assert-False (Test-Path -LiteralPath $Path) $Message
}

function Assert-PathCount([string]$PathValue, [string]$Entry, [int]$ExpectedCount, [string]$Message) {
    $entryKey = ConvertTo-JcodePathKey $Entry
    $count = 0
    foreach ($candidate in (Split-JcodePathList $PathValue)) {
        if ((ConvertTo-JcodePathKey $candidate) -eq $entryKey) { $count += 1 }
    }
    Assert-Equal $ExpectedCount $count $Message
}

function Invoke-Case([string]$Name, [scriptblock]$Body) {
    Write-Host "CASE $Name"
    & $Body
    $script:passedCases += 1
}

function New-IsolatedWindowsProfile([string]$Name) {
    $root = Join-Path $script:testRoot $Name
    $local = Join-Path $root 'Local App Data'
    $profile = Join-Path $root 'User Profile'
    $appData = Join-Path $profile 'AppData\Roaming'
    $jcodeHome = Join-Path $root 'Jcode Home'
    $temp = Join-Path $root 'Temp'
    foreach ($dir in @($local, $profile, $appData, $jcodeHome, $temp)) {
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
    }

    $env:LOCALAPPDATA = $local
    $env:USERPROFILE = $profile
    $env:APPDATA = $appData
    $env:JCODE_HOME = $jcodeHome
    $env:TEMP = $temp
    $env:TMP = $temp

    return [pscustomobject]@{
        Root = $root
        LocalAppData = $local
        UserProfile = $profile
        AppData = $appData
        JcodeHome = $jcodeHome
        Temp = $temp
        InstallDir = Join-Path $local 'jcode\bin'
        LauncherPath = Join-Path $local 'jcode\bin\jcode.exe'
        BuildsDir = Join-Path $local 'jcode\builds'
        SetupHintsPath = Join-Path $jcodeHome 'setup_hints.json'
        HotkeyDir = Join-Path $jcodeHome 'hotkey'
        StartupShortcutPath = Join-Path $appData 'Microsoft\Windows\Start Menu\Programs\Startup\jcode-hotkey.lnk'
    }
}

function Set-InstallScriptProfileGlobals($Profile) {
    $script:JcodeHome = $Profile.JcodeHome
    $script:HotkeyDir = $Profile.HotkeyDir
    $script:SetupHintsPath = $Profile.SetupHintsPath
}

function Get-CimInstance { @() }

$passedCases = 0
$coveredScenarios = [ordered]@{
    clean_install = $false
    upgrade_idempotency = $false
    path_persistence_deduplication = $false
    wm_settingchange = $false
    copilot_key_mapping = $false
    opt_out = $false
    uninstall_cleanup = $false
    spaces_non_ascii_paths = $false
    missing_windows_terminal = $false
    rollback_failure = $false
}
New-Item -ItemType Directory -Path $testRoot -Force | Out-Null

try {
    $env:JCODE_INSTALL_PS1_IMPORT_ONLY = '1'
    $env:JCODE_SKIP_SERVER_RELOAD = '1'
    $env:JCODE_INSTALL_SKIP_BINARY_VALIDATION = '1'
    . $installScript -SkipAlacrittySetup -SkipHotkeySetup

    Invoke-Case 'release_lookup_avoids_unauthenticated_github_api' {
        Assert-Equal 'v1.2.3' (Resolve-JcodeReleaseTagFromUri 'https://github.com/1jehuang/jcode/releases/tag/v1.2.3') 'release redirect parser should extract the stable tag'
        Assert-Equal 'v1.2.3-rc.1' (Resolve-JcodeReleaseTagFromUri 'https://github.com/1jehuang/jcode/releases/tag/v1.2.3-rc.1?source=latest') 'release redirect parser should stop before query parameters'
        Assert-Equal $true (Test-JcodeReleaseTag 'v1.2.3') 'stable release tags should validate'
        Assert-Equal $false (Test-JcodeReleaseTag 'latest') 'unversioned release labels should not validate'
        $scriptText = Get-Content -LiteralPath $installScript -Raw
        Assert-NotContains $scriptText 'api.github.com/repos/$Repo/releases/latest' 'installer should not use the rate-limited unauthenticated GitHub API'
        Assert-Contains $scriptText 'jcode.sh/releases' 'installer should include independent static release metadata'

        $script:releaseLookupRequests = @()
        function Invoke-WebRequest {
            param(
                [switch]$UseBasicParsing,
                [string]$Method,
                [Parameter(Mandatory = $true)][string]$Uri,
                [string]$OutFile
            )
            $script:releaseLookupRequests += $Uri
            if ($Uri -eq 'https://jcode.sh/releases/latest/version') {
                return [pscustomobject]@{ Content = "v1.2.3`n" }
            }
            if ($Uri -eq 'https://jcode.sh/releases/v1.2.3/download-bases') {
                return [pscustomobject]@{ Content = "https://mirror.example/releases/v1.2.3`n" }
            }
            if ($Uri -eq 'https://github.com/1jehuang/jcode/releases/latest') {
                throw 'simulated GitHub block'
            }
            throw "unexpected URI: $Uri"
        }
        try {
            Assert-Equal 'v1.2.3' (Get-LatestJcodeReleaseTag) 'static metadata should cover a blocked GitHub release lookup'
            $bases = @(Get-JcodeReleaseDownloadBases 'v1.2.3')
            Assert-Equal 'https://mirror.example/releases/v1.2.3' $bases[0] 'configured mirror should be preferred'
            Assert-Equal 'https://github.com/1jehuang/jcode/releases/download/v1.2.3' $bases[1] 'GitHub should remain the final fallback'
        } finally {
            Remove-Item Function:\Invoke-WebRequest -ErrorAction SilentlyContinue
        }
    }

    Invoke-Case 'path_persistence_and_deduplication' {
        $profile = New-IsolatedWindowsProfile 'path-dedupe'
        $installVariant = ($profile.InstallDir.ToUpperInvariant() + '\')
        $currentPath = "C:\Tools;$installVariant;$($profile.InstallDir);C:\Tools\;C:\Other"
        $pathUpdate = Resolve-JcodePathUpdate -InstallDir $profile.InstallDir -CurrentPath $currentPath
        Assert-Equal "$($profile.InstallDir);C:\Tools;C:\Tools\;C:\Other" $pathUpdate.Path 'install PATH update should prepend the canonical launcher dir without rewriting unrelated entries'
        Assert-PathCount $pathUpdate.Path $profile.InstallDir 1 'updated PATH should contain exactly one jcode launcher dir'
        Assert-Equal 2 $pathUpdate.RemovedManagedEntries 'PATH update should remove both stale jcode launcher entries before re-adding one'
        Assert-Equal 0 $pathUpdate.RemovedDuplicateEntries 'PATH update should preserve unrelated duplicate entries'

        $script:setCalls = 0
        $script:broadcastCalls = 0
        $script:appliedPath = $null
        $setPathAction = { param($value) $script:setCalls += 1; $script:appliedPath = $value }
        $broadcastAction = { $script:broadcastCalls += 1 }
        $first = Set-JcodeUserPath -InstallDir $profile.InstallDir -CurrentPath 'C:\Tools' -SetUserPathAction $setPathAction -BroadcastAction $broadcastAction
        Assert-Equal 1 $script:setCalls 'user PATH setter should be called once when PATH changes'
        Assert-Equal 1 $script:broadcastCalls 'environment broadcast should be called once when PATH changes'
        Assert-Equal $true $first.Broadcasted 'changed PATH update should report a broadcast'
        $second = Set-JcodeUserPath -InstallDir $profile.InstallDir -CurrentPath $script:appliedPath -SetUserPathAction $setPathAction -BroadcastAction $broadcastAction
        Assert-Equal 1 $script:setCalls 'user PATH setter should not be called when PATH is already correct'
        Assert-Equal 1 $script:broadcastCalls 'environment broadcast should not be called when PATH is unchanged'
        Assert-Equal $false $second.Broadcasted 'unchanged PATH update should not report a broadcast'
        $script:coveredScenarios.path_persistence_deduplication = $true
    }

    Invoke-Case 'wm_settingchange_broadcast_contract' {
        $text = Get-Content -LiteralPath $installScript -Raw
        Assert-Contains $text '0x001A' 'installer should broadcast WM_SETTINGCHANGE'
        Assert-Contains $text '"Environment"' 'installer should broadcast the Environment lParam'
        Assert-Contains $text 'SendMessageTimeout([IntPtr]0xffff' 'installer should broadcast to HWND_BROADCAST'
        $env:JCODE_DISABLE_ENV_BROADCAST = '1'
        Assert-Equal $false (Send-JcodeEnvironmentChangedBroadcast) 'broadcast helper should honor the deterministic no-mutate opt-out'
        $env:JCODE_DISABLE_ENV_BROADCAST = $null
        $script:coveredScenarios.wm_settingchange = $true
    }

    $script:mockUserPath = $null
    $script:pathWrites = 0
    $script:pathBroadcasts = 0
    function Set-JcodeUserPath {
        param(
            [Parameter(Mandatory = $true)][string]$InstallDir,
            [AllowNull()][string]$CurrentPath,
            [scriptblock]$SetUserPathAction,
            [scriptblock]$BroadcastAction,
            [bool]$Broadcast = $true
        )
        $update = Resolve-JcodePathUpdate -InstallDir $InstallDir -CurrentPath $script:mockUserPath
        if ($update.Changed) {
            $script:mockUserPath = $update.Path
            $script:pathWrites += 1
            if ($Broadcast) { $script:pathBroadcasts += 1 }
        }
        $update | Add-Member -NotePropertyName Broadcasted -NotePropertyValue ([bool]($update.Changed -and $Broadcast))
        return $update
    }
    function Test-AlacrittyInstalled { return $false }
    function Find-AlacrittyPath { return $null }

    Invoke-Case 'clean_install_isolated_profile_and_opt_out' {
        $profile = New-IsolatedWindowsProfile 'clean-install'
        Set-InstallScriptProfileGlobals $profile
        $source = Join-Path $profile.Root 'jcode-v1.exe'
        Set-Content -Path $source -Value 'version-one' -NoNewline
        $script:mockUserPath = 'C:\Tools'
        $script:pathWrites = 0
        $script:pathBroadcasts = 0
        $script:InstallDir = $profile.InstallDir
        $script:Version = 'v0.0.1-eval'
        $script:ArtifactExePath = $source
        $script:ArtifactTgzPath = $null
        $script:SkipAlacrittySetup = $true
        $script:SkipHotkeySetup = $true

        Invoke-JcodeInstall

        Assert-PathExists $profile.LauncherPath 'clean install should create the launcher in the isolated LOCALAPPDATA tree'
        Assert-Equal 'version-one' (Get-Content -LiteralPath $profile.LauncherPath -Raw) 'launcher should contain the local artifact contents'
        Assert-PathExists (Join-Path $profile.BuildsDir 'stable\jcode.exe') 'clean install should populate the stable build channel'
        Assert-PathExists (Join-Path $profile.BuildsDir 'versions\0.0.1-eval\jcode.exe') 'clean install should populate the immutable versioned build'
        Assert-PathCount $script:mockUserPath $profile.InstallDir 1 'clean install should persist exactly one launcher PATH entry in the mocked user PATH'
        Assert-Equal 1 $script:pathWrites 'clean install should write mocked user PATH once'
        Assert-Equal 1 $script:pathBroadcasts 'clean install should broadcast exactly once for a PATH change'
        Assert-PathExists $profile.SetupHintsPath 'clean install should write setup hints into isolated JCODE_HOME'
        $state = Get-Content -LiteralPath $profile.SetupHintsPath -Raw | ConvertFrom-Json
        Assert-Equal $false $state.hotkey_configured 'SkipHotkeySetup should record hotkey opt-out without configuring the listener'
        Assert-Equal $false $state.alacritty_configured 'SkipAlacrittySetup should avoid deterministic terminal installation side effects'
        Assert-PathMissing $profile.HotkeyDir 'hotkey opt-out should not create hotkey files'
        $script:coveredScenarios.clean_install = $true
        $script:coveredScenarios.opt_out = $true
    }

    Invoke-Case 'upgrade_and_idempotency_do_not_duplicate_path' {
        $profile = New-IsolatedWindowsProfile 'upgrade-install'
        Set-InstallScriptProfileGlobals $profile
        $sourceV1 = Join-Path $profile.Root 'jcode-v1.exe'
        $sourceV2 = Join-Path $profile.Root 'jcode-v2.exe'
        Set-Content -Path $sourceV1 -Value 'version-one' -NoNewline
        Set-Content -Path $sourceV2 -Value 'version-two' -NoNewline
        $script:mockUserPath = 'C:\Tools'
        $script:pathWrites = 0
        $script:pathBroadcasts = 0
        $script:InstallDir = $profile.InstallDir
        $script:SkipAlacrittySetup = $true
        $script:SkipHotkeySetup = $true

        $script:Version = 'v1.0.0-eval'
        $script:ArtifactExePath = $sourceV1
        Invoke-JcodeInstall
        $writesAfterFirstInstall = $script:pathWrites
        $broadcastsAfterFirstInstall = $script:pathBroadcasts

        $script:Version = 'v1.0.1-eval'
        $script:ArtifactExePath = $sourceV2
        Invoke-JcodeInstall
        Assert-Equal 'version-two' (Get-Content -LiteralPath $profile.LauncherPath -Raw) 'upgrade should replace launcher contents with the new build'
        Assert-PathCount $script:mockUserPath $profile.InstallDir 1 'upgrade should preserve exactly one launcher PATH entry'
        Assert-Equal $writesAfterFirstInstall $script:pathWrites 'upgrade should not rewrite PATH when it is already correct'
        Assert-Equal $broadcastsAfterFirstInstall $script:pathBroadcasts 'upgrade should not rebroadcast when PATH is unchanged'

        Invoke-JcodeInstall
        Assert-PathCount $script:mockUserPath $profile.InstallDir 1 'reinstalling same version should remain idempotent for PATH'
        Assert-Equal $writesAfterFirstInstall $script:pathWrites 'idempotent reinstall should not write PATH again'
        $script:coveredScenarios.upgrade_idempotency = $true
    }

    Invoke-Case 'copilot_key_mapping_and_spaces_non_ascii_paths' {
        $profile = New-IsolatedWindowsProfile 'hotkey-spaces-nonascii'
        Set-InstallScriptProfileGlobals $profile
        $env:JCODE_WINDOWS_SETUP_SKIP_EXTERNALS = '1'
        $jcodeExe = Join-Path $profile.Root '路径 With Spaces\jcode.exe'
        New-Item -ItemType Directory -Path (Split-Path -Parent $jcodeExe) -Force | Out-Null
        Set-Content -Path $jcodeExe -Value 'fake exe' -NoNewline
        New-Item -ItemType Directory -Path $profile.HotkeyDir -Force | Out-Null
        Set-Content -Path (Join-Path $profile.HotkeyDir 'jcode-hotkey.ps1') -Value 'legacy listener' -Force

        $ok = Install-JcodeHotkey -JcodeExePath $jcodeExe
        Assert-Equal $true $ok 'hotkey install should succeed using the deterministic external-command skip hook'
        $vbsPath = Join-Path $profile.HotkeyDir 'jcode-hotkey-launcher.vbs'
        Assert-PathMissing $vbsPath 'hotkey install should remove the legacy hidden VBScript trampoline'
        $shortcutScriptPath = Join-Path $profile.HotkeyDir 'jcode-hotkey-shortcut.ps1'
        Assert-PathExists $shortcutScriptPath 'hotkey install should render the deterministic Startup shortcut script under the isolated JCODE_HOME'
        $shortcutScript = Get-Content -LiteralPath $shortcutScriptPath -Raw
        Assert-Contains $shortcutScript 'powershell.exe' 'Startup shortcut should target PowerShell directly'
        Assert-Contains $shortcutScript 'ExecutionPolicy RemoteSigned' 'Startup shortcut should use RemoteSigned execution policy'
        Assert-NotContains $shortcutScript 'ExecutionPolicy Bypass' 'Startup shortcut should not bypass execution policy'
        Assert-Contains $shortcutScript 'setup-hotkey --listen-windows-hotkey' 'Startup shortcut should start the native Windows hotkey listener'
        Assert-Contains $shortcutScript $jcodeExe 'Startup shortcut should preserve spaces and non-ASCII characters in the jcode path'
        Assert-PathMissing (Join-Path $profile.HotkeyDir 'jcode-hotkey.ps1') 'hotkey upgrade should remove the legacy PowerShell listener'
        $scriptText = Get-Content -LiteralPath $installScript -Raw
        Assert-Contains $scriptText 'Configured Alt+; and the Copilot key' 'installer should document both Windows launch-key mappings'
        $script:coveredScenarios.copilot_key_mapping = $true
        $script:coveredScenarios.spaces_non_ascii_paths = $true
    }

    Invoke-Case 'missing_windows_terminal_is_not_required' {
        $profile = New-IsolatedWindowsProfile 'missing-windows-terminal'
        Set-InstallScriptProfileGlobals $profile
        $env:JCODE_WINDOWS_SETUP_SKIP_EXTERNALS = '1'
        $env:WT_SESSION = $null
        $env:WindowsTerminal = $null
        $jcodeExe = Join-Path $profile.Root 'No Windows Terminal\jcode.exe'
        New-Item -ItemType Directory -Path (Split-Path -Parent $jcodeExe) -Force | Out-Null
        Set-Content -Path $jcodeExe -Value 'fake exe' -NoNewline

        Assert-Equal $true (Install-JcodeHotkey -JcodeExePath $jcodeExe) 'hotkey setup should not require Windows Terminal to be installed or active'
        $scriptText = Get-Content -LiteralPath $installScript -Raw
        Assert-NotContains $scriptText 'wt.exe' 'installer should not shell out to Windows Terminal for hotkey setup'
        $script:coveredScenarios.missing_windows_terminal = $true
    }

    Invoke-Case 'launcher_rollback_failure_preserves_existing_install' {
        $profile = New-IsolatedWindowsProfile 'rollback-failure'
        New-Item -ItemType Directory -Path $profile.InstallDir -Force | Out-Null
        Set-Content -Path $profile.LauncherPath -Value 'known-good' -NoNewline
        $missingSource = Join-Path $profile.Root 'missing-source.exe'
        $threw = $false
        try {
            Install-JcodeLauncher -SourcePath $missingSource -LauncherPath $profile.LauncherPath | Out-Null
        } catch {
            $threw = $true
        }
        Assert-Equal $true $threw 'launcher install should surface copy failures'
        Assert-Equal 'known-good' (Get-Content -LiteralPath $profile.LauncherPath -Raw) 'failed launcher install should preserve the existing launcher'
        $tempLaunchers = @(Get-ChildItem -LiteralPath $profile.InstallDir -Filter '.jcode-launcher-*.tmp.exe' -Force -ErrorAction SilentlyContinue)
        Assert-Equal 0 $tempLaunchers.Count 'failed launcher install should not leave temporary launcher files behind'
        $script:coveredScenarios.rollback_failure = $true
    }

    $env:JCODE_UNINSTALL_PS1_IMPORT_ONLY = '1'
    . $uninstallScript

    function Get-CimInstance { @() }
    $script:uninstallUserPath = $null
    $script:uninstallSetCalls = 0
    $script:uninstallBroadcasts = 0
    function Remove-JcodeUserPath {
        param(
            [Parameter(Mandatory = $true)][string]$InstallDir,
            [AllowNull()][string]$CurrentPath,
            [scriptblock]$SetUserPathAction,
            [scriptblock]$BroadcastAction,
            [bool]$Broadcast = $true
        )
        $update = Resolve-JcodePathRemoval -InstallDir $InstallDir -CurrentPath $script:uninstallUserPath
        if ($update.Changed) {
            $script:uninstallUserPath = $update.Path
            $script:uninstallSetCalls += 1
            if ($Broadcast) { $script:uninstallBroadcasts += 1 }
        }
        $update | Add-Member -NotePropertyName Broadcasted -NotePropertyValue ([bool]($update.Changed -and $Broadcast))
        return $update
    }

    Invoke-Case 'uninstall_guards_purge_paths_and_process_scope' {
        $profile = New-IsolatedWindowsProfile 'uninstall-safety'
        Assert-Equal $true (Test-JcodeSafePurgePath $profile.JcodeHome) 'dedicated jcode data directories should be purgeable'
        Assert-Equal $false (Test-JcodeSafePurgePath $profile.UserProfile) 'the user profile must never be accepted as a purge target'
        Assert-Equal $false (Test-JcodeSafePurgePath $profile.Root) 'a parent workspace must never be accepted as a purge target'
        Assert-Equal $true (Test-JcodeManagedExecutablePath -ExecutablePath $profile.LauncherPath -LauncherPath $profile.LauncherPath -BuildsDir $profile.BuildsDir) 'the installed launcher should be recognized as managed'
        Assert-Equal $true (Test-JcodeManagedExecutablePath -ExecutablePath (Join-Path $profile.InstallDir '.jcode-launcher-old-a1b2c3.exe') -LauncherPath $profile.LauncherPath -BuildsDir $profile.BuildsDir) 'a renamed live-upgrade launcher should be recognized as managed'
        Assert-Equal $false (Test-JcodeManagedExecutablePath -ExecutablePath (Join-Path $profile.InstallDir 'other-tool.exe') -LauncherPath $profile.LauncherPath -BuildsDir $profile.BuildsDir) 'unrelated executables beside the launcher must not be terminated'
        Assert-Equal $true (Test-JcodeManagedExecutablePath -ExecutablePath (Join-Path $profile.BuildsDir 'stable\jcode.exe') -LauncherPath $profile.LauncherPath -BuildsDir $profile.BuildsDir) 'installed version binaries should be recognized as managed'
        Assert-Equal $false (Test-JcodeManagedExecutablePath -ExecutablePath (Join-Path $profile.Root 'development\jcode.exe') -LauncherPath $profile.LauncherPath -BuildsDir $profile.BuildsDir) 'unrelated development binaries must not be terminated'
    }

    Invoke-Case 'uninstall_cleanup_removes_binaries_path_and_keeps_user_data' {
        $profile = New-IsolatedWindowsProfile 'uninstall-cleanup'
        New-Item -ItemType Directory -Path $profile.InstallDir -Force | Out-Null
        New-Item -ItemType Directory -Path (Join-Path $profile.BuildsDir 'stable') -Force | Out-Null
        New-Item -ItemType Directory -Path $profile.JcodeHome -Force | Out-Null
        New-Item -ItemType Directory -Path $profile.HotkeyDir -Force | Out-Null
        New-Item -ItemType Directory -Path (Split-Path -Parent $profile.StartupShortcutPath) -Force | Out-Null
        Set-Content -Path $profile.LauncherPath -Value 'installed launcher' -NoNewline
        $oldLauncherPath = Join-Path $profile.InstallDir '.jcode-launcher-old-a1b2c3.exe'
        Set-Content -Path $oldLauncherPath -Value 'previous running launcher' -NoNewline
        Set-Content -Path (Join-Path $profile.BuildsDir 'stable\jcode.exe') -Value 'stable build' -NoNewline
        Set-Content -Path (Join-Path $profile.JcodeHome 'config.toml') -Value 'kept = true' -NoNewline
        Set-Content -Path (Join-Path $profile.HotkeyDir 'jcode-hotkey.ps1') -Value 'legacy listener' -NoNewline
        Set-Content -Path $profile.StartupShortcutPath -Value 'startup shortcut' -NoNewline
        @{ hotkey_configured = $true; hotkey_dismissed = $false } | ConvertTo-Json | Set-Content -Path $profile.SetupHintsPath -Encoding UTF8
        $installVariant = ($profile.InstallDir.ToUpperInvariant() + '\')
        $script:uninstallUserPath = "$($profile.InstallDir);C:\Keep;$installVariant"
        $script:uninstallSetCalls = 0
        $script:uninstallBroadcasts = 0

        $exitCode = Invoke-JcodeUninstall -InstallDir $profile.InstallDir -Yes
        Assert-Equal 0 $exitCode 'uninstall should complete successfully in the isolated profile'
        Assert-PathMissing $profile.LauncherPath 'uninstall should remove the launcher'
        Assert-PathMissing $oldLauncherPath 'uninstall should remove renamed live-upgrade launchers'
        Assert-PathMissing $profile.InstallDir 'uninstall should remove the empty launcher directory'
        Assert-PathMissing $profile.BuildsDir 'uninstall should remove installed build binaries'
        Assert-PathMissing $profile.StartupShortcutPath 'uninstall should remove the launch-hotkey Startup shortcut'
        Assert-PathMissing (Join-Path $profile.HotkeyDir 'jcode-hotkey.ps1') 'uninstall should remove legacy launch-hotkey artifacts'
        Assert-PathExists (Join-Path $profile.JcodeHome 'config.toml') 'uninstall without -Purge should keep user data'
        $setupHints = Get-Content -LiteralPath $profile.SetupHintsPath -Raw | ConvertFrom-Json
        Assert-Equal $false $setupHints.hotkey_configured 'uninstall should clear the persisted hotkey-configured state'
        Assert-Equal $true $setupHints.hotkey_dismissed 'uninstall should keep the removed hotkey prompt dismissed'
        Assert-Equal 'C:\Keep' $script:uninstallUserPath 'uninstall should remove all jcode-managed PATH variants and keep unrelated entries'
        Assert-Equal 1 $script:uninstallSetCalls 'uninstall should write mocked user PATH once when cleanup changes it'
        Assert-Equal 1 $script:uninstallBroadcasts 'uninstall should broadcast once after PATH cleanup'
        $script:coveredScenarios.uninstall_cleanup = $true
    }

    $missingScenarios = @($coveredScenarios.GetEnumerator() | Where-Object { -not $_.Value } | ForEach-Object { $_.Key })
    Assert-Equal 0 $missingScenarios.Count ("missing scenario coverage: {0}" -f ($missingScenarios -join ', '))
    Write-Host "Scenario checklist: $($coveredScenarios.Count)/$($coveredScenarios.Count) requested Windows setup scenarios covered." -ForegroundColor Green
    Write-Host "All $passedCases Windows setup evaluation cases passed." -ForegroundColor Green
} finally {
    Restore-TestEnvironment
    Remove-Item -LiteralPath $testRoot -Recurse -Force -ErrorAction SilentlyContinue
}
