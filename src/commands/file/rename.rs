use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct RenameCommand;

impl Command for RenameCommand {
    fn name(&self) -> &str { "rename" }
    fn description(&self) -> &str { "Rename file" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Rename placeholder"))
    }
}
