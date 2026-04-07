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

pub fn enter_selfdev_session(
    parent_session_id: Option<&str>,
    working_dir: Option<&Path>,
) -> Result<SelfDevLaunchResult> {
    let repo_dir = SelfDevTool::resolve_repo_dir(working_dir).ok_or_else(|| {
        anyhow::anyhow!("Could not find the jcode repository to enter self-dev mode")
    })?;

    let mut inherited_context = false;
    let mut session = if let Some(parent_session_id) = parent_session_id {
        match session::Session::load(parent_session_id) {
            Ok(parent) => {
                let mut child = session::Session::create(
                    Some(parent_session_id.to_string()),
                    Some("Self-development session".to_string()),
                );
                child.replace_messages(parent.messages.clone());
                child.compaction = parent.compaction.clone();
                child.model = parent.model.clone();
                child.provider_key = parent.provider_key.clone();
                child.subagent_model = parent.subagent_model.clone();
                child.improve_mode = parent.improve_mode;
                child.autoreview_enabled = parent.autoreview_enabled;
                child.autojudge_enabled = parent.autojudge_enabled;
                child.memory_injections = parent.memory_injections.clone();
                child.replay_events = parent.replay_events.clone();
                inherited_context = true;
                child
            }
            Err(err) => {
                crate::logging::warn(&format!(
                    "Failed to load parent session {} for self-dev enter; starting fresh session: {}",
                    parent_session_id, err
                ));
                session::Session::create(None, Some("Self-development session".to_string()))
            }
        }
    } else {
        session::Session::create(None, Some("Self-development session".to_string()))
    };
    session.set_canary("self-dev");
    session.working_dir = Some(repo_dir.display().to_string());
    session.status = session::SessionStatus::Closed;
    session.save()?;

    let session_id = session.id.clone();

    if SelfDevTool::is_test_session() {
        return Ok(SelfDevLaunchResult {
            session_id,
            repo_dir,
            launched: false,
            test_mode: true,
            exe: None,
            inherited_context,
        });
    }

    let exe = SelfDevTool::launch_binary()?;
    let launched = tui_launch::spawn_selfdev_in_new_terminal(&exe, &session_id, &repo_dir)?;

    Ok(SelfDevLaunchResult {
        session_id,
        repo_dir,
        launched,
        test_mode: false,
        exe: Some(exe),
        inherited_context,
    })
}

pub fn schedule_selfdev_prompt_delivery(session_id: String, prompt: String) {
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();
        match runtime {
            Ok(runtime) => {
                if let Err(err) =
                    runtime.block_on(SelfDevTool::send_prompt_to_session(&session_id, &prompt))
                {
                    crate::logging::warn(&format!(
                        "Failed to auto-deliver prompt to spawned self-dev session {}: {}",
                        session_id, err
                    ));
                }
            }
            Err(err) => crate::logging::warn(&format!(
                "Failed to initialize runtime for self-dev prompt delivery: {}",
                err
            )),
        }
    });
}

pub fn selfdev_status_output() -> Result<ToolOutput> {
    let manifest = build::BuildManifest::load()?;

    let mut status = String::new();

    status.push_str("## Current Version\n\n");
    status.push_str(&format!("**Running:** jcode {}\n", env!("JCODE_VERSION")));

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

    status.push_str("\n## Build Channels\n\n");

    if let Ok(Some(current)) = build::read_current_version() {
        status.push_str(&format!("**Current:** {}\n", current));
    } else {
        status.push_str("**Current:** none\n");
    }

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

    if let Some(pending) = manifest.pending_activation.as_ref() {
        status.push_str(&format!(
            "**Pending activation:** {} for session `{}`\n",
            pending.new_version, pending.session_id
        ));
        if let Some(previous) = pending.previous_current_version.as_deref() {
            status.push_str(&format!("**Rollback target:** {}\n", previous));
        }
        if let Some(fingerprint) = pending.source_fingerprint.as_deref() {
            status.push_str(&format!(
                "**Pending source fingerprint:** `{}`\n",
                fingerprint
            ));
        }
    }

    status.push_str("\n## Debug Socket\n\n");
    status.push_str(&format!(
        "**Path:** {}\n",
        server::debug_socket_path().display()
    ));

    if let Some(reload_state) = server::ReloadState::load() {
        status.push_str("\n## Reload State\n\n");
        status.push_str(&format!(
            "**Phase:** {:?}\n**Request:** {}\n**Hash:** {}\n**PID:** {}\n**Timestamp:** {}\n",
            reload_state.phase,
            reload_state.request_id,
            reload_state.hash,
            reload_state.pid,
            reload_state.timestamp,
        ));
        if let Some(detail) = reload_state.detail {
            status.push_str(&format!("**Detail:** {}\n", detail));
        }
    }

    let pending_requests = BuildRequest::pending_requests()?;
    if !pending_requests.is_empty() {
        status.push_str("\n## Build Queue\n\n");
        for (index, request) in pending_requests.iter().enumerate() {
            let watchers = BuildRequest::attached_watchers(&request.request_id)?;
            let state = match request.state {
                BuildRequestState::Queued => "queued",
                BuildRequestState::Building => "building",
                BuildRequestState::Attached => "attached",
                BuildRequestState::Completed => "completed",
                BuildRequestState::Superseded => "superseded",
                BuildRequestState::Failed => "failed",
                BuildRequestState::Cancelled => "cancelled",
            };
            status.push_str(&format!(
                "{}. **{}** — {}\n   Reason: {}\n   Requested: {}\n",
                index + 1,
                state,
                request.display_owner(),
                request.reason,
                request.requested_at,
            ));
            if let Some(version) = request.version.as_deref() {
                status.push_str(&format!("   Target version: `{}`\n", version));
            }
            if let Some(source) = request.requested_source.as_ref() {
                status.push_str(&format!(
                    "   Source fingerprint: `{}` (dirty={}, changed_paths={})\n",
                    source.fingerprint, source.dirty, source.changed_paths
                ));
            }
            if let Some(progress) = request.last_progress.as_deref() {
                status.push_str(&format!("   Progress: {}\n", progress));
            }
            if let Some(task_id) = request.background_task_id.as_deref() {
                status.push_str(&format!("   Task: `{}`\n", task_id));
            }
            if let Some(started_at) = request.started_at.as_deref() {
                status.push_str(&format!("   Started: {}\n", started_at));
            }
            if let Some(published) = request.published_version.as_deref() {
                status.push_str(&format!("   Published version: `{}`\n", published));
            }
            status.push_str(&format!("   Validated: {}\n", request.validated));
            if !watchers.is_empty() {
                let watcher_names = watchers
                    .iter()
                    .map(BuildRequest::display_owner)
                    .collect::<Vec<_>>()
                    .join(", ");
                status.push_str(&format!(
                    "   Attached watchers: {} ({})\n",
                    watchers.len(),
                    watcher_names
                ));
            }
        }
    }

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
                format!("{}...", crate::util::truncate_str(&crash.stderr, 500))
            } else {
                crash.stderr.clone()
            };
            status.push_str(&format!("\nStderr:\n```\n{}\n```\n", stderr_preview));
        }
    }

    if !manifest.history.is_empty() {
        status.push_str("\n## Recent Builds\n\n");
        for (i, info) in manifest.history.iter().take(5).enumerate() {
            let dirty_marker = if info.dirty { " (dirty)" } else { "" };
            let msg = info
                .commit_message
                .as_deref()
                .unwrap_or("No commit message");
            status.push_str(&format!(
                "{}. `{}`{} - {}\n   Built: {}\n",
                i + 1,
                info.hash,
                dirty_marker,
                msg,
                info.built_at.format("%Y-%m-%d %H:%M:%S UTC")
            ));
        }
    }

    Ok(ToolOutput::new(status))
}

impl ReloadContext {
    fn sanitize_session_id(session_id: &str) -> String {
        session_id
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                    ch
                } else {
                    '_'
                }
            })
            .collect()
    }

    pub fn path_for_session(session_id: &str) -> Result<std::path::PathBuf> {
        let sanitized = Self::sanitize_session_id(session_id);
        Ok(storage::jcode_dir()?.join(format!("reload-context-{}.json", sanitized)))
    }

    fn legacy_path() -> Result<std::path::PathBuf> {
        Ok(storage::jcode_dir()?.join("reload-context.json"))
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path_for_session(&self.session_id)?;
        storage::write_json(&path, self)?;
        Ok(())
    }

    pub fn load() -> Result<Option<Self>> {
        let legacy = Self::legacy_path()?;
        if !legacy.exists() {
            return Ok(None);
        }
        let ctx: Self = storage::read_json(&legacy)?;
        let _ = std::fs::remove_file(&legacy);
        Ok(Some(ctx))
    }

    /// Peek at context for a specific session without consuming it.
    pub fn peek_for_session(session_id: &str) -> Result<Option<Self>> {
        let session_path = Self::path_for_session(session_id)?;
        if session_path.exists() {
            let ctx: Self = storage::read_json(&session_path)?;
            return Ok(Some(ctx));
        }

        let legacy = Self::legacy_path()?;
        if !legacy.exists() {
            return Ok(None);
        }

        let ctx: Self = storage::read_json(&legacy)?;
        if ctx.session_id == session_id {
            Ok(Some(ctx))
        } else {
            Ok(None)
        }
    }

    /// Load context only if it belongs to the given session; consumes on success.
    pub fn load_for_session(session_id: &str) -> Result<Option<Self>> {
        let session_path = Self::path_for_session(session_id)?;
        if session_path.exists() {
            let ctx: Self = storage::read_json(&session_path)?;
            let _ = std::fs::remove_file(&session_path);
            return Ok(Some(ctx));
        }

        let legacy = Self::legacy_path()?;
        if !legacy.exists() {
            return Ok(None);
        }

        let ctx: Self = storage::read_json(&legacy)?;
        if ctx.session_id == session_id {
            let _ = std::fs::remove_file(&legacy);
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
        "Self-development tool for working on jcode itself. Actions: 'enter' (spawn a new self-dev session), \
         'build' (queue a background self-dev build with a reason/comment), 'cancel-build' (cancel a queued/running self-dev build request), \
         'status' (show build versions), and in self-dev mode also 'reload', 'socket-info', and 'socket-help'."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                        "enum": [
                        "enter",
                        "build",
                        "cancel-build",
                        "reload",
                        "status",
                        "socket-info",
                        "socket-help"
                    ],
                    "description": "Action to perform: 'enter' spawns a new self-dev session, \
                                   'build' queues a coordinated background build with a reason/comment, \
                                   'cancel-build' cancels a queued/running self-dev build request, \
                                   'reload' restarts with built binary, \
                                   'status' shows build versions and crash history, \
                                   'socket-info' returns debug socket paths and connection info, \
                                   'socket-help' shows available debug socket commands"
                },
                "prompt": {
                    "type": "string",
                    "description": "Optional prompt to send into the spawned self-dev session after it opens"
                },
                "context": {
                    "type": "string",
                    "description": "Optional context for reload - describe what you're working on. \
                                   This will be included in the continuation message after restart."
                },
                "reason": {
                    "type": "string",
                    "description": "Why this self-dev build is needed. Required for action='build' so other queued agents can see the reason."
                },
                "notify": {
                    "type": "boolean",
                    "description": "For action='build': notify the requesting agent when the queued background build completes (default: true)."
                },
                "wake": {
                    "type": "boolean",
                    "description": "For action='build': wake the requesting agent when the queued background build completes. Defaults to true."
                },
                "request_id": {
                    "type": "string",
                    "description": "For action='cancel-build': build request id to cancel."
                },
                "task_id": {
                    "type": "string",
                    "description": "For action='cancel-build': background task id to cancel if request id is not known."
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
            "enter" => self.do_enter(params.prompt, &ctx).await,
            "build" => {
                self.do_build(params.reason, params.notify, params.wake, &ctx)
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
                    self.do_reload(params.context, &ctx.session_id, ctx.execution_mode)
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
                "Unknown action: {}. Use 'enter', 'build', 'cancel-build', 'reload', 'status', 'socket-info', or 'socket-help'.",
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

    fn build_command(repo_dir: &Path) -> SelfDevBuildCommand {
        build::selfdev_build_command(repo_dir)
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
            .map(|session| (session.short_name, session.title))
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

    async fn append_output_line(file: &mut tokio::fs::File, line: impl AsRef<str>) {
        let _ = file.write_all(line.as_ref().as_bytes()).await;
        let _ = file.write_all(b"\n").await;
        let _ = file.flush().await;
    }

    async fn wait_for_turn(
        request_id: &str,
        worktree_scope: &str,
        file: &mut tokio::fs::File,
    ) -> Result<BuildLockGuard> {
        let mut last_note: Option<String> = None;
        loop {
            let pending = BuildRequest::pending_requests_for_scope(worktree_scope)?;
            let my_index = pending
                .iter()
                .position(|request| request.request_id == request_id)
                .ok_or_else(|| {
                    anyhow::anyhow!("Queued build request {} disappeared", request_id)
                })?;

            if my_index == 0 {
                if let Some(lock) = Self::try_acquire_build_lock(worktree_scope)? {
                    return Ok(lock);
                }
            }

            let note = if my_index == 0 {
                Some("Waiting for the self-dev build lock to become available".to_string())
            } else {
                pending.get(my_index - 1).map(|request| {
                    format!(
                        "Waiting in queue behind {} — {}",
                        request.display_owner(),
                        request.reason
                    )
                })
            };
            if note.as_ref() != last_note.as_ref() {
                if let Some(note) = note.as_ref() {
                    Self::append_output_line(file, note).await;
                }
                last_note = note;
            }

            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }

    async fn stream_build_command(
        repo_dir: PathBuf,
        command: SelfDevBuildCommand,
        output_path: PathBuf,
    ) -> Result<TaskResult> {
        let mut cmd = tokio::process::Command::new(&command.program);
        cmd.args(&command.args)
            .current_dir(repo_dir)
            .kill_on_drop(true)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to spawn build command: {}", e))?;

        let mut file = tokio::fs::File::create(&output_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create output file: {}", e))?;
        Self::append_output_line(
            &mut file,
            format!("Starting build with {}", command.display),
        )
        .await;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let mut stdout_lines = stdout.map(|s| BufReader::new(s).lines());
        let mut stderr_lines = stderr.map(|s| BufReader::new(s).lines());
        let mut stdout_done = stdout_lines.is_none();
        let mut stderr_done = stderr_lines.is_none();

        while !stdout_done || !stderr_done {
            tokio::select! {
                line = async {
                    match stdout_lines.as_mut() {
                        Some(r) => r.next_line().await,
                        None => std::future::pending().await,
                    }
                }, if !stdout_done => {
                    match line {
                        Ok(Some(line)) => Self::append_output_line(&mut file, line).await,
                        _ => stdout_done = true,
                    }
                }
                line = async {
                    match stderr_lines.as_mut() {
                        Some(r) => r.next_line().await,
                        None => std::future::pending().await,
                    }
                }, if !stderr_done => {
                    match line {
                        Ok(Some(line)) => Self::append_output_line(&mut file, format!("[stderr] {}", line)).await,
                        _ => stderr_done = true,
                    }
                }
            }
        }

        let status = child.wait().await?;
        let exit_code = status.code();
        Self::append_output_line(
            &mut file,
            format!(
                "--- Command finished with exit code: {} ---",
                exit_code.unwrap_or(-1)
            ),
        )
        .await;

        if status.success() {
            Ok(TaskResult::completed(exit_code))
        } else {
            Ok(TaskResult::failed(
                exit_code,
                format!("Command exited with code {}", exit_code.unwrap_or(-1)),
            ))
        }
    }

    async fn run_test_build(output_path: PathBuf, reason: &str) -> Result<TaskResult> {
        let mut file = tokio::fs::File::create(&output_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create output file: {}", e))?;
        Self::append_output_line(
            &mut file,
            format!("[test mode] Simulated selfdev build for reason: {}", reason),
        )
        .await;
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        Self::append_output_line(&mut file, "--- Command finished with exit code: 0 ---").await;
        Ok(TaskResult::completed(Some(0)))
    }

    async fn follow_existing_build(
        request_id: String,
        original_request_id: String,
        output_path: PathBuf,
    ) -> Result<TaskResult> {
        let mut file = tokio::fs::File::create(&output_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create output file: {}", e))?;
        Self::append_output_line(
            &mut file,
            format!(
                "Attached to existing selfdev build request {} instead of spawning a duplicate build.",
                original_request_id
            ),
        )
        .await;

        loop {
            let Some(original) = BuildRequest::load(&original_request_id)? else {
                anyhow::bail!("Original build request {} disappeared", original_request_id);
            };
            match original.state {
                BuildRequestState::Queued | BuildRequestState::Building => {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
                BuildRequestState::Completed => {
                    let mut request = BuildRequest::load(&request_id)?.ok_or_else(|| {
                        anyhow::anyhow!("Attached build request {} disappeared", request_id)
                    })?;
                    request.state = BuildRequestState::Completed;
                    request.completed_at = Some(Utc::now().to_rfc3339());
                    request.error = None;
                    request.save()?;
                    Self::append_output_line(
                        &mut file,
                        format!(
                            "Original build {} completed successfully.",
                            original_request_id
                        ),
                    )
                    .await;
                    return Ok(TaskResult::completed(Some(0)));
                }
                BuildRequestState::Superseded => {
                    let mut request = BuildRequest::load(&request_id)?.ok_or_else(|| {
                        anyhow::anyhow!("Attached build request {} disappeared", request_id)
                    })?;
                    request.state = BuildRequestState::Superseded;
                    request.completed_at = Some(Utc::now().to_rfc3339());
                    request.error = original.error.clone();
                    request.save()?;
                    let detail = original.error.clone().unwrap_or_else(|| {
                        format!(
                            "Original build {} completed but was superseded before activation",
                            original_request_id
                        )
                    });
                    Self::append_output_line(&mut file, &detail).await;
                    return Ok(TaskResult::superseded(Some(0), detail));
                }
                BuildRequestState::Failed | BuildRequestState::Cancelled => {
                    let mut request = BuildRequest::load(&request_id)?.ok_or_else(|| {
                        anyhow::anyhow!("Attached build request {} disappeared", request_id)
                    })?;
                    request.state = original.state.clone();
                    request.completed_at = Some(Utc::now().to_rfc3339());
                    request.error = original.error.clone();
                    request.save()?;
                    let error = original.error.clone().unwrap_or_else(|| {
                        format!("Original build {} did not complete", original_request_id)
                    });
                    Self::append_output_line(&mut file, &error).await;
                    return Ok(TaskResult::failed(None, error));
                }
                BuildRequestState::Attached => {
                    anyhow::bail!(
                        "Original build request {} is attached, not build-producing",
                        original_request_id
                    );
                }
            }
        }
    }

    async fn run_build_request(
        request_id: String,
        repo_dir: PathBuf,
        command: SelfDevBuildCommand,
        reason: String,
        output_path: PathBuf,
    ) -> Result<TaskResult> {
        let mut request = BuildRequest::load(&request_id)?
            .ok_or_else(|| anyhow::anyhow!("Missing queued build request {}", request_id))?;
        let mut queue_file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&output_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to open output file: {}", e))?;

        let worktree_scope = request.worktree_scope.clone();
        let _lock = Self::wait_for_turn(&request_id, &worktree_scope, &mut queue_file).await?;
        let expected_source = request
            .requested_source
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Missing requested source state for {}", request_id))?;
        let actual_source = if Self::is_test_session() {
            expected_source.clone()
        } else {
            build::ensure_source_state_matches(&repo_dir, &expected_source)?
        };
        request.state = BuildRequestState::Building;
        request.started_at = Some(Utc::now().to_rfc3339());
        request.version = Some(expected_source.version_label.clone());
        request.built_source = Some(actual_source.clone());
        request.last_progress = Some("building".to_string());
        request.save()?;
        Self::append_output_line(&mut queue_file, format!("Build starting now: {}", reason)).await;
        drop(queue_file);

        let result = if Self::is_test_session() {
            Self::run_test_build(output_path.clone(), &reason).await?
        } else {
            let result =
                Self::stream_build_command(repo_dir.clone(), command.clone(), output_path.clone())
                    .await?;
            if result.error.is_none() {
                match build::ensure_source_state_matches(&repo_dir, &expected_source) {
                    Ok(source_after_build) => {
                        let published = build::publish_local_current_build_for_source(
                            &repo_dir,
                            &source_after_build,
                        )?;
                        let mut manifest = build::BuildManifest::load()?;
                        manifest.add_to_history(build::current_build_info(&repo_dir)?)?;
                        let mut request = BuildRequest::load(&request_id)?.ok_or_else(|| {
                            anyhow::anyhow!("Missing queued build request {}", request_id)
                        })?;
                        request.published_version = Some(published.version.clone());
                        request.validated = true;
                        request.last_progress = Some("published and smoke-tested".to_string());
                        request.save()?;
                        result
                    }
                    Err(err) => {
                        let detail = format!(
                            "Build completed successfully, but the source changed before activation. Marking this result as superseded instead of failed. {}",
                            err
                        );
                        let mut file = tokio::fs::OpenOptions::new()
                            .append(true)
                            .open(&output_path)
                            .await
                            .map_err(|e| anyhow::anyhow!("Failed to append output note: {}", e))?;
                        Self::append_output_line(&mut file, &detail).await;
                        TaskResult::superseded(result.exit_code.or(Some(0)), detail)
                    }
                }
            } else {
                result
            }
        };

        let mut request = BuildRequest::load(&request_id)?
            .ok_or_else(|| anyhow::anyhow!("Missing queued build request {}", request_id))?;
        request.completed_at = Some(Utc::now().to_rfc3339());
        request.state = match result.status.as_ref().unwrap_or(&BackgroundTaskStatus::Failed) {
            BackgroundTaskStatus::Completed => BuildRequestState::Completed,
            BackgroundTaskStatus::Superseded => BuildRequestState::Superseded,
            BackgroundTaskStatus::Failed => BuildRequestState::Failed,
            BackgroundTaskStatus::Running => BuildRequestState::Building,
        };
        request.error = result.error.clone();
        request.last_progress = match request.state {
            BuildRequestState::Completed => request
                .last_progress
                .take()
                .or_else(|| Some("completed".to_string())),
            BuildRequestState::Superseded => Some("superseded by newer source state".to_string()),
            BuildRequestState::Failed => Some("failed".to_string()),
            BuildRequestState::Building => Some("building".to_string()),
            BuildRequestState::Queued => Some("queued".to_string()),
            BuildRequestState::Attached => Some("attached".to_string()),
            BuildRequestState::Cancelled => Some("cancelled".to_string()),
        };
        request.save()?;
        Ok(result)
    }

    async fn send_prompt_to_session(session_id: &str, prompt: &str) -> Result<()> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        let mut last_error: Option<String> = None;

        while std::time::Instant::now() < deadline {
            match Self::try_send_prompt_once(session_id, prompt).await {
                Ok(()) => return Ok(()),
                Err(err) => {
                    last_error = Some(err.to_string());
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
            }
        }

        Err(anyhow::anyhow!(
            "Timed out delivering prompt to spawned self-dev session {}: {}",
            session_id,
            last_error.unwrap_or_else(|| "unknown error".to_string())
        ))
    }

    async fn try_send_prompt_once(session_id: &str, prompt: &str) -> Result<()> {
        let mut client = server::Client::connect_debug().await?;
        let request_id = client
            .send_transcript(prompt, TranscriptMode::Send, Some(session_id.to_string()))
            .await?;

        loop {
            match client.read_event().await? {
                ServerEvent::Ack { id } if id == request_id => {}
                ServerEvent::Done { id } if id == request_id => return Ok(()),
                ServerEvent::Error { id, message, .. } if id == request_id => {
                    anyhow::bail!(message)
                }
                _ => {}
            }
        }
    }

    async fn do_build(
        &self,
        reason: Option<String>,
        notify: Option<bool>,
        wake: Option<bool>,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let reason = reason
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "`selfdev build` requires a non-empty `reason` so other queued agents can see why the build is needed."
                )
            })?;
        let repo_dir =
            SelfDevTool::resolve_repo_dir(ctx.working_dir.as_deref()).ok_or_else(|| {
                anyhow::anyhow!("Could not find the jcode repository directory for selfdev build")
            })?;

        let requested_source = SelfDevTool::requested_source_state(&repo_dir)?;
        let command = SelfDevTool::build_command(&repo_dir);
        let dedupe_key = SelfDevTool::build_dedupe_key(&requested_source, &command);
        let blocker = SelfDevTool::newest_active_request(&requested_source.worktree_scope)?;
        let duplicate =
            BuildRequest::find_duplicate_pending(&requested_source.worktree_scope, &dedupe_key)?;
        let (session_short_name, session_title) = SelfDevTool::load_session_labels(&ctx.session_id);
        let request_id = SelfDevTool::next_request_id();
        let wake = wake.unwrap_or(true);
        let notify = notify.unwrap_or(true) || wake;

        if let Some(existing) = duplicate {
            let mut request = BuildRequest {
                request_id: request_id.clone(),
                background_task_id: None,
                session_id: ctx.session_id.clone(),
                session_short_name,
                session_title,
                reason: reason.clone(),
                repo_dir: repo_dir.display().to_string(),
                repo_scope: requested_source.repo_scope.clone(),
                worktree_scope: requested_source.worktree_scope.clone(),
                command: command.display.clone(),
                requested_at: Utc::now().to_rfc3339(),
                started_at: None,
                completed_at: None,
                state: BuildRequestState::Attached,
                version: Some(requested_source.version_label.clone()),
                dedupe_key: Some(dedupe_key.clone()),
                requested_source: Some(requested_source.clone()),
                built_source: None,
                published_version: None,
                last_progress: Some("attached to existing build".to_string()),
                validated: false,
                error: None,
                output_file: None,
                status_file: None,
                attached_to_request_id: Some(existing.request_id.clone()),
            };
            request.save()?;

            let request_id_for_task = request_id.clone();
            let existing_request_id = existing.request_id.clone();
            let info = background::global()
                .spawn_with_notify(
                    "selfdev-build-watch",
                    &ctx.session_id,
                    notify,
                    wake,
                    move |output_path| async move {
                        SelfDevTool::follow_existing_build(
                            request_id_for_task,
                            existing_request_id,
                            output_path,
                        )
                        .await
                    },
                )
                .await;

            request.background_task_id = Some(info.task_id.clone());
            request.output_file = Some(info.output_file.display().to_string());
            request.status_file = Some(info.status_file.display().to_string());
            request.save()?;

            let delivery = if wake {
                "The requesting agent will be woken when the existing build finishes."
            } else if notify {
                "You will be notified when the existing build finishes."
            } else {
                "Completion delivery is disabled for this watcher."
            };
            let output = format!(
                "Matching self-dev build already queued/running, so this request was attached instead of spawning a duplicate build.\n\n- Your request ID: `{}`\n- Watcher task ID: `{}`\n- Existing request: `{}`\n- Requested by: {}\n- Reason: {}\n- Target version: `{}`\n- Source fingerprint: `{}`\n\n{}",
                request_id,
                info.task_id,
                existing.request_id,
                existing.display_owner(),
                existing.reason,
                requested_source.version_label,
                requested_source.fingerprint,
                delivery
            );

            return Ok(ToolOutput::new(output).with_metadata(json!({
                "background": true,
                "deduped": true,
                "request_id": request_id,
                "task_id": info.task_id,
                "output_file": info.output_file.to_string_lossy(),
                "status_file": info.status_file.to_string_lossy(),
                "duplicate_of": {
                    "request_id": existing.request_id,
                    "task_id": existing.background_task_id,
                    "session_id": existing.session_id,
                    "session_short_name": existing.session_short_name,
                    "session_title": existing.session_title,
                    "reason": existing.reason,
                    "version": existing.version,
                    "source_fingerprint": existing
                        .requested_source
                        .as_ref()
                        .map(|source| source.fingerprint.clone()),
                }
            })));
        }

        let mut request = BuildRequest {
            request_id: request_id.clone(),
            background_task_id: None,
            session_id: ctx.session_id.clone(),
            session_short_name,
            session_title,
            reason: reason.clone(),
            repo_dir: repo_dir.display().to_string(),
            repo_scope: requested_source.repo_scope.clone(),
            worktree_scope: requested_source.worktree_scope.clone(),
            command: command.display.clone(),
            requested_at: Utc::now().to_rfc3339(),
            started_at: None,
            completed_at: None,
            state: BuildRequestState::Queued,
            version: Some(requested_source.version_label.clone()),
            dedupe_key: Some(dedupe_key),
            requested_source: Some(requested_source.clone()),
            built_source: None,
            published_version: None,
            last_progress: Some("queued".to_string()),
            validated: false,
            error: None,
            output_file: None,
            status_file: None,
            attached_to_request_id: None,
        };
        request.save()?;

        let queue_position =
            SelfDevTool::current_queue_position(&request_id, &requested_source.worktree_scope)?
                .unwrap_or(1);

        let request_id_for_task = request_id.clone();
        let repo_dir_for_task = repo_dir.clone();
        let command_for_task = command.clone();
        let reason_for_task = reason.clone();
        let info = background::global()
            .spawn_with_notify(
                "selfdev-build",
                &ctx.session_id,
                notify,
                wake,
                move |output_path| async move {
                    SelfDevTool::run_build_request(
                        request_id_for_task,
                        repo_dir_for_task,
                        command_for_task,
                        reason_for_task,
                        output_path,
                    )
                    .await
                },
            )
            .await;

        request.background_task_id = Some(info.task_id.clone());
        request.output_file = Some(info.output_file.display().to_string());
        request.status_file = Some(info.status_file.display().to_string());
        request.save()?;
        let delivery = if wake {
            "The requesting agent will be woken when the build completes."
        } else if notify {
            "You will be notified when the build completes."
        } else {
            "Completion delivery is disabled for this build request."
        };
        let mut output = format!(
            "Self-dev build queued in background.\n\n- Request ID: `{}`\n- Task ID: `{}`\n- Reason: {}\n- Target version: `{}`\n- Source fingerprint: `{}`\n- Command: `{}`\n- Queue position: {}\n- Output file: `{}`\n- Status file: `{}`\n\n{}",
            request_id,
            info.task_id,
            reason,
            requested_source.version_label,
            requested_source.fingerprint,
            command.display,
            queue_position,
            info.output_file.display(),
            info.status_file.display(),
            delivery
        );

        if let Some(ref blocker) = blocker {
            output.push_str(&format!(
                "\n\nCurrently blocked by: {}\nReason: {}",
                blocker.display_owner(),
                blocker.reason
            ));
        }

        output.push_str(&format!(
            "\n\nUse `bg action=\"status\" task_id=\"{}\"` to check progress, or `selfdev status` to inspect the build queue.\nAfter it finishes, use `selfdev reload` when you want to restart onto the new binary.",
            info.task_id
        ));

        Ok(ToolOutput::new(output).with_metadata(json!({
            "background": true,
            "request_id": request_id,
            "task_id": info.task_id,
            "output_file": info.output_file.to_string_lossy(),
            "status_file": info.status_file.to_string_lossy(),
            "queue_position": queue_position,
            "version": requested_source.version_label,
            "source_fingerprint": requested_source.fingerprint,
            "blocked_by": blocker.as_ref().map(|request| json!({
                "session_id": request.session_id,
                "session_short_name": request.session_short_name,
                "session_title": request.session_title,
                "reason": request.reason,
                "version": request.version,
                "source_fingerprint": request
                    .requested_source
                    .as_ref()
                    .map(|source| source.fingerprint.clone()),
            }))
        })))
    }

    async fn do_cancel_build(
        &self,
        request_id: Option<String>,
        task_id: Option<String>,
        ctx: &ToolContext,
    ) -> Result<ToolOutput> {
        let Some(mut request) =
            BuildRequest::find_by_request_or_task(request_id.as_deref(), task_id.as_deref())?
        else {
            return Ok(ToolOutput::new(
                "No self-dev build request matched the provided request_id/task_id.",
            ));
        };

        if request.session_id != ctx.session_id {
            return Ok(ToolOutput::new(format!(
                "That self-dev build request belongs to {}, not this session ({}).",
                request.display_owner(),
                ctx.session_id
            )));
        }

        if matches!(
            request.state,
            BuildRequestState::Completed | BuildRequestState::Failed | BuildRequestState::Cancelled
        ) {
            return Ok(ToolOutput::new(format!(
                "Build request `{}` is already in terminal state `{}`.",
                request.request_id,
                match request.state {
                    BuildRequestState::Completed => "completed",
                    BuildRequestState::Failed => "failed",
                    BuildRequestState::Cancelled => "cancelled",
                    _ => unreachable!(),
                }
            )));
        }

        let cancelled_task = if let Some(task_id) = request.background_task_id.as_deref() {
            background::global().cancel(task_id).await?
        } else {
            false
        };

        request.state = BuildRequestState::Cancelled;
        request.completed_at = Some(Utc::now().to_rfc3339());
        request.error = Some("Cancelled by user".to_string());
        request.save()?;

        Ok(ToolOutput::new(format!(
            "Cancelled self-dev build request `{}`.\n\n- Task cancelled: {}\n- Reason: {}\n- Target version: {}",
            request.request_id,
            if cancelled_task { "yes" } else { "no (task may have already finished)" },
            request.reason,
            request.version.as_deref().unwrap_or("unknown")
        ))
        .with_metadata(json!({
            "request_id": request.request_id,
            "task_id": request.background_task_id,
            "cancelled": true,
            "cancelled_task": cancelled_task,
        })))
    }

    async fn do_enter(&self, prompt: Option<String>, ctx: &ToolContext) -> Result<ToolOutput> {
        let launch = enter_selfdev_session(Some(&ctx.session_id), ctx.working_dir.as_deref())?;

        if launch.test_mode {
            let mut output = format!(
                "Created self-dev session {} in {}.\n\nTest mode skipped launching a new terminal.",
                launch.session_id,
                launch.repo_dir.display()
            );
            if let Some(prompt) = prompt {
                output.push_str(&format!(
                    "\n\nSeed prompt captured ({} chars) but not delivered in test mode.",
                    prompt.chars().count()
                ));
            }
            return Ok(ToolOutput::new(output).with_metadata(json!({
                "session_id": launch.session_id,
                "repo_dir": launch.repo_dir,
                "launched": false,
                "test_mode": true,
                "inherited_context": launch.inherited_context
            })));
        }

        if !launch.launched {
            let command_preview = launch
                .command_preview()
                .unwrap_or_else(|| format!("jcode --resume {} self-dev", launch.session_id));
            return Ok(ToolOutput::new(format!(
                "Created self-dev session {} but could not find a supported terminal to spawn automatically.\n\nRun manually:\n`{} --resume {} self-dev`",
                launch.session_id,
                launch.exe.as_ref().map(|exe| exe.display().to_string()).unwrap_or_else(|| "jcode".to_string()),
                launch.session_id
            ))
            .with_metadata(json!({
                "session_id": launch.session_id,
                "repo_dir": launch.repo_dir,
                "launched": false,
                "inherited_context": launch.inherited_context
            }))
            .with_title(format!("selfdev enter: {}", command_preview)));
        }

        let mut output = format!(
            "Spawned a new self-dev session in a separate terminal.\n\n- Session: `{}`\n- Repo: `{}`\n- Command: `{} --resume {} self-dev`",
            launch.session_id,
            launch.repo_dir.display(),
            launch
                .exe
                .as_ref()
                .map(|exe| exe.display().to_string())
                .unwrap_or_else(|| "jcode".to_string()),
            launch.session_id
        );

        let prompt_delivery = if let Some(prompt_text) = prompt {
            match SelfDevTool::send_prompt_to_session(&launch.session_id, &prompt_text).await {
                Ok(()) => {
                    output.push_str("\n- Prompt: delivered to the spawned self-dev session");
                    Some(true)
                }
                Err(err) => {
                    output.push_str(&format!("\n- Prompt: failed to auto-deliver ({})", err));
                    Some(false)
                }
            }
        } else {
            None
        };

        if launch.inherited_context {
            output.push_str("\n- Context: cloned from the current session");
        }

        Ok(ToolOutput::new(output).with_metadata(json!({
            "session_id": launch.session_id,
            "repo_dir": launch.repo_dir,
            "launched": true,
            "prompt_delivered": prompt_delivery,
            "inherited_context": launch.inherited_context
        })))
    }

    async fn do_reload(
        &self,
        context: Option<String>,
        session_id: &str,
        execution_mode: ToolExecutionMode,
    ) -> Result<ToolOutput> {
        let repo_dir = build::get_repo_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find jcode repository directory"))?;

        let target_binary = build::find_dev_binary(&repo_dir)
            .unwrap_or_else(|| build::release_binary_path(&repo_dir));
        if !target_binary.exists() {
            return Ok(ToolOutput::new(
                format!(
                    "No binary found at {}.\n\
                     Run 'jcode self-dev --build' first, or build with 'scripts/dev_cargo.sh build --release --bin jcode' and then try reload again.",
                    target_binary.display()
                )
                .to_string(),
            ));
        }

        let source = if SelfDevTool::is_test_session() {
            build::SourceState {
                repo_scope: "test-repo-scope".to_string(),
                worktree_scope: "test-worktree-scope".to_string(),
                short_hash: "test-reload-hash".to_string(),
                full_hash: "test-reload-hash-full".to_string(),
                dirty: true,
                fingerprint: "test-reload-fingerprint".to_string(),
                version_label: "test-reload-hash".to_string(),
                changed_paths: 0,
            }
        } else {
            build::current_source_state(&repo_dir)?
        };
        let hash = source.version_label.clone();
        let version_before = env!("JCODE_VERSION").to_string();
        let published = if SelfDevTool::is_test_session() {
            None
        } else {
            Some(build::publish_local_current_build_for_source(
                &repo_dir, &source,
            )?)
        };

        // Update manifest - track what we're testing
        let mut manifest = build::BuildManifest::load()?;
        manifest.canary = Some(hash.clone());
        manifest.canary_status = Some(build::CanaryStatus::Testing);
        manifest.set_pending_activation(build::PendingActivation {
            session_id: session_id.to_string(),
            new_version: hash.clone(),
            previous_current_version: published
                .as_ref()
                .and_then(|published| published.previous_current_version.clone()),
            source_fingerprint: Some(source.fingerprint.clone()),
            requested_at: chrono::Utc::now(),
        })?;
        manifest.save()?;

        // Save reload context for continuation after restart
        let reload_ctx = ReloadContext {
            task_context: context,
            version_before,
            version_after: hash.clone(),
            session_id: session_id.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        crate::logging::info(&format!(
            "Saving reload context to {:?}",
            ReloadContext::path_for_session(session_id)
        ));
        if let Err(e) = reload_ctx.save() {
            crate::logging::error(&format!("Failed to save reload context: {}", e));
            return Err(e);
        }
        crate::logging::info("Reload context saved successfully");

        // Write reload info for post-restart display
        let info_path = crate::storage::jcode_dir()?.join("reload-info");
        let info = format!("reload:{}", hash);
        std::fs::write(&info_path, &info)?;

        // Signal the server via in-process channel (replaces filesystem-based rebuild-signal)
        let request_id =
            server::send_reload_signal(hash.clone(), Some(session_id.to_string()), true);
        crate::logging::info(&format!(
            "selfdev reload: request={} session_id={} hash={} execution_mode={:?}",
            request_id, session_id, hash, execution_mode
        ));

        let timeout = std::time::Duration::from_secs(SelfDevTool::reload_timeout_secs());
        let ack_wait_started = std::time::Instant::now();
        let ack = server::wait_for_reload_ack(&request_id, timeout)
            .await
            .map_err(|error| {
                let _ = build::rollback_pending_activation_for_session(session_id);
                anyhow::anyhow!(
                    "Timed out waiting for the server to begin reload after {}s: {}. The reload signal may not have been picked up; check that the connected server is running a build with unified self-dev reload support and try restarting the shared server.",
                    timeout.as_secs(),
                    error
                )
            })?;

        crate::logging::info(&format!(
            "selfdev reload: acked request={} hash={} after {}ms state={}",
            ack.request_id,
            ack.hash,
            ack_wait_started.elapsed().as_millis(),
            server::reload_state_summary(std::time::Duration::from_secs(60))
        ));

        match execution_mode {
            ToolExecutionMode::Direct => {
                if SelfDevTool::is_test_session() {
                    return Ok(ToolOutput::new(format!(
                        "Reload acknowledged for build {}. Server is restarting now.",
                        ack.hash
                    )));
                }
                match server::await_reload_handoff(&server::socket_path(), timeout).await {
                    server::ReloadWaitStatus::Ready => {
                        let _ = build::complete_pending_activation_for_session(session_id);
                        Ok(ToolOutput::new(format!(
                            "Reload completed successfully for build {}. Server reported ready.",
                            ack.hash
                        )))
                    }
                    server::ReloadWaitStatus::Failed(detail) => {
                        let _ = build::rollback_pending_activation_for_session(session_id);
                        Err(anyhow::anyhow!(
                            "Reload was acknowledged for build {}, but the replacement server failed before becoming ready: {}",
                            ack.hash,
                            detail.unwrap_or_else(|| "unknown reload failure".to_string())
                        ))
                    }
                    server::ReloadWaitStatus::Idle | server::ReloadWaitStatus::Waiting { .. } => {
                        let _ = build::rollback_pending_activation_for_session(session_id);
                        Err(anyhow::anyhow!(
                            "Reload was acknowledged for build {}, but readiness could not be confirmed within {}s.",
                            ack.hash,
                            timeout.as_secs()
                        ))
                    }
                }
            }
            ToolExecutionMode::AgentTurn => {
                let sleep_forever = async {
                    loop {
                        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    }
                };

                match tokio::time::timeout(timeout, sleep_forever).await {
                    Ok(_) => unreachable!("infinite wait future unexpectedly completed"),
                    Err(_) => {
                        crate::logging::warn(&format!(
                            "selfdev reload: request={} not interrupted after {}ms state={} ",
                            ack.request_id,
                            timeout.as_millis(),
                            server::reload_state_summary(std::time::Duration::from_secs(60))
                        ));
                        Err(anyhow::anyhow!(
                            "Reload was acknowledged by the server for build {}, but this tool execution was not interrupted within {}s. The server restart may be stuck; inspect logs and active sessions. Current reload state: {}",
                            ack.hash,
                            timeout.as_secs(),
                            server::reload_state_summary(std::time::Duration::from_secs(60))
                        ))
                    }
                }
            }
        }
    }

    async fn do_status(&self) -> Result<ToolOutput> {
        selfdev_status_output()
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
        ))
        .with_metadata(info))
    }

    async fn do_socket_help(&self) -> Result<ToolOutput> {
        Ok(ToolOutput::new(
            r#"## Debug Socket Commands

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
| `client:scroll-test[:<json>]` | Run offscreen scroll+diagram test |
| `client:scroll-suite[:<json>]` | Run scroll+diagram test suite |

### Tester Commands (spawn test instances)
| Command | Description |
|---------|-------------|
| `tester:spawn` | Spawn new tester instance |
| `tester:spawn {"cwd":"/path"}` | Spawn with options |
| `tester:list` | List active testers |
| `tester:<id>:frame` | Get frame from tester |
| `tester:<id>:state` | Get tester state |
| `tester:<id>:message:<text>` | Send message to tester |
| `tester:<id>:scroll-test[:<json>]` | Run offscreen scroll+diagram test |
| `tester:<id>:scroll-suite[:<json>]` | Run scroll+diagram test suite |
| `tester:<id>:stop` | Stop tester |

Use the `debug_socket` tool to execute these commands directly."#
                .to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::BackgroundTaskStatus;
    use std::ffi::OsStr;
    use std::sync::{LazyLock, Mutex};

    static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    struct EnvVarGuard {
        key: &'static str,
        original: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
            let original = std::env::var_os(key);
            crate::env::set_var(key, value);
            Self { key, original }
        }

        fn remove(key: &'static str) -> Self {
            let original = std::env::var_os(key);
            crate::env::remove_var(key);
            Self { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.original {
                Some(value) => crate::env::set_var(self.key, value),
                None => crate::env::remove_var(self.key),
            }
        }
    }

    fn create_test_context(
        session_id: &str,
        working_dir: Option<std::path::PathBuf>,
    ) -> ToolContext {
        ToolContext {
            session_id: session_id.to_string(),
            message_id: "test-message".to_string(),
            tool_call_id: "test-tool-call".to_string(),
            working_dir,
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: crate::tool::ToolExecutionMode::Direct,
        }
    }

    fn create_repo_fixture() -> tempfile::TempDir {
        let temp = tempfile::TempDir::new().expect("temp repo");
        std::fs::create_dir_all(temp.path().join(".git")).expect("git dir");
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"jcode\"\nversion = \"0.1.0\"\n",
        )
        .expect("cargo toml");
        temp
    }

    fn test_source_state(repo_dir: &std::path::Path) -> build::SourceState {
        build::SourceState {
            repo_scope: "test-repo-scope".to_string(),
            worktree_scope: build::worktree_scope_key(repo_dir)
                .unwrap_or_else(|_| "test-worktree".to_string()),
            short_hash: "test-build".to_string(),
            full_hash: "test-build-full".to_string(),
            dirty: true,
            fingerprint: "test-fingerprint".to_string(),
            version_label: "test-build".to_string(),
            changed_paths: 0,
        }
    }

    async fn wait_for_task_completion(task_id: &str) -> background::TaskStatusFile {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(status) = background::global().status(task_id).await {
                if status.status != BackgroundTaskStatus::Running {
                    return status;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for background task {}",
                task_id
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    #[test]
    fn test_reload_context_serialization() {
        // Create test context with task info
        let ctx = ReloadContext {
            task_context: Some("Testing the reload feature".to_string()),
            version_before: "v0.1.100".to_string(),
            version_after: "abc1234".to_string(),
            session_id: "test-session-123".to_string(),
            timestamp: "2025-01-20T00:00:00Z".to_string(),
        };

        // Serialize and deserialize
        let json = serde_json::to_string(&ctx).unwrap();
        let loaded: ReloadContext = serde_json::from_str(&json).unwrap();

        assert_eq!(
            loaded.task_context,
            Some("Testing the reload feature".to_string())
        );
        assert_eq!(loaded.version_before, "v0.1.100");
        assert_eq!(loaded.version_after, "abc1234");
        assert_eq!(loaded.session_id, "test-session-123");
    }

    #[test]
    fn test_reload_context_path() {
        // Just verify the session-scoped path function works
        let path = ReloadContext::path_for_session("test-session-123");
        assert!(path.is_ok());
        let path = path.unwrap();
        let path_str = path.to_string_lossy();
        assert!(path_str.contains("reload-context-test-session-123.json"));
    }

    #[test]
    fn test_reload_context_save_and_load_for_session_uses_session_scoped_file() {
        let _storage_guard = crate::storage::lock_test_env();
        let _lock = lock_env();
        let temp_home = tempfile::TempDir::new().expect("temp home");
        let _home_guard = EnvVarGuard::set("JCODE_HOME", temp_home.path());

        let ctx = ReloadContext {
            task_context: Some("Testing scoped reload context".to_string()),
            version_before: "v0.1.100".to_string(),
            version_after: "abc1234".to_string(),
            session_id: "test-session-123".to_string(),
            timestamp: "2025-01-20T00:00:00Z".to_string(),
        };

        ctx.save().expect("save reload context");

        let path = ReloadContext::path_for_session("test-session-123").expect("context path");
        assert!(
            path.exists(),
            "session-scoped reload context file should exist"
        );

        let peeked = ReloadContext::peek_for_session("test-session-123")
            .expect("peek should succeed")
            .expect("context should exist");
        assert_eq!(peeked.session_id, "test-session-123");

        let loaded = ReloadContext::load_for_session("test-session-123")
            .expect("load should succeed")
            .expect("context should exist");
        assert_eq!(loaded.session_id, "test-session-123");
        assert!(
            !path.exists(),
            "load_for_session should consume the context file"
        );
    }

    #[test]
    fn reload_timeout_secs_defaults_to_15() {
        let _storage_guard = crate::storage::lock_test_env();
        let _lock = lock_env();
        let _guard = EnvVarGuard::remove("JCODE_SELFDEV_RELOAD_TIMEOUT_SECS");
        assert_eq!(SelfDevTool::reload_timeout_secs(), 15);
    }

    #[test]
    fn reload_timeout_secs_honors_valid_env_override() {
        let _storage_guard = crate::storage::lock_test_env();
        let _lock = lock_env();
        let _guard = EnvVarGuard::set("JCODE_SELFDEV_RELOAD_TIMEOUT_SECS", "27");
        assert_eq!(SelfDevTool::reload_timeout_secs(), 27);
    }

    #[test]
    fn reload_timeout_secs_ignores_empty_invalid_and_zero_values() {
        let _storage_guard = crate::storage::lock_test_env();
        let _lock = lock_env();
        let _guard = EnvVarGuard::set("JCODE_SELFDEV_RELOAD_TIMEOUT_SECS", "   ");
        assert_eq!(SelfDevTool::reload_timeout_secs(), 15);
        drop(_guard);

        let _guard = EnvVarGuard::set("JCODE_SELFDEV_RELOAD_TIMEOUT_SECS", "abc");
        assert_eq!(SelfDevTool::reload_timeout_secs(), 15);
        drop(_guard);

        let _guard = EnvVarGuard::set("JCODE_SELFDEV_RELOAD_TIMEOUT_SECS", "0");
        assert_eq!(SelfDevTool::reload_timeout_secs(), 15);
    }

    #[tokio::test]
    async fn do_reload_returns_after_ack_in_direct_mode() {
        let request_id = server::send_reload_signal("direct-hash".to_string(), None, true);
        let waiter = tokio::spawn({
            let request_id = request_id.clone();
            async move {
                server::wait_for_reload_ack(&request_id, std::time::Duration::from_secs(1)).await
            }
        });

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        server::acknowledge_reload_signal(&crate::server::ReloadSignal {
            hash: "direct-hash".to_string(),
            triggering_session: None,
            prefer_selfdev_binary: true,
            request_id: "ignored".to_string(),
        });

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        server::acknowledge_reload_signal(&crate::server::ReloadSignal {
            hash: "direct-hash".to_string(),
            triggering_session: None,
            prefer_selfdev_binary: true,
            request_id,
        });

        let ack = waiter
            .await
            .expect("waiter task should complete")
            .expect("ack should be received");
        assert_eq!(ack.hash, "direct-hash");
    }

    #[tokio::test]
    async fn enter_creates_selfdev_session_in_test_mode() {
        let _storage_guard = crate::storage::lock_test_env();
        let _lock = lock_env();
        let temp_home = tempfile::TempDir::new().expect("temp home");
        let _home_guard = EnvVarGuard::set("JCODE_HOME", temp_home.path());
        let _test_guard = EnvVarGuard::set("JCODE_TEST_SESSION", "1");
        let repo = create_repo_fixture();

        let mut parent = session::Session::create(None, Some("Origin Session".to_string()));
        parent.working_dir = Some("/tmp/origin-project".to_string());
        parent.model = Some("gpt-test".to_string());
        parent.provider_key = Some("openai".to_string());
        parent.subagent_model = Some("gpt-subagent".to_string());
        parent.add_message(
            crate::message::Role::User,
            vec![crate::message::ContentBlock::Text {
                text: "hello from parent".to_string(),
                cache_control: None,
            }],
        );
        parent.compaction = Some(session::StoredCompactionState {
            summary_text: "summary".to_string(),
            openai_encrypted_content: None,
            covers_up_to_turn: 1,
            original_turn_count: 1,
            compacted_count: 1,
        });
        parent.record_replay_display_message("system", None, "remember this context");
        parent.save().expect("save parent session");

        let tool = SelfDevTool::new();
        let ctx = create_test_context(&parent.id, Some(repo.path().to_path_buf()));
        let output = tool
            .execute(
                json!({"action": "enter", "prompt": "Work on jcode itself"}),
                ctx,
            )
            .await
            .expect("selfdev enter should succeed in test mode");

        assert!(output.output.contains("Created self-dev session"));
        assert!(
            output
                .output
                .contains("Test mode skipped launching a new terminal")
        );
        assert!(
            output.output.contains("Seed prompt captured"),
            "test-mode enter should still report captured prompt"
        );

        let metadata = output.metadata.expect("metadata");
        let session_id = metadata["session_id"]
            .as_str()
            .expect("session id metadata");
        assert_eq!(metadata["inherited_context"].as_bool(), Some(true));
        let session = session::Session::load(session_id).expect("load spawned session");
        assert!(
            session.is_canary,
            "spawned session should be canary/self-dev"
        );
        assert_eq!(session.testing_build.as_deref(), Some("self-dev"));
        assert_eq!(
            session.working_dir.as_deref(),
            Some(repo.path().to_string_lossy().as_ref())
        );
        assert_eq!(session.parent_id.as_deref(), Some(parent.id.as_str()));
        assert_eq!(session.messages.len(), parent.messages.len());
        assert_eq!(session.messages[0].content_preview(), "hello from parent");
        assert_eq!(session.compaction, parent.compaction);
        assert_eq!(session.model, parent.model);
        assert_eq!(session.provider_key, parent.provider_key);
        assert_eq!(session.subagent_model, parent.subagent_model);
        assert_eq!(session.replay_events, parent.replay_events);
    }

    #[tokio::test]
    async fn enter_falls_back_to_fresh_session_when_parent_missing() {
        let _storage_guard = crate::storage::lock_test_env();
        let _lock = lock_env();
        let temp_home = tempfile::TempDir::new().expect("temp home");
        let _home_guard = EnvVarGuard::set("JCODE_HOME", temp_home.path());
        let _test_guard = EnvVarGuard::set("JCODE_TEST_SESSION", "1");
        let repo = create_repo_fixture();

        let tool = SelfDevTool::new();
        let ctx = create_test_context("missing-parent", Some(repo.path().to_path_buf()));
        let output = tool
            .execute(json!({"action": "enter"}), ctx)
            .await
            .expect("selfdev enter should succeed without a persisted parent session");

        let metadata = output.metadata.expect("metadata");
        let session_id = metadata["session_id"]
            .as_str()
            .expect("session id metadata");
        assert_eq!(metadata["inherited_context"].as_bool(), Some(false));

        let session = session::Session::load(session_id).expect("load spawned session");
        assert!(session.messages.is_empty());
        assert!(session.parent_id.is_none());
        assert_eq!(
            session.working_dir.as_deref(),
            Some(repo.path().to_string_lossy().as_ref())
        );
    }

    #[tokio::test]
    async fn reload_requires_selfdev_session() {
        let _storage_guard = crate::storage::lock_test_env();
        let _lock = lock_env();
        let temp_home = tempfile::TempDir::new().expect("temp home");
        let _home_guard = EnvVarGuard::set("JCODE_HOME", temp_home.path());

        let mut session = session::Session::create(None, Some("Normal Session".to_string()));
        session.save().expect("save session");

        let tool = SelfDevTool::new();
        let ctx = create_test_context(&session.id, session.working_dir.clone().map(Into::into));
        let output = tool
            .execute(json!({"action": "reload"}), ctx)
            .await
            .expect("reload should return guidance instead of failing");

        assert!(
            output
                .output
                .contains("only available inside a self-dev session")
        );
        assert!(output.output.contains("selfdev enter"));
    }

    #[tokio::test]
    async fn build_requires_reason() {
        let _storage_guard = crate::storage::lock_test_env();
        let _lock = lock_env();
        let temp_home = tempfile::TempDir::new().expect("temp home");
        let _home_guard = EnvVarGuard::set("JCODE_HOME", temp_home.path());
        let _test_guard = EnvVarGuard::set("JCODE_TEST_SESSION", "1");
        let repo = create_repo_fixture();

        let tool = SelfDevTool::new();
        let ctx = create_test_context("build-session", Some(repo.path().to_path_buf()));
        let err = tool
            .execute(json!({"action": "build"}), ctx)
            .await
            .expect_err("build without reason should fail");

        assert!(err.to_string().contains("requires a non-empty `reason`"));
    }

    #[tokio::test]
    async fn build_queues_background_tasks_and_reports_queue_status() {
        let _storage_guard = crate::storage::lock_test_env();
        let _lock = lock_env();
        let temp_home = tempfile::TempDir::new().expect("temp home");
        let _home_guard = EnvVarGuard::set("JCODE_HOME", temp_home.path());
        let _test_guard = EnvVarGuard::set("JCODE_TEST_SESSION", "1");
        let repo = create_repo_fixture();

        let mut session_one =
            session::Session::create(None, Some("First build session".to_string()));
        session_one.short_name = Some("alpha".to_string());
        session_one.save().expect("save session one");

        let mut session_two =
            session::Session::create(None, Some("Second build session".to_string()));
        session_two.short_name = Some("beta".to_string());
        session_two.save().expect("save session two");

        let tool = SelfDevTool::new();
        let first = tool
            .execute(
                json!({"action": "build", "reason": "first reason"}),
                create_test_context(&session_one.id, Some(repo.path().to_path_buf())),
            )
            .await
            .expect("first build should queue");
        let second = tool
            .execute(
                json!({"action": "build", "reason": "second reason"}),
                create_test_context(&session_two.id, Some(repo.path().to_path_buf())),
            )
            .await
            .expect("second build should queue");

        let first_meta = first.metadata.expect("first metadata");
        let second_meta = second.metadata.expect("second metadata");
        let first_task_id = first_meta["task_id"].as_str().expect("first task id");
        let second_task_id = second_meta["task_id"].as_str().expect("second task id");

        assert_eq!(first_meta["queue_position"].as_u64(), Some(1));
        assert_eq!(second_meta["deduped"].as_bool(), Some(true));
        assert!(
            second
                .output
                .contains("attached instead of spawning a duplicate build")
        );

        let status_output = selfdev_status_output().expect("status output");
        assert!(status_output.output.contains("## Build Queue"));
        assert!(status_output.output.contains("first reason"));
        assert!(status_output.output.contains("Attached watchers: 1"));
        assert!(
            status_output
                .output
                .contains("Target version: `test-build`")
        );

        let first_status = wait_for_task_completion(first_task_id).await;
        let second_status = wait_for_task_completion(second_task_id).await;
        assert_eq!(first_status.status, BackgroundTaskStatus::Completed);
        assert_eq!(second_status.status, BackgroundTaskStatus::Completed);

        let request_one =
            BuildRequest::load(first_meta["request_id"].as_str().expect("first request id"))
                .expect("load request one")
                .expect("request one exists");
        let request_two = BuildRequest::load(
            second_meta["request_id"]
                .as_str()
                .expect("second request id"),
        )
        .expect("load request two")
        .expect("request two exists");
        assert_eq!(request_one.state, BuildRequestState::Completed);
        assert_eq!(request_two.state, BuildRequestState::Completed);
    }

    #[tokio::test]
    async fn build_dedupes_identical_reason_and_version_with_attached_watcher() {
        let _storage_guard = crate::storage::lock_test_env();
        let _lock = lock_env();
        let temp_home = tempfile::TempDir::new().expect("temp home");
        let _home_guard = EnvVarGuard::set("JCODE_HOME", temp_home.path());
        let _test_guard = EnvVarGuard::set("JCODE_TEST_SESSION", "1");
        let repo = create_repo_fixture();

        let mut session_one = session::Session::create(None, Some("Build A".to_string()));
        session_one.short_name = Some("alpha".to_string());
        session_one.save().expect("save session one");

        let mut session_two = session::Session::create(None, Some("Build B".to_string()));
        session_two.short_name = Some("beta".to_string());
        session_two.save().expect("save session two");

        let tool = SelfDevTool::new();
        let first = tool
            .execute(
                json!({"action": "build", "reason": "same reason"}),
                create_test_context(&session_one.id, Some(repo.path().to_path_buf())),
            )
            .await
            .expect("first build should queue");
        let second = tool
            .execute(
                json!({"action": "build", "reason": "same reason"}),
                create_test_context(&session_two.id, Some(repo.path().to_path_buf())),
            )
            .await
            .expect("second build should attach");

        let first_meta = first.metadata.expect("first metadata");
        let second_meta = second.metadata.expect("second metadata");
        assert_eq!(second_meta["deduped"].as_bool(), Some(true));
        assert_eq!(
            second_meta["duplicate_of"]["request_id"].as_str(),
            first_meta["request_id"].as_str()
        );

        let status_output = selfdev_status_output().expect("status output");
        assert!(status_output.output.contains("Attached watchers: 1"));
        assert!(status_output.output.contains("alpha"));
        assert!(status_output.output.contains("beta"));

        let first_status = wait_for_task_completion(first_meta["task_id"].as_str().unwrap()).await;
        let second_status =
            wait_for_task_completion(second_meta["task_id"].as_str().unwrap()).await;
        assert_eq!(first_status.status, BackgroundTaskStatus::Completed);
        assert_eq!(second_status.status, BackgroundTaskStatus::Completed);

        let watcher_request = BuildRequest::load(second_meta["request_id"].as_str().unwrap())
            .expect("load watcher request")
            .expect("watcher request exists");
        assert_eq!(watcher_request.state, BuildRequestState::Completed);
        assert_eq!(
            watcher_request.attached_to_request_id.as_deref(),
            first_meta["request_id"].as_str()
        );
    }

    #[tokio::test]
    async fn cancel_build_marks_request_cancelled_and_removes_it_from_queue() {
        let _storage_guard = crate::storage::lock_test_env();
        let _lock = lock_env();
        let temp_home = tempfile::TempDir::new().expect("temp home");
        let _home_guard = EnvVarGuard::set("JCODE_HOME", temp_home.path());
        let _test_guard = EnvVarGuard::set("JCODE_TEST_SESSION", "1");
        let repo = create_repo_fixture();

        let mut session_one = session::Session::create(None, Some("Build A".to_string()));
        session_one.short_name = Some("alpha".to_string());
        session_one.save().expect("save session one");

        let mut session_two = session::Session::create(None, Some("Build B".to_string()));
        session_two.short_name = Some("beta".to_string());
        session_two.save().expect("save session two");

        let tool = SelfDevTool::new();
        let first = tool
            .execute(
                json!({"action": "build", "reason": "keep building"}),
                create_test_context(&session_one.id, Some(repo.path().to_path_buf())),
            )
            .await
            .expect("first build should queue");
        let second = tool
            .execute(
                json!({"action": "build", "reason": "cancel me"}),
                create_test_context(&session_two.id, Some(repo.path().to_path_buf())),
            )
            .await
            .expect("second build should queue");

        let second_meta = second.metadata.expect("second metadata");
        let cancel = tool
            .execute(
                json!({
                    "action": "cancel-build",
                    "request_id": second_meta["request_id"].as_str().unwrap()
                }),
                create_test_context(&session_two.id, Some(repo.path().to_path_buf())),
            )
            .await
            .expect("cancel should succeed");

        assert!(cancel.output.contains("Cancelled self-dev build request"));

        let second_status =
            wait_for_task_completion(second_meta["task_id"].as_str().unwrap()).await;
        assert_eq!(second_status.status, BackgroundTaskStatus::Failed);

        let cancelled_request = BuildRequest::load(second_meta["request_id"].as_str().unwrap())
            .expect("load cancelled request")
            .expect("cancelled request exists");
        assert_eq!(cancelled_request.state, BuildRequestState::Cancelled);

        let status_output = selfdev_status_output().expect("status output");
        assert!(status_output.output.contains("keep building"));
        assert!(!status_output.output.contains("cancel me"));

        let first_meta = first.metadata.expect("first metadata");
        let first_status = wait_for_task_completion(first_meta["task_id"].as_str().unwrap()).await;
        assert_eq!(first_status.status, BackgroundTaskStatus::Completed);
    }

    #[test]
    fn status_output_prunes_stale_pending_requests() {
        let _storage_guard = crate::storage::lock_test_env();
        let _lock = lock_env();
        let temp_home = tempfile::TempDir::new().expect("temp home");
        let _home_guard = EnvVarGuard::set("JCODE_HOME", temp_home.path());

        let mut session = session::Session::create(None, Some("Stale Build".to_string()));
        session.short_name = Some("ghost".to_string());
        session.save().expect("save session");

        let stale_status_path = temp_home.path().join("missing-selfdev.status.json");
        let source = test_source_state(std::path::Path::new("/tmp/jcode"));
        let request = BuildRequest {
            request_id: "stale-request".to_string(),
            background_task_id: Some("missing-task".to_string()),
            session_id: session.id.clone(),
            session_short_name: session.short_name.clone(),
            session_title: Some("Stale Build".to_string()),
            reason: "stale reason".to_string(),
            repo_dir: "/tmp/jcode".to_string(),
            repo_scope: source.repo_scope.clone(),
            worktree_scope: source.worktree_scope.clone(),
            command: "scripts/dev_cargo.sh build --release --bin jcode".to_string(),
            requested_at: Utc::now().to_rfc3339(),
            started_at: Some(Utc::now().to_rfc3339()),
            completed_at: None,
            state: BuildRequestState::Building,
            version: Some("stale-build".to_string()),
            dedupe_key: Some("stale-dedupe".to_string()),
            requested_source: Some(source),
            built_source: None,
            published_version: None,
            last_progress: Some("building".to_string()),
            validated: false,
            error: None,
            output_file: None,
            status_file: Some(stale_status_path.display().to_string()),
            attached_to_request_id: None,
        };
        request.save().expect("save stale request");

        let status_output = selfdev_status_output().expect("status output");
        assert!(
            !status_output.output.contains("stale reason"),
            "stale request should be pruned from queue output"
        );

        let request = BuildRequest::load("stale-request")
            .expect("load stale request")
            .expect("stale request exists");
        assert_eq!(request.state, BuildRequestState::Failed);
        assert!(
            request
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("pruning stale self-dev build request"),
            "stale request should record why it was pruned"
        );
    }

    #[tokio::test]
    async fn build_ignores_stale_pending_requests_when_computing_queue_position() {
        let _storage_guard = crate::storage::lock_test_env();
        let _lock = lock_env();
        let temp_home = tempfile::TempDir::new().expect("temp home");
        let _home_guard = EnvVarGuard::set("JCODE_HOME", temp_home.path());
        let _test_guard = EnvVarGuard::set("JCODE_TEST_SESSION", "1");
        let repo = create_repo_fixture();

        let mut stale_session = session::Session::create(None, Some("Stale Build".to_string()));
        stale_session.short_name = Some("ghost".to_string());
        stale_session.save().expect("save stale session");

        let stale_status_path = temp_home.path().join("stale-running.status.json");
        storage::write_json(
            &stale_status_path,
            &background::TaskStatusFile {
                task_id: "stale-task".to_string(),
                tool_name: "selfdev-build".to_string(),
                session_id: stale_session.id.clone(),
                status: BackgroundTaskStatus::Running,
                exit_code: None,
                error: None,
                started_at: Utc::now().to_rfc3339(),
                completed_at: None,
                duration_secs: None,
                pid: None,
                detached: false,
                notify: true,
                wake: true,
            },
        )
        .expect("write stale status file");

        let source = test_source_state(repo.path());
        let stale_request = BuildRequest {
            request_id: "stale-queued-request".to_string(),
            background_task_id: Some("stale-task".to_string()),
            session_id: stale_session.id.clone(),
            session_short_name: stale_session.short_name.clone(),
            session_title: Some("Stale Build".to_string()),
            reason: "stale blocker".to_string(),
            repo_dir: repo.path().display().to_string(),
            repo_scope: source.repo_scope.clone(),
            worktree_scope: source.worktree_scope.clone(),
            command: "scripts/dev_cargo.sh build --release --bin jcode".to_string(),
            requested_at: Utc::now().to_rfc3339(),
            started_at: Some(Utc::now().to_rfc3339()),
            completed_at: None,
            state: BuildRequestState::Queued,
            version: Some("test-build".to_string()),
            dedupe_key: Some("stale-dedupe".to_string()),
            requested_source: Some(source),
            built_source: None,
            published_version: None,
            last_progress: Some("queued".to_string()),
            validated: false,
            error: None,
            output_file: None,
            status_file: Some(stale_status_path.display().to_string()),
            attached_to_request_id: None,
        };
        stale_request.save().expect("save stale queued request");

        let mut live_session = session::Session::create(None, Some("Live Build".to_string()));
        live_session.short_name = Some("alpha".to_string());
        live_session.save().expect("save live session");

        let tool = SelfDevTool::new();
        let output = tool
            .execute(
                json!({"action": "build", "reason": "fresh build"}),
                create_test_context(&live_session.id, Some(repo.path().to_path_buf())),
            )
            .await
            .expect("build should queue");

        let metadata = output.metadata.expect("build metadata");
        assert_eq!(metadata["queue_position"].as_u64(), Some(1));
        assert!(
            !output.output.contains("Currently blocked by"),
            "stale queued requests should not block new builds"
        );

        let stale_request = BuildRequest::load("stale-queued-request")
            .expect("load stale queued request")
            .expect("stale queued request exists");
        assert_eq!(stale_request.state, BuildRequestState::Failed);

        let task_id = metadata["task_id"].as_str().expect("task id");
        let status = wait_for_task_completion(task_id).await;
        assert_eq!(status.status, BackgroundTaskStatus::Completed);
    }

    #[test]
    fn reconcile_pending_state_maps_superseded_background_status() {
        let _storage_guard = crate::storage::lock_test_env();
        let _lock = lock_env();
        let temp_home = tempfile::TempDir::new().expect("temp home");
        let _home_guard = EnvVarGuard::set("JCODE_HOME", temp_home.path());

        let mut session = session::Session::create(None, Some("Superseded Build".to_string()));
        session.short_name = Some("alpha".to_string());
        session.save().expect("save session");

        let status_path = temp_home.path().join("superseded.status.json");
        storage::write_json(
            &status_path,
            &background::TaskStatusFile {
                task_id: "superseded-task".to_string(),
                tool_name: "selfdev-build".to_string(),
                session_id: session.id.clone(),
                status: BackgroundTaskStatus::Superseded,
                exit_code: Some(0),
                error: Some("Build completed, but source changed before activation".to_string()),
                started_at: Utc::now().to_rfc3339(),
                completed_at: Some(Utc::now().to_rfc3339()),
                duration_secs: Some(1.0),
                pid: None,
                detached: false,
                notify: true,
                wake: true,
            },
        )
        .expect("write superseded status file");

        let source = test_source_state(std::path::Path::new("/tmp/jcode"));
        let request = BuildRequest {
            request_id: "superseded-request".to_string(),
            background_task_id: Some("superseded-task".to_string()),
            session_id: session.id.clone(),
            session_short_name: session.short_name.clone(),
            session_title: Some("Superseded Build".to_string()),
            reason: "superseded reason".to_string(),
            repo_dir: "/tmp/jcode".to_string(),
            repo_scope: source.repo_scope.clone(),
            worktree_scope: source.worktree_scope.clone(),
            command: "scripts/dev_cargo.sh build --release --bin jcode".to_string(),
            requested_at: Utc::now().to_rfc3339(),
            started_at: Some(Utc::now().to_rfc3339()),
            completed_at: None,
            state: BuildRequestState::Building,
            version: Some("superseded-build".to_string()),
            dedupe_key: Some("superseded-dedupe".to_string()),
            requested_source: Some(source),
            built_source: None,
            published_version: None,
            last_progress: Some("building".to_string()),
            validated: false,
            error: None,
            output_file: None,
            status_file: Some(status_path.display().to_string()),
            attached_to_request_id: None,
        };
        request.save().expect("save superseded request");

        let mut request = BuildRequest::load("superseded-request")
            .expect("load superseded request")
            .expect("request exists");
        assert!(!request
            .reconcile_pending_state()
            .expect("reconcile superseded request"));
        assert_eq!(request.state, BuildRequestState::Superseded);
        assert!(request
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("source changed before activation"));
    }
}
