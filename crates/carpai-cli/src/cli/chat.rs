//! `carpai chat` — Interactive TUI mode

use std::path::PathBuf;

use anyhow::{Context, Result};

/// Run interactive TUI chat mode
pub async fn run(dir: Option<String>) -> Result<()> {
    let working_dir = match dir {
        Some(d) => PathBuf::from(d),
        None => std::env::current_dir().context("Failed to get current directory")?,
    };

    // Load config
    let config_path = working_dir.join(".carpai").join("config.toml");
    let config = if config_path.exists() {
        crate::config::CliConfig::load(&config_path)?
    } else {
        crate::config::CliConfig::cli_default(working_dir.clone())
    };

    // Build agent context (local mode)
    let _ctx = carpai_core::build_local_agent_context(&config.core);
    tracing::info!(mode = %config.core.base.mode, "Starting CarpAI TUI");

    // Start TUI
    crate::tui::run(config).await?;

    Ok(())
}
