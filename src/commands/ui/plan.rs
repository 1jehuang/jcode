use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct PlanCommand;

impl Command for PlanCommand {
    fn name(&self) -> &str { "plan" }
    fn description(&self) -> &str { "Enter plan mode" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Plan placeholder"))
    }
}
