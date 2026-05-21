//! Effort control command - Control AI reasoning depth
//!
//! 对标: Claude Code `effort` command

use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct EffortCommand;

impl Command for EffortCommand {
    fn name(&self) -> &str {
        "effort"
    }

    fn description(&self) -> &str {
        "Control AI reasoning depth: auto|conserve|high"
    }

    async fn execute(&self, args: &[String]) -> Result<CommandResult> {
        if args.is_empty() {
            println!("Current effort level: auto");
            println!("Usage: /effort [auto|conserve|high]");
            return Ok(CommandResult::success(""));
        }

        match args[0].as_str() {
            "auto" | "conserve" | "high" => {
                // TODO: Persist to config
                println!("✅ Effort level set to: {}", args[0]);
                Ok(CommandResult::success(format!("Effort set to {}", args[0])))
            }
            _ => {
                anyhow::bail!("Invalid effort level. Use: auto, conserve, or high")
            }
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }
}
