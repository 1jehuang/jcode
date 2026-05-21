//! Passes command - Control auto-iteration count
//!
//! 对标: Claude Code `passes` command

use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct PassesCommand;

impl Command for PassesCommand {
    fn name(&self) -> &str {
        "passes"
    }

    fn description(&self) -> &str {
        "Set the number of auto-iteration passes (1-10)"
    }

    async fn execute(&self, args: &[String]) -> Result<CommandResult> {
        if args.is_empty() {
            println!("Current passes: 3");
            println!("Usage: /passes [1-10]");
            return Ok(CommandResult::success(""));
        }

        let count: u32 = args[0].parse()
            .map_err(|_| anyhow::anyhow!("Invalid number: {}", args[0]))?;

        if count < 1 || count > 10 {
            anyhow::bail!("Passes must be between 1 and 10");
        }

        // TODO: Persist to config
        println!("✅ Passes set to: {}", count);
        
        Ok(CommandResult::success(""))
    }

    fn is_read_only(&self) -> bool {
        false
    }
}
