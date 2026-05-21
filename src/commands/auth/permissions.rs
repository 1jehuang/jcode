use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct PermissionsCommand;

impl Command for PermissionsCommand {
    fn name(&self) -> &str { "permissions" }
    fn description(&self) -> &str { "Manage permissions" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Permissions placeholder"))
    }
}
