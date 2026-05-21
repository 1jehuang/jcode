use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct TeleportCommand;

impl Command for TeleportCommand {
    fn name(&self) -> &str { "teleport" }
    fn description(&self) -> &str { "Quick navigation" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Teleport placeholder"))
    }
}
