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
        "goal" => TelemetryToolCategory::Goal,
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
