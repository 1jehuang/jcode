use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct VersionCommand;

impl Command for VersionCommand {
    fn name(&self) -> &str { "version" }
    fn description(&self) -> &str { "Show version" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Version placeholder"))
    }
}
