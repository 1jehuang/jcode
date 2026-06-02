//! Platform setup hints shown on startup.
//!
//! - Windows: suggest Alt+; hotkey setup and Alacritty install.
//! - macOS: detect suboptimal terminal and offer guided Ghostty setup via jcode.
//! - Linux: create a .desktop launcher file.
//!
//! Each nudge can be dismissed permanently with "Don't ask again".
//! State is persisted in `~/.jcode/setup_hints.json`.

use crate::storage;
#[cfg(target_os = "macos")]
use anyhow::Context;
use anyhow::Result;
use serde::{Deserialize, Serialize};
#[cfg(any(windows, target_os = "macos"))]
use std::io::Write;
use std::io::{self, IsTerminal};
use std::path::PathBuf;

#[cfg(any(test, target_os = "macos"))]
mod macos_launcher;
#[cfg(any(test, target_os = "macos"))]
mod macos_terminal;
#[cfg(windows)]
mod windows_setup;
#[cfg(any(test, target_os = "macos"))]
use macos_launcher::{install_macos_app_launcher, should_refresh_macos_app_launcher};
#[cfg(target_os = "macos")]
use macos_terminal::launch_script_for_macos_terminal;
#[cfg(target_os = "macos")]
use macos_terminal::load_preferred_macos_terminal;
#[cfg(any(test, target_os = "macos"))]
use macos_terminal::{
    MacTerminalKind, effective_macos_terminal, escape_applescript_text, escape_shell_single_quotes,
    launch_command_for_macos_terminal, paused_jcode_shell_command, save_preferred_macos_terminal,
};
#[cfg(windows)]
use windows_setup::{
    create_windows_desktop_shortcut, maybe_show_windows_setup_hints, run_setup_hotkey_windows,
};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SetupHintsState {
    pub launch_count: u64,
    pub hotkey_configured: bool,
    pub hotkey_dismissed: bool,
    #[serde(alias = "wezterm_configured")]
    pub alacritty_configured: bool,
    #[serde(alias = "wezterm_dismissed")]
    pub alacritty_dismissed: bool,
    #[serde(default)]
    pub desktop_shortcut_created: bool,
    #[serde(default)]
    pub startup_spawn_hint_dismissed: bool,
    pub mac_ghostty_guided: bool,
    pub mac_ghostty_dismissed: bool,
    /// Version of the installed macOS Cmd+; hotkey listener. Bumped when the
    /// listener implementation changes in a way that requires reinstalling the
    /// LaunchAgent for already-configured users (e.g. the run-loop fix that made
    /// the hotkey actually fire). `0` = legacy/unknown.
    #[serde(default)]
    pub hotkey_listener_version: u32,
}

/// Current macOS hotkey listener implementation version.
///
/// Increment this whenever the listener needs to be reinstalled for existing
/// users on update. History:
/// - 1: pump the Core Foundation run loop on the main thread so Cmd+; fires
///   (previously the listener blocked and never delivered events).
/// - 2: promote the launchd process to a UIElement app (`TransformProcessType`)
///   and run the Carbon application event loop, so a faceless background
///   process is actually eligible to receive `RegisterEventHotKey` events.
///   Version 1 still never fired because the process had no window-server
///   connection.
#[cfg(any(test, target_os = "macos"))]
pub const HOTKEY_LISTENER_VERSION: u32 = 2;

#[derive(Debug, Clone, Default)]
pub struct StartupHints {
    pub auto_send_message: Option<String>,
    pub status_notice: Option<String>,
    pub display_message: Option<(String, String)>,
}

impl StartupHints {
    fn with_spawn_notice(message: String) -> Self {
        Self {
            auto_send_message: None,
            status_notice: Some(message.clone()),
            display_message: Some(("Launch".to_string(), message)),
        }
    }

    fn with_status_and_display(
        status_notice: String,
        title: impl Into<String>,
        display_message: String,
    ) -> Self {
        Self {
            auto_send_message: None,
            status_notice: Some(status_notice),
            display_message: Some((title.into(), display_message)),
        }
    }
}

impl SetupHintsState {
    fn path() -> Result<PathBuf> {
        Ok(storage::jcode_dir()?.join("setup_hints.json"))
    }

    pub fn load() -> Self {
        Self::path()
            .ok()
            .and_then(|p| storage::read_json(&p).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        storage::write_json(&path, self)
    }
}

#[cfg(target_os = "macos")]
fn is_ghostty_installed() -> bool {
    if std::path::Path::new("/Applications/Ghostty.app").exists() {
        return true;
    }

    if let Some(home) = dirs::home_dir() {
        if home.join("Applications/Ghostty.app").exists() {
            return true;
        }
    }

    std::process::Command::new("which")
        .arg("ghostty")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn mac_hotkey_support_dir() -> Result<PathBuf> {
    Ok(storage::jcode_dir()?.join("hotkey"))
}

#[cfg(target_os = "macos")]
fn mac_hotkey_launch_agent_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home
        .join("Library")
        .join("LaunchAgents")
        .join("com.jcode.hotkey.plist"))
}

#[cfg(any(test, target_os = "macos"))]
fn mac_hotkey_launch_agent_plist(
    exe: &str,
    stdout_path: &str,
    stderr_path: &str,
    terminal: &str,
) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.jcode.hotkey</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>setup-hotkey</string>
        <string>--listen-macos-hotkey</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>LimitLoadToSessionType</key>
    <string>Aqua</string>
    <key>StandardOutPath</key>
    <string>{stdout_path}</string>
    <key>StandardErrorPath</key>
    <string>{stderr_path}</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>JCODE_PREFERRED_TERMINAL</key>
        <string>{terminal}</string>
    </dict>
</dict>
</plist>
"#,
    )
}

#[cfg(target_os = "macos")]
fn install_macos_hotkey_listener(
    preferred_terminal: Option<MacTerminalKind>,
) -> Result<MacTerminalKind> {
    let terminal = preferred_terminal.unwrap_or_else(effective_macos_terminal);
    let hotkey_dir = mac_hotkey_support_dir()?;
    std::fs::create_dir_all(&hotkey_dir)?;

    let exe = std::env::current_exe()?;
    let exe_path = exe.to_string_lossy().into_owned();
    let shell_command = paused_jcode_shell_command(&exe_path);

    let launch_script_path = hotkey_dir.join("launch_jcode.sh");
    std::fs::write(
        &launch_script_path,
        launch_script_for_macos_terminal(terminal, &shell_command),
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&launch_script_path, std::fs::Permissions::from_mode(0o755))?;
    }

    let plist_path = mac_hotkey_launch_agent_path()?;
    if let Some(parent) = plist_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let stdout_path = hotkey_dir.join("mac_hotkey.out.log");
    let stderr_path = hotkey_dir.join("mac_hotkey.err.log");
    let plist = mac_hotkey_launch_agent_plist(
        &exe_path,
        &stdout_path.to_string_lossy(),
        &stderr_path.to_string_lossy(),
        terminal.cli_value(),
    );
    std::fs::write(&plist_path, plist)?;

    save_preferred_macos_terminal(terminal)?;

    let _ = std::process::Command::new("launchctl")
        .args(["unload", plist_path.to_string_lossy().as_ref()])
        .status();
    let status = std::process::Command::new("launchctl")
        .args(["load", "-w", plist_path.to_string_lossy().as_ref()])
        .status()
        .context("failed to load jcode LaunchAgent")?;
    if !status.success() {
        anyhow::bail!("launchctl load failed with exit code {:?}", status.code());
    }

    Ok(terminal)
}

fn startup_hints_for_launch(state: &SetupHintsState) -> Option<StartupHints> {
    #[cfg(any(test, target_os = "macos"))]
    let spawn_notice = if !state.hotkey_configured || state.startup_spawn_hint_dismissed {
        None
    } else {
        Some(format!(
            "Cmd+; launches a new jcode from anywhere, system-wide (opens in {}).",
            effective_macos_terminal().label()
        ))
    };
    #[cfg(not(any(test, target_os = "macos")))]
    let spawn_notice: Option<String> = None;

    if state.launch_count == 1 {
        let mut message = "Tip: jcode is left-aligned by default. Use `/alignment centered` or press `Alt+C` to toggle left/centered for the current session.".to_string();

        if let Some(spawn_notice) = spawn_notice {
            message.push_str("\n\n");
            message.push_str(&spawn_notice);
        }

        return Some(StartupHints::with_status_and_display(
            "Tip: `/alignment centered` or Alt+C toggles alignment.".to_string(),
            "Alignment",
            message,
        ));
    }

    if state.launch_count <= 3 {
        let config_path = crate::config::Config::path()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "~/.jcode/config.toml".to_string());

        let mut message = format!(
            "You can hotswap text alignment with `Alt+C` (left-aligned ↔ centered).\n\nTo save it permanently, use `/alignment centered` or `/alignment left`. You can also change it in `{}` with `display.centered = true` or `display.centered = false`.\n\nLeft-aligned mode is the default for new configs.",
            config_path
        );

        if let Some(spawn_notice) = spawn_notice {
            message.push_str("\n\n");
            message.push_str(&spawn_notice);
        }

        return Some(StartupHints::with_status_and_display(
            "Tip: Alt+C toggles left/center alignment.".to_string(),
            "Welcome",
            message,
        ));
    }

    spawn_notice.map(StartupHints::with_spawn_notice)
}

/// Read a single-character choice from the user.
#[cfg(any(windows, target_os = "macos"))]
fn read_choice() -> String {
    let mut input = String::new();
    let _ = io::stdin().read_line(&mut input);
    input.trim().to_lowercase()
}

#[cfg(target_os = "macos")]
fn macos_guided_ghostty_message(current_terminal: MacTerminalKind) -> String {
    format!(
        "I want to upgrade my macOS terminal setup for jcode. Please guide me step-by-step, wait for confirmation between steps, and keep each step concise.\n\nCurrent terminal: {}\nGoal: install Ghostty and use it for jcode.\n\nPlease help me with:\n1) Detecting if Homebrew is installed (and installing it if missing)\n2) Installing Ghostty\n3) Launching Ghostty and setting it as my preferred terminal for jcode\n4) Optional: adding a macOS keyboard shortcut/launcher flow for jcode\n5) Verifying jcode runs in Ghostty and that inline images/graphics work\n\nAssume I am not an expert; provide exact commands and where to click in macOS settings when needed.",
        current_terminal.label()
    )
}

#[cfg(target_os = "macos")]
fn nudge_macos_ghostty(state: &mut SetupHintsState) -> Option<String> {
    let terminal = effective_macos_terminal();
    let using_ghostty = terminal == MacTerminalKind::Ghostty;
    let ghostty_installed = is_ghostty_installed();

    if using_ghostty {
        state.mac_ghostty_guided = true;
        state.mac_ghostty_dismissed = true;
        let _ = state.save();
        return None;
    }

    eprintln!("\x1b[36m┌─────────────────────────────────────────────────────────────┐\x1b[0m");
    eprintln!(
        "\x1b[36m│\x1b[0m \x1b[1m💡 Better macOS terminal for jcode: Ghostty\x1b[0m                \x1b[36m│\x1b[0m"
    );
    eprintln!(
        "\x1b[36m│\x1b[0m                                                             \x1b[36m│\x1b[0m"
    );
    eprintln!(
        "\x1b[36m│\x1b[0m    Current terminal: {:<37} \x1b[36m│\x1b[0m",
        format!("{}.", terminal.label())
    );
    if ghostty_installed {
        eprintln!(
            "\x1b[36m│\x1b[0m    Ghostty is installed, but you are not using it now.      \x1b[36m│\x1b[0m"
        );
    } else {
        eprintln!(
            "\x1b[36m│\x1b[0m    Ghostty offers fast rendering and great jcode UX.         \x1b[36m│\x1b[0m"
        );
    }
    eprintln!(
        "\x1b[36m│\x1b[0m                                                             \x1b[36m│\x1b[0m"
    );
    eprintln!(
        "\x1b[36m│\x1b[0m    Let jcode guide you through setup right now?             \x1b[36m│\x1b[0m"
    );
    eprintln!(
        "\x1b[36m│\x1b[0m    \x1b[32m[y]\x1b[0m Yes      \x1b[90m[n]\x1b[0m Not now      \x1b[90m[d]\x1b[0m Don't ask again    \x1b[36m│\x1b[0m"
    );
    eprintln!("\x1b[36m└─────────────────────────────────────────────────────────────┘\x1b[0m");
    eprint!("\x1b[36m  >\x1b[0m ");
    let _ = io::stderr().flush();

    let choice = read_choice();

    match choice.as_str() {
        "y" | "yes" => {
            state.mac_ghostty_guided = true;
            let _ = state.save();
            Some(macos_guided_ghostty_message(terminal))
        }
        "d" | "dont" => {
            state.mac_ghostty_dismissed = true;
            let _ = state.save();
            None
        }
        _ => None,
    }
}

/// Manual `jcode setup-hotkey` command.
///
/// Runs the full interactive setup flow regardless of launch count.
pub fn run_setup_hotkey(_listen_macos_hotkey: bool) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        // The background listener (`--listen-macos-hotkey`) is intercepted earlier,
        // in `main()`, so it runs on the real main thread with a Core Foundation
        // run loop. If we somehow reach here with that flag (e.g. invoked directly),
        // honor it rather than running the interactive installer.
        if _listen_macos_hotkey {
            return run_macos_hotkey_listener();
        }

        let mut state = SetupHintsState::load();
        let terminal = effective_macos_terminal();
        eprintln!("\x1b[1mjcode setup-hotkey\x1b[0m");
        eprintln!();
        eprintln!("  Preferred terminal: {}", terminal.label());
        eprintln!(
            "  Installing a LaunchAgent so Cmd+; launches a new jcode from anywhere, system-wide."
        );
        eprintln!();

        match install_macos_hotkey_listener(Some(terminal)) {
            Ok(installed_terminal) => {
                state.hotkey_configured = true;
                state.hotkey_dismissed = true;
                state.hotkey_listener_version = HOTKEY_LISTENER_VERSION;
                let _ = state.save();
                eprintln!(
                    "  \x1b[32m✓\x1b[0m Created hotkey (\x1b[1mCmd+;\x1b[0m) → {} + jcode",
                    installed_terminal.label()
                );
                eprintln!();
                eprintln!(
                    "  Press \x1b[1mCmd+;\x1b[0m anywhere, system-wide, to launch a new jcode in {}.",
                    installed_terminal.label()
                );
                return Ok(());
            }
            Err(e) => {
                eprintln!("  \x1b[31m✗\x1b[0m Failed: {}", e);
                anyhow::bail!("macOS hotkey setup failed: {}", e);
            }
        }
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        eprintln!("Global hotkey setup is currently only supported on Windows.");
        eprintln!();
        eprintln!("On Linux/macOS, add a keybinding in your desktop environment:");
        eprintln!("  - niri: bindings in ~/.config/niri/config.kdl");
        eprintln!("  - GNOME: Settings > Keyboard > Custom Shortcuts");
        eprintln!("  - KDE: System Settings > Shortcuts > Custom Shortcuts");
        eprintln!("  - macOS: Shortcuts.app or System Settings > Keyboard > Shortcuts");
        Ok(())
    }

    #[cfg(windows)]
    {
        run_setup_hotkey_windows()
    }
}

/// Run the macOS global-hotkey listener on the current (main) thread.
///
/// This must be called from `main()` before any tokio runtime is created, so
/// that the Core Foundation run loop driving Carbon hotkey events lives on the
/// real main thread. On non-macOS platforms this is a no-op that returns `Ok`.
pub fn run_macos_hotkey_listener_main_thread() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        run_macos_hotkey_listener()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod macos_run_loop {
    // Minimal Carbon/ApplicationServices bindings to (a) make this faceless
    // launchd process eligible to receive global hotkeys and (b) run the Carbon
    // application event loop that dispatches `RegisterEventHotKey` events.
    //
    // We deliberately avoid pulling in a heavier `core-foundation`/`cocoa`
    // dependency just for these few calls.

    #[repr(C)]
    struct ProcessSerialNumber {
        high: u32,
        low: u32,
    }

    // `kCurrentProcess` from MacTypes / Process Manager.
    const K_CURRENT_PROCESS: u32 = 2;
    // `kProcessTransformToUIElementApplication` from ApplicationServices.
    // Promotes a background (faceless) process to a UIElement app so it has a
    // connection to the window server and can receive Carbon hotkey events,
    // without showing a Dock icon or menu bar.
    const K_PROCESS_TRANSFORM_TO_UI_ELEMENT_APPLICATION: u32 = 4;

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn TransformProcessType(psn: *const ProcessSerialNumber, transform_state: u32) -> i32;
    }

    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        fn RunApplicationEventLoop();
    }

    /// Promote this process to a UIElement application.
    ///
    /// A LaunchAgent started without an app bundle runs as a faceless background
    /// process with no window-server connection, so Carbon `RegisterEventHotKey`
    /// events are never delivered to it. Transforming the process type gives it
    /// the connection it needs while keeping it out of the Dock and menu bar.
    ///
    /// Returns the raw OSStatus (0 == `noErr`).
    pub fn promote_to_ui_element() -> i32 {
        let psn = ProcessSerialNumber {
            high: 0,
            low: K_CURRENT_PROCESS,
        };
        // SAFETY: `psn` points at a valid ProcessSerialNumber for the lifetime of
        // the call; the transform constant is a documented Process Manager value.
        unsafe { TransformProcessType(&psn, K_PROCESS_TRANSFORM_TO_UI_ELEMENT_APPLICATION) }
    }

    /// Block forever on the Carbon application event loop, dispatching hotkey
    /// (and other Carbon) events as they arrive.
    ///
    /// This must run on the real main thread that created the hotkey manager.
    /// `RunApplicationEventLoop` installs the standard application event handlers
    /// and pumps the main run loop; unlike a bare `CFRunLoopRun()` it guarantees
    /// the Carbon event target that `RegisterEventHotKey` dispatches through is
    /// actually serviced, and it does not return spuriously when no Core
    /// Foundation input source happens to be installed yet.
    pub fn run_forever() {
        // SAFETY: takes no arguments; runs the calling (main) thread's event loop.
        unsafe { RunApplicationEventLoop() };
    }
}

#[cfg(target_os = "macos")]
fn run_macos_hotkey_listener() -> Result<()> {
    use global_hotkey::hotkey::{Code, HotKey, Modifiers};
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
    use std::process::Command;

    // `global-hotkey` on macOS registers a Carbon hotkey (`RegisterEventHotKey`)
    // whose events are dispatched through the application's Carbon event target,
    // serviced by the **main thread's** event loop. Two things are required for a
    // LaunchAgent (started without an app bundle) to actually receive them:
    //
    //   1. The process must be promoted from a faceless background process to a
    //      UIElement application (`TransformProcessType`). Without a window-server
    //      connection, Carbon never delivers hotkey events at all. This was the
    //      reason Cmd+; stayed dead even after the run-loop fix.
    //   2. The main thread must run the Carbon application event loop
    //      (`RunApplicationEventLoop`), not a bare `CFRunLoopRun()`.
    //
    // This function is invoked directly from `main()` before the tokio runtime is
    // built, so it runs on the real main thread. We install an event handler that
    // launches jcode on key-down, then hand the thread to the event loop so the
    // handler is invoked whenever the hotkey fires. Using the event handler
    // (rather than polling the channel) avoids both busy-spinning and latency.

    // The listener runs as its own launchd process and never goes through the
    // normal startup path, so initialize logging here. Diagnostics land in the
    // standard jcode log plus the plist's StandardOut/ErrorPath.
    crate::logging::init();
    macos_hotkey_log("starting macOS Cmd+; hotkey listener");

    let status = macos_run_loop::promote_to_ui_element();
    if status != 0 {
        macos_hotkey_log(&format!(
            "warning: TransformProcessType returned status {status}; \
             Cmd+; may not be delivered to this process"
        ));
    }

    let launch_script = mac_hotkey_support_dir()?.join("launch_jcode.sh");
    let manager =
        GlobalHotKeyManager::new().context("failed to initialize global hotkey manager")?;
    let hotkey = HotKey::new(Some(Modifiers::META), Code::Semicolon);
    manager
        .register(hotkey)
        .context("failed to register Cmd+; hotkey")?;

    let hotkey_id = hotkey.id();
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.id == hotkey_id && event.state == HotKeyState::Pressed {
            macos_hotkey_log("Cmd+; pressed; launching new jcode");
            match Command::new("sh").arg(&launch_script).spawn() {
                Ok(_) => {}
                Err(err) => macos_hotkey_log(&format!("failed to launch jcode: {err}")),
            }
        }
    }));

    macos_hotkey_log("macOS Cmd+; hotkey listener registered; entering event loop");
    // Keep the manager alive for the lifetime of the event loop so the hotkey
    // registration and event handler stay installed.
    let _manager = manager;
    // Hand the main thread to the Carbon event loop so hotkey events are
    // delivered. This normally never returns for our long-lived listener.
    macos_run_loop::run_forever();
    macos_hotkey_log("macOS Cmd+; hotkey event loop exited");
    Ok(())
}

/// Log a hotkey-listener diagnostic to both the jcode log and stderr.
///
/// The LaunchAgent redirects stdout/stderr to log files in the hotkey support
/// dir, so emitting to stderr here makes the listener's lifecycle observable
/// even before/without the structured logger.
#[cfg(target_os = "macos")]
fn macos_hotkey_log(message: &str) {
    crate::logging::info(message);
    eprintln!("[jcode hotkey] {message}");
}

/// Decide what macOS hotkey listener action a launch should take, given the
/// persisted setup state. Extracted as a pure function so the upgrade/install
/// gating can be unit-tested without touching launchd.
#[cfg(any(test, target_os = "macos"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacHotkeyAction {
    /// First-time install (never configured, never dismissed).
    Install,
    /// Reinstall because the configured listener predates the current version.
    Migrate,
    /// Nothing to do.
    None,
}

#[cfg(any(test, target_os = "macos"))]
fn mac_hotkey_action_for_state(state: &SetupHintsState) -> MacHotkeyAction {
    if !state.hotkey_configured && !state.hotkey_dismissed {
        MacHotkeyAction::Install
    } else if state.hotkey_configured && state.hotkey_listener_version < HOTKEY_LISTENER_VERSION {
        MacHotkeyAction::Migrate
    } else {
        MacHotkeyAction::None
    }
}

/// Main entry point: check if we should show setup hints.
///
/// Called early in startup, before the TUI is initialized.
/// Returns optional structured startup hints for the TUI.
///
/// - Windows: On every 3rd launch, can show hotkey + Alacritty nudges.
/// - macOS: On every 3rd launch, can suggest Ghostty and optionally hand off
///   to AI-guided setup by returning a prebuilt prompt.
pub fn maybe_show_setup_hints() -> Option<StartupHints> {
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return None;
    }

    let mut state = SetupHintsState::load();
    state.launch_count += 1;
    let _ = state.save();

    #[cfg(any(test, target_os = "macos"))]
    {
        if should_refresh_macos_app_launcher(&state) {
            let _ = create_desktop_shortcut(&mut state);
        }
    }

    #[cfg(target_os = "macos")]
    {
        match mac_hotkey_action_for_state(&state) {
            MacHotkeyAction::Install => {
                if let Err(err) = auto_install_macos_hotkey_listener(&mut state) {
                    crate::logging::warn(&format!(
                        "failed to auto-install macOS Cmd+; hotkey listener: {err}"
                    ));
                }
            }
            MacHotkeyAction::Migrate => {
                // Already-configured user on an older listener: reinstall so the
                // updated listener (and current binary path) takes effect on
                // update without requiring them to re-run setup.
                if let Err(err) = migrate_macos_hotkey_listener(&mut state) {
                    crate::logging::warn(&format!(
                        "failed to migrate macOS Cmd+; hotkey listener: {err}"
                    ));
                }
            }
            MacHotkeyAction::None => {}
        }
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        if !state.desktop_shortcut_created {
            let _ = create_desktop_shortcut(&mut state);
        }
    }

    // On Windows, desktop shortcut creation shells out to PowerShell/COM and can
    // take tens of seconds or hang in some Windows Terminal/WSL launch contexts.
    // Do not run it on the critical startup path. Users can still run
    // `jcode setup-launcher` explicitly.

    let startup_hints = startup_hints_for_launch(&state);

    #[cfg(target_os = "macos")]
    {
        if state.launch_count % 3 != 0 {
            return startup_hints;
        }

        if !state.mac_ghostty_guided && !state.mac_ghostty_dismissed {
            let mut hints = startup_hints.unwrap_or_default();
            hints.auto_send_message = nudge_macos_ghostty(&mut state);
            return if hints.auto_send_message.is_none()
                && hints.status_notice.is_none()
                && hints.display_message.is_none()
            {
                None
            } else {
                Some(hints)
            };
        }

        return startup_hints;
    }

    #[cfg(windows)]
    {
        return maybe_show_windows_setup_hints(&mut state, startup_hints);
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        startup_hints
    }
}

/// Manual `jcode setup-launcher` command.
pub fn run_setup_launcher() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let mut state = SetupHintsState::load();
        eprintln!("\x1b[1mjcode setup-launcher\x1b[0m");
        eprintln!();

        match install_macos_app_launcher() {
            Ok((app_dir, terminal)) => {
                state.desktop_shortcut_created = true;
                let _ = state.save();
                eprintln!(
                    "  \x1b[32m✓\x1b[0m Installed launcher: {}",
                    app_dir.display()
                );
                eprintln!(
                    "  \x1b[32m✓\x1b[0m Spotlight/Launchpad/Dock will launch jcode in {}",
                    terminal.label()
                );
                eprintln!();
                eprintln!("  Tip: pin Jcode.app to your Dock or launch it with Cmd+Space.");
                return Ok(());
            }
            Err(e) => {
                eprintln!("  \x1b[31m✗\x1b[0m Failed: {}", e);
                anyhow::bail!("macOS launcher setup failed: {}", e);
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        // `setup-launcher` is a macOS-only command today (it installs a
        // `jcode.app` bundle so Spotlight/Launchpad/Dock can launch jcode).
        // Bail with a non-zero exit so scripted wrappers (installers, CI,
        // shell aliases) can detect the no-op instead of treating the
        // diagnostic eprintln as a successful install.
        anyhow::bail!(
            "`jcode setup-launcher` is currently only supported on macOS. On Windows use `Start Menu > jcode` after running scripts/install.ps1; on Linux create a .desktop file under ~/.local/share/applications/."
        );
    }
}

/// Create a desktop shortcut/launcher for jcode.
///
/// - Windows: creates a .lnk shortcut on the Desktop
/// - macOS: creates a jcode.app bundle in ~/Applications/
fn create_desktop_shortcut(state: &mut SetupHintsState) -> Result<()> {
    #[cfg(windows)]
    {
        create_windows_desktop_shortcut(state)?;
    }

    #[cfg(any(test, target_os = "macos"))]
    {
        let (app_dir, _terminal) = install_macos_app_launcher()?;

        state.desktop_shortcut_created = true;
        let _ = state.save();

        crate::logging::info(&format!("Created macOS app bundle: {}", app_dir.display()));
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        state.desktop_shortcut_created = true;
        let _ = state.save();
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn auto_install_macos_hotkey_listener(state: &mut SetupHintsState) -> Result<()> {
    let terminal = install_macos_hotkey_listener(None)?;
    state.hotkey_configured = true;
    state.hotkey_dismissed = true;
    state.hotkey_listener_version = HOTKEY_LISTENER_VERSION;
    state.save()?;
    crate::logging::info(&format!(
        "Installed macOS Cmd+; hotkey listener for {}",
        terminal.label()
    ));
    Ok(())
}

/// Reinstall the macOS hotkey LaunchAgent for an already-configured user after
/// an update that changed the listener implementation.
///
/// The LaunchAgent pins the binary path captured at setup time and the listener
/// process keeps running the old code until reloaded. Reinstalling re-points it
/// at the current binary and restarts it so the fixed listener takes effect
/// without the user re-running setup. The user's previously chosen terminal is
/// preserved.
#[cfg(target_os = "macos")]
fn migrate_macos_hotkey_listener(state: &mut SetupHintsState) -> Result<()> {
    let preferred = load_preferred_macos_terminal();
    let terminal = install_macos_hotkey_listener(preferred)?;
    state.hotkey_listener_version = HOTKEY_LISTENER_VERSION;
    state.save()?;
    crate::logging::info(&format!(
        "Migrated macOS Cmd+; hotkey listener to v{} for {}",
        HOTKEY_LISTENER_VERSION,
        terminal.label()
    ));
    Ok(())
}

#[cfg(test)]
#[path = "setup_hints_tests.rs"]
mod setup_hints_tests;
