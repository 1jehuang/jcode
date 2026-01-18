//! Self-development tool - manage canary builds when working on jcode itself

use crate::build;
use crate::id;
use crate::storage;
use crate::tool::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[derive(Debug, Deserialize)]
struct SelfDevInput {
    action: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    detail: Option<String>,
    #[serde(default)]
    binary: Option<String>,
    #[serde(default)]
    args: Option<Vec<String>>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    lines: Option<usize>,
    #[serde(default)]
    clear: Option<bool>,
    #[serde(default)]
    env: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TesterInfo {
    id: String,
    pid: u32,
    binary: String,
    args: Vec<String>,
    cwd: Option<String>,
    debug_cmd_path: String,
    debug_response_path: String,
    stdout_path: String,
    stderr_path: String,
    started_at: String,
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
         'status' (show build versions), 'rollback' (switch to stable), \
         'debug' (enable visual debug capture), 'debug-dump' (write frames), \
         'spawn-tester' (launch latest self-dev build for manual testing), \
         'tester' (control a spawned tester)."
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
                        "debug",
                        "debug-dump",
                        "spawn-tester",
                        "tester"
                    ],
                    "description": "Action to perform: 'reload' restarts with already-built binary, \
                                   'promote' marks current canary as stable for other sessions, \
                                   'status' shows build versions and any crash history, \
                                   'rollback' manually switches back to the stable build, \
                                   'debug' enables visual debugging for TUI issues, \
                                   'debug-dump' writes captured debug frames to file, \
                                   'spawn-tester' launches latest self-dev build in a separate TUI, \
                                   'tester' controls a spawned tester"
                },
                "message": {
                    "type": "string",
                    "description": "Optional message for promote action or tester send"
                },
                "detail": {
                    "type": "string",
                    "description": "Detail level for tester actions: summary | full"
                },
                "binary": {
                    "type": "string",
                    "description": "Binary path for spawn-tester (defaults to latest canary or target/release/jcode)"
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Extra args for spawn-tester"
                },
                "cwd": {
                    "type": "string",
                    "description": "Working directory for spawn-tester"
                },
                "id": {
                    "type": "string",
                    "description": "Tester id to control"
                },
                "command": {
                    "type": "string",
                    "description": "Tester command: status | list | tail-stdout | tail-stderr | send | response | clear-response | get-state | get-last-response | stop. For 'send', use message param with: message:<text>, reload, state, quit, last_response, history, screen, wait, scroll:<dir>, keys:<spec>, input, set_input:<text>, submit, version, help"
                },
                "lines": {
                    "type": "integer",
                    "description": "Line count for tail commands"
                },
                "clear": {
                    "type": "boolean",
                    "description": "Whether to clear debug response after reading"
                },
                "env": {
                    "type": "object",
                    "description": "Environment variables for spawn-tester"
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
            "spawn-tester" => self.do_spawn_tester(params).await,
            "tester" => self.do_tester_command(params).await,
            _ => Ok(ToolOutput::new(format!(
                "Unknown action: {}. Use 'reload', 'promote', 'status', 'rollback', 'debug', 'debug-dump', 'spawn-tester', or 'tester'.",
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
        let stable_hash = manifest
            .stable
            .clone()
            .unwrap_or_else(|| "none".to_string());
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

    fn tester_manifest_path() -> Result<PathBuf> {
        Ok(storage::jcode_dir()?.join("testers.json"))
    }

    fn load_testers() -> Result<Vec<TesterInfo>> {
        let path = Self::tester_manifest_path()?;
        if path.exists() {
            storage::read_json(&path)
        } else {
            Ok(Vec::new())
        }
    }

    fn save_testers(testers: &[TesterInfo]) -> Result<()> {
        let path = Self::tester_manifest_path()?;
        storage::write_json(&path, testers)
    }

    fn newest_canary_binary() -> Option<PathBuf> {
        if let Ok(path) = build::canary_binary_path() {
            if path.exists() {
                return Some(path);
            }
        }
        None
    }

    fn resolve_binary(binary: Option<String>) -> Result<PathBuf> {
        if let Some(path) = binary {
            return Ok(PathBuf::from(path));
        }
        if let Some(canary) = Self::newest_canary_binary() {
            return Ok(canary);
        }
        if let Some(repo) = build::get_repo_dir() {
            let target = repo.join("target/release/jcode");
            return Ok(target);
        }
        Ok(std::env::current_exe()?)
    }

    fn format_tester_summary(tester: &TesterInfo) -> String {
        let args = if tester.args.is_empty() {
            "(none)".to_string()
        } else {
            tester.args.join(" ")
        };
        format!("{} pid={} bin={} args={}", tester.id, tester.pid, tester.binary, args)
    }

    fn read_tail(path: &PathBuf, lines: usize) -> Result<String> {
        let content = std::fs::read_to_string(path).unwrap_or_default();
        let mut out = String::new();
        let mut total = 0usize;
        for line in content.lines().rev() {
            if total >= lines {
                break;
            }
            out.push_str(line);
            out.push('\n');
            total += 1;
        }
        Ok(out.lines().rev().collect::<Vec<_>>().join("\n"))
    }

    async fn do_spawn_tester(&self, params: SelfDevInput) -> Result<ToolOutput> {
        let binary_path = Self::resolve_binary(params.binary)?;
        if !binary_path.exists() {
            return Ok(ToolOutput::new(format!(
                "Binary not found: {}",
                binary_path.display()
            )));
        }

        let id = format!("tester_{}", id::new_id("tui"));
        let debug_cmd = std::env::temp_dir().join(format!("jcode_debug_cmd_{}", id));
        let debug_resp = std::env::temp_dir().join(format!("jcode_debug_response_{}", id));
        let stdout_path = std::env::temp_dir().join(format!("jcode_tester_stdout_{}", id));
        let stderr_path = std::env::temp_dir().join(format!("jcode_tester_stderr_{}", id));

        let mut cmd = Command::new(&binary_path);
        cmd.env("JCODE_SELFDEV_MODE", "1");
        cmd.env("JCODE_DEBUG_CMD_PATH", debug_cmd.to_string_lossy().to_string());
        cmd.env("JCODE_DEBUG_RESPONSE_PATH", debug_resp.to_string_lossy().to_string());
        if let Some(env) = params.env.as_ref() {
            for (key, value) in env {
                cmd.env(key, value);
            }
        }
        if let Some(cwd) = params.cwd.as_ref() {
            cmd.current_dir(cwd);
        }

        let mut args = params.args.unwrap_or_default();
        if args.is_empty() {
            args.push("self-dev".to_string());
        }
        let has_debug = args.iter().any(|s| s == "--debug-socket");
        cmd.args(&args);
        if !has_debug {
            cmd.arg("--debug-socket");
        }

        let stdout_file = std::fs::File::create(&stdout_path)?;
        let stderr_file = std::fs::File::create(&stderr_path)?;
        cmd.stdout(Stdio::from(stdout_file));
        cmd.stderr(Stdio::from(stderr_file));

        let child = cmd.spawn()?;
        let pid = child.id();

        let info = TesterInfo {
            id: id.clone(),
            pid,
            binary: binary_path.to_string_lossy().to_string(),
            args,
            cwd: params.cwd,
            debug_cmd_path: debug_cmd.to_string_lossy().to_string(),
            debug_response_path: debug_resp.to_string_lossy().to_string(),
            stdout_path: stdout_path.to_string_lossy().to_string(),
            stderr_path: stderr_path.to_string_lossy().to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
        };

        let mut testers = Self::load_testers()?;
        testers.push(info);
        Self::save_testers(&testers)?;

        Ok(ToolOutput::new(format!(
            "Spawned tester {} (pid {}).\nUse selfdev tester command=\"status\" id=\"{}\" to inspect.",
            id, pid, id
        )))
    }

    async fn do_tester_command(&self, params: SelfDevInput) -> Result<ToolOutput> {
        let command = params.command.unwrap_or_else(|| "status".to_string());
        let detail = params.detail.unwrap_or_else(|| "summary".to_string());
        let lines = params.lines.unwrap_or(40).max(1).min(500);

        let mut testers = Self::load_testers()?;
        if command == "list" {
            if testers.is_empty() {
                return Ok(ToolOutput::new("No active testers.".to_string()));
            }
            let entries = testers
                .iter()
                .map(|t| Self::format_tester_summary(t))
                .collect::<Vec<_>>()
                .join("\n");
            return Ok(ToolOutput::new(entries));
        }

        let id = match params.id {
            Some(id) => id,
            None => {
                return Ok(ToolOutput::new(
                    "Missing tester id. Use action=\"tester\" with id.".to_string(),
                ))
            }
        };

        let pos = testers.iter().position(|t| t.id == id);
        let Some(index) = pos else {
            return Ok(ToolOutput::new(format!("Tester not found: {}", id)));
        };

        let tester = testers[index].clone();
        match command.as_str() {
            "status" => {
                let status = if detail == "full" {
                    serde_json::to_string_pretty(&tester).unwrap_or_default()
                } else {
                    Self::format_tester_summary(&tester)
                };
                Ok(ToolOutput::new(status))
            }
            "tail-stdout" => {
                let path = PathBuf::from(&tester.stdout_path);
                let content = Self::read_tail(&path, lines)?;
                Ok(ToolOutput::new(content))
            }
            "tail-stderr" => {
                let path = PathBuf::from(&tester.stderr_path);
                let content = Self::read_tail(&path, lines)?;
                Ok(ToolOutput::new(content))
            }
            "send" => {
                let message = params.message.unwrap_or_default();
                if message.is_empty() {
                    return Ok(ToolOutput::new(
                        "Missing message to send (use message field).".to_string(),
                    ));
                }
                let path = PathBuf::from(&tester.debug_cmd_path);
                std::fs::write(&path, message)?;
                Ok(ToolOutput::new("Sent command.".to_string()))
            }
            "get-state" => {
                let path = PathBuf::from(&tester.debug_cmd_path);
                std::fs::write(&path, "state")?;
                let response_path = PathBuf::from(&tester.debug_response_path);
                let content = std::fs::read_to_string(&response_path).unwrap_or_default();
                Ok(ToolOutput::new(content))
            }
            "get-last-response" => {
                let path = PathBuf::from(&tester.debug_cmd_path);
                std::fs::write(&path, "last_response")?;
                let response_path = PathBuf::from(&tester.debug_response_path);
                let content = std::fs::read_to_string(&response_path).unwrap_or_default();
                Ok(ToolOutput::new(content))
            }
            "response" => {
                let path = PathBuf::from(&tester.debug_response_path);
                let content = std::fs::read_to_string(&path).unwrap_or_default();
                if params.clear.unwrap_or(false) {
                    let _ = std::fs::remove_file(&path);
                }
                Ok(ToolOutput::new(content))
            }
            "clear-response" => {
                let path = PathBuf::from(&tester.debug_response_path);
                let _ = std::fs::remove_file(&path);
                Ok(ToolOutput::new("Cleared response.".to_string()))
            }
            "stop" => {
                #[cfg(unix)]
                {
                    let _ = Command::new("kill")
                        .arg("-TERM")
                        .arg(tester.pid.to_string())
                        .output();
                }
                testers.remove(index);
                Self::save_testers(&testers)?;
                Ok(ToolOutput::new("Stopped tester.".to_string()))
            }
            _ => Ok(ToolOutput::new(format!(
                "Unknown tester command: {}",
                command
            ))),
        }
    }
}
