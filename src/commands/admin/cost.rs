use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct CostCommand;

impl Command for CostCommand {
    fn name(&self) -> &str { "cost" }
    fn description(&self) -> &str { "Track costs" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Cost placeholder"))
    }
}
