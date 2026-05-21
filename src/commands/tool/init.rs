use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct InitCommand;

impl Command for InitCommand {
    fn name(&self) -> &str { "init" }
    fn description(&self) -> &str { "Initialize project" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Init placeholder"))
    }
}
