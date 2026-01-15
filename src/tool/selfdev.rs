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
         Actions: 'rebuild' (build, test, restart with new code), 'promote' (mark current build as stable), \
         'status' (show build versions and crash history), 'rollback' (switch back to stable)."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["rebuild", "promote", "status", "rollback"],
                    "description": "Action to perform: 'rebuild' builds and tests changes then restarts, \
                                   'promote' marks current canary as stable for other sessions, \
                                   'status' shows current build versions and any crash history, \
                                   'rollback' manually switches back to the stable build"
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
            "rebuild" => self.do_rebuild().await,
            "promote" => self.do_promote(params.message).await,
            "status" => self.do_status().await,
            "rollback" => self.do_rollback().await,
            _ => Ok(ToolOutput::new(format!(
                "Unknown action: {}. Use 'rebuild', 'promote', 'status', or 'rollback'.",
                params.action
            ))),
        }
    }
}

impl SelfDevTool {
    async fn do_rebuild(&self) -> Result<ToolOutput> {
        let repo_dir = build::get_repo_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find jcode repository directory"))?;

        // Do the build
        match build::rebuild_canary(&repo_dir) {
            Ok(hash) => {
                // Write signal file for TUI to pick up
                let signal_path = crate::storage::jcode_dir()?.join("rebuild-signal");
                std::fs::write(&signal_path, &hash)?;

                Ok(ToolOutput::new(format!(
                    "Build successful ({}). Tests passed.\n\n\
                     **Restarting with new build...** The session will continue automatically.",
                    hash
                )))
            }
            Err(e) => Ok(ToolOutput::new(format!(
                "Build failed: {}\n\nFix the issue and try again.",
                e
            ))),
        }
    }

    async fn do_promote(&self, message: Option<String>) -> Result<ToolOutput> {
        let mut manifest = build::BuildManifest::load()?;

        let canary_hash = manifest.canary.clone()
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

        // Current versions
        status.push_str("## Build Status\n\n");

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
                    i + 1, info.hash, msg, dirty_marker
                ));
            }
        }

        Ok(ToolOutput::new(status))
    }

    async fn do_rollback(&self) -> Result<ToolOutput> {
        let manifest = build::BuildManifest::load()?;

        let stable_hash = manifest.stable.clone()
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
}
