use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct AgentsCommand;

impl Command for AgentsCommand {
    fn name(&self) -> &str { "agents" }
    fn description(&self) -> &str { "Manage agents" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Agents placeholder"))
    }
}
