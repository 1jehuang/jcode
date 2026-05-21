//! Fast mode command - Skip non-essential tool calls
//!
//! 对标: Claude Code `fast` command

use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct FastCommand;

impl Command for FastCommand {
    fn name(&self) -> &str {
        "fast"
    }

    fn description(&self) -> &str {
        "Toggle fast mode to skip non-essential tool calls"
    }

    async fn execute(&self, args: &[String]) -> Result<CommandResult> {
        let enabled = args.first().map(|s| s.as_str()) != Some("off");
        
        // TODO: Persist to config
        println!("✅ Fast mode: {}", if enabled { "ON" } else { "OFF" });
        
        Ok(CommandResult::success(""))
    }

    fn is_read_only(&self) -> bool {
        false
    }
}
