use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Debug)]
pub struct TerminalCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub title: Option<String>,
    pub fresh_spawn: bool,
    /// What this spawn is for (e.g. "resume", "selfdev", "swarm-agent").
    /// Exported as `JCODE_SPAWN_KIND` to spawn hooks and spawned terminals.
    pub kind: Option<String>,
    /// The jcode session this terminal will run, when known.
    /// Exported as `JCODE_SPAWN_SESSION_ID`.
    pub session_id: Option<String>,
    /// Extra metadata env entries (e.g. `JCODE_SPAWN_SWARM_ID`) exported to
    /// spawn hooks and spawned terminals. Applied after the first-class
    /// `JCODE_SPAWN_*` keys, so entries here win on key collisions.
    pub extra_env: Vec<(String, String)>,
    /// Terminal-identifying env vars captured from the *client* that requested
    /// this spawn (see [`snapshot_client_terminal_env`]). When set, these
    /// override the (possibly stale) server-inherited values for the same keys
    /// and are also exported under a `JCODE_CLIENT_*` prefix so spawn/focus
    /// hooks can target the terminal the user is actually attached to (#405).
    pub client_terminal_env: Vec<(String, String)>,
    /// The user's selected terminal (e.g. the macOS Cmd+; listener's saved
    /// preference). Tried first, after an explicit `JCODE_TERMINAL`.
    pub preferred_terminal: Option<String>,
}

impl TerminalCommand {
    pub fn new(program: impl Into<PathBuf>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
            title: None,
            fresh_spawn: false,
            kind: None,
            session_id: None,
            extra_env: Vec::new(),
            client_terminal_env: Vec::new(),
            preferred_terminal: None,
        }
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn fresh_spawn(mut self) -> Self {
        self.fresh_spawn = true;
        self
    }

    pub fn kind(mut self, kind: impl Into<String>) -> Self {
        self.kind = Some(kind.into());
        self
    }

    pub fn session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn spawn_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_env.push((key.into(), value.into()));
        self
    }

    /// Attach the client's terminal-identifying env snapshot (see
    /// [`snapshot_client_terminal_env`]) so the spawn follows the terminal the
    /// requesting client is attached to instead of the server's stale env.
    pub fn client_terminal_env(mut self, env: Vec<(String, String)>) -> Self {
        self.client_terminal_env = env;
        self
    }

    pub fn preferred_terminal(mut self, term: impl Into<String>) -> Self {
        self.preferred_terminal = Some(term.into());
        self
    }
}

/// Terminal/window-manager environment variables that identify *which*
/// terminal, multiplexer, or display a client is attached to.
///
/// The jcode server process is long-lived and captures these at *its* startup,
/// so once a client connects from a different terminal/tmux/zellij session the
/// server's copies are stale. Spawn and focus hooks executed by the server then
/// target the wrong terminal (see issue #405). To fix this, clients snapshot
/// these vars from their own environment and send them to the server, which
/// re-exports them to spawn/focus hooks so the hook places windows in the
/// terminal the user is actually looking at.
///
/// This intentionally covers terminal multiplexers (tmux, screen, zellij),
/// terminal emulators (kitty, wezterm, ghostty, iTerm, ...), and the display
/// server (X11 `DISPLAY`, Wayland `WAYLAND_DISPLAY`) so window placement and
/// routing all follow the connecting client.
pub const CLIENT_TERMINAL_ENV_VARS: &[&str] = &[
    // Terminal multiplexers
    "ZELLIJ",
    "ZELLIJ_SESSION_NAME",
    "ZELLIJ_PANE_ID",
    "TMUX",
    "TMUX_PANE",
    "STY",
    // herdr terminal multiplexer (https://herdr.dev), see issue #405
    "HERDR_ENV",
    "HERDR_SOCKET_PATH",
    "HERDR_PANE_ID",
    "HERDR_TAB_ID",
    "HERDR_WORKSPACE_ID",
    "HERDR_BIN_PATH",
    "HERDR_SESSION",
    "HERDR_AGENT",
    // Terminal emulators
    "TERM",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "COLORTERM",
    "KITTY_PID",
    "KITTY_WINDOW_ID",
    "KITTY_LISTEN_ON",
    "WEZTERM_PANE",
    "WEZTERM_EXECUTABLE",
    "WEZTERM_UNIX_SOCKET",
    "ALACRITTY_WINDOW_ID",
    "ALACRITTY_SOCKET",
    "GHOSTTY_RESOURCES_DIR",
    "GHOSTTY_BIN_DIR",
    "ITERM_SESSION_ID",
    "WINDOWID",
    "HANDTERM_SESSION",
    "HANDTERM_PID",
    "WT_SESSION",
    "WT_PROFILE_ID",
    // Display / window manager
    "DISPLAY",
    "WAYLAND_DISPLAY",
];

/// Snapshot the current process's terminal-identifying env vars (see
/// [`CLIENT_TERMINAL_ENV_VARS`]). Only vars that are actually set are included,
/// so the map is empty when nothing identifies the terminal.
pub fn snapshot_client_terminal_env() -> Vec<(String, String)> {
    CLIENT_TERMINAL_ENV_VARS
        .iter()
        .filter_map(|&key| {
            std::env::var(key)
                .ok()
                .map(|value| (key.to_string(), value))
        })
        .collect()
}

/// Replace inherited terminal identity with an authoritative client snapshot.
///
/// Removing every known key first is important for a shared server: an empty
/// client snapshot must not leak the pane that happened to start the server.
/// Aliases let integrations explicitly distinguish client values from other
/// process environment while native names preserve existing hook behavior.
pub fn apply_client_terminal_env(cmd: &mut Command, env: &[(String, String)]) {
    for key in CLIENT_TERMINAL_ENV_VARS {
        cmd.env_remove(key);
        cmd.env_remove(format!("JCODE_CLIENT_{key}"));
    }
    for (key, value) in env {
        if CLIENT_TERMINAL_ENV_VARS.contains(&key.as_str()) {
            cmd.env(key, value);
            cmd.env(format!("JCODE_CLIENT_{key}"), value);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnAttempt {
    pub terminal: String,
    pub program: String,
    pub args: Vec<String>,
}

pub fn sh_escape(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\"'\"'"))
}

pub fn shell_command(args: &[String]) -> String {
    #[cfg(unix)]
    {
        args.iter()
            .map(|arg| sh_escape(arg))
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[cfg(not(unix))]
    {
        args.join(" ")
    }
}

fn push_unique_terminal(candidates: &mut Vec<String>, term: impl Into<String>) {
    let term = term.into();
    if term.trim().is_empty() {
        return;
    }
    if !candidates.iter().any(|candidate| candidate == &term) {
        candidates.push(term);
    }
}

#[cfg(unix)]
fn terminal_env_value(client_terminal_env: &[(String, String)], key: &str) -> Option<String> {
    if client_terminal_env.is_empty() {
        return std::env::var(key).ok().filter(|value| !value.is_empty());
    }

    client_terminal_env
        .iter()
        .find(|(candidate, _)| candidate == key)
        .map(|(_, value)| value.clone())
        .filter(|value| !value.is_empty())
}

#[cfg(unix)]
fn detected_resume_terminal_with_client_env(
    client_terminal_env: &[(String, String)],
) -> Option<String> {
    let is_set = |key| terminal_env_value(client_terminal_env, key).is_some();
    // Herdr is a terminal multiplexer, so headed sessions should remain in the
    // workspace that requested them instead of escaping into the underlying
    // emulator. Prefer it even when the pane also advertises kitty/wezterm.
    if is_set("HERDR_ENV") && is_set("HERDR_PANE_ID") {
        return Some("herdr".to_string());
    }
    if is_set("HANDTERM_SESSION") || is_set("HANDTERM_PID") {
        return Some("handterm".to_string());
    }
    if terminal_env_value(client_terminal_env, "TERM_PROGRAM")
        .is_some_and(|value| value.eq_ignore_ascii_case("handterm"))
    {
        return Some("handterm".to_string());
    }
    if is_set("ZELLIJ") && (is_set("ZELLIJ_PANE_ID") || is_set("ZELLIJ_SESSION_NAME")) {
        return Some("zellij".to_string());
    }
    if is_set("STY") {
        return Some("screen".to_string());
    }
    if is_set("KITTY_PID") {
        return Some("kitty".to_string());
    }
    if is_set("WEZTERM_EXECUTABLE") || is_set("WEZTERM_PANE") {
        return Some("wezterm".to_string());
    }
    if is_set("ALACRITTY_WINDOW_ID") {
        return Some("alacritty".to_string());
    }
    if is_set("GHOSTTY_RESOURCES_DIR") || is_set("GHOSTTY_BIN_DIR") {
        return Some("ghostty".to_string());
    }

    let term_program = terminal_env_value(client_terminal_env, "TERM_PROGRAM")
        .map(|value| value.to_ascii_lowercase());
    if let Some(term) = match term_program.as_deref() {
        Some("ghostty") => Some("ghostty"),
        Some("kitty") => Some("kitty"),
        Some("wezterm") => Some("wezterm"),
        Some("alacritty") => Some("alacritty"),
        _ => None,
    } {
        return Some(term.to_string());
    }

    #[cfg(target_os = "macos")]
    {
        match term_program.as_deref() {
            Some("iterm.app") | Some("iterm2") => Some("iterm2".to_string()),
            Some("apple_terminal") | Some("terminal") => Some("terminal".to_string()),
            _ => None,
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[cfg(target_os = "macos")]
fn macos_app_installed(app_name: &str) -> bool {
    let system_app = Path::new("/Applications").join(app_name);
    if system_app.is_dir() {
        return true;
    }
    if let Some(home) = dirs::home_dir()
        && home.join("Applications").join(app_name).is_dir()
    {
        return true;
    }
    false
}

#[cfg(target_os = "macos")]
fn macos_current_terminal_is(term: &str) -> bool {
    detected_resume_terminal().as_deref() == Some(term)
}

#[cfg(target_os = "macos")]
fn macos_should_try_app_terminal(term: &str) -> bool {
    match term {
        "ghostty" => macos_current_terminal_is("ghostty") || macos_app_installed("Ghostty.app"),
        "kitty" => {
            macos_current_terminal_is("kitty")
                || macos_app_installed("kitty.app")
                || macos_app_installed("Kitty.app")
        }
        "wezterm" => {
            macos_current_terminal_is("wezterm")
                || macos_app_installed("WezTerm.app")
                || macos_app_installed("wezterm.app")
        }
        "alacritty" => {
            macos_current_terminal_is("alacritty") || macos_app_installed("Alacritty.app")
        }
        "iterm2" => {
            macos_current_terminal_is("iterm2")
                || macos_app_installed("iTerm.app")
                || macos_app_installed("iTerm2.app")
        }
        // Apple Terminal ships with every macOS install, so it is the guaranteed
        // last-resort fallback and is always worth trying.
        "terminal" => true,
        _ => true,
    }
}

/// Ordered macOS terminal preference list used when spawning a new window.
///
/// Earlier entries are preferred. Apple's built-in `Terminal.app` is intentionally
/// last because it is the guaranteed fallback that exists on every macOS install,
/// while the modern terminals above it are only attempted when actually
/// installed (or currently in use). See `macos_should_try_app_terminal`.
#[cfg(target_os = "macos")]
const MACOS_TERMINAL_PREFERENCE: &[&str] = &[
    "ghostty",
    "kitty",
    "wezterm",
    "alacritty",
    "iterm2",
    "terminal",
];

#[cfg(unix)]
pub fn detected_resume_terminal() -> Option<String> {
    detected_resume_terminal_with_client_env(&[])
}

#[cfg(not(unix))]
pub fn detected_resume_terminal() -> Option<String> {
    if std::env::var("WT_SESSION").is_ok() {
        return Some("wt".to_string());
    }
    if std::env::var("WEZTERM_EXECUTABLE").is_ok() || std::env::var("WEZTERM_PANE").is_ok() {
        return Some("wezterm".to_string());
    }
    if std::env::var("ALACRITTY_WINDOW_ID").is_ok() {
        return Some("alacritty".to_string());
    }
    None
}

#[cfg(unix)]
fn resume_terminal_candidates_with_client_env(
    client_terminal_env: &[(String, String)],
    configured_terminal: Option<&str>,
) -> Vec<String> {
    let mut candidates = Vec::new();
    if let Some(term) = configured_terminal {
        push_unique_terminal(&mut candidates, term);
    }

    // A tmux client already owns the user's terminal layout. Prefer a pane in
    // that exact client over opening another emulator window. Explicit
    // JCODE_TERMINAL and configured spawn hooks still take precedence.
    let in_herdr = terminal_env_value(client_terminal_env, "HERDR_ENV").is_some()
        && terminal_env_value(client_terminal_env, "HERDR_PANE_ID").is_some();
    let in_inner_multiplexer = in_herdr
        || terminal_env_value(client_terminal_env, "ZELLIJ").is_some()
        || terminal_env_value(client_terminal_env, "STY").is_some();
    if !in_inner_multiplexer
        && terminal_env_value(client_terminal_env, "TMUX").is_some()
        && terminal_env_value(client_terminal_env, "TMUX_PANE").is_some()
    {
        push_unique_terminal(&mut candidates, "tmux");
    }
    if let Some(term) = detected_resume_terminal_with_client_env(client_terminal_env) {
        push_unique_terminal(&mut candidates, term);
    }

    #[cfg(target_os = "macos")]
    {
        for &term in MACOS_TERMINAL_PREFERENCE {
            if macos_should_try_app_terminal(term) {
                push_unique_terminal(&mut candidates, term);
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        for term in [
            "handterm",
            "ghostty",
            "kitty",
            "wezterm",
            "alacritty",
            "gnome-terminal",
            "konsole",
            "xterm",
            "foot",
        ] {
            push_unique_terminal(&mut candidates, term);
        }
    }

    candidates
}

#[cfg(unix)]
pub fn resume_terminal_candidates() -> Vec<String> {
    let configured_terminal = std::env::var("JCODE_TERMINAL").ok();
    resume_terminal_candidates_with_client_env(&[], configured_terminal.as_deref())
}

#[cfg(not(unix))]
pub fn resume_terminal_candidates() -> Vec<String> {
    let mut candidates = Vec::new();
    if let Ok(term) = std::env::var("JCODE_TERMINAL") {
        push_unique_terminal(&mut candidates, term);
    }
    if let Some(term) = detected_resume_terminal() {
        push_unique_terminal(&mut candidates, term);
    }
    for term in ["alacritty", "wt", "wezterm", "cmd"] {
        push_unique_terminal(&mut candidates, term);
    }
    candidates
}

pub fn spawn_command_in_new_terminal_with(
    command: &TerminalCommand,
    cwd: &Path,
    mut spawn_detached: impl FnMut(&mut Command) -> std::io::Result<()>,
) -> Result<bool> {
    let mut last_spawn_error: Option<std::io::Error> = None;

    #[cfg(unix)]
    let candidates = {
        let configured_terminal = std::env::var("JCODE_TERMINAL")
            .ok()
            .or_else(|| command.preferred_terminal.clone());
        resume_terminal_candidates_with_client_env(
            &command.client_terminal_env,
            configured_terminal.as_deref(),
        )
    };
    #[cfg(not(unix))]
    let candidates = resume_terminal_candidates();

    for term in candidates {
        let Some(mut cmd) = build_spawn_command(&term, command, cwd) else {
            continue;
        };

        match spawn_detached(&mut cmd) {
            Ok(_) => return Ok(true),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => last_spawn_error = Some(err),
        }
    }

    if let Some(err) = last_spawn_error {
        Err(err.into())
    } else {
        Ok(false)
    }
}

/// Parse an external spawn-hook command line into argv parts.
///
/// Supports basic POSIX-style word splitting: whitespace separates arguments,
/// single and double quotes group words, and backslash escapes the next
/// character (outside single quotes). Errors on empty input, unterminated
/// quotes, and trailing escapes.
pub fn parse_hook_command(raw: &str) -> Result<Vec<String>> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut token_started = false;

    for ch in raw.chars() {
        if escaped {
            current.push(ch);
            token_started = true;
            escaped = false;
            continue;
        }

        if let Some(quote_ch) = quote {
            if ch == quote_ch {
                quote = None;
            } else if ch == '\\' && quote_ch == '"' {
                escaped = true;
            } else {
                current.push(ch);
                token_started = true;
            }
            continue;
        }

        match ch {
            '\\' => {
                escaped = true;
                token_started = true;
            }
            '\'' | '"' => {
                quote = Some(ch);
                token_started = true;
            }
            ch if ch.is_whitespace() => {
                if token_started {
                    parts.push(std::mem::take(&mut current));
                    token_started = false;
                }
            }
            ch => {
                current.push(ch);
                token_started = true;
            }
        }
    }

    if escaped {
        anyhow::bail!("spawn hook command ends with an escape character");
    }
    if quote.is_some() {
        anyhow::bail!("spawn hook command has an unterminated quote");
    }
    if token_started {
        parts.push(current);
    }
    if parts.is_empty() {
        anyhow::bail!("spawn hook command is empty");
    }

    Ok(parts)
}

/// Expand a leading `~/` in a hook program path to the user's home directory,
/// since the hook is executed directly (no shell) and would otherwise fail.
pub fn expand_home(program: &str) -> PathBuf {
    if let Some(rest) = program.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(program)
}

/// The `JCODE_SPAWN_*` metadata env exported to spawn hooks and to terminals
/// launched by the built-in fallback:
///
/// - `JCODE_SPAWN_KIND`: why this spawn happened ("resume", "selfdev",
///   "swarm-agent", ...), when known.
/// - `JCODE_SPAWN_SESSION_ID`: the jcode session the window will run.
/// - `JCODE_SPAWN_TITLE`: the suggested window/tab title.
/// - `JCODE_SPAWN_CWD`: the working directory for the session.
/// - `JCODE_SPAWN_PROGRAM`: path of the jcode binary to execute.
/// - `JCODE_SPAWN_COMMAND`: the full command line, shell-escaped, for hooks
///   (like tmux) that take a single shell-command string.
///
/// `TerminalCommand::extra_env` entries (e.g. `JCODE_SPAWN_SWARM_ID`,
/// `JCODE_SPAWN_COORDINATOR_SESSION_ID`) are appended last and win collisions.
fn spawn_metadata_env(command: &TerminalCommand, cwd: &Path) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = Vec::new();
    if let Some(kind) = &command.kind {
        env.push(("JCODE_SPAWN_KIND".to_string(), kind.clone()));
    }
    if let Some(session_id) = &command.session_id {
        env.push(("JCODE_SPAWN_SESSION_ID".to_string(), session_id.clone()));
    }
    if let Some(title) = &command.title {
        env.push(("JCODE_SPAWN_TITLE".to_string(), title.clone()));
    }
    env.push((
        "JCODE_SPAWN_CWD".to_string(),
        cwd.to_string_lossy().into_owned(),
    ));
    env.push((
        "JCODE_SPAWN_PROGRAM".to_string(),
        command.program.to_string_lossy().into_owned(),
    ));
    env.push((
        "JCODE_SPAWN_COMMAND".to_string(),
        shell_command(&command_parts(command)),
    ));
    // Re-export the requesting client's terminal env so spawn/focus hooks use
    // the client's terminal, not the server's stale startup env (#405). Each
    // var is exported both under its native name (overriding the inherited
    // value the spawned process/hook would otherwise see) and under a
    // `JCODE_CLIENT_<NAME>` alias so hooks can explicitly distinguish the
    // client's terminal from the server's.
    for (key, value) in &command.client_terminal_env {
        env.push((key.clone(), value.clone()));
        env.push((format!("JCODE_CLIENT_{key}"), value.clone()));
    }
    env.extend(command.extra_env.iter().cloned());
    env
}

/// Build the process invocation for an external spawn hook.
///
/// The hook command is parsed shell-style, then the target program and its
/// arguments are appended as additional argv entries (the `$TERMINAL -e`
/// convention), so `hook --flag` becomes `hook --flag <jcode> <args...>`.
/// The hook runs in the session working directory with the full
/// `JCODE_SPAWN_*` metadata env set (see [`spawn_metadata_env`]); hooks that
/// need a single shell-command string (tmux, kitty `@ launch`) can use
/// `$JCODE_SPAWN_COMMAND` instead of the appended argv.
pub fn build_hook_spawn_command(
    hook: &str,
    command: &TerminalCommand,
    cwd: &Path,
) -> Result<Command> {
    let parts = parse_hook_command(hook)?;
    let (program, prefix_args) = parts
        .split_first()
        .expect("parse_hook_command guarantees at least one part");

    let mut cmd = Command::new(expand_home(program));
    cmd.args(prefix_args)
        .arg(&command.program)
        .args(&command.args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if command.fresh_spawn {
        cmd.env("JCODE_FRESH_SPAWN", "1");
    }
    for (key, value) in spawn_metadata_env(command, cwd) {
        cmd.env(key, value);
    }
    Ok(cmd)
}

fn build_spawn_command(term: &str, command: &TerminalCommand, cwd: &Path) -> Option<Command> {
    let title = command.title.as_deref().unwrap_or("jcode");
    let mut cmd = Command::new(term);
    cmd.current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if command.fresh_spawn {
        cmd.env("JCODE_FRESH_SPAWN", "1");
    }

    match term {
        #[cfg(unix)]
        "herdr" => {
            // `pane split` deliberately creates a shell and returns its pane id;
            // `pane run` is the atomic, bracketed-paste-aware way to start a
            // command in that shell. Keep the small composition here rather
            // than requiring every user to configure an equivalent spawn hook.
            let herdr = terminal_env_value(&command.client_terminal_env, "HERDR_BIN_PATH")
                .unwrap_or_else(|| "herdr".to_string());
            // Unlike tmux/zellij, herdr runs the command inside an interactive
            // shell, which would outlive jcode and leave an empty pane behind
            // (e.g. after `swarm stop`). Exit that shell when jcode exits
            // cleanly so the pane closes, but keep it open after a failure so
            // the error stays readable.
            let shell = format!("{} && exit", shell_command(&command_parts(command)));
            let script = concat!(
                "set -eu; ",
                "response=\"$(\"$1\" pane split --current --direction right --cwd \"$2\" --focus)\"; ",
                "pane_id=\"$(printf '%s\\n' \"$response\" | sed -n '",
                "s/.*\\\"pane_id\\\"[[:space:]]*:[[:space:]]*\\\"\\([^\\\"]*\\)\\\".*/\\1/p' | head -n 1)\"; ",
                "test -n \"$pane_id\"; ",
                "exec \"$1\" pane run \"$pane_id\" \"$3\""
            );
            cmd = Command::new("sh");
            cmd.current_dir(cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .args(["-c", script, "jcode-herdr-spawn", &herdr])
                .arg(cwd)
                .arg(shell);
            if command.fresh_spawn {
                cmd.env("JCODE_FRESH_SPAWN", "1");
            }
        }
        #[cfg(unix)]
        "tmux" => {
            cmd.args(["split-window", "-h"]);
            if let Some(pane) = terminal_env_value(&command.client_terminal_env, "TMUX_PANE") {
                cmd.args(["-t", &pane]);
            }
            cmd.arg("-c")
                .arg(cwd)
                .arg(&command.program)
                .args(&command.args);
        }
        #[cfg(unix)]
        "zellij" => {
            cmd.args(["action", "new-pane", "--direction", "right", "--cwd"])
                .arg(cwd)
                .arg("--")
                .arg(&command.program)
                .args(&command.args);
        }
        #[cfg(unix)]
        "screen" => {
            let inner = format!(
                "cd {} && exec {}",
                sh_escape(&cwd.to_string_lossy()),
                shell_command(&command_parts(command))
            );
            if let Some(sty) = terminal_env_value(&command.client_terminal_env, "STY") {
                cmd.args(["-S", &sty]);
            }
            cmd.args(["-X", "screen", "sh", "-lc"]).arg(inner);
        }
        #[cfg(unix)]
        "handterm" => {
            let shell = shell_command(&command_parts(command));
            cmd.args(["--backend", "gpu", "--exec", &shell]);
        }
        #[cfg(target_os = "macos")]
        "ghostty" => {
            // Ghostty 1.3+ exposes a native AppleScript API. Use it instead of
            // `open -na`, which always creates a separate app instance/window
            // and therefore made `/fork` ignore Ghostty's tabbed workflow.
            // Run AppleScript synchronously inside a detached helper so errors
            // (Ghostty <1.3, disabled AppleScript, denied Automation access)
            // can fall back to the older, universally supported new-window
            // launch. Spawning `osascript` directly reports success before the
            // script runs and would silently lose the fork on those versions.
            cmd = Command::new("sh");
            cmd.current_dir(cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .args([
                    "-c",
                    macos_ghostty_spawn_wrapper(),
                    "jcode-ghostty-spawn",
                    &macos_ghostty_applescript(command, cwd),
                    &macos_terminal_inner_script(command, cwd),
                ]);
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        "ghostty" => {
            cmd.arg(format!("--working-directory={}", cwd.to_string_lossy()))
                .arg("-e")
                .arg(&command.program)
                .args(&command.args);
        }
        // launchd's PATH rarely includes kitty, so app installs take the `open`
        // route. Spawning `open` succeeds even when kitty.app is gone (only the
        // spawn is checked, not its exit), so a stale kitty preference skips to
        // the next candidate instead. Installs outside the standard Applications
        // folders are not detected.
        #[cfg(target_os = "macos")]
        "kitty" if !program_on_path("kitty") => {
            if !macos_should_try_app_terminal("kitty") {
                return None;
            }
            cmd = macos_kitty_open_command(command, cwd);
        }
        "kitty" => {
            cmd.args(["--title", title, "-e"])
                .arg(&command.program)
                .args(&command.args);
        }
        "wezterm" => {
            cmd.args([
                "start",
                "--always-new-process",
                "--",
                command.program.to_string_lossy().as_ref(),
            ]);
            cmd.args(&command.args);
        }
        "alacritty" => {
            cmd.args(["--title", title, "-e"])
                .arg(&command.program)
                .args(&command.args);
        }
        "gnome-terminal" => {
            cmd.arg("--title").arg(title);
            cmd.arg("--").arg(&command.program).args(&command.args);
        }
        "konsole" | "xterm" | "foot" => {
            cmd.args(["-e"]).arg(&command.program).args(&command.args);
        }
        #[cfg(target_os = "macos")]
        "iterm2" => {
            let shell = shell_command(&command_parts(command));
            cmd = Command::new("osascript");
            cmd.args([
                "-e",
                &format!(
                    r#"tell application "iTerm2"
                        create window with default profile command "{}"
                    end tell"#,
                    shell.replace('"', "\\\"")
                ),
            ]);
        }
        #[cfg(target_os = "macos")]
        "terminal" => {
            // `open -a Terminal <binary> --args ...` does NOT execute the binary with
            // arguments; it asks Terminal to open the file as a document. On a default
            // macOS install (where Apple Terminal is the only available terminal), that
            // means split/resume spawns silently fail to launch jcode. Use AppleScript's
            // `do script` so the command actually runs in a new Terminal window.
            cmd = Command::new("osascript");
            cmd.args(["-e", &macos_terminal_applescript(command, cwd)]);
        }
        #[cfg(not(unix))]
        "wt" => {
            cmd.args(["new-tab", "--title", title]);
            cmd.arg(&command.program).args(&command.args);
        }
        #[cfg(not(unix))]
        "cmd" => {
            cmd.args(["/C", "start", title, "cmd.exe", "/K"]);
            cmd.arg(windows_command_line(&command_parts(command)));
        }
        _ => return None,
    }

    // Export spawn metadata to the terminal process so programs running
    // inside (shells, multiplexers) can also see why the window was opened.
    // Note: terminals launched indirectly (macOS `open`/`osascript` paths) do
    // not inherit this env, matching the existing JCODE_FRESH_SPAWN caveat.
    for (key, value) in spawn_metadata_env(command, cwd) {
        cmd.env(key, value);
    }

    Some(cmd)
}

#[cfg(any(not(unix), test))]
fn windows_arg_quote(arg: &str) -> String {
    if arg.is_empty()
        || arg
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '"' | '&' | '|' | '<' | '>' | '^'))
    {
        format!("\"{}\"", arg.replace('"', "\\\""))
    } else {
        arg.to_string()
    }
}

#[cfg(any(not(unix), test))]
fn windows_command_line(args: &[String]) -> String {
    args.iter()
        .map(|arg| windows_arg_quote(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn command_parts(command: &TerminalCommand) -> Vec<String> {
    std::iter::once(command.program.to_string_lossy().into_owned())
        .chain(command.args.iter().cloned())
        .collect()
}

/// Build the inner `/bin/sh` script that Apple Terminal's `do script` will run.
///
/// `do script` always executes in a login shell, so we `cd` into the working
/// directory and `exec` the target command (optionally injecting the fresh-spawn
/// env var, which would otherwise be lost because the spawned shell does not
/// inherit the env of the `osascript` process).
#[cfg(any(target_os = "macos", test))]
fn macos_terminal_inner_script(command: &TerminalCommand, cwd: &Path) -> String {
    let shell = shell_command(&command_parts(command));
    format!(
        "cd {} && exec {}{}",
        sh_escape(&cwd.to_string_lossy()),
        if command.fresh_spawn {
            "env JCODE_FRESH_SPAWN=1 "
        } else {
            ""
        },
        shell
    )
}

#[cfg(target_os = "macos")]
fn program_on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

/// Launch kitty's app bundle through LaunchServices, matching the setup
/// scripts' `open -na kitty --args ...` (kitty takes the program positionally).
/// `open` does not pass our env or cwd, so the inner script carries both.
#[cfg(any(target_os = "macos", test))]
fn macos_kitty_open_command(command: &TerminalCommand, cwd: &Path) -> Command {
    let title = command.title.as_deref().unwrap_or("jcode");
    let mut cmd = Command::new("open");
    cmd.current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .args([
            "-na",
            "kitty",
            "--args",
            "--title",
            title,
            "/bin/bash",
            "-lc",
        ])
        .arg(macos_terminal_inner_script(command, cwd));
    cmd
}

/// Build the AppleScript used to create a Ghostty tab on macOS.
///
/// A running Ghostty window is expected for session forks, but the no-window
/// branch also makes Ghostty a safe configured/default launcher. The command is
/// wrapped in a login shell because Ghostty's surface `command` is an executable
/// command line, while our inner script contains `cd`, `exec`, and env setup.
#[cfg(any(target_os = "macos", test))]
fn macos_ghostty_applescript(command: &TerminalCommand, cwd: &Path) -> String {
    let inner = macos_terminal_inner_script(command, cwd);
    let launch = format!("/bin/bash -lc {}", sh_escape(&inner));
    let escaped_launch = launch.replace('\\', "\\\\").replace('"', "\\\"");
    let escaped_cwd = cwd
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");

    format!(
        "tell application \"Ghostty\"\n\
         \x20   set cfg to new surface configuration\n\
         \x20   set initial working directory of cfg to \"{escaped_cwd}\"\n\
         \x20   set command of cfg to \"{escaped_launch}\"\n\
         \x20   if (count of windows) > 0 then\n\
         \x20       set createdTab to new tab in front window with configuration cfg\n\
         \x20       select tab createdTab\n\
         \x20   else\n\
         \x20       new window with configuration cfg\n\
         \x20   end if\n\
         \x20   activate\n\
         end tell"
    )
}

/// Shell wrapper for the macOS Ghostty launch. Positional arguments keep both
/// AppleScript and shell commands opaque, including quotes and spaces.
#[cfg(any(target_os = "macos", test))]
fn macos_ghostty_spawn_wrapper() -> &'static str {
    concat!(
        "if osascript -e \"$1\"; then exit 0; fi; ",
        "exec open -na Ghostty --args -e /bin/bash -lc \"$2\""
    )
}

/// Build the full AppleScript passed to `osascript -e` for Apple Terminal.
#[cfg(any(target_os = "macos", test))]
fn macos_terminal_applescript(command: &TerminalCommand, cwd: &Path) -> String {
    let inner = macos_terminal_inner_script(command, cwd);
    // AppleScript string literals are double-quoted, so backslashes and double
    // quotes from the shell script must be escaped (backslashes first).
    let escaped = inner.replace('\\', "\\\\").replace('"', "\\\"");
    format!("tell application \"Terminal\"\n    activate\n    do script \"{escaped}\"\nend tell")
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "windows_portable_tests.rs"]
mod windows_portable_tests;
