#[derive(Debug, Clone, Default)]
pub struct ProviderUsage {
    pub provider_name: String,
    pub limits: Vec<UsageLimit>,
    pub extra_info: Vec<(String, String)>,
    pub hard_limit_reached: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UsageLimit {
    pub name: String,
    pub usage_percent: f32,
    pub resets_at: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ProviderUsageProgress {
    pub results: Vec<ProviderUsage>,
    pub completed: usize,
    pub total: usize,
    pub done: bool,
    pub from_cache: bool,
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CopilotUsageTracker {
    pub today: DayUsage,
    pub month: MonthUsage,
    pub all_time: AllTimeUsage,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DayUsage {
    pub date: String,
    pub requests: u64,
    pub premium_requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MonthUsage {
    pub month: String,
    pub requests: u64,
    pub premium_requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AllTimeUsage {
    pub requests: u64,
    pub premium_requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelemetryToolCategory {
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

pub fn classify_telemetry_tool_category(name: &str) -> TelemetryToolCategory {
    match name {
        "read"
        | "glob"
        | "grep"
        | "agentgrep"
        | "ls"
        | "conversation_search"
        | "session_search" => TelemetryToolCategory::ReadSearch,
        "write" | "edit" | "multiedit" | "patch" | "apply_patch" => TelemetryToolCategory::Write,
        "bash" | "bg" | "schedule" => TelemetryToolCategory::Shell,
        "webfetch" | "websearch" | "codesearch" | "open" => TelemetryToolCategory::Web,
        "memory" => TelemetryToolCategory::Memory,
        "subagent" => TelemetryToolCategory::Subagent,
        "swarm" | "communicate" => TelemetryToolCategory::Swarm,
        "gmail" => TelemetryToolCategory::Email,
        "side_panel" => TelemetryToolCategory::SidePanel,
        "initiative" => TelemetryToolCategory::Goal,
        "mcp" => TelemetryToolCategory::Mcp,
        other if other.starts_with("mcp__") => TelemetryToolCategory::Mcp,
        _ => TelemetryToolCategory::Other,
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TelemetryWorkflowCounts {
    pub had_user_prompt: bool,
    pub file_write_calls: u32,
    pub tests_run: u32,
    pub tests_passed: u32,
    pub feature_web_used: bool,
    pub feature_background_used: bool,
    pub feature_subagent_used: bool,
    pub feature_swarm_used: bool,
    pub tool_cat_write: u32,
    pub tool_cat_web: u32,
    pub tool_cat_subagent: u32,
    pub tool_cat_swarm: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TelemetryWorkflowFlags {
    pub chat_only: bool,
    pub coding_used: bool,
    pub research_used: bool,
    pub tests_used: bool,
    pub background_used: bool,
    pub subagent_used: bool,
    pub swarm_used: bool,
}

pub fn telemetry_workflow_flags_from_counts(
    counts: TelemetryWorkflowCounts,
) -> TelemetryWorkflowFlags {
    let coding_used = counts.file_write_calls > 0 || counts.tool_cat_write > 0;
    let research_used = counts.feature_web_used || counts.tool_cat_web > 0;
    let tests_used = counts.tests_run > 0 || counts.tests_passed > 0;
    let background_used = counts.feature_background_used;
    let subagent_used = counts.feature_subagent_used || counts.tool_cat_subagent > 0;
    let swarm_used = counts.feature_swarm_used || counts.tool_cat_swarm > 0;
    let chat_only = counts.had_user_prompt
        && !coding_used
        && !research_used
        && !tests_used
        && !background_used
        && !subagent_used
        && !swarm_used;
    TelemetryWorkflowFlags {
        chat_only,
        coding_used,
        research_used,
        tests_used,
        background_used,
        subagent_used,
        swarm_used,
    }
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
    pub fn as_str(self) -> &'static str {
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

#[derive(Debug, Clone, Copy)]
pub enum ErrorCategory {
    ProviderTimeout,
    AuthFailed,
    ToolError,
    McpError,
    RateLimited,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TelemetryProjectProfile {
    pub repo_present: bool,
    pub lang_rust: bool,
    pub lang_js_ts: bool,
    pub lang_python: bool,
    pub lang_go: bool,
    pub lang_markdown: bool,
}

impl TelemetryProjectProfile {
    pub fn mixed(&self) -> bool {
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

    pub fn note_extension(&mut self, extension: &str) {
        match extension {
            "rs" => self.lang_rust = true,
            "js" | "jsx" | "ts" | "tsx" => self.lang_js_ts = true,
            "py" => self.lang_python = true,
            "go" => self.lang_go = true,
            "md" | "mdx" => self.lang_markdown = true,
            _ => {}
        }
    }
}

pub fn sanitize_feedback_text(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\r' | '\t'))
        .collect::<String>()
        .trim()
        .chars()
        .take(2000)
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallEvent {
    pub event_id: String,
    pub id: String,
    pub event: &'static str,
    pub version: String,
    pub os: &'static str,
    pub arch: &'static str,
    pub schema_version: u32,
    pub build_channel: String,
    pub is_git_checkout: bool,
    pub is_ci: bool,
    pub ran_from_cargo: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpgradeEvent {
    pub event_id: String,
    pub id: String,
    pub event: &'static str,
    pub version: String,
    pub os: &'static str,
    pub arch: &'static str,
    pub from_version: String,
    pub schema_version: u32,
    pub build_channel: String,
    pub is_git_checkout: bool,
    pub is_ci: bool,
    pub ran_from_cargo: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthEvent {
    pub event_id: String,
    pub id: String,
    pub event: &'static str,
    pub version: String,
    pub os: &'static str,
    pub arch: &'static str,
    pub auth_provider: String,
    pub auth_method: String,
    pub schema_version: u32,
    pub build_channel: String,
    pub is_git_checkout: bool,
    pub is_ci: bool,
    pub ran_from_cargo: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStartEvent {
    pub event_id: String,
    pub id: String,
    pub session_id: String,
    pub event: &'static str,
    pub version: String,
    pub os: &'static str,
    pub arch: &'static str,
    pub provider_start: String,
    pub model_start: String,
    pub resumed_session: bool,
    pub session_start_hour_utc: u32,
    pub session_start_weekday_utc: u32,
    pub previous_session_gap_secs: Option<u64>,
    pub sessions_started_24h: u32,
    pub sessions_started_7d: u32,
    pub active_sessions_at_start: u32,
    pub other_active_sessions_at_start: u32,
    pub schema_version: u32,
    pub build_channel: String,
    pub is_git_checkout: bool,
    pub is_ci: bool,
    pub ran_from_cargo: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnboardingStepEvent {
    pub event_id: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub event: &'static str,
    pub version: String,
    pub os: &'static str,
    pub arch: &'static str,
    pub step: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_failure_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub milestone_elapsed_ms: Option<u64>,
    pub schema_version: u32,
    pub build_channel: String,
    pub is_git_checkout: bool,
    pub is_ci: bool,
    pub ran_from_cargo: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackEvent {
    pub event_id: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub event: &'static str,
    pub version: String,
    pub os: &'static str,
    pub arch: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feedback_rating: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feedback_reason: Option<String>,
    pub feedback_text: String,
    pub schema_version: u32,
    pub build_channel: String,
    pub is_git_checkout: bool,
    pub is_ci: bool,
    pub ran_from_cargo: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionLifecycleEvent {
    pub event_id: String,
    pub id: String,
    pub session_id: String,
    pub event: &'static str,
    pub version: String,
    pub os: &'static str,
    pub arch: &'static str,
    pub provider_start: String,
    pub provider_end: String,
    pub model_start: String,
    pub model_end: String,
    pub provider_switches: u32,
    pub model_switches: u32,
    pub duration_mins: u64,
    pub duration_secs: u64,
    pub turns: u32,
    pub had_user_prompt: bool,
    pub had_assistant_response: bool,
    pub assistant_responses: u32,
    pub first_assistant_response_ms: Option<u64>,
    pub first_tool_call_ms: Option<u64>,
    pub first_tool_success_ms: Option<u64>,
    pub first_file_edit_ms: Option<u64>,
    pub first_test_pass_ms: Option<u64>,
    pub tool_calls: u32,
    pub tool_failures: u32,
    pub executed_tool_calls: u32,
    pub executed_tool_successes: u32,
    pub executed_tool_failures: u32,
    pub tool_latency_total_ms: u64,
    pub tool_latency_max_ms: u64,
    pub file_write_calls: u32,
    pub tests_run: u32,
    pub tests_passed: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub total_tokens: u64,
    pub feature_memory_used: bool,
    pub feature_swarm_used: bool,
    pub feature_web_used: bool,
    pub feature_email_used: bool,
    pub feature_mcp_used: bool,
    pub feature_side_panel_used: bool,
    pub feature_goal_used: bool,
    pub feature_selfdev_used: bool,
    pub feature_background_used: bool,
    pub feature_subagent_used: bool,
    pub unique_mcp_servers: u32,
    pub session_success: bool,
    pub abandoned_before_response: bool,
    pub session_stop_reason: &'static str,
    pub agent_role: &'static str,
    pub parent_session_id: Option<String>,
    pub agent_active_ms_total: u64,
    pub agent_model_ms_total: u64,
    pub agent_tool_ms_total: u64,
    pub session_idle_ms_total: u64,
    pub agent_blocked_ms_total: u64,
    pub time_to_first_agent_action_ms: Option<u64>,
    pub time_to_first_useful_action_ms: Option<u64>,
    pub spawned_agent_count: u32,
    pub background_task_count: u32,
    pub background_task_completed_count: u32,
    pub subagent_task_count: u32,
    pub subagent_success_count: u32,
    pub swarm_task_count: u32,
    pub swarm_success_count: u32,
    pub user_cancelled_count: u32,
    pub transport_https: u32,
    pub transport_persistent_ws_fresh: u32,
    pub transport_persistent_ws_reuse: u32,
    pub transport_cli_subprocess: u32,
    pub transport_native_http2: u32,
    pub transport_other: u32,
    pub tool_cat_read_search: u32,
    pub tool_cat_write: u32,
    pub tool_cat_shell: u32,
    pub tool_cat_web: u32,
    pub tool_cat_memory: u32,
    pub tool_cat_subagent: u32,
    pub tool_cat_swarm: u32,
    pub tool_cat_email: u32,
    pub tool_cat_side_panel: u32,
    pub tool_cat_goal: u32,
    pub tool_cat_mcp: u32,
    pub tool_cat_other: u32,
    pub command_login_used: bool,
    pub command_model_used: bool,
    pub command_usage_used: bool,
    pub command_resume_used: bool,
    pub command_memory_used: bool,
    pub command_swarm_used: bool,
    pub command_goal_used: bool,
    pub command_selfdev_used: bool,
    pub command_feedback_used: bool,
    pub command_other_used: bool,
    pub workflow_chat_only: bool,
    pub workflow_coding_used: bool,
    pub workflow_research_used: bool,
    pub workflow_tests_used: bool,
    pub workflow_background_used: bool,
    pub workflow_subagent_used: bool,
    pub workflow_swarm_used: bool,
    pub project_repo_present: bool,
    pub project_lang_rust: bool,
    pub project_lang_js_ts: bool,
    pub project_lang_python: bool,
    pub project_lang_go: bool,
    pub project_lang_markdown: bool,
    pub project_lang_mixed: bool,
    pub days_since_install: Option<u32>,
    pub active_days_7d: u32,
    pub active_days_30d: u32,
    pub session_start_hour_utc: u32,
    pub session_start_weekday_utc: u32,
    pub session_end_hour_utc: u32,
    pub session_end_weekday_utc: u32,
    pub previous_session_gap_secs: Option<u64>,
    pub sessions_started_24h: u32,
    pub sessions_started_7d: u32,
    pub active_sessions_at_start: u32,
    pub other_active_sessions_at_start: u32,
    pub max_concurrent_sessions: u32,
    pub multi_sessioned: bool,
    pub resumed_session: bool,
    pub end_reason: &'static str,
    pub schema_version: u32,
    pub build_channel: String,
    pub is_git_checkout: bool,
    pub is_ci: bool,
    pub ran_from_cargo: bool,
    pub errors: ErrorCounts,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorCounts {
    pub provider_timeout: u32,
    pub auth_failed: u32,
    pub tool_error: u32,
    pub mcp_error: u32,
    pub rate_limited: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnEndEvent {
    pub event_id: String,
    pub id: String,
    pub session_id: String,
    pub event: &'static str,
    pub version: String,
    pub os: &'static str,
    pub arch: &'static str,
    pub turn_index: u32,
    pub turn_started_ms: u64,
    pub turn_active_duration_ms: u64,
    pub idle_before_turn_ms: Option<u64>,
    pub idle_after_turn_ms: u64,
    pub assistant_responses: u32,
    pub first_assistant_response_ms: Option<u64>,
    pub first_tool_call_ms: Option<u64>,
    pub first_tool_success_ms: Option<u64>,
    pub first_file_edit_ms: Option<u64>,
    pub first_test_pass_ms: Option<u64>,
    pub tool_calls: u32,
    pub tool_failures: u32,
    pub executed_tool_calls: u32,
    pub executed_tool_successes: u32,
    pub executed_tool_failures: u32,
    pub tool_latency_total_ms: u64,
    pub tool_latency_max_ms: u64,
    pub file_write_calls: u32,
    pub tests_run: u32,
    pub tests_passed: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub total_tokens: u64,
    pub feature_memory_used: bool,
    pub feature_swarm_used: bool,
    pub feature_web_used: bool,
    pub feature_email_used: bool,
    pub feature_mcp_used: bool,
    pub feature_side_panel_used: bool,
    pub feature_goal_used: bool,
    pub feature_selfdev_used: bool,
    pub feature_background_used: bool,
    pub feature_subagent_used: bool,
    pub unique_mcp_servers: u32,
    pub tool_cat_read_search: u32,
    pub tool_cat_write: u32,
    pub tool_cat_shell: u32,
    pub tool_cat_web: u32,
    pub tool_cat_memory: u32,
    pub tool_cat_subagent: u32,
    pub tool_cat_swarm: u32,
    pub tool_cat_email: u32,
    pub tool_cat_side_panel: u32,
    pub tool_cat_goal: u32,
    pub tool_cat_mcp: u32,
    pub tool_cat_other: u32,
    pub workflow_chat_only: bool,
    pub workflow_coding_used: bool,
    pub workflow_research_used: bool,
    pub workflow_tests_used: bool,
    pub workflow_background_used: bool,
    pub workflow_subagent_used: bool,
    pub workflow_swarm_used: bool,
    pub turn_success: bool,
    pub turn_abandoned: bool,
    pub turn_end_reason: &'static str,
    pub schema_version: u32,
    pub build_channel: String,
    pub is_git_checkout: bool,
    pub is_ci: bool,
    pub ran_from_cargo: bool,
}

pub fn sanitize_telemetry_label(value: &str) -> String {
    let mut cleaned = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            if matches!(chars.peek(), Some('[')) {
                let _ = chars.next();
                for next in chars.by_ref() {
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

pub fn looks_like_telemetry_test_run(name: &str, input: &serde_json::Value) -> bool {
    let mut haystacks = Vec::new();
    haystacks.push(name.to_ascii_lowercase());

    if let Some(command) = input.get("command").and_then(serde_json::Value::as_str) {
        haystacks.push(command.to_ascii_lowercase());
    }
    if let Some(description) = input.get("description").and_then(serde_json::Value::as_str) {
        haystacks.push(description.to_ascii_lowercase());
    }
    if let Some(task) = input.get("task").and_then(serde_json::Value::as_str) {
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

pub fn mcp_telemetry_server_name(name: &str, input: &serde_json::Value) -> Option<String> {
    if let Some(rest) = name.strip_prefix("mcp__") {
        return rest.split("__").next().map(|value| value.to_string());
    }
    if name == "mcp" {
        return input
            .get("server")
            .and_then(serde_json::Value::as_str)
            .map(sanitize_telemetry_label)
            .filter(|value| !value.is_empty());
    }
    None
}

#[cfg(test)]
mod telemetry_helper_tests {
    use super::*;

    #[test]
    fn classifies_known_tool_categories() {
        assert_eq!(
            classify_telemetry_tool_category("agentgrep"),
            TelemetryToolCategory::ReadSearch
        );
        assert_eq!(
            classify_telemetry_tool_category("apply_patch"),
            TelemetryToolCategory::Write
        );
        assert_eq!(
            classify_telemetry_tool_category("mcp__github__issue"),
            TelemetryToolCategory::Mcp
        );
    }

    #[test]
    fn derives_workflow_flags_from_counts() {
        let chat = telemetry_workflow_flags_from_counts(TelemetryWorkflowCounts {
            had_user_prompt: true,
            ..TelemetryWorkflowCounts::default()
        });
        assert!(chat.chat_only);

        let coding = telemetry_workflow_flags_from_counts(TelemetryWorkflowCounts {
            had_user_prompt: true,
            tool_cat_write: 1,
            tests_run: 1,
            ..TelemetryWorkflowCounts::default()
        });
        assert!(!coding.chat_only);
        assert!(coding.coding_used);
        assert!(coding.tests_used);
    }

    #[test]
    fn session_end_reason_labels_are_stable() {
        assert_eq!(SessionEndReason::NormalExit.as_str(), "normal_exit");
        assert_eq!(SessionEndReason::Disconnect.as_str(), "disconnect");
    }

    #[test]
    fn sanitizes_ansi_and_control_characters() {
        assert_eq!(
            sanitize_telemetry_label("\u{1b}[1mclaude-opus-4-6\u{1b}[0m\n"),
            "claude-opus-4-6"
        );
    }

    #[test]
    fn project_profile_tracks_languages_and_mixed_state() {
        let mut profile = TelemetryProjectProfile::default();
        profile.note_extension("rs");
        assert!(!profile.mixed());
        profile.note_extension("ts");
        assert!(profile.mixed());
        profile.note_extension("lock");
        assert!(profile.lang_rust);
        assert!(profile.lang_js_ts);
    }

    #[test]
    fn sanitizes_feedback_text() {
        let raw = format!("  ok\u{0000}\n{}  ", "x".repeat(2100));
        let sanitized = sanitize_feedback_text(&raw);
        assert!(sanitized.starts_with("ok\n"));
        assert_eq!(sanitized.chars().count(), 2000);
        assert!(!sanitized.contains('\u{0000}'));
    }

    #[test]
    fn detects_test_runs_from_tool_input() {
        assert!(looks_like_telemetry_test_run(
            "bash",
            &serde_json::json!({ "command": "cargo test -p jcode" })
        ));
        assert!(looks_like_telemetry_test_run(
            "schedule",
            &serde_json::json!({ "task": "run pytest overnight" })
        ));
        assert!(!looks_like_telemetry_test_run(
            "bash",
            &serde_json::json!({ "command": "cargo build" })
        ));
    }

    #[test]
    fn extracts_mcp_server_names() {
        assert_eq!(
            mcp_telemetry_server_name("mcp__github__issue", &serde_json::Value::Null).as_deref(),
            Some("github")
        );
        assert_eq!(
            mcp_telemetry_server_name(
                "mcp",
                &serde_json::json!({ "server": "\u{1b}[1mlinear\u{1b}[0m" })
            )
            .as_deref(),
            Some("linear")
        );
    }
}
