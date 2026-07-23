//! Platform setup hints shown on startup.
//!
//! - Windows: suggest native Alt launch hotkeys plus Copilot-key setup and terminal setup.
//! - macOS: if the user is on the default built-in Terminal.app, show a one-time
//!   notice that it renders jcode poorly and suggest a modern terminal (Ghostty).
//! - Linux: create a .desktop launcher file.
//!
//! Each nudge can be dismissed permanently with "Don't ask again".
//! State is persisted in `~/.jcode/setup_hints.json`.

// Several launch-hotkey helpers are gated `#[cfg(any(test, target_os = "macos"))]`
// because the unit tests exercise the macOS launch-hotkey notice logic on every
// platform. In a non-macOS *test* build their only production callers (the
// `#[cfg(target_os = "macos")]` notice/install paths) are compiled out, so the
// helpers the tests don't call directly look dead. They are real macOS code, so
// silence dead_code only for that specific build shape instead of deleting them.
#![cfg_attr(all(test, not(target_os = "macos")), allow(dead_code))]

#[cfg(any(target_os = "macos", target_os = "linux"))]
use anyhow::Context;
use anyhow::Result;
use jcode_storage as storage;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{self, IsTerminal};
use std::path::PathBuf;

pub mod keymap;

mod cli_launch_hints;

#[cfg(any(test, target_os = "macos", target_os = "linux", windows))]
mod launch_hotkeys;
#[cfg(any(test, target_os = "linux"))]
mod linux_env;
#[cfg(any(test, target_os = "linux"))]
mod linux_niri;
#[cfg(any(test, target_os = "macos"))]
mod macos_launcher;
#[cfg(any(test, target_os = "macos"))]
mod macos_terminal;
#[cfg(any(test, windows))]
mod windows_hotkeys;
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
    run_windows_hotkey_listener, uninstall_windows_hotkey_listener,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
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
    #[serde(default = "default_true")]
    pub startup_spawn_hint_dismissed: bool,
    pub mac_ghostty_guided: bool,
    pub mac_ghostty_dismissed: bool,
    /// Number of times we have shown the terminal/setup nudge prompt to the user
    /// (across all platforms). Used to cap the total number of nudges so we never
    /// pester someone forever if they keep choosing "Not now".
    #[serde(default)]
    pub terminal_nudge_count: u64,
    /// Version of the installed macOS Cmd+; hotkey listener. Bumped when the
    /// listener implementation changes in a way that requires reinstalling the
    /// LaunchAgent for already-configured users (e.g. the run-loop fix that made
    /// the hotkey actually fire). `0` = legacy/unknown.
    #[serde(default)]
    pub hotkey_listener_version: u32,
    /// Version of the cross-platform launch command metadata. Bumped when
    /// generated macOS/Linux/Windows launchers need refreshing so successful
    /// shortcut use continues feeding the learned-keybinding state.
    #[serde(default)]
    pub launch_hotkey_tracking_version: u32,
    /// Canonical signature of the keybinding conflicts we last warned the user
    /// about (sorted, joined chord+field pairs). Empty means "no conflicts known
    /// / never warned". We only re-show the startup conflict notice when this
    /// signature changes, so users are warned once per distinct conflict set and
    /// never nagged about the same conflicts on every launch.
    #[serde(default)]
    pub keymap_conflict_signature: String,
    /// Whether we've shown the one-time "glyph-safe mode is active" disclosure
    /// for fragile-glyph terminals (macOS VS Code integrated terminal / Apple
    /// Terminal). We surface the tradeoff once per install so the user knows
    /// colors are quantized to 256 to avoid the terminal's glyph corruption.
    #[serde(default)]
    pub glyph_safe_notice_shown: bool,
    /// Counts successful launches by canonical launch-hotkey chord. Used to stop
    /// showing already-learned repo hotkey hints.
    #[serde(default)]
    pub launch_hotkey_usage: HashMap<String, u64>,
    /// Last time a launch-shortcut reminder was shown for each external CLI.
    /// Keys are stable source ids such as `claude` and `codex`; values are Unix
    /// timestamps in seconds.
    #[serde(default)]
    pub cli_launch_hint_last_shown: HashMap<String, u64>,
    /// Lifetime reminder count per external CLI. The native SessionStart hooks
    /// may fire on every launch, but the reminder intentionally stops after a
    /// small number of spaced repetitions.
    #[serde(default)]
    pub cli_launch_hint_shown_count: HashMap<String, u64>,
}

/// Serde default helper: fields documented as "true by default".
fn default_true() -> bool {
    true
}

impl Default for SetupHintsState {
    fn default() -> Self {
        Self {
            launch_count: 0,
            hotkey_configured: false,
            hotkey_dismissed: false,
            alacritty_configured: false,
            alacritty_dismissed: false,
            desktop_shortcut_created: false,
            // Dismissed by default: the system-wide launch-hotkey spawn notice is
            // opt-in noise, so new state starts with it suppressed.
            startup_spawn_hint_dismissed: true,
            mac_ghostty_guided: false,
            mac_ghostty_dismissed: false,
            terminal_nudge_count: 0,
            hotkey_listener_version: 0,
            launch_hotkey_tracking_version: 0,
            keymap_conflict_signature: String::new(),
            glyph_safe_notice_shown: false,
            launch_hotkey_usage: HashMap::new(),
            cli_launch_hint_last_shown: HashMap::new(),
            cli_launch_hint_shown_count: HashMap::new(),
        }
    }
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
/// - 3: register three launch hotkeys instead of one. `Cmd+;` opens jcode in
///   `$HOME`, `Cmd+'` opens it in the last project directory, and `Cmd+Shift+'`
///   opens a self-dev session in the last jcode repo. Existing users are
///   migrated so the extra scripts/registrations are installed on update.
/// - 4: hotkeys are config-driven. The installer resolves `[launch_hotkeys]`
///   from config (empty -> the same three built-ins) into per-entry scripts and
///   a `plan.json`; the listener registers chords from that plan. Existing users
///   migrate so the plan file and per-entry scripts are written, enabling the
///   baked per-repo hotkeys auto-import can add.
/// - 5: the listener launches configured repos directly through
///   `jcode-terminal-launch`, avoiding the generated shell-script hop on hotkey
///   press. Scripts/plan are still written for compatibility and diagnostics.
/// - 6: direct launches pass `--spawn-hotkey` into the new Jcode process so
///   global shortcut proficiency is recorded by the same cross-platform path.
#[cfg(any(test, target_os = "macos"))]
pub const HOTKEY_LISTENER_VERSION: u32 = 6;

/// Current version of generated launch commands carrying learning metadata.
const LAUNCH_HOTKEY_TRACKING_VERSION: u32 = 1;

/// Maximum number of times we will ever show the terminal/setup nudge prompt
/// to a user (across all launches and platforms). After this many nudges we stop
/// asking, even if the user never explicitly picked "Don't ask again".
pub const MAX_TERMINAL_NUDGES: u64 = 5;
const LAUNCH_HOTKEY_LEARNED_USES: u64 = 3;
#[cfg(any(test, target_os = "macos", target_os = "linux", windows))]
const LAUNCH_HOTKEY_NOTICE_MIN_LAUNCHES_TO_STOP: u64 = 10;

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
        let Ok(path) = Self::path() else {
            return Self::default();
        };
        Self::load_from(&path)
    }

    /// Load state from `path`, falling back to its `.bak` sibling.
    ///
    /// The atomic writer keeps the previous version at `.bak`. If the primary
    /// file is missing or unreadable (deleted, interrupted swap), fall back to
    /// it instead of silently resetting state like `launch_count`, which
    /// downstream heuristics (e.g. first-run onboarding) rely on.
    fn load_from(path: &std::path::Path) -> Self {
        if let Ok(state) = storage::read_json(path) {
            return state;
        }
        let bak = path.with_extension("bak");
        storage::read_json(&bak).unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        // Best-effort UI state (launch counter + one-time hint/nudge flags).
        // This is written on every interactive launch and is not durability
        // critical: losing the most recent update on a power cut just re-shows a
        // hint or under-counts a launch. Use the non-fsync fast write so we do
        // not pay macOS's `F_FULLFSYNC` (full disk-platter flush, ~8ms here)
        // twice on the startup critical path. The atomic rename still protects
        // against torn/partial writes, and load() falls back to `.bak`.
        storage::write_json_fast(&path, self)
    }

    /// Whether we are still allowed to show a terminal/setup nudge. Once we have
    /// shown the prompt `MAX_TERMINAL_NUDGES` times we stop asking entirely.
    #[cfg(any(test, windows, target_os = "macos"))]
    fn nudge_budget_remaining(&self) -> bool {
        self.terminal_nudge_count < MAX_TERMINAL_NUDGES
    }

    /// Record that a nudge prompt was shown to the user and persist the count.
    /// Only invoked on Windows/macOS nudge paths; under `cfg(test)` on other
    /// platforms it compiles but has no caller.
    #[cfg(any(test, windows, target_os = "macos"))]
    #[cfg_attr(
        not(any(windows, target_os = "macos")),
        allow(dead_code, reason = "only called on Windows/macOS nudge paths")
    )]
    fn record_nudge_shown(&mut self) {
        self.terminal_nudge_count = self.terminal_nudge_count.saturating_add(1);
        let _ = self.save();
    }
}

#[cfg(any(test, target_os = "macos", target_os = "linux", windows))]
fn mac_hotkey_support_dir() -> Result<PathBuf> {
    Ok(storage::jcode_dir()?.join("hotkey"))
}

/// File holding the last project directory jcode was launched from. The `Cmd+'`
/// global hotkey reads this at fire time to reopen jcode there.
#[cfg(any(test, target_os = "macos", target_os = "linux", windows))]
fn mac_hotkey_last_dir_file() -> Result<PathBuf> {
    Ok(mac_hotkey_support_dir()?.join("last_dir"))
}

/// File holding the last jcode *repository* directory the user worked in. The
/// `Cmd+Shift+'` global hotkey reads this to open a self-dev session there.
#[cfg(any(test, target_os = "macos", target_os = "linux", windows))]
fn mac_hotkey_last_repo_file() -> Result<PathBuf> {
    Ok(mac_hotkey_support_dir()?.join("last_repo"))
}

/// JSON file mapping each registered chord to the launch script the listener
/// should run. Written by the installer from the resolved config, read by the
/// launchd listener so it never re-parses config at fire time.
#[cfg(any(test, target_os = "macos"))]
fn mac_hotkey_plan_file() -> Result<PathBuf> {
    Ok(mac_hotkey_support_dir()?.join("plan.json"))
}

/// Load the `[launch_hotkeys]` table from `~/.jcode/config.toml`.
///
/// Returns the default (empty -> built-in 3 hotkeys) when the file is missing or
/// the section is absent. Best-effort: a malformed config falls back to default
/// rather than blocking hotkey install.
fn load_launch_hotkeys_config() -> jcode_config_types::LaunchHotkeysConfig {
    #[derive(serde::Deserialize, Default)]
    struct Wrapper {
        #[serde(default)]
        launch_hotkeys: jcode_config_types::LaunchHotkeysConfig,
    }
    let Ok(dir) = storage::jcode_dir() else {
        return Default::default();
    };
    let path = dir.join("config.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Default::default();
    };
    toml::from_str::<Wrapper>(&text)
        .map(|w| w.launch_hotkeys)
        .unwrap_or_default()
}

/// Record the directories the global launch hotkeys should reopen.
///
/// Called once per interactive launch with the process's working directory.
/// `$HOME` launches are ignored for the "last project" file so the `Cmd+'`
/// hotkey keeps pointing at a real project rather than home (which already has
/// its own `Cmd+;` hotkey). When `dir` is inside a jcode repo, the repo root is
/// recorded for the self-dev hotkey.
///
/// Best-effort and side-effect-only: failures are logged, never propagated, so
/// this can be dropped onto the startup path without risk.
pub fn record_launch_dirs(dir: &std::path::Path, repo_dir: Option<&std::path::Path>) {
    #[cfg(any(target_os = "macos", target_os = "linux", windows))]
    {
        if let Err(err) = record_launch_dirs_inner(dir, repo_dir) {
            jcode_logging::warn(&format!("failed to record launch dirs for hotkeys: {err}"));
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        let _ = (dir, repo_dir);
    }
}

#[cfg(any(target_os = "macos", target_os = "linux", windows))]
fn record_launch_dirs_inner(
    dir: &std::path::Path,
    repo_dir: Option<&std::path::Path>,
) -> Result<()> {
    let support_dir = mac_hotkey_support_dir()?;
    std::fs::create_dir_all(&support_dir)?;

    if should_record_last_dir(dir, dirs::home_dir().as_deref()) {
        std::fs::write(mac_hotkey_last_dir_file()?, format!("{}\n", dir.display()))?;
    }

    if let Some(repo) = repo_dir {
        std::fs::write(
            mac_hotkey_last_repo_file()?,
            format!("{}\n", repo.display()),
        )?;
    }

    Ok(())
}

/// Whether `dir` should be recorded as the "last project" directory for the
/// `Cmd+'` hotkey. Home is skipped because it already has its own `Cmd+;`
/// hotkey, so recording it would make `Cmd+'` redundant with `Cmd+;`.
#[cfg(any(test, target_os = "macos", target_os = "linux", windows))]
fn should_record_last_dir(dir: &std::path::Path, home: Option<&std::path::Path>) -> bool {
    home != Some(dir)
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

/// Launch a new jcode window in the user's preferred macOS terminal, passing
/// `extra_args` (e.g. `["--resume", "<session-id>"]`) to the jcode invocation.
///
/// This reuses the same terminal detection as the global Cmd+; hotkey, but
/// deliberately avoids AppleScript automation: callers like the menu bar
/// helper run as background processes that cannot present the "control
/// Terminal" TCC prompt, so `osascript` would fail. Terminals that support
/// `open -na <App> --args ...` are launched directly; for the rest we write
/// the launch command to an executable `.command` file and `open` it, which
/// Terminal/iTerm run in a new window without any automation permission.
#[cfg(target_os = "macos")]
pub fn launch_jcode_in_macos_terminal(extra_args: &[String]) -> Result<()> {
    let terminal = effective_macos_terminal();
    let exe = std::env::current_exe()?;
    let exe_path = exe.to_string_lossy().into_owned();
    let shell_command = macos_terminal::paused_jcode_shell_command_with_args(&exe_path, extra_args);

    let command = match macos_terminal::no_automation_launch(terminal, &shell_command) {
        macos_terminal::NoAutomationLaunch::Shell(command) => command,
        macos_terminal::NoAutomationLaunch::CommandFile { app } => {
            let dir = storage::jcode_dir()?.join("launcher");
            std::fs::create_dir_all(&dir)?;
            let script_path = dir.join("open_session.command");
            std::fs::write(
                &script_path,
                format!("#!/bin/bash\nclear\n{shell_command}\n"),
            )?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))?;
            }
            let target =
                macos_terminal::escape_shell_single_quotes(script_path.to_string_lossy().as_ref());
            match app {
                Some(app) => format!("/usr/bin/open -a {app} '{target}'"),
                None => format!("/usr/bin/open '{target}'"),
            }
        }
    };

    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(&command)
        .status()
        .context("failed to launch terminal for jcode")?;
    if !status.success() {
        anyhow::bail!(
            "terminal launch command exited with status {:?}",
            status.code()
        );
    }
    Ok(())
}

/// Write one launch script per resolved hotkey into `hotkey_dir`, mark them
/// executable, and return the chord -> script plan the listener will register.
///
/// Extracted from [`install_macos_hotkey_listener`] so the script set + plan can
/// be verified in tests without invoking `launchctl`.
#[cfg(target_os = "macos")]
fn write_hotkey_launch_scripts(
    hotkey_dir: &std::path::Path,
    terminal: MacTerminalKind,
    exe_path: &str,
    resolved: &[launch_hotkeys::ResolvedLaunchHotkey],
) -> Result<Vec<launch_hotkeys::PlanEntry>> {
    let mut plan = Vec::with_capacity(resolved.len());
    for entry in resolved {
        let shell_command = launch_hotkeys::shell_command_for(entry, exe_path);
        let script_path = hotkey_dir.join(&entry.script_file_name);
        std::fs::write(
            &script_path,
            launch_script_for_macos_terminal(terminal, &shell_command),
        )?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))?;
        }
        plan.push(launch_hotkeys::PlanEntry {
            chord: entry.chord.clone(),
            script: script_path.to_string_lossy().into_owned(),
        });
    }
    Ok(plan)
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

    let last_dir_file = mac_hotkey_last_dir_file()?;
    let last_repo_file = mac_hotkey_last_repo_file()?;

    // Resolve the chord -> directory layout from config (empty config -> the
    // three built-in hotkeys), write one launch script per entry, and persist a
    // plan.json the listener registers from.
    let config = load_launch_hotkeys_config();
    let resolved = launch_hotkeys::resolve_launch_hotkeys(
        &config,
        &exe_path,
        &last_dir_file.to_string_lossy(),
        &last_repo_file.to_string_lossy(),
    );
    let plan = write_hotkey_launch_scripts(&hotkey_dir, terminal, &exe_path, &resolved)?;
    std::fs::write(
        mac_hotkey_plan_file()?,
        serde_json::to_string_pretty(&plan)?,
    )?;

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

fn startup_hints_for_launch(_state: &SetupHintsState) -> Option<StartupHints> {
    #[cfg(any(test, target_os = "macos"))]
    let spawn_notice = if !_state.hotkey_configured || _state.startup_spawn_hint_dismissed {
        None
    } else {
        Some(format!(
            "Cmd+; launches a new jcode in your home directory from anywhere, system-wide (opens in {}). Cmd+' reopens your last project; Cmd+Shift+' opens a self-dev session.",
            effective_macos_terminal().label()
        ))
    };
    #[cfg(not(any(test, target_os = "macos")))]
    let spawn_notice: Option<String> = None;

    spawn_notice.map(StartupHints::with_spawn_notice)
}

/// Read a single-character choice from the user.
#[cfg(windows)]
fn read_choice() -> String {
    let mut input = String::new();
    let _ = io::stdin().read_line(&mut input);
    input.trim().to_lowercase()
}

/// Pure decision for the macOS terminal notice, given the detected terminal.
///
/// We deliberately only nudge for the default built-in Terminal.app: other
/// terminals (iTerm2, WezTerm, Alacritty, Ghostty, etc.) are fine, so we leave
/// them alone. Regardless of the result the nudge is marked handled so it is
/// only ever shown once. The notice is informational (no prompt, no AI handoff).
///
/// This mutates `state`'s nudge flags but does not persist; the caller is
/// responsible for saving.
#[cfg(any(test, target_os = "macos"))]
fn macos_terminal_notice(
    state: &mut SetupHintsState,
    terminal: MacTerminalKind,
) -> Option<StartupHints> {
    state.mac_ghostty_guided = true;
    state.mac_ghostty_dismissed = true;

    if terminal != MacTerminalKind::AppleTerminal {
        return None;
    }

    let message = "The built-in macOS Terminal.app renders jcode poorly (slow, limited colors, no inline images). Consider a modern terminal such as Ghostty, iTerm2, or Alacritty for a much better experience.".to_string();

    Some(StartupHints::with_status_and_display(
        "Tip: Terminal.app renders jcode poorly. Try Ghostty, iTerm2, or Alacritty.".to_string(),
        "Terminal",
        message,
    ))
}

/// macOS entry point: show the one-time Terminal.app notice for the effective
/// terminal.
#[cfg(target_os = "macos")]
fn nudge_macos_ghostty(state: &mut SetupHintsState) -> Option<StartupHints> {
    let hints = macos_terminal_notice(state, effective_macos_terminal());
    let _ = state.save();
    hints
}

/// Manual `jcode setup-hotkey` command.
///
/// Runs the full interactive setup flow regardless of launch count.
#[cfg_attr(
    target_os = "linux",
    allow(
        clippy::needless_return,
        reason = "explicit return ends a cfg-gated block"
    )
)]
pub fn run_setup_hotkey(
    _listen_macos_hotkey: bool,
    _listen_windows_hotkey: bool,
    _uninstall: bool,
    notify_cli_launch: Option<&str>,
) -> Result<()> {
    if let Some(source) = notify_cli_launch {
        cli_launch_hints::maybe_notify(source)?;
        return Ok(());
    }
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
        eprintln!("  Installing a LaunchAgent with three system-wide jcode launch hotkeys.");
        eprintln!();

        match install_macos_hotkey_listener(Some(terminal)) {
            Ok(installed_terminal) => {
                state.hotkey_configured = true;
                state.hotkey_dismissed = true;
                state.hotkey_listener_version = HOTKEY_LISTENER_VERSION;
                state.launch_hotkey_tracking_version = LAUNCH_HOTKEY_TRACKING_VERSION;
                let _ = state.save();
                eprintln!(
                    "  \x1b[32m✓\x1b[0m Created launch hotkeys → {} + jcode",
                    installed_terminal.label()
                );
                eprintln!();
                eprintln!("  Press these anywhere, system-wide:");
                eprintln!("    \x1b[1mCmd+;\x1b[0m       new jcode in your home directory");
                eprintln!("    \x1b[1mCmd+'\x1b[0m       new jcode in your last project directory");
                eprintln!(
                    "    \x1b[1mCmd+Shift+'\x1b[0m new jcode self-dev session (last jcode repo)"
                );
                install_cli_launch_hints_notice();
                return Ok(());
            }
            Err(e) => {
                eprintln!("  \x1b[31m✗\x1b[0m Failed: {}", e);
                anyhow::bail!("macOS hotkey setup failed: {}", e);
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        let mut state = SetupHintsState::load();
        eprintln!("\x1b[1mjcode setup-hotkey\x1b[0m");
        eprintln!();
        if let Some(comp) = detect_linux_compositor() {
            let hotkeys = resolve_linux_hotkeys();
            match install_linux_launch_hotkeys(comp) {
                Ok(changed) => {
                    if changed {
                        eprintln!(
                            "  \x1b[32m✓\x1b[0m Installed jcode launch hotkeys into your {} config.",
                            comp.name()
                        );
                    } else {
                        eprintln!(
                            "  \x1b[32m✓\x1b[0m jcode launch hotkeys already up to date in your {} config.",
                            comp.name()
                        );
                    }
                    state.hotkey_configured = true;
                    state.hotkey_dismissed = true;
                    state.launch_hotkey_tracking_version = LAUNCH_HOTKEY_TRACKING_VERSION;
                    let _ = state.save();
                    eprintln!();
                    eprintln!("  Press these anywhere, system-wide:");
                    for hk in &hotkeys {
                        if linux_chord_expressible(comp, &hk.chord) {
                            let suffix = if hk.self_dev { " [self-dev]" } else { "" };
                            eprintln!(
                                "    \x1b[1m{}\x1b[0m → {} ({}){}",
                                hk.chord.display_super(),
                                hk.label,
                                hk.dir,
                                suffix
                            );
                        }
                    }
                    install_cli_launch_hints_notice();
                    return Ok(());
                }
                Err(e) => {
                    eprintln!("  \x1b[31m✗\x1b[0m Failed: {}", e);
                    anyhow::bail!("{} hotkey setup failed: {}", comp.name(), e);
                }
            }
        }

        eprintln!(
            "Automatic global hotkey setup on Linux supports niri, Hyprland (omarchy), sway, i3, bspwm, GNOME, KDE Plasma, Cinnamon, MATE, and XFCE."
        );
        eprintln!("Your session does not appear to be one of these.");
        eprintln!();
        eprintln!("Add a keybinding in your desktop environment's keyboard settings instead.");
        return Ok(());
    }

    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
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
        if _listen_windows_hotkey {
            return run_windows_hotkey_listener();
        }
        if _uninstall {
            return uninstall_windows_hotkey_listener();
        }
        run_setup_hotkey_windows()
    }
}

/// Install event-driven launch reminders into CLIs that are already present.
/// This is best-effort because the global hotkey itself is the primary feature;
/// a malformed third-party config must not turn successful hotkey setup into a
/// failure. Both integrations use the CLIs' native `SessionStart` lifecycle
/// event and never inspect command arguments, prompts, or process lists.
pub(crate) fn install_cli_launch_hints_notice() {
    match cli_launch_hints::install_available() {
        Ok(installed) if !installed.is_empty() => {
            eprintln!();
            eprintln!(
                "  \x1b[32m✓\x1b[0m Added launch-shortcut reminders to {}.",
                installed.join(" and ")
            );
            eprintln!("    Uses native SessionStart hooks; no prompts or commands are read.");
            if installed.iter().any(|name| name == "Codex CLI") {
                eprintln!(
                    "    Codex will ask you to review and trust the user hook once via /hooks."
                );
            }
        }
        Ok(_) => {}
        Err(err) => jcode_logging::warn(&format!(
            "could not install external CLI launch-shortcut reminders: {err}"
        )),
    }
}

/// Return the installed primary global launch shortcut as `(canonical, display)`.
/// A reminder is suppressed unless the shortcut is known to be active.
pub(crate) fn active_primary_launch_hotkey() -> Option<(String, String)> {
    let config = load_launch_hotkeys_config();
    if config.enabled == Some(false) {
        return None;
    }

    #[cfg(target_os = "macos")]
    {
        let state = SetupHintsState::load();
        if !state.hotkey_configured {
            return None;
        }
        let exe_path = std::env::current_exe()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "jcode".to_string());
        let last_dir = mac_hotkey_last_dir_file()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let last_repo = mac_hotkey_last_repo_file()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        return launch_hotkeys::resolve_launch_hotkeys(&config, &exe_path, &last_dir, &last_repo)
            .into_iter()
            .find(|entry| !entry.args.iter().any(|arg| arg == "self-dev"))
            .map(|entry| {
                let display = keymap::KeyChord::parse(&entry.chord)
                    .map(|chord| chord.display_symbols())
                    .unwrap_or_else(|| entry.chord.clone());
                (entry.chord, display)
            });
    }

    #[cfg(target_os = "linux")]
    {
        let comp = detect_linux_compositor()?;
        if !linux_hotkeys_installed(comp) {
            return None;
        }
        return resolve_linux_hotkeys()
            .into_iter()
            .find(|entry| !entry.self_dev && linux_chord_expressible(comp, &entry.chord))
            .map(|entry| (entry.chord.canonical(), entry.chord.display_super()));
    }

    #[cfg(windows)]
    {
        if !SetupHintsState::load().hotkey_configured {
            return None;
        }
        return windows_setup::primary_hotkey_display();
    }

    #[allow(unreachable_code)]
    None
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
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
    use jcode_terminal_launch::{TerminalCommand, spawn_command_in_new_terminal_with};

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
    jcode_logging::init();
    macos_hotkey_log("starting macOS jcode launch hotkey listener");

    let status = macos_run_loop::promote_to_ui_element();
    if status != 0 {
        macos_hotkey_log(&format!(
            "warning: TransformProcessType returned status {status}; \
             hotkeys may not be delivered to this process"
        ));
    }

    let manager =
        GlobalHotKeyManager::new().context("failed to initialize global hotkey manager")?;

    // Register each configured launch hotkey and map its registration id directly
    // to a cwd + jcode argv. Older versions dispatched through generated shell
    // scripts; keeping this direct avoids a shell/AppleScript hop and prevents
    // stale script contents from disagreeing with the live config.
    let launches = load_direct_hotkey_launches();
    let mut launch_for_id: std::collections::HashMap<u32, DirectHotkeyLaunch> =
        std::collections::HashMap::new();
    for entry in &launches {
        let Some(chord) = keymap::KeyChord::parse(&entry.chord) else {
            macos_hotkey_log(&format!("skipping unparseable chord: {}", entry.chord));
            continue;
        };
        let Some(hotkey) = launch_hotkeys::chord_to_global_hotkey(&chord) else {
            macos_hotkey_log(&format!("skipping unregisterable chord: {}", entry.chord));
            continue;
        };
        match manager.register(hotkey) {
            Ok(()) => {
                launch_for_id.insert(hotkey.id(), entry.clone());
                macos_hotkey_log(&format!(
                    "registered {} → {} ({})",
                    chord.display(),
                    entry.dir,
                    entry.label
                ));
            }
            Err(err) => macos_hotkey_log(&format!(
                "failed to register {} hotkey: {err}",
                chord.display()
            )),
        }
    }

    if launch_for_id.is_empty() {
        anyhow::bail!("failed to register any jcode launch hotkey");
    }

    let exe_path = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "jcode".to_string());

    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.state != HotKeyState::Pressed {
            return;
        }
        if let Some(launch) = launch_for_id.get(&event.id) {
            macos_hotkey_log(&format!(
                "hotkey pressed; launching {} in {}",
                launch.label,
                launch.resolved_cwd().display()
            ));
            let cwd = launch.resolved_cwd();
            let mut args = vec!["--spawn-hotkey".to_string(), launch.chord.clone()];
            args.extend(launch.args.clone());
            let command = TerminalCommand::new(&exe_path, args)
                .fresh_spawn()
                .kind("hotkey")
                .spawn_env("JCODE_SPAWN_LABEL", launch.label.clone());
            match spawn_command_in_new_terminal_with(&command, &cwd, |cmd| cmd.spawn().map(|_| ()))
            {
                Ok(true) => {}
                Ok(false) => {
                    macos_hotkey_log("failed to launch jcode: no terminal candidate worked")
                }
                Err(err) => macos_hotkey_log(&format!("failed to launch jcode: {err}")),
            }
        }
    }));

    macos_hotkey_log("macOS jcode launch hotkeys registered; entering event loop");
    // Keep the manager alive for the lifetime of the event loop so the hotkey
    // registration and event handler stay installed.
    let _manager = manager;
    // Hand the main thread to the Carbon event loop so hotkey events are
    // delivered. This normally never returns for our long-lived listener.
    macos_run_loop::run_forever();
    macos_hotkey_log("macOS jcode launch hotkey event loop exited");
    Ok(())
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone)]
struct DirectHotkeyLaunch {
    chord: String,
    dir: String,
    last_dir_file: String,
    last_repo_file: String,
    args: Vec<String>,
    label: String,
}

#[cfg(target_os = "macos")]
impl DirectHotkeyLaunch {
    fn resolved_cwd(&self) -> PathBuf {
        launch_hotkeys::resolve_target_dir(&self.dir, &self.last_dir_file, &self.last_repo_file)
    }
}

/// Load the live config into concrete direct-launch entries for the listener.
/// Dynamic targets (`$LAST_DIR`, `$LAST_REPO`) keep their source files and are
/// resolved at keypress time, so "last project" tracks new launches without a
/// listener restart.
#[cfg(target_os = "macos")]
fn load_direct_hotkey_launches() -> Vec<DirectHotkeyLaunch> {
    let exe_path = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "jcode".to_string());
    let last_dir = mac_hotkey_last_dir_file()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let last_repo = mac_hotkey_last_repo_file()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let config = load_launch_hotkeys_config();

    launch_hotkeys::resolve_launch_hotkeys(&config, &exe_path, &last_dir, &last_repo)
        .into_iter()
        .map(|entry| DirectHotkeyLaunch {
            chord: entry.chord,
            dir: entry.dir,
            last_dir_file: last_dir.clone(),
            last_repo_file: last_repo.clone(),
            args: entry.args,
            label: entry.label,
        })
        .collect()
}

/// Record one successful global launch-hotkey use. Launchers pass the canonical
/// chord through the hidden `--spawn-hotkey` argument; canonicalizing again here
/// keeps persisted learning state stable even if an older launcher passes a
/// differently ordered spelling.
pub fn record_launch_hotkey_use(chord: &str) {
    let Some(chord) = keymap::KeyChord::parse(chord).map(|chord| chord.canonical()) else {
        jcode_logging::warn(&format!(
            "ignored invalid launch hotkey usage chord: {chord}"
        ));
        return;
    };
    let mut state = SetupHintsState::load();
    let uses = state.launch_hotkey_usage.entry(chord.clone()).or_insert(0);
    *uses = uses.saturating_add(1);
    if let Err(err) = state.save() {
        jcode_logging::warn(&format!(
            "failed to record launch hotkey usage for {chord}: {err}"
        ));
    }
}

/// Log a hotkey-listener diagnostic to both the jcode log and stderr.
///
/// The LaunchAgent redirects stdout/stderr to log files in the hotkey support
/// dir, so emitting to stderr here makes the listener's lifecycle observable
/// even before/without the structured logger.
#[cfg(target_os = "macos")]
fn macos_hotkey_log(message: &str) {
    jcode_logging::info(message);
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
                    jcode_logging::warn(&format!(
                        "failed to auto-install macOS Cmd+; hotkey listener: {err}"
                    ));
                }
            }
            MacHotkeyAction::Migrate => {
                // Already-configured user on an older listener: reinstall so the
                // updated listener (and current binary path) takes effect on
                // update without requiring them to re-run setup.
                if let Err(err) = migrate_macos_hotkey_listener(&mut state) {
                    jcode_logging::warn(&format!(
                        "failed to migrate macOS Cmd+; hotkey listener: {err}"
                    ));
                }
            }
            MacHotkeyAction::None => {}
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Some(comp) = detect_linux_compositor() {
            let action = linux_hotkey_setup_action(
                load_launch_hotkeys_config().enabled,
                linux_hotkeys_installed(comp),
                state.launch_hotkey_tracking_version,
            );

            if action != LinuxHotkeySetupAction::None {
                match install_linux_launch_hotkeys(comp) {
                    Ok(_) => {
                        state.hotkey_configured = true;
                        state.hotkey_dismissed = true;
                        state.launch_hotkey_tracking_version = LAUNCH_HOTKEY_TRACKING_VERSION;
                        let _ = state.save();
                        if action == LinuxHotkeySetupAction::Install {
                            jcode_logging::info(&format!(
                                "Automatically installed {} launch hotkeys on first launch",
                                comp.name()
                            ));
                        } else {
                            jcode_logging::info(&format!(
                                "Migrated {} launch hotkeys to usage tracking v{}",
                                comp.name(),
                                LAUNCH_HOTKEY_TRACKING_VERSION
                            ));
                        }
                    }
                    Err(err) => jcode_logging::warn(&format!(
                        "failed to automatically configure {} launch hotkeys: {err}",
                        comp.name()
                    )),
                }
            }
        }
    }

    #[cfg(windows)]
    {
        if state.hotkey_configured
            && state.launch_hotkey_tracking_version < LAUNCH_HOTKEY_TRACKING_VERSION
        {
            match windows_setup::refresh_windows_launch_hotkeys() {
                Ok(()) => {
                    state.launch_hotkey_tracking_version = LAUNCH_HOTKEY_TRACKING_VERSION;
                    let _ = state.save();
                    jcode_logging::info("Migrated Windows launch hotkeys to usage tracking");
                }
                Err(err) => jcode_logging::warn(&format!(
                    "failed to migrate Windows launch hotkeys to usage tracking: {err}"
                )),
            }
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
    let startup_hints = startup_hints.or_else(|| macos_launch_hotkeys_notice(&state));

    #[cfg(target_os = "macos")]
    {
        if state.launch_count % 3 != 0 {
            return startup_hints;
        }

        if !state.mac_ghostty_guided
            && !state.mac_ghostty_dismissed
            && state.nudge_budget_remaining()
        {
            state.record_nudge_shown();
            // Prefer any earlier-launch hint (alignment/welcome) if present so we
            // do not clobber it; otherwise surface the Terminal.app notice.
            if startup_hints.is_some() {
                // Still mark the nudge as handled so it is only ever shown once.
                let _ = nudge_macos_ghostty(&mut state);
                return startup_hints;
            }
            return nudge_macos_ghostty(&mut state);
        }

        return startup_hints;
    }

    #[cfg(windows)]
    {
        let startup_hints =
            startup_hints.or_else(|| windows_setup::windows_launch_hotkeys_notice(&state));
        return maybe_show_windows_setup_hints(&mut state, startup_hints);
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        startup_hints.or_else(|| {
            #[cfg(target_os = "linux")]
            {
                linux_launch_hotkeys_notice(&state)
            }
            #[cfg(not(target_os = "linux"))]
            {
                None
            }
        })
    }
}

#[cfg(target_os = "macos")]
fn macos_launch_hotkeys_notice(state: &SetupHintsState) -> Option<StartupHints> {
    let config = load_launch_hotkeys_config();
    if config.enabled == Some(false) {
        return None;
    }
    let exe_path = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "jcode".to_string());
    let last_dir = mac_hotkey_last_dir_file()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let last_repo = mac_hotkey_last_repo_file()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let entries = launch_hotkeys::resolve_launch_hotkeys(&config, &exe_path, &last_dir, &last_repo);
    if entries.is_empty() {
        return None;
    }

    let rows: Vec<LaunchHotkeyRow> = entries
        .into_iter()
        .map(|entry| {
            let cwd = launch_hotkeys::resolve_target_dir(&entry.dir, &last_dir, &last_repo);
            let display = keymap::KeyChord::parse(&entry.chord)
                .map(|c| c.display_symbols())
                .unwrap_or_else(|| entry.chord.clone());
            LaunchHotkeyRow {
                chord: entry.chord,
                display,
                label: entry.label,
                cwd_display: cwd.display().to_string(),
                self_dev: entry.args.iter().any(|arg| arg == "self-dev"),
            }
        })
        .collect();

    let lines = launch_hotkey_notice_lines(&rows, &state.launch_hotkey_usage, state.launch_count)?;

    Some(StartupHints::with_status_and_display(
        "Launch hotkeys available".to_string(),
        "Launch hotkeys",
        format!("Configured Jcode launch hotkeys:\n{}", lines.join("\n")),
    ))
}

// ===========================================================================
// Linux global launch hotkeys (niri, Hyprland/omarchy, sway, i3)
//
// Wayland clients cannot grab system-wide hotkeys, so on Linux we bind the
// keys in the compositor's own config. niri and Hyprland hot-reload their
// configs on save; sway/i3 get an explicit `reload` IPC call.
// ===========================================================================

/// Detect the running compositor/window manager from the session environment.
#[cfg(target_os = "linux")]
fn detect_linux_compositor() -> Option<linux_env::LinuxCompositor> {
    linux_env::detect_compositor_from(&|key| std::env::var(key).ok())
}

/// Path to the niri config file, honoring `$XDG_CONFIG_HOME`.
#[cfg(any(test, target_os = "linux"))]
fn niri_config_path() -> Option<PathBuf> {
    Some(xdg_config_home()?.join("niri").join("config.kdl"))
}

/// `$XDG_CONFIG_HOME`, defaulting to `~/.config`.
#[cfg(any(test, target_os = "linux"))]
fn xdg_config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".config")))
}

/// Config file jcode manages for a flat (`#`-commented) compositor config.
/// For i3 the legacy `~/.i3/config` location is honored when the XDG path is
/// missing. GNOME/KDE do not use a spliceable config file and return `None`.
#[cfg(target_os = "linux")]
fn flat_compositor_config_path(comp: linux_env::LinuxCompositor) -> Option<PathBuf> {
    use linux_env::LinuxCompositor;
    let base = xdg_config_home()?;
    match comp {
        LinuxCompositor::Niri => niri_config_path(),
        LinuxCompositor::Hyprland => Some(base.join("hypr").join("hyprland.conf")),
        LinuxCompositor::Sway => Some(base.join("sway").join("config")),
        LinuxCompositor::Bspwm => Some(base.join("sxhkd").join("sxhkdrc")),
        LinuxCompositor::I3 => {
            let xdg = base.join("i3").join("config");
            if xdg.exists() {
                return Some(xdg);
            }
            let legacy = dirs::home_dir()?.join(".i3").join("config");
            if legacy.exists() {
                Some(legacy)
            } else {
                Some(xdg)
            }
        }
        LinuxCompositor::Gnome
        | LinuxCompositor::Kde
        | LinuxCompositor::Cinnamon
        | LinuxCompositor::Mate
        | LinuxCompositor::Xfce => None,
    }
}

/// KDE's global-shortcuts registry file.
#[cfg(target_os = "linux")]
fn kde_globalshortcutsrc_path() -> Option<PathBuf> {
    Some(xdg_config_home()?.join("kglobalshortcutsrc"))
}

/// Directory for jcode's hidden KDE launcher desktop files.
#[cfg(target_os = "linux")]
fn kde_applications_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".local").join("share")))?;
    Some(base.join("applications"))
}

/// The config file jcode would manage for the *current* session's compositor.
#[cfg(target_os = "linux")]
fn linux_hotkey_config_path(comp: linux_env::LinuxCompositor) -> Option<PathBuf> {
    match comp {
        linux_env::LinuxCompositor::Niri => niri_config_path(),
        linux_env::LinuxCompositor::Kde => kde_globalshortcutsrc_path(),
        other => flat_compositor_config_path(other),
    }
}

/// Human description of where the binds land, for the startup notice footer.
#[cfg(target_os = "linux")]
fn linux_hotkey_target_description(comp: linux_env::LinuxCompositor) -> String {
    use linux_env::LinuxCompositor;
    match comp {
        LinuxCompositor::Gnome => "GNOME custom shortcuts (via dconf)".to_string(),
        LinuxCompositor::Kde => "KDE global shortcuts (kglobalshortcutsrc)".to_string(),
        LinuxCompositor::Cinnamon => "Cinnamon custom shortcuts (via dconf)".to_string(),
        LinuxCompositor::Mate => "MATE custom shortcuts (via dconf)".to_string(),
        LinuxCompositor::Xfce => "XFCE keyboard shortcuts (via xfconf)".to_string(),
        other => {
            let path = linux_hotkey_config_path(other)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "its config".to_string());
            format!("your {} config ({})", other.name(), path)
        }
    }
}

/// The sentinel that marks jcode's managed region in `path` for `comp`.
#[cfg(target_os = "linux")]
fn linux_hotkey_sentinel(comp: linux_env::LinuxCompositor) -> &'static str {
    match comp {
        linux_env::LinuxCompositor::Niri => linux_niri::NIRI_BLOCK_BEGIN,
        _ => linux_env::HASH_BLOCK_BEGIN,
    }
}

/// Whether jcode's launch hotkeys are already installed for `comp`.
#[cfg(target_os = "linux")]
fn linux_hotkeys_installed(comp: linux_env::LinuxCompositor) -> bool {
    use linux_env::LinuxCompositor;
    match comp {
        LinuxCompositor::Gnome => gnome_keybinding_list().contains("/jcode-launch-"),
        LinuxCompositor::Cinnamon => {
            dconf_read("/org/cinnamon/desktop/keybindings/custom-list").contains("jcode-launch-")
        }
        LinuxCompositor::Mate => {
            dconf_list("/org/mate/desktop/keybindings/").contains("jcode-launch-")
        }
        LinuxCompositor::Xfce => xfce_shortcut_commands_text().contains("/launch_jcode_"),
        LinuxCompositor::Kde => kde_globalshortcutsrc_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|text| text.contains("[services][jcode-launch-"))
            .unwrap_or(false),
        other => linux_hotkey_config_path(other)
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|text| text.contains(linux_hotkey_sentinel(other)))
            .unwrap_or(false),
    }
}

#[cfg(any(test, target_os = "linux"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinuxHotkeySetupAction {
    None,
    Install,
    Refresh,
}

#[cfg(any(test, target_os = "linux"))]
fn linux_hotkey_setup_action(
    enabled: Option<bool>,
    installed: bool,
    tracking_version: u32,
) -> LinuxHotkeySetupAction {
    if enabled == Some(false) {
        LinuxHotkeySetupAction::None
    } else if !installed {
        LinuxHotkeySetupAction::Install
    } else if tracking_version < LAUNCH_HOTKEY_TRACKING_VERSION {
        LinuxHotkeySetupAction::Refresh
    } else {
        LinuxHotkeySetupAction::None
    }
}

/// Pick a terminal emulator to launch jcode in on Linux. Honors `$TERMINAL`,
/// otherwise probes common emulators on `PATH`, falling back to `kitty`.
#[cfg(any(test, target_os = "linux"))]
fn linux_launch_terminal() -> String {
    if let Ok(t) = std::env::var("TERMINAL")
        && !t.trim().is_empty()
    {
        return t;
    }
    const CANDIDATES: [&str; 6] = [
        "kitty",
        "alacritty",
        "foot",
        "wezterm",
        "ghostty",
        "konsole",
    ];
    for cand in CANDIDATES {
        if binary_on_path(cand) {
            return cand.to_string();
        }
    }
    "kitty".to_string()
}

/// Whether `name` resolves to an executable on `$PATH`.
#[cfg(any(test, target_os = "linux"))]
fn binary_on_path(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        let candidate = dir.join(name);
        candidate.is_file()
    })
}

/// Resolve the configured launch hotkeys into concrete Linux hotkeys, with each
/// directory sentinel expanded to a real path.
#[cfg(any(test, target_os = "linux"))]
fn resolve_linux_hotkeys() -> Vec<linux_niri::NiriHotkey> {
    let config = load_launch_hotkeys_config();
    let exe_path = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "jcode".to_string());
    let last_dir = mac_hotkey_last_dir_file()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let last_repo = mac_hotkey_last_repo_file()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let resolved =
        launch_hotkeys::resolve_launch_hotkeys(&config, &exe_path, &last_dir, &last_repo);
    resolved
        .into_iter()
        .filter_map(|entry| {
            let chord = crate::keymap::KeyChord::parse(&entry.chord)?;
            let cwd = launch_hotkeys::resolve_target_dir(&entry.dir, &last_dir, &last_repo);
            Some(linux_niri::NiriHotkey {
                chord,
                dir: cwd.to_string_lossy().into_owned(),
                label: entry.label,
                self_dev: entry.args.iter().any(|a| a == "self-dev"),
            })
        })
        .collect()
}

/// Whether `chord` can be expressed as a binding for `comp` (used to filter
/// the startup notice down to hotkeys that would actually install).
#[cfg(target_os = "linux")]
fn linux_chord_expressible(comp: linux_env::LinuxCompositor, chord: &keymap::KeyChord) -> bool {
    match comp {
        linux_env::LinuxCompositor::Niri => linux_niri::chord_to_niri_bind(chord).is_some(),
        linux_env::LinuxCompositor::Kde => linux_env::kde_shortcut(chord).is_some(),
        linux_env::LinuxCompositor::Gnome
        | linux_env::LinuxCompositor::Cinnamon
        | linux_env::LinuxCompositor::Mate
        | linux_env::LinuxCompositor::Xfce => linux_env::gnome_binding(chord).is_some(),
        _ => linux_env::xkb_key_name(&chord.key).is_some(),
    }
}

/// Install (or refresh) the launch hotkeys for the detected compositor.
/// Returns `Ok(true)` if anything was changed.
#[cfg(target_os = "linux")]
fn install_linux_launch_hotkeys(comp: linux_env::LinuxCompositor) -> Result<bool> {
    use linux_env::LinuxCompositor;
    match comp {
        LinuxCompositor::Niri => install_niri_launch_hotkeys(),
        LinuxCompositor::Gnome => install_gnome_launch_hotkeys(),
        LinuxCompositor::Kde => install_kde_launch_hotkeys(),
        LinuxCompositor::Cinnamon => install_cinnamon_launch_hotkeys(),
        LinuxCompositor::Mate => install_mate_launch_hotkeys(),
        LinuxCompositor::Xfce => install_xfce_launch_hotkeys(),
        other => install_flat_launch_hotkeys(other),
    }
}

/// Install (or refresh) the niri launch-hotkey binds into the user's
/// `config.kdl`. Writes a timestamped backup before modifying, and is a no-op
/// when the managed block already matches. Returns `Ok(true)` if the config was
/// changed.
#[cfg(target_os = "linux")]
fn install_niri_launch_hotkeys() -> Result<bool> {
    let Some(config_path) = niri_config_path() else {
        anyhow::bail!("could not locate niri config path");
    };
    if !config_path.exists() {
        anyhow::bail!("niri config not found at {}", config_path.display());
    }

    let exe_path = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "jcode".to_string());
    let terminal = linux_launch_terminal();
    let hotkeys = resolve_linux_hotkeys();

    let Some(block) = linux_niri::render_niri_block(&hotkeys, &exe_path, &terminal, "    ") else {
        anyhow::bail!("no installable launch hotkeys for niri");
    };

    let current = std::fs::read_to_string(&config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    let result = linux_niri::splice_managed_block(&current, &block);
    if !result.changed {
        return Ok(false);
    }

    backup_compositor_config(&config_path);

    storage::write_bytes(&config_path, result.text.as_bytes())
        .with_context(|| format!("writing {}", config_path.display()))?;
    jcode_logging::info(&format!(
        "installed {} niri launch hotkey(s) into {}",
        hotkeys.len(),
        config_path.display()
    ));
    Ok(true)
}

/// Install (or refresh) the launch-hotkey binds for a flat `#`-commented
/// compositor config (Hyprland/omarchy, sway, i3). Bind lines execute launch
/// scripts written under `~/.jcode/hotkey/`, so the config never embeds shell
/// one-liners. Writes a timestamped backup before modifying; no-op when the
/// managed block already matches. Returns `Ok(true)` if the config changed.
#[cfg(target_os = "linux")]
fn install_flat_launch_hotkeys(comp: linux_env::LinuxCompositor) -> Result<bool> {
    let Some(config_path) = flat_compositor_config_path(comp) else {
        anyhow::bail!("could not locate {} config path", comp.name());
    };
    if !config_path.exists() {
        anyhow::bail!(
            "{} config not found at {}",
            comp.name(),
            config_path.display()
        );
    }

    let binds = write_linux_launch_scripts()?;
    let block = match comp {
        linux_env::LinuxCompositor::Hyprland => linux_env::render_hyprland_block(&binds),
        linux_env::LinuxCompositor::Sway | linux_env::LinuxCompositor::I3 => {
            linux_env::render_sway_block(&binds)
        }
        linux_env::LinuxCompositor::Bspwm => linux_env::render_sxhkd_block(&binds),
        linux_env::LinuxCompositor::Niri
        | linux_env::LinuxCompositor::Gnome
        | linux_env::LinuxCompositor::Kde
        | linux_env::LinuxCompositor::Cinnamon
        | linux_env::LinuxCompositor::Mate
        | linux_env::LinuxCompositor::Xfce => {
            unreachable!("handled by dedicated install paths")
        }
    };
    let Some(block) = block else {
        anyhow::bail!("no installable launch hotkeys for {}", comp.name());
    };

    let current = std::fs::read_to_string(&config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;
    let result = linux_env::splice_flat_managed_block(&current, &block);
    if !result.changed {
        return Ok(false);
    }

    backup_compositor_config(&config_path);

    storage::write_bytes(&config_path, result.text.as_bytes())
        .with_context(|| format!("writing {}", config_path.display()))?;
    jcode_logging::info(&format!(
        "installed {} {} launch hotkey(s) into {}",
        binds.len(),
        comp.name(),
        config_path.display()
    ));

    reload_compositor_config(comp);
    Ok(true)
}

/// Read one dconf key's textual value. Empty string on any failure (missing
/// dconf, unset key, etc.).
#[cfg(target_os = "linux")]
fn dconf_read(path: &str) -> String {
    std::process::Command::new("dconf")
        .args(["read", path])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// List a dconf directory's children. Empty string on failure.
#[cfg(target_os = "linux")]
fn dconf_list(dir: &str) -> String {
    std::process::Command::new("dconf")
        .args(["list", dir])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

/// Read GNOME's custom-keybindings list via dconf.
#[cfg(target_os = "linux")]
fn gnome_keybinding_list() -> String {
    dconf_read("/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings")
}

/// Write one dconf key. Errors bubble up so a missing dconf binary fails the
/// install with a clear message. dconf is used instead of gsettings because it
/// does not require the settings-daemon schemas to be installed in the running
/// environment, and the desktops' media-keys plugins read dconf directly.
#[cfg(target_os = "linux")]
fn dconf_write(path: &str, value: &str) -> Result<()> {
    let status = std::process::Command::new("dconf")
        .args(["write", path, value])
        .status()
        .context("failed to run dconf (is this a GNOME-family session?)")?;
    if !status.success() {
        anyhow::bail!("dconf write {path} failed with {status}");
    }
    Ok(())
}

/// Write one dconf key only when its value differs; reports whether a write
/// happened so installers can distinguish "installed" from "already up to
/// date".
#[cfg(target_os = "linux")]
fn dconf_write_checked(path: &str, value: &str) -> Result<bool> {
    if dconf_read(path) == value {
        return Ok(false);
    }
    dconf_write(path, value)?;
    Ok(true)
}

/// Quote a string as a GVariant string literal for `dconf write`.
#[cfg(target_os = "linux")]
fn gvariant_string(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// Install (or refresh) the launch hotkeys as GNOME custom keybindings via
/// dconf. Slot-stable paths make re-installs overwrite in place, and the
/// custom-keybindings list merge preserves the user's own entries. GNOME
/// applies dconf changes immediately; no reload needed.
#[cfg(target_os = "linux")]
fn install_gnome_launch_hotkeys() -> Result<bool> {
    let binds = write_linux_launch_scripts()?;
    let keybindings = linux_env::gnome_keybindings(&binds);
    if keybindings.is_empty() {
        anyhow::bail!("no installable launch hotkeys for GNOME");
    }

    // Point the custom-keybindings list at our slots (plus everything the
    // user already had).
    let list_path = "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings";
    let current = gnome_keybinding_list();
    let ours: Vec<String> = keybindings.iter().map(|kb| kb.path.clone()).collect();
    let merged = linux_env::merge_gnome_keybinding_list(&current, &ours);
    let mut changed = dconf_write_checked(list_path, &merged)?;

    for kb in &keybindings {
        changed |= dconf_write_checked(&format!("{}name", kb.path), &gvariant_string(&kb.name))?;
        changed |= dconf_write_checked(
            &format!("{}command", kb.path),
            &gvariant_string(&kb.command),
        )?;
        changed |= dconf_write_checked(
            &format!("{}binding", kb.path),
            &gvariant_string(&kb.binding),
        )?;
    }

    jcode_logging::info(&format!(
        "installed {} GNOME launch hotkey(s) via dconf",
        keybindings.len()
    ));
    Ok(changed)
}

/// Install (or refresh) the launch hotkeys as Cinnamon custom keybindings.
/// Same dconf-backed shape as GNOME but under `/org/cinnamon/`, with slot
/// names (not paths) in `custom-list` and array-typed bindings.
#[cfg(target_os = "linux")]
fn install_cinnamon_launch_hotkeys() -> Result<bool> {
    let binds = write_linux_launch_scripts()?;
    let keybindings = linux_env::dconf_keybindings(&binds);
    if keybindings.is_empty() {
        anyhow::bail!("no installable launch hotkeys for Cinnamon");
    }

    let list_path = "/org/cinnamon/desktop/keybindings/custom-list";
    let current = dconf_read(list_path);
    let ours: Vec<String> = keybindings.iter().map(|kb| kb.slot.clone()).collect();
    let merged = linux_env::merge_gnome_keybinding_list(&current, &ours);
    let mut changed = dconf_write_checked(list_path, &merged)?;

    for kb in &keybindings {
        let base = format!(
            "/org/cinnamon/desktop/keybindings/custom-keybindings/{}/",
            kb.slot
        );
        changed |= dconf_write_checked(&format!("{base}name"), &gvariant_string(&kb.name))?;
        changed |= dconf_write_checked(&format!("{base}command"), &gvariant_string(&kb.command))?;
        // Cinnamon bindings are arrays of accelerator strings.
        changed |= dconf_write_checked(
            &format!("{base}binding"),
            &format!("[{}]", gvariant_string(&kb.binding)),
        )?;
    }

    jcode_logging::info(&format!(
        "installed {} Cinnamon launch hotkey(s) via dconf",
        keybindings.len()
    ));
    Ok(changed)
}

/// Install (or refresh) the launch hotkeys as MATE custom keybindings under
/// `/org/mate/desktop/keybindings/`. MATE discovers slots from the dconf tree
/// itself, so there is no master list to merge.
#[cfg(target_os = "linux")]
fn install_mate_launch_hotkeys() -> Result<bool> {
    let binds = write_linux_launch_scripts()?;
    let keybindings = linux_env::dconf_keybindings(&binds);
    if keybindings.is_empty() {
        anyhow::bail!("no installable launch hotkeys for MATE");
    }

    let mut changed = false;
    for kb in &keybindings {
        let base = format!("/org/mate/desktop/keybindings/{}/", kb.slot);
        changed |= dconf_write_checked(&format!("{base}name"), &gvariant_string(&kb.name))?;
        // MATE calls the command key `action`.
        changed |= dconf_write_checked(&format!("{base}action"), &gvariant_string(&kb.command))?;
        changed |= dconf_write_checked(&format!("{base}binding"), &gvariant_string(&kb.binding))?;
    }

    jcode_logging::info(&format!(
        "installed {} MATE launch hotkey(s) via dconf",
        keybindings.len()
    ));
    Ok(changed)
}

/// All `property value` lines from XFCE's keyboard-shortcuts channel. Empty
/// on failure (missing xfconf-query, not XFCE).
#[cfg(target_os = "linux")]
fn xfce_shortcut_commands_text() -> String {
    std::process::Command::new("xfconf-query")
        .args(["-c", "xfce4-keyboard-shortcuts", "-l", "-v"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

/// Install (or refresh) the launch hotkeys as XFCE keyboard shortcuts via
/// xfconf-query. Stale jcode entries bound to accelerators we no longer use
/// are removed so a re-baked chord layout never leaves orphaned bindings.
#[cfg(target_os = "linux")]
fn install_xfce_launch_hotkeys() -> Result<bool> {
    let binds = write_linux_launch_scripts()?;

    let mut wanted: Vec<(String, String)> = Vec::new();
    for bind in &binds {
        if let Some(accel) = linux_env::gnome_binding(&bind.chord) {
            wanted.push((format!("/commands/custom/{accel}"), bind.script.clone()));
        }
    }
    if wanted.is_empty() {
        anyhow::bail!("no installable launch hotkeys for XFCE");
    }

    let existing = xfce_shortcut_commands_text();
    let mut changed = false;

    // Remove stale jcode entries bound to accelerators we no longer use.
    for line in existing.lines() {
        let Some((prop, value)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        if value.trim().contains("/launch_jcode_") && !wanted.iter().any(|(p, _)| p == prop) {
            let _ = std::process::Command::new("xfconf-query")
                .args(["-c", "xfce4-keyboard-shortcuts", "-p", prop, "-r"])
                .status();
            changed = true;
        }
    }

    for (prop, script) in &wanted {
        let already = existing.lines().any(|line| {
            line.split_once(char::is_whitespace)
                .is_some_and(|(p, v)| p == *prop && v.trim() == script)
        });
        if already {
            continue;
        }
        let status = std::process::Command::new("xfconf-query")
            .args([
                "-c",
                "xfce4-keyboard-shortcuts",
                "-p",
                prop,
                "-n",
                "-t",
                "string",
                "-s",
                script,
            ])
            .status()
            .context("failed to run xfconf-query (is this an XFCE session?)")?;
        if !status.success() {
            anyhow::bail!("xfconf-query set {prop} failed with {status}");
        }
        changed = true;
    }

    jcode_logging::info(&format!(
        "installed {} XFCE launch hotkey(s) via xfconf",
        wanted.len()
    ));
    Ok(changed)
}

/// Install (or refresh) the launch hotkeys as KDE global shortcuts: one hidden
/// desktop file per hotkey plus a `_launch=` binding in `kglobalshortcutsrc`.
/// kglobalaccel watches for desktop-file changes; a re-login may be needed on
/// older Plasma versions.
#[cfg(target_os = "linux")]
fn install_kde_launch_hotkeys() -> Result<bool> {
    let binds = write_linux_launch_scripts()?;
    let shortcuts = linux_env::kde_shortcuts(&binds);
    if shortcuts.is_empty() {
        anyhow::bail!("no installable launch hotkeys for KDE");
    }

    let apps_dir = kde_applications_dir()
        .ok_or_else(|| anyhow::anyhow!("could not locate ~/.local/share/applications"))?;
    std::fs::create_dir_all(&apps_dir)?;
    for sc in &shortcuts {
        std::fs::write(apps_dir.join(&sc.desktop_file_name), &sc.desktop_file_body)?;
    }

    let rc_path = kde_globalshortcutsrc_path()
        .ok_or_else(|| anyhow::anyhow!("could not locate kglobalshortcutsrc"))?;
    let current = std::fs::read_to_string(&rc_path).unwrap_or_default();
    let updated = linux_env::upsert_kde_shortcut_sections(&current, &shortcuts);
    let changed = updated != current;
    if changed {
        if rc_path.exists() {
            backup_compositor_config(&rc_path);
        }
        storage::write_bytes(&rc_path, updated.as_bytes())
            .with_context(|| format!("writing {}", rc_path.display()))?;
    }

    // Nudge kglobalaccel to re-read its config. Best-effort: the shortcuts
    // still land after the next login if the daemon isn't reachable.
    let _ = std::process::Command::new("kquitapp6")
        .arg("kglobalaccel")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    jcode_logging::info(&format!(
        "installed {} KDE launch hotkey(s) ({} + desktop files)",
        shortcuts.len(),
        rc_path.display()
    ));
    Ok(changed)
}

/// Write one executable launch script per resolved hotkey to
/// `~/.jcode/hotkey/` and return the chord -> script binds. The scripts `cd`
/// into the target (with `$HOME` fallback for stale/dynamic dirs) and exec the
/// user's terminal running jcode, so compositor bind lines stay trivial.
#[cfg(target_os = "linux")]
fn write_linux_launch_scripts() -> Result<Vec<linux_env::ScriptBind>> {
    let hotkey_dir = mac_hotkey_support_dir()?;
    std::fs::create_dir_all(&hotkey_dir)?;

    let exe_path = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "jcode".to_string());
    let terminal = linux_launch_terminal();
    let last_dir = mac_hotkey_last_dir_file()?.to_string_lossy().into_owned();
    let last_repo = mac_hotkey_last_repo_file()?.to_string_lossy().into_owned();

    let config = load_launch_hotkeys_config();
    let resolved =
        launch_hotkeys::resolve_launch_hotkeys(&config, &exe_path, &last_dir, &last_repo);

    let mut binds = Vec::with_capacity(resolved.len());
    for entry in resolved {
        let Some(chord) = keymap::KeyChord::parse(&entry.chord) else {
            continue;
        };
        let self_dev = entry.args.iter().any(|a| a == "self-dev");
        let exec = linux_env::terminal_exec_command(&terminal, &exe_path, &entry.chord, self_dev);
        let script_body = format!(
            "#!/bin/sh\n# Auto-generated by jcode setup-hotkey; re-run it to refresh.\n{cd}exec {exec}\n",
            cd = entry.cd_prefix,
        );
        let script_path = hotkey_dir.join(&entry.script_file_name);
        std::fs::write(&script_path, script_body)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))?;
        }
        binds.push(linux_env::ScriptBind {
            chord,
            script: script_path.to_string_lossy().into_owned(),
            label: entry.label,
            self_dev,
        });
    }
    Ok(binds)
}

/// Timestamped backup matching the `.bak-jcode-*` convention, taken before
/// modifying a user's compositor config. Best-effort.
#[cfg(target_os = "linux")]
fn backup_compositor_config(config_path: &std::path::Path) {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let file_name = config_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config".to_string());
    let backup = config_path.with_file_name(format!("{file_name}.bak-jcode-hotkeys-{ts}"));
    if let Err(err) = std::fs::copy(config_path, &backup) {
        jcode_logging::warn(&format!(
            "failed to back up compositor config before hotkey install: {err}"
        ));
    }
}

/// Ask the compositor to reload its config so new binds take effect without a
/// re-login. niri and Hyprland watch their config files, so only sway/i3 and
/// sxhkd need an explicit poke. Best-effort.
#[cfg(target_os = "linux")]
fn reload_compositor_config(comp: linux_env::LinuxCompositor) {
    let cmd: &[&str] = match comp {
        linux_env::LinuxCompositor::Sway => &["swaymsg", "reload"],
        linux_env::LinuxCompositor::I3 => &["i3-msg", "reload"],
        // sxhkd re-reads its config on SIGUSR1.
        linux_env::LinuxCompositor::Bspwm => &["pkill", "-USR1", "-x", "sxhkd"],
        _ => return,
    };
    match std::process::Command::new(cmd[0])
        .args(&cmd[1..])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(status) if status.success() => {}
        Ok(status) => jcode_logging::warn(&format!(
            "{} exited with {status} while reloading hotkey binds",
            cmd[0]
        )),
        Err(err) => jcode_logging::warn(&format!("failed to run {} reload: {err}", cmd[0])),
    }
}

/// Build the TUI startup notice for the Linux launch hotkeys (or `None` when
/// there is nothing to show). Mirrors the macOS notice but renders Super-style
/// chords and covers every supported compositor (niri, Hyprland/omarchy, sway,
/// i3).
#[cfg(target_os = "linux")]
fn linux_launch_hotkeys_notice(state: &SetupHintsState) -> Option<StartupHints> {
    let comp = detect_linux_compositor()?;
    let config = load_launch_hotkeys_config();
    if config.enabled == Some(false) {
        return None;
    }

    let hotkeys = resolve_linux_hotkeys();
    if hotkeys.is_empty() {
        return None;
    }

    let rows: Vec<LaunchHotkeyRow> = hotkeys
        .iter()
        .filter(|hk| linux_chord_expressible(comp, &hk.chord))
        .map(|hk| LaunchHotkeyRow {
            chord: hk.chord.canonical(),
            display: hk.chord.display_super(),
            label: hk.label.clone(),
            cwd_display: hk.dir.clone(),
            self_dev: hk.self_dev,
        })
        .collect();

    let lines = launch_hotkey_notice_lines(&rows, &state.launch_hotkey_usage, state.launch_count)?;

    // Installation and first launch both configure Linux bindings
    // automatically. If that best-effort setup failed, avoid showing a stale
    // instruction that asks the user to repeat setup manually.
    if !linux_hotkeys_installed(comp) {
        return None;
    }
    let footer = format!(
        "Bound via {} and available system-wide.",
        linux_hotkey_target_description(comp)
    );

    Some(StartupHints::with_status_and_display(
        "Launch hotkeys available".to_string(),
        "Launch hotkeys",
        format!(
            "Configured Jcode launch hotkeys ({}): {} {}",
            comp.name(),
            lines.join("; "),
            footer
        ),
    ))
}

/// One resolved launch hotkey row for the startup notice.
#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
pub(crate) struct LaunchHotkeyRow {
    /// Canonical chord string used as the usage-tracking key (e.g. `cmd+;`).
    pub chord: String,
    /// Pretty, user-facing chord rendering (e.g. `⌘;` or `Super+;`).
    pub display: String,
    pub label: String,
    pub cwd_display: String,
    pub self_dev: bool,
}

/// Decide which launch-hotkey lines to surface, given how often each chord has
/// been used. Pure so the adaptive "stop nagging once learned" policy is
/// unit-tested without touching config or the filesystem.
///
/// Policy:
/// - Hide a per-repo binding once it has been used `LAUNCH_HOTKEY_LEARNED_USES`
///   times (the user has clearly internalized it).
/// - Once the user has learned at least one binding and has launched jcode at
///   least `LAUNCH_HOTKEY_NOTICE_MIN_LAUNCHES_TO_STOP` times, drop the whole
///   notice so it never lingers for an experienced user.
/// - Returns `None` when nothing should be shown.
#[cfg(any(test, target_os = "macos", target_os = "linux", windows))]
pub(crate) fn launch_hotkey_notice_lines(
    rows: &[LaunchHotkeyRow],
    usage: &HashMap<String, u64>,
    launch_count: u64,
) -> Option<Vec<String>> {
    if rows.is_empty() {
        return None;
    }

    let uses_for = |chord: &str| usage.get(chord).copied().unwrap_or(0);
    let learned_any = rows
        .iter()
        .any(|row| uses_for(&row.chord) >= LAUNCH_HOTKEY_LEARNED_USES);
    if learned_any && launch_count >= LAUNCH_HOTKEY_NOTICE_MIN_LAUNCHES_TO_STOP {
        return None;
    }

    let lines: Vec<String> = rows
        .iter()
        .filter(|row| uses_for(&row.chord) < LAUNCH_HOTKEY_LEARNED_USES)
        .map(|row| {
            let suffix = if row.self_dev { " [self-dev]" } else { "" };
            format!(
                "{} → {} ({}){}",
                row.display, row.label, row.cwd_display, suffix
            )
        })
        .collect();

    if lines.is_empty() { None } else { Some(lines) }
}

/// Pure debounce decision for the keybinding-conflict notice.
///
/// Given the freshly-computed conflict `signature` and the `previous` signature
/// we last stored, decide what to do. Separated from I/O so the
/// warn-once-per-change policy can be unit-tested without touching the machine
/// or the filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConflictHintDecision {
    /// Nothing changed since last time; stay silent and leave state untouched.
    Unchanged,
    /// The conflict set changed but is now empty (resolved); update the stored
    /// signature but show nothing.
    ResolvedSilently,
    /// New or changed conflicts; update the stored signature and show a notice.
    Warn,
}

pub(crate) fn conflict_hint_decision(signature: &str, previous: &str) -> ConflictHintDecision {
    if signature == previous {
        ConflictHintDecision::Unchanged
    } else if signature.is_empty() {
        ConflictHintDecision::ResolvedSilently
    } else {
        ConflictHintDecision::Warn
    }
}

/// Check whether jcode's keybindings conflict with shortcuts owned by the
/// terminal or the OS, and return a one-time startup notice when the set of
/// conflicts has changed since we last warned.
///
/// This is config-aware (the caller passes the user's live keybindings) and
/// debounced via a stored signature: a user is warned once per distinct
/// conflict set and never nagged about the same conflicts on subsequent
/// launches. Returns `None` when there are no conflicts, when nothing changed,
/// or when input is not a real TTY.
///
/// The actual diagnostics are always available on demand via the `/keys`
/// command; this only surfaces the proactive heads-up.
pub fn maybe_show_keymap_conflict_hint(
    keybindings: &jcode_config_types::KeybindingsConfig,
) -> Option<StartupHints> {
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return None;
    }

    let snapshot = keymap::snapshot_cached_or_refresh();
    let mut state = SetupHintsState::load();
    let (hint, changed) = keymap_conflict_hint_for(keybindings, &snapshot, &mut state);
    if changed {
        let _ = state.save();
    }
    hint
}

/// Core of [`maybe_show_keymap_conflict_hint`], separated from TTY detection and
/// disk I/O so the full decision + state-update path is unit-testable.
///
/// Returns the optional notice and whether `state` was mutated (and therefore
/// should be persisted by the caller).
pub(crate) fn keymap_conflict_hint_for(
    keybindings: &jcode_config_types::KeybindingsConfig,
    snapshot: &keymap::KeymapSnapshot,
    state: &mut SetupHintsState,
) -> (Option<StartupHints>, bool) {
    let conflicts = keymap::detect_conflicts(keybindings, snapshot);
    let signature = keymap::conflict_signature(&conflicts);

    match conflict_hint_decision(&signature, &state.keymap_conflict_signature) {
        ConflictHintDecision::Unchanged => (None, false),
        ConflictHintDecision::ResolvedSilently => {
            state.keymap_conflict_signature = signature;
            (None, true)
        }
        ConflictHintDecision::Warn => {
            state.keymap_conflict_signature = signature;
            let hint = keymap::render_status_line(keybindings, snapshot).map(|status| {
                let display = keymap::render_report(keybindings, snapshot);
                StartupHints::with_status_and_display(status, "Keybindings", display)
            });
            (hint, true)
        }
    }
}

/// Whether the current terminal triggers jcode's glyph-safe color quantization
/// (macOS VS Code integrated terminal / Apple Terminal). Mirrors the detection
/// in `jcode-tui-style`'s color module and `jcode-app-core::perf` so the
/// disclosure fires exactly when the behavior is active. Overridable with
/// `JCODE_GLYPH_SAFE_MODE=on|off`.
fn glyph_safe_mode_active() -> bool {
    if let Ok(raw) = std::env::var("JCODE_GLYPH_SAFE_MODE") {
        match raw.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => return true,
            "0" | "false" | "no" | "off" => return false,
            _ => {}
        }
    }
    if !cfg!(target_os = "macos") {
        return false;
    }
    match std::env::var("TERM_PROGRAM") {
        Ok(tp) => {
            let tp = tp.to_ascii_lowercase();
            tp == "vscode" || tp == "apple_terminal"
        }
        Err(_) => false,
    }
}

/// One-time disclosure that glyph-safe mode (256-color quantization) is active,
/// shown the first time jcode launches in a fragile-glyph terminal. Discloses
/// the tradeoff (slightly reduced color fidelity) and how to opt out.
pub fn maybe_show_glyph_safe_notice() -> Option<StartupHints> {
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return None;
    }
    let mut state = SetupHintsState::load();
    let (hint, changed) = glyph_safe_notice_for(glyph_safe_mode_active(), &mut state);
    if changed {
        let _ = state.save();
    }
    hint
}

/// Core of [`maybe_show_glyph_safe_notice`], split out for unit testing.
/// Returns the optional notice and whether `state` was mutated.
pub(crate) fn glyph_safe_notice_for(
    active: bool,
    state: &mut SetupHintsState,
) -> (Option<StartupHints>, bool) {
    if !active || state.glyph_safe_notice_shown {
        return (None, false);
    }
    state.glyph_safe_notice_shown = true;
    let status =
        "Glyph-safe mode: colors quantized to 256 to avoid this terminal's glyph corruption."
            .to_string();
    let display = "This terminal (VS Code integrated terminal / Apple Terminal on macOS) corrupts \
its glyph cache under jcode's full-color animations, rendering letters as boxes. \
jcode automatically quantizes colors to the 256-palette here to keep text readable; \
the only tradeoff is slightly reduced color fidelity. Animations still run. \
For full color, use Ghostty, iTerm2, kitty, or WezTerm, or set JCODE_GLYPH_SAFE_MODE=off."
        .to_string();
    (
        Some(StartupHints::with_status_and_display(
            status, "Display", display,
        )),
        true,
    )
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

    #[cfg(windows)]
    {
        let mut state = SetupHintsState::load();
        eprintln!("\x1b[1mjcode setup-launcher\x1b[0m");
        eprintln!();
        match create_windows_desktop_shortcut(&mut state) {
            Ok(()) => {
                eprintln!("  \x1b[32m✓\x1b[0m Created desktop shortcut: jcode.lnk");
                return Ok(());
            }
            Err(e) => {
                eprintln!("  \x1b[31m✗\x1b[0m Failed: {}", e);
                anyhow::bail!("Windows launcher setup failed: {}", e);
            }
        }
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        eprintln!("Launcher setup is currently only supported on macOS and Windows.");
        Ok(())
    }
}

/// Create a desktop shortcut/launcher for jcode.
///
/// - macOS: creates a jcode.app bundle in ~/Applications/
/// - Windows uses [`windows_setup::create_windows_desktop_shortcut`] via
///   `jcode setup-launcher` instead (PowerShell/COM is too slow for the
///   startup path).
#[cfg(any(test, not(windows)))]
fn create_desktop_shortcut(state: &mut SetupHintsState) -> Result<()> {
    #[cfg(any(test, target_os = "macos"))]
    {
        let (app_dir, _terminal) = install_macos_app_launcher()?;

        state.desktop_shortcut_created = true;
        let _ = state.save();

        jcode_logging::info(&format!("Created macOS app bundle: {}", app_dir.display()));
    }

    #[cfg(not(any(test, target_os = "macos")))]
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
    state.launch_hotkey_tracking_version = LAUNCH_HOTKEY_TRACKING_VERSION;
    state.save()?;
    jcode_logging::info(&format!(
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
    state.launch_hotkey_tracking_version = LAUNCH_HOTKEY_TRACKING_VERSION;
    state.save()?;
    jcode_logging::info(&format!(
        "Migrated macOS Cmd+; hotkey listener to v{} for {}",
        HOTKEY_LISTENER_VERSION,
        terminal.label()
    ));
    Ok(())
}

/// Reinstall the launch hotkeys after the `[launch_hotkeys]` config changed
/// (e.g. auto-import baked a per-repo mapping).
///
/// Re-resolves config into platform launch bindings so new chords take effect
/// immediately. An explicit `enabled = false` remains an opt-out. Best-effort:
/// errors are logged, never propagated, so this is safe on the startup path.
pub fn reinstall_launch_hotkeys_after_config_change() {
    #[cfg(target_os = "macos")]
    {
        let state = SetupHintsState::load();
        if !state.hotkey_configured {
            return;
        }
        let preferred = load_preferred_macos_terminal();
        match install_macos_hotkey_listener(preferred) {
            Ok(terminal) => jcode_logging::info(&format!(
                "Reinstalled launch hotkeys after config change for {}",
                terminal.label()
            )),
            Err(err) => jcode_logging::warn(&format!("failed to reinstall launch hotkeys: {err}")),
        }
    }

    #[cfg(windows)]
    {
        windows_setup::reinstall_windows_launch_hotkeys();
    }

    #[cfg(target_os = "linux")]
    {
        let Some(comp) = detect_linux_compositor() else {
            return;
        };
        if load_launch_hotkeys_config().enabled == Some(false) {
            return;
        }
        match install_linux_launch_hotkeys(comp) {
            Ok(true) => jcode_logging::info(&format!(
                "Refreshed {} launch hotkeys after config change",
                comp.name()
            )),
            Ok(false) => {}
            Err(err) => jcode_logging::warn(&format!(
                "failed to refresh {} launch hotkeys: {err}",
                comp.name()
            )),
        }
    }
}

#[cfg(test)]
#[path = "setup_hints_tests.rs"]
mod setup_hints_tests;
