//! `carpai complete` — Code completion mode
//!
//! Reads the file at the given position and returns AI-powered completion suggestions.
//! Uses the CodeCompletion trait from carpai-internal when available, with
//! graceful fallback to execute_agent_turn proxy.

use anyhow::{Context, Result};

/// Run code completion at the given file position
pub async fn run(file: String, line: usize, column: usize) -> Result<()> {
    let working_dir = std::env::current_dir().context("Failed to get current directory")?;
    let config = crate::config::CliConfig::cli_default(working_dir);

    // Read file content
    let content = tokio::fs::read_to_string(&file)
        .await
        .context(format!("Failed to read file: {}", file))?;

    tracing::info!(
        file = %file, line, column, bytes = content.len(),
        "Completion request"
    );

    // Build agent context
    let ctx = carpai_core::build_local_agent_context(&config.core);

    // Step 1: Try CodeCompletion trait directly
    if let Some(completion) = try_code_completion(&ctx, &file, line, column, &content).await {
        tracing::info!("Used CodeCompletion trait directly");
        println!("{}", completion);
        return Ok(());
    }

    // Step 2: Fallback to execute_agent_turn proxy
    tracing::info!("CodeCompletion trait unavailable, falling back to agent_turn proxy");
    let prompt = format!(
        "Provide ONLY the code completion at {}:{}:{}. \
         Do NOT include any explanation, markdown formatting, or backticks.\n\n\
         ```\n{}\n```\n\nCompletion:",
        file, line, column, content
    );

    let output = carpai_core::agent_loop::execute_agent_turn(&ctx, &prompt)
        .await
        .context("Completion failed")?;

    println!("{}", output.text);

    Ok(())
}

/// Try to use the CodeCompletion trait directly.
/// Returns `None` if the trait is not available or fails.
async fn try_code_completion(
    ctx: &carpai_internal::AgentContext,
    file: &str,
    line: usize,
    column: usize,
    content: &str,
) -> Option<String> {
    // TODO: When CodeCompletion trait is properly wired in carpai-core,
    // replace this with direct trait call:
    //
    // let request = CompletionRequest {
    //     file: file.to_string(),
    //     line,
    //     column,
    //     content: content.to_string(),
    //     language: detect_language(file),
    // };
    // let candidates = ctx.code_completion.complete(&request).await.ok()?;
    // candidates.first().map(|c| c.text.clone())

    // For now, CodeCompletion is not wired in carpai-core's AgentContext
    // (ctx.code_completion is not yet populated in build_local_agent_context)
    let _ = (ctx, file, line, column, content);
    None
}
