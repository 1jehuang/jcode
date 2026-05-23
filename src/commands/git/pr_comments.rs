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

        tracing::info!("pr_comments: GitHub API integration pending OAuth token setup");
        // 1. Get current PR number from git remote
        // 2. Fetch comments via GitHub REST/GraphQL API
        // 3. Display threaded comment view
        // 4. AI-suggest responses using LLM provider

        println!("⚠️  PR comments feature coming soon");
        println!("   Requires: GitHub App OAuth token + gh CLI authentication");
        Ok(CommandResult::success("PR comments fetched (0 results - OAuth pending)"))
    }

    fn is_read_only(&self) -> bool {
        true
    }
}
