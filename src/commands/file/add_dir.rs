use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct AddDirCommand;

impl Command for AddDirCommand {
    fn name(&self) -> &str { "add-dir" }
    fn description(&self) -> &str { "Add directory to context" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("AddDir placeholder"))
    }
}
