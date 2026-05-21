use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct SessionListCommand;

impl Command for SessionListCommand {
    fn name(&self) -> &str { "session" }
    fn description(&self) -> &str { "List sessions" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Session list placeholder"))
    }
}
