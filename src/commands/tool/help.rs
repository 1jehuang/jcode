use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct HelpCommand;

impl Command for HelpCommand {
    fn name(&self) -> &str { "help" }
    fn description(&self) -> &str { "Show help" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Help placeholder"))
    }
}
