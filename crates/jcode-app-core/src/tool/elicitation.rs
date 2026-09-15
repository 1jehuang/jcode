//! Native elicitation tool — replaces the external phinbox MCP dependency.
//!
//! When the agent needs structured user input (multi-choice, text, boolean),
//! it calls this tool which sends a request through the global elicitation
//! channel. The TUI renders a modal overlay, collects the user's response,
//! and sends it back through the oneshot channel.

use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use jcode_tool_core::{ElicitMessage, ElicitRequest, elicit_channel};
use serde_json::{Value, json};

pub struct ElicitateMcpTool;

impl ElicitateMcpTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(serde::Deserialize)]
struct ElicitInput {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    field: Option<Value>,
    #[serde(default)]
    intent: Option<String>,
    #[serde(default)]
    question: Option<String>,
    #[serde(default)]
    notes: Option<Value>,
    #[serde(default)]
    buttons: Option<Value>,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    timeout_secs: Option<u32>,
    #[serde(default)]
    urgency: Option<String>,
}

#[async_trait]
impl Tool for ElicitateMcpTool {
    fn name(&self) -> &str {
        "elicitate_mcp"
    }

    fn description(&self) -> &str {
        "Render a native OS popup and block until the human operator responds \
         (or the prompt times out). Use this whenever an autonomous agent needs \
         a single, structured decision from a human: a confirmation, a \
         multi-choice selection, a secret, a disambiguation. Returns a typed \
         JSON ElicitResponse."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["field", "title", "intent"],
            "properties": {
                "intent": super::intent_schema_property(),
                "title": {
                    "type": "string",
                    "description": "One-line title."
                },
                "question": {
                    "type": "string",
                    "description": "Multi-line body explaining context.",
                    "default": ""
                },
                "field": {
                    "description": "The input field configuration.",
                    "oneOf": [
                        {"type": "object", "properties": {"kind": {"const": "text"}, "label": {"type": "string"}, "default": {"type": "string"}, "placeholder": {"type": "string"}, "max_length": {"type": "integer"}, "secret": {"type": "boolean"}}, "required": ["kind", "label"]},
                        {"type": "object", "properties": {"kind": {"const": "long_text"}, "label": {"type": "string"}, "default": {"type": "string"}, "max_length": {"type": "integer"}}, "required": ["kind", "label"]},
                        {"type": "object", "properties": {"kind": {"const": "integer"}, "label": {"type": "string"}, "default": {"type": "integer"}, "min": {"type": "integer"}, "max": {"type": "integer"}}, "required": ["kind", "label"]},
                        {"type": "object", "properties": {"kind": {"const": "choice"}, "label": {"type": "string"}, "options": {"type": "array", "items": {"type": "object", "properties": {"label": {"type": "string"}, "value": {"type": "string"}, "description": {"type": "string"}}, "required": ["label", "value"]}}, "default_index": {"type": "integer"}}, "required": ["kind", "label", "options"]},
                        {"type": "object", "properties": {"kind": {"const": "boolean"}, "label": {"type": "string"}, "default": {"type": "boolean"}}, "required": ["kind", "label"]},
                        {"type": "object", "properties": {"kind": {"const": "date_time"}, "label": {"type": "string"}, "default": {"type": "string"}, "picker_kind": {"type": "string", "enum": ["date", "time", "datetime"]}}, "required": ["kind", "label"]}
                    ]
                },
                "notes": {
                    "description": "The optional notes / free-text box.",
                    "type": "object",
                    "properties": {
                        "label": {"type": "string"},
                        "default": {"type": "string"},
                        "max_length": {"type": "integer"},
                        "required": {"type": "boolean"}
                    }
                },
                "buttons": {
                    "description": "Custom button labels.",
                    "type": "object",
                    "properties": {
                        "confirm": {"type": "string", "description": "Confirm button label. Default: \"OK\"."},
                        "cancel": {"type": "string", "description": "Cancel button label. Default: \"Cancel\"."},
                        "default_is_cancel": {"type": "boolean", "description": "If true, swap which button is the default."}
                    }
                },
                "request_id": {
                    "type": "string",
                    "description": "Optional request ID for correlation."
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Timeout in seconds.",
                    "default": 600,
                    "minimum": 0
                },
                "urgency": {
                    "type": "string",
                    "description": "Urgency hint — affects icon and sound on GUI popups.",
                    "enum": ["info", "warning", "error", "secret"],
                    "default": "info"
                }
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: ElicitInput = serde_json::from_value(input)?;

        let field = params
            .field
            .ok_or_else(|| anyhow::anyhow!("field parameter is required"))?;

        let request = ElicitRequest {
            title: params.title.unwrap_or_default(),
            intent: params.intent.unwrap_or_default(),
            field,
            question: params.question,
            notes: params.notes,
            buttons: params.buttons,
            request_id: params.request_id,
            timeout_secs: params.timeout_secs.unwrap_or(600),
            urgency: params.urgency.unwrap_or_else(|| "info".to_string()),
        };

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();

        let msg = ElicitMessage {
            request,
            response_tx,
        };

        // Send request through global channel to TUI
        elicit_channel::try_send(msg)
            .map_err(|e| anyhow::anyhow!("failed to send elicitation request: {e}"))?;

        // Wait for user response (with timeout)
        let timeout = params.timeout_secs.unwrap_or(600);
        let response =
            tokio::time::timeout(std::time::Duration::from_secs(timeout as u64), response_rx)
                .await
                .map_err(|_| anyhow::anyhow!("elicitation timed out after {timeout}s"))?
                .map_err(|_| anyhow::anyhow!("elicitation response channel closed"))?;

        let output = json!({
            "action": response.action,
            "value": response.value,
            "notes": response.notes,
        });

        Ok(ToolOutput {
            output: output.to_string(),
            title: None,
            metadata: None,
            images: vec![],
        })
    }
}
