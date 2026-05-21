use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct HooksCommand;

impl Command for HooksCommand {
    fn name(&self) -> &str { "hooks" }
    fn description(&self) -> &str { "Manage hooks" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Hooks placeholder"))
    }
}
