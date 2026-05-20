#![cfg_attr(test, allow(clippy::await_holding_lock))]

//! Self-development tool - manage canary builds when working on jcode itself

use crate::background::{self, TaskResult};
use crate::build;
use crate::bus::BackgroundTaskStatus;
use crate::cli::tui_launch;
use crate::protocol::{ServerEvent, TranscriptMode};
use crate::server;
use crate::session;
use crate::storage;
use crate::tool::{Tool, ToolContext, ToolExecutionMode, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

mod build_queue;
mod launch;
mod reload;
mod status;
#[cfg(test)]
mod tests;

pub use launch::{enter_selfdev_session, schedule_selfdev_prompt_delivery};
pub use reload::{ReloadRecoveryDirective, persisted_background_tasks_note};
pub use status::selfdev_status_output;

#[derive(Debug, Deserialize)]
struct SelfDevInput {
    action: String,
    /// Optional prompt to seed the spawned self-dev session.
    #[serde(default)]
    prompt: Option<String>,
    /// Optional context for reload - what the agent is working on
    #[serde(default)]
    context: Option<String>,
    /// Why this build is needed; shown to other queued/blocked agents.
    #[serde(default)]
    reason: Option<String>,
    /// Build target for selfdev build: auto, tui, desktop, or all.
    #[serde(default)]
    target: Option<String>,
    /// Shell command for selfdev test/check action.
    #[serde(default)]
    command: Option<String>,
    /// Whether to notify the requesting agent when the queued background build completes.
    #[serde(default)]
    notify: Option<bool>,
    /// Whether to wake the requesting agent when the queued background build completes.
    #[serde(default)]
    wake: Option<bool>,
    /// Build request id for actions like cancel-build.
    #[serde(default)]
    request_id: Option<String>,
    /// Background task id for actions like cancel-build.
    #[serde(default)]
    task_id: Option<String>,
}

/// Context saved before reload, restored after restart
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
}

#[derive(Debug, Clone)]
pub struct SelfDevLaunchResult {
    pub session_id: String,
    pub repo_dir: PathBuf,
    pub launched: bool,
    pub test_mode: bool,
    pub exe: Option<PathBuf>,
    pub inherited_context: bool,
}

impl SelfDevLaunchResult {
    pub fn command_preview(&self) -> Option<String> {
        self.exe
            .as_ref()
            .map(|exe| format!("{} --resume {} self-dev", exe.display(), self.session_id))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum BuildRequestState {
    Queued,
    Building,
    Attached,
    Completed,
    Superseded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BuildRequest {
    request_id: String,
    background_task_id: Option<String>,
    session_id: String,
    session_short_name: Option<String>,
    session_title: Option<String>,
    reason: String,
    repo_dir: String,
    #[serde(default)]
    repo_scope: String,
    #[serde(default)]
    worktree_scope: String,
    command: String,
    requested_at: String,
    started_at: Option<String>,
    completed_at: Option<String>,
    state: BuildRequestState,
    version: Option<String>,
    #[serde(default)]
    dedupe_key: Option<String>,
    #[serde(default)]
    requested_source: Option<build::SourceState>,
    #[serde(default)]
    built_source: Option<build::SourceState>,
    #[serde(default)]
    published_version: Option<String>,
    #[serde(default)]
    last_progress: Option<String>,
    #[serde(default)]
    validated: bool,
    error: Option<String>,
    output_file: Option<String>,
    status_file: Option<String>,
    attached_to_request_id: Option<String>,
}

impl BuildRequest {
    fn requests_dir() -> Result<PathBuf> {
        let dir = storage::jcode_dir()?.join("selfdev-build-requests");
        storage::ensure_dir(&dir)?;
        Ok(dir)
    }

    fn path_for_request(request_id: &str) -> Result<PathBuf> {
        Ok(Self::requests_dir()?.join(format!("{}.json", request_id)))
    }

    fn save(&self) -> Result<()> {
        storage::write_json(&Self::path_for_request(&self.request_id)?, self)
    }

    fn load(request_id: &str) -> Result<Option<Self>> {
        let path = Self::path_for_request(request_id)?;
        if path.exists() {
            Ok(Some(storage::read_json(&path)?))
        } else {
            Ok(None)
        }
    }

    fn load_all() -> Result<Vec<Self>> {
        let dir = Self::requests_dir()?;
        let mut requests = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            if let Ok(request) = storage::read_json::<Self>(&path) {
                requests.push(request);
            }
        }
        requests.sort_by(|a, b| {
            a.requested_at
                .cmp(&b.requested_at)
                .then_with(|| a.request_id.cmp(&b.request_id))
        });
        Ok(requests)
    }

    fn pending_requests() -> Result<Vec<Self>> {
        let mut pending = Vec::new();

        for mut request in Self::load_all()? {
            if !matches!(
                request.state,
                BuildRequestState::Queued | BuildRequestState::Building
            ) {
                continue;
            }

            if request.reconcile_pending_state()? {
                pending.push(request);
            }
        }

        Ok(pending)
    }

    fn pending_requests_for_scope(worktree_scope: &str) -> Result<Vec<Self>> {
        Ok(Self::pending_requests()?
            .into_iter()
            .filter(|request| request.worktree_scope == worktree_scope)
            .collect())
    }

    fn attached_watchers(parent_request_id: &str) -> Result<Vec<Self>> {
        Ok(Self::load_all()?
            .into_iter()
            .filter(|request| {
                request.attached_to_request_id.as_deref() == Some(parent_request_id)
                    && request.state == BuildRequestState::Attached
            })
            .collect())
    }

    fn find_duplicate_pending(worktree_scope: &str, dedupe_key: &str) -> Result<Option<Self>> {
        Ok(Self::pending_requests_for_scope(worktree_scope)?
            .into_iter()
            .find(|request| request.dedupe_key.as_deref() == Some(dedupe_key)))
    }

    fn find_by_request_or_task(
        request_id: Option<&str>,
        task_id: Option<&str>,
    ) -> Result<Option<Self>> {
        if let Some(request_id) = request_id {
            return Self::load(request_id);
        }
        let Some(task_id) = task_id else {
            return Ok(None);
        };
        Ok(Self::load_all()?
            .into_iter()
            .find(|request| request.background_task_id.as_deref() == Some(task_id)))
    }

    fn display_owner(&self) -> String {
        if let Some(short_name) = self.session_short_name.as_deref() {
            return format!("{} ({})", short_name, self.session_id);
        }
        if let Some(title) = self.session_title.as_deref() {
            return format!("{} ({})", title, self.session_id);
        }
        self.session_id.clone()
    }

    fn status_path(&self) -> Option<PathBuf> {
        self.status_file.as_ref().map(PathBuf::from).or_else(|| {
            self.background_task_id.as_ref().map(|task_id| {
                std::env::temp_dir()
                    .join("jcode-bg-tasks")
                    .join(format!("{}.status.json", task_id))
            })
        })
    }

    fn mark_stale(&mut self, detail: impl Into<String>) -> Result<()> {
        self.state = BuildRequestState::Failed;
        self.completed_at = Some(Utc::now().to_rfc3339());
        self.error = Some(detail.into());
        self.save()
    }

    fn reconcile_pending_state(&mut self) -> Result<bool> {
        let Some(task_id) = self.background_task_id.as_deref() else {
            self.mark_stale("Self-dev build request is missing its background task id.")?;
            return Ok(false);
        };

        let Some(status_path) = self.status_path() else {
            self.mark_stale("Self-dev build request is missing its task status path.")?;
            return Ok(false);
        };

        let Some(task_status) = (if status_path.exists() && status_path.is_file() {
            storage::read_json::<background::TaskStatusFile>(&status_path).ok()
        } else {
            None
        }) else {
            self.mark_stale(
                "Background task status file is missing; pruning stale self-dev build request.",
            )?;
            return Ok(false);
        };

        match task_status.status {
            BackgroundTaskStatus::Running => {
                if task_status.detached || background::global().is_live_task(task_id) {
                    Ok(true)
                } else {
                    self.mark_stale(
                        "Background task is no longer live; pruning stale self-dev build request.",
                    )?;
                    Ok(false)
                }
            }
            BackgroundTaskStatus::Completed => {
                self.state = BuildRequestState::Completed;
                self.completed_at = task_status
                    .completed_at
                    .clone()
                    .or_else(|| Some(Utc::now().to_rfc3339()));
                self.error = None;
                self.save()?;
                Ok(false)
            }
            BackgroundTaskStatus::Superseded => {
                self.state = BuildRequestState::Superseded;
                self.completed_at = task_status
                    .completed_at
                    .clone()
                    .or_else(|| Some(Utc::now().to_rfc3339()));
                self.error = task_status.error.clone();
                self.save()?;
                Ok(false)
            }
            BackgroundTaskStatus::Failed => {
                self.state = BuildRequestState::Failed;
                self.completed_at = task_status
                    .completed_at
                    .clone()
                    .or_else(|| Some(Utc::now().to_rfc3339()));
                self.error = task_status.error.clone().or_else(|| {
                    Some("Background task failed without an error message.".to_string())
                });
                self.save()?;
                Ok(false)
            }
        }
    }
}

struct BuildLockGuard {
    _file: std::fs::File,
    path: PathBuf,
}

type SelfDevBuildCommand = build::SelfDevBuildCommand;

#[cfg(unix)]
impl Drop for BuildLockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[derive(Default)]
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
        "Manage self-dev builds and reloads."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "intent": super::intent_schema_property(),
                "action": {
                    "type": "string",
                    "enum": [
                        "enter",
                        "build",
                        "test",
                        "cancel-build",
                        "reload",
                        "status",
                        "socket-info",
                        "socket-help"
                    ],
                    "description": "Action."
                },
                "prompt": { "type": "string" },
                "context": { "type": "string" },
                "reason": { "type": "string" },
                "target": {
                    "type": "string",
                    "enum": ["auto", "tui", "desktop", "all"],
                    "description": "Build target for action=build. auto chooses from changed paths; tui builds jcode; desktop builds jcode-desktop; all builds both."
                },
                "command": {
                    "type": "string",
                    "description": "Shell command for action=test. Runs under the selfdev worktree compile lock."
                },
                "request_id": { "type": "string" },
                "task_id": { "type": "string" }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: SelfDevInput = serde_json::from_value(input)?;
        let action = params.action.clone();

        let title = format!("selfdev {}", action);

        let result = match action.as_str() {
            "enter" => self.do_enter(params.prompt, &ctx).await,
            "build" => {
                self.do_build(
                    params.reason,
                    params.target,
                    params.notify,
                    params.wake,
                    &ctx,
                )
                .await
            }
            "test" => {
                self.do_test(
                    params.command,
                    params.reason,
                    params.notify,
                    params.wake,
                    &ctx,
                )
                .await
            }
            "cancel-build" => {
                self.do_cancel_build(params.request_id, params.task_id, &ctx)
                    .await
            }
            "reload" => {
                if !SelfDevTool::session_is_selfdev(&ctx.session_id) {
                    Ok(ToolOutput::new(
                        "`selfdev reload` is only available inside a self-dev session. Use `selfdev enter` first.",
                    ))
                } else {
                    self.do_reload(
                        params.context,
                        &ctx.session_id,
                        ctx.execution_mode,
                        ctx.working_dir.as_deref(),
                    )
                    .await
                }
            }
            "status" => self.do_status().await,
            "socket-info" => {
                if !SelfDevTool::session_is_selfdev(&ctx.session_id) {
                    Ok(ToolOutput::new(
                        "`selfdev socket-info` is only available inside a self-dev session. Use `selfdev enter` first.",
                    ))
                } else {
                    self.do_socket_info().await
                }
            }
            "socket-help" => {
                if !SelfDevTool::session_is_selfdev(&ctx.session_id) {
                    Ok(ToolOutput::new(
                        "`selfdev socket-help` is only available inside a self-dev session. Use `selfdev enter` first.",
                    ))
                } else {
                    self.do_socket_help().await
                }
            }
            _ => Ok(ToolOutput::new(format!(
                "Unknown action: {}. Use 'enter', 'build', 'test', 'cancel-build', 'reload', 'status', 'socket-info', or 'socket-help'.",
                action
            ))),
        };

        result.map(|output| output.with_title(title))
    }
}

impl SelfDevTool {
    fn is_test_session() -> bool {
        std::env::var("JCODE_TEST_SESSION")
            .map(|value| {
                let trimmed = value.trim();
                !trimmed.is_empty() && trimmed != "0" && !trimmed.eq_ignore_ascii_case("false")
            })
            .unwrap_or(false)
    }

    fn reload_timeout_secs() -> u64 {
        std::env::var("JCODE_SELFDEV_RELOAD_TIMEOUT_SECS")
            .ok()
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .filter(|secs| *secs > 0)
            .unwrap_or(15)
    }

    fn session_is_selfdev(session_id: &str) -> bool {
        session::Session::load(session_id)
            .map(|session| session.is_canary)
            .unwrap_or(false)
    }

    fn resolve_repo_dir(working_dir: Option<&std::path::Path>) -> Option<std::path::PathBuf> {
        if let Some(dir) = working_dir {
            for ancestor in dir.ancestors() {
                if build::is_jcode_repo(ancestor) {
                    return Some(ancestor.to_path_buf());
                }
            }
        }

        build::get_repo_dir()
    }

    fn launch_binary() -> Result<std::path::PathBuf> {
        build::client_update_candidate(true)
            .map(|(path, _label)| path)
            .or_else(|| std::env::current_exe().ok())
            .ok_or_else(|| anyhow::anyhow!("Could not resolve jcode executable to launch"))
    }

    fn build_command(repo_dir: &Path, target: build::SelfDevBuildTarget) -> SelfDevBuildCommand {
        build::selfdev_build_command_for_target(repo_dir, target)
    }

    fn build_lock_path(worktree_scope: &str) -> Result<PathBuf> {
        let dir = storage::jcode_dir()?.join("selfdev-build-locks");
        storage::ensure_dir(&dir)?;
        Ok(dir.join(format!("{}.lock", worktree_scope)))
    }

    #[cfg(unix)]
    fn try_acquire_build_lock(worktree_scope: &str) -> Result<Option<BuildLockGuard>> {
        use std::fs::OpenOptions;
        use std::os::fd::AsRawFd;

        let path = Self::build_lock_path(worktree_scope)?;
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)?;
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if ret == 0 {
            Ok(Some(BuildLockGuard { _file: file, path }))
        } else {
            Ok(None)
        }
    }

    #[cfg(not(unix))]
    fn try_acquire_build_lock(worktree_scope: &str) -> Result<Option<BuildLockGuard>> {
        use std::fs::OpenOptions;

        let path = Self::build_lock_path(worktree_scope)?;
        match OpenOptions::new().create_new(true).write(true).open(&path) {
            Ok(file) => Ok(Some(BuildLockGuard { _file: file, path })),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    fn load_session_labels(session_id: &str) -> (Option<String>, Option<String>) {
        session::Session::load(session_id)
            .map(|session| {
                let title = session.display_title().map(ToOwned::to_owned);
                (session.short_name, title)
            })
            .unwrap_or((None, None))
    }

    fn requested_source_state(repo_dir: &Path) -> Result<build::SourceState> {
        if Self::is_test_session() {
            return Ok(build::SourceState {
                repo_scope: "test-repo-scope".to_string(),
                worktree_scope: "test-worktree-scope".to_string(),
                short_hash: "test-build".to_string(),
                full_hash: "test-build-full".to_string(),
                dirty: true,
                fingerprint: "test-fingerprint".to_string(),
                version_label: "test-build".to_string(),
                changed_paths: 0,
            });
        }
        build::current_source_state(repo_dir)
    }

    fn newest_active_request(worktree_scope: &str) -> Result<Option<BuildRequest>> {
        Ok(BuildRequest::pending_requests_for_scope(worktree_scope)?
            .into_iter()
            .find(|request| request.state == BuildRequestState::Building))
    }

    fn build_dedupe_key(source: &build::SourceState, command: &SelfDevBuildCommand) -> String {
        format!(
            "{}:{}:{}",
            source.worktree_scope, source.fingerprint, command.display
        )
    }

    fn next_request_id() -> String {
        format!("selfdev-build-{}", uuid::Uuid::new_v4().simple())
    }

    fn current_queue_position(request_id: &str, worktree_scope: &str) -> Result<Option<usize>> {
        Ok(BuildRequest::pending_requests_for_scope(worktree_scope)?
            .into_iter()
            .position(|request| request.request_id == request_id)
            .map(|index| index + 1))
    }
}
