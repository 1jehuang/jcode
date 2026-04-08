use crate::storage;
use chrono::{DateTime, Datelike, NaiveDate, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant, SystemTime};

const TELEMETRY_ENDPOINT: &str = "https://jcode-telemetry.jeremyhuang55555.workers.dev/v1/event";
const ASYNC_SEND_TIMEOUT: Duration = Duration::from_secs(5);
const BLOCKING_INSTALL_TIMEOUT: Duration = Duration::from_millis(1200);
const BLOCKING_LIFECYCLE_TIMEOUT: Duration = Duration::from_millis(800);
const TELEMETRY_SCHEMA_VERSION: u32 = 3;

static SESSION_STATE: Mutex<Option<SessionTelemetry>> = Mutex::new(None);

static ERROR_PROVIDER_TIMEOUT: AtomicU32 = AtomicU32::new(0);
static ERROR_AUTH_FAILED: AtomicU32 = AtomicU32::new(0);
static ERROR_TOOL_ERROR: AtomicU32 = AtomicU32::new(0);
static ERROR_MCP_ERROR: AtomicU32 = AtomicU32::new(0);
static ERROR_RATE_LIMITED: AtomicU32 = AtomicU32::new(0);
static PROVIDER_SWITCHES: AtomicU32 = AtomicU32::new(0);
static MODEL_SWITCHES: AtomicU32 = AtomicU32::new(0);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InstallEvent {
    event_id: String,
    id: String,
    event: &'static str,
    version: String,
    os: &'static str,
    arch: &'static str,
    schema_version: u32,
    build_channel: String,
    is_git_checkout: bool,
    is_ci: bool,
    ran_from_cargo: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UpgradeEvent {
    event_id: String,
    id: String,
    event: &'static str,
    version: String,
    os: &'static str,
    arch: &'static str,
    from_version: String,
    schema_version: u32,
    build_channel: String,
    is_git_checkout: bool,
    is_ci: bool,
    ran_from_cargo: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AuthEvent {
    event_id: String,
    id: String,
    event: &'static str,
    version: String,
    os: &'static str,
    arch: &'static str,
    auth_provider: String,
    auth_method: String,
    schema_version: u32,
    build_channel: String,
    is_git_checkout: bool,
    is_ci: bool,
    ran_from_cargo: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionStartEvent {
    event_id: String,
    id: String,
    session_id: String,
    event: &'static str,
    version: String,
    os: &'static str,
    arch: &'static str,
    provider_start: String,
    model_start: String,
    resumed_session: bool,
    session_start_hour_utc: u32,
    session_start_weekday_utc: u32,
    previous_session_gap_secs: Option<u64>,
    sessions_started_24h: u32,
    sessions_started_7d: u32,
    active_sessions_at_start: u32,
    other_active_sessions_at_start: u32,
    schema_version: u32,
    build_channel: String,
    is_git_checkout: bool,
    is_ci: bool,
    ran_from_cargo: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OnboardingStepEvent {
    event_id: String,
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    event: &'static str,
    version: String,
    os: &'static str,
    arch: &'static str,
    step: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    auth_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    auth_method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    milestone_elapsed_ms: Option<u64>,
    schema_version: u32,
    build_channel: String,
    is_git_checkout: bool,
    is_ci: bool,
    ran_from_cargo: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FeedbackEvent {
    event_id: String,
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    event: &'static str,
    version: String,
    os: &'static str,
    arch: &'static str,
    feedback_rating: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    feedback_reason: Option<String>,
    schema_version: u32,
    build_channel: String,
    is_git_checkout: bool,
    is_ci: bool,
    ran_from_cargo: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionLifecycleEvent {
    event_id: String,
    id: String,
    session_id: String,
    event: &'static str,
    version: String,
    os: &'static str,
    arch: &'static str,
    provider_start: String,
    provider_end: String,
    model_start: String,
    model_end: String,
    provider_switches: u32,
    model_switches: u32,
    duration_mins: u64,
    duration_secs: u64,
    turns: u32,
    had_user_prompt: bool,
    had_assistant_response: bool,
    assistant_responses: u32,
    first_assistant_response_ms: Option<u64>,
    first_tool_call_ms: Option<u64>,
    first_tool_success_ms: Option<u64>,
    first_file_edit_ms: Option<u64>,
    first_test_pass_ms: Option<u64>,
    tool_calls: u32,
    tool_failures: u32,
    executed_tool_calls: u32,
    executed_tool_successes: u32,
    executed_tool_failures: u32,
    tool_latency_total_ms: u64,
    tool_latency_max_ms: u64,
    file_write_calls: u32,
    tests_run: u32,
    tests_passed: u32,
    feature_memory_used: bool,
    feature_swarm_used: bool,
    feature_web_used: bool,
    feature_email_used: bool,
    feature_mcp_used: bool,
    feature_side_panel_used: bool,
    feature_goal_used: bool,
    feature_selfdev_used: bool,
    feature_background_used: bool,
    feature_subagent_used: bool,
    unique_mcp_servers: u32,
    session_success: bool,
    abandoned_before_response: bool,
    transport_https: u32,
    transport_persistent_ws_fresh: u32,
    transport_persistent_ws_reuse: u32,
    transport_cli_subprocess: u32,
    transport_native_http2: u32,
    transport_other: u32,
    tool_cat_read_search: u32,
    tool_cat_write: u32,
    tool_cat_shell: u32,
    tool_cat_web: u32,
    tool_cat_memory: u32,
    tool_cat_subagent: u32,
    tool_cat_swarm: u32,
    tool_cat_email: u32,
    tool_cat_side_panel: u32,
    tool_cat_goal: u32,
    tool_cat_mcp: u32,
    tool_cat_other: u32,
    command_login_used: bool,
    command_model_used: bool,
    command_usage_used: bool,
    command_resume_used: bool,
    command_memory_used: bool,
    command_swarm_used: bool,
    command_goal_used: bool,
    command_selfdev_used: bool,
    command_feedback_used: bool,
    command_other_used: bool,
    workflow_chat_only: bool,
    workflow_coding_used: bool,
    workflow_research_used: bool,
    workflow_tests_used: bool,
    workflow_background_used: bool,
    workflow_subagent_used: bool,
    workflow_swarm_used: bool,
    project_repo_present: bool,
    project_lang_rust: bool,
    project_lang_js_ts: bool,
    project_lang_python: bool,
    project_lang_go: bool,
    project_lang_markdown: bool,
    project_lang_mixed: bool,
    days_since_install: Option<u32>,
    active_days_7d: u32,
    active_days_30d: u32,
    session_start_hour_utc: u32,
    session_start_weekday_utc: u32,
    session_end_hour_utc: u32,
    session_end_weekday_utc: u32,
    previous_session_gap_secs: Option<u64>,
    sessions_started_24h: u32,
    sessions_started_7d: u32,
    active_sessions_at_start: u32,
    other_active_sessions_at_start: u32,
    max_concurrent_sessions: u32,
    multi_sessioned: bool,
    resumed_session: bool,
    end_reason: &'static str,
    schema_version: u32,
    build_channel: String,
    is_git_checkout: bool,
    is_ci: bool,
    ran_from_cargo: bool,
    errors: ErrorCounts,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ErrorCounts {
    provider_timeout: u32,
    auth_failed: u32,
    tool_error: u32,
    mcp_error: u32,
    rate_limited: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TurnEndEvent {
    event_id: String,
    id: String,
    session_id: String,
    event: &'static str,
    version: String,
    os: &'static str,
    arch: &'static str,
    turn_index: u32,
    turn_started_ms: u64,
    turn_active_duration_ms: u64,
    idle_before_turn_ms: Option<u64>,
    idle_after_turn_ms: u64,
    assistant_responses: u32,
    first_assistant_response_ms: Option<u64>,
    first_tool_call_ms: Option<u64>,
    first_tool_success_ms: Option<u64>,
    first_file_edit_ms: Option<u64>,
    first_test_pass_ms: Option<u64>,
    tool_calls: u32,
    tool_failures: u32,
    executed_tool_calls: u32,
    executed_tool_successes: u32,
    executed_tool_failures: u32,
    tool_latency_total_ms: u64,
    tool_latency_max_ms: u64,
    file_write_calls: u32,
    tests_run: u32,
    tests_passed: u32,
    feature_memory_used: bool,
    feature_swarm_used: bool,
    feature_web_used: bool,
    feature_email_used: bool,
    feature_mcp_used: bool,
    feature_side_panel_used: bool,
    feature_goal_used: bool,
    feature_selfdev_used: bool,
    feature_background_used: bool,
    feature_subagent_used: bool,
    unique_mcp_servers: u32,
    tool_cat_read_search: u32,
    tool_cat_write: u32,
    tool_cat_shell: u32,
    tool_cat_web: u32,
    tool_cat_memory: u32,
    tool_cat_subagent: u32,
    tool_cat_swarm: u32,
    tool_cat_email: u32,
    tool_cat_side_panel: u32,
    tool_cat_goal: u32,
    tool_cat_mcp: u32,
    tool_cat_other: u32,
    workflow_chat_only: bool,
    workflow_coding_used: bool,
    workflow_research_used: bool,
    workflow_tests_used: bool,
    workflow_background_used: bool,
    workflow_subagent_used: bool,
    workflow_swarm_used: bool,
    turn_success: bool,
    turn_abandoned: bool,
    turn_end_reason: &'static str,
    schema_version: u32,
    build_channel: String,
    is_git_checkout: bool,
    is_ci: bool,
    ran_from_cargo: bool,
}

#[derive(Debug, Clone)]
struct TurnTelemetry {
    turn_index: u32,
    started_at: Instant,
    last_activity_at: Instant,
    started_ms_since_session: u64,
    idle_before_turn_ms: Option<u64>,
    assistant_responses: u32,
    first_assistant_response_ms: Option<u64>,
    first_tool_call_ms: Option<u64>,
    first_tool_success_ms: Option<u64>,
    first_file_edit_ms: Option<u64>,
    first_test_pass_ms: Option<u64>,
    tool_calls: u32,
    tool_failures: u32,
    executed_tool_calls: u32,
    executed_tool_successes: u32,
    executed_tool_failures: u32,
    tool_latency_total_ms: u64,
    tool_latency_max_ms: u64,
    file_write_calls: u32,
    tests_run: u32,
    tests_passed: u32,
    feature_memory_used: bool,
    feature_swarm_used: bool,
    feature_web_used: bool,
    feature_email_used: bool,
    feature_mcp_used: bool,
    feature_side_panel_used: bool,
    feature_goal_used: bool,
    feature_selfdev_used: bool,
    feature_background_used: bool,
    feature_subagent_used: bool,
    unique_mcp_servers: HashSet<String>,
    tool_cat_read_search: u32,
    tool_cat_write: u32,
    tool_cat_shell: u32,
    tool_cat_web: u32,
    tool_cat_memory: u32,
    tool_cat_subagent: u32,
    tool_cat_swarm: u32,
    tool_cat_email: u32,
    tool_cat_side_panel: u32,
    tool_cat_goal: u32,
    tool_cat_mcp: u32,
    tool_cat_other: u32,
}

#[derive(Debug, Clone)]
struct SessionTelemetry {
    session_id: String,
    started_at: Instant,
    started_at_utc: DateTime<Utc>,
    provider_start: String,
    model_start: String,
    turns: u32,
    had_user_prompt: bool,
    had_assistant_response: bool,
    assistant_responses: u32,
    first_assistant_response_ms: Option<u64>,
    first_tool_call_ms: Option<u64>,
    first_tool_success_ms: Option<u64>,
    first_file_edit_ms: Option<u64>,
    first_test_pass_ms: Option<u64>,
    tool_calls: u32,
    tool_failures: u32,
    executed_tool_calls: u32,
    executed_tool_successes: u32,
    executed_tool_failures: u32,
    tool_latency_total_ms: u64,
    tool_latency_max_ms: u64,
    file_write_calls: u32,
    tests_run: u32,
    tests_passed: u32,
    feature_memory_used: bool,
    feature_swarm_used: bool,
    feature_web_used: bool,
    feature_email_used: bool,
    feature_mcp_used: bool,
    feature_side_panel_used: bool,
    feature_goal_used: bool,
    feature_selfdev_used: bool,
    feature_background_used: bool,
    feature_subagent_used: bool,
    unique_mcp_servers: HashSet<String>,
    transport_https: u32,
    transport_persistent_ws_fresh: u32,
    transport_persistent_ws_reuse: u32,
    transport_cli_subprocess: u32,
    transport_native_http2: u32,
    transport_other: u32,
    tool_cat_read_search: u32,
    tool_cat_write: u32,
    tool_cat_shell: u32,
    tool_cat_web: u32,
    tool_cat_memory: u32,
    tool_cat_subagent: u32,
    tool_cat_swarm: u32,
    tool_cat_email: u32,
    tool_cat_side_panel: u32,
    tool_cat_goal: u32,
    tool_cat_mcp: u32,
    tool_cat_other: u32,
    command_login_used: bool,
    command_model_used: bool,
    command_usage_used: bool,
    command_resume_used: bool,
    command_memory_used: bool,
    command_swarm_used: bool,
    command_goal_used: bool,
    command_selfdev_used: bool,
    command_feedback_used: bool,
    command_other_used: bool,
    previous_session_gap_secs: Option<u64>,
    sessions_started_24h: u32,
    sessions_started_7d: u32,
    active_sessions_at_start: u32,
    other_active_sessions_at_start: u32,
    max_concurrent_sessions: u32,
    current_turn: Option<TurnTelemetry>,
    resumed_session: bool,
    start_event_sent: bool,
}

impl TurnTelemetry {
    fn new(
        turn_index: u32,
        started_at: Instant,
        started_ms_since_session: u64,
        idle_before_turn_ms: Option<u64>,
    ) -> Self {
        Self {
            turn_index,
            started_at,
            last_activity_at: started_at,
            started_ms_since_session,
            idle_before_turn_ms,
            assistant_responses: 0,
            first_assistant_response_ms: None,
            first_tool_call_ms: None,
            first_tool_success_ms: None,
            first_file_edit_ms: None,
            first_test_pass_ms: None,
            tool_calls: 0,
            tool_failures: 0,
            executed_tool_calls: 0,
            executed_tool_successes: 0,
            executed_tool_failures: 0,
            tool_latency_total_ms: 0,
            tool_latency_max_ms: 0,
            file_write_calls: 0,
            tests_run: 0,
            tests_passed: 0,
            feature_memory_used: false,
            feature_swarm_used: false,
            feature_web_used: false,
            feature_email_used: false,
            feature_mcp_used: false,
            feature_side_panel_used: false,
            feature_goal_used: false,
            feature_selfdev_used: false,
            feature_background_used: false,
            feature_subagent_used: false,
            unique_mcp_servers: HashSet::new(),
            tool_cat_read_search: 0,
            tool_cat_write: 0,
            tool_cat_shell: 0,
            tool_cat_web: 0,
            tool_cat_memory: 0,
            tool_cat_subagent: 0,
            tool_cat_swarm: 0,
            tool_cat_email: 0,
            tool_cat_side_panel: 0,
            tool_cat_goal: 0,
            tool_cat_mcp: 0,
            tool_cat_other: 0,
        }
    }
}

fn workflow_flags_from_counts(
    had_user_prompt: bool,
    file_write_calls: u32,
    tests_run: u32,
    tests_passed: u32,
    feature_web_used: bool,
    feature_background_used: bool,
    feature_subagent_used: bool,
    feature_swarm_used: bool,
    tool_cat_write: u32,
    tool_cat_web: u32,
    tool_cat_subagent: u32,
    tool_cat_swarm: u32,
) -> (bool, bool, bool, bool, bool, bool, bool) {
    let workflow_coding_used = file_write_calls > 0 || tool_cat_write > 0;
    let workflow_research_used = feature_web_used || tool_cat_web > 0;
    let workflow_tests_used = tests_run > 0 || tests_passed > 0;
    let workflow_background_used = feature_background_used;
    let workflow_subagent_used = feature_subagent_used || tool_cat_subagent > 0;
    let workflow_swarm_used = feature_swarm_used || tool_cat_swarm > 0;
    let workflow_chat_only = had_user_prompt
        && !workflow_coding_used
        && !workflow_research_used
        && !workflow_tests_used
        && !workflow_background_used
        && !workflow_subagent_used
        && !workflow_swarm_used;
    (
        workflow_chat_only,
        workflow_coding_used,
        workflow_research_used,
        workflow_tests_used,
        workflow_background_used,
        workflow_subagent_used,
        workflow_swarm_used,
    )
}

#[derive(Debug, Clone, Default)]
struct ProjectProfile {
    repo_present: bool,
    lang_rust: bool,
    lang_js_ts: bool,
    lang_python: bool,
    lang_go: bool,
    lang_markdown: bool,
}

impl ProjectProfile {
    fn mixed(&self) -> bool {
        [
            self.lang_rust,
            self.lang_js_ts,
            self.lang_python,
            self.lang_go,
            self.lang_markdown,
        ]
        .into_iter()
        .filter(|value| *value)
        .count()
            > 1
    }
}

#[derive(Debug, Clone, Copy)]
enum ToolCategory {
    ReadSearch,
    Write,
    Shell,
    Web,
    Memory,
    Subagent,
    Swarm,
    Email,
    SidePanel,
    Goal,
    Mcp,
    Other,
}

#[derive(Debug, Clone, Copy)]
enum DeliveryMode {
    Background,
    Blocking(Duration),
}

#[derive(Debug, Clone, Copy)]
pub enum SessionEndReason {
    NormalExit,
    Panic,
    Signal,
    Disconnect,
    Reload,
    Unknown,
}

impl SessionEndReason {
    fn as_str(self) -> &'static str {
        match self {
            SessionEndReason::NormalExit => "normal_exit",
            SessionEndReason::Panic => "panic",
            SessionEndReason::Signal => "signal",
            SessionEndReason::Disconnect => "disconnect",
            SessionEndReason::Reload => "reload",
            SessionEndReason::Unknown => "unknown",
        }
    }
}

pub fn is_enabled() -> bool {
    if std::env::var("JCODE_NO_TELEMETRY").is_ok() || std::env::var("DO_NOT_TRACK").is_ok() {
        return false;
    }
    if let Ok(dir) = storage::jcode_dir() {
        if dir.join("no_telemetry").exists() {
            return false;
        }
    }
    true
}

fn telemetry_id_path() -> Option<PathBuf> {
    storage::jcode_dir().ok().map(|d| d.join("telemetry_id"))
}

fn install_recorded_path() -> Option<PathBuf> {
    storage::jcode_dir()
        .ok()
        .map(|d| d.join("telemetry_install_sent"))
}

fn version_recorded_path() -> Option<PathBuf> {
    storage::jcode_dir()
        .ok()
        .map(|d| d.join("telemetry_version_sent"))
}

fn telemetry_state_path(name: &str) -> Option<PathBuf> {
    storage::jcode_dir().ok().map(|d| d.join(name))
}

fn milestone_recorded_path(id: &str, step: &str) -> Option<PathBuf> {
    telemetry_state_path(&format!(
        "telemetry_milestone_{}_{}",
        sanitize_telemetry_label(step),
        id
    ))
}

fn active_days_path(id: &str) -> Option<PathBuf> {
    telemetry_state_path(&format!("telemetry_active_days_{}.txt", id))
}

fn session_starts_path(id: &str) -> Option<PathBuf> {
    telemetry_state_path(&format!("telemetry_session_starts_{}.txt", id))
}

fn active_sessions_dir() -> Option<PathBuf> {
    telemetry_state_path("telemetry_active_sessions")
}

fn active_session_file(session_id: &str) -> Option<PathBuf> {
    active_sessions_dir().map(|dir| dir.join(format!("{}.active", session_id)))
}

fn write_private_file(path: &PathBuf, value: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, value);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}

fn utc_hour(timestamp: DateTime<Utc>) -> u32 {
    timestamp.hour()
}

fn utc_weekday(timestamp: DateTime<Utc>) -> u32 {
    timestamp.weekday().num_days_from_monday()
}

fn write_private_dir_file(path: &PathBuf, value: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    write_private_file(path, value);
}

fn read_epoch_lines(path: &PathBuf) -> Vec<i64> {
    std::fs::read_to_string(path)
        .ok()
        .into_iter()
        .flat_map(|text| {
            text.lines()
                .map(str::trim)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter_map(|line| line.parse::<i64>().ok())
        .collect()
}

fn update_session_start_history(
    id: &str,
    started_at_utc: DateTime<Utc>,
) -> (Option<u64>, u32, u32) {
    let Some(path) = session_starts_path(id) else {
        return (None, 0, 0);
    };
    let now = started_at_utc.timestamp();
    let cutoff_30d = now - 30 * 24 * 60 * 60;
    let mut starts = read_epoch_lines(&path)
        .into_iter()
        .filter(|value| *value >= cutoff_30d)
        .collect::<Vec<_>>();
    starts.sort_unstable();
    let previous = starts.last().copied();
    starts.push(now);
    let rendered = starts
        .iter()
        .map(i64::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    write_private_dir_file(&path, &rendered);
    let sessions_started_24h = starts
        .iter()
        .filter(|value| now.saturating_sub(**value) < 24 * 60 * 60)
        .count()
        .min(u32::MAX as usize) as u32;
    let sessions_started_7d = starts
        .iter()
        .filter(|value| now.saturating_sub(**value) < 7 * 24 * 60 * 60)
        .count()
        .min(u32::MAX as usize) as u32;
    let previous_session_gap_secs = previous
        .and_then(|value| now.checked_sub(value))
        .map(|value| value.min(u64::MAX as i64) as u64);
    (
        previous_session_gap_secs,
        sessions_started_24h,
        sessions_started_7d,
    )
}

fn prune_active_session_files(dir: &PathBuf) -> u32 {
    let _ = std::fs::create_dir_all(dir);
    let now = SystemTime::now();
    let max_age = Duration::from_secs(24 * 60 * 60);
    let mut count = 0u32;
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return 0,
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let fresh = entry
            .metadata()
            .ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .map(|age| age <= max_age)
            .unwrap_or(false);
        if fresh {
            count = count.saturating_add(1);
        } else {
            let _ = std::fs::remove_file(path);
        }
    }
    count
}

fn register_active_session(session_id: &str) -> (u32, u32) {
    let Some(dir) = active_sessions_dir() else {
        return (0, 0);
    };
    let existing = prune_active_session_files(&dir);
    if let Some(path) = active_session_file(session_id) {
        write_private_dir_file(&path, "1");
    }
    (existing.saturating_add(1), existing)
}

fn observe_active_sessions() -> u32 {
    active_sessions_dir()
        .map(|dir| prune_active_session_files(&dir))
        .unwrap_or(0)
}

fn unregister_active_session(session_id: &str) {
    if let Some(path) = active_session_file(session_id) {
        let _ = std::fs::remove_file(path);
    }
}

fn get_or_create_id() -> Option<String> {
    let path = telemetry_id_path()?;
    if let Ok(id) = std::fs::read_to_string(&path) {
        let id = id.trim().to_string();
        if !id.is_empty() {
            return Some(id);
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    write_private_file(&path, &id);
    Some(id)
}

fn is_first_run() -> bool {
    telemetry_id_path().map(|p| !p.exists()).unwrap_or(false)
}

fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

fn install_recorded_for_id(id: &str) -> bool {
    install_recorded_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|stored| stored.trim() == id)
        .unwrap_or(false)
}

fn mark_install_recorded(id: &str) {
    if let Some(path) = install_recorded_path() {
        write_private_file(&path, id);
    }
}

fn previously_recorded_version() -> Option<String> {
    version_recorded_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn mark_current_version_recorded() {
    if let Some(path) = version_recorded_path() {
        write_private_file(&path, &version());
    }
}

fn new_event_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn build_channel() -> String {
    if std::env::var(crate::cli::selfdev::CLIENT_SELFDEV_ENV).is_ok() {
        return "selfdev".to_string();
    }
    if let Ok(exe) = std::env::current_exe() {
        let path = exe.to_string_lossy();
        if path.contains("/target/debug/") || path.contains("\\target\\debug\\") {
            return "debug".to_string();
        }
        if path.contains("/target/release/") || path.contains("\\target\\release\\") {
            return "local_build".to_string();
        }
    }
    if crate::build::get_repo_dir().is_some() {
        return "git_checkout".to_string();
    }
    "release".to_string()
}

fn is_git_checkout() -> bool {
    crate::build::get_repo_dir().is_some()
}

fn is_ci() -> bool {
    [
        "CI",
        "GITHUB_ACTIONS",
        "BUILDKITE",
        "JENKINS_URL",
        "GITLAB_CI",
        "CIRCLECI",
    ]
    .iter()
    .any(|key| std::env::var(key).is_ok())
}

fn ran_from_cargo() -> bool {
    std::env::var("CARGO").is_ok() || std::env::var("CARGO_MANIFEST_DIR").is_ok()
}

fn install_anchor_time(id: &str) -> Option<SystemTime> {
    install_recorded_path()
        .filter(|path| install_recorded_for_id(id) && path.exists())
        .and_then(|path| std::fs::metadata(path).ok())
        .and_then(|meta| meta.modified().ok())
        .or_else(|| {
            telemetry_id_path()
                .and_then(|path| std::fs::metadata(path).ok())
                .and_then(|meta| meta.modified().ok())
        })
}

fn elapsed_since_install_ms(id: &str) -> Option<u64> {
    let anchor = install_anchor_time(id)?;
    let elapsed = SystemTime::now().duration_since(anchor).ok()?;
    Some(elapsed.as_millis().min(u128::from(u64::MAX)) as u64)
}

fn days_since_install(id: &str) -> Option<u32> {
    let anchor = install_anchor_time(id)?;
    let elapsed = SystemTime::now().duration_since(anchor).ok()?;
    Some((elapsed.as_secs() / 86_400).min(u64::from(u32::MAX)) as u32)
}

fn milestone_recorded(id: &str, step: &str) -> bool {
    milestone_recorded_path(id, step)
        .map(|path| path.exists())
        .unwrap_or(false)
}

fn mark_milestone_recorded(id: &str, step: &str) {
    if let Some(path) = milestone_recorded_path(id, step) {
        write_private_file(&path, "1");
    }
}

fn current_session_id() -> Option<String> {
    SESSION_STATE
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|state| state.session_id.clone()))
}

fn telemetry_envelope() -> (u32, String, bool, bool, bool) {
    (
        TELEMETRY_SCHEMA_VERSION,
        build_channel(),
        is_git_checkout(),
        is_ci(),
        ran_from_cargo(),
    )
}

fn emit_onboarding_step_once(
    step: &'static str,
    auth_provider: Option<&str>,
    auth_method: Option<&str>,
) {
    if !is_enabled() {
        return;
    }
    let Some(id) = get_or_create_id() else {
        return;
    };
    if milestone_recorded(&id, step) {
        return;
    }
    let (schema_version, build_channel, git_checkout, ci, from_cargo) = telemetry_envelope();
    let event = OnboardingStepEvent {
        event_id: new_event_id(),
        id: id.clone(),
        session_id: current_session_id(),
        event: "onboarding_step",
        version: version(),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        step,
        auth_provider: auth_provider.map(sanitize_telemetry_label),
        auth_method: auth_method.map(sanitize_telemetry_label),
        milestone_elapsed_ms: elapsed_since_install_ms(&id),
        schema_version,
        build_channel,
        is_git_checkout: git_checkout,
        is_ci: ci,
        ran_from_cargo: from_cargo,
    };
    if let Ok(payload) = serde_json::to_value(&event)
        && send_payload(payload, DeliveryMode::Background)
    {
        mark_milestone_recorded(&id, step);
    }
}

pub fn record_feedback(rating: &str, reason: Option<&str>) {
    if !is_enabled() {
        return;
    }
    let Some(id) = get_or_create_id() else {
        return;
    };
    let normalized_rating = sanitize_telemetry_label(rating).to_ascii_lowercase();
    if normalized_rating.is_empty() {
        return;
    }
    let normalized_reason = reason
        .map(sanitize_telemetry_label)
        .filter(|value| !value.is_empty());
    let (schema_version, build_channel, git_checkout, ci, from_cargo) = telemetry_envelope();
    let event = FeedbackEvent {
        event_id: new_event_id(),
        id,
        session_id: current_session_id(),
        event: "feedback",
        version: version(),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        feedback_rating: normalized_rating,
        feedback_reason: normalized_reason,
        schema_version,
        build_channel,
        is_git_checkout: git_checkout,
        is_ci: ci,
        ran_from_cargo: from_cargo,
    };
    if let Ok(payload) = serde_json::to_value(&event) {
        let _ = send_payload(payload, DeliveryMode::Background);
    }
}

fn update_active_days(id: &str) -> (u32, u32) {
    let Some(path) = active_days_path(id) else {
        return (0, 0);
    };
    let today = Utc::now().date_naive();
    let mut days = std::fs::read_to_string(&path)
        .ok()
        .into_iter()
        .flat_map(|text| {
            text.lines()
                .map(str::trim)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter_map(|line| NaiveDate::parse_from_str(&line, "%Y-%m-%d").ok())
        .collect::<Vec<_>>();
    days.push(today);
    days.sort_unstable();
    days.dedup();
    let rendered = days
        .iter()
        .map(NaiveDate::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    write_private_file(&path, &rendered);
    let days_7 = days
        .iter()
        .filter(|day| (today.signed_duration_since(**day).num_days()) < 7)
        .count()
        .min(u32::MAX as usize) as u32;
    let days_30 = days
        .iter()
        .filter(|day| (today.signed_duration_since(**day).num_days()) < 30)
        .count()
        .min(u32::MAX as usize) as u32;
    (days_7, days_30)
}

fn detect_project_profile() -> ProjectProfile {
    fn keep_project_entry(entry: &walkdir::DirEntry) -> bool {
        if !entry.file_type().is_dir() {
            return true;
        }
        let name = entry.file_name().to_str().unwrap_or_default();
        !matches!(
            name,
            ".git" | "target" | "node_modules" | "dist" | "build" | ".next"
        )
    }

    let cwd = std::env::current_dir().ok();
    let mut profile = ProjectProfile::default();
    let Some(root) = cwd.as_deref() else {
        return profile;
    };
    profile.repo_present = root.join(".git").exists() || crate::build::is_jcode_repo(root);
    let mut scanned_files = 0usize;
    for entry in walkdir::WalkDir::new(root)
        .max_depth(3)
        .into_iter()
        .filter_entry(keep_project_entry)
        .filter_map(Result::ok)
    {
        if scanned_files >= 400 {
            break;
        }
        if entry.file_type().is_dir() {
            continue;
        }
        scanned_files += 1;
        match entry
            .path()
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default()
        {
            "rs" => profile.lang_rust = true,
            "js" | "jsx" | "ts" | "tsx" => profile.lang_js_ts = true,
            "py" => profile.lang_python = true,
            "go" => profile.lang_go = true,
            "md" | "mdx" => profile.lang_markdown = true,
            _ => {}
        }
    }
    profile
}

fn now_ms_since(started_at: Instant) -> u64 {
    started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

fn classify_tool_category(name: &str) -> ToolCategory {
    match name {
        "read"
        | "glob"
        | "grep"
        | "agentgrep"
        | "ls"
        | "conversation_search"
        | "session_search" => ToolCategory::ReadSearch,
        "write" | "edit" | "multiedit" | "patch" | "apply_patch" => ToolCategory::Write,
        "bash" | "bg" => ToolCategory::Shell,
        "webfetch" | "websearch" | "codesearch" | "open" => ToolCategory::Web,
        "memory" => ToolCategory::Memory,
        "subagent" => ToolCategory::Subagent,
        "communicate" => ToolCategory::Swarm,
        "gmail" => ToolCategory::Email,
        "side_panel" => ToolCategory::SidePanel,
        "goal" => ToolCategory::Goal,
        "mcp" => ToolCategory::Mcp,
        other if other.starts_with("mcp__") => ToolCategory::Mcp,
        _ => ToolCategory::Other,
    }
}

fn increment_tool_category(state: &mut SessionTelemetry, category: ToolCategory) {
    match category {
        ToolCategory::ReadSearch => state.tool_cat_read_search += 1,
        ToolCategory::Write => state.tool_cat_write += 1,
        ToolCategory::Shell => state.tool_cat_shell += 1,
        ToolCategory::Web => state.tool_cat_web += 1,
        ToolCategory::Memory => state.tool_cat_memory += 1,
        ToolCategory::Subagent => state.tool_cat_subagent += 1,
        ToolCategory::Swarm => state.tool_cat_swarm += 1,
        ToolCategory::Email => state.tool_cat_email += 1,
        ToolCategory::SidePanel => state.tool_cat_side_panel += 1,
        ToolCategory::Goal => state.tool_cat_goal += 1,
        ToolCategory::Mcp => state.tool_cat_mcp += 1,
        ToolCategory::Other => state.tool_cat_other += 1,
    }
}

fn increment_turn_tool_category(state: &mut TurnTelemetry, category: ToolCategory) {
    match category {
        ToolCategory::ReadSearch => state.tool_cat_read_search += 1,
        ToolCategory::Write => state.tool_cat_write += 1,
        ToolCategory::Shell => state.tool_cat_shell += 1,
        ToolCategory::Web => state.tool_cat_web += 1,
        ToolCategory::Memory => state.tool_cat_memory += 1,
        ToolCategory::Subagent => state.tool_cat_subagent += 1,
        ToolCategory::Swarm => state.tool_cat_swarm += 1,
        ToolCategory::Email => state.tool_cat_email += 1,
        ToolCategory::SidePanel => state.tool_cat_side_panel += 1,
        ToolCategory::Goal => state.tool_cat_goal += 1,
        ToolCategory::Mcp => state.tool_cat_mcp += 1,
        ToolCategory::Other => state.tool_cat_other += 1,
    }
}

fn observe_session_concurrency(state: &mut SessionTelemetry) {
    state.max_concurrent_sessions = state.max_concurrent_sessions.max(observe_active_sessions());
}

fn update_turn_activity_timestamp(turn: &mut TurnTelemetry, now: Instant) {
    if now >= turn.last_activity_at {
        turn.last_activity_at = now;
    }
}

fn mark_command_family_usage(state: &mut SessionTelemetry, command: &str) {
    let family = command
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_start_matches('/');
    match family {
        "login" | "auth" => state.command_login_used = true,
        "model" => state.command_model_used = true,
        "usage" => state.command_usage_used = true,
        "resume" | "session" | "back" | "catchup" => state.command_resume_used = true,
        "memory" => state.command_memory_used = true,
        "swarm" | "agents" => state.command_swarm_used = true,
        "goal" | "goals" => state.command_goal_used = true,
        "selfdev" | "dev" => state.command_selfdev_used = true,
        "feedback" => state.command_feedback_used = true,
        _ => state.command_other_used = true,
    }
}

fn mark_tool_feature_usage(state: &mut SessionTelemetry, name: &str, input: &Value) {
    let category = classify_tool_category(name);
    increment_tool_category(state, category);
    if let Some(turn) = state.current_turn.as_mut() {
        increment_turn_tool_category(turn, category);
    }

    match name {
        "memory" => {
            state.feature_memory_used = true;
            if let Some(turn) = state.current_turn.as_mut() {
                turn.feature_memory_used = true;
            }
        }
        "communicate" => {
            state.feature_swarm_used = true;
            if let Some(turn) = state.current_turn.as_mut() {
                turn.feature_swarm_used = true;
            }
        }
        "webfetch" | "websearch" | "codesearch" => {
            state.feature_web_used = true;
            if let Some(turn) = state.current_turn.as_mut() {
                turn.feature_web_used = true;
            }
        }
        "gmail" => {
            state.feature_email_used = true;
            if let Some(turn) = state.current_turn.as_mut() {
                turn.feature_email_used = true;
            }
        }
        "side_panel" => {
            state.feature_side_panel_used = true;
            if let Some(turn) = state.current_turn.as_mut() {
                turn.feature_side_panel_used = true;
            }
        }
        "goal" => {
            state.feature_goal_used = true;
            if let Some(turn) = state.current_turn.as_mut() {
                turn.feature_goal_used = true;
            }
        }
        "selfdev" => {
            state.feature_selfdev_used = true;
            if let Some(turn) = state.current_turn.as_mut() {
                turn.feature_selfdev_used = true;
            }
        }
        "bg" | "schedule" => {
            state.feature_background_used = true;
            if let Some(turn) = state.current_turn.as_mut() {
                turn.feature_background_used = true;
            }
        }
        "subagent" => {
            state.feature_subagent_used = true;
            if let Some(turn) = state.current_turn.as_mut() {
                turn.feature_subagent_used = true;
            }
        }
        _ => {}
    }

    if matches!(
        name,
        "write" | "edit" | "multiedit" | "patch" | "apply_patch"
    ) {
        state.file_write_calls += 1;
        if let Some(turn) = state.current_turn.as_mut() {
            turn.file_write_calls += 1;
        }
    }

    if name == "mcp" || name.starts_with("mcp__") {
        state.feature_mcp_used = true;
        if let Some(turn) = state.current_turn.as_mut() {
            turn.feature_mcp_used = true;
        }
        if let Some(server) = mcp_server_name(name, input) {
            state.unique_mcp_servers.insert(server);
            if let Some(turn) = state.current_turn.as_mut() {
                if let Some(server) = mcp_server_name(name, input) {
                    turn.unique_mcp_servers.insert(server);
                }
            }
        }
    }

    if looks_like_test_run(name, input) {
        state.tests_run += 1;
        if let Some(turn) = state.current_turn.as_mut() {
            turn.tests_run += 1;
        }
    }
}

fn mark_tool_success_side_effects(state: &mut SessionTelemetry, name: &str, input: &Value) {
    if looks_like_test_run(name, input) {
        state.tests_passed += 1;
        if state.first_test_pass_ms.is_none() {
            state.first_test_pass_ms = Some(now_ms_since(state.started_at));
        }
        if let Some(turn) = state.current_turn.as_mut() {
            turn.tests_passed += 1;
            if turn.first_test_pass_ms.is_none() {
                turn.first_test_pass_ms = Some(now_ms_since(turn.started_at));
            }
        }
    }

    if state.first_tool_success_ms.is_none() {
        state.first_tool_success_ms = Some(now_ms_since(state.started_at));
    }
    if let Some(turn) = state.current_turn.as_mut() {
        if turn.first_tool_success_ms.is_none() {
            turn.first_tool_success_ms = Some(now_ms_since(turn.started_at));
        }
    }

    if matches!(
        name,
        "write" | "edit" | "multiedit" | "patch" | "apply_patch"
    ) && state.first_file_edit_ms.is_none()
    {
        state.first_file_edit_ms = Some(now_ms_since(state.started_at));
    }
    if matches!(
        name,
        "write" | "edit" | "multiedit" | "patch" | "apply_patch"
    ) {
        if let Some(turn) = state.current_turn.as_mut() {
            if turn.first_file_edit_ms.is_none() {
                turn.first_file_edit_ms = Some(now_ms_since(turn.started_at));
            }
        }
    }

    if name == "memory" {
        state.feature_memory_used = true;
        if let Some(turn) = state.current_turn.as_mut() {
            turn.feature_memory_used = true;
        }
    }
}

pub fn record_command_family(command: &str) {
    if let Ok(mut guard) = SESSION_STATE.lock() {
        if let Some(ref mut state) = *guard {
            observe_session_concurrency(state);
            mark_command_family_usage(state, command);
            if let Some(turn) = state.current_turn.as_mut() {
                update_turn_activity_timestamp(turn, Instant::now());
            }
        }
    }
    maybe_emit_session_start();
}

fn looks_like_test_run(name: &str, input: &Value) -> bool {
    let mut haystacks = Vec::new();
    haystacks.push(name.to_ascii_lowercase());

    if let Some(command) = input.get("command").and_then(Value::as_str) {
        haystacks.push(command.to_ascii_lowercase());
    }
    if let Some(description) = input.get("description").and_then(Value::as_str) {
        haystacks.push(description.to_ascii_lowercase());
    }
    if let Some(task) = input.get("task").and_then(Value::as_str) {
        haystacks.push(task.to_ascii_lowercase());
    }

    haystacks.into_iter().any(|value| {
        value.contains("cargo test")
            || value.contains("npm test")
            || value.contains("pnpm test")
            || value.contains("pytest")
            || value.contains("jest")
            || value.contains("vitest")
            || value.contains("go test")
            || value.contains("rspec")
            || value.contains("bun test")
            || value.contains(" test")
    })
}

fn mcp_server_name(name: &str, input: &Value) -> Option<String> {
    if let Some(rest) = name.strip_prefix("mcp__") {
        return rest.split("__").next().map(|value| value.to_string());
    }
    if name == "mcp" {
        return input
            .get("server")
            .and_then(Value::as_str)
            .map(sanitize_telemetry_label)
            .filter(|value| !value.is_empty());
    }
    None
}

fn post_payload(payload: serde_json::Value, timeout: Duration) -> bool {
    let client = match reqwest::blocking::Client::builder()
        .timeout(timeout)
        .build()
    {
        Ok(client) => client,
        Err(_) => return false,
    };
    match client.post(TELEMETRY_ENDPOINT).json(&payload).send() {
        Ok(response) => response.error_for_status().is_ok(),
        Err(_) => false,
    }
}

fn send_payload(payload: serde_json::Value, mode: DeliveryMode) -> bool {
    match mode {
        DeliveryMode::Background => {
            std::thread::spawn(move || {
                let _ = post_payload(payload, ASYNC_SEND_TIMEOUT);
            });
            true
        }
        DeliveryMode::Blocking(timeout) => post_payload(payload, timeout),
    }
}

fn reset_counters() {
    ERROR_PROVIDER_TIMEOUT.store(0, Ordering::Relaxed);
    ERROR_AUTH_FAILED.store(0, Ordering::Relaxed);
    ERROR_TOOL_ERROR.store(0, Ordering::Relaxed);
    ERROR_MCP_ERROR.store(0, Ordering::Relaxed);
    ERROR_RATE_LIMITED.store(0, Ordering::Relaxed);
    PROVIDER_SWITCHES.store(0, Ordering::Relaxed);
    MODEL_SWITCHES.store(0, Ordering::Relaxed);
}

fn current_error_counts() -> ErrorCounts {
    ErrorCounts {
        provider_timeout: ERROR_PROVIDER_TIMEOUT.load(Ordering::Relaxed),
        auth_failed: ERROR_AUTH_FAILED.load(Ordering::Relaxed),
        tool_error: ERROR_TOOL_ERROR.load(Ordering::Relaxed),
        mcp_error: ERROR_MCP_ERROR.load(Ordering::Relaxed),
        rate_limited: ERROR_RATE_LIMITED.load(Ordering::Relaxed),
    }
}

fn sanitize_telemetry_label(value: &str) -> String {
    let mut cleaned = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if matches!(chars.peek(), Some('[')) {
                let _ = chars.next();
                while let Some(next) = chars.next() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
                continue;
            }
            continue;
        }
        if ch.is_control() {
            continue;
        }
        cleaned.push(ch);
    }
    cleaned.trim().to_string()
}

fn has_any_errors(errors: &ErrorCounts) -> bool {
    errors.provider_timeout > 0
        || errors.auth_failed > 0
        || errors.tool_error > 0
        || errors.mcp_error > 0
        || errors.rate_limited > 0
}

fn session_has_meaningful_activity(state: &SessionTelemetry, errors: &ErrorCounts) -> bool {
    state.had_user_prompt
        || state.had_assistant_response
        || state.assistant_responses > 0
        || state.tool_calls > 0
        || state.tool_failures > 0
        || state.executed_tool_calls > 0
        || state.feature_memory_used
        || state.feature_swarm_used
        || state.feature_web_used
        || state.feature_email_used
        || state.feature_mcp_used
        || state.feature_side_panel_used
        || state.feature_goal_used
        || state.feature_selfdev_used
        || state.feature_background_used
        || state.feature_subagent_used
        || PROVIDER_SWITCHES.load(Ordering::Relaxed) > 0
        || MODEL_SWITCHES.load(Ordering::Relaxed) > 0
        || has_any_errors(errors)
}

fn emit_turn_end_event(event: TurnEndEvent, mode: DeliveryMode) -> bool {
    if let Ok(payload) = serde_json::to_value(&event) {
        return send_payload(payload, mode);
    }
    false
}

fn finalize_current_turn(
    id: &str,
    state: &mut SessionTelemetry,
    now: Instant,
    end_reason: &'static str,
    mode: DeliveryMode,
) {
    let Some(turn) = state.current_turn.take() else {
        return;
    };
    let idle_after_turn_ms = now
        .checked_duration_since(turn.last_activity_at)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0);
    let turn_active_duration_ms = turn
        .last_activity_at
        .checked_duration_since(turn.started_at)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0);
    let turn_success = turn.assistant_responses > 0
        || turn.executed_tool_successes > 0
        || turn.tests_passed > 0
        || turn.file_write_calls > 0;
    let turn_abandoned =
        !turn_success && turn.tool_failures == 0 && turn.executed_tool_failures == 0;
    let (
        workflow_chat_only,
        workflow_coding_used,
        workflow_research_used,
        workflow_tests_used,
        workflow_background_used,
        workflow_subagent_used,
        workflow_swarm_used,
    ) = workflow_flags_from_counts(
        true,
        turn.file_write_calls,
        turn.tests_run,
        turn.tests_passed,
        turn.feature_web_used,
        turn.feature_background_used,
        turn.feature_subagent_used,
        turn.feature_swarm_used,
        turn.tool_cat_write,
        turn.tool_cat_web,
        turn.tool_cat_subagent,
        turn.tool_cat_swarm,
    );
    let (schema_version, build_channel, git_checkout, ci, from_cargo) = telemetry_envelope();
    let event = TurnEndEvent {
        event_id: new_event_id(),
        id: id.to_string(),
        session_id: state.session_id.clone(),
        event: "turn_end",
        version: version(),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        turn_index: turn.turn_index,
        turn_started_ms: turn.started_ms_since_session,
        turn_active_duration_ms,
        idle_before_turn_ms: turn.idle_before_turn_ms,
        idle_after_turn_ms,
        assistant_responses: turn.assistant_responses,
        first_assistant_response_ms: turn.first_assistant_response_ms,
        first_tool_call_ms: turn.first_tool_call_ms,
        first_tool_success_ms: turn.first_tool_success_ms,
        first_file_edit_ms: turn.first_file_edit_ms,
        first_test_pass_ms: turn.first_test_pass_ms,
        tool_calls: turn.tool_calls,
        tool_failures: turn.tool_failures,
        executed_tool_calls: turn.executed_tool_calls,
        executed_tool_successes: turn.executed_tool_successes,
        executed_tool_failures: turn.executed_tool_failures,
        tool_latency_total_ms: turn.tool_latency_total_ms,
        tool_latency_max_ms: turn.tool_latency_max_ms,
        file_write_calls: turn.file_write_calls,
        tests_run: turn.tests_run,
        tests_passed: turn.tests_passed,
        feature_memory_used: turn.feature_memory_used,
        feature_swarm_used: turn.feature_swarm_used,
        feature_web_used: turn.feature_web_used,
        feature_email_used: turn.feature_email_used,
        feature_mcp_used: turn.feature_mcp_used,
        feature_side_panel_used: turn.feature_side_panel_used,
        feature_goal_used: turn.feature_goal_used,
        feature_selfdev_used: turn.feature_selfdev_used,
        feature_background_used: turn.feature_background_used,
        feature_subagent_used: turn.feature_subagent_used,
        unique_mcp_servers: turn.unique_mcp_servers.len() as u32,
        tool_cat_read_search: turn.tool_cat_read_search,
        tool_cat_write: turn.tool_cat_write,
        tool_cat_shell: turn.tool_cat_shell,
        tool_cat_web: turn.tool_cat_web,
        tool_cat_memory: turn.tool_cat_memory,
        tool_cat_subagent: turn.tool_cat_subagent,
        tool_cat_swarm: turn.tool_cat_swarm,
        tool_cat_email: turn.tool_cat_email,
        tool_cat_side_panel: turn.tool_cat_side_panel,
        tool_cat_goal: turn.tool_cat_goal,
        tool_cat_mcp: turn.tool_cat_mcp,
        tool_cat_other: turn.tool_cat_other,
        workflow_chat_only,
        workflow_coding_used,
        workflow_research_used,
        workflow_tests_used,
        workflow_background_used,
        workflow_subagent_used,
        workflow_swarm_used,
        turn_success,
        turn_abandoned,
        turn_end_reason: end_reason,
        schema_version,
        build_channel,
        is_git_checkout: git_checkout,
        is_ci: ci,
        ran_from_cargo: from_cargo,
    };
    let _ = emit_turn_end_event(event, mode);
}

fn maybe_emit_session_start() {
    if !is_enabled() {
        return;
    }
    let event = {
        let mut guard = match SESSION_STATE.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let state = match guard.as_mut() {
            Some(state) => state,
            None => return,
        };
        if state.start_event_sent {
            return;
        }
        state.start_event_sent = true;
        observe_session_concurrency(state);
        let (schema_version, build_channel, git_checkout, ci, from_cargo) = telemetry_envelope();
        SessionStartEvent {
            event_id: new_event_id(),
            id: match get_or_create_id() {
                Some(id) => id,
                None => return,
            },
            session_id: state.session_id.clone(),
            event: "session_start",
            version: version(),
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            provider_start: state.provider_start.clone(),
            model_start: state.model_start.clone(),
            resumed_session: state.resumed_session,
            session_start_hour_utc: utc_hour(state.started_at_utc),
            session_start_weekday_utc: utc_weekday(state.started_at_utc),
            previous_session_gap_secs: state.previous_session_gap_secs,
            sessions_started_24h: state.sessions_started_24h,
            sessions_started_7d: state.sessions_started_7d,
            active_sessions_at_start: state.active_sessions_at_start,
            other_active_sessions_at_start: state.other_active_sessions_at_start,
            schema_version,
            build_channel,
            is_git_checkout: git_checkout,
            is_ci: ci,
            ran_from_cargo: from_cargo,
        }
    };
    if let Ok(payload) = serde_json::to_value(&event) {
        let _ = send_payload(payload, DeliveryMode::Background);
    }
}

fn emit_session_start_for_state(id: String, state: &SessionTelemetry, mode: DeliveryMode) -> bool {
    let (schema_version, build_channel, git_checkout, ci, from_cargo) = telemetry_envelope();
    let event = SessionStartEvent {
        event_id: new_event_id(),
        id,
        session_id: state.session_id.clone(),
        event: "session_start",
        version: version(),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        provider_start: state.provider_start.clone(),
        model_start: state.model_start.clone(),
        resumed_session: state.resumed_session,
        session_start_hour_utc: utc_hour(state.started_at_utc),
        session_start_weekday_utc: utc_weekday(state.started_at_utc),
        previous_session_gap_secs: state.previous_session_gap_secs,
        sessions_started_24h: state.sessions_started_24h,
        sessions_started_7d: state.sessions_started_7d,
        active_sessions_at_start: state.active_sessions_at_start,
        other_active_sessions_at_start: state.other_active_sessions_at_start,
        schema_version,
        build_channel,
        is_git_checkout: git_checkout,
        is_ci: ci,
        ran_from_cargo: from_cargo,
    };
    if let Ok(payload) = serde_json::to_value(&event) {
        return send_payload(payload, mode);
    }
    false
}

pub fn record_install_if_first_run() {
    if !is_enabled() {
        return;
    }
    let first_run = is_first_run();
    let id = match get_or_create_id() {
        Some(id) => id,
        None => return,
    };
    if install_recorded_for_id(&id) {
        return;
    }
    let (schema_version, build_channel, git_checkout, ci, from_cargo) = telemetry_envelope();
    let event = InstallEvent {
        event_id: new_event_id(),
        id: id.clone(),
        event: "install",
        version: version(),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        schema_version,
        build_channel,
        is_git_checkout: git_checkout,
        is_ci: ci,
        ran_from_cargo: from_cargo,
    };
    if let Ok(payload) = serde_json::to_value(&event) {
        if send_payload(payload, DeliveryMode::Blocking(BLOCKING_INSTALL_TIMEOUT)) {
            mark_install_recorded(&id);
        }
    }
    if first_run {
        emit_onboarding_step_once("first_run", None, None);
        show_first_run_notice();
    }
    mark_current_version_recorded();
}

pub fn record_upgrade_if_needed() {
    if !is_enabled() {
        return;
    }
    let current = version();
    let Some(previous) = previously_recorded_version() else {
        mark_current_version_recorded();
        return;
    };
    if previous == current {
        return;
    }
    let Some(id) = get_or_create_id() else {
        return;
    };
    let (schema_version, build_channel, git_checkout, ci, from_cargo) = telemetry_envelope();
    let event = UpgradeEvent {
        event_id: new_event_id(),
        id,
        event: "upgrade",
        version: current,
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        from_version: previous,
        schema_version,
        build_channel,
        is_git_checkout: git_checkout,
        is_ci: ci,
        ran_from_cargo: from_cargo,
    };
    if let Ok(payload) = serde_json::to_value(&event) {
        let _ = send_payload(payload, DeliveryMode::Background);
    }
    mark_current_version_recorded();
}

pub fn record_provider_selected(provider: &str) {
    emit_onboarding_step_once("provider_selected", Some(provider), None);
}

pub fn record_auth_started(provider: &str, method: &str) {
    emit_onboarding_step_once("auth_started", Some(provider), Some(method));
}

pub fn record_auth_success(provider: &str, method: &str) {
    if !is_enabled() {
        return;
    }
    let Some(id) = get_or_create_id() else {
        return;
    };
    let (schema_version, build_channel, git_checkout, ci, from_cargo) = telemetry_envelope();
    let event = AuthEvent {
        event_id: new_event_id(),
        id,
        event: "auth_success",
        version: version(),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        auth_provider: sanitize_telemetry_label(provider),
        auth_method: sanitize_telemetry_label(method),
        schema_version,
        build_channel,
        is_git_checkout: git_checkout,
        is_ci: ci,
        ran_from_cargo: from_cargo,
    };
    if let Ok(payload) = serde_json::to_value(&event) {
        let _ = send_payload(payload, DeliveryMode::Background);
    }
    emit_onboarding_step_once("auth_success", Some(provider), Some(method));
}

pub fn begin_session(provider: &str, model: &str) {
    begin_session_with_mode(provider, model, false);
}

pub fn begin_resumed_session(provider: &str, model: &str) {
    begin_session_with_mode(provider, model, true);
}

fn begin_session_with_mode(provider: &str, model: &str, resumed_session: bool) {
    if !is_enabled() {
        return;
    }
    let started_at = Instant::now();
    let started_at_utc = Utc::now();
    let session_id = uuid::Uuid::new_v4().to_string();
    let (previous_session_gap_secs, sessions_started_24h, sessions_started_7d) = get_or_create_id()
        .map(|id| update_session_start_history(&id, started_at_utc))
        .unwrap_or((None, 0, 0));
    let (active_sessions_at_start, other_active_sessions_at_start) =
        register_active_session(&session_id);
    let state = SessionTelemetry {
        session_id,
        started_at,
        started_at_utc,
        provider_start: sanitize_telemetry_label(provider),
        model_start: sanitize_telemetry_label(model),
        turns: 0,
        had_user_prompt: false,
        had_assistant_response: false,
        assistant_responses: 0,
        first_assistant_response_ms: None,
        first_tool_call_ms: None,
        first_tool_success_ms: None,
        first_file_edit_ms: None,
        first_test_pass_ms: None,
        tool_calls: 0,
        tool_failures: 0,
        executed_tool_calls: 0,
        executed_tool_successes: 0,
        executed_tool_failures: 0,
        tool_latency_total_ms: 0,
        tool_latency_max_ms: 0,
        file_write_calls: 0,
        tests_run: 0,
        tests_passed: 0,
        feature_memory_used: false,
        feature_swarm_used: false,
        feature_web_used: false,
        feature_email_used: false,
        feature_mcp_used: false,
        feature_side_panel_used: false,
        feature_goal_used: false,
        feature_selfdev_used: false,
        feature_background_used: false,
        feature_subagent_used: false,
        unique_mcp_servers: HashSet::new(),
        transport_https: 0,
        transport_persistent_ws_fresh: 0,
        transport_persistent_ws_reuse: 0,
        transport_cli_subprocess: 0,
        transport_native_http2: 0,
        transport_other: 0,
        tool_cat_read_search: 0,
        tool_cat_write: 0,
        tool_cat_shell: 0,
        tool_cat_web: 0,
        tool_cat_memory: 0,
        tool_cat_subagent: 0,
        tool_cat_swarm: 0,
        tool_cat_email: 0,
        tool_cat_side_panel: 0,
        tool_cat_goal: 0,
        tool_cat_mcp: 0,
        tool_cat_other: 0,
        command_login_used: false,
        command_model_used: false,
        command_usage_used: false,
        command_resume_used: false,
        command_memory_used: false,
        command_swarm_used: false,
        command_goal_used: false,
        command_selfdev_used: false,
        command_feedback_used: false,
        command_other_used: false,
        previous_session_gap_secs,
        sessions_started_24h,
        sessions_started_7d,
        active_sessions_at_start,
        other_active_sessions_at_start,
        max_concurrent_sessions: active_sessions_at_start,
        current_turn: None,
        resumed_session,
        start_event_sent: false,
    };
    if let Ok(mut guard) = SESSION_STATE.lock() {
        *guard = Some(state);
    }
    reset_counters();
}

pub fn record_turn() {
    let id = get_or_create_id();
    if let Ok(mut guard) = SESSION_STATE.lock() {
        if let Some(ref mut state) = *guard {
            observe_session_concurrency(state);
            let now = Instant::now();
            let previous_last_activity = state
                .current_turn
                .as_ref()
                .map(|turn| turn.last_activity_at);
            if let Some(ref id) = id {
                finalize_current_turn(id, state, now, "next_user_prompt", DeliveryMode::Background);
            }
            state.turns += 1;
            state.had_user_prompt = true;
            let idle_before_turn_ms = previous_last_activity.and_then(|last| {
                now.checked_duration_since(last)
                    .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
            });
            state.current_turn = Some(TurnTelemetry::new(
                state.turns,
                now,
                now_ms_since(state.started_at),
                idle_before_turn_ms,
            ));
        }
    }
    emit_onboarding_step_once("first_prompt_sent", None, None);
    maybe_emit_session_start();
}

pub fn record_assistant_response() {
    if let Ok(mut guard) = SESSION_STATE.lock() {
        if let Some(ref mut state) = *guard {
            observe_session_concurrency(state);
            let now = Instant::now();
            if state.first_assistant_response_ms.is_none() {
                state.first_assistant_response_ms = Some(now_ms_since(state.started_at));
            }
            state.had_assistant_response = true;
            state.assistant_responses += 1;
            if let Some(turn) = state.current_turn.as_mut() {
                if turn.first_assistant_response_ms.is_none() {
                    turn.first_assistant_response_ms = Some(now_ms_since(turn.started_at));
                }
                turn.assistant_responses += 1;
                update_turn_activity_timestamp(turn, now);
            }
        }
    }
    emit_onboarding_step_once("first_assistant_response", None, None);
    maybe_emit_session_start();
}

pub fn record_memory_injected(_count: usize, _age_ms: u64) {
    if let Ok(mut guard) = SESSION_STATE.lock() {
        if let Some(ref mut state) = *guard {
            observe_session_concurrency(state);
            state.feature_memory_used = true;
            if let Some(turn) = state.current_turn.as_mut() {
                turn.feature_memory_used = true;
                update_turn_activity_timestamp(turn, Instant::now());
            }
        }
    }
    maybe_emit_session_start();
}

pub fn record_tool_call() {
    if let Ok(mut guard) = SESSION_STATE.lock() {
        if let Some(ref mut state) = *guard {
            observe_session_concurrency(state);
            let now = Instant::now();
            state.tool_calls += 1;
            if state.first_tool_call_ms.is_none() {
                state.first_tool_call_ms = Some(now_ms_since(state.started_at));
            }
            if let Some(turn) = state.current_turn.as_mut() {
                turn.tool_calls += 1;
                if turn.first_tool_call_ms.is_none() {
                    turn.first_tool_call_ms = Some(now_ms_since(turn.started_at));
                }
                update_turn_activity_timestamp(turn, now);
            }
        }
    }
    maybe_emit_session_start();
}

pub fn record_tool_failure() {
    if let Ok(mut guard) = SESSION_STATE.lock() {
        if let Some(ref mut state) = *guard {
            observe_session_concurrency(state);
            state.tool_failures += 1;
            if let Some(turn) = state.current_turn.as_mut() {
                turn.tool_failures += 1;
                update_turn_activity_timestamp(turn, Instant::now());
            }
        }
    }
    maybe_emit_session_start();
}

pub fn record_connection_type(connection: &str) {
    if let Ok(mut guard) = SESSION_STATE.lock() {
        if let Some(ref mut state) = *guard {
            observe_session_concurrency(state);
            let normalized = sanitize_telemetry_label(connection).to_ascii_lowercase();
            if normalized.contains("websocket/persistent-reuse") {
                state.transport_persistent_ws_reuse += 1;
            } else if normalized.contains("websocket/persistent-fresh")
                || normalized.contains("websocket/persistent")
            {
                state.transport_persistent_ws_fresh += 1;
            } else if normalized.contains("native http2") {
                state.transport_native_http2 += 1;
            } else if normalized.contains("cli subprocess") {
                state.transport_cli_subprocess += 1;
            } else if normalized.starts_with("https") {
                state.transport_https += 1;
            } else {
                state.transport_other += 1;
            }
            if let Some(turn) = state.current_turn.as_mut() {
                update_turn_activity_timestamp(turn, Instant::now());
            }
        }
    }
    maybe_emit_session_start();
}

pub fn record_error(category: ErrorCategory) {
    if let Ok(mut guard) = SESSION_STATE.lock() {
        if let Some(ref mut state) = *guard {
            observe_session_concurrency(state);
            if let Some(turn) = state.current_turn.as_mut() {
                update_turn_activity_timestamp(turn, Instant::now());
            }
        }
    }
    match category {
        ErrorCategory::ProviderTimeout => {
            ERROR_PROVIDER_TIMEOUT.fetch_add(1, Ordering::Relaxed);
        }
        ErrorCategory::AuthFailed => {
            ERROR_AUTH_FAILED.fetch_add(1, Ordering::Relaxed);
        }
        ErrorCategory::ToolError => {
            ERROR_TOOL_ERROR.fetch_add(1, Ordering::Relaxed);
        }
        ErrorCategory::McpError => {
            ERROR_MCP_ERROR.fetch_add(1, Ordering::Relaxed);
        }
        ErrorCategory::RateLimited => {
            ERROR_RATE_LIMITED.fetch_add(1, Ordering::Relaxed);
        }
    }
    maybe_emit_session_start();
}

pub fn record_provider_switch() {
    if let Ok(mut guard) = SESSION_STATE.lock() {
        if let Some(ref mut state) = *guard {
            observe_session_concurrency(state);
            if let Some(turn) = state.current_turn.as_mut() {
                update_turn_activity_timestamp(turn, Instant::now());
            }
        }
    }
    PROVIDER_SWITCHES.fetch_add(1, Ordering::Relaxed);
    maybe_emit_session_start();
}

pub fn record_model_switch() {
    if let Ok(mut guard) = SESSION_STATE.lock() {
        if let Some(ref mut state) = *guard {
            observe_session_concurrency(state);
            if let Some(turn) = state.current_turn.as_mut() {
                update_turn_activity_timestamp(turn, Instant::now());
            }
        }
    }
    MODEL_SWITCHES.fetch_add(1, Ordering::Relaxed);
    maybe_emit_session_start();
}

pub fn record_tool_execution(name: &str, input: &Value, succeeded: bool, latency_ms: u64) {
    if let Ok(mut guard) = SESSION_STATE.lock() {
        if let Some(ref mut state) = *guard {
            observe_session_concurrency(state);
            let now = Instant::now();
            state.executed_tool_calls += 1;
            state.tool_latency_total_ms = state.tool_latency_total_ms.saturating_add(latency_ms);
            state.tool_latency_max_ms = state.tool_latency_max_ms.max(latency_ms);
            if let Some(turn) = state.current_turn.as_mut() {
                turn.executed_tool_calls += 1;
                turn.tool_latency_total_ms = turn.tool_latency_total_ms.saturating_add(latency_ms);
                turn.tool_latency_max_ms = turn.tool_latency_max_ms.max(latency_ms);
                update_turn_activity_timestamp(turn, now);
            }
            mark_tool_feature_usage(state, name, input);
            if succeeded {
                state.executed_tool_successes += 1;
                if let Some(turn) = state.current_turn.as_mut() {
                    turn.executed_tool_successes += 1;
                }
                mark_tool_success_side_effects(state, name, input);
            } else {
                state.executed_tool_failures += 1;
                if let Some(turn) = state.current_turn.as_mut() {
                    turn.executed_tool_failures += 1;
                }
            }
        }
    }
    if succeeded {
        emit_onboarding_step_once("first_successful_tool", None, None);
        if matches!(
            name,
            "write" | "edit" | "multiedit" | "patch" | "apply_patch"
        ) {
            emit_onboarding_step_once("first_file_edit", None, None);
        }
    }
    maybe_emit_session_start();
}

pub fn end_session(provider_end: &str, model_end: &str) {
    end_session_with_reason(provider_end, model_end, SessionEndReason::NormalExit);
}

pub fn end_session_with_reason(provider_end: &str, model_end: &str, reason: SessionEndReason) {
    emit_lifecycle_event("session_end", provider_end, model_end, reason, true);
}

pub fn record_crash(provider_end: &str, model_end: &str, reason: SessionEndReason) {
    emit_lifecycle_event("session_crash", provider_end, model_end, reason, true);
}

pub fn current_provider_model() -> Option<(String, String)> {
    SESSION_STATE.lock().ok().and_then(|guard| {
        guard
            .as_ref()
            .map(|state| (state.provider_start.clone(), state.model_start.clone()))
    })
}

fn emit_lifecycle_event(
    event_name: &'static str,
    provider_end: &str,
    model_end: &str,
    reason: SessionEndReason,
    clear_state: bool,
) {
    if !is_enabled() {
        return;
    }
    let id = match get_or_create_id() {
        Some(id) => id,
        None => return,
    };
    let state = {
        let mut guard = match SESSION_STATE.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let now = Instant::now();
        if let Some(active) = guard.as_mut() {
            finalize_current_turn(&id, active, now, reason.as_str(), DeliveryMode::Background);
            observe_session_concurrency(active);
        }
        let state = match guard.as_ref() {
            Some(s) => SessionTelemetry {
                session_id: s.session_id.clone(),
                started_at: s.started_at,
                started_at_utc: s.started_at_utc,
                provider_start: s.provider_start.clone(),
                model_start: s.model_start.clone(),
                turns: s.turns,
                had_user_prompt: s.had_user_prompt,
                had_assistant_response: s.had_assistant_response,
                assistant_responses: s.assistant_responses,
                first_assistant_response_ms: s.first_assistant_response_ms,
                first_tool_call_ms: s.first_tool_call_ms,
                first_tool_success_ms: s.first_tool_success_ms,
                first_file_edit_ms: s.first_file_edit_ms,
                first_test_pass_ms: s.first_test_pass_ms,
                tool_calls: s.tool_calls,
                tool_failures: s.tool_failures,
                executed_tool_calls: s.executed_tool_calls,
                executed_tool_successes: s.executed_tool_successes,
                executed_tool_failures: s.executed_tool_failures,
                tool_latency_total_ms: s.tool_latency_total_ms,
                tool_latency_max_ms: s.tool_latency_max_ms,
                file_write_calls: s.file_write_calls,
                tests_run: s.tests_run,
                tests_passed: s.tests_passed,
                feature_memory_used: s.feature_memory_used,
                feature_swarm_used: s.feature_swarm_used,
                feature_web_used: s.feature_web_used,
                feature_email_used: s.feature_email_used,
                feature_mcp_used: s.feature_mcp_used,
                feature_side_panel_used: s.feature_side_panel_used,
                feature_goal_used: s.feature_goal_used,
                feature_selfdev_used: s.feature_selfdev_used,
                feature_background_used: s.feature_background_used,
                feature_subagent_used: s.feature_subagent_used,
                unique_mcp_servers: s.unique_mcp_servers.clone(),
                transport_https: s.transport_https,
                transport_persistent_ws_fresh: s.transport_persistent_ws_fresh,
                transport_persistent_ws_reuse: s.transport_persistent_ws_reuse,
                transport_cli_subprocess: s.transport_cli_subprocess,
                transport_native_http2: s.transport_native_http2,
                transport_other: s.transport_other,
                tool_cat_read_search: s.tool_cat_read_search,
                tool_cat_write: s.tool_cat_write,
                tool_cat_shell: s.tool_cat_shell,
                tool_cat_web: s.tool_cat_web,
                tool_cat_memory: s.tool_cat_memory,
                tool_cat_subagent: s.tool_cat_subagent,
                tool_cat_swarm: s.tool_cat_swarm,
                tool_cat_email: s.tool_cat_email,
                tool_cat_side_panel: s.tool_cat_side_panel,
                tool_cat_goal: s.tool_cat_goal,
                tool_cat_mcp: s.tool_cat_mcp,
                tool_cat_other: s.tool_cat_other,
                command_login_used: s.command_login_used,
                command_model_used: s.command_model_used,
                command_usage_used: s.command_usage_used,
                command_resume_used: s.command_resume_used,
                command_memory_used: s.command_memory_used,
                command_swarm_used: s.command_swarm_used,
                command_goal_used: s.command_goal_used,
                command_selfdev_used: s.command_selfdev_used,
                command_feedback_used: s.command_feedback_used,
                command_other_used: s.command_other_used,
                previous_session_gap_secs: s.previous_session_gap_secs,
                sessions_started_24h: s.sessions_started_24h,
                sessions_started_7d: s.sessions_started_7d,
                active_sessions_at_start: s.active_sessions_at_start,
                other_active_sessions_at_start: s.other_active_sessions_at_start,
                max_concurrent_sessions: s.max_concurrent_sessions,
                current_turn: None,
                resumed_session: s.resumed_session,
                start_event_sent: s.start_event_sent,
            },
            None => return,
        };
        if clear_state {
            *guard = None;
        }
        state
    };
    let errors = current_error_counts();
    if !session_has_meaningful_activity(&state, &errors) {
        reset_counters();
        return;
    }
    if !state.start_event_sent {
        let _ = emit_session_start_for_state(
            id.clone(),
            &state,
            DeliveryMode::Blocking(BLOCKING_LIFECYCLE_TIMEOUT),
        );
    }
    let duration = state.started_at.elapsed();
    let session_success = state.had_assistant_response
        || state.executed_tool_successes > 0
        || state.tests_passed > 0
        || state.file_write_calls > 0;
    let abandoned_before_response = state.had_user_prompt
        && !state.had_assistant_response
        && state.executed_tool_successes == 0;
    let workflow_coding_used = state.file_write_calls > 0 || state.tool_cat_write > 0;
    let workflow_research_used = state.feature_web_used || state.tool_cat_web > 0;
    let workflow_tests_used = state.tests_run > 0 || state.tests_passed > 0;
    let workflow_background_used = state.feature_background_used;
    let workflow_subagent_used = state.feature_subagent_used || state.tool_cat_subagent > 0;
    let workflow_swarm_used = state.feature_swarm_used || state.tool_cat_swarm > 0;
    let workflow_chat_only = state.had_user_prompt
        && !workflow_coding_used
        && !workflow_research_used
        && !workflow_tests_used
        && !workflow_background_used
        && !workflow_subagent_used
        && !workflow_swarm_used;
    let project_profile = detect_project_profile();
    let (active_days_7d, active_days_30d) = update_active_days(&id);
    let days_since_install = days_since_install(&id);
    let ended_at_utc = Utc::now();
    let (schema_version, build_channel, git_checkout, ci, from_cargo) = telemetry_envelope();
    let event = SessionLifecycleEvent {
        event_id: new_event_id(),
        id,
        session_id: state.session_id.clone(),
        event: event_name,
        version: version(),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        provider_start: state.provider_start,
        provider_end: sanitize_telemetry_label(provider_end),
        model_start: state.model_start,
        model_end: sanitize_telemetry_label(model_end),
        provider_switches: PROVIDER_SWITCHES.load(Ordering::Relaxed),
        model_switches: MODEL_SWITCHES.load(Ordering::Relaxed),
        duration_mins: duration.as_secs() / 60,
        duration_secs: duration.as_secs(),
        turns: state.turns,
        had_user_prompt: state.had_user_prompt,
        had_assistant_response: state.had_assistant_response,
        assistant_responses: state.assistant_responses,
        first_assistant_response_ms: state.first_assistant_response_ms,
        first_tool_call_ms: state.first_tool_call_ms,
        first_tool_success_ms: state.first_tool_success_ms,
        first_file_edit_ms: state.first_file_edit_ms,
        first_test_pass_ms: state.first_test_pass_ms,
        tool_calls: state.tool_calls,
        tool_failures: state.tool_failures,
        executed_tool_calls: state.executed_tool_calls,
        executed_tool_successes: state.executed_tool_successes,
        executed_tool_failures: state.executed_tool_failures,
        tool_latency_total_ms: state.tool_latency_total_ms,
        tool_latency_max_ms: state.tool_latency_max_ms,
        file_write_calls: state.file_write_calls,
        tests_run: state.tests_run,
        tests_passed: state.tests_passed,
        feature_memory_used: state.feature_memory_used,
        feature_swarm_used: state.feature_swarm_used,
        feature_web_used: state.feature_web_used,
        feature_email_used: state.feature_email_used,
        feature_mcp_used: state.feature_mcp_used,
        feature_side_panel_used: state.feature_side_panel_used,
        feature_goal_used: state.feature_goal_used,
        feature_selfdev_used: state.feature_selfdev_used,
        feature_background_used: state.feature_background_used,
        feature_subagent_used: state.feature_subagent_used,
        unique_mcp_servers: state.unique_mcp_servers.len() as u32,
        session_success,
        abandoned_before_response,
        transport_https: state.transport_https,
        transport_persistent_ws_fresh: state.transport_persistent_ws_fresh,
        transport_persistent_ws_reuse: state.transport_persistent_ws_reuse,
        transport_cli_subprocess: state.transport_cli_subprocess,
        transport_native_http2: state.transport_native_http2,
        transport_other: state.transport_other,
        tool_cat_read_search: state.tool_cat_read_search,
        tool_cat_write: state.tool_cat_write,
        tool_cat_shell: state.tool_cat_shell,
        tool_cat_web: state.tool_cat_web,
        tool_cat_memory: state.tool_cat_memory,
        tool_cat_subagent: state.tool_cat_subagent,
        tool_cat_swarm: state.tool_cat_swarm,
        tool_cat_email: state.tool_cat_email,
        tool_cat_side_panel: state.tool_cat_side_panel,
        tool_cat_goal: state.tool_cat_goal,
        tool_cat_mcp: state.tool_cat_mcp,
        tool_cat_other: state.tool_cat_other,
        command_login_used: state.command_login_used,
        command_model_used: state.command_model_used,
        command_usage_used: state.command_usage_used,
        command_resume_used: state.command_resume_used,
        command_memory_used: state.command_memory_used,
        command_swarm_used: state.command_swarm_used,
        command_goal_used: state.command_goal_used,
        command_selfdev_used: state.command_selfdev_used,
        command_feedback_used: state.command_feedback_used,
        command_other_used: state.command_other_used,
        workflow_chat_only,
        workflow_coding_used,
        workflow_research_used,
        workflow_tests_used,
        workflow_background_used,
        workflow_subagent_used,
        workflow_swarm_used,
        project_repo_present: project_profile.repo_present,
        project_lang_rust: project_profile.lang_rust,
        project_lang_js_ts: project_profile.lang_js_ts,
        project_lang_python: project_profile.lang_python,
        project_lang_go: project_profile.lang_go,
        project_lang_markdown: project_profile.lang_markdown,
        project_lang_mixed: project_profile.mixed(),
        days_since_install,
        active_days_7d,
        active_days_30d,
        session_start_hour_utc: utc_hour(state.started_at_utc),
        session_start_weekday_utc: utc_weekday(state.started_at_utc),
        session_end_hour_utc: utc_hour(ended_at_utc),
        session_end_weekday_utc: utc_weekday(ended_at_utc),
        previous_session_gap_secs: state.previous_session_gap_secs,
        sessions_started_24h: state.sessions_started_24h,
        sessions_started_7d: state.sessions_started_7d,
        active_sessions_at_start: state.active_sessions_at_start,
        other_active_sessions_at_start: state.other_active_sessions_at_start,
        max_concurrent_sessions: state.max_concurrent_sessions,
        multi_sessioned: state.max_concurrent_sessions > 1
            || state.other_active_sessions_at_start > 0,
        resumed_session: state.resumed_session,
        end_reason: reason.as_str(),
        schema_version,
        build_channel,
        is_git_checkout: git_checkout,
        is_ci: ci,
        ran_from_cargo: from_cargo,
        errors,
    };
    if let Ok(payload) = serde_json::to_value(&event) {
        let _ = send_payload(payload, DeliveryMode::Blocking(BLOCKING_LIFECYCLE_TIMEOUT));
    }
    unregister_active_session(&state.session_id);
    if session_success {
        emit_onboarding_step_once("first_session_success", None, None);
    }
    reset_counters();
}

#[derive(Debug, Clone, Copy)]
pub enum ErrorCategory {
    ProviderTimeout,
    AuthFailed,
    ToolError,
    McpError,
    RateLimited,
}

fn show_first_run_notice() {
    eprintln!("\x1b[90m");
    eprintln!("  jcode collects anonymous usage statistics (install count, version, OS,");
    eprintln!("  session activity, tool counts, and crash/exit reasons). No code, filenames,");
    eprintln!("  prompts, or personal data is sent.");
    eprintln!("  To opt out: export JCODE_NO_TELEMETRY=1");
    eprintln!("  Details: https://github.com/1jehuang/jcode/blob/master/TELEMETRY.md");
    eprintln!("\x1b[0m");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::lock_test_env;
    use std::sync::{Mutex, OnceLock};

    fn lock_telemetry_test_state() -> std::sync::MutexGuard<'static, ()> {
        static TELEMETRY_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        TELEMETRY_TEST_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn test_opt_out_env_var() {
        let _guard = lock_test_env();
        crate::env::set_var("JCODE_NO_TELEMETRY", "1");
        assert!(!is_enabled());
        crate::env::remove_var("JCODE_NO_TELEMETRY");
    }

    #[test]
    fn test_do_not_track() {
        let _guard = lock_test_env();
        crate::env::set_var("DO_NOT_TRACK", "1");
        assert!(!is_enabled());
        crate::env::remove_var("DO_NOT_TRACK");
    }

    #[test]
    fn test_error_counters() {
        let _guard = lock_telemetry_test_state();
        reset_counters();
        record_error(ErrorCategory::ProviderTimeout);
        record_error(ErrorCategory::ProviderTimeout);
        record_error(ErrorCategory::ToolError);
        assert_eq!(ERROR_PROVIDER_TIMEOUT.load(Ordering::Relaxed), 2);
        assert_eq!(ERROR_TOOL_ERROR.load(Ordering::Relaxed), 1);
        reset_counters();
    }

    #[test]
    fn test_session_reason_labels() {
        assert_eq!(SessionEndReason::NormalExit.as_str(), "normal_exit");
        assert_eq!(SessionEndReason::Disconnect.as_str(), "disconnect");
    }

    #[test]
    fn test_session_start_event_serialization() {
        let event = SessionStartEvent {
            event_id: "event-1".to_string(),
            id: "test-uuid".to_string(),
            session_id: "session-1".to_string(),
            event: "session_start",
            version: "0.6.1".to_string(),
            os: "linux",
            arch: "x86_64",
            provider_start: "claude".to_string(),
            model_start: "claude-sonnet-4".to_string(),
            resumed_session: true,
            session_start_hour_utc: 13,
            session_start_weekday_utc: 2,
            previous_session_gap_secs: Some(3600),
            sessions_started_24h: 3,
            sessions_started_7d: 8,
            active_sessions_at_start: 2,
            other_active_sessions_at_start: 1,
            schema_version: TELEMETRY_SCHEMA_VERSION,
            build_channel: "release".to_string(),
            is_git_checkout: false,
            is_ci: false,
            ran_from_cargo: false,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["event"], "session_start");
        assert_eq!(json["resumed_session"], true);
        assert_eq!(json["session_id"], "session-1");
        assert_eq!(json["sessions_started_24h"], 3);
    }

    #[test]
    fn test_session_end_event_serialization() {
        let event = SessionLifecycleEvent {
            event_id: "event-2".to_string(),
            id: "test-uuid".to_string(),
            session_id: "session-2".to_string(),
            event: "session_end",
            version: "0.6.1".to_string(),
            os: "linux",
            arch: "x86_64",
            provider_start: "claude".to_string(),
            provider_end: "openrouter".to_string(),
            model_start: "claude-sonnet-4-20250514".to_string(),
            model_end: "anthropic/claude-sonnet-4".to_string(),
            provider_switches: 1,
            model_switches: 2,
            duration_mins: 45,
            duration_secs: 2700,
            turns: 23,
            had_user_prompt: true,
            had_assistant_response: true,
            assistant_responses: 3,
            first_assistant_response_ms: Some(1200),
            first_tool_call_ms: Some(900),
            first_tool_success_ms: Some(1500),
            first_file_edit_ms: Some(2200),
            first_test_pass_ms: Some(4100),
            tool_calls: 4,
            tool_failures: 1,
            executed_tool_calls: 5,
            executed_tool_successes: 4,
            executed_tool_failures: 1,
            tool_latency_total_ms: 3200,
            tool_latency_max_ms: 1400,
            file_write_calls: 2,
            tests_run: 1,
            tests_passed: 1,
            feature_memory_used: true,
            feature_swarm_used: false,
            feature_web_used: true,
            feature_email_used: false,
            feature_mcp_used: true,
            feature_side_panel_used: true,
            feature_goal_used: false,
            feature_selfdev_used: false,
            feature_background_used: false,
            feature_subagent_used: true,
            unique_mcp_servers: 2,
            session_success: true,
            abandoned_before_response: false,
            transport_https: 2,
            transport_persistent_ws_fresh: 1,
            transport_persistent_ws_reuse: 5,
            transport_cli_subprocess: 0,
            transport_native_http2: 0,
            transport_other: 0,
            tool_cat_read_search: 2,
            tool_cat_write: 2,
            tool_cat_shell: 1,
            tool_cat_web: 1,
            tool_cat_memory: 1,
            tool_cat_subagent: 1,
            tool_cat_swarm: 0,
            tool_cat_email: 0,
            tool_cat_side_panel: 1,
            tool_cat_goal: 0,
            tool_cat_mcp: 1,
            tool_cat_other: 0,
            command_login_used: false,
            command_model_used: true,
            command_usage_used: false,
            command_resume_used: false,
            command_memory_used: true,
            command_swarm_used: false,
            command_goal_used: false,
            command_selfdev_used: false,
            command_feedback_used: false,
            command_other_used: false,
            workflow_chat_only: false,
            workflow_coding_used: true,
            workflow_research_used: true,
            workflow_tests_used: true,
            workflow_background_used: false,
            workflow_subagent_used: true,
            workflow_swarm_used: false,
            project_repo_present: true,
            project_lang_rust: true,
            project_lang_js_ts: false,
            project_lang_python: false,
            project_lang_go: false,
            project_lang_markdown: true,
            project_lang_mixed: true,
            days_since_install: Some(12),
            active_days_7d: 4,
            active_days_30d: 9,
            session_start_hour_utc: 13,
            session_start_weekday_utc: 2,
            session_end_hour_utc: 14,
            session_end_weekday_utc: 2,
            previous_session_gap_secs: Some(1800),
            sessions_started_24h: 5,
            sessions_started_7d: 12,
            active_sessions_at_start: 2,
            other_active_sessions_at_start: 1,
            max_concurrent_sessions: 3,
            multi_sessioned: true,
            resumed_session: false,
            end_reason: "normal_exit",
            schema_version: TELEMETRY_SCHEMA_VERSION,
            build_channel: "release".to_string(),
            is_git_checkout: false,
            is_ci: false,
            ran_from_cargo: false,
            errors: ErrorCounts {
                provider_timeout: 2,
                auth_failed: 0,
                tool_error: 1,
                mcp_error: 0,
                rate_limited: 0,
            },
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["event"], "session_end");
        assert_eq!(json["assistant_responses"], 3);
        assert_eq!(json["duration_secs"], 2700);
        assert_eq!(json["executed_tool_calls"], 5);
        assert_eq!(json["transport_https"], 2);
        assert_eq!(json["tool_cat_write"], 2);
        assert_eq!(json["workflow_coding_used"], true);
        assert_eq!(json["active_days_30d"], 9);
        assert_eq!(json["transport_persistent_ws_reuse"], 5);
        assert_eq!(json["multi_sessioned"], true);
        assert_eq!(json["end_reason"], "normal_exit");
        assert_eq!(json["errors"]["provider_timeout"], 2);
    }

    #[test]
    fn test_record_connection_type_buckets_transport() {
        let _guard = lock_telemetry_test_state();
        reset_counters();
        if let Ok(mut session) = SESSION_STATE.lock() {
            *session = None;
        }
        begin_session_with_mode("openai", "gpt-5.4", false);
        record_connection_type("websocket/persistent-fresh");
        record_connection_type("websocket/persistent-reuse");
        record_connection_type("https/sse");
        record_connection_type("native http2");
        record_connection_type("cli subprocess");
        record_connection_type("weird-transport");

        let guard = SESSION_STATE.lock().unwrap();
        let state = guard.as_ref().expect("session telemetry state");
        assert_eq!(state.transport_persistent_ws_fresh, 1);
        assert_eq!(state.transport_persistent_ws_reuse, 1);
        assert_eq!(state.transport_https, 1);
        assert_eq!(state.transport_native_http2, 1);
        assert_eq!(state.transport_cli_subprocess, 1);
        assert_eq!(state.transport_other, 1);
        if let Ok(mut session) = SESSION_STATE.lock() {
            *session = None;
        }
        reset_counters();
    }

    #[test]
    fn test_sanitize_telemetry_label_strips_ansi_and_controls() {
        assert_eq!(
            sanitize_telemetry_label("\u{1b}[1mclaude-opus-4-6\u{1b}[0m\n"),
            "claude-opus-4-6"
        );
    }

    #[test]
    fn test_install_marker_tracks_current_telemetry_id() {
        let _guard = lock_test_env();
        let prev_home = std::env::var_os("JCODE_HOME");
        let temp = tempfile::TempDir::new().expect("create temp dir");
        crate::env::set_var("JCODE_HOME", temp.path());

        assert!(!install_recorded_for_id("id-a"));
        mark_install_recorded("id-a");
        assert!(install_recorded_for_id("id-a"));
        assert!(!install_recorded_for_id("id-b"));

        if let Some(prev_home) = prev_home {
            crate::env::set_var("JCODE_HOME", prev_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}
