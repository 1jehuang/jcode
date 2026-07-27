//! Deserialization types for the Anthropic streaming (SSE) response format.
//!
//! Extracted from `lib.rs` so the runtime entry point stays within the
//! oversized-file ratchet.

use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct MessageStartEvent {
    pub(crate) message: MessageStartMessage,
}

#[derive(Deserialize)]
pub(crate) struct MessageStartMessage {
    #[serde(default)]
    pub(crate) model: Option<String>,
    pub(crate) usage: Option<UsageInfo>,
}

#[derive(Deserialize)]
pub(crate) struct ContentBlockStartEvent {
    #[serde(rename = "index")]
    pub(crate) _index: u32,
    pub(crate) content_block: ApiContentBlockStart,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
pub(crate) enum ApiContentBlockStart {
    #[serde(rename = "text")]
    Text {
        #[serde(rename = "text")]
        _text: String,
    },
    #[serde(rename = "thinking")]
    Thinking {
        #[serde(default, rename = "thinking")]
        _thinking: String,
        #[serde(default, rename = "signature")]
        _signature: Option<String>,
    },
    #[serde(rename = "redacted_thinking")]
    RedactedThinking {
        #[serde(default, rename = "data")]
        _data: String,
    },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
    /// A block type this build does not recognize (for example a newer
    /// server-side tool block). Kept as an explicit catch-all so the
    /// surrounding `content_block_start` event still deserializes instead of
    /// being dropped whole.
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
pub(crate) struct ContentBlockDeltaEvent {
    #[serde(rename = "index")]
    pub(crate) _index: u32,
    pub(crate) delta: ApiDelta,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
pub(crate) enum ApiDelta {
    #[serde(rename = "text_delta")]
    Text { text: String },
    #[serde(rename = "input_json_delta")]
    InputJson { partial_json: String },
    #[serde(rename = "thinking_delta")]
    Thinking { thinking: String },
    #[serde(rename = "signature_delta")]
    Signature { signature: String },
}

#[derive(Deserialize)]
pub(crate) struct MessageDeltaEvent {
    pub(crate) delta: MessageDeltaDelta,
    pub(crate) usage: Option<UsageInfo>,
}

#[derive(Deserialize)]
pub(crate) struct MessageDeltaDelta {
    pub(crate) stop_reason: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct UsageInfo {
    pub(crate) input_tokens: Option<u32>,
    pub(crate) output_tokens: Option<u32>,
    pub(crate) cache_read_input_tokens: Option<u32>,
    pub(crate) cache_creation_input_tokens: Option<u32>,
    pub(crate) service_tier: Option<String>,
}
