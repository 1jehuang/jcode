use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct BridgeCommand;

impl Command for BridgeCommand {
    fn name(&self) -> &str { "bridge" }
    fn description(&self) -> &str { "Bridge mode" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("Bridge placeholder"))
    }
}
