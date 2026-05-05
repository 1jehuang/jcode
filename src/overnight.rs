use crate::agent::Agent;
use crate::provider::Provider;
use crate::session::{Session, SessionStatus};
use crate::storage;
use crate::tool::Registry;
use anyhow::{Context, Result};
use chrono::{Duration as ChronoDuration, Utc};
use serde_json::{Value, json};
use std::ffi::CString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub use jcode_overnight_core::{
    GitSnapshot, OVERNIGHT_VERSION, OvernightCommand, OvernightDuration, OvernightEvent,
    OvernightManifest, OvernightPreflight, OvernightProgressCard, OvernightRunStatus,
    OvernightTaskCard, OvernightTaskCardAfter, OvernightTaskCardBefore, OvernightTaskCardSummary,
    OvernightTaskCardValidation, OvernightTaskStatusCounts, ResourceSnapshot, UsageLimitSnapshot,
    UsageProjection, UsageProviderSnapshot, build_continuation_prompt, build_coordinator_prompt,
    build_final_wrapup_prompt, build_handoff_ready_prompt, build_morning_report_prompt,
    build_post_wake_continuation_prompt, build_progress_card_from_parts, build_review_html,
    build_visible_current_session_prompt, event_class, format_log_markdown_from_events,
    format_minutes, format_status_markdown_from_summary, git_summary, html_escape, overnight_usage,
    parse_duration, parse_overnight_command, preflight_summary, prompt_event_summary,
    render_task_cards_html, resource_summary, summarize_task_cards_slice, task_card_title,
    task_card_validated, task_status_bucket,
};

const RESOURCE_SAMPLE_INTERVAL: Duration = Duration::from_secs(5 * 60);
const LONG_TURN_NOTICE_INTERVAL: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone)]
pub struct OvernightLaunch {
    pub manifest: OvernightManifest,
    /// Initial coordinator prompt to enqueue in the visible launching session.
    /// When present, the TUI should run this as a normal user turn so tool calls,
    /// spawned agents, and streaming output are visible like any other session.
    pub initial_prompt: Option<String>,
}

pub struct OvernightStartOptions {
    pub duration: OvernightDuration,
    pub mission: Option<String>,
    pub parent_session: Session,
    pub provider: Arc<dyn Provider>,
    pub registry: Registry,
    pub working_dir: Option<PathBuf>,
    /// When true, run the overnight coordinator in the session that launched
    /// `/overnight` instead of forking an invisible child transcript.
    pub use_current_session: bool,
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

    let mut child = if options.use_current_session {
        options.parent_session.clone()
    } else {
        create_coordinator_session(&options.parent_session, &options.mission)?
    };
    if let Some(working_dir) = options.working_dir.as_ref() {
        child.working_dir = Some(working_dir.to_string_lossy().to_string());
    }
    child.model = Some(options.provider.model());
    let coordinator_session_id = child.id.clone();
    let coordinator_session_name = child.display_name().to_string();
    let child_is_canary = child.is_canary;
    if !options.use_current_session {
        child.status = SessionStatus::Closed;
    }
    child.save()?;

    if !options.use_current_session
        && let Ok(todos) = crate::todo::load_todos(&options.parent_session.id)
    {
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
    write_task_card_schema(&manifest)?;
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

    let initial_prompt = if options.use_current_session {
        Some(build_visible_current_session_prompt(&manifest))
    } else {
        spawn_supervisor(
            manifest.clone(),
            child,
            options.provider,
            options.registry,
            child_is_canary,
        );
        None
    };

    Ok(OvernightLaunch {
        manifest,
        initial_prompt,
    })
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
    child.reasoning_effort = parent.reasoning_effort.clone();
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

pub fn read_task_cards(manifest: &OvernightManifest) -> Result<Vec<OvernightTaskCard>> {
    if !manifest.task_cards_dir.exists() {
        return Ok(Vec::new());
    }

    let mut paths = Vec::new();
    for entry in std::fs::read_dir(&manifest.task_cards_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file() {
            continue;
        }
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if file_name.starts_with('_')
            || path.extension().and_then(|ext| ext.to_str()) != Some("json")
        {
            continue;
        }
        paths.push(path);
    }
    paths.sort();

    let mut cards = Vec::new();
    for path in paths {
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(mut parsed) = serde_json::from_str::<Vec<OvernightTaskCard>>(&contents) {
            cards.append(&mut parsed);
        } else if let Ok(card) = serde_json::from_str::<OvernightTaskCard>(&contents) {
            cards.push(card);
        }
    }

    cards.retain(|card| !card.title.trim().is_empty() || !card.id.trim().is_empty());
    cards.sort_by(|a, b| {
        a.updated_at
            .cmp(&b.updated_at)
            .then_with(|| a.id.cmp(&b.id))
            .then_with(|| a.title.cmp(&b.title))
    });
    Ok(cards)
}

pub fn summarize_task_cards(manifest: &OvernightManifest) -> OvernightTaskCardSummary {
    summarize_task_cards_slice(&read_task_cards(manifest).unwrap_or_default())
}

pub fn format_progress_card_content(manifest: &OvernightManifest) -> Result<String> {
    Ok(serde_json::to_string(&build_progress_card(manifest))?)
}

pub fn latest_progress_card_content() -> Result<Option<String>> {
    latest_manifest()?
        .map(|manifest| format_progress_card_content(&manifest))
        .transpose()
}

pub fn build_progress_card(manifest: &OvernightManifest) -> OvernightProgressCard {
    let events = read_events(manifest).unwrap_or_default();
    let preflight = read_preflight(manifest);
    let task_cards = read_task_cards(manifest).unwrap_or_default();
    build_progress_card_from_parts(
        manifest,
        &events,
        preflight.as_ref(),
        &task_cards,
        Utc::now(),
    )
}

fn read_preflight(manifest: &OvernightManifest) -> Option<OvernightPreflight> {
    if !manifest.preflight_path.exists() {
        return None;
    }
    storage::read_json(&manifest.preflight_path).ok()
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
    let task_summary = summarize_task_cards(manifest);
    format_status_markdown_from_summary(manifest, &task_summary, Utc::now())
}

pub fn format_log_markdown(manifest: &OvernightManifest, max_lines: usize) -> String {
    let events = read_events(manifest).unwrap_or_default();
    format_log_markdown_from_events(manifest, &events, max_lines)
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

fn write_task_card_schema(manifest: &OvernightManifest) -> Result<()> {
    let schema_path = manifest.task_cards_dir.join("task-card-schema.md");
    if schema_path.exists() {
        return Ok(());
    }
    let content = r#"# Overnight task-card schema

Create one `*.json` file per meaningful task. Keep it current while you work. The generated review page and TUI progress card read these files continuously.

Required spirit: make the morning review objectively useful. Each completed or important attempted task should show why it was selected, what was true before, what changed after, and exactly how it was validated.

```json
{
  "id": "task-001",
  "title": "Fix deterministic provider reload timeout",
  "status": "active | completed | blocked | deferred | failed | skipped",
  "priority": "high | medium | low",
  "source": "GH issue | failing test | static analysis | code quality | user request",
  "why_selected": "Objective, bounded, high-confidence reason for choosing this task.",
  "verifiability": "How we can prove the problem and prove the fix.",
  "risk": "low | medium | high",
  "outcome": "Concise final outcome or current state.",
  "before": {
    "problem": "Observed bug/code state before work.",
    "evidence": ["validation/task-001-before.txt"]
  },
  "after": {
    "change": "Changed behavior or code state after work.",
    "files_changed": ["src/example.rs"],
    "evidence": ["validation/task-001-after.txt"]
  },
  "validation": {
    "commands": ["cargo test provider_reload"],
    "result": "passed",
    "evidence": ["validation/task-001-test.txt"]
  },
  "followups": ["Optional remaining safe next step"],
  "updated_at": "2026-05-01T08:00:00Z"
}
```

Notes:
- Use `active` for the task currently being worked.
- Use `completed` only when validation evidence exists or the task is intentionally documentation/issue-draft only.
- Use `blocked` when user input, credentials, external access, or taste is required.
- Use `deferred` or `skipped` when a task was considered but not pursued.
"#;
    write_text_file(&schema_path, content)
}

pub fn render_review_html(manifest: &OvernightManifest) -> Result<()> {
    let events = read_events(manifest).unwrap_or_default();
    let notes = std::fs::read_to_string(&manifest.review_notes_path).unwrap_or_else(|_| {
        "# Overnight review notes

Coordinator has not written notes yet."
            .to_string()
    });
    let preflight = if manifest.preflight_path.exists() {
        std::fs::read_to_string(&manifest.preflight_path).unwrap_or_default()
    } else {
        String::new()
    };
    let task_cards = read_task_cards(manifest).unwrap_or_default();
    let html = build_review_html(manifest, &events, &notes, &preflight, &task_cards);
    write_text_file(&manifest.review_path, &html)
}

fn write_text_file(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_manifest(root: &Path, run_id: &str) -> OvernightManifest {
        let run_dir = root.join("run");
        let now = Utc::now();
        OvernightManifest {
            version: OVERNIGHT_VERSION,
            run_id: run_id.to_string(),
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
        }
    }

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
        let manifest = test_manifest(temp.path(), "overnight_test");
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

    #[test]
    fn task_card_summary_reads_structured_json_cards() {
        let temp = tempfile::tempdir().expect("tempdir");
        let manifest = test_manifest(temp.path(), "overnight_cards");
        std::fs::create_dir_all(&manifest.task_cards_dir).expect("task card dir");
        std::fs::write(
            manifest.task_cards_dir.join("task-001.json"),
            r#"{
              "id": "task-001",
              "title": "Fix deterministic bug",
              "status": "completed",
              "risk": "low",
              "validation": { "commands": ["cargo test bug"], "result": "passed" },
              "updated_at": "2026-05-01T08:00:00Z"
            }"#,
        )
        .expect("write completed card");
        std::fs::write(
            manifest.task_cards_dir.join("task-002.json"),
            r#"{
              "id": "task-002",
              "title": "Investigate static-analysis finding",
              "status": "active",
              "risk": "high",
              "updated_at": "2026-05-01T08:10:00Z"
            }"#,
        )
        .expect("write active card");

        let cards = read_task_cards(&manifest).expect("read cards");
        assert_eq!(cards.len(), 2);
        let summary = summarize_task_cards_slice(&cards);
        assert_eq!(summary.total, 2);
        assert_eq!(summary.counts.completed, 1);
        assert_eq!(summary.counts.active, 1);
        assert_eq!(summary.validated, 1);
        assert_eq!(summary.high_risk, 1);
        assert_eq!(
            summary.latest_title.as_deref(),
            Some("Investigate static-analysis finding")
        );
    }

    #[test]
    fn progress_card_content_includes_task_summary_and_latest_event() {
        let temp = tempfile::tempdir().expect("tempdir");
        let manifest = test_manifest(temp.path(), "overnight_progress");
        std::fs::create_dir_all(&manifest.task_cards_dir).expect("task card dir");
        std::fs::create_dir_all(manifest.events_path.parent().unwrap()).expect("events dir");
        std::fs::write(
            manifest.task_cards_dir.join("task-001.json"),
            r#"{
              "id": "task-001",
              "title": "Verify reload race",
              "status": "completed",
              "validation": { "result": "passed" },
              "updated_at": "2026-05-01T08:00:00Z"
            }"#,
        )
        .expect("write card");
        let event = OvernightEvent {
            timestamp: Utc::now(),
            run_id: manifest.run_id.clone(),
            session_id: Some(manifest.coordinator_session_id.clone()),
            kind: "coordinator_turn_completed".to_string(),
            summary: "Coordinator turn completed".to_string(),
            details: json!({}),
            meaningful: true,
        };
        std::fs::write(
            &manifest.events_path,
            format!("{}\n", serde_json::to_string(&event).unwrap()),
        )
        .expect("write event");

        let card: OvernightProgressCard =
            serde_json::from_str(&format_progress_card_content(&manifest).expect("progress card"))
                .expect("parse card");
        assert_eq!(card.task_summary.counts.completed, 1);
        assert_eq!(card.task_summary.validated, 1);
        assert_eq!(
            card.latest_event_kind.as_deref(),
            Some("coordinator_turn_completed")
        );
        assert_eq!(
            card.active_task_title.as_deref(),
            Some("Verify reload race")
        );
    }

    #[test]
    fn render_review_html_includes_structured_task_cards() {
        let temp = tempfile::tempdir().expect("tempdir");
        let manifest = test_manifest(temp.path(), "overnight_review_cards");
        write_initial_review_notes(&manifest).expect("write notes");
        std::fs::create_dir_all(&manifest.task_cards_dir).expect("task card dir");
        std::fs::write(
            manifest.task_cards_dir.join("task-001.json"),
            r#"{
              "id": "task-001",
              "title": "Fix deterministic bug",
              "status": "completed",
              "why_selected": "Reproducible failure",
              "before": { "problem": "Test failed before the fix" },
              "after": { "change": "Test passes after the fix", "files_changed": ["src/example.rs"] },
              "validation": { "commands": ["cargo test deterministic_bug"], "result": "passed" },
              "updated_at": "2026-05-01T08:00:00Z"
            }"#,
        )
        .expect("write card");

        render_review_html(&manifest).expect("render review");
        let html = std::fs::read_to_string(&manifest.review_path).expect("read html");
        assert!(html.contains("Structured task cards"));
        assert!(html.contains("Fix deterministic bug"));
        assert!(html.contains("Reproducible failure"));
        assert!(html.contains("cargo test deterministic_bug"));
    }
}
