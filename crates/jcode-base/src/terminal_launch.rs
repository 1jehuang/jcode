use anyhow::Result;
pub use jcode_terminal_launch::{
    SpawnAttempt, TerminalCommand, detected_resume_terminal, resume_terminal_candidates, sh_escape,
    shell_command,
};
use std::path::Path;

pub fn spawn_command_in_new_terminal(command: &TerminalCommand, cwd: &Path) -> Result<bool> {
    jcode_terminal_launch::spawn_command_in_new_terminal_with(command, cwd, |cmd| {
        crate::platform::spawn_detached(cmd).map(|_| ())
    })
}
