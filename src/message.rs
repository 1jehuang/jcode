#![allow(dead_code)]

use chrono::Utc;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::OnceLock;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InputShellResult {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub output: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub failed_to_start: bool,
}

fn sanitize_fenced_block(text: &str) -> String {
    text.replace("```", "``\u{200b}`")
}

pub fn format_input_shell_result_markdown(shell: &InputShellResult) -> String {
    let status = if shell.failed_to_start {
        "✗ failed to start".to_string()
    } else if shell.exit_code == Some(0) {
        "✓ exit 0".to_string()
    } else if let Some(code) = shell.exit_code {
        format!("✗ exit {}", code)
    } else {
        "✗ terminated".to_string()
    };

    let mut meta = vec![status, Message::format_duration(shell.duration_ms)];
    if let Some(cwd) = shell.cwd.as_deref() {
        meta.push(format!("cwd `{}`", cwd));
    }
    if shell.truncated {
        meta.push("truncated".to_string());
    }

    let mut message = format!(
        "**Shell command** · {}\n\n```bash\n{}\n```",
        meta.join(" · "),
        sanitize_fenced_block(&shell.command)
    );

    if shell.output.trim().is_empty() {
        message.push_str("\n\n_No output._");
    } else {
        message.push_str(&format!(
            "\n\n```text\n{}\n```",
            sanitize_fenced_block(shell.output.trim_end())
        ));
    }

    message
}

pub fn input_shell_status_notice(shell: &InputShellResult) -> String {
    if shell.failed_to_start {
        "Shell command failed to start".to_string()
    } else if shell.exit_code == Some(0) {
        "Shell command completed".to_string()
    } else if let Some(code) = shell.exit_code {
        format!("Shell command failed (exit {})", code)
    } else {
        "Shell command terminated".to_string()
    }
}

/// Role in conversation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

/// Plain-text tool output placeholder when execution was interrupted.
pub const TOOL_OUTPUT_MISSING_TEXT: &str =
    "Tool output missing (session interrupted before tool execution completed)";

/// A message in the conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<chrono::DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_duration_ms: Option<u64>,
}

/// Cache control metadata for prompt caching
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheControl {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<String>,
}

impl CacheControl {
    pub fn ephemeral(ttl: Option<String>) -> Self {
        Self {
            kind: "ephemeral".to_string(),
            ttl,
        }
    }
}

/// Content block within a message
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    /// Hidden reasoning content used for providers that require it (not displayed)
    Reasoning {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
    Image {
        media_type: String,
        data: String,
    },
    /// Hidden OpenAI Responses compaction item used to preserve native
    /// compaction state across turns/saves when jcode explicitly triggers it.
    OpenAICompaction {
        encrypted_content: String,
    },
}

impl Message {
    pub fn user(text: &str) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
            timestamp: Some(Utc::now()),
            tool_duration_ms: None,
        }
    }

    pub fn user_with_images(text: &str, images: Vec<(String, String)>) -> Self {
        let mut content: Vec<ContentBlock> = images
            .into_iter()
            .map(|(media_type, data)| ContentBlock::Image { media_type, data })
            .collect();
        content.push(ContentBlock::Text {
            text: text.to_string(),
            cache_control: None,
        });
        Self {
            role: Role::User,
            content,
            timestamp: Some(Utc::now()),
            tool_duration_ms: None,
        }
    }

    pub fn assistant_text(text: &str) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
            timestamp: Some(Utc::now()),
            tool_duration_ms: None,
        }
    }

    pub fn tool_result(tool_use_id: &str, content: &str, is_error: bool) -> Self {
        Self::tool_result_with_duration(tool_use_id, content, is_error, None)
    }

    pub fn tool_result_with_duration(
        tool_use_id: &str,
        content: &str,
        is_error: bool,
        tool_duration_ms: Option<u64>,
    ) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: content.to_string(),
                is_error: if is_error { Some(true) } else { None },
            }],
            timestamp: Some(Utc::now()),
            tool_duration_ms,
        }
    }

    /// Format a timestamp deterministically in UTC for injection into model-visible content.
    pub fn format_timestamp(ts: &chrono::DateTime<Utc>) -> String {
        ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    }

    pub fn format_duration(duration_ms: u64) -> String {
        match duration_ms {
            0..=999 => format!("{}ms", duration_ms),
            1_000..=9_999 => format!("{:.1}s", duration_ms as f64 / 1000.0),
            10_000..=59_999 => format!("{}s", duration_ms / 1000),
            _ => {
                let total_seconds = duration_ms / 1000;
                let minutes = total_seconds / 60;
                let seconds = total_seconds % 60;
                if seconds == 0 {
                    format!("{}m", minutes)
                } else {
                    format!("{}m {}s", minutes, seconds)
                }
            }
        }
    }

    fn should_skip_timestamp_injection(&self) -> bool {
        self.content
            .iter()
            .find_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text.trim_start()),
                _ => None,
            })
            .is_some_and(|text| text.starts_with("<system-reminder>"))
    }

    fn tool_result_tag(&self, ts: &chrono::DateTime<Utc>) -> String {
        match self.tool_duration_ms {
            Some(duration_ms) => {
                let duration_ms_i64 = i64::try_from(duration_ms).unwrap_or(i64::MAX);
                let start_ts = ts
                    .checked_sub_signed(chrono::Duration::milliseconds(duration_ms_i64))
                    .unwrap_or(*ts);
                format!(
                    "[tool timing: start={} finish={} duration={}]",
                    Self::format_timestamp(&start_ts),
                    Self::format_timestamp(ts),
                    Self::format_duration(duration_ms)
                )
            }
            None => format!("[{}]", Self::format_timestamp(ts)),
        }
    }

    /// Return a copy of messages with timestamps injected into user-role text content.
    /// Tool results get a stable UTC timing header prepended to content.
    /// User text messages get a stable UTC timestamp prepended to the first text block.
    pub fn with_timestamps(messages: &[Message]) -> Vec<Message> {
        messages
            .iter()
            .map(|msg| {
                let Some(ts) = msg.timestamp else {
                    return msg.clone();
                };
                if msg.role != Role::User || msg.should_skip_timestamp_injection() {
                    return msg.clone();
                }
                let text_tag = format!("[{}]", Self::format_timestamp(&ts));
                let tool_result_tag = msg.tool_result_tag(&ts);
                let mut msg = msg.clone();
                let mut tagged = false;
                for block in &mut msg.content {
                    match block {
                        ContentBlock::Text { text, .. } if !tagged => {
                            *text = format!("{} {}", text_tag, text);
                            tagged = true;
                        }
                        ContentBlock::ToolResult { content, .. } if !tagged => {
                            *content = format!("{} {}", tool_result_tag, content);
                            tagged = true;
                        }
                        _ => {}
                    }
                }
                msg
            })
            .collect()
    }
}

/// Sanitize a tool ID so it matches the pattern `^[a-zA-Z0-9_-]+$`.
///
/// Different providers generate tool IDs in different formats. When switching
/// from one provider to another mid-conversation, the historical tool IDs may
/// contain characters that the new provider rejects (e.g., dots in Copilot IDs
/// sent to Anthropic). This function replaces any invalid characters with
/// underscores.
pub fn sanitize_tool_id(id: &str) -> String {
    if id.is_empty() {
        return "unknown".to_string();
    }
    let sanitized: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

/// Redact likely secrets from persisted tool output.
///
/// This is a best-effort safeguard for local session history files. It targets
/// high-confidence token/key patterns and common `KEY=VALUE` assignments used by
/// auth flows.
pub fn redact_secrets(text: &str) -> String {
    // Fast path to avoid regex work for most tool outputs.
    let lower = text.to_ascii_lowercase();

    if !text.contains("sk-")
        && !text.contains("ghp_")
        && !text.contains("github_pat_")
        && !text.contains("AIza")
        && !text.contains("ya29.")
        && !text.contains("xox")
        && !lower.contains("api_key")
        && !lower.contains("token")
    {
        return text.to_string();
    }

    static DIRECT_PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    static ASSIGNMENT_PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();

    let direct_patterns = DIRECT_PATTERNS.get_or_init(|| {
        vec![
            Regex::new(r"sk-ant-(?:oat|ort)01-[A-Za-z0-9_-]{20,}")
                .expect("valid Anthropic OAuth token regex"),
            Regex::new(r"sk-or-v1-[A-Za-z0-9_-]{20,}").expect("valid OpenRouter key regex"),
            Regex::new(r"ghp_[A-Za-z0-9]{20,}").expect("valid GitHub PAT regex"),
            Regex::new(r"github_pat_[A-Za-z0-9_]{20,}")
                .expect("valid GitHub fine-grained PAT regex"),
            Regex::new(r"ya29\.[A-Za-z0-9._-]{20,}").expect("valid Google OAuth token regex"),
            Regex::new(r"AIza[0-9A-Za-z_-]{20,}").expect("valid Google API key regex"),
            Regex::new(r"xox[baprs]-[A-Za-z0-9-]{10,}").expect("valid Slack token regex"),
        ]
    });

    let assignment_patterns = ASSIGNMENT_PATTERNS.get_or_init(|| {
        vec![
            Regex::new(r"(?m)^\s*(OPENROUTER_API_KEY\s*=\s*)[^\r\n]+")
                .expect("valid OPENROUTER_API_KEY assignment regex"),
            Regex::new(r"(?m)^\s*(OPENCODE_API_KEY\s*=\s*)[^\r\n]+")
                .expect("valid OPENCODE_API_KEY assignment regex"),
            Regex::new(r"(?m)^\s*(OPENCODE_GO_API_KEY\s*=\s*)[^\r\n]+")
                .expect("valid OPENCODE_GO_API_KEY assignment regex"),
            Regex::new(r"(?m)^\s*(ZAI_API_KEY\s*=\s*)[^\r\n]+")
                .expect("valid ZAI_API_KEY assignment regex"),
            Regex::new(r"(?m)^\s*(CHUTES_API_KEY\s*=\s*)[^\r\n]+")
                .expect("valid CHUTES_API_KEY assignment regex"),
            Regex::new(r"(?m)^\s*(CEREBRAS_API_KEY\s*=\s*)[^\r\n]+")
                .expect("valid CEREBRAS_API_KEY assignment regex"),
            Regex::new(r"(?m)^\s*(OPENAI_COMPAT_API_KEY\s*=\s*)[^\r\n]+")
                .expect("valid OPENAI_COMPAT_API_KEY assignment regex"),
            Regex::new(r"(?m)^\s*(ANTHROPIC_API_KEY\s*=\s*)[^\r\n]+")
                .expect("valid ANTHROPIC_API_KEY assignment regex"),
            Regex::new(r"(?m)^\s*(OPENAI_API_KEY\s*=\s*)[^\r\n]+")
                .expect("valid OPENAI_API_KEY assignment regex"),
            Regex::new(r"(?m)^\s*(AZURE_OPENAI_API_KEY\s*=\s*)[^\r\n]+")
                .expect("valid AZURE_OPENAI_API_KEY assignment regex"),
            Regex::new(r"(?m)^\s*(CURSOR_API_KEY\s*=\s*)[^\r\n]+")
                .expect("valid CURSOR_API_KEY assignment regex"),
            Regex::new(r"(?m)^\s*(GITHUB_TOKEN\s*=\s*)[^\r\n]+")
                .expect("valid GITHUB_TOKEN assignment regex"),
        ]
    });

    let mut redacted = text.to_string();
    let mut redacted_keys: HashSet<String> = [
        "OPENROUTER_API_KEY",
        "OPENCODE_API_KEY",
        "OPENCODE_GO_API_KEY",
        "ZAI_API_KEY",
        "CHUTES_API_KEY",
        "CEREBRAS_API_KEY",
        "OPENAI_COMPAT_API_KEY",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "AZURE_OPENAI_API_KEY",
        "CURSOR_API_KEY",
        "GITHUB_TOKEN",
    ]
    .iter()
    .map(|k| (*k).to_string())
    .collect();

    for re in direct_patterns {
        redacted = re.replace_all(&redacted, "[REDACTED_SECRET]").into_owned();
    }

    for re in assignment_patterns {
        redacted = re
            .replace_all(&redacted, "${1}[REDACTED_SECRET]")
            .into_owned();
    }

    // Also redact custom API key variable names configured at runtime.
    for source in [
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENAI_COMPAT_API_KEY_NAME",
    ] {
        let Some(key_name) = std::env::var(source)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
        else {
            continue;
        };

        if !key_name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        {
            continue;
        }
        if !redacted_keys.insert(key_name.clone()) {
            continue;
        }

        let pattern = format!(r"(?m)^\s*({}\s*=\s*)[^\r\n]+", regex::escape(&key_name));
        if let Ok(re) = Regex::new(&pattern) {
            redacted = re
                .replace_all(&redacted, "${1}[REDACTED_SECRET]")
                .into_owned();
        }
    }

    redacted
}

/// Tool definition for the API
#[derive(Debug, Clone, Serialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// A tool call from the model
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ToolCall {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub input: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
}

/// Connection phase for status bar transparency
#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionPhase {
    /// Refreshing OAuth token
    Authenticating,
    /// TCP + TLS connection to API
    Connecting,
    /// HTTP request sent, waiting for first response byte
    WaitingForResponse,
    /// First byte received, stream is active
    Streaming,
    /// Retrying after a transient error
    Retrying { attempt: u32, max: u32 },
}

impl std::fmt::Display for ConnectionPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectionPhase::Authenticating => write!(f, "authenticating"),
            ConnectionPhase::Connecting => write!(f, "connecting"),
            ConnectionPhase::WaitingForResponse => write!(f, "waiting for response"),
            ConnectionPhase::Streaming => write!(f, "streaming"),
            ConnectionPhase::Retrying { attempt, max } => {
                write!(f, "retrying ({}/{})", attempt, max)
            }
        }
    }
}

/// Streaming event from provider
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// Text content delta
    TextDelta(String),
    /// Tool use started
    ToolUseStart { id: String, name: String },
    /// Tool input delta (JSON fragment)
    ToolInputDelta(String),
    /// Tool use complete
    ToolUseEnd,
    /// Tool result from provider (provider already executed the tool)
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    /// Extended thinking started
    ThinkingStart,
    /// Extended thinking delta (reasoning content)
    ThinkingDelta(String),
    /// Extended thinking ended
    ThinkingEnd,
    /// Extended thinking completed with duration
    ThinkingDone { duration_secs: f64 },
    /// Message complete (may have stop reason)
    MessageEnd { stop_reason: Option<String> },
    /// Token usage update
    TokenUsage {
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        cache_read_input_tokens: Option<u64>,
        cache_creation_input_tokens: Option<u64>,
    },
    /// Active transport/connection type for this stream
    ConnectionType { connection: String },
    /// Connection phase update (for status bar transparency)
    ConnectionPhase { phase: ConnectionPhase },
    /// Error occurred
    Error {
        message: String,
        /// Seconds until rate limit resets (if this is a rate limit error)
        retry_after_secs: Option<u64>,
    },
    /// Provider session ID (for conversation resume)
    SessionId(String),
    /// Compaction occurred (context was summarized)
    Compaction {
        trigger: String,
        pre_tokens: Option<u64>,
    },
    /// Upstream provider info (e.g., which provider OpenRouter routed to)
    UpstreamProvider { provider: String },
    /// Native tool call from a provider bridge that needs execution by jcode
    NativeToolCall {
        request_id: String,
        tool_name: String,
        input: serde_json::Value,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_tool_id_alphanumeric_passthrough() {
        assert_eq!(
            sanitize_tool_id("toolu_01XFDUDYJgAACzvnptvVer6u"),
            "toolu_01XFDUDYJgAACzvnptvVer6u"
        );
        assert_eq!(sanitize_tool_id("call_abc123"), "call_abc123");
        assert_eq!(
            sanitize_tool_id("call_1234567890_9876543210"),
            "call_1234567890_9876543210"
        );
    }

    #[test]
    fn sanitize_tool_id_hyphens_passthrough() {
        assert_eq!(sanitize_tool_id("call-abc-123"), "call-abc-123");
        assert_eq!(
            sanitize_tool_id("tool_use-id_with-mixed"),
            "tool_use-id_with-mixed"
        );
    }

    #[test]
    fn sanitize_tool_id_replaces_dots() {
        assert_eq!(
            sanitize_tool_id("chatcmpl-abc.def.ghi"),
            "chatcmpl-abc_def_ghi"
        );
        assert_eq!(sanitize_tool_id("call.123"), "call_123");
    }

    #[test]
    fn sanitize_tool_id_replaces_colons() {
        assert_eq!(sanitize_tool_id("call:123:456"), "call_123_456");
    }

    #[test]
    fn sanitize_tool_id_replaces_special_chars() {
        assert_eq!(
            sanitize_tool_id("id@with#special$chars"),
            "id_with_special_chars"
        );
        assert_eq!(sanitize_tool_id("id with spaces"), "id_with_spaces");
    }

    #[test]
    fn sanitize_tool_id_empty_returns_unknown() {
        assert_eq!(sanitize_tool_id(""), "unknown");
    }

    #[test]
    fn sanitize_tool_id_copilot_to_anthropic() {
        assert_eq!(
            sanitize_tool_id("chatcmpl-BF2xX.tool_call.0"),
            "chatcmpl-BF2xX_tool_call_0"
        );
    }

    #[test]
    fn sanitize_tool_id_already_valid_unchanged() {
        let valid_ids = [
            "toolu_01XFDUDYJgAACzvnptvVer6u",
            "call_abc123",
            "fallback_text_call_call_1234567890_9876543210",
            "tool_123",
            "a",
            "A",
            "0",
            "_",
            "-",
            "a-b_c",
        ];
        for id in valid_ids {
            assert_eq!(sanitize_tool_id(id), id, "ID '{}' should be unchanged", id);
        }
    }

    #[test]
    fn redact_secrets_redacts_known_direct_token_formats() {
        let input = "access=sk-ant-oat01-ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789\nopenrouter=sk-or-v1-abcdefghijklmnopqrstuvwxyz0123456789\ngithub=ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123\n";
        let out = redact_secrets(input);
        assert!(!out.contains("sk-ant-oat01-"));
        assert!(!out.contains("sk-or-v1-"));
        assert!(!out.contains("ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123"));
        assert!(out.matches("[REDACTED_SECRET]").count() >= 3);
    }

    #[test]
    fn redact_secrets_redacts_env_style_assignments() {
        let input = "OPENROUTER_API_KEY=sk-or-v1-abc123abc123abc123abc123\nOPENCODE_API_KEY=oc_test_secret\nOPENCODE_GO_API_KEY=ocgo_test_secret\nZAI_API_KEY=zai_secret\nCHUTES_API_KEY=chutes_secret\nCEREBRAS_API_KEY=cerebras_secret\nOPENAI_COMPAT_API_KEY=compat_secret\nCURSOR_API_KEY='my_cursor_secret_value'\nOPENAI_API_KEY=sk-test-openai-example\nAZURE_OPENAI_API_KEY=azure-openai-secret\n";
        let out = redact_secrets(input);
        assert!(out.contains("OPENROUTER_API_KEY=[REDACTED_SECRET]"));
        assert!(out.contains("OPENCODE_API_KEY=[REDACTED_SECRET]"));
        assert!(out.contains("OPENCODE_GO_API_KEY=[REDACTED_SECRET]"));
        assert!(out.contains("ZAI_API_KEY=[REDACTED_SECRET]"));
        assert!(out.contains("CHUTES_API_KEY=[REDACTED_SECRET]"));
        assert!(out.contains("CEREBRAS_API_KEY=[REDACTED_SECRET]"));
        assert!(out.contains("OPENAI_COMPAT_API_KEY=[REDACTED_SECRET]"));
        assert!(out.contains("CURSOR_API_KEY=[REDACTED_SECRET]"));
        assert!(out.contains("OPENAI_API_KEY=[REDACTED_SECRET]"));
        assert!(out.contains("AZURE_OPENAI_API_KEY=[REDACTED_SECRET]"));
        assert!(!out.contains("my_cursor_secret_value"));
    }

    #[test]
    fn redact_secrets_redacts_runtime_key_assignment() {
        let key_var = "JCODE_OPENAI_COMPAT_API_KEY_NAME";
        let prev = std::env::var(key_var).ok();
        crate::env::set_var(key_var, "GROQ_API_KEY");

        let input = "GROQ_API_KEY=my_secret_token_value";
        let out = redact_secrets(input);
        assert_eq!(out, "GROQ_API_KEY=[REDACTED_SECRET]");

        if let Some(v) = prev {
            crate::env::set_var(key_var, v);
        } else {
            crate::env::remove_var(key_var);
        }
    }

    #[test]
    fn redact_secrets_redacts_mixed_case_token_assignments() {
        let input = "my_token=ya29.ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        let out = redact_secrets(input);
        assert!(out.contains("[REDACTED_SECRET]"));
        assert!(!out.contains("ya29.ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"));
    }

    #[test]
    fn redact_secrets_leaves_normal_output_unchanged() {
        let input = "Found 5 files\nNo auth errors\nDone.";
        assert_eq!(redact_secrets(input), input);
    }

    #[test]
    fn format_timestamp_is_stable_utc_rfc3339() {
        let ts = chrono::DateTime::parse_from_rfc3339("2025-03-15T02:24:13.250Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(Message::format_timestamp(&ts), "2025-03-15T02:24:13.250Z");
    }

    #[test]
    fn with_timestamps_prepends_utc_prefix_to_user_text() {
        let ts = chrono::DateTime::parse_from_rfc3339("2025-03-15T02:24:03Z")
            .unwrap()
            .with_timezone(&Utc);
        let stamped = Message::with_timestamps(&[Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "hello".to_string(),
                cache_control: None,
            }],
            timestamp: Some(ts),
            tool_duration_ms: None,
        }]);
        match &stamped[0].content[0] {
            ContentBlock::Text { text, .. } => {
                assert_eq!(text, "[2025-03-15T02:24:03.000Z] hello");
            }
            other => panic!("expected text block, got {other:?}"),
        }
    }

    #[test]
    fn with_timestamps_adds_tool_timing_header_with_duration() {
        let ts = chrono::DateTime::parse_from_rfc3339("2025-03-15T02:24:13Z")
            .unwrap()
            .with_timezone(&Utc);
        let stamped = Message::with_timestamps(&[Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "call_1".to_string(),
                content: "ok".to_string(),
                is_error: None,
            }],
            timestamp: Some(ts),
            tool_duration_ms: Some(3_200),
        }]);
        match &stamped[0].content[0] {
            ContentBlock::ToolResult { content, .. } => {
                assert_eq!(
                    content,
                    "[tool timing: start=2025-03-15T02:24:09.800Z finish=2025-03-15T02:24:13.000Z duration=3.2s] ok"
                );
            }
            other => panic!("expected tool result block, got {other:?}"),
        }
    }

    #[test]
    fn with_timestamps_skips_internal_system_reminders() {
        let ts = chrono::DateTime::parse_from_rfc3339("2025-03-15T02:24:13Z")
            .unwrap()
            .with_timezone(&Utc);
        let original = Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "<system-reminder>\ninternal\n</system-reminder>".to_string(),
                cache_control: None,
            }],
            timestamp: Some(ts),
            tool_duration_ms: None,
        };
        let stamped = Message::with_timestamps(&[original.clone()]);
        match &stamped[0].content[0] {
            ContentBlock::Text { text, .. } => {
                assert_eq!(text, "<system-reminder>\ninternal\n</system-reminder>");
            }
            other => panic!("expected text block, got {other:?}"),
        }
    }
}
