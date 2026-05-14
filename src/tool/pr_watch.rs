use super::{Tool, ToolContext, ToolOutput};
use crate::ambient::{AmbientManager, Priority, ScheduleRequest, ScheduleTarget};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use chrono::{Duration, Utc};
use jcode_pr_watch_core::{
    ActionableItem, CheckRunState, CycleOutcome, Marker, PrTarget, PrWatchState, SurfaceError,
    WatchEvent, normalize_watch_state_json, parse_gh_checks, parse_gh_issue_comments,
    parse_gh_pr_view, parse_gh_review_comments, parse_gh_review_threads, parse_gh_reviews,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration as StdDuration;
use tokio::process::Command;

pub struct PrWatchTool;

impl PrWatchTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PrWatchAction {
    Start,
    Status,
    List,
    PollNow,
    Monitor,
    Stop,
    Readiness,
    Handoff,
    AckBaseline,
}

#[derive(Debug, Deserialize)]
struct PrWatchInput {
    action: PrWatchAction,
    repo: Option<String>,
    pr: Option<u64>,
    watch_id: Option<String>,
    dry_run: Option<bool>,
    #[serde(default)]
    schedule_next: bool,
    #[serde(default)]
    poll_interval_seconds: Option<u64>,
    #[serde(default)]
    quiet_cycles_required: Option<u64>,
    #[serde(default)]
    max_runtime_seconds: Option<u64>,
    #[serde(default)]
    target: Option<String>,
}

const DEFAULT_MONITOR_MAX_RUNTIME_SECONDS: u64 = 540;
const MAX_MONITOR_MAX_RUNTIME_SECONDS: u64 = 900;

#[async_trait]
impl Tool for PrWatchTool {
    fn name(&self) -> &str {
        "pr_watch"
    }

    fn description(&self) -> &str {
        "PR feedback watch state. Start a local watch, run read-only gh collection, schedule follow-up polls, list watches, show status, or compute readiness. No pushes, comments, thread resolution, or merges are performed."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "intent": super::intent_schema_property(),
                "action": {
                    "type": "string",
                    "enum": ["start", "status", "list", "poll_now", "monitor", "ack_baseline", "stop", "readiness", "handoff"],
                    "description": "Action. poll_now performs read-only gh CLI collection and updates local state; no mutations are performed."
                },
                "repo": {"type": "string", "description": "Repository in owner/name form."},
                "pr": {"type": "integer", "description": "Pull request number."},
                "watch_id": {"type": "string", "description": "Existing watch ID, e.g. owner-repo-pr-123."},
                "dry_run": {"type": "boolean", "description": "Preview changes without writing state."},
                "schedule_next": {"type": "boolean", "description": "If true, schedule the next visible poll wakeup after start, poll_now, or ack_baseline."},
                "poll_interval_seconds": {"type": "integer", "description": "Interval for the next scheduled poll. Defaults to state polling interval."},
                "quiet_cycles_required": {"type": "integer", "description": "Quiet cycles required before the monitor stops as satisfied. Defaults to watch state or 3."},
                "max_runtime_seconds": {"type": "integer", "description": "Maximum monitor runtime budget. Single-cycle monitor caps this to 900 and records the bounded value."},
                "target": {"type": "string", "enum": ["resume", "spawn"], "description": "Schedule delivery target. Defaults to resuming the current session."}
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: PrWatchInput = serde_json::from_value(input)?;
        let root = ctx
            .working_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from("."));
        let store = watch_dir(&root);
        match params.action {
            PrWatchAction::Start => start_watch(&store, params, &ctx),
            PrWatchAction::List => list_watches(&store),
            PrWatchAction::PollNow => poll_now(&root, &store, params, &ctx).await,
            PrWatchAction::Monitor => monitor_once(&root, &store, params, &ctx).await,
            PrWatchAction::AckBaseline => ack_baseline(&root, &store, params, &ctx).await,
            PrWatchAction::Status => status_like(&store, params),
            PrWatchAction::Readiness => readiness_report(&store, params),
            PrWatchAction::Handoff => handoff_report(&store, params),
            PrWatchAction::Stop => stop_watch(&store, params),
        }
    }
}

fn watch_dir(root: &Path) -> PathBuf {
    root.join(".jcode").join("pr-feedback-watch")
}

fn state_path(store: &Path, watch_id: &str) -> PathBuf {
    store.join(format!("{watch_id}-state.json"))
}

fn lock_path(store: &Path, watch_id: &str) -> PathBuf {
    store.join(format!("{watch_id}.lock"))
}

struct WatchLock {
    path: PathBuf,
}

impl Drop for WatchLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn acquire_watch_lock(store: &Path, watch_id: &str) -> Result<Option<WatchLock>> {
    fs::create_dir_all(store)?;
    let path = lock_path(store, watch_id);
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => {
            let lock = WatchLock { path: path.clone() };
            writeln!(file, "pid={} at={}", std::process::id(), now_iso())?;
            Ok(Some(lock))
        }
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(None),
        Err(err) => Err(err).with_context(|| format!("failed to create {}", path.display())),
    }
}

fn watch_locked_output(store: &Path, state: &PrWatchState, action: &str) -> ToolOutput {
    ToolOutput::new(format!(
        "PR watch {action} already running or locked: {}\nRepo: {}\nPR: #{}\nLock: {}\nNo state was changed.",
        state.watch_id,
        state.pr.repo,
        state.pr.number,
        lock_path(store, &state.watch_id).display()
    ))
    .with_title(format!("{} locked", state.watch_id))
    .with_metadata(json!({
        "watch": state,
        "watch_locked": true,
        "action": action,
        "written": false,
    }))
}

fn write_state_atomic(path: &Path, state: &PrWatchState) -> Result<()> {
    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, serde_json::to_vec_pretty(state)?)?;
    fs::rename(&tmp_path, path)
        .with_context(|| format!("failed to atomically replace {}", path.display()))?;
    Ok(())
}

fn target_from_params(params: &PrWatchInput) -> Result<PrTarget> {
    let repo = params.repo.clone().context("repo is required")?;
    let number = params.pr.context("pr is required")?;
    Ok(PrTarget { repo, number })
}

fn start_watch(store: &Path, params: PrWatchInput, ctx: &ToolContext) -> Result<ToolOutput> {
    let target = target_from_params(&params)?;
    let mut state = PrWatchState::new(target);
    let path = state_path(store, &state.watch_id);
    apply_schedule_fields(&mut state, &params);
    let would_write = !params.dry_run.unwrap_or(false);
    if would_write {
        fs::create_dir_all(store)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .with_context(|| {
                format!(
                    "watch state already exists or cannot be created: {}",
                    path.display()
                )
            })?;
        file.write_all(&serde_json::to_vec_pretty(&state)?)?;
    }
    let scheduled = maybe_schedule_next(ctx, &state, &params)?;
    Ok(ToolOutput::new(format!(
        "PR watch initialized: {}\nPath: {}\nMode: local state initialized. Use poll_now for read-only gh collection{}{}",
        state.watch_id,
        path.display(),
        scheduled.as_deref().map(|s| format!("\nScheduled: {s}")).unwrap_or_default(),
        if would_write {
            ""
        } else {
            "\nDry run: no file written"
        }
    ))
    .with_title(format!("watch {}", state.watch_id))
    .with_metadata(json!({"watch": state, "path": path, "written": would_write})))
}

fn list_watches(store: &Path) -> Result<ToolOutput> {
    let states = load_all_states(store)?;
    let mut lines = vec![format!("{} PR watches", states.len())];
    for (path, state) in &states {
        lines.push(format!(
            "- {}: {}/#{} quiet={}/{} actionable={} status={:?} path={}",
            state.watch_id,
            state.pr.repo,
            state.pr.number,
            state.polling.quiet_cycles,
            state.polling.required_quiet_cycles,
            state.pending_actionable.len(),
            state.last_cycle.status,
            path.display()
        ));
    }
    Ok(ToolOutput::new(lines.join("\n"))
        .with_title(format!("{} watches", states.len()))
        .with_metadata(json!({"watches": states.into_iter().map(|(_, s)| s).collect::<Vec<_>>() })))
}

fn apply_schedule_fields(state: &mut PrWatchState, params: &PrWatchInput) {
    if let Some(seconds) = params.poll_interval_seconds {
        state.polling.poll_interval_seconds = seconds.max(60);
    }
    if let Some(required) = params.quiet_cycles_required {
        state.polling.required_quiet_cycles = required.max(1);
    }
    if params.schedule_next {
        let wake_at = Utc::now() + Duration::seconds(state.polling.poll_interval_seconds as i64);
        state.polling.next_poll_at = Some(wake_at.format("%Y-%m-%dT%H:%M:%SZ").to_string());
    }
}

fn monitor_max_runtime_seconds(params: &PrWatchInput) -> u64 {
    params
        .max_runtime_seconds
        .unwrap_or(DEFAULT_MONITOR_MAX_RUNTIME_SECONDS)
        .clamp(1, MAX_MONITOR_MAX_RUNTIME_SECONDS)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MonitorStatus {
    QuietSatisfied,
    PendingNextPoll,
    ActionRequired,
    ChecksPending,
    ChecksFailed,
    TransientFailure,
    AlreadyRunning,
    Stopped,
}

impl MonitorStatus {
    fn as_str(self) -> &'static str {
        match self {
            MonitorStatus::QuietSatisfied => "quiet_satisfied",
            MonitorStatus::PendingNextPoll => "pending_next_poll",
            MonitorStatus::ActionRequired => "action_required",
            MonitorStatus::ChecksPending => "checks_pending",
            MonitorStatus::ChecksFailed => "checks_failed",
            MonitorStatus::TransientFailure => "transient_failure",
            MonitorStatus::AlreadyRunning => "already_running",
            MonitorStatus::Stopped => "stopped",
        }
    }
}

fn monitor_status_for_state(state: &PrWatchState, partial_failure: bool) -> MonitorStatus {
    if state.terminal {
        return if state.stop_reason.as_deref() == Some("quiet_cycles_satisfied") {
            MonitorStatus::QuietSatisfied
        } else {
            MonitorStatus::Stopped
        };
    }
    if !state.pending_actionable.is_empty() {
        MonitorStatus::ActionRequired
    } else if state.last_cycle.failed_check_count > 0 {
        MonitorStatus::ChecksFailed
    } else if state.last_cycle.pending_check_count > 0 {
        MonitorStatus::ChecksPending
    } else if partial_failure || state.polling.consecutive_transient_failures > 0 {
        MonitorStatus::TransientFailure
    } else if state.polling.quiet_cycles >= state.polling.required_quiet_cycles {
        MonitorStatus::QuietSatisfied
    } else {
        MonitorStatus::PendingNextPoll
    }
}

fn monitor_should_schedule_followup(status: MonitorStatus) -> bool {
    matches!(
        status,
        MonitorStatus::PendingNextPoll
            | MonitorStatus::ChecksPending
            | MonitorStatus::TransientFailure
    )
}

fn watch_state_changed_since_load(
    current_state: &PrWatchState,
    loaded_existing_state: bool,
    loaded_updated_at: &Option<String>,
    loaded_cycle_number: u64,
) -> bool {
    !loaded_existing_state
        || current_state.updated_at != *loaded_updated_at
        || current_state.polling.cycle_number != loaded_cycle_number
}

fn timed_out_collection(max_runtime_seconds: u64) -> GhCollection {
    let message = format!("monitor collection exceeded max_runtime_seconds={max_runtime_seconds}");
    GhCollection {
        metadata: Err(SurfaceError::transient("metadata", message.clone())),
        checks: Err(SurfaceError::transient("checks", message.clone())),
        review_comments: Err(SurfaceError::transient("review_comments", message.clone())),
        issue_comments: Err(SurfaceError::transient("issue_comments", message.clone())),
        reviews: Err(SurfaceError::transient("reviews", message.clone())),
        review_threads: Err(SurfaceError::transient("review_threads", message)),
    }
}

fn maybe_schedule_next(
    ctx: &ToolContext,
    state: &PrWatchState,
    params: &PrWatchInput,
) -> Result<Option<String>> {
    if !params.schedule_next || params.dry_run.unwrap_or(false) || state.terminal {
        return Ok(None);
    }
    let wake_at = Utc::now() + Duration::seconds(state.polling.poll_interval_seconds as i64);
    let task = scheduled_poll_prompt(state);
    let target = match params.target.as_deref() {
        Some("spawn") => ScheduleTarget::Spawn {
            parent_session_id: ctx.session_id.clone(),
        },
        Some("resume") | None => ScheduleTarget::Session {
            session_id: ctx.session_id.clone(),
        },
        Some(other) => bail!("invalid schedule target {other}; expected resume or spawn"),
    };
    let mut manager = AmbientManager::new()?;
    let id = manager.schedule(ScheduleRequest {
        wake_in_minutes: None,
        wake_at: Some(wake_at),
        context: task.clone(),
        priority: Priority::Normal,
        target,
        created_by_session: ctx.session_id.clone(),
        working_dir: ctx
            .working_dir
            .as_ref()
            .map(|path| path.display().to_string()),
        task_description: Some(task),
        relevant_files: vec![format!(
            ".jcode/pr-feedback-watch/{}-state.json",
            state.watch_id
        )],
        git_branch: None,
        additional_context: Some(
            "Scheduled by pr_watch schedule_next; read-only poll only.".to_string(),
        ),
    })?;
    super::ambient::nudge_schedule_runner();
    Ok(Some(format!(
        "{} at {}",
        id,
        wake_at.format("%Y-%m-%dT%H:%M:%SZ")
    )))
}

fn maybe_schedule_next_monitor(
    ctx: &ToolContext,
    state: &PrWatchState,
    params: &PrWatchInput,
) -> Result<Option<String>> {
    if !params.schedule_next || params.dry_run.unwrap_or(false) || state.terminal {
        return Ok(None);
    }
    let wake_at = Utc::now() + Duration::seconds(state.polling.poll_interval_seconds as i64);
    let task = scheduled_monitor_prompt(state, monitor_max_runtime_seconds(params));
    let target = match params.target.as_deref() {
        Some("spawn") => ScheduleTarget::Spawn {
            parent_session_id: ctx.session_id.clone(),
        },
        Some("resume") | None => ScheduleTarget::Session {
            session_id: ctx.session_id.clone(),
        },
        Some(other) => bail!("invalid schedule target {other}; expected resume or spawn"),
    };
    let mut manager = AmbientManager::new()?;
    let id = manager.schedule(ScheduleRequest {
        wake_in_minutes: None,
        wake_at: Some(wake_at),
        context: task.clone(),
        priority: Priority::Normal,
        target,
        created_by_session: ctx.session_id.clone(),
        working_dir: ctx
            .working_dir
            .as_ref()
            .map(|path| path.display().to_string()),
        task_description: Some(task),
        relevant_files: vec![format!(
            ".jcode/pr-feedback-watch/{}-state.json",
            state.watch_id
        )],
        git_branch: None,
        additional_context: Some(
            "Scheduled by pr_watch monitor; invoke structured monitor action only.".to_string(),
        ),
    })?;
    super::ambient::nudge_schedule_runner();
    Ok(Some(format!(
        "{} at {}",
        id,
        wake_at.format("%Y-%m-%dT%H:%M:%SZ")
    )))
}

fn scheduled_poll_prompt(state: &PrWatchState) -> String {
    if state.last_successful_fetch.is_empty() {
        return format!(
            "Run the first read-only PR watch baseline acknowledgement for {}. Use pr_watch with action=ack_baseline, repo={}, pr={}, watch_id={}, schedule_next=true. Do not push, comment, resolve threads, or merge.",
            state.watch_id, state.pr.repo, state.pr.number, state.watch_id
        );
    }
    format!(
        "Run the next read-only PR watch poll for {}. Use pr_watch with action=poll_now, repo={}, pr={}, watch_id={}, schedule_next=true. Do not push, comment, resolve threads, or merge.",
        state.watch_id, state.pr.repo, state.pr.number, state.watch_id
    )
}

fn scheduled_monitor_prompt(state: &PrWatchState, max_runtime_seconds: u64) -> String {
    format!(
        "Run the next structured PR watch monitor cycle for {}. Use pr_watch with action=monitor, repo={}, pr={}, watch_id={}, schedule_next=true, poll_interval_seconds={}, quiet_cycles_required={}, max_runtime_seconds={}. The monitor is read-only: do not push, comment, resolve threads, or merge.",
        state.watch_id,
        state.pr.repo,
        state.pr.number,
        state.watch_id,
        state.polling.poll_interval_seconds,
        state.polling.required_quiet_cycles,
        max_runtime_seconds,
    )
}

async fn ack_baseline(
    root: &Path,
    store: &Path,
    params: PrWatchInput,
    ctx: &ToolContext,
) -> Result<ToolOutput> {
    let mut state = load_state_for_params(store, &params)?;
    let path = state_path(store, &state.watch_id);
    let loaded_updated_at = state.updated_at.clone();
    let loaded_cycle_number = state.polling.cycle_number;
    let would_write = !params.dry_run.unwrap_or(false);
    if state.terminal {
        let readiness = state.readiness();
        return Ok(ToolOutput::new(format!(
            "PR watch is stopped: {}\nRepo: {}\nPR: #{}\nStop reason: {}\nNo poll was run and no state was changed.",
            state.watch_id,
            state.pr.repo,
            state.pr.number,
            state.stop_reason.as_deref().unwrap_or("terminal")
        ))
        .with_title(format!("{} stopped", state.watch_id))
        .with_metadata(json!({"watch": state, "readiness": readiness, "written": false})));
    }
    let _lock = if would_write {
        match acquire_watch_lock(store, &state.watch_id)? {
            Some(lock) => Some(lock),
            None => return Ok(watch_locked_output(store, &state, "ack_baseline")),
        }
    } else {
        None
    };
    let collected_at = now_iso();
    let collection = collect_with_gh(root, &state.pr.repo, state.pr.number).await;
    let partial_failure = apply_baseline_from_collection(&mut state, collection, &collected_at);
    apply_schedule_fields(&mut state, &params);
    if would_write {
        let current_text = fs::read_to_string(&path).with_context(|| {
            format!(
                "failed to re-read {} before writing poll result",
                path.display()
            )
        })?;
        let current_state = normalize_watch_state_json(&current_text).with_context(|| {
            format!(
                "failed to parse {} before writing poll result",
                path.display()
            )
        })?;
        if current_state.updated_at != loaded_updated_at
            || current_state.polling.cycle_number != loaded_cycle_number
        {
            let readiness = current_state.readiness();
            return Ok(ToolOutput::new(format!(
                "PR watch poll result is stale: {}\nRepo: {}\nPR: #{}\nNo state was changed because another poll updated the watch first.",
                current_state.watch_id, current_state.pr.repo, current_state.pr.number
            ))
            .with_title(format!("{} stale poll", current_state.watch_id))
            .with_metadata(json!({"watch": current_state, "readiness": readiness, "written": false, "stale_poll": true})));
        }
        write_state_atomic(&path, &state)?;
    }
    let scheduled = maybe_schedule_next(ctx, &state, &params)?;
    let text = format!(
        "PR watch baseline acknowledged: {}\nRepo: {}\nPR: #{}\nUnresolved threads: {}\nReview comments seen: {}\nIssue comments seen: {}\nReviews seen: {}\nPartial failure: {}{}{}",
        state.watch_id,
        state.pr.repo,
        state.pr.number,
        state.baseline.unresolved_thread_ids.len(),
        state.last_seen.review_comments.len(),
        state.last_seen.issue_comments.len(),
        state.last_seen.reviews.len(),
        partial_failure,
        scheduled
            .as_deref()
            .map(|s| format!("\nScheduled: {s}"))
            .unwrap_or_default(),
        if would_write {
            ""
        } else {
            "\nDry run: no file written"
        }
    );
    Ok(ToolOutput::new(text)
        .with_title(format!("baseline {}", state.watch_id))
        .with_metadata(
            json!({"watch": state, "partial_failure": partial_failure, "written": would_write}),
        ))
}

fn apply_baseline_from_collection(
    state: &mut PrWatchState,
    collection: GhCollection,
    collected_at: &str,
) -> bool {
    state.updated_at = Some(collected_at.to_string());
    state.baseline.established_at = Some(collected_at.to_string());
    state.polling.quiet_cycles = 0;
    state.pending_actionable.clear();
    state.last_cycle.completed_at = Some(collected_at.to_string());
    state.last_cycle.status = jcode_pr_watch_core::CycleStatus::BaselineEstablished;
    state.last_cycle.actionable_count = 0;
    state.last_cycle.pending_check_count = 0;
    state.last_cycle.failed_check_count = 0;
    state.last_cycle.surfaces_checked = vec![
        "metadata".to_string(),
        "checks".to_string(),
        "review_comments".to_string(),
        "issue_comments".to_string(),
        "reviews".to_string(),
        "review_threads".to_string(),
    ];
    state.last_cycle.surface_counts = BTreeMap::new();
    let mut partial_failure = false;

    match collection.metadata {
        Ok(metadata) => {
            state.pr = metadata.identity;
            state.baseline.head_sha = state.pr.head_sha.clone();
            state
                .last_successful_fetch
                .insert("metadata".to_string(), collected_at.to_string());
            state
                .last_cycle
                .surface_counts
                .insert("metadata".to_string(), 1);
        }
        Err(err) => {
            partial_failure = true;
            state.push_event(surface_error_event(collected_at, err));
        }
    }

    match collection.checks {
        Ok(checks) => {
            state.last_checks_for_sha.head_sha = state.pr.head_sha.clone();
            state.last_checks_for_sha.runs = checks;
            state
                .last_successful_fetch
                .insert("checks".to_string(), collected_at.to_string());
            state
                .last_cycle
                .surface_counts
                .insert("checks".to_string(), state.last_checks_for_sha.runs.len());
        }
        Err(err) => {
            partial_failure = true;
            state.push_event(surface_error_event(collected_at, err));
        }
    }

    match collection.review_comments {
        Ok(comments) => {
            state.baseline.review_comment_count = comments.len();
            state
                .last_cycle
                .surface_counts
                .insert("review_comments".to_string(), comments.len());
            state
                .last_successful_fetch
                .insert("review_comments".to_string(), collected_at.to_string());
            for comment in comments {
                state.last_seen.review_comments.insert(
                    comment.id.clone(),
                    Marker {
                        id: comment.id,
                        updated_at: comment.updated_at,
                        author: comment.author,
                        body_hash: comment.body.as_ref().map(|body| stable_body_hash(body)),
                        url: comment.url,
                    },
                );
            }
        }
        Err(err) => {
            partial_failure = true;
            state.push_event(surface_error_event(collected_at, err));
        }
    }

    match collection.issue_comments {
        Ok(comments) => {
            state.baseline.issue_comment_count = comments.len();
            state
                .last_cycle
                .surface_counts
                .insert("issue_comments".to_string(), comments.len());
            state
                .last_successful_fetch
                .insert("issue_comments".to_string(), collected_at.to_string());
            for comment in comments {
                state.last_seen.issue_comments.insert(
                    comment.id.clone(),
                    Marker {
                        id: comment.id,
                        updated_at: comment.updated_at,
                        author: comment.author,
                        body_hash: comment.body.as_ref().map(|body| stable_body_hash(body)),
                        url: comment.url,
                    },
                );
            }
        }
        Err(err) => {
            partial_failure = true;
            state.push_event(surface_error_event(collected_at, err));
        }
    }

    match collection.reviews {
        Ok(reviews) => {
            state.baseline.review_count = reviews.len();
            state
                .last_cycle
                .surface_counts
                .insert("reviews".to_string(), reviews.len());
            state
                .last_successful_fetch
                .insert("reviews".to_string(), collected_at.to_string());
            for review in reviews {
                state.last_seen.reviews.insert(
                    review.id.clone(),
                    Marker {
                        id: review.id,
                        updated_at: review.submitted_at,
                        author: review.author,
                        body_hash: review.body.as_ref().map(|body| stable_body_hash(body)),
                        url: None,
                    },
                );
            }
        }
        Err(err) => {
            partial_failure = true;
            state.push_event(surface_error_event(collected_at, err));
        }
    }

    match collection.review_threads {
        Ok(threads) => {
            state.baseline.unresolved_thread_ids = threads
                .iter()
                .filter(|thread| !thread.is_resolved && !thread.is_outdated)
                .map(|thread| thread.id.clone())
                .collect();
            state
                .last_cycle
                .surface_counts
                .insert("review_threads".to_string(), threads.len());
            state
                .last_successful_fetch
                .insert("review_threads".to_string(), collected_at.to_string());
            for thread in threads {
                state.last_seen.review_threads.insert(
                    thread.id.clone(),
                    jcode_pr_watch_core::ReviewThreadMarker {
                        id: thread.id,
                        updated_at: thread.updated_at,
                        resolved: thread.is_resolved,
                        outdated: thread.is_outdated,
                        body_hash: thread.body.as_ref().map(|body| stable_body_hash(body)),
                        url: thread.url,
                    },
                );
            }
        }
        Err(err) => {
            partial_failure = true;
            state.push_event(surface_error_event(collected_at, err));
        }
    }

    state.push_event(WatchEvent {
        at: collected_at.to_string(),
        kind: "baseline_acknowledged".to_string(),
        data: json!({
            "partial_failure": partial_failure,
            "review_comment_count": state.baseline.review_comment_count,
            "issue_comment_count": state.baseline.issue_comment_count,
            "review_count": state.baseline.review_count,
            "unresolved_thread_count": state.baseline.unresolved_thread_ids.len(),
        }),
    });
    partial_failure
}

async fn poll_now(
    root: &Path,
    store: &Path,
    params: PrWatchInput,
    ctx: &ToolContext,
) -> Result<ToolOutput> {
    let mut state = load_state_for_params(store, &params)?;
    let path = state_path(store, &state.watch_id);
    let loaded_updated_at = state.updated_at.clone();
    let loaded_cycle_number = state.polling.cycle_number;
    let would_write = !params.dry_run.unwrap_or(false);
    if state.terminal {
        let readiness = state.readiness();
        return Ok(ToolOutput::new(format!(
            "PR watch is stopped: {}\nRepo: {}\nPR: #{}\nStop reason: {}\nNo poll was run and no state was changed.",
            state.watch_id,
            state.pr.repo,
            state.pr.number,
            state.stop_reason.as_deref().unwrap_or("terminal")
        ))
        .with_title(format!("{} stopped", state.watch_id))
        .with_metadata(json!({"watch": state, "readiness": readiness, "written": false})));
    }
    let _lock = if would_write {
        match acquire_watch_lock(store, &state.watch_id)? {
            Some(lock) => Some(lock),
            None => return Ok(watch_locked_output(store, &state, "poll_now")),
        }
    } else {
        None
    };
    let collected_at = now_iso();
    let result = collect_with_gh(root, &state.pr.repo, state.pr.number).await;
    let outcome = update_state_from_collection(&mut state, result, &collected_at);
    apply_schedule_fields(&mut state, &params);
    if state.polling.quiet_cycles >= state.polling.required_quiet_cycles
        && state.pending_actionable.is_empty()
        && state.last_cycle.pending_check_count == 0
        && state.last_cycle.failed_check_count == 0
        && !outcome.partial_failure
    {
        state.terminal = true;
        state.stop_reason = Some("quiet_cycles_satisfied".to_string());
        state.polling.next_poll_at = None;
    }
    if would_write {
        let current_text = fs::read_to_string(&path).with_context(|| {
            format!(
                "failed to re-read {} before writing poll result",
                path.display()
            )
        })?;
        let current_state = normalize_watch_state_json(&current_text).with_context(|| {
            format!(
                "failed to parse {} before writing poll result",
                path.display()
            )
        })?;
        if current_state.updated_at != loaded_updated_at
            || current_state.polling.cycle_number != loaded_cycle_number
        {
            let readiness = current_state.readiness();
            return Ok(ToolOutput::new(format!(
                "PR watch poll result is stale: {}\nRepo: {}\nPR: #{}\nNo state was changed because another poll updated the watch first.",
                current_state.watch_id, current_state.pr.repo, current_state.pr.number
            ))
            .with_title(format!("{} stale poll", current_state.watch_id))
            .with_metadata(json!({"watch": current_state, "readiness": readiness, "written": false, "stale_poll": true})));
        }
        write_state_atomic(&path, &state)?;
    }
    let scheduled = maybe_schedule_next(ctx, &state, &params)?;
    let readiness = state.readiness();
    let text = format!(
        "PR watch polled: {}\nRepo: {}\nPR: #{}\nState: {:?}\nReadiness: {:?}\nQuiet cycles: {}/{}\nActionable: {}\nPending checks: {}\nFailed checks: {}\nPartial failure: {}\nFailed surfaces: {}{}{}",
        state.watch_id,
        state.pr.repo,
        state.pr.number,
        state.last_cycle.status,
        readiness,
        state.polling.quiet_cycles,
        state.polling.required_quiet_cycles,
        state.pending_actionable.len(),
        state.last_cycle.pending_check_count,
        state.last_cycle.failed_check_count,
        outcome.partial_failure,
        failed_surface_names(&state).join(", "),
        scheduled
            .as_deref()
            .map(|s| format!("\nScheduled: {s}"))
            .unwrap_or_default(),
        if would_write {
            ""
        } else {
            "\nDry run: no file written"
        }
    );
    Ok(ToolOutput::new(text)
        .with_title(format!("{} {:?}", state.watch_id, state.last_cycle.status))
        .with_metadata(json!({"watch": state, "readiness": readiness, "written": would_write})))
}

async fn monitor_once(
    root: &Path,
    store: &Path,
    params: PrWatchInput,
    ctx: &ToolContext,
) -> Result<ToolOutput> {
    let watch_id = match &params.watch_id {
        Some(id) => id.clone(),
        None => target_from_params(&params)?.watch_id(),
    };
    let path = state_path(store, &watch_id);
    let Some(_lock) = acquire_watch_lock(store, &watch_id)? else {
        return Ok(ToolOutput::new(format!(
            "PR watch monitor already running: {watch_id}\nLock: {}\nNo state was changed.",
            lock_path(store, &watch_id).display()
        ))
        .with_title(format!("{} monitor locked", watch_id))
        .with_metadata(json!({"watch_id": watch_id, "monitor_status": MonitorStatus::AlreadyRunning.as_str(), "written": false})));
    };

    let loaded_existing_state = path.exists();
    let mut state = if loaded_existing_state {
        load_state_for_params(store, &params)?
    } else {
        let target = target_from_params(&params)?;
        let mut state = PrWatchState::new(target);
        if let Some(id) = &params.watch_id {
            state.watch_id = id.clone();
        }
        state.created_at = Some(now_iso());
        state
    };
    let loaded_updated_at = state.updated_at.clone();
    let loaded_cycle_number = state.polling.cycle_number;

    let max_runtime_seconds = monitor_max_runtime_seconds(&params);
    let would_write = !params.dry_run.unwrap_or(false);
    let mut partial_failure = false;
    let mode;

    if state.terminal {
        mode = "terminal";
    } else {
        let collected_at = now_iso();
        let collection = match tokio::time::timeout(
            StdDuration::from_secs(max_runtime_seconds),
            collect_with_gh(root, &state.pr.repo, state.pr.number),
        )
        .await
        {
            Ok(collection) => collection,
            Err(_) => timed_out_collection(max_runtime_seconds),
        };
        if state.last_successful_fetch.is_empty() {
            partial_failure = apply_baseline_from_collection(&mut state, collection, &collected_at);
            mode = "baseline";
        } else {
            let outcome = update_state_from_collection(&mut state, collection, &collected_at);
            partial_failure = outcome.partial_failure;
            mode = "poll";
        }
        apply_schedule_fields(&mut state, &params);
        if state.polling.quiet_cycles >= state.polling.required_quiet_cycles
            && state.pending_actionable.is_empty()
            && state.last_cycle.pending_check_count == 0
            && state.last_cycle.failed_check_count == 0
            && !partial_failure
        {
            state.terminal = true;
            state.stop_reason = Some("quiet_cycles_satisfied".to_string());
            state.polling.next_poll_at = None;
        }
        state.push_event(WatchEvent {
            at: collected_at,
            kind: "monitor_cycle_completed".to_string(),
            data: json!({
                "mode": mode,
                "max_runtime_seconds": max_runtime_seconds,
                "partial_failure": partial_failure,
            }),
        });
    }

    if would_write {
        fs::create_dir_all(store)?;
        if path.exists() {
            let current_text = fs::read_to_string(&path).with_context(|| {
                format!(
                    "failed to re-read {} before writing monitor result",
                    path.display()
                )
            })?;
            let current_state = normalize_watch_state_json(&current_text).with_context(|| {
                format!(
                    "failed to parse {} before writing monitor result",
                    path.display()
                )
            })?;
            if watch_state_changed_since_load(
                &current_state,
                loaded_existing_state,
                &loaded_updated_at,
                loaded_cycle_number,
            ) {
                let readiness = current_state.readiness();
                return Ok(ToolOutput::new(format!(
                    "PR watch monitor result is stale: {}\nRepo: {}\nPR: #{}\nNo state was changed because another watch action updated the state first.",
                    current_state.watch_id, current_state.pr.repo, current_state.pr.number
                ))
                .with_title(format!("{} stale monitor", current_state.watch_id))
                .with_metadata(json!({"watch": current_state, "readiness": readiness, "monitor_status": "stale", "written": false, "stale_monitor": true})));
            }
        } else if loaded_existing_state {
            return Ok(ToolOutput::new(format!(
                "PR watch monitor result is stale: {watch_id}\nState path disappeared before write: {}\nNo state was changed.",
                path.display()
            ))
            .with_title(format!("{} stale monitor", watch_id))
            .with_metadata(json!({"watch_id": watch_id, "monitor_status": "stale", "written": false, "stale_monitor": true})));
        }
        write_state_atomic(&path, &state)?;
    }
    let status = monitor_status_for_state(&state, partial_failure);
    let scheduled = if monitor_should_schedule_followup(status) {
        maybe_schedule_next_monitor(ctx, &state, &params)?
    } else {
        None
    };
    let readiness = state.readiness();
    let text = format!(
        "PR watch monitor cycle: {}\nRepo: {}\nPR: #{}\nMode: {}\nMonitor status: {}\nState: {:?}\nReadiness: {}\nQuiet cycles: {}/{}\nActionable: {}\nPending checks: {}\nFailed checks: {}\nPartial failure: {}\nMax runtime seconds: {}\nState path: {}{}{}",
        state.watch_id,
        state.pr.repo,
        state.pr.number,
        mode,
        status.as_str(),
        state.last_cycle.status,
        readiness_label(&readiness),
        state.polling.quiet_cycles,
        state.polling.required_quiet_cycles,
        state.pending_actionable.len(),
        state.last_cycle.pending_check_count,
        state.last_cycle.failed_check_count,
        partial_failure,
        max_runtime_seconds,
        path.display(),
        scheduled
            .as_deref()
            .map(|s| format!("\nScheduled: {s}"))
            .unwrap_or_default(),
        if would_write {
            ""
        } else {
            "\nDry run: no file written"
        }
    );
    Ok(ToolOutput::new(text)
        .with_title(format!("{} monitor {}", state.watch_id, status.as_str()))
        .with_metadata(json!({
            "watch": state,
            "readiness": readiness,
            "monitor_status": status.as_str(),
            "monitor_mode": mode,
            "max_runtime_seconds": max_runtime_seconds,
            "scheduled": scheduled,
            "written": would_write,
        })))
}

async fn collect_with_gh(root: &Path, repo: &str, pr: u64) -> GhCollection {
    GhCollection {
        metadata: run_gh(root, &["pr", "view", &pr.to_string(), "--repo", repo, "--json", "url,state,baseRefName,headRefName,headRefOid,mergeStateStatus,reviewDecision,isDraft"]).await
            .and_then(|stdout| parse_gh_pr_view(repo, pr, &stdout).map_err(|err| SurfaceError::transient("metadata", err.to_string()))),
        checks: run_gh_pr_checks(root, repo, pr).await.and_then(|stdout| {
            parse_gh_checks(&stdout).map_err(|err| SurfaceError::transient("checks", err.to_string()))
        }),
        review_comments: run_gh(root, &["api", &format!("repos/{repo}/pulls/{pr}/comments"), "--paginate"]).await
            .and_then(|stdout| parse_gh_review_comments(&stdout).map_err(|err| SurfaceError::transient("review_comments", err.to_string()))),
        issue_comments: run_gh(root, &["api", &format!("repos/{repo}/issues/{pr}/comments"), "--paginate"]).await
            .and_then(|stdout| parse_gh_issue_comments(&stdout).map_err(|err| SurfaceError::transient("issue_comments", err.to_string()))),
        reviews: run_gh(root, &["api", &format!("repos/{repo}/pulls/{pr}/reviews"), "--paginate"]).await
            .and_then(|stdout| parse_gh_reviews(&stdout).map_err(|err| SurfaceError::transient("reviews", err.to_string()))),
        review_threads: run_gh_graphql_review_threads(root, repo, pr).await
            .and_then(|stdout| parse_gh_review_threads(&stdout).map_err(|err| SurfaceError::transient("review_threads", err.to_string()))),
    }
}

#[derive(Debug)]
struct GhCollection {
    metadata: Result<jcode_pr_watch_core::PrMetadata, SurfaceError>,
    checks: Result<Vec<CheckRunState>, SurfaceError>,
    review_comments: Result<Vec<jcode_pr_watch_core::ReviewComment>, SurfaceError>,
    issue_comments: Result<Vec<jcode_pr_watch_core::IssueComment>, SurfaceError>,
    reviews: Result<Vec<jcode_pr_watch_core::Review>, SurfaceError>,
    review_threads: Result<Vec<jcode_pr_watch_core::ReviewThread>, SurfaceError>,
}

async fn run_gh_graphql_review_threads(
    root: &Path,
    repo: &str,
    pr: u64,
) -> Result<String, SurfaceError> {
    let (owner, name) = repo
        .split_once('/')
        .ok_or_else(|| SurfaceError::permanent("review_threads", "repo must be owner/name"))?;
    let pr_s = pr.to_string();
    let mut after: Option<String> = None;
    let mut all_nodes: Vec<Value> = Vec::new();

    loop {
        let after_clause = after
            .as_ref()
            .map(|cursor| format!(", after:\"{}\"", cursor.replace('"', "\\\"")))
            .unwrap_or_default();
        let query = format!(
            r#"
query($owner:String!, $name:String!, $number:Int!) {{
  repository(owner:$owner, name:$name) {{
    pullRequest(number:$number) {{
      reviewThreads(first:100{after_clause}) {{
        pageInfo {{ hasNextPage endCursor }}
        nodes {{
          id
          isResolved
          isOutdated
          comments(first:100) {{
            pageInfo {{ hasNextPage endCursor }}
            nodes {{
              path
              line
              url
              body
              createdAt
              updatedAt
              author {{ login }}
            }}
          }}
        }}
      }}
    }}
  }}
}}
"#
        );
        let stdout = run_gh(
            root,
            &[
                "api",
                "graphql",
                "-f",
                &format!("owner={owner}"),
                "-f",
                &format!("name={name}"),
                "-F",
                &format!("number={pr_s}"),
                "-f",
                &format!("query={query}"),
            ],
        )
        .await?;
        let page: Value = serde_json::from_str(&stdout).map_err(|err| {
            SurfaceError::transient("review_threads", format!("invalid GraphQL JSON: {err}"))
        })?;
        let connection = page
            .pointer("/data/repository/pullRequest/reviewThreads")
            .ok_or_else(|| {
                SurfaceError::transient("review_threads", "missing reviewThreads connection")
            })?;
        if let Some(nodes) = connection.get("nodes").and_then(Value::as_array) {
            for node in nodes {
                let enriched = enrich_review_thread_comments(root, node.clone()).await?;
                all_nodes.push(enriched);
            }
        }
        let page_info = connection.get("pageInfo").unwrap_or(&Value::Null);
        if !page_info
            .get("hasNextPage")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            break;
        }
        after = page_info
            .get("endCursor")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if after.is_none() {
            return Err(SurfaceError::transient(
                "review_threads",
                "reviewThreads pageInfo indicated more pages without an endCursor",
            ));
        }
    }

    Ok(
        json!({"data":{"repository":{"pullRequest":{"reviewThreads":{"nodes": all_nodes}}}}})
            .to_string(),
    )
}

async fn enrich_review_thread_comments(
    root: &Path,
    mut thread: Value,
) -> Result<Value, SurfaceError> {
    let thread_id = thread
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| SurfaceError::transient("review_threads", "thread missing id"))?
        .to_string();
    let mut after = thread
        .pointer("/comments/pageInfo/endCursor")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let mut has_next = thread
        .pointer("/comments/pageInfo/hasNextPage")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    while has_next {
        let after_clause = after
            .as_ref()
            .map(|cursor| format!(", after:\"{}\"", cursor.replace('"', "\\\"")))
            .unwrap_or_default();
        let query = format!(
            r#"
query($threadId:ID!) {{
  node(id:$threadId) {{
    ... on PullRequestReviewThread {{
      comments(first:100{after_clause}) {{
        pageInfo {{ hasNextPage endCursor }}
        nodes {{
          path
          line
          url
          body
          createdAt
          updatedAt
          author {{ login }}
        }}
      }}
    }}
  }}
}}
"#
        );
        let stdout = run_gh(
            root,
            &[
                "api",
                "graphql",
                "-f",
                &format!("threadId={thread_id}"),
                "-f",
                &format!("query={query}"),
            ],
        )
        .await?;
        let page: Value = serde_json::from_str(&stdout).map_err(|err| {
            SurfaceError::transient(
                "review_threads",
                format!("invalid thread comments GraphQL JSON: {err}"),
            )
        })?;
        let comments = page.pointer("/data/node/comments").ok_or_else(|| {
            SurfaceError::transient("review_threads", "missing thread comments connection")
        })?;
        let extra_nodes = comments
            .get("nodes")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if let Some(existing) = thread
            .pointer_mut("/comments/nodes")
            .and_then(Value::as_array_mut)
        {
            existing.extend(extra_nodes);
        }
        let page_info = comments.get("pageInfo").unwrap_or(&Value::Null);
        has_next = page_info
            .get("hasNextPage")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        after = page_info
            .get("endCursor")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        if has_next && after.is_none() {
            return Err(SurfaceError::transient(
                "review_threads",
                "thread comments pageInfo indicated more pages without an endCursor",
            ));
        }
    }
    Ok(thread)
}

async fn run_gh(root: &Path, args: &[&str]) -> Result<String, SurfaceError> {
    run_gh_allow_exit(root, args, &[]).await
}

async fn run_gh_pr_checks(root: &Path, repo: &str, pr: u64) -> Result<String, SurfaceError> {
    let pr_s = pr.to_string();
    let args = [
        "pr",
        "checks",
        &pr_s,
        "--repo",
        repo,
        "--json",
        "name,state,event,link,bucket,workflow,description,startedAt,completedAt",
    ];
    let output = Command::new("gh")
        .args(args)
        .current_dir(root)
        .output()
        .await
        .map_err(|err| SurfaceError::transient("gh", format!("failed to run gh: {err}")))?;
    let code = output.status.code().unwrap_or(-1);
    if output.status.success() || code == 8 {
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }
    if gh_pr_checks_reported_no_checks(code, &output.stdout, &output.stderr) {
        return Ok("[]".to_string());
    }
    Err(SurfaceError::transient(
        "gh",
        String::from_utf8_lossy(&output.stderr).trim().to_string(),
    ))
}

fn gh_pr_checks_reported_no_checks(code: i32, stdout: &[u8], stderr: &[u8]) -> bool {
    if code != 1 {
        return false;
    }
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(stdout),
        String::from_utf8_lossy(stderr)
    )
    .to_ascii_lowercase();
    combined.contains("no checks reported")
}

async fn run_gh_allow_exit(
    root: &Path,
    args: &[&str],
    allowed_nonzero_exit_codes: &[i32],
) -> Result<String, SurfaceError> {
    let output = Command::new("gh")
        .args(args)
        .current_dir(root)
        .output()
        .await
        .map_err(|err| SurfaceError::transient("gh", format!("failed to run gh: {err}")))?;
    let code = output.status.code().unwrap_or(-1);
    if !output.status.success() && !allowed_nonzero_exit_codes.contains(&code) {
        return Err(SurfaceError::transient(
            "gh",
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn update_state_from_collection(
    state: &mut PrWatchState,
    collection: GhCollection,
    collected_at: &str,
) -> CycleOutcome {
    state.updated_at = Some(collected_at.to_string());
    state.last_cycle.completed_at = Some(collected_at.to_string());
    state.last_cycle.surfaces_checked = vec![
        "metadata".to_string(),
        "checks".to_string(),
        "review_comments".to_string(),
        "issue_comments".to_string(),
        "reviews".to_string(),
        "review_threads".to_string(),
    ];
    state.last_cycle.surface_counts = BTreeMap::new();

    let mut partial_failure = false;
    let mut pending_actionable = Vec::new();
    let mut pending_check_count = 0;
    let mut failed_check_count = 0;
    let mut any_surface_success = false;

    match collection.metadata {
        Ok(metadata) => {
            any_surface_success = true;
            let previous_head_sha = state.pr.head_sha.clone();
            let next_head_sha = metadata.identity.head_sha.clone();
            if previous_head_sha.is_some()
                && next_head_sha.is_some()
                && previous_head_sha != next_head_sha
            {
                state.polling.quiet_cycles = 0;
                state.baseline.head_sha = next_head_sha.clone();
                state.push_event(WatchEvent {
                    at: collected_at.to_string(),
                    kind: "head_changed".to_string(),
                    data: json!({
                        "previous_head_sha": previous_head_sha,
                        "new_head_sha": next_head_sha,
                    }),
                });
            }
            state.pr = metadata.identity;
            state
                .last_successful_fetch
                .insert("metadata".to_string(), collected_at.to_string());
            state
                .last_cycle
                .surface_counts
                .insert("metadata".to_string(), 1);
        }
        Err(err) => {
            partial_failure = true;
            state.push_event(surface_error_event(collected_at, err));
        }
    }

    match collection.checks {
        Ok(checks) => {
            any_surface_success = true;
            pending_check_count = checks
                .iter()
                .filter(|check| is_pending_check(check))
                .count();
            failed_check_count = checks.iter().filter(|check| is_failed_check(check)).count();
            state.last_checks_for_sha.head_sha = state.pr.head_sha.clone();
            state.last_checks_for_sha.runs = checks;
            state
                .last_successful_fetch
                .insert("checks".to_string(), collected_at.to_string());
            state
                .last_cycle
                .surface_counts
                .insert("checks".to_string(), state.last_checks_for_sha.runs.len());
        }
        Err(err) => {
            partial_failure = true;
            state.push_event(surface_error_event(collected_at, err));
        }
    }

    match collection.review_comments {
        Ok(comments) => {
            any_surface_success = true;
            state
                .last_cycle
                .surface_counts
                .insert("review_comments".to_string(), comments.len());
            state
                .last_successful_fetch
                .insert("review_comments".to_string(), collected_at.to_string());
            for comment in comments {
                let previous = state.last_seen.review_comments.get(&comment.id);
                let body_hash = comment.body.as_ref().map(|body| stable_body_hash(body));
                let is_new = previous.is_none();
                let is_edited = previous
                    .map(|marker| {
                        marker.updated_at != comment.updated_at || marker.body_hash != body_hash
                    })
                    .unwrap_or(false);
                state.last_seen.review_comments.insert(
                    comment.id.clone(),
                    Marker {
                        id: comment.id.clone(),
                        updated_at: comment.updated_at.clone(),
                        author: comment.author.clone(),
                        body_hash,
                        url: comment.url.clone(),
                    },
                );
                if (is_new || is_edited)
                    && !is_automation_chatter(comment.author.as_deref(), comment.body.as_deref())
                {
                    pending_actionable.push(ActionableItem {
                        id: comment.id,
                        surface: "review_comments".to_string(),
                        summary: comment
                            .body
                            .unwrap_or_else(|| "New review comment".to_string()),
                        url: comment.url,
                        path: comment.path,
                        status: Some(if is_edited { "edited" } else { "new" }.to_string()),
                    });
                }
            }
        }
        Err(err) => {
            partial_failure = true;
            state.push_event(surface_error_event(collected_at, err));
        }
    }

    match collection.issue_comments {
        Ok(comments) => {
            any_surface_success = true;
            state
                .last_cycle
                .surface_counts
                .insert("issue_comments".to_string(), comments.len());
            state
                .last_successful_fetch
                .insert("issue_comments".to_string(), collected_at.to_string());
            for comment in comments {
                let previous = state.last_seen.issue_comments.get(&comment.id);
                let body_hash = comment.body.as_ref().map(|body| stable_body_hash(body));
                let is_new = previous.is_none();
                let is_edited = previous
                    .map(|marker| {
                        marker.updated_at != comment.updated_at || marker.body_hash != body_hash
                    })
                    .unwrap_or(false);
                state.last_seen.issue_comments.insert(
                    comment.id.clone(),
                    Marker {
                        id: comment.id.clone(),
                        updated_at: comment.updated_at.clone(),
                        author: comment.author.clone(),
                        body_hash,
                        url: comment.url.clone(),
                    },
                );
                if (is_new || is_edited)
                    && !is_automation_chatter(comment.author.as_deref(), comment.body.as_deref())
                {
                    pending_actionable.push(ActionableItem {
                        id: comment.id,
                        surface: "issue_comments".to_string(),
                        summary: comment
                            .body
                            .unwrap_or_else(|| "New issue comment".to_string()),
                        url: comment.url,
                        path: None,
                        status: Some(if is_edited { "edited" } else { "new" }.to_string()),
                    });
                }
            }
        }
        Err(err) => {
            partial_failure = true;
            state.push_event(surface_error_event(collected_at, err));
        }
    }

    match collection.review_threads {
        Ok(threads) => {
            any_surface_success = true;
            state
                .last_cycle
                .surface_counts
                .insert("review_threads".to_string(), threads.len());
            state
                .last_successful_fetch
                .insert("review_threads".to_string(), collected_at.to_string());
            state.baseline.unresolved_thread_ids = threads
                .iter()
                .filter(|thread| !thread.is_resolved && !thread.is_outdated)
                .map(|thread| thread.id.clone())
                .collect();
            for thread in threads {
                let previous = state.last_seen.review_threads.get(&thread.id);
                let body_hash = thread.body.as_ref().map(|body| stable_body_hash(body));
                let has_new_reply = previous
                    .and_then(|marker| marker.body_hash.as_ref())
                    .zip(body_hash.as_ref())
                    .map(|(old, new)| old != new)
                    .unwrap_or(false);
                state.last_seen.review_threads.insert(
                    thread.id.clone(),
                    jcode_pr_watch_core::ReviewThreadMarker {
                        id: thread.id.clone(),
                        updated_at: thread.updated_at.clone(),
                        resolved: thread.is_resolved,
                        outdated: thread.is_outdated,
                        body_hash,
                        url: thread.url.clone(),
                    },
                );
                if !thread.is_resolved && !thread.is_outdated {
                    pending_actionable.push(ActionableItem {
                        id: thread.id,
                        surface: "review_threads".to_string(),
                        summary: thread
                            .body
                            .unwrap_or_else(|| "Unresolved review thread".to_string()),
                        url: thread.url,
                        path: thread.path,
                        status: Some(
                            if has_new_reply {
                                "new_reply"
                            } else {
                                "unresolved"
                            }
                            .to_string(),
                        ),
                    });
                }
            }
        }
        Err(err) => {
            partial_failure = true;
            state.push_event(surface_error_event(collected_at, err));
        }
    }

    match collection.reviews {
        Ok(reviews) => {
            any_surface_success = true;
            state
                .last_cycle
                .surface_counts
                .insert("reviews".to_string(), reviews.len());
            state
                .last_successful_fetch
                .insert("reviews".to_string(), collected_at.to_string());
            for review in reviews {
                let is_new = !state.last_seen.reviews.contains_key(&review.id);
                state.last_seen.reviews.insert(
                    review.id.clone(),
                    Marker {
                        id: review.id.clone(),
                        updated_at: review.submitted_at.clone(),
                        author: review.author.clone(),
                        body_hash: review.body.as_ref().map(|body| stable_body_hash(body)),
                        url: None,
                    },
                );
                if is_new && review.state.as_deref() == Some("CHANGES_REQUESTED") {
                    pending_actionable.push(ActionableItem {
                        id: review.id,
                        surface: "reviews".to_string(),
                        summary: review
                            .body
                            .unwrap_or_else(|| "Review requested changes".to_string()),
                        url: None,
                        path: None,
                        status: review.state,
                    });
                }
            }
        }
        Err(err) => {
            partial_failure = true;
            state.push_event(surface_error_event(collected_at, err));
        }
    }

    if partial_failure && !any_surface_success {
        pending_actionable = state.pending_actionable.clone();
        pending_check_count = state.last_cycle.pending_check_count;
        failed_check_count = state.last_cycle.failed_check_count;
    }

    let outcome = CycleOutcome {
        pending_actionable,
        pending_check_count,
        failed_check_count,
        partial_failure,
    };
    state.apply_cycle_outcome(outcome.clone());
    state.push_event(WatchEvent {
        at: collected_at.to_string(),
        kind: "poll_completed".to_string(),
        data: json!({
            "status": format!("{:?}", state.last_cycle.status),
            "actionable_count": state.pending_actionable.len(),
            "pending_check_count": state.last_cycle.pending_check_count,
            "failed_check_count": state.last_cycle.failed_check_count,
            "partial_failure": partial_failure,
        }),
    });
    outcome
}

fn surface_error_event(at: &str, err: SurfaceError) -> WatchEvent {
    WatchEvent {
        at: at.to_string(),
        kind: "surface_error".to_string(),
        data: json!({"surface": err.surface, "message": err.message, "transient": err.transient}),
    }
}

fn failed_surface_names(state: &PrWatchState) -> Vec<String> {
    state
        .events
        .iter()
        .rev()
        .take(10)
        .filter_map(|event| {
            if event.kind == "surface_error" {
                event
                    .data
                    .get("surface")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            } else {
                None
            }
        })
        .collect()
}

fn is_pending_check(check: &CheckRunState) -> bool {
    let status = check
        .status
        .as_deref()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let conclusion = check
        .conclusion
        .as_deref()
        .unwrap_or_default()
        .to_ascii_uppercase();
    matches!(
        status.as_str(),
        "IN_PROGRESS" | "QUEUED" | "PENDING" | "WAITING" | "REQUESTED"
    ) || (conclusion.is_empty()
        && !matches!(
            status.as_str(),
            "COMPLETED" | "SUCCESS" | "FAILURE" | "ERROR" | "CANCELLED" | "SKIPPED"
        ))
}

fn is_failed_check(check: &CheckRunState) -> bool {
    let conclusion = check
        .conclusion
        .as_deref()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let status = check
        .status
        .as_deref()
        .unwrap_or_default()
        .to_ascii_uppercase();
    matches!(
        conclusion.as_str(),
        "FAILURE" | "FAIL" | "ERROR" | "TIMED_OUT" | "ACTION_REQUIRED" | "CANCELLED" | "CANCEL"
    ) || matches!(
        status.as_str(),
        "FAILURE" | "FAIL" | "ERROR" | "FAILED" | "CANCELLED" | "CANCEL"
    )
}

fn is_automation_chatter(_author: Option<&str>, body: Option<&str>) -> bool {
    let body = body.unwrap_or_default().to_ascii_lowercase();
    body.starts_with("fix-summary:")
        || body.contains("triggered the review bot")
        || body.contains("automation progress")
        || body.contains("<!-- jcode-pr-watch-ignore -->")
}

fn stable_body_hash(body: &str) -> String {
    let mut hasher = 0xcbf29ce484222325u64;
    for byte in body.as_bytes() {
        hasher ^= u64::from(*byte);
        hasher = hasher.wrapping_mul(0x100000001b3);
    }
    format!("hash:{:016x}", hasher)
}

fn now_iso() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn status_like(store: &Path, params: PrWatchInput) -> Result<ToolOutput> {
    let state = load_state_for_params(store, &params)?;
    let readiness = state.readiness();
    let text = format_status_report(&state, readiness_label(&readiness));
    Ok(ToolOutput::new(text)
        .with_title(format!(
            "{} {}",
            state.watch_id,
            readiness_label(&readiness)
        ))
        .with_metadata(json!({"watch": state, "readiness": readiness})))
}

fn readiness_report(store: &Path, params: PrWatchInput) -> Result<ToolOutput> {
    let state = load_state_for_params(store, &params)?;
    let readiness = state.readiness();
    let mut text = format_status_report(&state, readiness_label(&readiness));
    text.push_str("\n\nReadiness decision:\n");
    text.push_str(&format!("- {}\n", readiness_label(&readiness)));
    for reason in readiness_reasons(&state) {
        text.push_str(&format!("- {}\n", reason));
    }
    Ok(ToolOutput::new(text)
        .with_title(format!("readiness {}", readiness_label(&readiness)))
        .with_metadata(json!({"watch": state, "readiness": readiness})))
}

fn handoff_report(store: &Path, params: PrWatchInput) -> Result<ToolOutput> {
    let state = load_state_for_params(store, &params)?;
    let readiness = state.readiness();
    let mut text = String::new();
    text.push_str(&format!("# PR watch handoff: {}\n\n", state.watch_id));
    text.push_str(&format!("- PR: {}/#{}\n", state.pr.repo, state.pr.number));
    if let Some(url) = &state.pr.url {
        text.push_str(&format!("- URL: {}\n", url));
    }
    text.push_str(&format!("- Readiness: {}\n", readiness_label(&readiness)));
    text.push_str(&format!(
        "- Current status: {:?}\n",
        state.last_cycle.status
    ));
    text.push_str(&format!(
        "- Quiet cycles: {}/{}\n",
        state.polling.quiet_cycles, state.polling.required_quiet_cycles
    ));
    text.push_str("\n## Evidence\n");
    for line in evidence_lines(&state) {
        text.push_str(&format!("- {}\n", line));
    }
    text.push_str("\n## Pending actionable items\n");
    if state.pending_actionable.is_empty() {
        text.push_str("- None recorded.\n");
    } else {
        for item in &state.pending_actionable {
            text.push_str(&format!(
                "- [{}] {}{}\n",
                item.surface,
                item.summary,
                item.url
                    .as_ref()
                    .map(|u| format!(" ({u})"))
                    .unwrap_or_default()
            ));
        }
    }
    text.push_str("\n## Human next step\n");
    text.push_str(&human_next_step(&state));
    text.push('\n');
    text.push_str("\nNo mutation was performed by this report. Do not merge unless repository policy and a human maintainer approve it.\n");
    Ok(ToolOutput::new(text)
        .with_title(format!("handoff {}", readiness_label(&readiness)))
        .with_metadata(json!({"watch": state, "readiness": readiness})))
}

fn format_status_report(state: &PrWatchState, readiness: &str) -> String {
    let mut text = format!(
        "PR watch: {}\nRepo: {}\nPR: #{}\nState: {:?}\nReadiness: {}\nQuiet cycles: {}/{}\nActionable: {}\nPending checks: {}\nFailed checks: {}\nUnresolved threads: {}\nNext poll: {}\nPolicy: local_fix={}, commit={}, push={}, comment={}, resolve_threads={}",
        state.watch_id,
        state.pr.repo,
        state.pr.number,
        state.last_cycle.status,
        readiness,
        state.polling.quiet_cycles,
        state.polling.required_quiet_cycles,
        state.pending_actionable.len(),
        state.last_cycle.pending_check_count,
        state.last_cycle.failed_check_count,
        state.baseline.unresolved_thread_ids.len(),
        state
            .polling
            .next_poll_at
            .as_deref()
            .unwrap_or("not scheduled"),
        state.policy.local_fix,
        state.policy.commit,
        state.policy.push,
        state.policy.comment,
        state.policy.resolve_threads,
    );
    if !state.last_successful_fetch.is_empty() {
        text.push_str("\nLast successful fetch:");
        for (surface, at) in &state.last_successful_fetch {
            text.push_str(&format!("\n- {}: {}", surface, at));
        }
    }
    text
}

fn readiness_label(readiness: &jcode_pr_watch_core::Readiness) -> &'static str {
    match readiness {
        jcode_pr_watch_core::Readiness::NotReadyActionRequired => "not_ready_action_required",
        jcode_pr_watch_core::Readiness::NotReadyChecksPending => "not_ready_checks_pending",
        jcode_pr_watch_core::Readiness::NotReadyChecksFailed => "not_ready_checks_failed",
        jcode_pr_watch_core::Readiness::NotReadyValidationStale => "not_ready_validation_stale",
        jcode_pr_watch_core::Readiness::ReadyForHumanReview => "ready_for_human_review",
        jcode_pr_watch_core::Readiness::ReadyForHumanPush => "ready_for_human_push",
        jcode_pr_watch_core::Readiness::ReadyForHumanMerge => "ready_for_human_merge",
        jcode_pr_watch_core::Readiness::BlockedByPolicy => "blocked_by_policy",
        jcode_pr_watch_core::Readiness::BlockedByClosedPr => "blocked_by_closed_pr",
    }
}

fn readiness_reasons(state: &PrWatchState) -> Vec<String> {
    let mut reasons = Vec::new();
    if state.pr.state.as_deref() != Some("OPEN") && state.pr.state.is_some() {
        reasons.push("PR is not open.".to_string());
    }
    if !state.pending_actionable.is_empty() {
        reasons.push(format!(
            "{} actionable item(s) need attention.",
            state.pending_actionable.len()
        ));
    }
    if state.last_cycle.failed_check_count > 0 {
        reasons.push(format!(
            "{} check(s) failed.",
            state.last_cycle.failed_check_count
        ));
    }
    if state.last_cycle.pending_check_count > 0 {
        reasons.push(format!(
            "{} check(s) are pending.",
            state.last_cycle.pending_check_count
        ));
    }
    if state.polling.quiet_cycles < state.polling.required_quiet_cycles {
        reasons.push(format!(
            "Quiet cycle requirement not yet met: {}/{}.",
            state.polling.quiet_cycles, state.polling.required_quiet_cycles
        ));
    }
    if state.last_successful_fetch.is_empty() {
        reasons.push("No successful fetch evidence recorded yet.".to_string());
    }
    if reasons.is_empty() {
        reasons.push("Required quiet cycles are satisfied and no actionable items or blocking checks are recorded.".to_string());
    }
    reasons
}

fn evidence_lines(state: &PrWatchState) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!(
        "Last cycle completed: {}",
        state
            .last_cycle
            .completed_at
            .as_deref()
            .unwrap_or("unknown")
    ));
    lines.push(format!(
        "Surfaces checked: {}",
        if state.last_cycle.surfaces_checked.is_empty() {
            "none".to_string()
        } else {
            state.last_cycle.surfaces_checked.join(", ")
        }
    ));
    lines.push(format!(
        "Checks: {} pending, {} failed",
        state.last_cycle.pending_check_count, state.last_cycle.failed_check_count
    ));
    lines.push(format!(
        "Actionable items: {}",
        state.pending_actionable.len()
    ));
    lines.push(format!(
        "Unresolved review threads at baseline/latest poll: {}",
        state.baseline.unresolved_thread_ids.len()
    ));
    if let Some(head) = &state.pr.head_sha {
        lines.push(format!("Head SHA: {}", head));
    }
    if let Some(next) = &state.polling.next_poll_at {
        lines.push(format!("Next scheduled poll: {}", next));
    }
    lines
}

fn human_next_step(state: &PrWatchState) -> String {
    let readiness = state.readiness();
    match readiness {
        jcode_pr_watch_core::Readiness::ReadyForHumanMerge => format!(
            "- Human maintainer may review repository policy and choose an approved merge strategy, for example: `gh pr merge {} --repo {} [--squash|--merge|--rebase]`",
            state.pr.number, state.pr.repo
        ),
        jcode_pr_watch_core::Readiness::NotReadyActionRequired => "- Address actionable review feedback, validate locally, then run `pr_watch poll_now` again.".to_string(),
        jcode_pr_watch_core::Readiness::NotReadyChecksPending => "- Wait for pending checks, then run `pr_watch poll_now` again.".to_string(),
        jcode_pr_watch_core::Readiness::NotReadyChecksFailed => "- Investigate failing checks, fix locally if appropriate, then run `pr_watch poll_now` again.".to_string(),
        jcode_pr_watch_core::Readiness::BlockedByClosedPr => "- PR is closed; stop the watcher or reopen the PR before continuing.".to_string(),
        _ => "- Continue monitoring until quiet-cycle and validation requirements are satisfied.".to_string(),
    }
}

fn stop_watch(store: &Path, params: PrWatchInput) -> Result<ToolOutput> {
    let mut state = load_state_for_params(store, &params)?;
    let would_write = !params.dry_run.unwrap_or(false);
    let _lock = if would_write {
        match acquire_watch_lock(store, &state.watch_id)? {
            Some(lock) => Some(lock),
            None => return Ok(watch_locked_output(store, &state, "stop")),
        }
    } else {
        None
    };
    state.terminal = true;
    state.stop_reason = Some("stopped_by_pr_watch_tool".to_string());
    state.polling.next_poll_at = None;
    state.last_cycle.status = jcode_pr_watch_core::CycleStatus::Stopped;
    let path = state_path(store, &state.watch_id);
    if would_write {
        write_state_atomic(&path, &state)?;
    }
    Ok(ToolOutput::new(format!(
        "PR watch stopped: {}{}",
        state.watch_id,
        if would_write {
            ""
        } else {
            "\nDry run: no file written"
        }
    ))
    .with_title(format!("stopped {}", state.watch_id))
    .with_metadata(json!({"watch": state, "written": would_write})))
}

fn load_state_for_params(store: &Path, params: &PrWatchInput) -> Result<PrWatchState> {
    let watch_id = match &params.watch_id {
        Some(id) => id.clone(),
        None => target_from_params(params)?.watch_id(),
    };
    let path = state_path(store, &watch_id);
    let text =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    normalize_watch_state_json(&text).with_context(|| format!("failed to parse {}", path.display()))
}

fn load_all_states(store: &Path) -> Result<Vec<(PathBuf, PrWatchState)>> {
    let mut states = Vec::new();
    if !store.exists() {
        return Ok(states);
    }
    for entry in fs::read_dir(store)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        if !path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .ends_with("-state.json")
        {
            continue;
        }
        let text = fs::read_to_string(&path)?;
        if let Ok(state) = normalize_watch_state_json(&text) {
            states.push((path, state));
        }
    }
    states.sort_by(|a, b| a.1.watch_id.cmp(&b.1.watch_id));
    Ok(states)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_path_uses_watch_id() {
        assert_eq!(
            state_path(Path::new("/tmp/watch"), "owner-repo-pr-1"),
            PathBuf::from("/tmp/watch/owner-repo-pr-1-state.json")
        );
    }

    #[test]
    fn schema_lists_read_only_actions() {
        let schema = PrWatchTool::new().parameters_schema();
        let actions = schema
            .pointer("/properties/action/enum")
            .unwrap()
            .as_array()
            .unwrap();
        assert!(actions.iter().any(|value| value == "start"));
        assert!(actions.iter().any(|value| value == "poll_now"));
        assert!(actions.iter().any(|value| value == "monitor"));
        assert!(actions.iter().any(|value| value == "ack_baseline"));
        assert!(
            schema
                .pointer("/properties/quiet_cycles_required")
                .is_some()
        );
        assert!(schema.pointer("/properties/max_runtime_seconds").is_some());
        assert!(!actions.iter().any(|value| value == "authorize"));
    }

    fn monitor_params(max_runtime_seconds: Option<u64>) -> PrWatchInput {
        PrWatchInput {
            action: PrWatchAction::Monitor,
            repo: Some("owner/repo".to_string()),
            pr: Some(7),
            watch_id: None,
            dry_run: None,
            schedule_next: false,
            poll_interval_seconds: None,
            quiet_cycles_required: None,
            max_runtime_seconds,
            target: None,
        }
    }

    #[test]
    fn monitor_defaults_are_bounded() {
        assert_eq!(
            monitor_max_runtime_seconds(&monitor_params(None)),
            DEFAULT_MONITOR_MAX_RUNTIME_SECONDS
        );
        assert!(DEFAULT_MONITOR_MAX_RUNTIME_SECONDS <= 540);
        assert_eq!(
            monitor_max_runtime_seconds(&monitor_params(Some(5_000))),
            MAX_MONITOR_MAX_RUNTIME_SECONDS
        );
        assert_eq!(monitor_max_runtime_seconds(&monitor_params(Some(0))), 1);
    }

    #[test]
    fn monitor_lock_prevents_concurrent_runs() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let first = acquire_watch_lock(temp.path(), "owner-repo-pr-7")
            .expect("first lock")
            .expect("lock acquired");
        let second = acquire_watch_lock(temp.path(), "owner-repo-pr-7").expect("second lock");
        assert!(second.is_none());
        drop(first);
        let third = acquire_watch_lock(temp.path(), "owner-repo-pr-7")
            .expect("third lock")
            .expect("lock reacquired");
        drop(third);
    }

    #[test]
    fn watch_locked_output_uses_shared_lock_metadata() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let state = PrWatchState::new(PrTarget {
            repo: "owner/repo".into(),
            number: 7,
        });

        let output = watch_locked_output(temp.path(), &state, "poll_now");
        assert!(output.text.contains("already running or locked"));
        assert!(output.text.contains("owner/repo"));
        assert_eq!(
            output.metadata.pointer("/watch_locked"),
            Some(&Value::Bool(true))
        );
        assert_eq!(
            output.metadata.pointer("/action"),
            Some(&Value::String("poll_now".to_string()))
        );
    }

    #[test]
    fn monitor_status_maps_actionable_and_checks() {
        let mut state = PrWatchState::new(PrTarget {
            repo: "owner/repo".into(),
            number: 7,
        });
        state.pending_actionable.push(ActionableItem {
            id: "thread-1".into(),
            surface: "review_threads".into(),
            summary: "fix it".into(),
            url: None,
            path: None,
            status: Some("unresolved".into()),
        });
        assert_eq!(
            monitor_status_for_state(&state, false),
            MonitorStatus::ActionRequired
        );
        state.pending_actionable.clear();
        state.last_cycle.failed_check_count = 1;
        assert_eq!(
            monitor_status_for_state(&state, false),
            MonitorStatus::ChecksFailed
        );
        state.last_cycle.failed_check_count = 0;
        state.last_cycle.pending_check_count = 1;
        assert_eq!(
            monitor_status_for_state(&state, false),
            MonitorStatus::ChecksPending
        );
        state.last_cycle.pending_check_count = 0;
        assert_eq!(
            monitor_status_for_state(&state, true),
            MonitorStatus::TransientFailure
        );
        state.polling.quiet_cycles = state.polling.required_quiet_cycles;
        assert_eq!(
            monitor_status_for_state(&state, false),
            MonitorStatus::QuietSatisfied
        );
    }

    #[test]
    fn monitor_should_schedule_recoverable_statuses() {
        assert!(monitor_should_schedule_followup(
            MonitorStatus::PendingNextPoll
        ));
        assert!(monitor_should_schedule_followup(
            MonitorStatus::ChecksPending
        ));
        assert!(monitor_should_schedule_followup(
            MonitorStatus::TransientFailure
        ));
        assert!(!monitor_should_schedule_followup(
            MonitorStatus::ActionRequired
        ));
        assert!(!monitor_should_schedule_followup(
            MonitorStatus::ChecksFailed
        ));
        assert!(!monitor_should_schedule_followup(
            MonitorStatus::QuietSatisfied
        ));
        assert!(!monitor_should_schedule_followup(MonitorStatus::Stopped));
    }

    #[test]
    fn monitor_stale_guard_detects_concurrent_state_changes() {
        let mut state = PrWatchState::new(PrTarget {
            repo: "owner/repo".into(),
            number: 7,
        });
        state.updated_at = Some("2026-05-14T11:00:00Z".into());
        state.polling.cycle_number = 2;

        assert!(!watch_state_changed_since_load(
            &state,
            true,
            &state.updated_at,
            state.polling.cycle_number,
        ));

        let mut updated_at_changed = state.clone();
        updated_at_changed.updated_at = Some("2026-05-14T11:01:00Z".into());
        assert!(watch_state_changed_since_load(
            &updated_at_changed,
            true,
            &state.updated_at,
            state.polling.cycle_number,
        ));

        let mut cycle_changed = state.clone();
        cycle_changed.polling.cycle_number += 1;
        assert!(watch_state_changed_since_load(
            &cycle_changed,
            true,
            &state.updated_at,
            state.polling.cycle_number,
        ));

        assert!(watch_state_changed_since_load(
            &state,
            false,
            &state.updated_at,
            state.polling.cycle_number,
        ));
    }

    #[test]
    fn timed_out_collection_marks_all_surfaces_transient() {
        let collection = timed_out_collection(12);
        for (surface, result) in [
            ("metadata", collection.metadata.map(|_| ())),
            ("checks", collection.checks.map(|_| ())),
            ("review_comments", collection.review_comments.map(|_| ())),
            ("issue_comments", collection.issue_comments.map(|_| ())),
            ("reviews", collection.reviews.map(|_| ())),
            ("review_threads", collection.review_threads.map(|_| ())),
        ] {
            let err = result.expect_err("surface should time out");
            assert_eq!(err.surface, surface);
            assert!(err.transient);
            assert!(err.message.contains("max_runtime_seconds=12"));
        }
    }

    #[test]
    fn total_transient_failure_preserves_existing_blockers() {
        let mut state = PrWatchState::new(PrTarget {
            repo: "owner/repo".into(),
            number: 7,
        });
        state.pending_actionable.push(ActionableItem {
            id: "THREAD_BLOCKER".into(),
            surface: "review_threads".into(),
            summary: "Existing unresolved thread".into(),
            url: Some("https://thread".into()),
            path: Some("src/lib.rs".into()),
            status: Some("unresolved".into()),
        });
        state.last_cycle.pending_check_count = 1;
        state.last_cycle.failed_check_count = 1;

        let outcome = update_state_from_collection(
            &mut state,
            timed_out_collection(12),
            "2026-05-14T13:00:00Z",
        );

        assert!(outcome.partial_failure);
        assert_eq!(outcome.pending_actionable.len(), 1);
        assert_eq!(outcome.pending_actionable[0].id, "THREAD_BLOCKER");
        assert_eq!(outcome.pending_check_count, 1);
        assert_eq!(outcome.failed_check_count, 1);
        assert_eq!(state.pending_actionable.len(), 1);
        assert_eq!(state.last_cycle.pending_check_count, 1);
        assert_eq!(state.last_cycle.failed_check_count, 1);
    }

    #[test]
    fn scheduled_monitor_prompt_is_structured() {
        let mut state = PrWatchState::new(PrTarget {
            repo: "owner/repo".into(),
            number: 7,
        });
        state.polling.poll_interval_seconds = 120;
        state.polling.required_quiet_cycles = 2;
        let prompt = scheduled_monitor_prompt(&state, 540);
        assert!(prompt.contains("action=monitor"));
        assert!(prompt.contains("repo=owner/repo"));
        assert!(prompt.contains("pr=7"));
        assert!(prompt.contains("poll_interval_seconds=120"));
        assert!(prompt.contains("quiet_cycles_required=2"));
        assert!(prompt.contains("max_runtime_seconds=540"));
        assert!(prompt.contains("read-only"));
    }

    #[test]
    fn check_classification_detects_pending_and_failed() {
        let pending = CheckRunState {
            id: None,
            name: "ci".into(),
            status: Some("IN_PROGRESS".into()),
            conclusion: None,
            url: None,
        };
        let failed = CheckRunState {
            id: None,
            name: "lint".into(),
            status: Some("COMPLETED".into()),
            conclusion: Some("FAILURE".into()),
            url: None,
        };
        let passed = CheckRunState {
            id: None,
            name: "test".into(),
            status: Some("COMPLETED".into()),
            conclusion: Some("SUCCESS".into()),
            url: None,
        };
        assert!(is_pending_check(&pending));
        assert!(!is_failed_check(&pending));
        assert!(is_failed_check(&failed));
        assert!(!is_pending_check(&passed));
        assert!(!is_failed_check(&passed));
    }

    #[test]
    fn gh_pr_checks_no_checks_exit_one_is_not_transient_failure() {
        assert!(gh_pr_checks_reported_no_checks(
            1,
            b"",
            b"no checks reported on the 'feature' branch"
        ));
        assert!(gh_pr_checks_reported_no_checks(
            1,
            b"No checks reported on the 'feature' branch",
            b""
        ));
    }

    #[test]
    fn gh_pr_checks_classifier_keeps_pending_and_real_failures_distinct() {
        assert!(!gh_pr_checks_reported_no_checks(
            8,
            br#"[{"name":"ci","state":"IN_PROGRESS"}]"#,
            b""
        ));
        assert!(!gh_pr_checks_reported_no_checks(
            1,
            b"",
            b"HTTP 404: Not Found"
        ));
        assert!(!gh_pr_checks_reported_no_checks(
            2,
            b"",
            b"no checks reported"
        ));
    }

    #[test]
    fn update_state_from_collection_preserves_partial_failure_and_actionable() {
        let mut state = PrWatchState::new(PrTarget {
            repo: "owner/repo".into(),
            number: 9,
        });
        let collection = GhCollection {
            metadata: Ok(jcode_pr_watch_core::PrMetadata {
                identity: jcode_pr_watch_core::PrIdentity {
                    repo: "owner/repo".into(),
                    number: 9,
                    url: Some("https://github.com/owner/repo/pull/9".into()),
                    state: Some("OPEN".into()),
                    base_ref: Some("main".into()),
                    head_ref: Some("feature".into()),
                    head_sha: Some("abc".into()),
                    merge_state: Some("CLEAN".into()),
                    review_decision: None,
                },
                is_draft: Some(false),
            }),
            checks: Ok(vec![CheckRunState {
                id: Some("1".into()),
                name: "ci".into(),
                status: Some("COMPLETED".into()),
                conclusion: Some("SUCCESS".into()),
                url: None,
            }]),
            review_comments: Ok(vec![jcode_pr_watch_core::ReviewComment {
                id: "RC_1".into(),
                path: Some("src/lib.rs".into()),
                line: Some(7),
                url: Some("https://comment".into()),
                updated_at: Some("2026-05-13T17:00:00Z".into()),
                author: Some("reviewer".into()),
                body: Some("Please fix".into()),
            }]),
            issue_comments: Err(SurfaceError::transient("issue_comments", "timeout")),
            reviews: Ok(Vec::new()),
            review_threads: Ok(Vec::new()),
        };
        let outcome = update_state_from_collection(&mut state, collection, "2026-05-13T17:00:00Z");
        assert!(outcome.partial_failure);
        assert_eq!(state.pending_actionable.len(), 1);
        assert_eq!(
            state.last_cycle.status,
            jcode_pr_watch_core::CycleStatus::ActionRequired
        );
        assert!(state.last_seen.review_comments.contains_key("RC_1"));
        assert!(
            state
                .events
                .iter()
                .any(|event| event.kind == "surface_error")
        );
    }

    #[test]
    fn readiness_reasons_explain_actionable_items() {
        let mut state = PrWatchState::new(PrTarget {
            repo: "owner/repo".into(),
            number: 14,
        });
        state.pending_actionable.push(ActionableItem {
            id: "a1".into(),
            surface: "review_threads".into(),
            summary: "Fix thread".into(),
            url: Some("https://thread".into()),
            path: Some("src/lib.rs".into()),
            status: Some("unresolved".into()),
        });
        let reasons = readiness_reasons(&state);
        assert!(reasons.iter().any(|reason| reason.contains("actionable")));
        assert!(human_next_step(&state).contains("Address actionable"));
    }

    #[test]
    fn handoff_helpers_include_merge_template_only_when_ready() {
        let mut state = PrWatchState::new(PrTarget {
            repo: "owner/repo".into(),
            number: 15,
        });
        state.pr.state = Some("OPEN".into());
        state.polling.quiet_cycles = 3;
        state.polling.required_quiet_cycles = 3;
        state.last_cycle.completed_at = Some("2026-05-13T19:00:00Z".into());
        let status = format_status_report(&state, "ready_for_human_merge");
        assert!(status.contains("Next poll: not scheduled"));
        let next = human_next_step(&state);
        assert!(next.contains("gh pr merge 15 --repo owner/repo"));
        assert!(next.contains("[--squash|--merge|--rebase]"));
        let evidence = evidence_lines(&state);
        assert!(
            evidence
                .iter()
                .any(|line| line.contains("Last cycle completed"))
        );
    }
    #[test]
    fn schedule_fields_set_interval_and_next_poll() {
        let mut state = PrWatchState::new(PrTarget {
            repo: "owner/repo".into(),
            number: 12,
        });
        let params = PrWatchInput {
            action: PrWatchAction::PollNow,
            repo: None,
            pr: None,
            watch_id: Some(state.watch_id.clone()),
            dry_run: Some(true),
            schedule_next: true,
            poll_interval_seconds: Some(1),
            quiet_cycles_required: None,
            max_runtime_seconds: None,
            target: None,
        };
        apply_schedule_fields(&mut state, &params);
        assert_eq!(state.polling.poll_interval_seconds, 60);
        assert!(state.polling.next_poll_at.is_some());
    }

    #[test]
    fn scheduled_prompt_is_read_only_and_specific() {
        let state = PrWatchState::new(PrTarget {
            repo: "owner/repo".into(),
            number: 13,
        });
        let prompt = scheduled_poll_prompt(&state);
        assert!(prompt.contains("action=ack_baseline"));
        assert!(prompt.contains("repo=owner/repo"));
        assert!(prompt.contains("pr=13"));
        assert!(prompt.contains("Do not push"));
        assert!(prompt.contains("merge"));

        let mut baselined = state;
        baselined
            .last_successful_fetch
            .insert("metadata".to_string(), "2026-05-13T21:00:00Z".to_string());
        assert!(scheduled_poll_prompt(&baselined).contains("action=poll_now"));
    }
    #[test]
    fn ack_baseline_marks_current_feedback_seen_without_actionable() {
        let mut state = PrWatchState::new(PrTarget {
            repo: "owner/repo".into(),
            number: 11,
        });
        let collection = GhCollection {
            metadata: Ok(jcode_pr_watch_core::PrMetadata {
                identity: jcode_pr_watch_core::PrIdentity {
                    repo: "owner/repo".into(),
                    number: 11,
                    url: Some("https://github.com/owner/repo/pull/11".into()),
                    state: Some("OPEN".into()),
                    base_ref: Some("main".into()),
                    head_ref: Some("feature".into()),
                    head_sha: Some("sha1".into()),
                    merge_state: Some("CLEAN".into()),
                    review_decision: None,
                },
                is_draft: Some(false),
            }),
            checks: Ok(Vec::new()),
            review_comments: Ok(vec![jcode_pr_watch_core::ReviewComment {
                id: "RC_BASE".into(),
                path: Some("src/lib.rs".into()),
                line: Some(1),
                url: Some("https://comment".into()),
                updated_at: Some("2026-05-13T18:00:00Z".into()),
                author: Some("reviewer".into()),
                body: Some("Existing comment".into()),
            }]),
            issue_comments: Ok(Vec::new()),
            reviews: Ok(Vec::new()),
            review_threads: Ok(vec![jcode_pr_watch_core::ReviewThread {
                id: "THREAD_BASE".into(),
                is_resolved: false,
                is_outdated: false,
                path: Some("src/lib.rs".into()),
                line: Some(2),
                url: Some("https://thread".into()),
                updated_at: Some("2026-05-13T18:00:00Z".into()),
                author: Some("reviewer".into()),
                body: Some("Existing thread".into()),
            }]),
        };
        let partial =
            apply_baseline_from_collection(&mut state, collection, "2026-05-13T18:00:00Z");
        assert!(!partial);
        assert!(state.pending_actionable.is_empty());
        assert_eq!(
            state.last_cycle.status,
            jcode_pr_watch_core::CycleStatus::BaselineEstablished
        );
        assert_eq!(state.baseline.head_sha.as_deref(), Some("sha1"));
        assert_eq!(state.baseline.review_comment_count, 1);
        assert_eq!(
            state.baseline.unresolved_thread_ids,
            vec!["THREAD_BASE".to_string()]
        );
        assert!(state.last_seen.review_comments.contains_key("RC_BASE"));
        assert!(state.last_seen.review_threads.contains_key("THREAD_BASE"));

        let collection = GhCollection {
            metadata: Err(SurfaceError::transient("metadata", "skip")),
            checks: Ok(Vec::new()),
            review_comments: Ok(vec![jcode_pr_watch_core::ReviewComment {
                id: "RC_BASE".into(),
                path: Some("src/lib.rs".into()),
                line: Some(1),
                url: Some("https://comment".into()),
                updated_at: Some("2026-05-13T18:00:00Z".into()),
                author: Some("reviewer".into()),
                body: Some("Existing comment".into()),
            }]),
            issue_comments: Ok(Vec::new()),
            reviews: Ok(Vec::new()),
            review_threads: Ok(vec![jcode_pr_watch_core::ReviewThread {
                id: "THREAD_BASE".into(),
                is_resolved: false,
                is_outdated: false,
                path: Some("src/lib.rs".into()),
                line: Some(2),
                url: Some("https://thread".into()),
                updated_at: Some("2026-05-13T18:00:00Z".into()),
                author: Some("reviewer".into()),
                body: Some("Existing thread".into()),
            }]),
        };
        update_state_from_collection(&mut state, collection, "2026-05-13T18:05:00Z");
        assert_eq!(state.pending_actionable.len(), 1);
        assert_eq!(state.pending_actionable[0].id, "THREAD_BASE");
    }
    #[test]
    fn unresolved_review_threads_are_actionable_and_resolved_are_not() {
        let mut state = PrWatchState::new(PrTarget {
            repo: "owner/repo".into(),
            number: 10,
        });
        let collection = GhCollection {
            metadata: Err(SurfaceError::transient("metadata", "skip")),
            checks: Ok(Vec::new()),
            review_comments: Ok(Vec::new()),
            issue_comments: Ok(Vec::new()),
            reviews: Ok(Vec::new()),
            review_threads: Ok(vec![
                jcode_pr_watch_core::ReviewThread {
                    id: "THREAD_OPEN".into(),
                    is_resolved: false,
                    is_outdated: false,
                    path: Some("src/lib.rs".into()),
                    line: Some(12),
                    url: Some("https://thread-open".into()),
                    updated_at: Some("2026-05-13T17:00:00Z".into()),
                    author: Some("reviewer".into()),
                    body: Some("Please address this thread".into()),
                },
                jcode_pr_watch_core::ReviewThread {
                    id: "THREAD_RESOLVED".into(),
                    is_resolved: true,
                    is_outdated: false,
                    path: Some("src/lib.rs".into()),
                    line: Some(20),
                    url: Some("https://thread-resolved".into()),
                    updated_at: Some("2026-05-13T17:00:00Z".into()),
                    author: Some("reviewer".into()),
                    body: Some("Already resolved".into()),
                },
            ]),
        };
        let outcome = update_state_from_collection(&mut state, collection, "2026-05-13T17:00:00Z");
        assert!(outcome.partial_failure);
        assert_eq!(state.pending_actionable.len(), 1);
        assert_eq!(state.pending_actionable[0].id, "THREAD_OPEN");
        assert_eq!(
            state.baseline.unresolved_thread_ids,
            vec!["THREAD_OPEN".to_string()]
        );
        assert!(state.last_seen.review_threads.contains_key("THREAD_OPEN"));
        assert!(
            state
                .last_seen
                .review_threads
                .contains_key("THREAD_RESOLVED")
        );
    }
    #[test]
    fn automation_chatter_is_not_actionable() {
        assert!(!is_automation_chatter(
            Some("github-actions[bot]"),
            Some("Progress update")
        ));
        assert!(is_automation_chatter(
            Some("human"),
            Some("fix-summary: addressed feedback")
        ));
        assert!(!is_automation_chatter(
            Some("reviewer"),
            Some("Please fix this")
        ));
    }
}
