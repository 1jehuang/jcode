use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct DebugCommand;

impl Command for DebugCommand {
    fn name(&self) -> &str { "debug" }
    fn description(&self) -> &str { "Debug code" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Debug placeholder"))
    }
}
