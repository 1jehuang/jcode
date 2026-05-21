use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct ThemeCommand;

impl Command for ThemeCommand {
    fn name(&self) -> &str { "theme" }
    fn description(&self) -> &str { "Change theme" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Theme placeholder"))
    }
}
