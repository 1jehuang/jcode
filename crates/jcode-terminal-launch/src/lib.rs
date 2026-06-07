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

pub fn spawn_command_in_new_terminal_with(
    command: &TerminalCommand,
    cwd: &Path,
    mut spawn_detached: impl FnMut(&mut Command) -> std::io::Result<()>,
) -> Result<bool> {
    let mut last_spawn_error: Option<std::io::Error> = None;

    for term in resume_terminal_candidates() {
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
        _ => return None,
    }

    Some(cmd)
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
        assert!(applescript.contains("exec \\\"") == false);
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
}
