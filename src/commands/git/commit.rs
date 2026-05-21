use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct CommitCommand;

impl Command for CommitCommand {
    fn name(&self) -> &str { "commit" }
    fn description(&self) -> &str { "Git commit" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Commit placeholder"))
    }
}
