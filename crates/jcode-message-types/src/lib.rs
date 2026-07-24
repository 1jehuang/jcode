#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ToolCall {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub input: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<String>,
    /// Gemini 3 thought signature attached to this tool call, replayed on
    /// later turns so the Cloud Code backend accepts the function call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
}

/// Tool definition advertised to model providers.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ToolDefinition {
    pub name: String,
    /// Prompt-visible text sent to the model by provider adapters.
    /// Approximate prompt cost: description.len() / 4. Use
    /// ToolDefinition::description_token_estimate() when reviewing tool bloat.
    pub description: String,
    pub input_schema: serde_json::Value,
}

impl ToolDefinition {
    /// Serialized size of the full tool definition payload sent to providers.
    pub fn prompt_chars(&self) -> usize {
        serde_json::json!({
            "name": self.name,
            "description": self.description,
            "input_schema": self.input_schema,
        })
        .to_string()
        .len()
    }

    /// Approximate prompt-token cost of this tool's top-level description.
    ///
    /// This uses jcode's standard chars/4 heuristic, matching other token
    /// budget estimates in the codebase.
    pub fn description_token_estimate(&self) -> usize {
        estimate_tokens(&self.description)
    }

    /// Approximate prompt-token cost of the full tool definition payload.
    pub fn prompt_token_estimate(&self) -> usize {
        estimate_tokens(
            &serde_json::json!({
                "name": self.name,
                "description": self.description,
                "input_schema": self.input_schema,
            })
            .to_string(),
        )
    }

    pub fn aggregate_prompt_chars(defs: &[ToolDefinition]) -> usize {
        defs.iter().map(Self::prompt_chars).sum()
    }

    pub fn aggregate_prompt_token_estimate(defs: &[ToolDefinition]) -> usize {
        defs.iter().map(Self::prompt_token_estimate).sum()
    }
}

fn estimate_tokens(s: &str) -> usize {
    const APPROX_CHARS_PER_TOKEN: usize = 4;
    s.len() / APPROX_CHARS_PER_TOKEN
}

/// Role in conversation
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

/// A message in the conversation
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_duration_ms: Option<u64>,
}

/// Cache control metadata for prompt caching
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
    /// History-only reasoning trace. Captured purely so the model's thinking is
    /// preserved in the transcript for later recall/debugging. Unlike
    /// `Reasoning`/`AnthropicThinking`/`OpenAIReasoning`, this block is never
    /// replayed back to any provider, so it carries no token cost on later turns
    /// and cannot trigger provider-side "unsigned thinking" rejections.
    ReasoningTrace {
        text: String,
    },
    /// Anthropic signed thinking content. Anthropic requires the signature when
    /// replaying thinking blocks in future request context.
    AnthropicThinking {
        thinking: String,
        signature: String,
    },
    /// OpenAI Responses reasoning item. When `store=false`, OpenAI returns
    /// encrypted reasoning content so clients can replay reasoning state in
    /// future turns by sending the native reasoning item back in `input`.
    OpenAIReasoning {
        id: String,
        summary: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        encrypted_content: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<String>,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
        /// Gemini 3 "thought signature" for this function call. The Antigravity
        /// / Cloud Code backend requires the original signature to be replayed
        /// on the matching `functionCall` part in subsequent turns, otherwise it
        /// rejects the request ("Function call is missing a thought_signature").
        /// Empty/None for providers that do not use thought signatures.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
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
            timestamp: Some(chrono::Utc::now()),
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
            timestamp: Some(chrono::Utc::now()),
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
            timestamp: Some(chrono::Utc::now()),
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
            timestamp: Some(chrono::Utc::now()),
            tool_duration_ms,
        }
    }

    /// Format a timestamp deterministically in UTC for injection into model-visible content.
    pub fn format_timestamp(ts: &chrono::DateTime<chrono::Utc>) -> String {
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

    pub fn is_internal_system_reminder(&self) -> bool {
        self.content
            .iter()
            .find_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text.trim_start()),
                _ => None,
            })
            .is_some_and(|text| text.starts_with("<system-reminder>"))
    }

    fn should_skip_timestamp_injection(&self) -> bool {
        self.is_internal_system_reminder()
    }

    fn tool_result_tag(&self, ts: &chrono::DateTime<chrono::Utc>) -> String {
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

pub const TOOL_OUTPUT_MISSING_TEXT: &str =
    "Tool output missing (session interrupted before tool execution completed)";

const STABLE_HASH_SEED: u64 = 0xcbf29ce484222325;
const STABLE_HASH_PRIME: u64 = 0x100000001b3;

fn stable_hash_bytes(bytes: &[u8]) -> u64 {
    let mut hash = STABLE_HASH_SEED;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(STABLE_HASH_PRIME);
    }
    hash
}

pub fn extend_stable_hash(acc: u64, next: u64) -> u64 {
    stable_hash_bytes(&[acc.to_le_bytes().as_slice(), next.to_le_bytes().as_slice()].concat())
}

pub fn stable_message_hash(message: &Message) -> u64 {
    match serde_json::to_vec(message) {
        Ok(bytes) => stable_hash_bytes(&bytes),
        Err(_) => stable_hash_bytes(format!("{:?}", message).as_bytes()),
    }
}

/// Project a message down to the fields that actually influence a provider's
/// KV-cache key, dropping harness-only / volatile metadata.
///
/// Providers never receive `Message.timestamp`, `Message.tool_duration_ms`,
/// history-only `ReasoningTrace` blocks, or the positional `cache_control`
/// ephemeral breakpoint markers. Hashing the raw `Message` therefore reports
/// spurious `harness:_prefix_changed` cache misses whenever one of those
/// fields differs on an already-sent boundary message even though the bytes
/// sent upstream are unchanged. A confirmed production instance: persisted
/// memory-injection messages are stored with a slightly later timestamp than
/// the in-flight copy hashed for the request signature, so the raw hash of
/// the same message differed across turns and falsely flagged a prefix edit.
///
/// For user messages the human-readable timestamp is already baked into the
/// text by `Message::with_timestamps` before this projection runs (and
/// system-reminder messages skip timestamp injection entirely), so removing
/// the struct-level `timestamp` field cannot hide a real content change.
pub fn cache_relevant_message_value(message: &Message) -> serde_json::Value {
    let mut value = serde_json::to_value(message).unwrap_or(serde_json::Value::Null);
    if let serde_json::Value::Object(map) = &mut value {
        // Struct-level metadata that is never part of the prompt token stream.
        map.remove("timestamp");
        map.remove("tool_duration_ms");
        if let Some(serde_json::Value::Array(blocks)) = map.get_mut("content") {
            // `ReasoningTrace` is documented as history-only and is never
            // replayed to any provider, so it must not affect the cache key.
            blocks.retain(|block| {
                block.get("type").and_then(|kind| kind.as_str()) != Some("reasoning_trace")
            });
            for block in blocks.iter_mut() {
                if let serde_json::Value::Object(block) = block
                    && block.get("type").and_then(|kind| kind.as_str()) == Some("text")
                {
                    // The ephemeral cache breakpoint marker hops to the newest
                    // message each turn; it marks where caching ends, not the
                    // cached content itself.
                    block.remove("cache_control");
                }
            }
        }
    }
    value
}

/// Cache-relevant projections for a whole message list (see
/// [`cache_relevant_message_value`]).
pub fn cache_relevant_messages(messages: &[Message]) -> Vec<serde_json::Value> {
    messages.iter().map(cache_relevant_message_value).collect()
}

/// Per-message hashes over the cache-relevant projection. These are the
/// hashes compared across turns to decide whether the conversation prefix was
/// mutated (a harness bug) or merely appended to (normal growth). Both the
/// local TUI path and the server event path must use this same projection so
/// prefix-change detection never keys off non-transmitted metadata.
pub fn cache_relevant_message_hashes(messages: &[Message]) -> Vec<u64> {
    messages
        .iter()
        .map(|message| {
            let encoded =
                serde_json::to_string(&cache_relevant_message_value(message)).unwrap_or_default();
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hash::hash(&encoded, &mut hasher);
            std::hash::Hasher::finish(&hasher)
        })
        .collect()
}

pub fn ends_with_fresh_user_turn(messages: &[Message]) -> bool {
    for msg in messages.iter().rev() {
        if msg.role != Role::User {
            return false;
        }

        if msg
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
        {
            return false;
        }

        if msg.content.is_empty() {
            return false;
        }

        let mut saw_user_text = false;
        for block in &msg.content {
            match block {
                ContentBlock::Text { text, .. } => {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() && !trimmed.starts_with("<system-reminder>") {
                        saw_user_text = true;
                    }
                }
                _ => return false,
            }
        }

        if saw_user_text {
            return true;
        }

        if msg.is_internal_system_reminder() {
            continue;
        }

        return false;
    }

    false
}

fn is_fresh_user_text_message(message: &Message) -> bool {
    if message.role != Role::User {
        return false;
    }

    let mut saw_user_text = false;
    for block in &message.content {
        match block {
            ContentBlock::Text { text, .. } => {
                let trimmed = text.trim();
                if !trimmed.is_empty() && !trimmed.starts_with("<system-reminder>") {
                    saw_user_text = true;
                }
            }
            ContentBlock::Image { .. } => {}
            _ => return false,
        }
    }

    saw_user_text
}

fn dynamic_system_context_message(system_dynamic: &str) -> Option<Message> {
    let trimmed = system_dynamic.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(Message::user(&format!(
        "<system-reminder>\n{}\n</system-reminder>",
        trimmed
    )))
}

/// Insert dynamic system context after the latest fresh user prompt without
/// disturbing the stable cached history prefix.
pub fn messages_with_dynamic_system_context(
    messages: &[Message],
    system_dynamic: &str,
) -> Vec<Message> {
    let Some(dynamic_message) = dynamic_system_context_message(system_dynamic) else {
        return messages.to_vec();
    };

    let mut out = messages.to_vec();
    let insert_at = out
        .iter()
        .rposition(is_fresh_user_text_message)
        .map(|idx| idx + 1)
        .unwrap_or(out.len());
    out.insert(insert_at, dynamic_message);
    out
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

impl ToolCall {
    pub fn normalize_input_to_object(input: serde_json::Value) -> serde_json::Value {
        match input {
            serde_json::Value::Object(_) => input,
            _ => serde_json::Value::Object(serde_json::Map::new()),
        }
    }

    pub fn input_as_object(input: &serde_json::Value) -> serde_json::Value {
        Self::normalize_input_to_object(input.clone())
    }

    pub fn parse_streamed_input_to_object(input: &str) -> serde_json::Value {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return serde_json::Value::Object(serde_json::Map::new());
        }

        match serde_json::from_str::<serde_json::Value>(trimmed) {
            Ok(value) => Self::normalize_input_to_object(value),
            Err(_) => serde_json::Value::Null,
        }
    }

    pub fn validation_error(&self) -> Option<String> {
        if self.name.trim().is_empty() {
            return Some("Invalid tool call: tool name must not be empty.".to_string());
        }

        if !self.input.is_object() {
            return Some(format!(
                "Invalid tool call for '{}': arguments must be a JSON object, got {}.",
                self.name,
                json_value_kind(&self.input)
            ));
        }

        None
    }

    pub fn intent_from_input(input: &serde_json::Value) -> Option<String> {
        input
            .get("intent")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|intent| !intent.is_empty())
            .map(ToString::to_string)
    }

    pub fn refresh_intent_from_input(&mut self) {
        self.intent = Self::intent_from_input(&self.input);
    }
}

fn json_value_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
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

/// Connection phase for status bar transparency.
#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionPhase {
    /// Refreshing OAuth token
    Authenticating,
    /// TCP + TLS / websocket transport setup to the API
    Connecting,
    /// Uploading the request (context) and waiting for response headers
    SendingRequest,
    /// Request accepted, waiting for the model's first output byte
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
            ConnectionPhase::SendingRequest => write!(f, "sending request"),
            ConnectionPhase::WaitingForResponse => write!(f, "waiting for response"),
            ConnectionPhase::Streaming => write!(f, "streaming"),
            ConnectionPhase::Retrying { attempt, max } => {
                write!(f, "retrying ({}/{})", attempt, max)
            }
        }
    }
}

/// Streaming event from provider.
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
    /// Gemini 3 thought signature for the most recent tool call. Emitted right
    /// after the matching `ToolUseStart`/`ToolUseEnd` so the agent loop can
    /// persist it on the `ToolUse` block and replay it on later turns.
    ToolUseSignature(String),
    /// Tool result from provider (provider already executed the tool)
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
    /// Image generated by a provider-native image generation tool.
    GeneratedImage {
        id: String,
        path: String,
        metadata_path: Option<String>,
        output_format: String,
        revised_prompt: Option<String>,
    },
    /// Extended thinking started
    ThinkingStart,
    /// Extended thinking delta (reasoning content)
    ThinkingDelta(String),
    /// Provider signature for the current thinking block.
    ThinkingSignatureDelta(String),
    /// Native OpenAI Responses reasoning item for future-turn replay.
    OpenAIReasoning {
        id: String,
        summary: Vec<String>,
        encrypted_content: Option<String>,
        status: Option<String>,
    },
    /// Extended thinking ended
    ThinkingEnd,
    /// Extended thinking completed with duration
    ThinkingDone { duration_secs: f64 },
    /// Message complete (may have stop reason)
    MessageEnd { stop_reason: Option<String> },
    /// A transient transport fault interrupted the stream and the provider is
    /// about to retry the same request from the top. Consumers must discard
    /// any partial output accumulated for the current attempt (text, tool
    /// calls, reasoning) so the replayed stream renders cleanly instead of
    /// duplicating. Safe for jcode HTTP providers because tools only execute
    /// after the stream completes, so a partial attempt has no side effects.
    RetryRollback { attempt: u32, max: u32 },
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
    /// Provider-supplied human-readable transport detail for the status line
    StatusDetail { detail: String },
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
        /// Provider-native compaction artifact, if one was emitted.
        openai_encrypted_content: Option<String>,
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

    fn text_of(message: &Message) -> &str {
        match message.content.first() {
            Some(ContentBlock::Text { text, .. }) => text,
            other => panic!("expected text block, got {:?}", other),
        }
    }

    fn assert_role_text(message: &Message, role: Role, text: &str) {
        assert_eq!(message.role, role);
        assert_eq!(text_of(message), text);
    }

    #[test]
    fn dynamic_context_is_inserted_after_current_user_prompt() {
        let messages = vec![
            Message::user("first user"),
            Message::assistant_text("assistant"),
            Message::user("current user"),
        ];

        let out =
            messages_with_dynamic_system_context(&messages, "# Environment\nTime: 10:00:00 UTC");

        assert_eq!(out.len(), 4);
        assert_eq!(text_of(&out[0]), "first user");
        assert_eq!(text_of(&out[1]), "assistant");
        assert_eq!(text_of(&out[2]), "current user");
        assert!(text_of(&out[3]).starts_with("<system-reminder>\n# Environment"));
    }

    #[test]
    fn dynamic_context_does_not_move_existing_history_prefix() {
        let messages = vec![
            Message::user("stable cached user"),
            Message::assistant_text("stable cached assistant"),
            Message::user("latest prompt"),
        ];

        let out_a = messages_with_dynamic_system_context(&messages, "Time: 10:00:00 UTC");
        let out_b = messages_with_dynamic_system_context(&messages, "Time: 10:00:01 UTC");

        assert_role_text(&out_a[0], Role::User, "stable cached user");
        assert_role_text(&out_a[1], Role::Assistant, "stable cached assistant");
        assert_role_text(&out_b[0], Role::User, "stable cached user");
        assert_role_text(&out_b[1], Role::Assistant, "stable cached assistant");
        assert_role_text(&out_a[2], Role::User, "latest prompt");
        assert_role_text(&out_b[2], Role::User, "latest prompt");
        assert_ne!(text_of(&out_a[3]), text_of(&out_b[3]));
    }

    #[test]
    fn empty_dynamic_context_leaves_messages_unchanged() {
        let messages = vec![Message::user("hello")];
        let out = messages_with_dynamic_system_context(&messages, "\n  \n");
        assert_eq!(out.len(), 1);
        assert_role_text(&out[0], Role::User, "hello");
    }

    #[test]
    fn dynamic_context_appends_when_no_fresh_user_prompt_exists() {
        let messages = vec![
            Message::assistant_text("assistant"),
            Message::user("<system-reminder>\ninternal\n</system-reminder>"),
        ];

        let out = messages_with_dynamic_system_context(&messages, "Time: 10:00:00 UTC");

        assert_eq!(out.len(), 3);
        assert_role_text(&out[0], Role::Assistant, "assistant");
        assert_role_text(
            &out[1],
            Role::User,
            "<system-reminder>\ninternal\n</system-reminder>",
        );
        assert!(text_of(&out[2]).contains("Time: 10:00:00 UTC"));
    }

    #[test]
    fn cache_relevant_hashes_ignore_non_transmitted_metadata() {
        // A persisted memory-injection message is re-serialized on the next
        // turn with a later struct-level timestamp (and possibly a backfilled
        // tool_duration_ms, a hopped cache_control breakpoint, or a
        // history-only ReasoningTrace block). None of that is sent upstream,
        // so the cache-relevant hash must be identical across both copies.
        let sent = Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "<system-reminder>\n# Memory\n</system-reminder>".to_string(),
                cache_control: None,
            }],
            timestamp: Some(chrono::Utc::now()),
            tool_duration_ms: None,
        };
        let persisted = Message {
            role: Role::User,
            content: vec![
                ContentBlock::Text {
                    text: "<system-reminder>\n# Memory\n</system-reminder>".to_string(),
                    cache_control: Some(CacheControl::ephemeral(None)),
                },
                ContentBlock::ReasoningTrace {
                    text: "history-only scratch".to_string(),
                },
            ],
            timestamp: Some(chrono::Utc::now() + chrono::Duration::seconds(7)),
            tool_duration_ms: Some(42),
        };

        assert_eq!(
            cache_relevant_message_hashes(&[sent.clone()]),
            cache_relevant_message_hashes(&[persisted.clone()]),
            "non-transmitted metadata must not change the cache-relevant hash"
        );
        assert_eq!(
            cache_relevant_messages(&[sent]),
            cache_relevant_messages(&[persisted]),
            "cache-relevant projections must be byte-identical"
        );
    }

    #[test]
    fn cache_relevant_hashes_detect_real_content_change() {
        let original = Message::user("original prompt");
        let mut edited = original.clone();
        if let Some(ContentBlock::Text { text, .. }) = edited.content.first_mut() {
            *text = "edited prompt".to_string();
        }

        assert_ne!(
            cache_relevant_message_hashes(&[original]),
            cache_relevant_message_hashes(&[edited]),
            "real content edits must still change the hash"
        );
    }
}
