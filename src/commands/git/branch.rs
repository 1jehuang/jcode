use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct BranchCommand;

impl Command for BranchCommand {
    fn name(&self) -> &str { "branch" }
    fn description(&self) -> &str { "Git branch management" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Branch placeholder"))
    }
}
