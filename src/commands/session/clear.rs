use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct ClearCommand;

impl Command for ClearCommand {
    fn name(&self) -> &str { "clear" }
    fn description(&self) -> &str { "Clear screen" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Clear placeholder"))
    }
}
