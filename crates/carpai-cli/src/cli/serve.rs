//! `carpai serve` — Launch the CarpAI server
//!
//! This command can operate in two modes:
//! 1. **Subprocess mode** (default): Find and spawn `carpai-server` binary
//! 2. **Library mode** (future): Import carpai-server as a Rust library
//!
//! Currently implemented as subprocess launcher. Library mode requires
//! adding `carpai-server = { path = "../carpai-server" }` to Cargo.toml.

use anyhow::{Context, Result};

/// The name of the server binary
const SERVER_BINARY: &str = "carpai-server";

/// Optional server mode flags
#[derive(Debug, Clone, Default)]
pub struct ServeOptions {
    /// Port to listen on
    pub port: Option<u16>,
    /// Host to bind to
    pub host: Option<String>,
    /// Path to config file
    pub config: Option<String>,
}

/// Launch the CarpAI server
///
/// Currently implements subprocess mode. When carpai-server is added
/// as a library dependency, will also support in-process mode.
pub async fn run() -> Result<()> {
    let options = ServeOptions::default();
    run_with_options(options).await
}

/// Launch the server with specific options
pub async fn run_with_options(options: ServeOptions) -> Result<()> {
    tracing::info!("Looking for server binary: {}", SERVER_BINARY);

    // Try to find the server binary in PATH or next to the CLI binary
    let server_path = find_server_binary().context(format!(
        "Server binary '{}' not found. Install with: cargo install --path crates/carpai-server",
        SERVER_BINARY
    ))?;

    println!("Starting CarpAI server from: {}", server_path.display());

    // Build command arguments
    let mut cmd = tokio::process::Command::new(&server_path);

    // Pass through remaining CLI args
    let extra_args: Vec<String> = std::env::args().skip(2).collect();
    if !extra_args.is_empty() {
        cmd.args(&extra_args);
    }

    // Apply options
    if let Some(port) = options.port {
        cmd.args(["--port", &port.to_string()]);
    }
    if let Some(host) = options.host {
        cmd.args(["--host", &host]);
    }
    if let Some(config) = options.config {
        cmd.args(["--config", &config]);
    }

    let status = cmd
        .spawn()
        .context("Failed to spawn server process")?
        .wait()
        .await
        .context("Server process failed")?;

    if !status.success() {
        anyhow::bail!("Server exited with status: {}", status);
    }

    Ok(())
}

/// Find the server binary in the expected locations
use std::path::PathBuf;

fn find_server_binary() -> Result<PathBuf> {
    // 1. Check CARGO_HOME / target directory (dev mode)
    if let Ok(exe_path) = std::env::current_exe() {
        let sibling = exe_path.parent().map(|p| p.join(SERVER_BINARY));
        let sibling = if cfg!(windows) {
            sibling.map(|p| p.with_extension("exe"))
        } else {
            sibling
        };

        if let Some(ref p) = sibling {
            if p.exists() {
                return Ok(p.clone());
            }
        }
    }

    // 2. Check PATH
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(SERVER_BINARY);
            let candidate = if cfg!(windows) {
                candidate.with_extension("exe")
            } else {
                candidate
            };
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }

    anyhow::bail!(
        "Server binary '{}' not found.\n\
         Install with: cargo install --path crates/carpai-server\n\
         Or build with: cargo build -p carpai-server",
        SERVER_BINARY
    )
}
