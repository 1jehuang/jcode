//! Launching jcode sessions in new terminal windows.
//!
//! These helpers spawn a fresh `jcode` process (resume or self-dev) inside a
//! new terminal window. They are pure process/terminal orchestration built on
//! the low-level `terminal_launch` facade and depend only on core modules
//! (`id`, `process_title`, `platform`), so
//! they live in the core layer rather than the CLI command layer. This lets
//! lower layers like `server`, `restart_snapshot`, and `tool` relaunch
//! sessions without depending on `cli`.

use anyhow::Result;

use crate::{id, server};

/// Map a persisted session/runtime provider key (e.g. `anthropic-api-key`,
/// `claude-oauth`) to the value the resumed process accepts for `--provider`
/// (the CLI `ProviderChoice` vocabulary, e.g. `anthropic-api`, `claude`).
///
/// The two vocabularies are not identical, so passing the raw runtime key
/// straight through makes clap reject it (`invalid value 'anthropic-api-key'`)
/// and the freshly spawned window exits immediately before the TUI starts.
/// Returns `None` when the key has no clean standalone CLI provider value; the
/// flag is then omitted and the persisted session reconstructs the route on
/// resume.
fn resume_provider_arg(provider_key: Option<&str>) -> Option<&'static str> {
    provider_key
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(crate::provider::cli_provider_arg_for_session_key)
}

/// Metadata describing why a session window is being spawned, exported to
/// spawn hooks and spawned terminals as `JCODE_SPAWN_*` env vars so external
/// programs (tmux, kitty remote, herd, window managers) can reroute or place
/// the window. See `[terminal] spawn_hook` in config.
#[derive(Debug, Clone, Default)]
pub struct SessionSpawnContext {
    /// Spawn kind override (e.g. "swarm-agent", "restart"). Defaults to
    /// "resume" or "selfdev" based on the launch helper used.
    pub kind: Option<String>,
    /// Extra `JCODE_SPAWN_*` env entries (e.g. swarm/coordinator ids).
    pub extra_env: Vec<(String, String)>,
    /// Terminal-identifying env vars captured from the client that requested
    /// the spawn (tmux/zellij/kitty/DISPLAY/...). Re-exported to spawn/focus
    /// hooks so the new window lands in the client's terminal instead of the
    /// server's stale startup env (#405).
    pub client_terminal_env: Vec<(String, String)>,
    /// Explicit non-secret runtime variables for the child process.
    pub runtime_env: Vec<(String, String)>,
}

impl SessionSpawnContext {
    pub fn kind(kind: impl Into<String>) -> Self {
        Self {
            kind: Some(kind.into()),
            extra_env: Vec::new(),
            runtime_env: Vec::new(),
            client_terminal_env: Vec::new(),
        }
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_env.push((key.into(), value.into()));
        self
    }

    /// Attach the requesting client's terminal env snapshot (#405).
    pub fn with_client_terminal_env(mut self, env: Vec<(String, String)>) -> Self {
        self.client_terminal_env = env;
        self
    }

    /// Attach the explicit runtime env sent only to the spawned jcode child.
    pub fn with_runtime_env(mut self, env: Vec<(String, String)>) -> Self {
        self.runtime_env = env;
        self
    }

    /// Capture effective local runtime values, not just inherited overrides.
    /// The connected socket takes precedence over any stale `JCODE_SOCKET`.
    pub fn with_local_runtime(mut self, connected_socket: &std::path::Path) -> Self {
        let mut env = Vec::new();
        if let Ok(home) = crate::storage::jcode_dir() {
            env.push((
                "JCODE_HOME".to_string(),
                home.to_string_lossy().into_owned(),
            ));
        }
        env.extend([
            (
                "JCODE_RUNTIME_DIR".to_string(),
                crate::storage::runtime_dir().to_string_lossy().into_owned(),
            ),
            (
                "JCODE_SOCKET".to_string(),
                connected_socket.to_string_lossy().into_owned(),
            ),
        ]);
        for key in ["JCODE_API_SOCKET", "JCODE_NO_AUTO_UPDATE"] {
            if let Ok(value) = std::env::var(key) {
                env.push((key.to_string(), value));
            }
        }
        self.runtime_env = env;
        self
    }

    fn apply(
        &self,
        mut command: crate::terminal_launch::TerminalCommand,
        default_kind: &str,
        session_id: &str,
    ) -> crate::terminal_launch::TerminalCommand {
        command = command
            .kind(self.kind.as_deref().unwrap_or(default_kind))
            .session_id(session_id);
        for (key, value) in &self.extra_env {
            command = command.spawn_env(key.clone(), value.clone());
        }
        if !self.runtime_env.is_empty() {
            command = command.runtime_env(self.runtime_env.clone());
        }
        if !self.client_terminal_env.is_empty() {
            command = command.client_terminal_env(self.client_terminal_env.clone());
        }
        command
    }
}

#[cfg(test)]
mod runtime_context_tests {
    use super::*;

    #[test]
    fn app_core_runtime_allowlist_facade_is_portable() {
        let _: &'static [&'static str] = crate::terminal_launch::RUNTIME_ENV_VARS;
        assert!(crate::terminal_launch::RUNTIME_ENV_VARS.contains(&"JCODE_SOCKET"));
    }

    fn value<'a>(context: &'a SessionSpawnContext, key: &str) -> Option<&'a str> {
        context
            .runtime_env
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value.as_str())
    }

    fn run_isolated_context_case(mode: &str) -> std::process::Output {
        let parent_home =
            std::path::PathBuf::from(std::env::var_os("HOME").expect("isolated HOME"));

        let root = parent_home.join(format!(
            "jcode-runtime-context-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("create isolated runtime context directory");
        let home = root.join("home with spaces/it's 🧪");
        let runtime_dir = root.join("runtime dir/it's 🧪");
        let xdg_runtime_dir = root.join("xdg runtime/it's 🧪");
        let jcode_home = root.join("jcode home/it's 🧪");
        let api_socket = root.join("api socket.sock");
        let exe = std::env::current_exe().expect("test binary path");
        let mut command = std::process::Command::new(exe);
        command
            .args([
                "--exact",
                "session_launch::runtime_context_tests::runtime_context_child",
                "--nocapture",
            ])
            .env_clear()
            .env("HOME", &home)
            .env(
                "XDG_RUNTIME_DIR",
                if mode == "custom" {
                    root.join("unused runtime/it's 🧪")
                } else {
                    xdg_runtime_dir.clone()
                },
            )
            .env("JCODE_SOCKET", root.join("stale socket.sock"))
            .env("JCODE_CONTEXT_TEST_MODE", mode)
            .env("JCODE_CONTEXT_TEST_HOME", &home)
            .env("JCODE_CONTEXT_TEST_RUNTIME", &runtime_dir)
            .env("JCODE_CONTEXT_TEST_JCODE_HOME", &jcode_home)
            .env("JCODE_CONTEXT_TEST_XDG_RUNTIME", &xdg_runtime_dir)
            .env("JCODE_CONTEXT_TEST_API_SOCKET", &api_socket);
        if mode == "custom" {
            command
                .env("JCODE_HOME", &jcode_home)
                .env("JCODE_RUNTIME_DIR", &runtime_dir);
        } else {
            command
                .env("JCODE_API_SOCKET", &api_socket)
                .env("JCODE_NO_AUTO_UPDATE", "1");
        }
        let output = command.output().expect("run isolated context case");
        std::fs::remove_dir_all(&root).expect("remove isolated runtime context directory");
        output
    }

    #[cfg(unix)]
    #[test]
    fn local_runtime_context_captures_custom_paths_and_connected_socket() {
        let output = run_isolated_context_case("custom");
        assert!(
            output.status.success(),
            "stdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(unix)]
    #[test]
    fn local_runtime_context_uses_effective_default_home_and_optional_values() {
        let output = run_isolated_context_case("default");
        assert!(
            output.status.success(),
            "stdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(unix)]
    #[test]
    fn runtime_context_child() {
        let Ok(mode) = std::env::var("JCODE_CONTEXT_TEST_MODE") else {
            return;
        };
        let home = std::env::var("JCODE_CONTEXT_TEST_HOME").expect("isolated test home");
        let jcode_home =
            std::env::var("JCODE_CONTEXT_TEST_JCODE_HOME").expect("isolated jcode home");
        let runtime_dir =
            std::env::var("JCODE_CONTEXT_TEST_RUNTIME").expect("isolated runtime dir");
        let xdg_runtime_dir =
            std::env::var("JCODE_CONTEXT_TEST_XDG_RUNTIME").expect("isolated XDG runtime dir");
        let api_socket =
            std::env::var("JCODE_CONTEXT_TEST_API_SOCKET").expect("isolated API socket");
        let socket = std::path::Path::new(&home).join("connected socket/it's 🧪.sock");
        let context = SessionSpawnContext::default().with_local_runtime(&socket);
        if mode == "custom" {
            assert_eq!(value(&context, "JCODE_HOME"), Some(jcode_home.as_str()));
            assert_eq!(
                value(&context, "JCODE_RUNTIME_DIR"),
                Some(runtime_dir.as_str())
            );

            assert_eq!(value(&context, "JCODE_API_SOCKET"), None);
            assert_eq!(value(&context, "JCODE_NO_AUTO_UPDATE"), None);
        } else {
            let expected_home = std::path::Path::new(&home).join(".jcode");
            assert_eq!(
                value(&context, "JCODE_HOME"),
                Some(expected_home.to_str().unwrap())
            );
            assert_eq!(
                value(&context, "JCODE_RUNTIME_DIR"),
                Some(xdg_runtime_dir.as_str())
            );
            assert_eq!(
                value(&context, "JCODE_API_SOCKET"),
                Some(api_socket.as_str())
            );
            assert_eq!(value(&context, "JCODE_NO_AUTO_UPDATE"), Some("1"));
        }
        assert_eq!(
            value(&context, "JCODE_SOCKET"),
            Some(socket.to_str().unwrap())
        );
        let socket_value = socket.to_string_lossy().into_owned();
        for session_id in ["ses active child", "ses completed child"] {
            let args = resume_args(session_id, None, &context);
            assert_eq!(
                args,
                vec![
                    "--fresh-spawn".to_string(),
                    "--socket".to_string(),
                    socket_value.clone(),
                    "--resume".to_string(),
                    session_id.to_string(),
                ]
            );
            let command = context.apply(
                crate::terminal_launch::TerminalCommand::new(
                    "/usr/local/bin/jcode",
                    resume_args(session_id, None, &context),
                )
                .fresh_spawn(),
                "resume",
                session_id,
            );
            assert_eq!(command.session_id.as_deref(), Some(session_id));
            assert_eq!(command.args, args);
            assert_eq!(command.runtime_env, context.runtime_env);
        }
    }
}

#[cfg(unix)]
fn resume_args(
    session_id: &str,
    provider_key: Option<&str>,
    context: &SessionSpawnContext,
) -> Vec<String> {
    let mut args = vec!["--fresh-spawn".to_string()];
    if let Some(provider_arg) = resume_provider_arg(provider_key) {
        args.push("--provider".to_string());
        args.push(provider_arg.to_string());
    }
    if let Some((_, socket)) = context
        .runtime_env
        .iter()
        .find(|(key, _)| key == "JCODE_SOCKET")
    {
        args.extend(["--socket".to_string(), socket.clone()]);
    }
    args.extend(["--resume".to_string(), session_id.to_string()]);
    args
}

/// Compute the window/terminal title used when (re)launching a session.
pub fn resumed_window_title(session_id: &str) -> String {
    let session_name = crate::process_title::session_name(session_id);
    let icon = id::session_icon(&session_name);
    let display_title = crate::process_title::terminal_display_title_for_id(session_id);
    let session_label = crate::process_title::terminal_session_label(&session_name, None);
    let fallback_label = if let Some(server_info) =
        crate::registry::find_server_by_socket_sync(&server::socket_path())
    {
        format!("jcode/{} {}", server_info.name, session_label)
    } else {
        format!("jcode {}", session_label)
    };
    crate::process_title::terminal_window_title(
        icon,
        display_title.as_deref(),
        Some(&fallback_label),
        false,
    )
}

/// Focus/raise the window for `session_id` via the configured focus hook.
///
/// Returns `true` when a hook was configured and its process started (the
/// built-in wmctrl/xdotool fallback should then be skipped). The hook receives
/// `JCODE_FOCUS_SESSION_ID` and `JCODE_FOCUS_TITLE` env vars.
pub fn focus_session_via_hook(session_id: &str, title: &str) -> bool {
    focus_session_via_hook_with_env(session_id, title, &[])
}

/// Like [`focus_session_via_hook`] but also re-exports the requesting client's
/// terminal env (#405) so focus hooks (e.g. `zellij action go-to-tab-name`)
/// target the client's terminal session instead of the server's stale env. Each
/// var is exported natively and under a `JCODE_CLIENT_<NAME>` alias.
pub fn focus_session_via_hook_with_env(
    session_id: &str,
    title: &str,
    client_terminal_env: &[(String, String)],
) -> bool {
    let hook = {
        let config = &crate::config::config().terminal;
        config
            .focus_hook
            .as_deref()
            .map(str::trim)
            .filter(|hook| !hook.is_empty())
            .map(str::to_string)
    };
    let Some(hook) = hook else {
        return false;
    };

    let parts = match crate::terminal_launch::parse_hook_command(&hook) {
        Ok(parts) => parts,
        Err(error) => {
            crate::logging::warn(&format!("Focus hook '{hook}' failed to parse: {error}"));
            return false;
        }
    };
    let (program, args) = parts
        .split_first()
        .expect("parse_hook_command guarantees at least one part");

    let mut cmd = std::process::Command::new(crate::terminal_launch::expand_home(program));
    cmd.args(args)
        .env("JCODE_FOCUS_SESSION_ID", session_id)
        .env("JCODE_FOCUS_TITLE", title)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    for (key, value) in client_terminal_env {
        cmd.env(key, value);
        cmd.env(format!("JCODE_CLIENT_{key}"), value);
    }
    match crate::platform::spawn_detached(&mut cmd) {
        Ok(_) => true,
        Err(error) => {
            crate::logging::warn(&format!(
                "Focus hook '{hook}' failed to start ({error}); falling back to built-in focus"
            ));
            false
        }
    }
}

/// Focus a session window: configured focus hook first, then the built-in
/// wmctrl/xdotool title search (Linux only) as a best-effort fallback.
pub fn focus_session_window_best_effort(session_id: &str, title: &str) {
    focus_session_window_best_effort_with_env(session_id, title, &[]);
}

/// Like [`focus_session_window_best_effort`] but forwards the requesting
/// client's terminal env to the focus hook (#405).
pub fn focus_session_window_best_effort_with_env(
    session_id: &str,
    title: &str,
    client_terminal_env: &[(String, String)],
) {
    if focus_session_via_hook_with_env(session_id, title, client_terminal_env) {
        return;
    }
    focus_title_best_effort(title);
}

#[cfg(all(unix, not(target_os = "macos")))]
fn focus_title_best_effort(title: &str) {
    use std::process::{Command, Stdio};

    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg(
            "sleep 0.4; \
             if command -v wmctrl >/dev/null 2>&1; then wmctrl -a \"$JCODE_WINDOW_TITLE\" >/dev/null 2>&1 && exit 0; fi; \
             if command -v xdotool >/dev/null 2>&1; then xdotool search --name \"$JCODE_WINDOW_TITLE\" windowactivate >/dev/null 2>&1 && exit 0; fi; \
             exit 0",
        )
        .env("JCODE_WINDOW_TITLE", title)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let _ = crate::platform::spawn_detached(&mut cmd);
}

#[cfg(any(not(unix), target_os = "macos"))]
fn focus_title_best_effort(_title: &str) {}

#[cfg(unix)]
pub fn spawn_resume_in_new_terminal(
    exe: &std::path::Path,
    session_id: &str,
    cwd: &std::path::Path,
) -> Result<bool> {
    spawn_resume_in_new_terminal_with_provider(exe, session_id, cwd, None)
}

#[cfg(unix)]
pub fn spawn_resume_in_new_terminal_with_provider(
    exe: &std::path::Path,
    session_id: &str,
    cwd: &std::path::Path,
    provider_key: Option<&str>,
) -> Result<bool> {
    spawn_resume_in_new_terminal_with_context(
        exe,
        session_id,
        cwd,
        provider_key,
        &SessionSpawnContext::default(),
    )
}

#[cfg(unix)]
pub fn spawn_resume_in_new_terminal_with_context(
    exe: &std::path::Path,
    session_id: &str,
    cwd: &std::path::Path,
    provider_key: Option<&str>,
    context: &SessionSpawnContext,
) -> Result<bool> {
    let title = resumed_window_title(session_id);
    let args = resume_args(session_id, provider_key, context);
    let command = crate::terminal_launch::TerminalCommand::new(exe, args)
        .title(title)
        .fresh_spawn();
    let command = context.apply(command, "resume", session_id);
    crate::terminal_launch::spawn_command_in_new_terminal(&command, cwd)
}

#[cfg(unix)]
pub fn spawn_selfdev_in_new_terminal(
    exe: &std::path::Path,
    session_id: &str,
    cwd: &std::path::Path,
) -> Result<bool> {
    spawn_selfdev_in_new_terminal_with_provider(exe, session_id, cwd, None)
}

#[cfg(unix)]
pub fn spawn_selfdev_in_new_terminal_with_provider(
    exe: &std::path::Path,
    session_id: &str,
    cwd: &std::path::Path,
    provider_key: Option<&str>,
) -> Result<bool> {
    spawn_selfdev_in_new_terminal_with_context(
        exe,
        session_id,
        cwd,
        provider_key,
        &SessionSpawnContext::default(),
    )
}

#[cfg(unix)]
pub fn spawn_selfdev_in_new_terminal_with_context(
    exe: &std::path::Path,
    session_id: &str,
    cwd: &std::path::Path,
    provider_key: Option<&str>,
    context: &SessionSpawnContext,
) -> Result<bool> {
    let selfdev_title = format!("{} [self-dev]", resumed_window_title(session_id));
    let mut args = vec!["--fresh-spawn".to_string()];
    if let Some(provider_arg) = resume_provider_arg(provider_key) {
        args.push("--provider".to_string());
        args.push(provider_arg.to_string());
    }
    args.extend([
        "--resume".to_string(),
        session_id.to_string(),
        "self-dev".to_string(),
    ]);
    let command = crate::terminal_launch::TerminalCommand::new(exe, args)
        .title(selfdev_title.clone())
        .fresh_spawn();
    let command = context.apply(command, "selfdev", session_id);
    let spawned = crate::terminal_launch::spawn_command_in_new_terminal(&command, cwd)?;
    if spawned {
        focus_session_window_best_effort_with_env(
            session_id,
            &selfdev_title,
            &context.client_terminal_env,
        );
    }
    Ok(spawned)
}

#[cfg(not(unix))]
fn find_wezterm_gui_binary() -> Option<String> {
    use std::process::{Command, Stdio};

    if let Ok(exe) = std::env::var("WEZTERM_EXECUTABLE") {
        let p = std::path::Path::new(&exe);
        let gui = p.with_file_name("wezterm-gui.exe");
        if gui.exists() {
            return Some(gui.to_string_lossy().into_owned());
        }
        return Some(exe);
    }

    let candidates = [
        r"C:\Program Files\WezTerm\wezterm-gui.exe",
        r"C:\Program Files (x86)\WezTerm\wezterm-gui.exe",
    ];
    for c in &candidates {
        if std::path::Path::new(c).exists() {
            return Some(c.to_string());
        }
    }

    for bin in &["wezterm-gui", "wezterm"] {
        if let Ok(output) = Command::new("where")
            .arg(bin)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Some(line) = stdout.lines().next() {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        if *bin == "wezterm" {
                            let p = std::path::Path::new(trimmed);
                            let gui = p.with_file_name("wezterm-gui.exe");
                            if gui.exists() {
                                return Some(gui.to_string_lossy().into_owned());
                            }
                        }
                        return Some(trimmed.to_string());
                    }
                }
            }
        }
    }

    None
}

#[cfg(not(unix))]
fn resume_terminal_candidates_windows() -> Vec<String> {
    std::env::var("JCODE_RESUME_TERMINAL")
        .ok()
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .filter(|candidates| !candidates.is_empty())
        .unwrap_or_else(|| {
            vec![
                "wezterm".to_string(),
                "wt".to_string(),
                "alacritty".to_string(),
            ]
        })
}

#[cfg(not(unix))]
pub fn spawn_resume_in_new_terminal(
    exe: &std::path::Path,
    session_id: &str,
    cwd: &std::path::Path,
) -> Result<bool> {
    spawn_resume_in_new_terminal_with_provider(exe, session_id, cwd, None)
}

#[cfg(not(unix))]
pub fn spawn_resume_in_new_terminal_with_provider(
    exe: &std::path::Path,
    session_id: &str,
    cwd: &std::path::Path,
    provider_key: Option<&str>,
) -> Result<bool> {
    spawn_resume_in_new_terminal_with_context(
        exe,
        session_id,
        cwd,
        provider_key,
        &SessionSpawnContext::default(),
    )
}

#[cfg(not(unix))]
pub fn spawn_resume_in_new_terminal_with_context(
    exe: &std::path::Path,
    session_id: &str,
    cwd: &std::path::Path,
    provider_key: Option<&str>,
    context: &SessionSpawnContext,
) -> Result<bool> {
    use std::process::{Command, Stdio};

    let mut jcode_args: Vec<String> = Vec::new();
    if let Some(provider_arg) = resume_provider_arg(provider_key) {
        jcode_args.push("--provider".to_string());
        jcode_args.push(provider_arg.to_string());
    }
    jcode_args.push("--resume".to_string());
    jcode_args.push(session_id.to_string());

    let hook_command = crate::terminal_launch::TerminalCommand::new(exe, jcode_args.clone())
        .title(resumed_window_title(session_id));
    let hook_command = context.apply(hook_command, "resume", session_id);
    if crate::terminal_launch::try_spawn_via_configured_hook(&hook_command, cwd) {
        return Ok(true);
    }

    let wezterm_gui = find_wezterm_gui_binary();
    let alacritty_available = Command::new("where")
        .arg("alacritty")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let wt_available = std::env::var("WT_SESSION").is_ok()
        || Command::new("where")
            .arg("wt")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

    for term in resume_terminal_candidates_windows() {
        let status = match term.as_str() {
            "wezterm" => {
                let Some(ref wezterm_bin) = wezterm_gui else {
                    continue;
                };
                let mut cmd = Command::new(wezterm_bin);
                cmd.args(["start", "--always-new-process", "--"])
                    .arg(exe)
                    .args(&jcode_args)
                    .current_dir(cwd)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                crate::terminal_launch::apply_runtime_env(&mut cmd, &context.runtime_env);
                crate::platform::spawn_detached(&mut cmd)
            }
            "wt" | "windows-terminal" => {
                if !wt_available {
                    continue;
                }
                let mut cmd = Command::new("wt.exe");
                cmd.args(["-p", "Command Prompt"])
                    .arg(exe)
                    .args(&jcode_args)
                    .current_dir(cwd)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                crate::terminal_launch::apply_runtime_env(&mut cmd, &context.runtime_env);
                crate::platform::spawn_detached(&mut cmd)
            }
            "alacritty" => {
                if !alacritty_available {
                    continue;
                }
                let mut cmd = Command::new("alacritty");
                cmd.args(["-e"])
                    .arg(exe)
                    .args(&jcode_args)
                    .current_dir(cwd)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                crate::terminal_launch::apply_runtime_env(&mut cmd, &context.runtime_env);
                crate::platform::spawn_detached(&mut cmd)
            }
            _ => continue,
        };

        if status.is_ok() {
            return Ok(true);
        }
    }

    Ok(false)
}

#[cfg(not(unix))]
pub fn spawn_selfdev_in_new_terminal(
    exe: &std::path::Path,
    session_id: &str,
    cwd: &std::path::Path,
) -> Result<bool> {
    spawn_selfdev_in_new_terminal_with_provider(exe, session_id, cwd, None)
}

#[cfg(not(unix))]
pub fn spawn_selfdev_in_new_terminal_with_provider(
    exe: &std::path::Path,
    session_id: &str,
    cwd: &std::path::Path,
    provider_key: Option<&str>,
) -> Result<bool> {
    spawn_selfdev_in_new_terminal_with_context(
        exe,
        session_id,
        cwd,
        provider_key,
        &SessionSpawnContext::default(),
    )
}

#[cfg(not(unix))]
pub fn spawn_selfdev_in_new_terminal_with_context(
    exe: &std::path::Path,
    session_id: &str,
    cwd: &std::path::Path,
    provider_key: Option<&str>,
    context: &SessionSpawnContext,
) -> Result<bool> {
    use std::process::{Command, Stdio};

    let mut jcode_args: Vec<String> = Vec::new();
    if let Some(provider_arg) = resume_provider_arg(provider_key) {
        jcode_args.push("--provider".to_string());
        jcode_args.push(provider_arg.to_string());
    }
    jcode_args.extend([
        "--resume".to_string(),
        session_id.to_string(),
        "self-dev".to_string(),
    ]);

    let hook_command = crate::terminal_launch::TerminalCommand::new(exe, jcode_args.clone())
        .title(format!("{} [self-dev]", resumed_window_title(session_id)));
    let hook_command = context.apply(hook_command, "selfdev", session_id);
    if crate::terminal_launch::try_spawn_via_configured_hook(&hook_command, cwd) {
        return Ok(true);
    }

    let wezterm_gui = find_wezterm_gui_binary();
    let alacritty_available = Command::new("where")
        .arg("alacritty")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let wt_available = std::env::var("WT_SESSION").is_ok()
        || Command::new("where")
            .arg("wt")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

    for term in resume_terminal_candidates_windows() {
        let status = match term.as_str() {
            "wezterm" => {
                let Some(ref wezterm_bin) = wezterm_gui else {
                    continue;
                };
                let mut cmd = Command::new(wezterm_bin);
                cmd.args(["start", "--always-new-process", "--"])
                    .arg(exe)
                    .args(&jcode_args)
                    .current_dir(cwd)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                crate::platform::spawn_detached(&mut cmd)
            }
            "wt" | "windows-terminal" => {
                if !wt_available {
                    continue;
                }
                let mut cmd = Command::new("wt.exe");
                cmd.args(["-p", "Command Prompt"])
                    .arg(exe)
                    .args(&jcode_args)
                    .current_dir(cwd)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                crate::platform::spawn_detached(&mut cmd)
            }
            "alacritty" => {
                if !alacritty_available {
                    continue;
                }
                let mut cmd = Command::new("alacritty");
                cmd.args(["-e"])
                    .arg(exe)
                    .args(&jcode_args)
                    .current_dir(cwd)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                crate::platform::spawn_detached(&mut cmd)
            }
            _ => continue,
        };

        if status.is_ok() {
            return Ok(true);
        }
    }

    Ok(false)
}
