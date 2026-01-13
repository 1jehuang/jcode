//! Auto-debug feature for jcode
//!
//! When an error occurs, spawn a Claude Code session to analyze and potentially fix it.

use std::process::Command;
use crate::logging;

/// Analyze an error and potentially spawn Claude Code to fix it
pub fn analyze_error(error: &str, context: &str) {
    logging::crash(error, context);

    // Build the prompt for Claude Code
    let prompt = format!(
        r#"JCODE AUTO-DEBUG

An error occurred in jcode. Your task is to:

1. FIRST, analyze if this error is:
   a) A BUG IN THE CODEBASE that should be fixed
   b) A USER ERROR or expected behavior (bad input, missing config, network issue, etc.)
   c) An EXTERNAL ISSUE (API down, rate limit, etc.)

2. If it's a CODEBASE BUG:
   - Find the relevant code in /home/jeremy/jcode/
   - Fix the bug
   - Run tests to verify: cargo test

3. If it's NOT a codebase bug:
   - Just explain what happened and how the user can fix it
   - Do NOT modify code

ERROR:
{}

CONTEXT:
{}

Start by reading the error carefully and determining the category before taking any action."#,
        error, context
    );

    // Spawn Claude Code in background
    let result = Command::new("claude")
        .args([
            "--dangerously-skip-permissions",
            "-p", &prompt,
        ])
        .current_dir("/home/jeremy/jcode")
        .spawn();

    match result {
        Ok(child) => {
            logging::info(&format!("Spawned auto-debug session (pid: {})", child.id()));
            eprintln!("\n[auto-debug] Spawned Claude Code to analyze error (pid: {})", child.id());
        }
        Err(e) => {
            logging::error(&format!("Failed to spawn auto-debug: {}", e));
        }
    }
}

/// Check if auto-debug is enabled
pub fn is_enabled() -> bool {
    // Can be disabled via environment variable
    std::env::var("JCODE_NO_AUTO_DEBUG").is_err()
}
