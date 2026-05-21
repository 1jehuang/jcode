use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct UsageCommand;

impl Command for UsageCommand {
    fn name(&self) -> &str { "usage" }
    fn description(&self) -> &str { "View usage" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Usage placeholder"))
    }
}
