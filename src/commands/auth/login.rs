use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct LoginCommand;

impl Command for LoginCommand {
    fn name(&self) -> &str { "login" }
    fn description(&self) -> &str { "Login to CarpAI" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Login placeholder"))
    }
}
