//! Self-dev only: rehearse the *real* release-update flow in a sandbox.
//!
//! Unlike `/update-sim` (which only replays UI states), this launches
//! `scripts/update_rehearsal.py` in a new terminal. That script builds a
//! throwaway `JCODE_HOME` on the previous version, serves the current build as
//! a newer fake release from localhost, and starts jcode so the genuine
//! check -> download -> install -> reload path runs end to end. Each run starts
//! from a wiped sandbox, so the experience is repeatable.

use super::{App, DisplayMessage};
use crossterm::event::{KeyCode, KeyModifiers};
use std::path::{Path, PathBuf};

pub(super) const UPDATE_REHEARSAL_COMMAND: &str = "/update-rehearsal";
const UPDATE_REHEARSAL_SCRIPT: &str = "scripts/update_rehearsal.py";
const UPDATE_REHEARSAL_KEY_LABEL: &str = "Alt+Shift+U";

const USAGE: &str = "Usage: `/update-rehearsal [options]` (self-dev only, also Alt+Shift+U)\n\n\
Opens a new terminal running the real updater against a fake local release in a throwaway sandbox. Your real install is never touched.\n\n\
Options are passed to `scripts/update_rehearsal.py`:\n\
- `--rate N` download speed in MiB/s (default 10, 0 = unlimited)\n\
- `--drop-at PCT` drop the connection once at PCT% to exercise resume\n\
- `--bad-checksum` serve a mismatching SHA256SUMS\n\
- `--up-to-date` serve a release that is not newer\n\
- `--manual` no auto-update, type `/update` yourself\n\
- `--check-delay S` slow down the release check";

/// Alt+Shift+U. Terminals without the kitty protocol report it as an
/// uppercase `U` with only ALT set, so accept both encodings.
pub(super) fn is_update_rehearsal_key(code: KeyCode, modifiers: KeyModifiers) -> bool {
    let alt_only = modifiers == KeyModifiers::ALT;
    let alt_shift = modifiers == KeyModifiers::ALT | KeyModifiers::SHIFT;
    match code {
        KeyCode::Char('U') => alt_only || alt_shift,
        KeyCode::Char('u') => alt_shift,
        _ => false,
    }
}

pub(super) fn rehearsal_script_path(repo_dir: &Path) -> PathBuf {
    repo_dir.join(UPDATE_REHEARSAL_SCRIPT)
}

/// Split `/update-rehearsal --rate 5 --drop-at 40` into script arguments.
pub(super) fn parse_rehearsal_args(trimmed: &str) -> Option<Vec<String>> {
    let rest = trimmed.strip_prefix(UPDATE_REHEARSAL_COMMAND)?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    Some(rest.split_whitespace().map(str::to_string).collect())
}

impl App {
    fn update_rehearsal_allowed(&self) -> bool {
        self.is_selfdev_canary_session()
    }

    /// Keybinding entry point. Outside self-dev the chord is left untouched so
    /// it keeps whatever meaning it has for normal users.
    pub(super) fn handle_update_rehearsal_shortcut(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> bool {
        if !is_update_rehearsal_key(code, modifiers) || !self.update_rehearsal_allowed() {
            return false;
        }
        self.launch_update_rehearsal(Vec::new());
        true
    }

    /// Slash-command entry point. Returns true when the input was this command.
    pub(super) fn handle_update_rehearsal_command(&mut self, trimmed: &str) -> bool {
        let Some(args) = parse_rehearsal_args(trimmed) else {
            return false;
        };
        if !self.update_rehearsal_allowed() {
            self.push_display_message(DisplayMessage::system(
                "`/update-rehearsal` is only available in self-dev sessions. Use `/update-sim` for a UI-only preview."
                    .to_string(),
            ));
            return true;
        }
        if args
            .iter()
            .any(|a| a == "help" || a == "--help" || a == "-h")
        {
            self.push_display_message(DisplayMessage::system(USAGE.to_string()));
            return true;
        }
        self.launch_update_rehearsal(args);
        true
    }

    fn launch_update_rehearsal(&mut self, args: Vec<String>) {
        if crate::tui::is_ssh_remote() {
            self.set_status_notice(
                "Update rehearsal must run on the machine with the jcode checkout",
            );
            return;
        }
        let Some(repo_dir) = crate::build::get_repo_dir() else {
            self.push_display_message(DisplayMessage::error(
                "Update rehearsal: could not locate the jcode repository.".to_string(),
            ));
            return;
        };
        let script = rehearsal_script_path(&repo_dir);
        if !script.exists() {
            self.push_display_message(DisplayMessage::error(format!(
                "Update rehearsal: missing `{}`.",
                script.display()
            )));
            return;
        }

        let shown_args = if args.is_empty() {
            "default scenario".to_string()
        } else {
            args.join(" ")
        };
        match spawn_rehearsal_terminal(&script, args, &repo_dir) {
            Ok(true) => {
                self.set_status_notice(format!("Update rehearsal launched ({shown_args})"));
                self.push_display_message(DisplayMessage::system(format!(
                    "Update rehearsal launched in a new terminal ({shown_args}). It starts a sandboxed jcode on the previous version and runs the real check → download → install → reload against a local fake release. Your install is untouched. Press {UPDATE_REHEARSAL_KEY_LABEL} to run it again, or `/update-rehearsal help` for scenarios."
                )));
            }
            Ok(false) => {
                self.push_display_message(DisplayMessage::system(format!(
                    "No terminal emulator found. Run it yourself:\n\n`{} {}`",
                    script.display(),
                    shown_args
                )));
            }
            Err(error) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Update rehearsal failed to launch: {error}"
                )));
            }
        }
    }
}

fn spawn_rehearsal_terminal(script: &Path, args: Vec<String>, cwd: &Path) -> anyhow::Result<bool> {
    if cfg!(test) {
        return Ok(false);
    }
    let command = crate::terminal_launch::TerminalCommand::new(script, args)
        .title("jcode · update rehearsal".to_string())
        .kind("update-rehearsal");
    crate::terminal_launch::spawn_command_in_new_terminal(&command, cwd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rehearsal_key_accepts_both_alt_shift_encodings() {
        assert!(is_update_rehearsal_key(
            KeyCode::Char('U'),
            KeyModifiers::ALT
        ));
        assert!(is_update_rehearsal_key(
            KeyCode::Char('U'),
            KeyModifiers::ALT | KeyModifiers::SHIFT
        ));
        assert!(is_update_rehearsal_key(
            KeyCode::Char('u'),
            KeyModifiers::ALT | KeyModifiers::SHIFT
        ));
        // Plain Alt+U is page-up and must not be stolen.
        assert!(!is_update_rehearsal_key(
            KeyCode::Char('u'),
            KeyModifiers::ALT
        ));
        assert!(!is_update_rehearsal_key(
            KeyCode::Char('U'),
            KeyModifiers::ALT | KeyModifiers::CONTROL
        ));
    }

    #[test]
    fn parses_command_arguments() {
        assert_eq!(parse_rehearsal_args("/update-rehearsal"), Some(vec![]));
        assert_eq!(
            parse_rehearsal_args("/update-rehearsal --rate 5  --drop-at 40"),
            Some(vec![
                "--rate".to_string(),
                "5".to_string(),
                "--drop-at".to_string(),
                "40".to_string()
            ])
        );
        assert_eq!(parse_rehearsal_args("/update-rehearsals"), None);
        assert_eq!(parse_rehearsal_args("/update-sim"), None);
    }

    #[test]
    fn script_lives_in_repo_scripts_dir() {
        let path = rehearsal_script_path(Path::new("/repo"));
        assert_eq!(path, Path::new("/repo/scripts/update_rehearsal.py"));
    }

    #[test]
    fn checked_in_script_exists() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        assert!(rehearsal_script_path(&repo).exists());
    }
}
