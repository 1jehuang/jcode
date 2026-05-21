use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct BuddyCommand;

impl Command for BuddyCommand {
    fn name(&self) -> &str { "buddy" }
    fn description(&self) -> &str { "Pair programming mode" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Buddy placeholder"))
    }
}
