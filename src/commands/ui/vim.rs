use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct VimCommand;

impl Command for VimCommand {
    fn name(&self) -> &str { "vim" }
    fn description(&self) -> &str { "Toggle vim mode" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Vim placeholder"))
    }
}
