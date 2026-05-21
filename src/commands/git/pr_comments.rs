//! PR comments command - Manage GitHub PR reviews
//!
//! 对标: Claude Code `pr_comments` command

use crate::cli::CommandResult;
use crate::commands::Command;
use anyhow::Result;

pub struct PrCommentsCommand;

impl Command for PrCommentsCommand {
    fn name(&self) -> &str {
        "pr-comments"
    }

    fn description(&self) -> &str {
        "Fetch and manage GitHub PR review comments with AI-assisted responses"
    }

    async fn execute(&self, _args: &[String]) -> Result<CommandResult> {
        println!("🔍 Fetching PR comments...");
        
        // TODO: Implement GitHub API integration
        // 1. Get current PR number
        // 2. Fetch comments via GitHub API
        // 3. Display comments
        // 4. AI-suggest responses
        
        println!("⚠️  PR comments feature coming soon");
        Ok(CommandResult::success("PR comments fetched"))
    }

    fn is_read_only(&self) -> bool {
        true
    }
}
