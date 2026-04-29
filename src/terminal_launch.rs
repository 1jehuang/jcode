use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Debug)]
pub struct TerminalCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub title: Option<String>,
    pub fresh_spawn: bool,
}

impl TerminalCommand {
    pub fn new(program: impl Into<PathBuf>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
            title: None,
            fresh_spawn: false,
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
        "iterm2" => {
            macos_current_terminal_is("iterm2")
                || macos_app_installed("iTerm.app")
                || macos_app_installed("iTerm2.app")
        }
        "terminal" => true,
        _ => true,
    }
}

#[cfg(unix)]
pub fn detected_resume_terminal() -> Option<String> {
    if std::env::var("HANDTERM_SESSION").is_ok() || std::env::var("HANDTERM_PID").is_ok() {
        return Some("handterm".to_string());
    }
    if std::env::var("TERM_PROGRAM")
        .ok()
        .map(|value| value.eq_ignore_ascii_case("handterm"))
        .unwrap_or(false)
    {
        return Some("handterm".to_string());
    }
    if std::env::var("KITTY_PID").is_ok() {
        return Some("kitty".to_string());
    }
    if std::env::var("WEZTERM_EXECUTABLE").is_ok() || std::env::var("WEZTERM_PANE").is_ok() {
        return Some("wezterm".to_string());
    }
    if std::env::var("ALACRITTY_WINDOW_ID").is_ok() {
        return Some("alacritty".to_string());
    }

    #[cfg(target_os = "macos")]
    {
        if std::env::var("GHOSTTY_RESOURCES_DIR").is_ok()
            || std::env::var("GHOSTTY_BIN_DIR").is_ok()
        {
            return Some("ghostty".to_string());
        }
        let term_program = std::env::var("TERM_PROGRAM")
            .ok()
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
pub fn resume_terminal_candidates() -> Vec<String> {
    let mut candidates = Vec::new();
    if let Ok(term) = std::env::var("JCODE_TERMINAL") {
        push_unique_terminal(&mut candidates, term);
    }
    if let Some(term) = detected_resume_terminal() {
        push_unique_terminal(&mut candidates, term);
    }

    #[cfg(target_os = "macos")]
    {
        for term in [
            "ghostty",
            "kitty",
            "wezterm",
            "alacritty",
            "iterm2",
            "terminal",
        ] {
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

#[cfg(not(unix))]
pub fn resume_terminal_candidates() -> Vec<String> {
    let mut candidates = Vec::new();
    if let Ok(term) = std::env::var("JCODE_TERMINAL") {
        push_unique_terminal(&mut candidates, term);
    }
    if let Some(term) = detected_resume_terminal() {
        push_unique_terminal(&mut candidates, term);
    }
    for term in ["wezterm", "wt", "alacritty"] {
        push_unique_terminal(&mut candidates, term);
    }
    candidates
}

pub fn spawn_command_in_new_terminal(command: &TerminalCommand, cwd: &Path) -> Result<bool> {
    let mut last_spawn_error: Option<std::io::Error> = None;

    for term in resume_terminal_candidates() {
        let Some(mut cmd) = build_spawn_command(&term, command, cwd) else {
            continue;
        };

        match crate::platform::spawn_detached(&mut cmd) {
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
        "handterm" => {
            let shell = shell_command(&command_parts(command));
            cmd.args(["--standalone", "--backend", "gpu", "--exec", &shell]);
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
            cmd = Command::new("open");
            cmd.args([
                "-a",
                "Terminal",
                command.program.to_str().unwrap_or("jcode"),
                "--args",
            ]);
            cmd.args(&command.args);
        }
        #[cfg(not(unix))]
        "wt" => {
            cmd.args(["new-tab", "--title", title]);
            cmd.arg(&command.program).args(&command.args);
        }
        _ => return None,
    }

    Some(cmd)
}

fn command_parts(command: &TerminalCommand) -> Vec<String> {
    std::iter::once(command.program.to_string_lossy().into_owned())
        .chain(command.args.iter().cloned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

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
    fn shell_command_quotes_arguments() {
        let shell = shell_command(&["jcode".to_string(), "it's ok".to_string()]);
        #[cfg(unix)]
        assert_eq!(shell, "'jcode' 'it'\"'\"'s ok'");
    }
}
