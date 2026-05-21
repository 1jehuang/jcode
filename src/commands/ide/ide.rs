use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct IdeCommand;

impl Command for IdeCommand {
    fn name(&self) -> &str { "ide" }
    fn description(&self) -> &str { "IDE integration status" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("IDE placeholder"))
    }
}
