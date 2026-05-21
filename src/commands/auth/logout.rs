use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct LogoutCommand;

impl Command for LogoutCommand {
    fn name(&self) -> &str { "logout" }
    fn description(&self) -> &str { "Logout from CarpAI" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Logout placeholder"))
    }
}
