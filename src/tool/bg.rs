//! Background task management tool
//!
//! Allows the agent to list, wait on, inspect, read output from, and manage
//! background tasks.

use super::{Tool, ToolContext, ToolOutput};
use crate::background;
use crate::bus::BackgroundTaskStatus;
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::time::{Duration, Instant};

fn default_watch_notify() -> bool {
    true
}

fn default_watch_wake() -> bool {
    true
}

fn default_wait_return_on_progress() -> bool {
    true
}

const DEFAULT_WAIT_SECONDS: u64 = 60;
const MAX_WAIT_SECONDS: u64 = 60 * 60;
const DEFAULT_TAIL_LINES: usize = 80;
const DEFAULT_WAIT_PREVIEW_LINES: usize = 40;
const MAX_OUTPUT_BYTES: usize = 50_000;

pub struct BgTool;

impl BgTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct BgInput {
    /// Action to perform: list, status, output, tail, cancel, cleanup, watch, delivery, subscribe, wait
    #[serde(default)]
    action: Option<String>,
    /// Short display label describing why this tool call is being made.
    #[serde(default)]
    intent: Option<String>,
    /// Task ID (for single-task actions)
    #[serde(default)]
    task_id: Option<String>,
    /// Task IDs (for multi-task wait/status/output where supported)
    #[serde(default)]
    task_ids: Option<Vec<String>>,
    /// Use the latest matching task when task_id is omitted
    #[serde(default)]
    latest: Option<bool>,
    /// Restrict implicit selection/listing to this session. Defaults to false for list and true for implicit selection.
    #[serde(default)]
    session_only: Option<bool>,
    /// Status filter, either a string or array of strings: running/completed/failed/superseded/terminal/all
    #[serde(default)]
    status_filter: Option<Value>,
    /// Max age in hours for cleanup (default: 24)
    #[serde(default)]
    max_age_hours: Option<u64>,
    /// Dry-run cleanup without deleting files
    #[serde(default)]
    dry_run: Option<bool>,
    /// Whether to notify on completion when using watch/delivery (default: true)
    #[serde(default)]
    notify: Option<bool>,
    /// Whether to wake on completion when using watch/delivery (default: true)
    #[serde(default)]
    wake: Option<bool>,
    /// Max seconds to block when using wait (default: 60, capped at 3600)
    #[serde(default)]
    max_wait_seconds: Option<u64>,
    /// Whether wait should return on progress/checkpoint events (default: true)
    #[serde(default)]
    return_on_progress: Option<bool>,
    /// Multi-task wait mode: any, all, first_failure
    #[serde(default)]
    wait_mode: Option<String>,
    /// Tail only the last N lines for output/tail and wait previews
    #[serde(default)]
    tail_lines: Option<usize>,
    /// Alias for tail_lines
    #[serde(default)]
    lines: Option<usize>,
    /// Include an output preview when wait returns; failed tasks preview by default
    #[serde(default)]
    include_output_preview: Option<bool>,
    /// Optional grace period for detached cancellation before SIGKILL
    #[serde(default)]
    graceful_timeout_ms: Option<u64>,
}

fn infer_action_from_intent(intent: Option<&str>) -> Option<&'static str> {
    let intent = intent?.trim().to_ascii_lowercase();
    if intent.is_empty() {
        return None;
    }

    if intent.contains("wait") || intent.contains("await") {
        Some("wait")
    } else if intent.contains("tail") {
        Some("tail")
    } else if intent.contains("output") || intent.contains("log") {
        Some("output")
    } else if intent.contains("status") || intent.contains("progress") || intent.contains("check") {
        Some("status")
    } else if intent.contains("cancel") || intent.contains("stop") {
        Some("cancel")
    } else if intent.contains("clean") {
        Some("cleanup")
    } else if intent.contains("list") || intent.contains("show") {
        Some("list")
    } else {
        None
    }
}

fn resolve_action(params: &BgInput) -> Result<String> {
    if let Some(action) = params
        .action
        .as_deref()
        .map(str::trim)
        .filter(|action| !action.is_empty())
    {
        return Ok(action.to_ascii_lowercase());
    }

    if let Some(action) = infer_action_from_intent(params.intent.as_deref()) {
        return Ok(action.to_string());
    }

    Err(anyhow::anyhow!(
        "Missing required bg action. Use one of: list, status, output, tail, cancel, cleanup, delivery, subscribe, wait. For example: bg action="wait"."
    ))
}

fn status_label(status: &BackgroundTaskStatus) -> &'static str {
    match status {
        BackgroundTaskStatus::Running => "running",
        BackgroundTaskStatus::Completed => "completed",
        BackgroundTaskStatus::Superseded => "superseded",
        BackgroundTaskStatus::Failed => "failed",
    }
}

fn is_terminal(status: &BackgroundTaskStatus) -> bool {
    !matches!(status, BackgroundTaskStatus::Running)
}

fn is_success(status: &BackgroundTaskStatus, exit_code: Option<i32>) -> Option<bool> {
    match status {
        BackgroundTaskStatus::Running => None,
        BackgroundTaskStatus::Completed => Some(exit_code.unwrap_or(0) == 0),
        BackgroundTaskStatus::Superseded => Some(true),
        BackgroundTaskStatus::Failed => Some(false),
    }
}

fn parse_status_filter(value: Option<&Value>) -> HashSet<&'static str> {
    let mut set = HashSet::new();
    let Some(value) = value else {
        return set;
    };

    let mut add = |raw: &str| match raw.to_ascii_lowercase().as_str() {
        "running" => {
            set.insert("running");
        }
        "completed" | "success" | "succeeded" => {
            set.insert("completed");
        }
        "failed" | "failure" | "error" => {
            set.insert("failed");
        }
        "superseded" => {
            set.insert("superseded");
        }
        "terminal" | "finished" | "done" => {
            set.insert("completed");
            set.insert("failed");
            set.insert("superseded");
        }
        "all" | "any" => {}
        _ => {}
    };

    match value {
        Value::String(s) => add(s),
        Value::Array(items) => {
            for item in items {
                if let Some(s) = item.as_str() {
                    add(s);
                }
            }
        }
        _ => {}
    }

    set
}

fn task_matches_filter(task: &background::TaskStatusFile, filter: &HashSet<&str>) -> bool {
    filter.is_empty() || filter.contains(status_label(&task.status))
}

fn task_metadata(
    manager: &background::BackgroundTaskManager,
    task: &background::TaskStatusFile,
) -> Value {
    json!({
        "task_id": task.task_id,
        "display_name": task.display_name,
        "tool_name": task.tool_name,
        "status": status_label(&task.status),
        "is_terminal": is_terminal(&task.status),
        "is_success": is_success(&task.status, task.exit_code),
        "exit_code": task.exit_code,
        "error": task.error,
        "session_id": task.session_id,
        "started_at": task.started_at,
        "completed_at": task.completed_at,
        "duration_secs": task.duration_secs,
        "notify": task.notify,
        "wake": task.wake,
        "pid": task.pid,
        "detached": task.detached,
        "progress": task.progress,
        "event_history": task.event_history,
        "output_file": manager.output_path_for(&task.task_id).to_string_lossy(),
        "status_file": manager.status_path_for(&task.task_id).to_string_lossy(),
    })
}

fn format_task_details(task: &background::TaskStatusFile) -> String {
    let mut output = format!(
        "Task: {}\n\
         Name: {}\n\
         Tool: {}\n\
         Status: {}\n\
         Session: {}\n\
         Started: {}\n",
        task.task_id,
        crate::message::background_task_display_label(
            &task.tool_name,
            task.display_name.as_deref()
        ),
        task.tool_name,
        status_label(&task.status),
        task.session_id,
        task.started_at,
    );

    if let Some(completed) = task.completed_at.as_ref() {
        output.push_str(&format!("Completed: {}\n", completed));
    }
    if let Some(duration) = task.duration_secs {
        output.push_str(&format!("Duration: {:.2}s\n", duration));
    }
    if let Some(exit_code) = task.exit_code {
        output.push_str(&format!("Exit code: {}\n", exit_code));
    }
    if let Some(progress) = task.progress.as_ref() {
        output.push_str(&format!(
            "Progress: {}\n",
            crate::background::format_progress_display(progress, 18)
        ));
        output.push_str(&format!("Progress updated: {}\n", progress.updated_at));
    }
    output.push_str(&format!("Notify: {}\n", task.notify));
    output.push_str(&format!("Wake: {}\n", task.wake));
    if let Some(error) = task.error.as_ref() {
        output.push_str(&format!("Error: {}\n", error));
    }

    if !task.event_history.is_empty() {
        output.push_str("Recent events:\n");
        let start = task.event_history.len().saturating_sub(5);
        for event in &task.event_history[start..] {
            let message = event
                .message
                .as_deref()
                .filter(|message| !message.is_empty())
                .map(|message| format!(" · {}", crate::util::truncate_str(message, 80)))
                .unwrap_or_default();
            output.push_str(&format!(
                "- {:?} · {}{}\n",
                event.kind, event.timestamp, message
            ));
        }
    }

    output
}

fn tail_lines(output: &str, lines: usize) -> String {
    if lines == 0 {
        return String::new();
    }
    let collected: Vec<&str> = output.lines().rev().take(lines).collect();
    collected.into_iter().rev().collect::<Vec<_>>().join("\n")
}

fn output_preview(output: &str, tail: Option<usize>) -> (String, bool) {
    if let Some(lines) = tail {
        let tailed = tail_lines(output, lines);
        let truncated = tailed.len() < output.len();
        return (tailed, truncated);
    }

    if output.len() > MAX_OUTPUT_BYTES {
        (
            format!(
                "{}...\n\n(Output truncated. Use `read` tool on the output file for full content, or `bg action=\"tail\"` for recent lines.)",
                crate::util::truncate_str(output, MAX_OUTPUT_BYTES)
            ),
            true,
        )
    } else {
        (output.to_string(), false)
    }
}

fn wait_reason_label(reason: background::BackgroundTaskWaitReason) -> &'static str {
    match reason {
        background::BackgroundTaskWaitReason::AlreadyFinished => "already_finished",
        background::BackgroundTaskWaitReason::Finished => "finished",
        background::BackgroundTaskWaitReason::Progress => "progress",
        background::BackgroundTaskWaitReason::Checkpoint => "checkpoint",
        background::BackgroundTaskWaitReason::Timeout => "timeout",
    }
}

async fn filtered_tasks(
    manager: &background::BackgroundTaskManager,
    ctx: &ToolContext,
    params: &BgInput,
    default_session_only: bool,
) -> Vec<background::TaskStatusFile> {
    let mut tasks = manager.list().await;
    let session_only = params.session_only.unwrap_or(default_session_only);
    let filter = parse_status_filter(params.status_filter.as_ref());
    tasks.retain(|task| {
        (!session_only || task.session_id == ctx.session_id) && task_matches_filter(task, &filter)
    });
    tasks
}

async fn resolve_task_ids(
    manager: &background::BackgroundTaskManager,
    ctx: &ToolContext,
    params: &BgInput,
    action: &str,
    allow_multiple: bool,
) -> Result<Vec<String>> {
    let task_ids = params.task_ids.as_deref().unwrap_or(&[]);
    if !task_ids.is_empty() {
        if !allow_multiple && task_ids.len() > 1 {
            return Err(anyhow::anyhow!(
                "action '{}' accepts only one task_id; got {} task_ids",
                action,
                task_ids.len()
            ));
        }
        return Ok(task_ids.to_vec());
    }
    if let Some(task_id) = params.task_id.clone() {
        return Ok(vec![task_id]);
    }

    let mut tasks = filtered_tasks(manager, ctx, params, true).await;
    if params.latest.unwrap_or(false) {
        return tasks
            .first()
            .map(|task| vec![task.task_id.clone()])
            .ok_or_else(|| anyhow::anyhow!("No matching background tasks found for latest=true"));
    }

    tasks.retain(|task| task.status == BackgroundTaskStatus::Running);
    match tasks.as_slice() {
        [task] => Ok(vec![task.task_id.clone()]),
        [] => Err(anyhow::anyhow!(
            "task_id is required for {} action unless exactly one matching running task exists in this session. Try `bg action=\"list\" status_filter=\"running\" session_only=true` or pass latest=true.",
            action
        )),
        _ => Err(anyhow::anyhow!(
            "Multiple matching running tasks found; pass task_id, task_ids, or latest=true. Matching task IDs: {}",
            tasks
                .iter()
                .map(|task| task.task_id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

async fn wait_many_polling(
    manager: &background::BackgroundTaskManager,
    task_ids: &[String],
    max_wait: Duration,
    return_on_progress: bool,
    mode: &str,
) -> Result<(String, Vec<background::TaskStatusFile>)> {
    let deadline = Instant::now() + max_wait;
    let mut last_progress = std::collections::HashMap::new();
    for task_id in task_ids {
        if let Some(task) = manager.status(task_id).await {
            last_progress.insert(task_id.clone(), task.progress.clone());
        }
    }

    loop {
        let mut tasks = Vec::new();
        for task_id in task_ids {
            if let Some(task) = manager.status(task_id).await {
                tasks.push(task);
            }
        }

        if mode == "first_failure"
            && tasks
                .iter()
                .any(|task| matches!(task.status, BackgroundTaskStatus::Failed))
        {
            return Ok(("first_failure".to_string(), tasks));
        }
        if mode == "all" && tasks.iter().all(|task| is_terminal(&task.status)) {
            return Ok(("all_finished".to_string(), tasks));
        }
        if mode != "all" && tasks.iter().any(|task| is_terminal(&task.status)) {
            return Ok(("any_finished".to_string(), tasks));
        }
        if return_on_progress {
            for task in &tasks {
                let previous = last_progress.get(&task.task_id).cloned().unwrap_or(None);
                if task.progress != previous {
                    return Ok(("progress".to_string(), tasks));
                }
            }
        }

        if Instant::now() >= deadline {
            return Ok(("timeout".to_string(), tasks));
        }
        for task in &tasks {
            last_progress.insert(task.task_id.clone(), task.progress.clone());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

#[async_trait]
impl Tool for BgTool {
    fn name(&self) -> &str {
        "bg"
    }

    fn description(&self) -> &str {
        "Manage background tasks. Prefer action='wait' over polling or sleeping. Use action='tail' or output with tail_lines for logs, action='delivery' to change notify/wake behavior, and JCODE_CHECKPOINT/JCODE_PROGRESS from background commands for reliable wakeups."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "status", "output", "tail", "cancel", "cleanup", "watch", "delivery", "subscribe", "wait"],
                    "description": "Action. Prefer wait for blocking until completion/checkpoints; watch is a compatibility alias for delivery."
                },
                "task_id": { "type": "string", "description": "Task ID." },
                "task_ids": { "type": "array", "items": {"type":"string"}, "description": "Task IDs for multi-task wait/status." },
                "latest": { "type": "boolean", "description": "Use latest matching task when task_id is omitted." },
                "session_only": { "type": "boolean", "description": "Restrict list/implicit selection to current session." },
                "status_filter": {
                    "anyOf": [
                        { "type": "string" },
                        { "type": "array", "items": { "type": "string" } }
                    ],
                    "description": "Status filter string or array: running, completed, failed, superseded, terminal, all."
                },
                "max_age_hours": { "type": "integer", "description": "Cleanup age in hours." },
                "dry_run": { "type": "boolean", "description": "For cleanup, report what would be removed without deleting." },
                "notify": { "type": "boolean", "description": "When using delivery/watch/subscribe, whether to notify on completion. Defaults to true." },
                "wake": { "type": "boolean", "description": "When using delivery/watch/subscribe, whether to wake on completion. Defaults to true." },
                "max_wait_seconds": { "type": "integer", "description": "When using wait, maximum seconds to block before returning. Defaults to 60, capped at 3600. Use 0 for an immediate check." },
                "return_on_progress": { "type": "boolean", "description": "When using wait, return as soon as the task emits a progress/checkpoint event instead of only completion or timeout. Defaults to true." },
                "wait_mode": { "type": "string", "enum": ["any", "all", "first_failure"], "description": "For multi-task wait, return on any completion, all completions, or first failure. Defaults to any." },
                "tail_lines": { "type": "integer", "description": "Return only the last N output lines for output/tail/wait preview." },
                "lines": { "type": "integer", "description": "Alias for tail_lines." },
                "include_output_preview": { "type": "boolean", "description": "When wait returns, include a recent output preview. Failed tasks include a preview by default." },
                "graceful_timeout_ms": { "type": "integer", "description": "For cancel, grace period for detached process termination before force kill." }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: BgInput = serde_json::from_value(input)?;
        let action = resolve_action(&params)?;
        let manager = background::global();

        match action.as_str() {
            "list" => {
                let tasks = filtered_tasks(manager, &ctx, &params, false).await;
                if tasks.is_empty() {
                    return Ok(ToolOutput::new("No matching background tasks found.")
                        .with_title("bg list"));
                }

                let mut output = String::from("Background Tasks:\n\n");
                output.push_str(&format!(
                    "{:<12} {:<28} {:<10} {:<12} {:<10} {:<28} {}\n",
                    "TASK_ID", "NAME", "TOOL", "STATUS", "DURATION", "PROGRESS", "SESSION"
                ));
                output.push_str(&"-".repeat(121));
                output.push('\n');

                for task in &tasks {
                    let duration = task
                        .duration_secs
                        .map(|d| format!("{:.1}s", d))
                        .unwrap_or_else(|| "running".to_string());
                    let progress = task
                        .progress
                        .as_ref()
                        .map(|progress| crate::background::format_progress_display(progress, 10))
                        .unwrap_or_else(|| "-".to_string());
                    let display_name = crate::message::background_task_display_label(
                        &task.tool_name,
                        task.display_name.as_deref(),
                    );
                    output.push_str(&format!(
                        "{:<12} {:<28} {:<10} {:<12} {:<10} {:<28} {}\n",
                        task.task_id,
                        crate::util::truncate_str(&display_name, 28),
                        task.tool_name,
                        status_label(&task.status),
                        duration,
                        crate::util::truncate_str(&progress, 28),
                        &task.session_id[..8.min(task.session_id.len())]
                    ));
                }

                Ok(ToolOutput::new(output).with_title("bg list").with_metadata(json!({
                    "tasks": tasks.iter().map(|task| task_metadata(manager, task)).collect::<Vec<_>>(),
                    "count": tasks.len(),
                })))
            }

            "status" => {
                let task_ids = resolve_task_ids(manager, &ctx, &params, "status", true).await?;
                let mut tasks = Vec::new();
                let mut output = String::new();
                for task_id in task_ids {
                    let task = manager
                        .status(&task_id)
                        .await
                        .ok_or_else(|| anyhow::anyhow!("Task not found: {}", task_id))?;
                    if !output.is_empty() {
                        output.push_str("\n---\n");
                    }
                    output.push_str(&format_task_details(&task));
                    if matches!(task.status, BackgroundTaskStatus::Failed) {
                        crate::logging::warn(&format!(
                            "[tool:bg] task {} ({}) failed in session {} exit_code={:?} error={}",
                            task.task_id,
                            task.tool_name,
                            task.session_id,
                            task.exit_code,
                            task.error.as_deref().unwrap_or("<none>")
                        ));
                    }
                    tasks.push(task);
                }
                Ok(ToolOutput::new(output).with_title("bg status").with_metadata(json!({
                    "tasks": tasks.iter().map(|task| task_metadata(manager, task)).collect::<Vec<_>>(),
                    "task": tasks.first().map(|task| task_metadata(manager, task)),
                })))
            }

            "output" | "tail" => {
                let task_id = resolve_task_ids(manager, &ctx, &params, "output", false)
                    .await?
                    .remove(0);
                let tail = if action == "tail" {
                    Some(
                        params
                            .tail_lines
                            .or(params.lines)
                            .unwrap_or(DEFAULT_TAIL_LINES),
                    )
                } else {
                    params.tail_lines.or(params.lines)
                };
                let output = manager.output(&task_id).await.ok_or_else(|| {
                    anyhow::anyhow!(
                        "Output not found for task: {}. Task may not exist or output file was deleted.",
                        task_id
                    )
                })?;
                let (rendered, truncated) = output_preview(&output, tail);
                let status = manager.status(&task_id).await;
                Ok(ToolOutput::new(rendered)
                    .with_title(format!("bg {} {}", action, task_id))
                    .with_metadata(json!({
                        "task_id": task_id,
                        "task": status.as_ref().map(|task| task_metadata(manager, task)),
                        "tail_lines": tail,
                        "truncated": truncated,
                        "output_bytes": output.len(),
                    })))
            }

            "cancel" => {
                let task_id = resolve_task_ids(manager, &ctx, &params, "cancel", false)
                    .await?
                    .remove(0);
                let grace = Duration::from_millis(params.graceful_timeout_ms.unwrap_or(400));
                match manager.cancel_with_grace(&task_id, grace).await? {
                    true => Ok(ToolOutput::new(format!("Task {} cancelled.", task_id))
                        .with_title(format!("bg cancel {}", task_id))
                        .with_metadata(json!({"task_id": task_id, "cancelled": true, "graceful_timeout_ms": grace.as_millis()}))),
                    false => Err(anyhow::anyhow!(
                        "Task {} not found or already completed.",
                        task_id
                    )),
                }
            }

            "cleanup" => {
                let max_age = params.max_age_hours.unwrap_or(24);
                let filter = parse_status_filter(params.status_filter.as_ref());
                let dry_run = params.dry_run.unwrap_or(false);
                let result = manager.cleanup_filtered(max_age, &filter, dry_run).await?;
                Ok(ToolOutput::new(format!(
                    "{} {} old task files (older than {} hours). Skipped {} running task file(s).",
                    if dry_run { "Would remove" } else { "Removed" },
                    result.removed_files,
                    max_age,
                    result.skipped_running_files
                ))
                .with_title("bg cleanup")
                .with_metadata(json!({
                    "removed_files": result.removed_files,
                    "matched_files": result.matched_files,
                    "skipped_running_files": result.skipped_running_files,
                    "dry_run": dry_run,
                    "max_age_hours": max_age,
                })))
            }

            "watch" | "delivery" | "subscribe" => {
                let task_id = resolve_task_ids(manager, &ctx, &params, "delivery", false)
                    .await?
                    .remove(0);
                let notify = params.notify.unwrap_or_else(default_watch_notify);
                let wake = params.wake.unwrap_or_else(default_watch_wake);
                match manager.update_delivery(&task_id, notify, wake).await? {
                    Some(task) => Ok(ToolOutput::new(format!(
                        "Updated background task delivery for {}.\nStatus: {}\nNotify: {}\nWake: {}",
                        task_id,
                        status_label(&task.status),
                        task.notify,
                        task.wake
                    ))
                    .with_title(format!("bg delivery {}", task_id))
                    .with_metadata(json!({
                        "task_id": task.task_id,
                        "task": task_metadata(manager, &task),
                        "status": status_label(&task.status),
                        "notify": task.notify,
                        "wake": task.wake,
                    }))),
                    None => Err(anyhow::anyhow!("Task not found: {}", task_id)),
                }
            }

            "wait" => {
                let task_ids = resolve_task_ids(manager, &ctx, &params, "wait", true).await?;
                let requested_wait = params.max_wait_seconds.unwrap_or(DEFAULT_WAIT_SECONDS);
                let capped_wait = requested_wait.min(MAX_WAIT_SECONDS);
                let wait_duration = Duration::from_secs(capped_wait);

                if task_ids.len() > 1 {
                    let mode = params.wait_mode.as_deref().unwrap_or("any");
                    let mode = if matches!(mode, "all" | "first_failure") {
                        mode
                    } else {
                        "any"
                    };
                    let (reason, tasks) = wait_many_polling(
                        manager,
                        &task_ids,
                        wait_duration,
                        params
                            .return_on_progress
                            .unwrap_or_else(default_wait_return_on_progress),
                        mode,
                    )
                    .await?;
                    let mut output =
                        format!("Multi-task wait returned: {}\nMode: {}\n\n", reason, mode);
                    for task in &tasks {
                        output.push_str(&format!(
                            "- {} · {} · {}\n",
                            task.task_id,
                            crate::message::background_task_display_label(
                                &task.tool_name,
                                task.display_name.as_deref()
                            ),
                            status_label(&task.status)
                        ));
                    }
                    return Ok(ToolOutput::new(output).with_title("bg wait multiple").with_metadata(json!({
                        "wait_reason": reason,
                        "wait_mode": mode,
                        "timed_out": reason == "timeout",
                        "max_wait_seconds": capped_wait,
                        "tasks": tasks.iter().map(|task| task_metadata(manager, task)).collect::<Vec<_>>(),
                    })));
                }

                let task_id = task_ids.into_iter().next().expect("one task id");
                match manager
                    .wait(
                        &task_id,
                        wait_duration,
                        params
                            .return_on_progress
                            .unwrap_or_else(default_wait_return_on_progress),
                    )
                    .await
                {
                    Some(wait_result) => {
                        let task = wait_result.task;
                        let reason = wait_result.reason;
                        let reason_str = wait_reason_label(reason);
                        let mut output = match reason {
                            background::BackgroundTaskWaitReason::AlreadyFinished => {
                                "Background task was already finished.\n\n".to_string()
                            }
                            background::BackgroundTaskWaitReason::Finished => {
                                "Background task finished.\n\n".to_string()
                            }
                            background::BackgroundTaskWaitReason::Progress => {
                                "Background task emitted a progress event.\n\n".to_string()
                            }
                            background::BackgroundTaskWaitReason::Checkpoint => {
                                "Background task emitted a checkpoint event.\n\n".to_string()
                            }
                            background::BackgroundTaskWaitReason::Timeout => format!(
                                "No terminal event before max wait of {}s. Check again with `bg action=\"wait\" task_id=\"{}\"` or inspect status/output.\n\n",
                                capped_wait, task_id
                            ),
                        };
                        output.push_str(&format_task_details(&task));
                        if requested_wait > MAX_WAIT_SECONDS {
                            output.push_str(&format!(
                                "Requested wait was capped from {}s to {}s.\n",
                                requested_wait, MAX_WAIT_SECONDS
                            ));
                        }

                        let include_preview = params.include_output_preview.unwrap_or_else(|| {
                            matches!(task.status, BackgroundTaskStatus::Failed)
                                || matches!(reason, background::BackgroundTaskWaitReason::Finished)
                        });
                        let mut preview_meta = Value::Null;
                        if include_preview {
                            if let Some(full_output) = manager.output(&task.task_id).await {
                                let tail = Some(
                                    params
                                        .tail_lines
                                        .or(params.lines)
                                        .unwrap_or(DEFAULT_WAIT_PREVIEW_LINES),
                                );
                                let (preview, truncated) = output_preview(&full_output, tail);
                                if !preview.trim().is_empty() {
                                    output.push_str("\nOutput preview:\n```text\n");
                                    output.push_str(&preview);
                                    if !preview.ends_with('\n') {
                                        output.push('\n');
                                    }
                                    output.push_str("```\n");
                                }
                                preview_meta = json!({
                                    "tail_lines": tail,
                                    "truncated": truncated,
                                    "output_bytes": full_output.len(),
                                });
                            }
                        }

                        Ok(ToolOutput::new(output)
                            .with_title(format!("bg wait {}", task_id))
                            .with_metadata(json!({
                                "task_id": task.task_id,
                                "task": task_metadata(manager, &task),
                                "display_name": task.display_name,
                                "status": status_label(&task.status),
                                "wait_reason": reason_str,
                                "timed_out": matches!(reason, background::BackgroundTaskWaitReason::Timeout),
                                "max_wait_seconds": capped_wait,
                                "return_on_progress": params.return_on_progress.unwrap_or_else(default_wait_return_on_progress),
                                "exit_code": task.exit_code,
                                "progress": task.progress,
                                "progress_event": wait_result.progress_event,
                                "event_record": wait_result.event_record,
                                "output_preview": preview_meta,
                            })))
                    }
                    None => Err(anyhow::anyhow!("Task not found: {}", task_id)),
                }
            }

            _ => Err(anyhow::anyhow!(
                "Unknown action: {}. Valid actions: list, status, output, tail, cancel, cleanup, watch, delivery, subscribe, wait",
                action
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_filter_schema_any_of_branches_have_types() {
        let schema = BgTool::new().parameters_schema();
        let branches = schema["properties"]["status_filter"]["anyOf"]
            .as_array()
            .expect("status_filter should define anyOf branches");

        assert_eq!(branches[0]["type"], json!("string"));
        assert_eq!(branches[1]["type"], json!("array"));
        assert_eq!(branches[1]["items"]["type"], json!("string"));
    }

    #[test]
    fn resolve_action_infers_wait_from_intent_only_call() {
        let params: BgInput = serde_json::from_value(json!({
            "intent": "Wait for library tests",
            "latest": true
        }))
        .expect("intent-only bg input should deserialize");

        assert_eq!(resolve_action(&params).expect("action should be inferred"), "wait");
    }

    #[test]
    fn resolve_action_reports_clear_error_when_missing_and_not_inferable() {
        let params: BgInput = serde_json::from_value(json!({
            "intent": "Background task",
        }))
        .expect("intent-only bg input should deserialize");

        let err = resolve_action(&params).expect_err("action should be required");
        assert!(
            err.to_string().contains("Missing required bg action"),
            "err={err:?}"
        );
    }
}
