use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct RefactorCommand;

impl Command for RefactorCommand {
    fn name(&self) -> &str { "refactor" }
    fn description(&self) -> &str { "Refactor code" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Refactor placeholder"))
    }
}
