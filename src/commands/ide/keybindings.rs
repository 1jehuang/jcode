use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct KeybindingsCommand;

impl Command for KeybindingsCommand {
    fn name(&self) -> &str { "keybindings" }
    fn description(&self) -> &str { "Manage keybindings" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Keybindings placeholder"))
    }
}
