use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct TerminalSetupCommand;

impl Command for TerminalSetupCommand {
    fn name(&self) -> &str { "terminal-setup" }
    fn description(&self) -> &str { "Terminal setup wizard" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("TerminalSetup placeholder"))
    }
}
