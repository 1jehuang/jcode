use serde::Deserialize;
use serde_json::Value;

// ---------------------------------------------------------------------------
// CLI output parsing types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum CliOutput {
    System {
        #[serde(default)]
        session_id: Option<String>,
    },
    StreamEvent {
        event: Value,
        #[serde(default, rename = "session_id")]
        _session_id: Option<String>,
    },
    Assistant {
        message: CliMessage,
        #[serde(default, rename = "session_id")]
        _session_id: Option<String>,
    },
    User {
        message: CliMessage,
        #[serde(default, rename = "session_id")]
        _session_id: Option<String>,
    },
    Result {
        #[serde(default)]
        is_error: bool,
        #[serde(default)]
        usage: Option<UsageInfo>,
        #[serde(default)]
        session_id: Option<String>,
    },
    Error {
        message: String,
        #[serde(default)]
        retry_after_secs: Option<u64>,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
pub(crate) struct CliMessage {
    pub(crate) content: Value,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum SdkContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: Option<Value>,
        #[serde(default)]
        is_error: Option<bool>,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
pub(crate) enum SseEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: Value },
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        #[serde(rename = "index")]
        _index: usize,
        content_block: ContentBlockInfo,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta {
        #[serde(rename = "index")]
        _index: usize,
        delta: DeltaInfo,
    },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop {
        #[serde(rename = "index")]
        _index: usize,
    },
    #[serde(rename = "message_delta")]
    MessageDelta {
        delta: MessageDeltaInfo,
        #[serde(default)]
        usage: Option<UsageInfo>,
    },
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "error")]
    Error { error: ErrorInfo },
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
pub(crate) enum ContentBlockInfo {
    #[serde(rename = "text")]
    Text {
        #[serde(rename = "text")]
        _text: String,
    },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
    #[serde(rename = "thinking")]
    Thinking {
        #[serde(rename = "thinking")]
        _thinking: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
pub(crate) enum DeltaInfo {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta {
        #[serde(rename = "thinking")]
        _thinking: String,
    },
    #[serde(rename = "signature_delta")]
    SignatureDelta {
        #[serde(rename = "signature")]
        _signature: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize, Debug)]
pub(crate) struct UsageInfo {
    #[serde(default)]
    pub(crate) input_tokens: Option<u64>,
    #[serde(default)]
    pub(crate) output_tokens: Option<u64>,
    #[serde(default)]
    pub(crate) cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    pub(crate) cache_read_input_tokens: Option<u64>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct MessageDeltaInfo {
    pub(crate) stop_reason: Option<String>,
}

#[derive(Deserialize, Debug)]
pub(crate) struct ErrorInfo {
    pub(crate) message: String,
    #[serde(default)]
    pub(crate) retry_after_secs: Option<u64>,
    #[serde(default, rename = "status_code")]
    pub(crate) _status_code: Option<u16>,
    #[serde(default, rename = "error_type")]
    pub(crate) _error_type: Option<String>,
}
