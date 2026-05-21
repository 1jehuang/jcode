use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct CopyCommand;

impl Command for CopyCommand {
    fn name(&self) -> &str { "copy" }
    fn description(&self) -> &str { "Copy file" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Copy placeholder"))
    }
}
