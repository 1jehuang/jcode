//! Self-development tool - manage canary builds when working on jcode itself

use crate::build;
use crate::tool::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
struct SelfDevInput {
    action: String,
    #[serde(default)]
    message: Option<String>,
}

pub struct SelfDevTool;

impl SelfDevTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for SelfDevTool {
    fn name(&self) -> &str {
        "selfdev"
    }

    fn description(&self) -> &str {
        "Self-development tool for working on jcode itself. Only available in self-dev mode. \
         Actions: 'reload' (restart with already-built binary), 'promote' (mark current build as stable), \
         'status' (show build versions), 'rollback' (switch to stable)."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["reload", "promote", "status", "rollback", "debug", "debug-dump"],
                    "description": "Action to perform: 'reload' restarts with already-built binary, \
                                   'promote' marks current canary as stable for other sessions, \
                                   'status' shows current build versions and any crash history, \
                                   'rollback' manually switches back to the stable build, \
                                   'debug' enables visual debugging for TUI issues, \
                                   'debug-dump' writes captured debug frames to file"
                },
                "message": {
                    "type": "string",
                    "description": "Optional message for promote action (describes what was changed)"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: SelfDevInput = serde_json::from_value(input)?;

        match params.action.as_str() {
            "reload" => self.do_reload().await,
            "promote" => self.do_promote(params.message).await,
            "status" => self.do_status().await,
            "rollback" => self.do_rollback().await,
            "debug" => self.do_debug_enable().await,
            "debug-dump" => self.do_debug_dump().await,
            _ => Ok(ToolOutput::new(format!(
                "Unknown action: {}. Use 'reload', 'promote', 'status', 'rollback', 'debug', or 'debug-dump'.",
                params.action
            ))),
        }
    }
}

impl SelfDevTool {
    async fn do_reload(&self) -> Result<ToolOutput> {
        let repo_dir = build::get_repo_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find jcode repository directory"))?;

        // Check that binary exists
        let target_binary = repo_dir.join("target/release/jcode");
        if !target_binary.exists() {
            return Ok(ToolOutput::new(
                "No binary found at target/release/jcode.\n\
                 Run 'cargo build --release' first, then try reload again."
                    .to_string(),
            ));
        }

        let hash = build::current_git_hash(&repo_dir)?;

        // Install this version and set as canary (stable stays as safety net)
        build::install_version(&repo_dir, &hash)?;
        build::update_canary_symlink(&hash)?;

        // Update manifest - set as canary, keep stable unchanged
        let mut manifest = build::BuildManifest::load()?;
        let stable_hash = manifest.stable.clone().unwrap_or_else(|| "none".to_string());
        manifest.canary = Some(hash.clone());
        manifest.canary_status = Some(build::CanaryStatus::Testing);
        manifest.save()?;

        // Write reload info for post-restart display
        let info_path = crate::storage::jcode_dir()?.join("reload-info");
        let info = format!("reload:{}", hash);
        std::fs::write(&info_path, &info)?;

        // Write signal file for TUI to pick up
        let signal_path = crate::storage::jcode_dir()?.join("rebuild-signal");
        std::fs::write(&signal_path, &hash)?;

        Ok(ToolOutput::new(format!(
            "Reloading with canary build {}.\n\
             Stable: {} (safety net)\n\n\
             **Restarting...** The session will continue automatically.",
            hash, stable_hash
        )))
    }

    async fn do_promote(&self, message: Option<String>) -> Result<ToolOutput> {
        let mut manifest = build::BuildManifest::load()?;

        let canary_hash = manifest
            .canary
            .clone()
            .ok_or_else(|| anyhow::anyhow!("No canary build to promote"))?;

        // Update stable symlink
        build::update_stable_symlink(&canary_hash)?;

        // Write stable version file (triggers auto-migration in other sessions)
        build::write_stable_version(&canary_hash)?;

        // Update manifest
        manifest.stable = Some(canary_hash.clone());
        manifest.canary_status = Some(build::CanaryStatus::Passed);
        manifest.save()?;

        let msg = message.unwrap_or_else(|| "Promoted to stable".to_string());

        Ok(ToolOutput::new(format!(
            "Promoted {} to stable.\n\n\
             Other active sessions will auto-migrate to this version.\n\n\
             Message: {}",
            canary_hash, msg
        )))
    }

    async fn do_status(&self) -> Result<ToolOutput> {
        let manifest = build::BuildManifest::load()?;

        let mut status = String::new();

        // Current running version
        status.push_str("## Current Version\n\n");
        status.push_str(&format!(
            "**Running:** jcode {}\n",
            env!("JCODE_VERSION")
        ));

        // Working tree status
        if let Some(repo_dir) = build::get_repo_dir() {
            let output = std::process::Command::new("git")
                .args(["status", "--porcelain"])
                .current_dir(&repo_dir)
                .output()
                .ok();

            if let Some(output) = output {
                let changes: Vec<&str> = std::str::from_utf8(&output.stdout)
                    .unwrap_or("")
                    .lines()
                    .collect();
                if changes.is_empty() {
                    status.push_str("**Working tree:** clean\n");
                } else {
                    status.push_str(&format!(
                        "**Working tree:** {} uncommitted change{}\n",
                        changes.len(),
                        if changes.len() == 1 { "" } else { "s" }
                    ));
                }
            }
        }

        // Build versions
        status.push_str("\n## Build Status\n\n");

        if let Some(ref stable) = manifest.stable {
            status.push_str(&format!("**Stable:** {}\n", stable));
        } else {
            status.push_str("**Stable:** none\n");
        }

        if let Some(ref canary) = manifest.canary {
            let status_str = match &manifest.canary_status {
                Some(build::CanaryStatus::Testing) => "testing",
                Some(build::CanaryStatus::Passed) => "passed",
                Some(build::CanaryStatus::Failed) => "failed",
                None => "unknown",
            };
            status.push_str(&format!("**Canary:** {} ({})\n", canary, status_str));
        } else {
            status.push_str("**Canary:** none\n");
        }

        // Recent crash info
        if let Some(ref crash) = manifest.last_crash {
            status.push_str(&format!(
                "\n## Last Crash\n\n\
                 Build: {}\n\
                 Exit code: {}\n\
                 Time: {}\n",
                crash.build_hash,
                crash.exit_code,
                crash.crashed_at.format("%Y-%m-%d %H:%M:%S UTC")
            ));

            if !crash.stderr.is_empty() {
                let stderr_preview = if crash.stderr.len() > 500 {
                    format!("{}...", &crash.stderr[..500])
                } else {
                    crash.stderr.clone()
                };
                status.push_str(&format!("\nStderr:\n```\n{}\n```\n", stderr_preview));
            }
        }

        // Recent builds
        if !manifest.history.is_empty() {
            status.push_str("\n## Recent Builds\n\n");
            for (i, info) in manifest.history.iter().take(5).enumerate() {
                let dirty_marker = if info.dirty { " (dirty)" } else { "" };
                let msg = info.commit_message.as_deref().unwrap_or("no message");
                status.push_str(&format!(
                    "{}. {} - {}{}\n",
                    i + 1,
                    info.hash,
                    msg,
                    dirty_marker
                ));
            }
        }

        Ok(ToolOutput::new(status))
    }

    async fn do_rollback(&self) -> Result<ToolOutput> {
        let manifest = build::BuildManifest::load()?;

        let stable_hash = manifest
            .stable
            .clone()
            .ok_or_else(|| anyhow::anyhow!("No stable build to rollback to"))?;

        // Write signal file with stable hash to trigger rollback
        let signal_path = crate::storage::jcode_dir()?.join("rollback-signal");
        std::fs::write(&signal_path, &stable_hash)?;

        Ok(ToolOutput::new(format!(
            "Rolling back to stable build {}.\n\n\
             **Restarting...** The session will continue automatically.",
            stable_hash
        )))
    }

    async fn do_debug_enable(&self) -> Result<ToolOutput> {
        use crate::tui::visual_debug;

        visual_debug::enable();

        Ok(ToolOutput::new(
            "Visual debugging enabled. Frames are being captured.\n\n\
             Use `selfdev debug-dump` to write captured frames to file for analysis.\n\
             The debug file will contain detailed frame-by-frame TUI state."
                .to_string(),
        ))
    }

    async fn do_debug_dump(&self) -> Result<ToolOutput> {
        use crate::tui::visual_debug;

        match visual_debug::dump_to_file() {
            Ok(path) => Ok(ToolOutput::new(format!(
                "Debug frames written to: {}\n\n\
                 You can read this file to analyze TUI rendering issues.",
                path.display()
            ))),
            Err(e) => Ok(ToolOutput::new(format!(
                "Failed to dump debug frames: {}\n\n\
                 Make sure visual debugging is enabled first with `selfdev debug`.",
                e
            ))),
        }
    }
}
