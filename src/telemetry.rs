use crate::storage;
mod lifecycle;
mod state_support;
use chrono::{DateTime, NaiveDate, Utc};
pub use jcode_usage_types::{ErrorCategory, SessionEndReason};
use jcode_usage_types::{
    TelemetryToolCategory as ToolCategory, TelemetryWorkflowCounts,
    classify_telemetry_tool_category as classify_tool_category,
    looks_like_telemetry_test_run as looks_like_test_run,
    mcp_telemetry_server_name as mcp_server_name, sanitize_telemetry_label,
    telemetry_workflow_flags_from_counts,
};
use lifecycle::emit_lifecycle_event;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use state_support::*;
use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

const TELEMETRY_ENDPOINT: &str = "https://jcode-telemetry.jeremyhuang55555.workers.dev/v1/event";
const ASYNC_SEND_TIMEOUT: Duration = Duration::from_secs(5);
const BLOCKING_INSTALL_TIMEOUT: Duration = Duration::from_millis(1200);
const BLOCKING_LIFECYCLE_TIMEOUT: Duration = Duration::from_millis(800);
const TELEMETRY_SCHEMA_VERSION: u32 = 5;

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
    auth_failure_reason: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    feedback_rating: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    feedback_reason: Option<String>,
    feedback_text: String,
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
    input_tokens: u64,
    output_tokens: u64,
    cache_read_input_tokens: u64,
    cache_creation_input_tokens: u64,
    total_tokens: u64,
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
    session_stop_reason: &'static str,
    agent_role: &'static str,
    parent_session_id: Option<String>,
    agent_active_ms_total: u64,
    agent_model_ms_total: u64,
    agent_tool_ms_total: u64,
    session_idle_ms_total: u64,
    agent_blocked_ms_total: u64,
    time_to_first_agent_action_ms: Option<u64>,
    time_to_first_useful_action_ms: Option<u64>,
    spawned_agent_count: u32,
    background_task_count: u32,
    background_task_completed_count: u32,
    subagent_task_count: u32,
    subagent_success_count: u32,
    swarm_task_count: u32,
    swarm_success_count: u32,
    user_cancelled_count: u32,
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
    input_tokens: u64,
    output_tokens: u64,
    cache_read_input_tokens: u64,
    cache_creation_input_tokens: u64,
    total_tokens: u64,
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
    input_tokens: u64,
    output_tokens: u64,
    cache_read_input_tokens: u64,
    cache_creation_input_tokens: u64,
    total_tokens: u64,
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
    parent_session_id: Option<String>,
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
    input_tokens: u64,
    output_tokens: u64,
    cache_read_input_tokens: u64,
    cache_creation_input_tokens: u64,
    total_tokens: u64,
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
    agent_active_ms_total: u64,
    agent_model_ms_total: u64,
    agent_tool_ms_total: u64,
    session_idle_ms_total: u64,
    agent_blocked_ms_total: u64,
    time_to_first_agent_action_ms: Option<u64>,
    time_to_first_useful_action_ms: Option<u64>,
    spawned_agent_count: u32,
    background_task_count: u32,
    background_task_completed_count: u32,
    subagent_task_count: u32,
    subagent_success_count: u32,
    swarm_task_count: u32,
    swarm_success_count: u32,
    user_cancelled_count: u32,
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
            input_tokens: 0,
            output_tokens: 0,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
            total_tokens: 0,
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
enum DeliveryMode {
    Background,
    Blocking(Duration),
}

pub fn is_enabled() -> bool {
    if std::env::var("JCODE_NO_TELEMETRY").is_ok() || std::env::var("DO_NOT_TRACK").is_ok() {
        return false;
    }
    if let Ok(dir) = storage::jcode_dir()
        && dir.join("no_telemetry").exists()
    {
        return false;
    }
    true
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

fn emit_onboarding_step(
    step: &'static str,
    auth_provider: Option<&str>,
    auth_method: Option<&str>,
    auth_failure_reason: Option<&str>,
) {
    if !is_enabled() {
        return;
    }
    let Some(id) = get_or_create_id() else {
        return;
    };
    let _ = send_onboarding_step_for_id(&id, step, auth_provider, auth_method, auth_failure_reason);
}

fn send_onboarding_step_for_id(
    id: &str,
    step: &'static str,
    auth_provider: Option<&str>,
    auth_method: Option<&str>,
    auth_failure_reason: Option<&str>,
) -> bool {
    let (schema_version, build_channel, git_checkout, ci, from_cargo) = telemetry_envelope();
    let event = OnboardingStepEvent {
        event_id: new_event_id(),
        id: id.to_string(),
        session_id: current_session_id(),
        event: "onboarding_step",
        version: version(),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        step,
        auth_provider: auth_provider.map(sanitize_telemetry_label),
        auth_method: auth_method.map(sanitize_telemetry_label),
        auth_failure_reason: auth_failure_reason.map(sanitize_telemetry_label),
        milestone_elapsed_ms: elapsed_since_install_ms(id),
        schema_version,
        build_channel,
        is_git_checkout: git_checkout,
        is_ci: ci,
        ran_from_cargo: from_cargo,
    };
    if let Ok(payload) = serde_json::to_value(&event) {
        return send_payload(payload, DeliveryMode::Background);
    }
    false
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
    let milestone_key = onboarding_step_milestone_key(step, auth_provider, auth_method);
    if milestone_recorded(&id, &milestone_key) {
        return;
    }
    if send_onboarding_step_for_id(&id, step, auth_provider, auth_method, None) {
        mark_milestone_recorded(&id, &milestone_key);
    }
}

pub fn record_setup_step_once(step: &'static str) {
    emit_onboarding_step_once(step, None, None);
}

pub fn record_feedback(text: &str) {
    if !is_enabled() {
        return;
    }
    let Some(id) = get_or_create_id() else {
        return;
    };
    let feedback_text = sanitize_feedback_text(text);
    if feedback_text.is_empty() {
        return;
    }
    let (schema_version, build_channel, git_checkout, ci, from_cargo) = telemetry_envelope();
    let event = FeedbackEvent {
        event_id: new_event_id(),
        id,
        session_id: current_session_id(),
        event: "feedback",
        version: version(),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        feedback_rating: None,
        feedback_reason: None,
        feedback_text,
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

fn sanitize_feedback_text(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\r' | '\t'))
        .collect::<String>()
        .trim()
        .chars()
        .take(2000)
        .collect()
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

fn min_optional_ms(values: impl IntoIterator<Item = Option<u64>>) -> Option<u64> {
    values.into_iter().flatten().min()
}

fn time_to_first_agent_action_ms(state: &SessionTelemetry) -> Option<u64> {
    min_optional_ms([
        state.first_assistant_response_ms,
        state.first_tool_call_ms,
        state.first_tool_success_ms,
        state.first_file_edit_ms,
        state.first_test_pass_ms,
    ])
}

fn time_to_first_useful_action_ms(state: &SessionTelemetry) -> Option<u64> {
    min_optional_ms([
        state.first_tool_success_ms,
        state.first_file_edit_ms,
        state.first_test_pass_ms,
    ])
    .or(state.first_assistant_response_ms)
}

fn infer_agent_role(state: &SessionTelemetry) -> &'static str {
    if state.feature_swarm_used || state.tool_cat_swarm > 0 {
        "swarm"
    } else if state.feature_subagent_used || state.tool_cat_subagent > 0 {
        "subagent"
    } else if state.feature_background_used || state.background_task_count > 0 {
        "background"
    } else {
        "foreground"
    }
}

fn infer_session_stop_reason(
    event_name: &'static str,
    reason: SessionEndReason,
    state: &SessionTelemetry,
    errors: &ErrorCounts,
    duration_secs: u64,
    session_success: bool,
    abandoned_before_response: bool,
    workflow_coding_used: bool,
) -> &'static str {
    if event_name == "session_crash"
        || matches!(reason, SessionEndReason::Panic | SessionEndReason::Signal)
    {
        return "crash";
    }
    if errors.auth_failed > 0 {
        return "auth_blocked";
    }
    if errors.rate_limited > 0 {
        return "rate_limited";
    }
    if errors.provider_timeout > 0 {
        return "provider_timeout";
    }
    if !state.had_user_prompt {
        return "never_prompted";
    }
    if abandoned_before_response {
        return "no_first_response";
    }
    if state.user_cancelled_count > 0 || matches!(reason, SessionEndReason::Disconnect) {
        return "user_interrupted";
    }
    if matches!(state.first_assistant_response_ms, Some(ms) if ms > 60_000)
        && time_to_first_useful_action_ms(state).is_none_or(|ms| ms > 60_000)
    {
        return "too_slow";
    }
    if state.executed_tool_failures >= 3 && state.executed_tool_successes == 0 {
        return "tool_error_loop";
    }
    if errors.tool_error > 0 && state.executed_tool_successes == 0 {
        return "tool_failures";
    }
    if workflow_coding_used && state.file_write_calls == 0 {
        return "no_file_change";
    }
    if state.tests_run > 0 && state.tests_passed == 0 {
        return "test_failure_unresolved";
    }
    if !session_success && duration_secs >= 300 && state.agent_active_ms_total >= 300_000 {
        return "agent_got_stuck";
    }
    if !session_success {
        return "no_useful_action";
    }
    "completed_successfully"
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
            if let Some(turn) = state.current_turn.as_mut()
                && let Some(server) = mcp_server_name(name, input)
            {
                turn.unique_mcp_servers.insert(server);
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
    if let Some(turn) = state.current_turn.as_mut()
        && turn.first_tool_success_ms.is_none()
    {
        turn.first_tool_success_ms = Some(now_ms_since(turn.started_at));
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
    ) && let Some(turn) = state.current_turn.as_mut()
        && turn.first_file_edit_ms.is_none()
    {
        turn.first_file_edit_ms = Some(now_ms_since(turn.started_at));
    }

    if name == "memory" {
        state.feature_memory_used = true;
        if let Some(turn) = state.current_turn.as_mut() {
            turn.feature_memory_used = true;
        }
    }
}

pub fn record_command_family(command: &str) {
    if let Ok(mut guard) = SESSION_STATE.lock()
        && let Some(ref mut state) = *guard
    {
        observe_session_concurrency(state);
        mark_command_family_usage(state, command);
        if let Some(turn) = state.current_turn.as_mut() {
            update_turn_activity_timestamp(turn, Instant::now());
        }
    }
    maybe_emit_session_start();
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
        DeliveryMode::Blocking(timeout) => {
            if tokio::runtime::Handle::try_current().is_ok() {
                let (tx, rx) = std::sync::mpsc::sync_channel(1);
                std::thread::spawn(move || {
                    let _ = tx.send(post_payload(payload, timeout));
                });
                rx.recv_timeout(timeout).unwrap_or(false)
            } else {
                post_payload(payload, timeout)
            }
        }
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
    state.agent_active_ms_total = state
        .agent_active_ms_total
        .saturating_add(turn_active_duration_ms);
    state.agent_tool_ms_total = state
        .agent_tool_ms_total
        .saturating_add(turn.tool_latency_total_ms);
    state.agent_model_ms_total = state.agent_model_ms_total.saturating_add(
        turn_active_duration_ms
            .saturating_sub(turn.tool_latency_total_ms.min(turn_active_duration_ms)),
    );
    state.session_idle_ms_total = state
        .session_idle_ms_total
        .saturating_add(idle_after_turn_ms)
        .saturating_add(turn.idle_before_turn_ms.unwrap_or(0));
    let turn_success = turn.assistant_responses > 0
        || turn.executed_tool_successes > 0
        || turn.tests_passed > 0
        || turn.file_write_calls > 0;
    let turn_abandoned =
        !turn_success && turn.tool_failures == 0 && turn.executed_tool_failures == 0;
    let workflow_flags = telemetry_workflow_flags_from_counts(TelemetryWorkflowCounts {
        had_user_prompt: true,
        file_write_calls: turn.file_write_calls,
        tests_run: turn.tests_run,
        tests_passed: turn.tests_passed,
        feature_web_used: turn.feature_web_used,
        feature_background_used: turn.feature_background_used,
        feature_subagent_used: turn.feature_subagent_used,
        feature_swarm_used: turn.feature_swarm_used,
        tool_cat_write: turn.tool_cat_write,
        tool_cat_web: turn.tool_cat_web,
        tool_cat_subagent: turn.tool_cat_subagent,
        tool_cat_swarm: turn.tool_cat_swarm,
    });
    let workflow_chat_only = workflow_flags.chat_only;
    let workflow_coding_used = workflow_flags.coding_used;
    let workflow_research_used = workflow_flags.research_used;
    let workflow_tests_used = workflow_flags.tests_used;
    let workflow_background_used = workflow_flags.background_used;
    let workflow_subagent_used = workflow_flags.subagent_used;
    let workflow_swarm_used = workflow_flags.swarm_used;
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
        input_tokens: turn.input_tokens,
        output_tokens: turn.output_tokens,
        cache_read_input_tokens: turn.cache_read_input_tokens,
        cache_creation_input_tokens: turn.cache_creation_input_tokens,
        total_tokens: turn.total_tokens,
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
    if let Ok(payload) = serde_json::to_value(&event)
        && send_payload(payload, DeliveryMode::Blocking(BLOCKING_INSTALL_TIMEOUT))
    {
        mark_install_recorded(&id);
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
    emit_onboarding_step("auth_started", Some(provider), Some(method), None);
}

pub fn record_auth_failed(provider: &str, method: &str) {
    record_auth_failed_reason(provider, method, "unknown");
}

pub fn record_auth_failed_reason(provider: &str, method: &str, reason: &str) {
    emit_onboarding_step("auth_failed", Some(provider), Some(method), Some(reason));
}

pub fn record_auth_cancelled(provider: &str, method: &str) {
    emit_onboarding_step("auth_cancelled", Some(provider), Some(method), None);
}

pub fn record_auth_surface_blocked(provider: &str, method: &str) {
    emit_onboarding_step("auth_surface_blocked", Some(provider), Some(method), None);
}

pub fn record_auth_surface_blocked_reason(provider: &str, method: &str, reason: &str) {
    emit_onboarding_step(
        "auth_surface_blocked",
        Some(provider),
        Some(method),
        Some(reason),
    );
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
    begin_session_with_parent(provider, model, None, false);
}

pub fn begin_session_with_parent(
    provider: &str,
    model: &str,
    parent_session_id: Option<String>,
    resumed_session: bool,
) {
    begin_session_with_mode(provider, model, parent_session_id, resumed_session);
}

pub fn begin_resumed_session(provider: &str, model: &str) {
    begin_session_with_mode(provider, model, None, true);
}

fn begin_session_with_mode(
    provider: &str,
    model: &str,
    parent_session_id: Option<String>,
    resumed_session: bool,
) {
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
        parent_session_id,
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
        input_tokens: 0,
        output_tokens: 0,
        cache_read_input_tokens: 0,
        cache_creation_input_tokens: 0,
        total_tokens: 0,
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
        agent_active_ms_total: 0,
        agent_model_ms_total: 0,
        agent_tool_ms_total: 0,
        session_idle_ms_total: 0,
        agent_blocked_ms_total: 0,
        time_to_first_agent_action_ms: None,
        time_to_first_useful_action_ms: None,
        spawned_agent_count: 0,
        background_task_count: 0,
        background_task_completed_count: 0,
        subagent_task_count: 0,
        subagent_success_count: 0,
        swarm_task_count: 0,
        swarm_success_count: 0,
        user_cancelled_count: 0,
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
    if let Ok(mut guard) = SESSION_STATE.lock()
        && let Some(ref mut state) = *guard
    {
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
    emit_onboarding_step_once("first_prompt_sent", None, None);
    maybe_emit_session_start();
}

pub fn record_assistant_response() {
    if let Ok(mut guard) = SESSION_STATE.lock()
        && let Some(ref mut state) = *guard
    {
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
    emit_onboarding_step_once("first_assistant_response", None, None);
    maybe_emit_session_start();
}

pub fn record_memory_injected(_count: usize, _age_ms: u64) {
    if let Ok(mut guard) = SESSION_STATE.lock()
        && let Some(ref mut state) = *guard
    {
        observe_session_concurrency(state);
        state.feature_memory_used = true;
        if let Some(turn) = state.current_turn.as_mut() {
            turn.feature_memory_used = true;
            update_turn_activity_timestamp(turn, Instant::now());
        }
    }
    maybe_emit_session_start();
}

pub fn record_tool_call() {
    if let Ok(mut guard) = SESSION_STATE.lock()
        && let Some(ref mut state) = *guard
    {
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
    maybe_emit_session_start();
}

pub fn record_tool_failure() {
    if let Ok(mut guard) = SESSION_STATE.lock()
        && let Some(ref mut state) = *guard
    {
        observe_session_concurrency(state);
        state.tool_failures += 1;
        if let Some(turn) = state.current_turn.as_mut() {
            turn.tool_failures += 1;
            update_turn_activity_timestamp(turn, Instant::now());
        }
    }
    maybe_emit_session_start();
}

pub fn record_connection_type(connection: &str) {
    if let Ok(mut guard) = SESSION_STATE.lock()
        && let Some(ref mut state) = *guard
    {
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
    maybe_emit_session_start();
}

pub fn record_token_usage(
    input_tokens: u64,
    output_tokens: u64,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
) {
    if let Ok(mut guard) = SESSION_STATE.lock()
        && let Some(ref mut state) = *guard
    {
        observe_session_concurrency(state);
        let cache_read = cache_read_input_tokens.unwrap_or(0);
        let cache_creation = cache_creation_input_tokens.unwrap_or(0);
        let total = input_tokens
            .saturating_add(output_tokens)
            .saturating_add(cache_read)
            .saturating_add(cache_creation);

        state.input_tokens = state.input_tokens.saturating_add(input_tokens);
        state.output_tokens = state.output_tokens.saturating_add(output_tokens);
        state.cache_read_input_tokens = state.cache_read_input_tokens.saturating_add(cache_read);
        state.cache_creation_input_tokens = state
            .cache_creation_input_tokens
            .saturating_add(cache_creation);
        state.total_tokens = state.total_tokens.saturating_add(total);

        if let Some(turn) = state.current_turn.as_mut() {
            turn.input_tokens = turn.input_tokens.saturating_add(input_tokens);
            turn.output_tokens = turn.output_tokens.saturating_add(output_tokens);
            turn.cache_read_input_tokens = turn.cache_read_input_tokens.saturating_add(cache_read);
            turn.cache_creation_input_tokens = turn
                .cache_creation_input_tokens
                .saturating_add(cache_creation);
            turn.total_tokens = turn.total_tokens.saturating_add(total);
            update_turn_activity_timestamp(turn, Instant::now());
        }
    }
    maybe_emit_session_start();
}

pub fn record_error(category: ErrorCategory) {
    if let Ok(mut guard) = SESSION_STATE.lock()
        && let Some(ref mut state) = *guard
    {
        observe_session_concurrency(state);
        if let Some(turn) = state.current_turn.as_mut() {
            update_turn_activity_timestamp(turn, Instant::now());
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
    if let Ok(mut guard) = SESSION_STATE.lock()
        && let Some(ref mut state) = *guard
    {
        observe_session_concurrency(state);
        if let Some(turn) = state.current_turn.as_mut() {
            update_turn_activity_timestamp(turn, Instant::now());
        }
    }
    PROVIDER_SWITCHES.fetch_add(1, Ordering::Relaxed);
    maybe_emit_session_start();
}

pub fn record_model_switch() {
    if let Ok(mut guard) = SESSION_STATE.lock()
        && let Some(ref mut state) = *guard
    {
        observe_session_concurrency(state);
        if let Some(turn) = state.current_turn.as_mut() {
            update_turn_activity_timestamp(turn, Instant::now());
        }
    }
    MODEL_SWITCHES.fetch_add(1, Ordering::Relaxed);
    maybe_emit_session_start();
}

pub fn record_user_cancelled() {
    if let Ok(mut guard) = SESSION_STATE.lock()
        && let Some(ref mut state) = *guard
    {
        observe_session_concurrency(state);
        state.user_cancelled_count = state.user_cancelled_count.saturating_add(1);
        if let Some(turn) = state.current_turn.as_mut() {
            update_turn_activity_timestamp(turn, Instant::now());
        }
    }
    maybe_emit_session_start();
}

pub fn record_tool_execution(name: &str, input: &Value, succeeded: bool, latency_ms: u64) {
    if let Ok(mut guard) = SESSION_STATE.lock()
        && let Some(ref mut state) = *guard
    {
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
        match classify_tool_category(name) {
            ToolCategory::Subagent => {
                state.subagent_task_count = state.subagent_task_count.saturating_add(1);
                if succeeded {
                    state.subagent_success_count = state.subagent_success_count.saturating_add(1);
                }
            }
            ToolCategory::Swarm => {
                state.swarm_task_count = state.swarm_task_count.saturating_add(1);
                if succeeded {
                    state.swarm_success_count = state.swarm_success_count.saturating_add(1);
                }
            }
            ToolCategory::Shell
                if matches!(name, "bg" | "schedule")
                    || input
                        .get("run_in_background")
                        .and_then(Value::as_bool)
                        .unwrap_or(false) =>
            {
                state.background_task_count = state.background_task_count.saturating_add(1);
                if succeeded {
                    state.background_task_completed_count =
                        state.background_task_completed_count.saturating_add(1);
                }
            }
            _ => {}
        }
        state.spawned_agent_count = state
            .background_task_count
            .saturating_add(state.subagent_task_count)
            .saturating_add(state.swarm_task_count);
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
mod tests;
