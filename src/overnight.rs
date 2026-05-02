use crate::agent::Agent;
use crate::provider::Provider;
use crate::session::{Session, SessionStatus};
use crate::storage;
use crate::tool::Registry;
use anyhow::{Context, Result};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::ffi::CString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const OVERNIGHT_VERSION: u32 = 1;
const RESOURCE_SAMPLE_INTERVAL: Duration = Duration::from_secs(5 * 60);
const LONG_TURN_NOTICE_INTERVAL: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OvernightDuration {
    pub minutes: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OvernightCommand {
    Start {
        duration: OvernightDuration,
        mission: Option<String>,
    },
    Status,
    Log,
    Review,
    Cancel,
    Help,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OvernightRunStatus {
    Running,
    CancelRequested,
    Completed,
    Failed,
}

impl OvernightRunStatus {
    fn label(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::CancelRequested => "cancel requested",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OvernightManifest {
    pub version: u32,
    pub run_id: String,
    pub parent_session_id: String,
    pub coordinator_session_id: String,
    pub coordinator_session_name: String,
    pub started_at: DateTime<Utc>,
    pub target_wake_at: DateTime<Utc>,
    pub handoff_ready_at: DateTime<Utc>,
    pub post_wake_grace_until: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub morning_report_posted_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel_requested_at: Option<DateTime<Utc>>,
    pub status: OvernightRunStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mission: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    pub provider_name: String,
    pub model: String,
    pub max_agents_guidance: u8,
    pub process_id: u32,
    pub run_dir: PathBuf,
    pub events_path: PathBuf,
    pub human_log_path: PathBuf,
    pub review_path: PathBuf,
    pub review_notes_path: PathBuf,
    pub preflight_path: PathBuf,
    pub task_cards_dir: PathBuf,
    pub issue_drafts_dir: PathBuf,
    pub validation_dir: PathBuf,
    pub last_activity_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OvernightEvent {
    pub timestamp: DateTime<Utc>,
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub kind: String,
    pub summary: String,
    #[serde(default)]
    pub details: Value,
    #[serde(default)]
    pub meaningful: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResourceSnapshot {
    pub captured_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_total_mb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_available_mb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_used_percent: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swap_total_mb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swap_free_mb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_one: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery_percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_available_gb: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageProviderSnapshot {
    pub provider_name: String,
    pub hard_limit_reached: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub limits: Vec<UsageLimitSnapshot>,
    pub extra_info: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageLimitSnapshot {
    pub name: String,
    pub usage_percent: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageProjection {
    pub captured_at: DateTime<Utc>,
    pub risk: String,
    pub confidence: String,
    pub projected_delta_min_percent: Option<f32>,
    pub projected_delta_max_percent: Option<f32>,
    pub projected_end_min_percent: Option<f32>,
    pub projected_end_max_percent: Option<f32>,
    pub providers: Vec<UsageProviderSnapshot>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitSnapshot {
    pub captured_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dirty_count: Option<usize>,
    #[serde(default)]
    pub dirty_summary: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OvernightPreflight {
    pub captured_at: DateTime<Utc>,
    pub usage: UsageProjection,
    pub resources: ResourceSnapshot,
    pub git: GitSnapshot,
}

#[derive(Debug, Clone)]
pub struct OvernightLaunch {
    pub manifest: OvernightManifest,
}

pub struct OvernightStartOptions {
    pub duration: OvernightDuration,
    pub mission: Option<String>,
    pub parent_session: Session,
    pub provider: Arc<dyn Provider>,
    pub registry: Registry,
    pub working_dir: Option<PathBuf>,
}

pub fn parse_overnight_command(trimmed: &str) -> Option<Result<OvernightCommand, String>> {
    let rest = trimmed.strip_prefix("/overnight")?.trim();
    if rest.is_empty() || rest == "help" || rest == "--help" || rest == "-h" {
        return Some(Ok(OvernightCommand::Help));
    }

    match rest {
        "status" => return Some(Ok(OvernightCommand::Status)),
        "log" => return Some(Ok(OvernightCommand::Log)),
        "review" | "open" => return Some(Ok(OvernightCommand::Review)),
        "cancel" | "stop" => return Some(Ok(OvernightCommand::Cancel)),
        _ => {}
    }

    if rest.starts_with("status ")
        || rest.starts_with("log ")
        || rest.starts_with("review ")
        || rest.starts_with("cancel ")
    {
        return Some(Err(overnight_usage().to_string()));
    }

    let mut parts = rest.splitn(2, char::is_whitespace);
    let duration_raw = parts.next().unwrap_or_default();
    let mission = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let duration = match parse_duration(duration_raw) {
        Ok(duration) => duration,
        Err(error) => return Some(Err(error)),
    };

    Some(Ok(OvernightCommand::Start { duration, mission }))
}

pub fn overnight_usage() -> &'static str {
    "Usage: `/overnight <hours>[h|m] [mission]`, `/overnight status`, `/overnight log`, `/overnight review`, or `/overnight cancel`"
}

pub fn parse_duration(input: &str) -> std::result::Result<OvernightDuration, String> {
    let raw = input.trim();
    if raw.is_empty() {
        return Err(overnight_usage().to_string());
    }

    let (number, multiplier) = if let Some(hours) = raw.strip_suffix('h') {
        (hours, 60.0)
    } else if let Some(minutes) = raw.strip_suffix('m') {
        (minutes, 1.0)
    } else {
        (raw, 60.0)
    };

    let value: f64 = number.parse().map_err(|_| {
        format!(
            "Invalid overnight duration `{}`. {}",
            raw,
            overnight_usage()
        )
    })?;
    if !value.is_finite() || value <= 0.0 {
        return Err(format!(
            "Invalid overnight duration `{}`. Duration must be greater than zero.",
            raw
        ));
    }
    let minutes = (value * multiplier).round() as u32;
    if minutes == 0 || minutes > 72 * 60 {
        return Err("Overnight duration must be between 1 minute and 72 hours.".to_string());
    }
    Ok(OvernightDuration { minutes })
}

pub fn start_overnight_run(options: OvernightStartOptions) -> Result<OvernightLaunch> {
    let run_id = crate::id::new_id("overnight");
    let started_at = Utc::now();
    let duration = ChronoDuration::minutes(options.duration.minutes as i64);
    let target_wake_at = started_at + duration;
    let handoff_ready_at = target_wake_at - ChronoDuration::minutes(30).min(duration / 4);
    let post_wake_grace_until = target_wake_at + ChronoDuration::hours(2);
    let run_dir = run_dir(&run_id)?;
    let events_path = run_dir.join("events.jsonl");
    let human_log_path = run_dir.join("run.log");
    let review_path = run_dir.join("review.html");
    let review_notes_path = run_dir.join("review-notes.md");
    let preflight_path = run_dir.join("preflight.json");
    let task_cards_dir = run_dir.join("task-cards");
    let issue_drafts_dir = run_dir.join("issue-drafts");
    let validation_dir = run_dir.join("validation");
    std::fs::create_dir_all(&task_cards_dir)?;
    std::fs::create_dir_all(&issue_drafts_dir)?;
    std::fs::create_dir_all(&validation_dir)?;

    let mut child = create_coordinator_session(&options.parent_session, &options.mission)?;
    if let Some(working_dir) = options.working_dir.as_ref() {
        child.working_dir = Some(working_dir.to_string_lossy().to_string());
    }
    child.model = Some(options.provider.model());
    let coordinator_session_id = child.id.clone();
    let coordinator_session_name = child.display_name().to_string();
    let child_is_canary = child.is_canary;
    child.status = SessionStatus::Closed;
    child.save()?;

    if let Ok(todos) = crate::todo::load_todos(&options.parent_session.id) {
        let _ = crate::todo::save_todos(&coordinator_session_id, &todos);
    }

    let manifest = OvernightManifest {
        version: OVERNIGHT_VERSION,
        run_id: run_id.clone(),
        parent_session_id: options.parent_session.id.clone(),
        coordinator_session_id: coordinator_session_id.clone(),
        coordinator_session_name,
        started_at,
        target_wake_at,
        handoff_ready_at,
        post_wake_grace_until,
        morning_report_posted_at: None,
        completed_at: None,
        cancel_requested_at: None,
        status: OvernightRunStatus::Running,
        mission: options.mission.clone(),
        working_dir: child.working_dir.clone(),
        provider_name: options.provider.name().to_string(),
        model: options.provider.model(),
        max_agents_guidance: 2,
        process_id: std::process::id(),
        run_dir,
        events_path,
        human_log_path,
        review_path,
        review_notes_path,
        preflight_path,
        task_cards_dir,
        issue_drafts_dir,
        validation_dir,
        last_activity_at: started_at,
    };

    save_manifest(&manifest)?;
    write_initial_review_notes(&manifest)?;
    record_event(
        &manifest,
        "run_started",
        format!(
            "Started overnight run for {} (target wake: {})",
            format_minutes(options.duration.minutes),
            manifest.target_wake_at.to_rfc3339()
        ),
        json!({
            "mission": manifest.mission,
            "parent_session_id": manifest.parent_session_id,
            "coordinator_session_id": manifest.coordinator_session_id,
            "review_path": manifest.review_path,
        }),
        true,
    )?;
    render_review_html(&manifest)?;

    spawn_supervisor(
        manifest.clone(),
        child,
        options.provider,
        options.registry,
        child_is_canary,
    );

    Ok(OvernightLaunch { manifest })
}

fn create_coordinator_session(parent: &Session, mission: &Option<String>) -> Result<Session> {
    let title = Some(match mission {
        Some(mission) => format!("Overnight: {}", crate::util::truncate_str(mission, 48)),
        None => "Overnight coordinator".to_string(),
    });
    let mut child = Session::create(Some(parent.id.clone()), title);
    child.replace_messages(parent.messages.clone());
    child.compaction = parent.compaction.clone();
    child.provider_key = parent.provider_key.clone();
    child.subagent_model = parent.subagent_model.clone();
    child.improve_mode = parent.improve_mode;
    child.autoreview_enabled = Some(false);
    child.autojudge_enabled = Some(false);
    child.is_canary = parent.is_canary;
    child.testing_build = parent.testing_build.clone();
    child.working_dir = parent.working_dir.clone();
    child.provider_session_id = None;
    Ok(child)
}

fn spawn_supervisor(
    manifest: OvernightManifest,
    child: Session,
    provider: Arc<dyn Provider>,
    registry: Registry,
    child_is_canary: bool,
) {
    let fut = async move {
        if let Err(err) =
            run_supervisor(manifest.clone(), child, provider, registry, child_is_canary).await
        {
            let mut updated = load_manifest(&manifest.run_id).unwrap_or(manifest.clone());
            updated.status = OvernightRunStatus::Failed;
            updated.completed_at = Some(Utc::now());
            let _ = save_manifest(&updated);
            let _ = record_event(
                &updated,
                "run_failed",
                format!("Overnight supervisor failed: {}", err),
                json!({ "error": crate::util::format_error_chain(&err) }),
                true,
            );
            let _ = render_review_html(&updated);
        }
    };

    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::spawn(fut);
    } else {
        std::thread::spawn(move || match tokio::runtime::Runtime::new() {
            Ok(runtime) => runtime.block_on(fut),
            Err(err) => crate::logging::error(&format!(
                "Failed to start overnight supervisor runtime: {}",
                err
            )),
        });
    }
}

async fn run_supervisor(
    manifest: OvernightManifest,
    child: Session,
    provider: Arc<dyn Provider>,
    registry: Registry,
    child_is_canary: bool,
) -> Result<()> {
    record_event(
        &manifest,
        "preflight_started",
        "Collecting overnight usage/resource/git preflight".to_string(),
        json!({}),
        true,
    )?;
    let preflight = gather_preflight(&manifest).await;
    storage::write_json(&manifest.preflight_path, &preflight)?;
    record_event(
        &manifest,
        "preflight_completed",
        preflight_summary(&preflight),
        serde_json::to_value(&preflight).unwrap_or_else(|_| json!({})),
        true,
    )?;
    render_review_html(&manifest)?;

    if child_is_canary {
        registry.register_selfdev_tools().await;
    }

    let mut agent = Agent::new_with_session(provider, registry, child, None);
    let mut next_prompt = build_coordinator_prompt(&manifest, &preflight);
    let mut handoff_notice_sent = false;
    let mut morning_report_prompt_sent = false;
    let mut final_wrapup_prompt_sent = false;

    loop {
        let current = load_manifest(&manifest.run_id)?;
        if matches!(current.status, OvernightRunStatus::CancelRequested) {
            record_event(
                &current,
                "run_cancel_acknowledged",
                "Cancellation requested; stopping before next coordinator turn".to_string(),
                json!({}),
                true,
            )?;
            mark_completed(
                &current,
                OvernightRunStatus::Completed,
                "Cancelled before next turn",
            )?;
            break;
        }

        let now = Utc::now();
        if !handoff_notice_sent && now >= current.handoff_ready_at && now < current.target_wake_at {
            record_event(
                &current,
                "handoff_ready_notice",
                "Entering handoff-ready mode".to_string(),
                json!({ "target_wake_at": current.target_wake_at }),
                true,
            )?;
            next_prompt = build_handoff_ready_prompt(&current);
            handoff_notice_sent = true;
        }

        record_event(
            &current,
            "coordinator_turn_started",
            prompt_event_summary(&next_prompt),
            json!({ "prompt_preview": crate::util::truncate_str(&next_prompt, 600) }),
            true,
        )?;
        render_review_html(&current)?;

        let output = run_turn_monitored(&mut agent, &current, &next_prompt).await?;
        let after_turn = load_manifest(&manifest.run_id)?;
        record_event(
            &after_turn,
            "coordinator_turn_completed",
            "Coordinator turn completed".to_string(),
            json!({ "output_preview": crate::util::truncate_str(&output, 4000) }),
            true,
        )?;
        render_review_html(&after_turn)?;

        let after_turn = load_manifest(&manifest.run_id)?;
        if matches!(after_turn.status, OvernightRunStatus::CancelRequested) {
            mark_completed(
                &after_turn,
                OvernightRunStatus::Completed,
                "Cancelled after coordinator turn",
            )?;
            break;
        }

        let now = Utc::now();
        if now >= after_turn.target_wake_at {
            if !morning_report_prompt_sent && after_turn.morning_report_posted_at.is_none() {
                let mut updated = after_turn.clone();
                updated.morning_report_posted_at = Some(now);
                save_manifest(&updated)?;
                record_event(
                    &updated,
                    "morning_report_requested",
                    "Target wake time reached; requesting morning report".to_string(),
                    json!({ "target_wake_at": updated.target_wake_at }),
                    true,
                )?;
                next_prompt = build_morning_report_prompt(&updated);
                morning_report_prompt_sent = true;
                continue;
            }

            if now < after_turn.post_wake_grace_until {
                record_event(
                    &after_turn,
                    "post_wake_continuation",
                    "Morning report is posted; allowing bounded post-wake continuation".to_string(),
                    json!({ "post_wake_grace_until": after_turn.post_wake_grace_until }),
                    true,
                )?;
                next_prompt = build_post_wake_continuation_prompt(&after_turn);
                continue;
            }

            if !final_wrapup_prompt_sent {
                record_event(
                    &after_turn,
                    "post_wake_grace_expired",
                    "Post-wake grace window expired; requesting final wrap-up".to_string(),
                    json!({ "post_wake_grace_until": after_turn.post_wake_grace_until }),
                    true,
                )?;
                next_prompt = build_final_wrapup_prompt(&after_turn);
                final_wrapup_prompt_sent = true;
                continue;
            }

            mark_completed(
                &after_turn,
                OvernightRunStatus::Completed,
                "Morning report turn completed",
            )?;
            break;
        }

        next_prompt = build_continuation_prompt(&after_turn);
    }

    Ok(())
}

async fn run_turn_monitored(
    agent: &mut Agent,
    manifest: &OvernightManifest,
    prompt: &str,
) -> Result<String> {
    let started = Utc::now();
    let mut sample_interval = tokio::time::interval_at(
        tokio::time::Instant::now() + RESOURCE_SAMPLE_INTERVAL,
        RESOURCE_SAMPLE_INTERVAL,
    );
    sample_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut long_notice_interval = tokio::time::interval_at(
        tokio::time::Instant::now() + LONG_TURN_NOTICE_INTERVAL,
        LONG_TURN_NOTICE_INTERVAL,
    );
    long_notice_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let run_future = agent.run_once_capture(prompt);
    tokio::pin!(run_future);

    loop {
        tokio::select! {
            result = &mut run_future => return result,
            _ = sample_interval.tick() => {
                let snapshot = gather_resource_snapshot(manifest.working_dir.as_deref().map(Path::new));
                let _ = record_event(
                    manifest,
                    "resource_sample",
                    resource_summary(&snapshot),
                    serde_json::to_value(&snapshot).unwrap_or_else(|_| json!({})),
                    false,
                );
                let _ = render_review_html(manifest);
            }
            _ = long_notice_interval.tick() => {
                let elapsed = Utc::now().signed_duration_since(started).num_minutes().max(0);
                let _ = record_event(
                    manifest,
                    "coordinator_turn_still_running",
                    format!("Coordinator turn still running after {}m", elapsed),
                    json!({ "elapsed_minutes": elapsed }),
                    true,
                );
                let _ = render_review_html(manifest);
            }
        }
    }
}

async fn gather_preflight(manifest: &OvernightManifest) -> OvernightPreflight {
    let usage_reports = crate::usage::fetch_all_provider_usage().await;
    let usage = build_usage_projection(&usage_reports, manifest);
    let resources = gather_resource_snapshot(manifest.working_dir.as_deref().map(Path::new));
    let git = gather_git_snapshot(manifest.working_dir.as_deref().map(Path::new));
    OvernightPreflight {
        captured_at: Utc::now(),
        usage,
        resources,
        git,
    }
}

fn build_usage_projection(
    reports: &[crate::usage::ProviderUsage],
    manifest: &OvernightManifest,
) -> UsageProjection {
    let providers: Vec<UsageProviderSnapshot> = reports
        .iter()
        .map(|provider| UsageProviderSnapshot {
            provider_name: provider.provider_name.clone(),
            hard_limit_reached: provider.hard_limit_reached,
            error: provider.error.clone(),
            limits: provider
                .limits
                .iter()
                .map(|limit| UsageLimitSnapshot {
                    name: limit.name.clone(),
                    usage_percent: limit.usage_percent,
                    resets_at: limit.resets_at.clone(),
                })
                .collect(),
            extra_info: provider.extra_info.clone(),
        })
        .collect();

    let max_usage = providers
        .iter()
        .flat_map(|provider| provider.limits.iter().map(|limit| limit.usage_percent))
        .fold(None::<f32>, |acc, value| {
            Some(acc.unwrap_or(value).max(value))
        });
    let hard_limit = providers.iter().any(|provider| provider.hard_limit_reached);
    let has_errors = providers.iter().any(|provider| provider.error.is_some());
    let hours = manifest
        .target_wake_at
        .signed_duration_since(manifest.started_at)
        .num_minutes()
        .max(1) as f32
        / 60.0;
    let delta_min = (hours * 3.0).min(35.0);
    let delta_max = (hours * 7.0 * manifest.max_agents_guidance as f32 / 2.0).min(75.0);
    let projected_end_min = max_usage.map(|current| (current + delta_min).min(100.0));
    let projected_end_max = max_usage.map(|current| (current + delta_max).min(100.0));

    let risk = if hard_limit || projected_end_max.is_some_and(|value| value >= 95.0) {
        "high"
    } else if projected_end_max.is_some_and(|value| value >= 80.0) || has_errors {
        "medium"
    } else if max_usage.is_some() {
        "low"
    } else {
        "unknown"
    }
    .to_string();

    let confidence = if max_usage.is_some() && !has_errors {
        "medium"
    } else if !providers.is_empty() {
        "low"
    } else {
        "low"
    }
    .to_string();

    let mut notes = Vec::new();
    if providers.is_empty() {
        notes.push(
            "No connected-provider usage reports were available; projection is heuristic."
                .to_string(),
        );
    } else {
        notes.push("Projection uses provider usage percentages plus a conservative overnight burn-rate heuristic.".to_string());
    }
    notes.push("This is a warning only; the run starts regardless and should adapt concurrency conservatively.".to_string());

    UsageProjection {
        captured_at: Utc::now(),
        risk,
        confidence,
        projected_delta_min_percent: max_usage.map(|_| delta_min),
        projected_delta_max_percent: max_usage.map(|_| delta_max),
        projected_end_min_percent: projected_end_min,
        projected_end_max_percent: projected_end_max,
        providers,
        notes,
    }
}

pub fn gather_resource_snapshot(working_dir: Option<&Path>) -> ResourceSnapshot {
    let (memory_total_mb, memory_available_mb, swap_total_mb, swap_free_mb) = detect_memory();
    let memory_used_percent =
        memory_total_mb
            .zip(memory_available_mb)
            .and_then(|(total, available)| {
                if total == 0 {
                    None
                } else {
                    Some(((total.saturating_sub(available)) as f32 / total as f32) * 100.0)
                }
            });
    let (load_one, cpu_count) = detect_load();
    let (battery_percent, battery_status) = detect_battery();
    let disk_available_gb = working_dir.and_then(disk_available_gb);

    ResourceSnapshot {
        captured_at: Utc::now(),
        memory_total_mb,
        memory_available_mb,
        memory_used_percent,
        swap_total_mb,
        swap_free_mb,
        load_one,
        cpu_count,
        battery_percent,
        battery_status,
        disk_available_gb,
    }
}

fn detect_memory() -> (Option<u64>, Option<u64>, Option<u64>, Option<u64>) {
    #[cfg(target_os = "linux")]
    {
        let Ok(contents) = std::fs::read_to_string("/proc/meminfo") else {
            return (None, None, None, None);
        };
        let mut total_kb = None;
        let mut available_kb = None;
        let mut swap_total_kb = None;
        let mut swap_free_kb = None;
        for line in contents.lines() {
            if let Some(rest) = line.strip_prefix("MemTotal:") {
                total_kb = parse_meminfo_kb(rest);
            } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
                available_kb = parse_meminfo_kb(rest);
            } else if let Some(rest) = line.strip_prefix("SwapTotal:") {
                swap_total_kb = parse_meminfo_kb(rest);
            } else if let Some(rest) = line.strip_prefix("SwapFree:") {
                swap_free_kb = parse_meminfo_kb(rest);
            }
        }
        (
            total_kb.map(|kb| kb / 1024),
            available_kb.map(|kb| kb / 1024),
            swap_total_kb.map(|kb| kb / 1024),
            swap_free_kb.map(|kb| kb / 1024),
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        (None, None, None, None)
    }
}

#[cfg(target_os = "linux")]
fn parse_meminfo_kb(rest: &str) -> Option<u64> {
    rest.split_whitespace().next()?.parse().ok()
}

fn detect_load() -> (Option<f64>, Option<usize>) {
    #[cfg(target_os = "linux")]
    {
        let load = std::fs::read_to_string("/proc/loadavg")
            .ok()
            .and_then(|contents| contents.split_whitespace().next()?.parse::<f64>().ok());
        let cpus = std::thread::available_parallelism()
            .ok()
            .map(|value| value.get());
        (load, cpus)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let cpus = std::thread::available_parallelism()
            .ok()
            .map(|value| value.get());
        (None, cpus)
    }
}

fn detect_battery() -> (Option<u8>, Option<String>) {
    #[cfg(target_os = "linux")]
    {
        let Ok(entries) = std::fs::read_dir("/sys/class/power_supply") else {
            return (None, None);
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.starts_with("BAT") {
                continue;
            }
            let percent = std::fs::read_to_string(path.join("capacity"))
                .ok()
                .and_then(|value| value.trim().parse::<u8>().ok());
            let status = std::fs::read_to_string(path.join("status"))
                .ok()
                .map(|value| value.trim().to_string());
            return (percent, status);
        }
        (None, None)
    }
    #[cfg(not(target_os = "linux"))]
    {
        (None, None)
    }
}

fn disk_available_gb(path: &Path) -> Option<f64> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
        if rc != 0 {
            return None;
        }
        let bytes = stat.f_bavail as f64 * stat.f_frsize as f64;
        Some(bytes / 1024.0 / 1024.0 / 1024.0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

pub fn gather_git_snapshot(working_dir: Option<&Path>) -> GitSnapshot {
    let captured_at = Utc::now();
    let dir = working_dir.unwrap_or_else(|| Path::new("."));
    let branch = run_git(dir, &["branch", "--show-current"])
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    match run_git(dir, &["status", "--short"]) {
        Ok(status) => {
            let dirty_summary: Vec<String> = status
                .lines()
                .filter(|line| !line.trim().is_empty())
                .take(20)
                .map(str::to_string)
                .collect();
            let dirty_count = status
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count();
            GitSnapshot {
                captured_at,
                branch,
                dirty_count: Some(dirty_count),
                dirty_summary,
                error: None,
            }
        }
        Err(error) => GitSnapshot {
            captured_at,
            branch,
            dirty_count: None,
            dirty_summary: Vec::new(),
            error: Some(error),
        },
    }
}

fn run_git(dir: &Path, args: &[&str]) -> std::result::Result<String, String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|err| format!("failed to run git {}: {}", args.join(" "), err))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

pub fn overnight_root_dir() -> Result<PathBuf> {
    Ok(storage::jcode_dir()?.join("overnight"))
}

pub fn runs_dir() -> Result<PathBuf> {
    Ok(overnight_root_dir()?.join("runs"))
}

pub fn run_dir(run_id: &str) -> Result<PathBuf> {
    Ok(runs_dir()?.join(run_id))
}

pub fn manifest_path(run_id: &str) -> Result<PathBuf> {
    Ok(run_dir(run_id)?.join("manifest.json"))
}

pub fn save_manifest(manifest: &OvernightManifest) -> Result<()> {
    storage::write_json(&manifest_path(&manifest.run_id)?, manifest)
}

pub fn load_manifest(run_id: &str) -> Result<OvernightManifest> {
    storage::read_json(&manifest_path(run_id)?)
}

pub fn latest_manifest() -> Result<Option<OvernightManifest>> {
    let dir = runs_dir()?;
    if !dir.exists() {
        return Ok(None);
    }
    let mut manifests = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path().join("manifest.json");
        if path.exists()
            && let Ok(manifest) = storage::read_json::<OvernightManifest>(&path)
        {
            manifests.push(manifest);
        }
    }
    manifests.sort_by_key(|manifest| manifest.started_at);
    Ok(manifests.pop())
}

pub fn cancel_latest_run() -> Result<OvernightManifest> {
    let mut manifest = latest_manifest()?.context("No overnight runs found")?;
    if matches!(
        manifest.status,
        OvernightRunStatus::Completed | OvernightRunStatus::Failed
    ) {
        return Ok(manifest);
    }
    manifest.status = OvernightRunStatus::CancelRequested;
    manifest.cancel_requested_at = Some(Utc::now());
    save_manifest(&manifest)?;
    record_event(
        &manifest,
        "cancel_requested",
        "User requested overnight cancellation".to_string(),
        json!({}),
        true,
    )?;
    render_review_html(&manifest)?;
    Ok(manifest)
}

pub fn read_events(manifest: &OvernightManifest) -> Result<Vec<OvernightEvent>> {
    if !manifest.events_path.exists() {
        return Ok(Vec::new());
    }
    let contents = std::fs::read_to_string(&manifest.events_path)?;
    Ok(contents
        .lines()
        .filter_map(|line| serde_json::from_str::<OvernightEvent>(line).ok())
        .collect())
}

pub fn record_event(
    manifest: &OvernightManifest,
    kind: &str,
    summary: String,
    details: Value,
    meaningful: bool,
) -> Result<()> {
    let event = OvernightEvent {
        timestamp: Utc::now(),
        run_id: manifest.run_id.clone(),
        session_id: Some(manifest.coordinator_session_id.clone()),
        kind: kind.to_string(),
        summary: summary.clone(),
        details,
        meaningful,
    };

    if let Some(parent) = manifest.events_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut events = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&manifest.events_path)?;
    writeln!(events, "{}", serde_json::to_string(&event)?)?;

    if let Some(parent) = manifest.human_log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut human = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&manifest.human_log_path)?;
    writeln!(
        human,
        "{} [{}] {}",
        event.timestamp.format("%H:%M:%S"),
        event.kind,
        summary
    )?;

    if meaningful {
        let mut updated = load_manifest(&manifest.run_id).unwrap_or_else(|_| manifest.clone());
        updated.last_activity_at = event.timestamp;
        let _ = save_manifest(&updated);
    }

    Ok(())
}

fn mark_completed(
    manifest: &OvernightManifest,
    status: OvernightRunStatus,
    summary: &str,
) -> Result<()> {
    let mut updated = load_manifest(&manifest.run_id).unwrap_or_else(|_| manifest.clone());
    updated.status = status;
    updated.completed_at = Some(Utc::now());
    updated.last_activity_at = Utc::now();
    save_manifest(&updated)?;
    record_event(
        &updated,
        "run_completed",
        summary.to_string(),
        json!({ "status": updated.status.label() }),
        true,
    )?;
    render_review_html(&updated)?;
    Ok(())
}

pub fn format_status_markdown(manifest: &OvernightManifest) -> String {
    let remaining = manifest
        .target_wake_at
        .signed_duration_since(Utc::now())
        .num_minutes();
    let remaining_line = if remaining >= 0 {
        format!("Target wake time in {}.", format_minutes(remaining as u32))
    } else {
        format!(
            "Target wake time passed {} ago.",
            format_minutes((-remaining) as u32)
        )
    };
    format!(
        "🌙 **Overnight run `{}`**\n\nStatus: **{}**\nCoordinator: `{}` ({})\n{}\nPost-wake soft grace until: `{}`\nLast meaningful activity: {}\nReview: `{}`\nLog: `{}`",
        manifest.run_id,
        manifest.status.label(),
        manifest.coordinator_session_id,
        manifest.coordinator_session_name,
        remaining_line,
        manifest.post_wake_grace_until.to_rfc3339(),
        manifest.last_activity_at.to_rfc3339(),
        manifest.review_path.display(),
        manifest.human_log_path.display()
    )
}

pub fn format_log_markdown(manifest: &OvernightManifest, max_lines: usize) -> String {
    let events = read_events(manifest).unwrap_or_default();
    let start = events.len().saturating_sub(max_lines);
    let mut out = format!("🌙 **Overnight log `{}`**\n\n", manifest.run_id);
    for event in &events[start..] {
        out.push_str(&format!(
            "- `{}` **{}**: {}\n",
            event.timestamp.format("%H:%M:%S"),
            event.kind,
            event.summary
        ));
    }
    if events.is_empty() {
        out.push_str("No events recorded yet.\n");
    }
    out.push_str(&format!(
        "\nFull log: `{}`",
        manifest.human_log_path.display()
    ));
    out
}

fn write_initial_review_notes(manifest: &OvernightManifest) -> Result<()> {
    if manifest.review_notes_path.exists() {
        return Ok(());
    }
    let content = format!(
        "# Overnight review notes\n\nRun: `{}`\nCoordinator session: `{}`\nTarget wake time: `{}`\n\nThe coordinator must keep this file useful as the run progresses. Required sections for each meaningful task:\n\n## Executive summary\n\n- Status: running\n- Current task: not started\n- Verified fixes: 0\n- Issue drafts/posts: 0\n- Repo risk: unknown\n\n## Task reviews\n\nFor each task, include:\n\n### Task: <title>\n\n- Source: user request / GH issue / static analysis / failing test / code quality\n- Why chosen:\n- Verifiability:\n- Risk:\n- Outcome:\n\n#### Before\n\n- Observed behavior or code state:\n- Reproduction/evidence:\n\n#### After\n\n- Changed behavior or code state:\n- Validation run:\n- Files changed:\n\n## Decisions and skipped work\n\nRecord tasks considered but skipped, with reasons.\n\n## Open questions and next steps\n\nRecord user decisions needed and safe continuation options.\n",
        manifest.run_id,
        manifest.coordinator_session_id,
        manifest.target_wake_at.to_rfc3339(),
    );
    write_text_file(&manifest.review_notes_path, &content)
}

pub fn render_review_html(manifest: &OvernightManifest) -> Result<()> {
    let events = read_events(manifest).unwrap_or_default();
    let notes = std::fs::read_to_string(&manifest.review_notes_path).unwrap_or_else(|_| {
        "# Overnight review notes\n\nCoordinator has not written notes yet.".to_string()
    });
    let preflight = if manifest.preflight_path.exists() {
        std::fs::read_to_string(&manifest.preflight_path).unwrap_or_default()
    } else {
        String::new()
    };

    let mut timeline = String::new();
    for event in events
        .iter()
        .rev()
        .take(200)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        let class = event_class(&event.kind);
        timeline.push_str(&format!(
            "<li class=\"{}\"><time>{}</time><strong>{}</strong><span>{}</span></li>\n",
            class,
            html_escape(&event.timestamp.format("%H:%M:%S").to_string()),
            html_escape(&event.kind),
            html_escape(&event.summary)
        ));
    }

    let status = manifest.status.label();
    let html = format!(
        r#"<!doctype html>
<html>
<head>
<meta charset="utf-8">
<title>Overnight run {run_id}</title>
<style>
body {{ font-family: system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; margin: 0; background: #0f1117; color: #e8eaf0; }}
a {{ color: #8ab4ff; }}
header {{ padding: 28px 36px; background: linear-gradient(135deg, #1d2340, #12141c); border-bottom: 1px solid #30364a; }}
main {{ padding: 24px 36px 48px; max-width: 1200px; margin: 0 auto; }}
.cards {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: 14px; margin-top: 18px; }}
.card {{ background: #171b26; border: 1px solid #2c3347; border-radius: 14px; padding: 16px; }}
.card .label {{ color: #9aa4bc; font-size: 12px; text-transform: uppercase; letter-spacing: .08em; }}
.card .value {{ font-size: 18px; margin-top: 6px; }}
section {{ margin-top: 28px; background: #151923; border: 1px solid #2a3041; border-radius: 16px; padding: 20px; }}
h1, h2 {{ margin: 0 0 12px; }}
ul.timeline {{ list-style: none; padding: 0; margin: 0; }}
.timeline li {{ display: grid; grid-template-columns: 86px 240px 1fr; gap: 12px; padding: 9px 0; border-bottom: 1px solid #252b3a; }}
.timeline li:last-child {{ border-bottom: none; }}
.timeline time {{ color: #9aa4bc; }}
.timeline strong {{ color: #d9def0; }}
.timeline .ok strong {{ color: #8ee99a; }}
.timeline .warn strong {{ color: #ffd166; }}
.timeline .bad strong {{ color: #ff7b7b; }}
pre {{ white-space: pre-wrap; word-break: break-word; background: #0b0d12; color: #e8eaf0; padding: 14px; border-radius: 12px; border: 1px solid #272d3c; overflow-x: auto; }}
.badge {{ display: inline-block; padding: 4px 9px; border-radius: 999px; background: #24314f; color: #cfe0ff; font-size: 12px; }}
.path {{ font-family: ui-monospace, SFMono-Regular, Menlo, monospace; color: #b7c4e8; }}
</style>
</head>
<body>
<header>
  <h1>🌙 Overnight run <code>{run_id}</code></h1>
  <div class="badge">{status}</div>
  <div class="cards">
    <div class="card"><div class="label">Coordinator</div><div class="value"><code>{coordinator}</code><br>{coordinator_name}</div></div>
    <div class="card"><div class="label">Started</div><div class="value">{started}</div></div>
    <div class="card"><div class="label">Target wake</div><div class="value">{target}</div></div>
    <div class="card"><div class="label">Last activity</div><div class="value">{last_activity}</div></div>
  </div>
</header>
<main>
<section>
  <h2>Executive summary</h2>
  <p>Mission: {mission}</p>
  <p>Working directory: <span class="path">{working_dir}</span></p>
  <p>Provider/model: <code>{provider}</code> / <code>{model}</code></p>
</section>
<section>
  <h2>Coordinator review notes</h2>
  <pre>{notes}</pre>
</section>
<section>
  <h2>Timeline</h2>
  <ul class="timeline">
  {timeline}
  </ul>
</section>
<section>
  <h2>Preflight, usage, and resources</h2>
  <pre>{preflight}</pre>
</section>
<section>
  <h2>Artifacts</h2>
  <ul>
    <li>Human log: <span class="path">{human_log}</span></li>
    <li>Events JSONL: <span class="path">{events_path}</span></li>
    <li>Task cards: <span class="path">{task_cards}</span></li>
    <li>Issue drafts: <span class="path">{issue_drafts}</span></li>
    <li>Validation outputs: <span class="path">{validation}</span></li>
  </ul>
</section>
</main>
</body>
</html>"#,
        run_id = html_escape(&manifest.run_id),
        status = html_escape(status),
        coordinator = html_escape(&manifest.coordinator_session_id),
        coordinator_name = html_escape(&manifest.coordinator_session_name),
        started = html_escape(&manifest.started_at.to_rfc3339()),
        target = html_escape(&manifest.target_wake_at.to_rfc3339()),
        last_activity = html_escape(&manifest.last_activity_at.to_rfc3339()),
        mission = html_escape(
            manifest
                .mission
                .as_deref()
                .unwrap_or("Continue the current session's highest-value verified work.")
        ),
        working_dir = html_escape(manifest.working_dir.as_deref().unwrap_or("unknown")),
        provider = html_escape(&manifest.provider_name),
        model = html_escape(&manifest.model),
        notes = html_escape(&notes),
        timeline = timeline,
        preflight = html_escape(&preflight),
        human_log = html_escape(&manifest.human_log_path.display().to_string()),
        events_path = html_escape(&manifest.events_path.display().to_string()),
        task_cards = html_escape(&manifest.task_cards_dir.display().to_string()),
        issue_drafts = html_escape(&manifest.issue_drafts_dir.display().to_string()),
        validation = html_escape(&manifest.validation_dir.display().to_string()),
    );
    write_text_file(&manifest.review_path, &html)
}

fn event_class(kind: &str) -> &'static str {
    if kind.contains("failed") || kind.contains("cancel") {
        "bad"
    } else if kind.contains("warning") || kind.contains("requested") || kind.contains("handoff") {
        "warn"
    } else if kind.contains("completed") || kind.contains("started") {
        "ok"
    } else {
        "info"
    }
}

fn write_text_file(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    Ok(())
}

fn html_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn build_coordinator_prompt(
    manifest: &OvernightManifest,
    preflight: &OvernightPreflight,
) -> String {
    let mission = manifest
        .mission
        .as_deref()
        .unwrap_or("Continue the current session's highest-value work, prioritizing verified, low-risk progress.");
    format!(
        r#"You are the Overnight Coordinator for Jcode run `{run_id}`.

The user expects to be away until approximately `{target_wake_at}`. This is a target wake/report time, not a hard stop. By that time, the run must be handoff-ready and the review page must explain what happened. You may continue past the target only to finish a bounded, safe, verifiable chunk. The default soft post-wake grace window ends at `{post_wake_grace_until}`.

Mission:
{mission}

Operating contract:
- Optimize for verified, low-risk progress.
- Prefer GH bug issues with objective reproduction, failing tests, static-analysis findings, regression tests, bounded code-quality fixes, and clear crash/panic/wrong-output bugs.
- Avoid taste-based work, vague product decisions, broad rewrites, risky migrations, payments, sending email, pushing to remotes, deleting data, or other external side effects unless explicitly allowed by the user.
- If a bug is found, reproduce/prove it before fixing it.
- Only fix issues that are important, bounded, and verifiable. Otherwise draft a high-quality issue in `{issue_drafts}`.
- You own the run. Spawn swarm/helper agents only if the expected value exceeds usage/resource cost. Default to one coordinator plus at most one helper. Read-only scouts/verifiers are preferred over multiple editors.
- Be aware of RAM/load/battery, especially around compiles, browser automation, indexing, and full test suites. Do not run multiple heavy activities at once unless resources are clearly healthy.
- Do not wait for the user. If you need user judgment/credentials/taste, record it and switch to another useful task.
- Continue finding useful verified work until the target wake/report time unless usage/resources make that unreasonable.

Review/log requirements:
- Keep `{review_notes}` updated as you work.
- For each meaningful task, include a task card with clear Before/After, evidence, validation, files changed, risk, and outcome.
- Put detailed task cards in `{task_cards}` when useful.
- Put reproduction/test/command outputs in `{validation}` when useful.
- The generated review page is `{review_html}` and will be regenerated from logs plus your review notes.

Preflight summary:
{preflight_summary}

Initial steps:
1. Inspect current repo/session state and git status.
2. Build a ranked queue of verifiable candidate tasks.
3. Pick the highest-confidence bounded task.
4. Prove/reproduce before fixing.
5. Validate and update review notes.
6. If done early, repeat discovery and continue.
"#,
        run_id = manifest.run_id,
        target_wake_at = manifest.target_wake_at.to_rfc3339(),
        post_wake_grace_until = manifest.post_wake_grace_until.to_rfc3339(),
        mission = mission,
        issue_drafts = manifest.issue_drafts_dir.display(),
        review_notes = manifest.review_notes_path.display(),
        task_cards = manifest.task_cards_dir.display(),
        validation = manifest.validation_dir.display(),
        review_html = manifest.review_path.display(),
        preflight_summary = preflight_summary(preflight),
    )
}

fn build_continuation_prompt(manifest: &OvernightManifest) -> String {
    let remaining = manifest
        .target_wake_at
        .signed_duration_since(Utc::now())
        .num_minutes()
        .max(0) as u32;
    format!(
        "Overnight continuation: there is about {} remaining until the target wake/report time. If your current task is complete, run another discovery/scoring pass and choose another high-confidence, verifiable task. If you are stuck, record why in `{}` and switch to a smaller bounded task. Update the review notes before continuing.",
        format_minutes(remaining),
        manifest.review_notes_path.display()
    )
}

fn build_handoff_ready_prompt(manifest: &OvernightManifest) -> String {
    format!(
        "Handoff-ready reminder: target wake/report time is in about 30 minutes. Do not abandon useful work, but make the run easy to understand. Update `{}` with current task, completed work, validation state, files changed, risks, skipped work, and next steps. Avoid starting large/risky new changes unless they are isolated and clearly verifiable.",
        manifest.review_notes_path.display()
    )
}

fn build_morning_report_prompt(manifest: &OvernightManifest) -> String {
    format!(
        "Target wake/report time reached. Post a morning report now, even if work is still ongoing. Update `{}` and make sure `{}` is useful. Include completed work, current task, before/after evidence, files changed, validation, risks, usage/resource notes if relevant, and whether you plan to continue. You may continue only if the next chunk is bounded, safe, and verifiable.",
        manifest.review_notes_path.display(),
        manifest.review_path.display()
    )
}

fn build_post_wake_continuation_prompt(manifest: &OvernightManifest) -> String {
    format!(
        "Post-wake continuation: the target wake/report time has passed and the morning report should already be available. You may continue only with bounded, safe, verifiable work that is already in progress or clearly high-value. Do not start broad/risky new changes. Keep `{}` current so the user can safely inspect or interrupt at any time. Soft grace window ends at `{}`.",
        manifest.review_notes_path.display(),
        manifest.post_wake_grace_until.to_rfc3339()
    )
}

fn build_final_wrapup_prompt(manifest: &OvernightManifest) -> String {
    format!(
        "Final overnight wrap-up: the post-wake grace window has expired. Stop starting new work. Finish only immediate cleanup, update `{}` and `{}` with final before/after evidence, validation status, dirty repo state, remaining risks, and next steps, then stop.",
        manifest.review_notes_path.display(),
        manifest.review_path.display()
    )
}

fn prompt_event_summary(prompt: &str) -> String {
    if prompt.starts_with("You are the Overnight Coordinator") {
        "Sending initial overnight coordinator mission".to_string()
    } else if prompt.starts_with("Handoff-ready") {
        "Sending handoff-ready poke".to_string()
    } else if prompt.starts_with("Target wake") {
        "Sending morning report poke".to_string()
    } else if prompt.starts_with("Post-wake continuation") {
        "Sending post-wake continuation poke".to_string()
    } else if prompt.starts_with("Final overnight wrap-up") {
        "Sending final wrap-up poke".to_string()
    } else {
        "Sending continuation poke".to_string()
    }
}

fn preflight_summary(preflight: &OvernightPreflight) -> String {
    format!(
        "Usage risk: {} (confidence: {}). Projected end: {}. Resources: {}. Git: {}.",
        preflight.usage.risk,
        preflight.usage.confidence,
        match (
            preflight.usage.projected_end_min_percent,
            preflight.usage.projected_end_max_percent,
        ) {
            (Some(min), Some(max)) => format!("{:.0}% to {:.0}%", min, max),
            _ => "unknown".to_string(),
        },
        resource_summary(&preflight.resources),
        git_summary(&preflight.git),
    )
}

fn resource_summary(snapshot: &ResourceSnapshot) -> String {
    let memory = snapshot
        .memory_used_percent
        .map(|pct| format!("RAM {:.0}%", pct))
        .unwrap_or_else(|| "RAM unknown".to_string());
    let load = snapshot
        .load_one
        .zip(snapshot.cpu_count)
        .map(|(load, cpus)| format!("load {:.1}/{}", load, cpus))
        .unwrap_or_else(|| "load unknown".to_string());
    let battery = snapshot
        .battery_percent
        .map(|pct| {
            format!(
                "battery {}%{}",
                pct,
                snapshot
                    .battery_status
                    .as_ref()
                    .map(|status| format!(" {}", status))
                    .unwrap_or_default()
            )
        })
        .unwrap_or_else(|| "battery unknown".to_string());
    format!("{}, {}, {}", memory, load, battery)
}

fn git_summary(snapshot: &GitSnapshot) -> String {
    if let Some(error) = snapshot.error.as_ref() {
        return format!("git unavailable ({})", error);
    }
    let dirty = snapshot.dirty_count.unwrap_or(0);
    let branch = snapshot.branch.as_deref().unwrap_or("unknown branch");
    if dirty == 0 {
        format!("{} clean", branch)
    } else {
        format!(
            "{} with {} dirty file{}",
            branch,
            dirty,
            if dirty == 1 { "" } else { "s" }
        )
    }
}

pub fn format_minutes(minutes: u32) -> String {
    if minutes < 60 {
        return format!("{}m", minutes);
    }
    let hours = minutes / 60;
    let mins = minutes % 60;
    if mins == 0 {
        format!("{}h", hours)
    } else {
        format!("{}h {}m", hours, mins)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_duration_accepts_hours_minutes_and_decimals() {
        assert_eq!(parse_duration("7").unwrap().minutes, 420);
        assert_eq!(parse_duration("7h").unwrap().minutes, 420);
        assert_eq!(parse_duration("90m").unwrap().minutes, 90);
        assert_eq!(parse_duration("1.5").unwrap().minutes, 90);
    }

    #[test]
    fn parse_overnight_command_start_with_mission() {
        let parsed = parse_overnight_command("/overnight 7 fix verified bugs")
            .unwrap()
            .unwrap();
        match parsed {
            OvernightCommand::Start { duration, mission } => {
                assert_eq!(duration.minutes, 420);
                assert_eq!(mission.as_deref(), Some("fix verified bugs"));
            }
            other => panic!("unexpected command: {:?}", other),
        }
    }

    #[test]
    fn parse_overnight_command_subcommands() {
        assert_eq!(
            parse_overnight_command("/overnight status")
                .unwrap()
                .unwrap(),
            OvernightCommand::Status
        );
        assert_eq!(
            parse_overnight_command("/overnight log").unwrap().unwrap(),
            OvernightCommand::Log
        );
        assert_eq!(
            parse_overnight_command("/overnight review")
                .unwrap()
                .unwrap(),
            OvernightCommand::Review
        );
        assert_eq!(
            parse_overnight_command("/overnight cancel")
                .unwrap()
                .unwrap(),
            OvernightCommand::Cancel
        );
    }

    #[test]
    fn html_escape_escapes_basic_entities() {
        assert_eq!(html_escape("<a&b>\"'"), "&lt;a&amp;b&gt;&quot;&#39;");
    }

    #[test]
    fn render_review_html_writes_required_sections() {
        let temp = tempfile::tempdir().expect("tempdir");
        let run_dir = temp.path().join("run");
        let now = Utc::now();
        let manifest = OvernightManifest {
            version: OVERNIGHT_VERSION,
            run_id: "overnight_test".to_string(),
            parent_session_id: "parent".to_string(),
            coordinator_session_id: "coord".to_string(),
            coordinator_session_name: "coordinator".to_string(),
            started_at: now,
            target_wake_at: now + ChronoDuration::hours(7),
            handoff_ready_at: now + ChronoDuration::hours(6),
            post_wake_grace_until: now + ChronoDuration::hours(9),
            morning_report_posted_at: None,
            completed_at: None,
            cancel_requested_at: None,
            status: OvernightRunStatus::Running,
            mission: Some("verify things".to_string()),
            working_dir: Some("/tmp/project".to_string()),
            provider_name: "test-provider".to_string(),
            model: "test-model".to_string(),
            max_agents_guidance: 2,
            process_id: 123,
            run_dir: run_dir.clone(),
            events_path: run_dir.join("events.jsonl"),
            human_log_path: run_dir.join("run.log"),
            review_path: run_dir.join("review.html"),
            review_notes_path: run_dir.join("review-notes.md"),
            preflight_path: run_dir.join("preflight.json"),
            task_cards_dir: run_dir.join("task-cards"),
            issue_drafts_dir: run_dir.join("issue-drafts"),
            validation_dir: run_dir.join("validation"),
            last_activity_at: now,
        };
        write_initial_review_notes(&manifest).expect("write notes");
        render_review_html(&manifest).expect("render review");

        let html = std::fs::read_to_string(&manifest.review_path).expect("read review html");
        assert!(html.contains("Executive summary"));
        assert!(html.contains("Coordinator review notes"));
        assert!(html.contains("Timeline"));
        assert!(html.contains("Artifacts"));
        assert!(html.contains("Before"));
        assert!(html.contains("After"));
    }
}
