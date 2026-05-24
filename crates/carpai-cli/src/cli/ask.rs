//! `carpai ask` — One-shot question mode
//!
//! Sends a single question to the agent and prints the response, then exits.

use std::path::PathBuf;

use anyhow::{Context, Result};

/// Run one-shot question
pub async fn run(question: String, dir: Option<String>) -> Result<()> {
    let working_dir = match dir {
        Some(d) => PathBuf::from(d),
        None => std::env::current_dir().context("Failed to get current directory")?,
    };
    let config = crate::config::CliConfig::cli_default(working_dir);

    tracing::info!("Building agent context for one-shot question");

    let ctx = carpai_core::build_local_agent_context(&config.core);

    tracing::info!("Executing agent turn for: {}", question);
    let output = carpai_core::agent_loop::execute_agent_turn(&ctx, &question)
        .await
        .context("Failed to execute agent turn")?;

    println!("{}", output.text);

    tracing::info!(
        prompt_tokens = output.usage.prompt_tokens,
        completion_tokens = output.usage.completion_tokens,
        total_tokens = output.usage.total_tokens,
        duration_ms = output.duration_ms,
        "One-shot question completed"
    );

    Ok(())
}
