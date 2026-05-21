use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct CompactCommand;

impl Command for CompactCommand {
    fn name(&self) -> &str { "compact" }
    fn description(&self) -> &str { "Compact context" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Compact placeholder"))
    }
}
