use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct StatsCommand;

impl Command for StatsCommand {
    fn name(&self) -> &str { "stats" }
    fn description(&self) -> &str { "View statistics" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Stats placeholder"))
    }
}
