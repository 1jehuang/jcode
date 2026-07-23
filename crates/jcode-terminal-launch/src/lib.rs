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
    if is_set("HANDTERM_SESSION") || is_set("HANDTERM_PID") {
        return Some("handterm".to_string());
    }
    if terminal_env_value(client_terminal_env, "TERM_PROGRAM")
        .is_some_and(|value| value.eq_ignore_ascii_case("handterm"))
    {
        return Some("handterm".to_string());
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

    #[cfg(target_os = "macos")]
    {
        if is_set("GHOSTTY_RESOURCES_DIR") || is_set("GHOSTTY_BIN_DIR") {
            return Some("ghostty".to_string());
        }
        let term_program = terminal_env_value(client_terminal_env, "TERM_PROGRAM")
            .map(|value| value.to_ascii_lowercase());
        return match term_program.as_deref() {
            Some("ghostty") => Some("ghostty".to_string()),
            Some("kitty") => Some("kitty".to_string()),
            Some("wezterm") => Some("wezterm".to_string()),
            Some("alacritty") => Some("alacritty".to_string()),
            Some("iterm.app") | Some("iterm2") => Some("iterm2".to_string()),
            Some("apple_terminal") | Some("terminal") => Some("terminal".to_string()),
            _ => None,
        };
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
    if terminal_env_value(client_terminal_env, "TMUX").is_some()
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
        let configured_terminal = std::env::var("JCODE_TERMINAL").ok();
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
        "handterm" => {
            let shell = shell_command(&command_parts(command));
            cmd.args(["--backend", "gpu", "--exec", &shell]);
        }
        #[cfg(target_os = "macos")]
        "ghostty" => {
            let shell = shell_command(&command_parts(command));
            cmd = Command::new("open");
            cmd.current_dir(cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .args(["-na", "Ghostty", "--args", "-e", "/bin/bash", "-lc"])
                .arg(shell);
            if command.fresh_spawn {
                cmd.env("JCODE_FRESH_SPAWN", "1");
            }
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
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn spawn_metadata_env_reexports_client_terminal_env_with_native_and_client_keys() {
        // A spawn carrying the requesting client's terminal env (#405) should
        // export each var both natively (overriding the spawned process's
        // inherited/stale value) and under a `JCODE_CLIENT_*` alias so hooks can
        // distinguish the client's terminal from the server's.
        let command = TerminalCommand::new("/usr/local/bin/jcode", vec!["--resume".to_string()])
            .kind("swarm-agent")
            .session_id("ses_405")
            .client_terminal_env(vec![
                ("ZELLIJ_SESSION_NAME".to_string(), "sessionB".to_string()),
                ("DISPLAY".to_string(), ":1".to_string()),
            ]);
        let env = spawn_metadata_env(&command, Path::new("/tmp/work"));

        let lookup = |key: &str| {
            env.iter()
                .filter(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
                .next_back()
        };

        assert_eq!(lookup("ZELLIJ_SESSION_NAME").as_deref(), Some("sessionB"));
        assert_eq!(
            lookup("JCODE_CLIENT_ZELLIJ_SESSION_NAME").as_deref(),
            Some("sessionB")
        );
        assert_eq!(lookup("DISPLAY").as_deref(), Some(":1"));
        assert_eq!(lookup("JCODE_CLIENT_DISPLAY").as_deref(), Some(":1"));
        // The first-class spawn metadata still flows through.
        assert_eq!(lookup("JCODE_SPAWN_KIND").as_deref(), Some("swarm-agent"));
    }

    #[test]
    fn spawn_metadata_env_without_client_env_has_no_client_keys() {
        let command = TerminalCommand::new("/usr/local/bin/jcode", vec!["--resume".to_string()])
            .kind("resume")
            .session_id("ses_plain");
        let env = spawn_metadata_env(&command, Path::new("/tmp/work"));
        assert!(
            !env.iter().any(|(k, _)| k.starts_with("JCODE_CLIENT_")),
            "no client terminal env should produce no JCODE_CLIENT_* keys"
        );
    }

    #[test]
    #[cfg(unix)]
    fn snapshot_client_terminal_env_captures_set_vars_only() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::set_var("ZELLIJ_SESSION_NAME", "snapshot-test");
            std::env::remove_var("TMUX");
        }
        let snapshot = snapshot_client_terminal_env();
        assert!(
            snapshot
                .iter()
                .any(|(k, v)| k == "ZELLIJ_SESSION_NAME" && v == "snapshot-test")
        );
        assert!(!snapshot.iter().any(|(k, _)| k == "TMUX"));
        unsafe {
            std::env::remove_var("ZELLIJ_SESSION_NAME");
        }
    }

    #[test]
    #[cfg(unix)]
    fn detected_resume_terminal_recognizes_ghostty_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        unsafe {
            std::env::remove_var("HANDTERM_SESSION");
            std::env::remove_var("HANDTERM_PID");
            std::env::remove_var("KITTY_PID");
            std::env::remove_var("WEZTERM_EXECUTABLE");
            std::env::remove_var("WEZTERM_PANE");
            std::env::remove_var("ALACRITTY_WINDOW_ID");
            std::env::set_var("GHOSTTY_RESOURCES_DIR", "/tmp/ghostty");
        }
        #[cfg(target_os = "macos")]
        assert_eq!(detected_resume_terminal().as_deref(), Some("ghostty"));
        unsafe {
            std::env::remove_var("GHOSTTY_RESOURCES_DIR");
        }
    }

    #[test]
    #[cfg(unix)]
    fn tmux_client_context_is_preferred_over_terminal_emulator() {
        let client_env = vec![
            (
                "TMUX".to_string(),
                "/tmp/tmux-1000/default,123,0".to_string(),
            ),
            ("TMUX_PANE".to_string(), "%42".to_string()),
            ("KITTY_PID".to_string(), "1234".to_string()),
        ];

        let candidates = resume_terminal_candidates_with_client_env(&client_env, None);
        assert_eq!(candidates.first().map(String::as_str), Some("tmux"));
        assert_eq!(candidates.get(1).map(String::as_str), Some("kitty"));
    }

    #[test]
    #[cfg(unix)]
    fn explicit_terminal_override_stays_ahead_of_tmux() {
        let client_env = vec![
            (
                "TMUX".to_string(),
                "/tmp/tmux-1000/default,123,0".to_string(),
            ),
            ("TMUX_PANE".to_string(), "%42".to_string()),
        ];

        let candidates = resume_terminal_candidates_with_client_env(&client_env, Some("wezterm"));
        assert_eq!(candidates.first().map(String::as_str), Some("wezterm"));
        assert_eq!(candidates.get(1).map(String::as_str), Some("tmux"));
    }

    #[test]
    #[cfg(unix)]
    fn authoritative_non_tmux_client_context_ignores_server_tmux() {
        let client_env = vec![("TERM".to_string(), "xterm-256color".to_string())];

        let candidates = resume_terminal_candidates_with_client_env(&client_env, None);
        assert!(!candidates.iter().any(|candidate| candidate == "tmux"));
    }

    #[test]
    #[cfg(unix)]
    fn tmux_context_without_current_pane_uses_emulator_fallbacks() {
        let client_env = vec![
            (
                "TMUX".to_string(),
                "/tmp/tmux-1000/default,123,0".to_string(),
            ),
            ("KITTY_PID".to_string(), "1234".to_string()),
        ];

        let candidates = resume_terminal_candidates_with_client_env(&client_env, None);
        assert_eq!(candidates.first().map(String::as_str), Some("kitty"));
        assert!(!candidates.iter().any(|candidate| candidate == "tmux"));
    }

    #[test]
    #[cfg(unix)]
    fn missing_tmux_binary_falls_back_to_detected_emulator() {
        let _guard = ENV_LOCK.lock().unwrap();
        let previous_terminal = std::env::var_os("JCODE_TERMINAL");
        unsafe {
            std::env::remove_var("JCODE_TERMINAL");
        }
        let command =
            TerminalCommand::new("/usr/local/bin/jcode", vec![]).client_terminal_env(vec![
                (
                    "TMUX".to_string(),
                    "/tmp/tmux-1000/default,123,0".to_string(),
                ),
                ("TMUX_PANE".to_string(), "%42".to_string()),
                ("KITTY_PID".to_string(), "1234".to_string()),
            ]);
        let mut attempts = Vec::new();

        let result =
            spawn_command_in_new_terminal_with(&command, Path::new("/work/dir"), |candidate| {
                let program = candidate.get_program().to_string_lossy().into_owned();
                attempts.push(program.clone());
                if program == "tmux" {
                    Err(std::io::Error::from(std::io::ErrorKind::NotFound))
                } else {
                    Ok(())
                }
            });

        match previous_terminal {
            Some(value) => unsafe { std::env::set_var("JCODE_TERMINAL", value) },
            None => unsafe { std::env::remove_var("JCODE_TERMINAL") },
        }
        assert!(matches!(result, Ok(true)));
        assert_eq!(attempts, vec!["tmux", "kitty"]);
    }

    #[test]
    #[cfg(unix)]
    fn tmux_spawn_opens_right_split_in_requesting_pane() {
        let command = TerminalCommand::new(
            "/usr/local/bin/jcode",
            vec!["--resume".to_string(), "ses_tmux".to_string()],
        )
        .client_terminal_env(vec![
            (
                "TMUX".to_string(),
                "/tmp/tmux-1000/default,123,0".to_string(),
            ),
            ("TMUX_PANE".to_string(), "%42".to_string()),
        ]);

        let cmd = build_spawn_command("tmux", &command, Path::new("/work/dir"))
            .expect("tmux spawn command should build");
        assert_eq!(cmd.get_program().to_string_lossy(), "tmux");
        let args: Vec<String> = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            vec![
                "split-window",
                "-h",
                "-t",
                "%42",
                "-c",
                "/work/dir",
                "/usr/local/bin/jcode",
                "--resume",
                "ses_tmux",
            ]
        );
        assert_eq!(env_value(&cmd, "TMUX_PANE").as_deref(), Some("%42"));
    }

    #[test]
    fn shell_command_quotes_arguments() {
        let shell = shell_command(&["jcode".to_string(), "it's ok".to_string()]);
        #[cfg(unix)]
        assert_eq!(shell, "'jcode' 'it'\"'\"'s ok'");
    }

    #[test]
    #[cfg(not(unix))]
    fn windows_candidates_end_with_cmd_fallback() {
        let candidates = resume_terminal_candidates();
        assert!(candidates.contains(&"alacritty".to_string()));
        assert!(candidates.contains(&"wt".to_string()));
        assert_eq!(candidates.last().map(String::as_str), Some("cmd"));
    }

    #[test]
    #[cfg(not(unix))]
    fn windows_cmd_fallback_runs_jcode_under_cmd_k() {
        let command = TerminalCommand::new(
            std::path::PathBuf::from(r"C:\Program Files\jcode\jcode.exe"),
            vec!["self-dev".to_string()],
        )
        .title("jcode");
        let cmd = build_spawn_command("cmd", &command, Path::new(r"C:\Users\me")).unwrap();
        let args: Vec<String> = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(&args[..5], ["/C", "start", "jcode", "cmd.exe", "/K"]);
        assert!(args[5].contains(r#""C:\Program Files\jcode\jcode.exe""#));
        assert!(args[5].contains("self-dev"));
    }

    #[test]
    #[cfg(unix)]
    fn macos_terminal_inner_script_runs_jcode() {
        let command = TerminalCommand::new(
            std::path::PathBuf::from("/usr/local/bin/jcode"),
            vec!["--resume".to_string(), "abc-123".to_string()],
        );
        let script = macos_terminal_inner_script(&command, Path::new("/work/dir"));
        assert_eq!(
            script,
            "cd '/work/dir' && exec '/usr/local/bin/jcode' '--resume' 'abc-123'"
        );
        // Must actually exec jcode, not the broken `open -a Terminal <file>` form.
        assert!(script.contains("exec '/usr/local/bin/jcode'"));
    }

    #[test]
    #[cfg(unix)]
    fn macos_terminal_inner_script_injects_fresh_spawn() {
        let command =
            TerminalCommand::new(std::path::PathBuf::from("/usr/local/bin/jcode"), vec![])
                .fresh_spawn();
        let script = macos_terminal_inner_script(&command, Path::new("/tmp"));
        assert_eq!(
            script,
            "cd '/tmp' && exec env JCODE_FRESH_SPAWN=1 '/usr/local/bin/jcode'"
        );
    }

    #[test]
    #[cfg(unix)]
    fn macos_terminal_applescript_uses_do_script() {
        let command = TerminalCommand::new(
            std::path::PathBuf::from("/usr/local/bin/jcode"),
            vec!["--resume".to_string(), "abc-123".to_string()],
        );
        let applescript = macos_terminal_applescript(&command, Path::new("/work/dir"));
        assert!(applescript.contains("tell application \"Terminal\""));
        assert!(applescript.contains("do script"));
        // The shell's single quotes survive; AppleScript only escapes \\ and ".
        assert!(!applescript.contains("exec \\\""));
        assert!(applescript.contains("'/usr/local/bin/jcode'"));
    }

    // Reproduction for issue #203 part 3: when no terminal emulator can be
    // spawned, the new-terminal resume path returns Ok(false), which the app
    // surfaces as "No terminal found. Resume manually:".
    #[test]
    fn no_terminal_available_returns_ok_false() {
        let command = TerminalCommand::new(
            std::path::PathBuf::from("/usr/local/bin/jcode"),
            vec!["--resume".to_string(), "abc-123".to_string()],
        );
        let result = spawn_command_in_new_terminal_with(&command, Path::new("/tmp"), |_cmd| {
            // Simulate every candidate terminal being absent.
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        });
        assert!(matches!(result, Ok(false)));
    }

    #[test]
    fn parse_hook_command_splits_words_and_quotes() {
        assert_eq!(
            parse_hook_command("tmux new-window --").unwrap(),
            vec!["tmux", "new-window", "--"]
        );
        assert_eq!(
            parse_hook_command("my-hook --label 'two words'").unwrap(),
            vec!["my-hook", "--label", "two words"]
        );
        assert_eq!(
            parse_hook_command(r#"hook "a \"b\" c""#).unwrap(),
            vec!["hook", r#"a "b" c"#]
        );
    }

    #[test]
    fn parse_hook_command_rejects_bad_input() {
        assert!(parse_hook_command("").is_err());
        assert!(parse_hook_command("   ").is_err());
        assert!(parse_hook_command("hook 'unterminated").is_err());
        assert!(parse_hook_command("hook trailing\\").is_err());
    }

    fn env_value(cmd: &Command, key: &str) -> Option<String> {
        cmd.get_envs().find_map(|(k, v)| {
            (k.to_string_lossy() == key).then(|| {
                v.map(|v| v.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
        })
    }

    #[test]
    fn hook_spawn_command_appends_program_args_and_exports_metadata() {
        let command = TerminalCommand::new(
            std::path::PathBuf::from("/usr/local/bin/jcode"),
            vec!["--resume".to_string(), "ses_abc".to_string()],
        )
        .title("🦊 jcode ses_abc")
        .kind("swarm-agent")
        .session_id("ses_abc")
        .spawn_env("JCODE_SPAWN_SWARM_ID", "swarm-1")
        .fresh_spawn();

        let cmd = build_hook_spawn_command("tmux-hook --flag", &command, Path::new("/work/dir"))
            .expect("hook command should build");

        assert_eq!(cmd.get_program().to_string_lossy(), "tmux-hook");
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            vec!["--flag", "/usr/local/bin/jcode", "--resume", "ses_abc"]
        );
        assert_eq!(
            cmd.get_current_dir(),
            Some(Path::new("/work/dir")),
            "hook should run in the session working dir"
        );

        assert_eq!(
            env_value(&cmd, "JCODE_SPAWN_KIND").as_deref(),
            Some("swarm-agent")
        );
        assert_eq!(
            env_value(&cmd, "JCODE_SPAWN_SESSION_ID").as_deref(),
            Some("ses_abc")
        );
        assert_eq!(
            env_value(&cmd, "JCODE_SPAWN_TITLE").as_deref(),
            Some("🦊 jcode ses_abc")
        );
        assert_eq!(
            env_value(&cmd, "JCODE_SPAWN_CWD").as_deref(),
            Some("/work/dir")
        );
        assert_eq!(
            env_value(&cmd, "JCODE_SPAWN_PROGRAM").as_deref(),
            Some("/usr/local/bin/jcode")
        );
        #[cfg(unix)]
        assert_eq!(
            env_value(&cmd, "JCODE_SPAWN_COMMAND").as_deref(),
            Some("'/usr/local/bin/jcode' '--resume' 'ses_abc'")
        );
        assert_eq!(
            env_value(&cmd, "JCODE_SPAWN_SWARM_ID").as_deref(),
            Some("swarm-1")
        );
        assert_eq!(env_value(&cmd, "JCODE_FRESH_SPAWN").as_deref(), Some("1"));
    }

    #[test]
    #[cfg(all(unix, not(target_os = "macos")))]
    fn builtin_terminal_spawn_exports_metadata_env() {
        let command = TerminalCommand::new(
            std::path::PathBuf::from("/usr/local/bin/jcode"),
            vec!["--resume".to_string(), "ses_abc".to_string()],
        )
        .kind("resume")
        .session_id("ses_abc");

        let cmd = build_spawn_command("kitty", &command, Path::new("/work/dir"))
            .expect("kitty spawn command should build");
        assert_eq!(
            env_value(&cmd, "JCODE_SPAWN_KIND").as_deref(),
            Some("resume")
        );
        assert_eq!(
            env_value(&cmd, "JCODE_SPAWN_SESSION_ID").as_deref(),
            Some("ses_abc")
        );
    }
}
