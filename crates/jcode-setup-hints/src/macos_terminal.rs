use anyhow::Result;
use jcode_storage as storage;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MacTerminalKind {
    Ghostty,
    Iterm2,
    AppleTerminal,
    WezTerm,
    Warp,
    Alacritty,
    Vscode,
    Unknown,
}

impl MacTerminalKind {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Ghostty => "Ghostty",
            Self::Iterm2 => "iTerm2",
            Self::AppleTerminal => "Terminal.app",
            Self::WezTerm => "WezTerm",
            Self::Warp => "Warp",
            Self::Alacritty => "Alacritty",
            Self::Vscode => "VS Code terminal",
            Self::Unknown => "your current terminal",
        }
    }

    pub(super) fn cli_value(self) -> &'static str {
        match self {
            Self::Ghostty => "ghostty",
            Self::Iterm2 => "iterm2",
            Self::AppleTerminal => "terminal",
            Self::WezTerm => "wezterm",
            Self::Warp => "warp",
            Self::Alacritty => "alacritty",
            Self::Vscode => "vscode",
            Self::Unknown => "terminal",
        }
    }

    fn from_cli_value(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "ghostty" => Some(Self::Ghostty),
            "iterm2" | "iterm" => Some(Self::Iterm2),
            "terminal" | "terminal.app" | "apple_terminal" => Some(Self::AppleTerminal),
            "wezterm" => Some(Self::WezTerm),
            "warp" => Some(Self::Warp),
            "alacritty" => Some(Self::Alacritty),
            "vscode" | "code" => Some(Self::Vscode),
            _ => None,
        }
    }

    fn open_command_app_and_args(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Ghostty => Some(("Ghostty", "-e /bin/bash -lc")),
            Self::Alacritty => Some(("Alacritty", "-e /bin/bash -lc")),
            Self::WezTerm => Some(("WezTerm", "start --always-new-process -- /bin/bash -lc")),
            Self::Iterm2 | Self::AppleTerminal | Self::Warp | Self::Vscode | Self::Unknown => None,
        }
    }
}

impl fmt::Display for MacTerminalKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MacTerminalPreference {
    terminal: String,
}

fn mac_terminal_pref_path() -> Result<PathBuf> {
    Ok(storage::jcode_dir()?.join("preferred_terminal.json"))
}

pub(super) fn load_preferred_macos_terminal() -> Option<MacTerminalKind> {
    let path = mac_terminal_pref_path().ok()?;
    let pref: MacTerminalPreference = storage::read_json(&path).ok()?;
    MacTerminalKind::from_cli_value(&pref.terminal)
}

pub(super) fn save_preferred_macos_terminal(terminal: MacTerminalKind) -> Result<()> {
    let path = mac_terminal_pref_path()?;
    storage::write_json(
        &path,
        &MacTerminalPreference {
            terminal: terminal.cli_value().to_string(),
        },
    )
}

pub(super) fn effective_macos_terminal() -> MacTerminalKind {
    // Precedence: config.toml `[terminal] preferred` (discoverable, documented)
    // > legacy `~/.jcode/preferred_terminal.json` > runtime detection.
    config_preferred_macos_terminal()
        .or_else(load_preferred_macos_terminal)
        .unwrap_or_else(detect_macos_terminal)
}

/// Read `[terminal] preferred` from `~/.jcode/config.toml`, if present and valid.
fn config_preferred_macos_terminal() -> Option<MacTerminalKind> {
    #[derive(Deserialize, Default)]
    struct Wrapper {
        #[serde(default)]
        terminal: jcode_config_types::TerminalConfig,
    }
    let dir = storage::jcode_dir().ok()?;
    let text = std::fs::read_to_string(dir.join("config.toml")).ok()?;
    let wrapper = toml::from_str::<Wrapper>(&text).ok()?;
    let preferred = wrapper.terminal.preferred?;
    MacTerminalKind::from_cli_value(&preferred)
}

fn detect_macos_terminal() -> MacTerminalKind {
    let term_program = std::env::var("TERM_PROGRAM")
        .unwrap_or_default()
        .to_lowercase();
    let term = std::env::var("TERM").unwrap_or_default().to_lowercase();

    if std::env::var("GHOSTTY_RESOURCES_DIR").is_ok()
        || std::env::var("GHOSTTY_BIN_DIR").is_ok()
        || term_program == "ghostty"
        || term.contains("ghostty")
    {
        return MacTerminalKind::Ghostty;
    }

    match term_program.as_str() {
        "iterm.app" => MacTerminalKind::Iterm2,
        "apple_terminal" => MacTerminalKind::AppleTerminal,
        "wezterm" => MacTerminalKind::WezTerm,
        "vscode" => MacTerminalKind::Vscode,
        _ => {
            if term.contains("alacritty") {
                MacTerminalKind::Alacritty
            } else if term.contains("warp") {
                MacTerminalKind::Warp
            } else {
                MacTerminalKind::Unknown
            }
        }
    }
}

pub(super) fn escape_shell_single_quotes(input: &str) -> String {
    input.replace('\'', r#"'\''"#)
}

pub(super) fn escape_applescript_text(input: &str) -> String {
    input.replace('\\', "\\\\").replace('"', "\\\"")
}

pub(super) fn paused_jcode_shell_command(exe_path: &str) -> String {
    paused_jcode_shell_command_with_args(exe_path, &[])
}

/// Like [`paused_jcode_shell_command`] but passes extra CLI args (each
/// single-quoted) to the jcode invocation, e.g. `--resume <session-id>`.
pub(super) fn paused_jcode_shell_command_with_args(exe_path: &str, args: &[String]) -> String {
    let escaped_exe = escape_shell_single_quotes(exe_path);
    let mut arg_str = String::new();
    for arg in args {
        arg_str.push_str(" '");
        arg_str.push_str(&escape_shell_single_quotes(arg));
        arg_str.push('\'');
    }
    format!(
        r#"if [ ! -x '{exe}' ]; then printf 'jcode executable not found.\n'; exit 127; fi; '{exe}'{args}; status=$?; if [ "$status" -ne 0 ]; then printf '\nJcode exited with status %s.\n' "$status"; printf 'Press Enter to close... '; read -r _; fi; exit "$status""#,
        exe = escaped_exe,
        args = arg_str,
    )
}

fn open_command_for_terminal(app_name: &str, app_args: &str, shell_command: &str) -> String {
    let escaped_shell = escape_shell_single_quotes(shell_command);
    format!("/usr/bin/open -na {app_name} --args {app_args} '{escaped_shell}'")
}

/// Wrap a POSIX/bash launcher snippet so it always runs under bash, regardless
/// of the user's login shell.
///
/// macOS `do script` (Terminal.app/Warp/VS Code) and iTerm's `command` run the
/// string in the user's *default login shell*. Our launcher snippet is plain
/// bash (`status=$?`, `[ ... ]`, `read -r _`), which non-POSIX shells like fish
/// cannot parse (`fish: Unsupported use of '='`). Running it through
/// `/bin/bash -lc` makes the launcher shell-agnostic, matching what the
/// open-command terminals (Ghostty/Alacritty/WezTerm) already do.
fn run_in_bash_login(shell_command: &str) -> String {
    format!(
        "/bin/bash -lc '{}'",
        escape_shell_single_quotes(shell_command)
    )
}

fn applescript_command_for_terminal(app_name: &str, shell_command: &str) -> String {
    let bash_command = run_in_bash_login(shell_command);
    format!(
        "/usr/bin/osascript <<'APPLESCRIPT'\ntell application \"{app_name}\"\n    activate\n    do script \"{}\"\nend tell\nAPPLESCRIPT",
        escape_applescript_text(&bash_command)
    )
}

fn applescript_command_for_iterm(shell_command: &str) -> String {
    let bash_command = run_in_bash_login(shell_command);
    format!(
        "/usr/bin/osascript <<'APPLESCRIPT'\ntell application \"iTerm2\"\n    create window with default profile command \"{}\"\n    activate\nend tell\nAPPLESCRIPT",
        escape_applescript_text(&bash_command)
    )
}

pub(super) fn launch_command_for_macos_terminal(
    terminal: MacTerminalKind,
    shell_command: &str,
) -> String {
    if let Some((app_name, app_args)) = terminal.open_command_app_and_args() {
        return open_command_for_terminal(app_name, app_args, shell_command);
    }

    match terminal {
        MacTerminalKind::Iterm2 => applescript_command_for_iterm(shell_command),
        MacTerminalKind::AppleTerminal
        | MacTerminalKind::Warp
        | MacTerminalKind::Vscode
        | MacTerminalKind::Unknown => applescript_command_for_terminal("Terminal", shell_command),
        MacTerminalKind::Ghostty | MacTerminalKind::WezTerm | MacTerminalKind::Alacritty => {
            unreachable!("open-command terminals should be handled above")
        }
    }
}

#[cfg(target_os = "macos")]
pub(super) fn launch_script_for_macos_terminal(
    terminal: MacTerminalKind,
    shell_command: &str,
) -> String {
    format!(
        "#!/bin/bash\nset -e\n{}\n",
        launch_command_for_macos_terminal(terminal, shell_command)
    )
}

/// How to launch a shell command in a new terminal window without Apple
/// Events automation. Background helpers (the menu bar app, launchd agents)
/// cannot reliably get the "control Terminal" TCC permission that the
/// AppleScript launch path needs, so they use this strategy instead.
pub(super) enum NoAutomationLaunch {
    /// Run this shell command directly (terminals launchable via
    /// `open -na <App> --args ...`).
    Shell(String),
    /// Write the shell command to an executable `.command` file and open it
    /// with the named app (`None` = system default handler, Terminal.app).
    CommandFile { app: Option<&'static str> },
}

pub(super) fn no_automation_launch(
    terminal: MacTerminalKind,
    shell_command: &str,
) -> NoAutomationLaunch {
    if let Some((app_name, app_args)) = terminal.open_command_app_and_args() {
        return NoAutomationLaunch::Shell(open_command_for_terminal(
            app_name,
            app_args,
            shell_command,
        ));
    }
    match terminal {
        MacTerminalKind::Iterm2 => NoAutomationLaunch::CommandFile { app: Some("iTerm") },
        _ => NoAutomationLaunch::CommandFile { app: None },
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MacTerminalKind, applescript_command_for_iterm, applescript_command_for_terminal,
        launch_command_for_macos_terminal, open_command_for_terminal,
    };

    #[test]
    fn from_cli_value_maps_documented_config_values() {
        // These are the values documented for `[terminal] preferred` in
        // config.toml; they must round-trip to a known terminal kind (#401).
        let cases = [
            ("ghostty", MacTerminalKind::Ghostty),
            ("iterm2", MacTerminalKind::Iterm2),
            ("iterm", MacTerminalKind::Iterm2),
            ("terminal", MacTerminalKind::AppleTerminal),
            ("wezterm", MacTerminalKind::WezTerm),
            ("warp", MacTerminalKind::Warp),
            ("alacritty", MacTerminalKind::Alacritty),
            ("vscode", MacTerminalKind::Vscode),
            ("code", MacTerminalKind::Vscode),
            ("  Ghostty  ", MacTerminalKind::Ghostty),
        ];
        for (value, expected) in cases {
            assert_eq!(
                MacTerminalKind::from_cli_value(value),
                Some(expected),
                "config value {value:?} should map to {expected:?}"
            );
        }
        assert_eq!(MacTerminalKind::from_cli_value("nope"), None);
    }

    #[test]
    fn open_command_terminals_use_open_with_expected_args() {
        let shell_command = "printf 'hi'";
        assert_eq!(
            launch_command_for_macos_terminal(MacTerminalKind::Ghostty, shell_command),
            open_command_for_terminal("Ghostty", "-e /bin/bash -lc", shell_command)
        );
        assert_eq!(
            launch_command_for_macos_terminal(MacTerminalKind::Alacritty, shell_command),
            open_command_for_terminal("Alacritty", "-e /bin/bash -lc", shell_command)
        );
        assert_eq!(
            launch_command_for_macos_terminal(MacTerminalKind::WezTerm, shell_command),
            open_command_for_terminal(
                "WezTerm",
                "start --always-new-process -- /bin/bash -lc",
                shell_command,
            )
        );
    }

    #[test]
    fn paused_shell_command_quotes_extra_args() {
        let cmd = super::paused_jcode_shell_command_with_args(
            "/usr/local/bin/jcode",
            &["--resume".to_string(), "session_fox_123_abc".to_string()],
        );
        assert!(cmd.contains("'/usr/local/bin/jcode' '--resume' 'session_fox_123_abc';"));

        // Single quotes in args must be escaped, not break out of quoting.
        let cmd = super::paused_jcode_shell_command_with_args(
            "/usr/local/bin/jcode",
            &["it's".to_string()],
        );
        assert!(cmd.contains(r#"'it'\''s'"#));

        // No args matches the plain command.
        assert_eq!(
            super::paused_jcode_shell_command_with_args("/usr/local/bin/jcode", &[]),
            super::paused_jcode_shell_command("/usr/local/bin/jcode"),
        );
    }

    #[test]
    fn applescript_terminals_use_expected_launcher_commands() {
        let shell_command = r#"echo "hi""#;
        assert_eq!(
            launch_command_for_macos_terminal(MacTerminalKind::Iterm2, shell_command),
            applescript_command_for_iterm(shell_command)
        );
        assert_eq!(
            launch_command_for_macos_terminal(MacTerminalKind::AppleTerminal, shell_command),
            applescript_command_for_terminal("Terminal", shell_command)
        );
        assert_eq!(
            launch_command_for_macos_terminal(MacTerminalKind::Warp, shell_command),
            applescript_command_for_terminal("Terminal", shell_command)
        );
    }

    #[test]
    fn applescript_launchers_run_under_bash_for_non_bash_login_shells() {
        // The launcher snippet is bash-specific (`status=$?`). `do script` and
        // iTerm `command` run in the user's login shell, so a fish/zsh-quirky
        // login shell would otherwise choke ("fish: Unsupported use of '='").
        // Both must wrap the snippet in `/bin/bash -lc`.
        let shell_command = super::paused_jcode_shell_command("/usr/local/bin/jcode");
        assert!(shell_command.contains("status=$?"));

        for terminal in [
            MacTerminalKind::AppleTerminal,
            MacTerminalKind::Warp,
            MacTerminalKind::Vscode,
            MacTerminalKind::Unknown,
            MacTerminalKind::Iterm2,
        ] {
            let launcher = launch_command_for_macos_terminal(terminal, &shell_command);
            assert!(
                launcher.contains("/bin/bash -lc"),
                "{terminal:?} launcher must run under bash: {launcher}"
            );
        }
    }

    #[test]
    fn open_command_terminals_pass_snippet_to_bash_not_login_shell() {
        // Ghostty/Alacritty/WezTerm wrap via `open -na ... -e /bin/bash -lc`,
        // so the bash-specific snippet never reaches the login shell.
        let shell_command = super::paused_jcode_shell_command("/usr/local/bin/jcode");
        for terminal in [
            MacTerminalKind::Ghostty,
            MacTerminalKind::Alacritty,
            MacTerminalKind::WezTerm,
        ] {
            let launcher = launch_command_for_macos_terminal(terminal, &shell_command);
            assert!(
                launcher.contains("/bin/bash -lc"),
                "{terminal:?} launcher must run under bash: {launcher}"
            );
        }
    }
}
