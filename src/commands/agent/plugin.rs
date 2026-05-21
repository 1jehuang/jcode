use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct PluginCommand;

impl Command for PluginCommand {
    fn name(&self) -> &str { "plugin" }
    fn description(&self) -> &str { "Manage plugins" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Plugin placeholder"))
    }
}
