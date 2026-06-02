use crate::storage;
use anyhow::Result;
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
    load_preferred_macos_terminal().unwrap_or_else(detect_macos_terminal)
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
    let escaped_exe = escape_shell_single_quotes(exe_path);
    format!(
        r#"if [ ! -x '{exe}' ]; then printf 'jcode executable not found.\n'; exit 127; fi; '{exe}'; status=$?; if [ "$status" -ne 0 ]; then printf '\nJcode exited with status %s.\n' "$status"; printf 'Press Enter to close... '; read -r _; fi; exit "$status""#,
        exe = escaped_exe,
    )
}

fn open_command_for_terminal(app_name: &str, app_args: &str, shell_command: &str) -> String {
    let escaped_shell = escape_shell_single_quotes(shell_command);
    format!("/usr/bin/open -na {app_name} --args {app_args} '{escaped_shell}'")
}

fn applescript_command_for_terminal(app_name: &str, shell_command: &str) -> String {
    format!(
        "/usr/bin/osascript <<'APPLESCRIPT'\ntell application \"{app_name}\"\n    activate\n    do script \"{}\"\nend tell\nAPPLESCRIPT",
        escape_applescript_text(shell_command)
    )
}

fn applescript_command_for_iterm(shell_command: &str) -> String {
    format!(
        "/usr/bin/osascript <<'APPLESCRIPT'\ntell application \"iTerm2\"\n    create window with default profile command \"{}\"\n    activate\nend tell\nAPPLESCRIPT",
        escape_applescript_text(shell_command)
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

#[cfg(test)]
mod tests {
    use super::{
        MacTerminalKind, applescript_command_for_iterm, applescript_command_for_terminal,
        launch_command_for_macos_terminal, open_command_for_terminal,
    };

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
}
