use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct FeedbackCommand;

impl Command for FeedbackCommand {
    fn name(&self) -> &str { "feedback" }
    fn description(&self) -> &str { "Submit feedback" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Feedback placeholder"))
    }
}
