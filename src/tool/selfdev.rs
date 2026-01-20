//! Self-development tool - manage canary builds when working on jcode itself

use crate::build;
use crate::server;
use crate::storage;
use crate::tool::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
struct SelfDevInput {
    action: String,
    #[serde(default)]
    message: Option<String>,
    /// Optional context for reload - what the agent is working on
    #[serde(default)]
    context: Option<String>,
}

/// Context saved before reload, restored after restart
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReloadContext {
    /// What the agent was working on (user-provided or auto-detected)
    pub task_context: Option<String>,
    /// Version before reload
    pub version_before: String,
    /// New version (target)
    pub version_after: String,
    /// Session ID
    pub session_id: String,
    /// Timestamp
    pub timestamp: String,
    /// Whether this was a rollback
    pub is_rollback: bool,
}

impl ReloadContext {
    pub fn path() -> Result<std::path::PathBuf> {
        Ok(storage::jcode_dir()?.join("reload-context.json"))
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        storage::write_json(&path, self)?;
        Ok(())
    }

    pub fn load() -> Result<Option<Self>> {
        let path = Self::path()?;
        if path.exists() {
            let ctx: Self = storage::read_json(&path)?;
            // Delete after loading (one-time use)
            let _ = std::fs::remove_file(&path);
            Ok(Some(ctx))
        } else {
            Ok(None)
        }
    }
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
         Actions: 'reload' (restart with built binary), 'promote' (mark build as stable), \
         'status' (show build versions), 'rollback' (switch to stable), \
         'socket-info' (debug socket connection info), 'socket-help' (debug socket commands)."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": [
                        "reload",
                        "promote",
                        "status",
                        "rollback",
                        "socket-info",
                        "socket-help"
                    ],
                    "description": "Action to perform: 'reload' restarts with built binary, \
                                   'promote' marks current canary as stable, \
                                   'status' shows build versions and crash history, \
                                   'rollback' switches back to stable build, \
                                   'socket-info' returns debug socket paths and connection info, \
                                   'socket-help' shows available debug socket commands"
                },
                "message": {
                    "type": "string",
                    "description": "Optional message for promote action"
                },
                "context": {
                    "type": "string",
                    "description": "Optional context for reload/rollback - describe what you're working on. \
                                   This will be included in the continuation message after restart."
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: SelfDevInput = serde_json::from_value(input)?;
        let action = params.action.clone();

        let title = format!("selfdev {}", action);

        let result = match action.as_str() {
            "reload" => self.do_reload(params.context, &ctx.session_id).await,
            "promote" => self.do_promote(params.message).await,
            "status" => self.do_status().await,
            "rollback" => self.do_rollback(params.context, &ctx.session_id).await,
            "socket-info" => self.do_socket_info().await,
            "socket-help" => self.do_socket_help().await,
            _ => Ok(ToolOutput::new(format!(
                "Unknown action: {}. Use 'reload', 'promote', 'status', 'rollback', 'socket-info', or 'socket-help'.",
                action
            ))),
        };

        result.map(|output| output.with_title(title))
    }
}

impl SelfDevTool {
    async fn do_reload(&self, context: Option<String>, session_id: &str) -> Result<ToolOutput> {
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
        let version_before = env!("JCODE_VERSION").to_string();

        // Install this version and set as canary (stable stays as safety net)
        build::install_version(&repo_dir, &hash)?;
        build::update_canary_symlink(&hash)?;

        // Update manifest - set as canary, keep stable unchanged
        let mut manifest = build::BuildManifest::load()?;
        let stable_hash = manifest
            .stable
            .clone()
            .unwrap_or_else(|| "none".to_string());
        manifest.canary = Some(hash.clone());
        manifest.canary_status = Some(build::CanaryStatus::Testing);
        manifest.save()?;

        // Save reload context for continuation after restart
        let reload_ctx = ReloadContext {
            task_context: context,
            version_before,
            version_after: hash.clone(),
            session_id: session_id.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            is_rollback: false,
        };
        crate::logging::info(&format!("Saving reload context to {:?}", ReloadContext::path()));
        if let Err(e) = reload_ctx.save() {
            crate::logging::error(&format!("Failed to save reload context: {}", e));
            return Err(e);
        }
        crate::logging::info("Reload context saved successfully");

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

        // Clear canary symlink since it's now promoted to stable
        build::clear_canary_symlink()?;

        // Update manifest - clear all canary state
        manifest.stable = Some(canary_hash.clone());
        manifest.canary = None;
        manifest.canary_session = None;
        manifest.canary_status = None;
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
        status.push_str(&format!("**Running:** jcode {}\n", env!("JCODE_VERSION")));

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

        // Debug socket info
        status.push_str("\n## Debug Socket\n\n");
        status.push_str(&format!("**Path:** {}\n", server::debug_socket_path().display()));

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

    async fn do_rollback(&self, context: Option<String>, session_id: &str) -> Result<ToolOutput> {
        let manifest = build::BuildManifest::load()?;

        let stable_hash = manifest
            .stable
            .clone()
            .ok_or_else(|| anyhow::anyhow!("No stable build to rollback to"))?;

        let version_before = env!("JCODE_VERSION").to_string();

        // Save reload context for continuation after restart
        let reload_ctx = ReloadContext {
            task_context: context,
            version_before,
            version_after: stable_hash.clone(),
            session_id: session_id.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            is_rollback: true,
        };
        reload_ctx.save()?;

        // Write signal file with stable hash to trigger rollback
        let signal_path = crate::storage::jcode_dir()?.join("rollback-signal");
        std::fs::write(&signal_path, &stable_hash)?;

        Ok(ToolOutput::new(format!(
            "Rolling back to stable build {}.\n\n\
             **Restarting...** The session will continue automatically.",
            stable_hash
        )))
    }

    async fn do_socket_info(&self) -> Result<ToolOutput> {
        let debug_socket = server::debug_socket_path();
        let main_socket = server::socket_path();

        let info = json!({
            "debug_socket": debug_socket.to_string_lossy(),
            "main_socket": main_socket.to_string_lossy(),
            "debug_enabled": crate::config::config().display.debug_socket ||
                             std::env::var("JCODE_DEBUG_CONTROL").is_ok() ||
                             crate::storage::jcode_dir().map(|d| d.join("debug_control").exists()).unwrap_or(false),
            "connect_example": format!(
                "echo '{{\"type\":\"debug_command\",\"id\":1,\"command\":\"help\"}}' | nc -U {}",
                debug_socket.display()
            ),
        });

        Ok(ToolOutput::new(format!(
            "## Debug Socket Info\n\n\
             **Debug socket:** {}\n\
             **Main socket:** {}\n\n\
             Use the `debug_socket` tool to send commands, or connect directly:\n\
             ```bash\n\
             echo '{{\"type\":\"debug_command\",\"id\":1,\"command\":\"help\"}}' | nc -U {}\n\
             ```\n\n\
             For programmatic access, use the `debug_socket` tool with the command parameter.",
            debug_socket.display(),
            main_socket.display(),
            debug_socket.display()
        )).with_metadata(info))
    }

    async fn do_socket_help(&self) -> Result<ToolOutput> {
        Ok(ToolOutput::new(r#"## Debug Socket Commands

Commands are namespaced with `server:`, `client:`, or `tester:` prefixes.
Unnamespaced commands default to `server:`.

### Server Commands (agent/tools)
| Command | Description |
|---------|-------------|
| `state` | Agent state (session, model, canary) |
| `history` | Conversation history as JSON |
| `tools` | List available tools |
| `last_response` | Last assistant response |
| `message:<text>` | Send message, get LLM response |
| `tool:<name> <json>` | Execute tool directly |
| `sessions` | List all sessions |
| `create_session` | Create headless session |
| `help` | Full help text |

### Client Commands (TUI/visual debug)
| Command | Description |
|---------|-------------|
| `client:frame` | Get latest visual debug frame (JSON) |
| `client:frame-normalized` | Normalized frame for diffs |
| `client:screen` | Dump frames to file |
| `client:enable` | Enable visual debug capture |
| `client:disable` | Disable visual debug capture |
| `client:status` | Client debug status |

### Tester Commands (spawn test instances)
| Command | Description |
|---------|-------------|
| `tester:spawn` | Spawn new tester instance |
| `tester:spawn {"cwd":"/path"}` | Spawn with options |
| `tester:list` | List active testers |
| `tester:<id>:frame` | Get frame from tester |
| `tester:<id>:state` | Get tester state |
| `tester:<id>:message:<text>` | Send message to tester |
| `tester:<id>:stop` | Stop tester |

Use the `debug_socket` tool to execute these commands directly."#.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reload_context_serialization() {
        // Create test context with task info
        let ctx = ReloadContext {
            task_context: Some("Testing the reload feature".to_string()),
            version_before: "v0.1.100".to_string(),
            version_after: "abc1234".to_string(),
            session_id: "test-session-123".to_string(),
            timestamp: "2025-01-20T00:00:00Z".to_string(),
            is_rollback: false,
        };

        // Serialize and deserialize
        let json = serde_json::to_string(&ctx).unwrap();
        let loaded: ReloadContext = serde_json::from_str(&json).unwrap();

        assert_eq!(loaded.task_context, Some("Testing the reload feature".to_string()));
        assert_eq!(loaded.version_before, "v0.1.100");
        assert_eq!(loaded.version_after, "abc1234");
        assert_eq!(loaded.session_id, "test-session-123");
        assert!(!loaded.is_rollback);
    }

    #[test]
    fn test_reload_context_rollback() {
        let ctx = ReloadContext {
            task_context: None,
            version_before: "canary-xyz".to_string(),
            version_after: "stable-abc".to_string(),
            session_id: "session-456".to_string(),
            timestamp: "2025-01-20T00:00:00Z".to_string(),
            is_rollback: true,
        };

        let json = serde_json::to_string(&ctx).unwrap();
        let loaded: ReloadContext = serde_json::from_str(&json).unwrap();

        assert!(loaded.is_rollback);
        assert!(loaded.task_context.is_none());
    }

    #[test]
    fn test_reload_context_path() {
        // Just verify the path function works
        let path = ReloadContext::path();
        assert!(path.is_ok());
        let path = path.unwrap();
        assert!(path.to_string_lossy().contains("reload-context.json"));
    }
}
