use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct ConfigCommand;

impl Command for ConfigCommand {
    fn name(&self) -> &str { "config" }
    fn description(&self) -> &str { "Manage config" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Config placeholder"))
    }
}
