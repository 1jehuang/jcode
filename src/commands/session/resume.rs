use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct ResumeCommand;

impl Command for ResumeCommand {
    fn name(&self) -> &str { "resume" }
    fn description(&self) -> &str { "Resume session" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Resume placeholder"))
    }
}
