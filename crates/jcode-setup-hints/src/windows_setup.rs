use super::{SetupHintsState, StartupHints, read_choice};
use crate::windows_hotkeys::{self, WindowsHotkey};
use anyhow::{Context, Result};
use jcode_config_types::{LaunchHotkeyEntry, LaunchHotkeysConfig};
use jcode_storage as storage;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

fn detect_terminal() -> &'static str {
    if std::env::var("WT_SESSION").is_ok() {
        "windows-terminal"
    } else if std::env::var("WEZTERM_EXECUTABLE").is_ok() || std::env::var("WEZTERM_PANE").is_ok() {
        "wezterm"
    } else if std::env::var("ALACRITTY_WINDOW_ID").is_ok() {
        "alacritty"
    } else {
        "unknown"
    }
}

fn is_alacritty_installed() -> bool {
    std::process::Command::new("where")
        .arg("alacritty")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn is_winget_available() -> bool {
    std::process::Command::new("where")
        .arg("winget")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub(super) fn find_alacritty_path() -> Option<String> {
    let candidates = [
        r"C:\Program Files\Alacritty\alacritty.exe",
        r"C:\Program Files (x86)\Alacritty\alacritty.exe",
    ];
    for c in &candidates {
        if std::path::Path::new(c).exists() {
            return Some(c.to_string());
        }
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let p = format!(r"{}\Microsoft\WinGet\Links\alacritty.exe", local);
        if std::path::Path::new(&p).exists() {
            return Some(p);
        }
    }
    let output = std::process::Command::new("where")
        .arg("alacritty")
        .output()
        .ok()?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Some(line) = stdout.lines().next() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Resolve the `[launch_hotkeys]` config into Windows listener entries.
/// Empty config keeps the shared macOS/Linux launch layout (mapped to Alt on
/// Windows) and adds the native Copilot-key chord (Win+Shift+F23).
fn resolve_windows_hotkeys() -> Vec<WindowsHotkey> {
    let config = effective_windows_launch_hotkeys_config();
    if config.enabled == Some(false) {
        return Vec::new();
    }
    let exe_path = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "jcode".to_string());
    let last_dir = super::mac_hotkey_last_dir_file()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let last_repo = super::mac_hotkey_last_repo_file()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    crate::launch_hotkeys::resolve_launch_hotkeys(&config, &exe_path, &last_dir, &last_repo)
        .into_iter()
        .filter_map(|entry| {
            let chord = crate::keymap::KeyChord::parse(&entry.chord)?;
            let win_modifier = windows_hotkeys::raw_chord_uses_win_modifier(&entry.chord);
            Some(WindowsHotkey {
                chord,
                win_modifier,
                dir: entry.dir,
                self_dev: entry.args.iter().any(|a| a == "self-dev"),
                label: entry.label,
            })
        })
        .collect()
}

pub(super) fn primary_hotkey_display() -> Option<(String, String)> {
    resolve_windows_hotkeys()
        .into_iter()
        .find(|entry| !entry.self_dev && windows_hotkeys::hotkey_to_win32(entry).is_some())
        .map(|entry| {
            (
                entry.chord.canonical(),
                windows_hotkeys::display_windows_hotkey(&entry),
            )
        })
}

fn default_windows_launch_entries() -> Vec<LaunchHotkeyEntry> {
    let mut entries = crate::launch_hotkeys::default_launch_entries();
    entries.push(LaunchHotkeyEntry {
        chord: "win+shift+f23".to_string(),
        dir: "$HOME".to_string(),
        label: "home".to_string(),
        self_dev: false,
    });
    entries
}
fn effective_windows_launch_hotkeys_config() -> LaunchHotkeysConfig {
    let mut config = super::load_launch_hotkeys_config();
    if config.entries.is_empty() {
        config.entries = default_windows_launch_entries();
    }
    config
}

fn startup_dir() -> PathBuf {
    let appdata = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Users\Default\AppData\Roaming"));
    appdata.join(r"Microsoft\Windows\Start Menu\Programs\Startup")
}
fn startup_shortcut_path() -> PathBuf {
    startup_dir().join("jcode-hotkey.lnk")
}

fn hotkey_vbs_path() -> Result<PathBuf> {
    Ok(storage::jcode_dir()?
        .join("hotkey")
        .join("jcode-hotkey-launcher.vbs"))
}

fn legacy_hotkey_ps1_path() -> Result<PathBuf> {
    Ok(storage::jcode_dir()?
        .join("hotkey")
        .join("jcode-hotkey.ps1"))
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("failed to remove {}", path.display())),
    }
}

fn ps_single_quote(input: &str) -> String {
    format!("'{}'", input.replace('\'', "''"))
}

fn render_stop_windows_hotkey_listeners_script(current_pid: u32) -> String {
    format!(
        r#"
$current = {current_pid}
Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
  Where-Object {{
    ($_.ProcessId -ne $current) -and ($_.ProcessId -ne $PID) -and (
      (($_.Name -eq 'powershell.exe' -or $_.Name -eq 'pwsh.exe') -and $_.CommandLine -like '*jcode-hotkey*') -or
      (($_.Name -eq 'jcode.exe') -and $_.CommandLine -like '*--listen-windows-hotkey*')
    )
  }} |
  ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }}
"#
    )
}

fn stop_windows_hotkey_listeners() {
    let script = render_stop_windows_hotkey_listeners_script(std::process::id());
    let _ = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output();
}

fn render_startup_shortcut_script(shortcut_path: &Path, exe_path: &Path) -> String {
    let listener_command = format!(
        "& {} setup-hotkey --listen-windows-hotkey",
        ps_single_quote(&exe_path.to_string_lossy())
    );
    let listener_arguments = format!(
        "-NoProfile -ExecutionPolicy RemoteSigned -WindowStyle Hidden -Command \"{listener_command}\""
    );
    format!(
        r#"
$ErrorActionPreference = "Stop"
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut({shortcut_path})
$shortcut.TargetPath = 'powershell.exe'
$shortcut.Arguments = {listener_arguments}
$shortcut.Description = 'jcode global launch hotkey listener'
$shortcut.WindowStyle = 7
$shortcut.Save()
Write-Output 'OK'
"#,
        shortcut_path = ps_single_quote(&shortcut_path.to_string_lossy()),
        listener_arguments = ps_single_quote(&listener_arguments),
    )
}

fn create_startup_shortcut(exe_path: &Path) -> Result<()> {
    let startup_dir = startup_dir();
    std::fs::create_dir_all(&startup_dir)?;
    let shortcut_path = startup_shortcut_path();
    let script = render_startup_shortcut_script(&shortcut_path, exe_path);

    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output()?;
    if !output.status.success() {
        anyhow::bail!(
            "Failed to create startup shortcut: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.contains("OK") {
        anyhow::bail!("Startup shortcut creation did not confirm success");
    }
    Ok(())
}

pub(super) fn uninstall_windows_hotkey_listener() -> Result<()> {
    stop_windows_hotkey_listeners();
    remove_file_if_exists(&startup_shortcut_path())?;
    remove_file_if_exists(&hotkey_vbs_path()?)?;
    remove_file_if_exists(&legacy_hotkey_ps1_path()?)?;

    let mut state = SetupHintsState::load();
    state.hotkey_configured = false;
    state.hotkey_dismissed = true;
    state.save()?;
    eprintln!("  \x1b[32m✓\x1b[0m Removed jcode Windows launch-hotkey listener");
    Ok(())
}

fn create_hotkey_shortcut(_use_alacritty: bool) -> Result<()> {
    let config = effective_windows_launch_hotkeys_config();
    if config.enabled == Some(false) {
        uninstall_windows_hotkey_listener()?;
        anyhow::bail!("[launch_hotkeys].enabled is false; removed Windows hotkey listener");
    }

    let entries = resolve_windows_hotkeys();
    if !entries
        .iter()
        .any(|entry| windows_hotkeys::hotkey_to_win32(entry).is_some())
    {
        anyhow::bail!("no registerable launch hotkeys in config");
    }

    let exe = std::env::current_exe()?;
    let hotkey_dir = storage::jcode_dir()?.join("hotkey");
    std::fs::create_dir_all(&hotkey_dir)?;
    stop_windows_hotkey_listeners();

    // Upgrade cleanup: older builds generated a PowerShell listener. The new
    // first-party lifecycle runs the Rust binary directly and removes the stale
    // script so future upgrades cannot accidentally resurrect it.
    remove_file_if_exists(&legacy_hotkey_ps1_path()?)?;
    remove_file_if_exists(&hotkey_vbs_path()?)?;
    create_startup_shortcut(&exe)?;

    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let start_output = std::process::Command::new(&exe)
        .args(["setup-hotkey", "--listen-windows-hotkey"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
    if let Err(e) = start_output {
        eprintln!(
            "  \x1b[33m⚠\x1b[0m  Could not start hotkey listener now: {}",
            e
        );
        eprintln!("    It will start automatically on next login.");
    }

    Ok(())
}

fn launch_windows_hotkey(entry: &WindowsHotkey) -> Result<()> {
    let exe = std::env::current_exe()?;
    let last_dir = super::mac_hotkey_last_dir_file()?
        .to_string_lossy()
        .into_owned();
    let last_repo = super::mac_hotkey_last_repo_file()?
        .to_string_lossy()
        .into_owned();
    let cwd = crate::launch_hotkeys::resolve_target_dir(&entry.dir, &last_dir, &last_repo);
    let mut args = Vec::new();
    if entry.self_dev {
        args.push("self-dev".to_string());
    }
    let command = jcode_terminal_launch::TerminalCommand::new(exe, args)
        .title("jcode")
        .fresh_spawn()
        .kind("launch-hotkey");

    let launched =
        jcode_terminal_launch::spawn_command_in_new_terminal_with(&command, &cwd, |cmd| {
            cmd.spawn().map(|_| ())
        })?;
    if !launched {
        anyhow::bail!("no terminal found to launch jcode");
    }
    Ok(())
}

fn prewarm_windows_server() -> Option<std::process::Child> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let Ok(exe) = std::env::current_exe() else {
        return None;
    };
    // The listener is launched at interactive login and remains resident, so
    // it is the natural place to pay the daemon's cold-start cost before the
    // user opens a terminal. The child keeps one lightweight server connection
    // open using stdin as a lifetime pipe. Killing or uninstalling this listener
    // closes the pipe, so the helper exits and normal server idle shutdown resumes.
    std::process::Command::new(exe)
        .args(["--provider", "auto", "server", "keepalive"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .ok()
}

pub(super) fn run_windows_hotkey_listener() -> Result<()> {
    let entries: Vec<WindowsHotkey> = resolve_windows_hotkeys()
        .into_iter()
        .filter(|entry| windows_hotkeys::hotkey_to_win32(entry).is_some())
        .collect();
    if entries.is_empty() {
        return Ok(());
    }
    let _server_keepalive = prewarm_windows_server();
    windows_native_hotkey_loop(entries)
}

#[cfg(windows)]
fn wide_null(text: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(text)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(windows)]
const COPILOT_HOOK_MESSAGE: u32 = 0x8000 + 0x4A;
#[cfg(windows)]
static COPILOT_HOOK_THREAD_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
#[cfg(windows)]
static COPILOT_F23_HELD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Capture the physical Copilot key even when Windows Shell reserves
/// Win+Shift+F23 and rejects `RegisterHotKey`. The hook only posts a private
/// message to the listener thread; terminal launch work stays outside the hook.
#[cfg(windows)]
unsafe extern "system" fn copilot_keyboard_hook(
    code: i32,
    wparam: windows_sys::Win32::Foundation::WPARAM,
    lparam: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::LRESULT {
    use std::sync::atomic::Ordering;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_F23, VK_LWIN, VK_RWIN, VK_SHIFT,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, KBDLLHOOKSTRUCT, PostThreadMessageW, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN,
        WM_SYSKEYUP,
    };

    if code >= 0 {
        let event = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        if event.vkCode == u32::from(VK_F23) {
            let message = wparam as u32;
            if message == WM_KEYDOWN || message == WM_SYSKEYDOWN {
                let win_down = unsafe { GetAsyncKeyState(i32::from(VK_LWIN)) } < 0
                    || unsafe { GetAsyncKeyState(i32::from(VK_RWIN)) } < 0;
                let shift_down = unsafe { GetAsyncKeyState(i32::from(VK_SHIFT)) } < 0;
                if win_down && shift_down {
                    let first_press = !COPILOT_F23_HELD.swap(true, Ordering::Relaxed);
                    if first_press {
                        let thread_id = COPILOT_HOOK_THREAD_ID.load(Ordering::Relaxed);
                        if thread_id != 0 {
                            let posted = unsafe {
                                PostThreadMessageW(thread_id, COPILOT_HOOK_MESSAGE, 0, 0)
                            } != 0;
                            if !posted {
                                COPILOT_F23_HELD.store(false, Ordering::Relaxed);
                                return unsafe {
                                    CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
                                };
                            }
                        }
                    }
                    return 1;
                }
            } else if (message == WM_KEYUP || message == WM_SYSKEYUP)
                && COPILOT_F23_HELD.swap(false, Ordering::Relaxed)
            {
                return 1;
            }
        }
    }

    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

#[cfg(windows)]
fn windows_native_hotkey_loop(entries: Vec<WindowsHotkey>) -> Result<()> {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError};
    use windows_sys::Win32::System::Threading::{CreateMutexW, GetCurrentThreadId};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
        WH_KEYBOARD_LL, WM_HOTKEY,
    };

    const MUTEX_NAME: &str = "Local\\JcodeLaunchHotkeyListener";

    let mutex_name = wide_null(MUTEX_NAME);
    let mut retry_count = 0;
    let mutex = loop {
        let mutex = unsafe { CreateMutexW(std::ptr::null_mut(), 1, mutex_name.as_ptr()) };
        if mutex.is_null() {
            return Err(std::io::Error::last_os_error()).context("failed to create hotkey mutex");
        }
        if unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
            break mutex;
        }
        unsafe {
            CloseHandle(mutex);
        }
        if retry_count >= 40 {
            jcode_logging::warn("previous Windows launch-hotkey listener is still exiting");
            return Ok(());
        }
        retry_count += 1;
        std::thread::sleep(std::time::Duration::from_millis(50));
    };

    let mut registered: Vec<(i32, WindowsHotkey)> = Vec::new();
    let mut copilot_entry: Option<(i32, WindowsHotkey)> = None;
    for (index, entry) in entries.into_iter().enumerate() {
        let Some((mods, vk)) = windows_hotkeys::hotkey_to_win32(&entry) else {
            continue;
        };
        let id = 0x4A00_i32 + index as i32;
        if windows_hotkeys::is_copilot_hotkey(&entry) {
            copilot_entry = Some((id, entry));
            continue;
        }
        let ok = unsafe { RegisterHotKey(std::ptr::null_mut(), id, mods | MOD_NOREPEAT, vk) } != 0;
        if ok {
            registered.push((id, entry));
        } else {
            jcode_logging::warn(&format!(
                "failed to register Windows launch hotkey {}",
                windows_hotkeys::display_windows_hotkey(&entry)
            ));
        }
    }

    let mut copilot_hook = std::ptr::null_mut();
    if copilot_entry.is_some() {
        // `PostThreadMessageW` requires a thread message queue. Create it before
        // the global hook can receive the first physical Copilot-key event.
        let mut queue_probe: MSG = unsafe { std::mem::zeroed() };
        unsafe {
            PeekMessageW(&mut queue_probe, std::ptr::null_mut(), 0, 0, PM_NOREMOVE);
        }
        COPILOT_HOOK_THREAD_ID.store(
            unsafe { GetCurrentThreadId() },
            std::sync::atomic::Ordering::Relaxed,
        );
        COPILOT_F23_HELD.store(false, std::sync::atomic::Ordering::Relaxed);
        copilot_hook = unsafe {
            SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(copilot_keyboard_hook),
                std::ptr::null_mut(),
                0,
            )
        };
        if copilot_hook.is_null() {
            jcode_logging::warn(&format!(
                "failed to install Windows Copilot-key hook: {}",
                std::io::Error::last_os_error()
            ));
            COPILOT_HOOK_THREAD_ID.store(0, std::sync::atomic::Ordering::Relaxed);
            copilot_entry = None;
        }
    }

    if registered.is_empty() && copilot_entry.is_none() {
        unsafe {
            CloseHandle(mutex);
        }
        anyhow::bail!("no Windows launch hotkeys could be registered");
    }

    let result = (|| -> Result<()> {
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        loop {
            let rc = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
            if rc == 0 {
                break;
            }
            if rc < 0 {
                return Err(std::io::Error::last_os_error()).context("GetMessageW failed");
            }
            let entry = if msg.message == WM_HOTKEY {
                let id = msg.wParam as i32;
                registered
                    .iter()
                    .find(|(registered_id, _)| *registered_id == id)
                    .map(|(_, entry)| entry)
            } else if msg.message == COPILOT_HOOK_MESSAGE {
                copilot_entry.as_ref().map(|(_, entry)| entry)
            } else {
                None
            };
            if let Some(entry) = entry.cloned() {
                std::thread::spawn(move || {
                    if let Err(err) = launch_windows_hotkey(&entry) {
                        jcode_logging::warn(&format!(
                            "failed to launch jcode from Windows hotkey: {err}"
                        ));
                    }
                });
            }
        }
        Ok(())
    })();

    for (id, _) in &registered {
        unsafe {
            UnregisterHotKey(std::ptr::null_mut(), *id);
        }
    }
    if !copilot_hook.is_null() {
        unsafe {
            UnhookWindowsHookEx(copilot_hook);
        }
    }
    COPILOT_HOOK_THREAD_ID.store(0, std::sync::atomic::Ordering::Relaxed);
    COPILOT_F23_HELD.store(false, std::sync::atomic::Ordering::Relaxed);
    unsafe {
        CloseHandle(mutex);
    }
    result
}

#[cfg(not(windows))]
fn windows_native_hotkey_loop(_entries: Vec<WindowsHotkey>) -> Result<()> {
    Ok(())
}

/// Build the TUI startup notice for the Windows launch hotkeys (or `None` when
/// there is nothing to show). Mirrors the macOS/Linux notices with Windows-native
/// display labels. Only shown once the listener is configured, since Windows needs the
/// interactive `jcode setup-hotkey` flow to install it.
pub(super) fn windows_launch_hotkeys_notice(state: &SetupHintsState) -> Option<StartupHints> {
    if !state.hotkey_configured {
        return None;
    }
    let config = super::load_launch_hotkeys_config();
    if config.enabled == Some(false) {
        return None;
    }

    let last_dir = super::mac_hotkey_last_dir_file()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let last_repo = super::mac_hotkey_last_repo_file()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();

    let rows: Vec<super::LaunchHotkeyRow> = resolve_windows_hotkeys()
        .into_iter()
        .filter(|hk| windows_hotkeys::hotkey_to_win32(hk).is_some())
        .map(|hk| {
            let cwd = crate::launch_hotkeys::resolve_target_dir(&hk.dir, &last_dir, &last_repo);
            super::LaunchHotkeyRow {
                chord: hk.chord.canonical(),
                display: windows_hotkeys::display_windows_hotkey(&hk),
                label: hk.label.clone(),
                cwd_display: cwd.display().to_string(),
                self_dev: hk.self_dev,
            }
        })
        .collect();

    let lines =
        super::launch_hotkey_notice_lines(&rows, &state.launch_hotkey_usage, state.launch_count)?;

    Some(StartupHints::with_status_and_display(
        "Launch hotkeys available".to_string(),
        "Launch hotkeys",
        format!(
            "Configured Jcode launch hotkeys:\n{}\n\nThese fire system-wide.",
            lines.join("\n")
        ),
    ))
}

/// Reinstall the Windows hotkey listener after the `[launch_hotkeys]` config
/// changed. No-op unless the user already configured the hotkey (we never
/// install behind someone who opted out). Best-effort.
pub(super) fn reinstall_windows_launch_hotkeys() {
    let state = SetupHintsState::load();
    if !state.hotkey_configured {
        return;
    }
    match refresh_windows_launch_hotkeys() {
        Ok(()) => jcode_logging::info("Reinstalled Windows launch hotkeys after config change"),
        Err(err) => jcode_logging::warn(&format!(
            "failed to reinstall Windows launch hotkeys: {err}"
        )),
    }
}

pub(super) fn refresh_windows_launch_hotkeys() -> Result<()> {
    let use_alacritty = detect_terminal() == "alacritty" || is_alacritty_installed();
    create_hotkey_shortcut(use_alacritty)
}

fn install_alacritty() -> Result<()> {
    eprintln!("  Installing Alacritty via winget...");
    eprintln!("  (Windows may ask for permission to install)\n");

    let status = std::process::Command::new("winget")
        .args([
            "install",
            "-e",
            "--id",
            "Alacritty.Alacritty",
            "--accept-source-agreements",
        ])
        .status()?;

    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("winget install failed (exit code: {:?})", status.code())
    }
}

fn nudge_hotkey(state: &mut SetupHintsState) -> bool {
    let terminal = detect_terminal();
    let using_alacritty = terminal == "alacritty" || is_alacritty_installed();

    let terminal_name = if using_alacritty {
        "Alacritty"
    } else {
        "Windows Terminal"
    };

    eprintln!("\x1b[36m┌─────────────────────────────────────────────────────────────┐\x1b[0m");
    eprintln!(
        "\x1b[36m│\x1b[0m \x1b[1m💡 Set up global keys to launch jcode?\x1b[0m                  \x1b[36m│\x1b[0m"
    );
    eprintln!(
        "\x1b[36m│\x1b[0m                                                             \x1b[36m│\x1b[0m"
    );
    eprintln!(
        "\x1b[36m│\x1b[0m    Creates a global hotkey - no extra software needed.       \x1b[36m│\x1b[0m"
    );
    eprintln!(
        "\x1b[36m│\x1b[0m    Opens jcode in {:<39}    \x1b[36m│\x1b[0m",
        format!("{}.", terminal_name)
    );
    eprintln!(
        "\x1b[36m│\x1b[0m                                                             \x1b[36m│\x1b[0m"
    );
    eprintln!(
        "\x1b[36m│\x1b[0m    \x1b[32m[y]\x1b[0m Set up   \x1b[90m[n]\x1b[0m Not now   \x1b[90m[d]\x1b[0m Don't ask again        \x1b[36m│\x1b[0m"
    );
    eprintln!("\x1b[36m└─────────────────────────────────────────────────────────────┘\x1b[0m");
    eprint!("\x1b[36m  >\x1b[0m ");
    let _ = io::stderr().flush();

    let choice = read_choice();

    match choice.as_str() {
        "y" | "yes" => {
            eprint!("\n");
            match create_hotkey_shortcut(using_alacritty) {
                Ok(()) => {
                    state.hotkey_configured = true;
                    state.launch_hotkey_tracking_version = super::LAUNCH_HOTKEY_TRACKING_VERSION;
                    let _ = state.save();
                    eprintln!(
                        "  \x1b[32m✓\x1b[0m Created hotkeys (\x1b[1mAlt+;\x1b[0m and \x1b[1mCopilot\x1b[0m) → {} + jcode",
                        terminal_name
                    );
                    eprintln!();
                    true
                }
                Err(e) => {
                    eprintln!("  \x1b[31m✗\x1b[0m Failed to create hotkey: {}", e);
                    eprintln!(
                        "    You can set it up manually later with: \x1b[1mjcode setup-hotkey\x1b[0m"
                    );
                    eprintln!();
                    false
                }
            }
        }
        "d" | "dont" => {
            state.hotkey_dismissed = true;
            let _ = state.save();
            false
        }
        _ => false,
    }
}

fn nudge_alacritty(state: &mut SetupHintsState) -> bool {
    let terminal = detect_terminal();

    let current_terminal = match terminal {
        "windows-terminal" => "Windows Terminal",
        "wezterm" => "WezTerm",
        _ => "your current terminal",
    };

    eprintln!("\x1b[36m┌─────────────────────────────────────────────────────────────┐\x1b[0m");
    eprintln!(
        "\x1b[36m│\x1b[0m \x1b[1m💡 Alacritty: the fastest terminal for jcode\x1b[0m               \x1b[36m│\x1b[0m"
    );
    eprintln!(
        "\x1b[36m│\x1b[0m                                                             \x1b[36m│\x1b[0m"
    );
    eprintln!(
        "\x1b[36m│\x1b[0m    {:<55} \x1b[36m│\x1b[0m",
        format!("You're using {}.", current_terminal)
    );
    eprintln!(
        "\x1b[36m│\x1b[0m    Alacritty is GPU-accelerated with the lowest latency.    \x1b[36m│\x1b[0m"
    );
    eprintln!(
        "\x1b[36m│\x1b[0m                                                             \x1b[36m│\x1b[0m"
    );
    eprintln!(
        "\x1b[36m│\x1b[0m    \x1b[32m[y]\x1b[0m Install   \x1b[90m[n]\x1b[0m Not now   \x1b[90m[d]\x1b[0m Don't ask again       \x1b[36m│\x1b[0m"
    );
    eprintln!("\x1b[36m└─────────────────────────────────────────────────────────────┘\x1b[0m");
    eprint!("\x1b[36m  >\x1b[0m ");
    let _ = io::stderr().flush();

    let choice = read_choice();

    match choice.as_str() {
        "y" | "yes" => {
            eprint!("\n");
            if !is_winget_available() {
                eprintln!("  \x1b[33m⚠\x1b[0m  winget not found. Install Alacritty manually:");
                eprintln!("     https://alacritty.org/");
                eprintln!();
                eprintln!("     Or install winget first: https://aka.ms/getwinget");
                eprintln!();
                return false;
            }

            match install_alacritty() {
                Ok(()) => {
                    state.alacritty_configured = true;
                    let _ = state.save();
                    eprintln!("  \x1b[32m✓\x1b[0m Alacritty installed!");

                    if state.hotkey_configured {
                        eprintln!("  Updating hotkey to use Alacritty...");
                        match create_hotkey_shortcut(true) {
                            Ok(()) => {
                                eprintln!(
                                    "  \x1b[32m✓\x1b[0m Hotkeys updated: \x1b[1mAlt+;\x1b[0m and \x1b[1mCopilot\x1b[0m → Alacritty + jcode"
                                );
                            }
                            Err(e) => {
                                eprintln!("  \x1b[33m⚠\x1b[0m  Could not update hotkey: {}", e);
                            }
                        }
                    }
                    eprintln!();
                    true
                }
                Err(e) => {
                    eprintln!("  \x1b[31m✗\x1b[0m Failed to install Alacritty: {}", e);
                    eprintln!("    Install manually: https://alacritty.org/");
                    eprintln!();
                    false
                }
            }
        }
        "d" | "dont" => {
            state.alacritty_dismissed = true;
            let _ = state.save();
            false
        }
        _ => false,
    }
}

fn prompt_try_it_out(installed_alacritty: bool) {
    eprintln!("\x1b[32m┌─────────────────────────────────────────────────────────────┐\x1b[0m");
    eprintln!(
        "\x1b[32m│\x1b[0m \x1b[1m✨ All set! Try it out:\x1b[0m                                     \x1b[32m│\x1b[0m"
    );
    eprintln!(
        "\x1b[32m│\x1b[0m                                                             \x1b[32m│\x1b[0m"
    );
    eprintln!(
        "\x1b[32m│\x1b[0m    Press \x1b[1mAlt+;\x1b[0m or the \x1b[1mCopilot key\x1b[0m to launch jcode.       \x1b[32m│\x1b[0m"
    );
    eprintln!(
        "\x1b[32m│\x1b[0m    The listener is native Jcode, no AutoHotkey required.          \x1b[32m│\x1b[0m"
    );
    if installed_alacritty {
        eprintln!(
            "\x1b[32m│\x1b[0m    It will open in \x1b[1mAlacritty\x1b[0m for maximum performance.    \x1b[32m│\x1b[0m"
        );
    }
    eprintln!(
        "\x1b[32m│\x1b[0m                                                             \x1b[32m│\x1b[0m"
    );
    eprintln!(
        "\x1b[32m│\x1b[0m    \x1b[90m(Starting jcode normally in 3 seconds...)\x1b[0m                 \x1b[32m│\x1b[0m"
    );
    eprintln!("\x1b[32m└─────────────────────────────────────────────────────────────┘\x1b[0m");
    eprintln!();

    std::thread::sleep(std::time::Duration::from_secs(3));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_windows_hotkeys_are_shared_defaults_plus_copilot_key() {
        let entries = default_windows_launch_entries();
        let shared = crate::launch_hotkeys::default_launch_entries();
        assert_eq!(entries.len(), shared.len() + 1);
        assert_eq!(&entries[..shared.len()], shared.as_slice());
        let copilot = entries.last().unwrap();
        assert_eq!(copilot.chord, "win+shift+f23");
        assert_eq!(copilot.dir, "$HOME");
        assert_eq!(copilot.label, "home");
    }

    #[test]
    fn startup_lifecycle_paths_are_stable_for_upgrade_and_uninstall_cleanup() {
        assert_eq!(
            startup_shortcut_path()
                .file_name()
                .unwrap()
                .to_string_lossy(),
            "jcode-hotkey.lnk"
        );
        assert_eq!(
            hotkey_vbs_path()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy(),
            "jcode-hotkey-launcher.vbs"
        );
        assert_eq!(
            legacy_hotkey_ps1_path()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy(),
            "jcode-hotkey.ps1"
        );
    }

    #[test]
    fn powershell_single_quote_escapes_embedded_quotes() {
        assert_eq!(ps_single_quote(r"C:\O'Hara\jcode"), r"'C:\O''Hara\jcode'");
    }

    #[test]
    fn listener_stop_sweep_excludes_jcode_and_powershell_processes_running_it() {
        let script = render_stop_windows_hotkey_listeners_script(4242);
        assert!(script.contains("($_.ProcessId -ne $current)"));
        assert!(script.contains("($_.ProcessId -ne $PID)"));
    }

    #[test]
    fn startup_shortcut_uses_native_listener_without_vbscript_or_bypass() {
        let script = render_startup_shortcut_script(
            Path::new(r"C:\Users\O'Hara\Startup\jcode-hotkey.lnk"),
            Path::new(r"C:\Program Files\Jcode O'Hara\jcode.exe"),
        );
        assert!(script.contains("$shortcut.TargetPath = 'powershell.exe'"));
        assert!(script.contains("-ExecutionPolicy RemoteSigned"));
        assert!(script.contains("setup-hotkey --listen-windows-hotkey"));
        assert!(script.contains("$shortcut.WindowStyle = 7\n$shortcut.Save()"));
        assert!(!script.contains("ExecutionPolicy Bypass"));
        assert!(!script.contains("wscript.exe"));
        assert!(!script.contains(".vbs"));
    }
}

pub(super) fn maybe_show_windows_setup_hints(
    state: &mut SetupHintsState,
    startup_hints: Option<StartupHints>,
) -> Option<StartupHints> {
    if state.launch_count % 3 != 0 {
        return startup_hints;
    }

    let terminal = detect_terminal();
    let already_using_alacritty = terminal == "alacritty";

    if already_using_alacritty {
        state.alacritty_configured = true;
        state.alacritty_dismissed = true;
        let _ = state.save();
    }

    let wants_hotkey_nudge = !state.hotkey_configured && !state.hotkey_dismissed;
    let wants_alacritty_nudge =
        !state.alacritty_configured && !state.alacritty_dismissed && !already_using_alacritty;

    // Stop pestering the user once we have shown the nudge prompt enough times,
    // even if they never explicitly chose "Don't ask again".
    if (wants_hotkey_nudge || wants_alacritty_nudge) && !state.nudge_budget_remaining() {
        return startup_hints;
    }

    let mut did_setup_hotkey = false;
    let mut did_install_alacritty = false;

    if wants_hotkey_nudge {
        state.record_nudge_shown();
        did_setup_hotkey = nudge_hotkey(state);
    }

    if wants_alacritty_nudge {
        state.record_nudge_shown();
        did_install_alacritty = nudge_alacritty(state);
    }

    if did_setup_hotkey || (did_install_alacritty && state.hotkey_configured) {
        prompt_try_it_out(did_install_alacritty);
    }

    startup_hints
}

pub(super) fn run_setup_hotkey_windows() -> Result<()> {
    let mut state = SetupHintsState::load();
    let terminal = detect_terminal();
    let already_using_alacritty = terminal == "alacritty";

    eprintln!("\x1b[1mjcode setup-hotkey\x1b[0m");
    eprintln!();

    eprintln!(
        "  Detected terminal: {}",
        match terminal {
            "windows-terminal" => "Windows Terminal",
            "wezterm" => "WezTerm",
            "alacritty" => "Alacritty",
            _ => "Unknown",
        }
    );

    if is_alacritty_installed() && !already_using_alacritty {
        eprintln!("  Alacritty: \x1b[32minstalled\x1b[0m");
    } else if already_using_alacritty {
        eprintln!("  Alacritty: \x1b[32mactive\x1b[0m");
    } else {
        eprintln!("  Alacritty: \x1b[90mnot installed\x1b[0m");
    }
    eprintln!();

    let mut installed_alacritty = false;
    if !already_using_alacritty && !is_alacritty_installed() {
        eprintln!(
            "  Alacritty is the fastest terminal emulator (GPU-accelerated, lowest latency)."
        );
        eprint!("  Install Alacritty? \x1b[32m[y]\x1b[0m/\x1b[90m[n]\x1b[0m: ");
        let _ = io::stderr().flush();
        let choice = read_choice();
        if choice == "y" || choice == "yes" {
            if !is_winget_available() {
                eprintln!("\n  \x1b[33m⚠\x1b[0m  winget not found. Install Alacritty manually:");
                eprintln!("     https://alacritty.org/\n");
            } else {
                match install_alacritty() {
                    Ok(()) => {
                        state.alacritty_configured = true;
                        installed_alacritty = true;
                        eprintln!("  \x1b[32m✓\x1b[0m Alacritty installed!\n");
                    }
                    Err(e) => {
                        eprintln!("  \x1b[31m✗\x1b[0m Install failed: {}\n", e);
                    }
                }
            }
        }
        eprintln!();
    }

    let use_alacritty = already_using_alacritty || is_alacritty_installed();
    let terminal_name = if use_alacritty {
        "Alacritty"
    } else {
        "Windows Terminal"
    };

    eprintln!(
        "  Setting up global launch hotkeys → {} + jcode...",
        terminal_name
    );

    match create_hotkey_shortcut(use_alacritty) {
        Ok(()) => {
            state.hotkey_configured = true;
            state.launch_hotkey_tracking_version = super::LAUNCH_HOTKEY_TRACKING_VERSION;
            let _ = state.save();
            eprintln!("  \x1b[32m✓\x1b[0m Created launch hotkeys");
            eprintln!();
            eprintln!("  Press these anywhere, system-wide:");
            for hk in resolve_windows_hotkeys() {
                if windows_hotkeys::hotkey_to_win32(&hk).is_some() {
                    let suffix = if hk.self_dev { " [self-dev]" } else { "" };
                    eprintln!(
                        "    \x1b[1m{}\x1b[0m → {}{}",
                        windows_hotkeys::display_windows_hotkey(&hk),
                        hk.label,
                        suffix
                    );
                }
            }
            eprintln!();
            super::install_cli_launch_hints_notice();
            prompt_try_it_out(installed_alacritty);
        }
        Err(e) => {
            eprintln!("  \x1b[31m✗\x1b[0m Failed: {}", e);
        }
    }

    Ok(())
}

pub(super) fn create_windows_desktop_shortcut(state: &mut SetupHintsState) -> Result<()> {
    let exe = std::env::current_exe()?;
    let exe_path = exe.to_string_lossy();

    let (target, args) = if is_alacritty_installed() {
        let alacritty = find_alacritty_path().unwrap_or_else(|| "alacritty".to_string());
        (alacritty, format!("-e \"{}\"", exe_path))
    } else {
        (exe_path.to_string(), String::new())
    };

    let desktop_dir = std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\Users\\Default".into());
    let shortcut_path = format!("{}\\Desktop\\jcode.lnk", desktop_dir);

    let ps_script = format!(
        r#"
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut("{shortcut_path}")
$shortcut.TargetPath = "{target}"
$shortcut.Arguments = '{args}'
$shortcut.Description = "jcode - AI coding agent"
$shortcut.Save()
Write-Output "OK"
"#,
        shortcut_path = shortcut_path,
        target = target,
        args = args,
    );

    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &ps_script])
        .output()?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.contains("OK") {
            state.desktop_shortcut_created = true;
            let _ = state.save();
            jcode_logging::info(&format!("Created desktop shortcut: {}", shortcut_path));
        }
    }

    Ok(())
}
