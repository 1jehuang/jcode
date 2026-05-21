use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct McpCommand;

impl Command for McpCommand {
    fn name(&self) -> &str { "mcp" }
    fn description(&self) -> &str { "Manage MCP servers" }
    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        Ok(CommandResult::success("MCP placeholder"))
    }
}
