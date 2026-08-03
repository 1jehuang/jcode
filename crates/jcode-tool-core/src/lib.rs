use anyhow::Result;
use async_trait::async_trait;
use jcode_agent_runtime::InterruptSignal;
use jcode_message_types::ToolDefinition;
use jcode_tool_types::ToolOutput;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub const TOOL_INTENT_DESCRIPTION: &str =
    "Required short label shown in the UI: why this call is being made.";

/// Input key a caller sets to accept the token cost of an oversized result.
///
/// The context guard withholds any tool result too large for the remaining
/// context and states its token cost. Setting this repeats the call and spends
/// that cost deliberately. Kept in sync with the registry constant of the same
/// name, which reads the flag off raw tool input.
pub const ACCEPT_LARGE_OUTPUT_KEY: &str = "accept_large_output";

/// Deliberately terse: this rides on every tool schema on every request, so
/// each word is paid forever. The full explanation lives in the refusal
/// message, which is only ever shown when it is actually relevant.
pub const ACCEPT_LARGE_OUTPUT_DESCRIPTION: &str =
    "Re-run accepting the stated token cost of a withheld result.";

pub fn intent_schema_property() -> Value {
    serde_json::json!({
        "type": "string",
        "description": TOOL_INTENT_DESCRIPTION,
    })
}

pub fn accept_large_output_schema_property() -> Value {
    serde_json::json!({
        "type": "boolean",
        "description": ACCEPT_LARGE_OUTPUT_DESCRIPTION,
    })
}

/// Ensure a tool parameter schema declares the shared `intent` property and
/// marks it required. Applied centrally when converting tools to provider
/// definitions so every tool (including MCP proxies) asks the model for an
/// intent without each tool wiring it manually.
///
/// The optional `accept_large_output` escape hatch is added the same way. Any
/// tool can produce a result too large to return, so documenting it per tool
/// would mean editing dozens of schemas and missing MCP proxies entirely.
pub fn ensure_intent_in_schema(mut schema: Value) -> Value {
    let Some(object) = schema.as_object_mut() else {
        return schema;
    };
    // Only touch object-shaped parameter schemas.
    let is_object_schema = object
        .get("type")
        .and_then(|t| t.as_str())
        .map(|t| t == "object")
        .unwrap_or_else(|| object.contains_key("properties"));
    if !is_object_schema {
        return schema;
    }

    let properties = object
        .entry("properties")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if let Some(properties) = properties.as_object_mut() {
        properties
            .entry("intent")
            .or_insert_with(intent_schema_property);
        // Optional, so it is deliberately not added to `required`.
        properties
            .entry(ACCEPT_LARGE_OUTPUT_KEY)
            .or_insert_with(accept_large_output_schema_property);
    } else {
        return schema;
    }

    match object.get_mut("required") {
        Some(Value::Array(required)) => {
            if !required.iter().any(|v| v.as_str() == Some("intent")) {
                required.push(Value::String("intent".to_string()));
            }
        }
        _ => {
            object.insert(
                "required".to_string(),
                Value::Array(vec![Value::String("intent".to_string())]),
            );
        }
    }

    schema
}

/// A request for stdin input from a running command.
pub struct StdinInputRequest {
    pub request_id: String,
    pub prompt: String,
    pub is_password: bool,
    pub response_tx: tokio::sync::oneshot::Sender<String>,
}

#[derive(Clone)]
pub struct ToolContext {
    pub session_id: String,
    pub message_id: String,
    pub tool_call_id: String,
    pub working_dir: Option<PathBuf>,
    pub stdin_request_tx: Option<tokio::sync::mpsc::UnboundedSender<StdinInputRequest>>,
    pub graceful_shutdown_signal: Option<InterruptSignal>,
    pub execution_mode: ToolExecutionMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExecutionMode {
    AgentTurn,
    Direct,
}

impl ToolContext {
    pub fn for_subcall(&self, tool_call_id: String) -> Self {
        Self {
            session_id: self.session_id.clone(),
            message_id: self.message_id.clone(),
            tool_call_id,
            working_dir: self.working_dir.clone(),
            stdin_request_tx: self.stdin_request_tx.clone(),
            graceful_shutdown_signal: self.graceful_shutdown_signal.clone(),
            execution_mode: self.execution_mode,
        }
    }

    pub fn resolve_path(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else if let Some(ref base) = self.working_dir {
            base.join(path)
        } else {
            path.to_path_buf()
        }
    }
}

/// A tool that can be executed by the agent.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name (must match what's sent to the API).
    fn name(&self) -> &str;

    /// Human-readable description.
    fn description(&self) -> &str;

    /// JSON Schema for the input parameters.
    fn parameters_schema(&self) -> Value;

    /// Execute the tool with the given input.
    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput>;

    /// Convert to API tool definition.
    fn to_definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            input_schema: ensure_intent_in_schema(self.parameters_schema()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_intent_adds_property_and_required() {
        let schema = serde_json::json!({
            "type": "object",
            "required": ["command"],
            "properties": {
                "command": {"type": "string"}
            }
        });
        let out = ensure_intent_in_schema(schema);
        assert!(out["properties"]["intent"].is_object());
        let required: Vec<_> = out["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(required.contains(&"command"));
        assert!(required.contains(&"intent"));
    }

    #[test]
    fn ensure_intent_creates_required_array_when_missing() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {}
        });
        let out = ensure_intent_in_schema(schema);
        assert_eq!(out["required"], serde_json::json!(["intent"]));
    }

    #[test]
    fn ensure_intent_preserves_existing_intent_property() {
        let schema = serde_json::json!({
            "type": "object",
            "required": ["intent"],
            "properties": {
                "intent": {"type": "string", "description": "custom"}
            }
        });
        let out = ensure_intent_in_schema(schema);
        assert_eq!(out["properties"]["intent"]["description"], "custom");
        assert_eq!(
            out["required"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|v| v.as_str() == Some("intent"))
                .count(),
            1
        );
    }

    #[test]
    fn ensure_intent_skips_non_object_schemas() {
        let schema = serde_json::json!({"type": "string"});
        let out = ensure_intent_in_schema(schema.clone());
        assert_eq!(out, schema);
    }
}

#[cfg(test)]
mod escape_hatch_tests {
    use super::*;

    #[test]
    fn injects_the_escape_hatch_into_any_object_schema() {
        // MCP tools are built from remote definitions and never edit their own
        // schemas, so they can only advertise the flag if injection is central.
        // A schema shaped like an MCP proxy's proves the mechanism.
        let mcp_shaped = serde_json::json!({
            "type": "object",
            "required": ["path"],
            "properties": { "path": { "type": "string" } }
        });
        let out = ensure_intent_in_schema(mcp_shaped);
        assert_eq!(
            out["properties"][ACCEPT_LARGE_OUTPUT_KEY]["type"], "boolean",
            "every object schema must advertise the escape hatch"
        );
        // Optional by design: requiring it would make the model answer a token
        // budget question on every call.
        let required: Vec<&str> = out["required"]
            .as_array()
            .expect("required array")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(required.contains(&"intent"));
        assert!(!required.contains(&ACCEPT_LARGE_OUTPUT_KEY));
    }

    #[test]
    fn never_overwrites_a_schema_that_declares_the_flag_itself() {
        let custom = serde_json::json!({
            "type": "object",
            "properties": {
                ACCEPT_LARGE_OUTPUT_KEY: { "type": "boolean", "description": "custom" }
            }
        });
        let out = ensure_intent_in_schema(custom);
        assert_eq!(
            out["properties"][ACCEPT_LARGE_OUTPUT_KEY]["description"], "custom",
            "a tool's own declaration must survive injection"
        );
    }

    #[test]
    fn the_schema_key_matches_what_the_guard_reads() {
        // The registry reads this exact constant off raw tool input. If the two
        // ever diverge, the flag would be advertised but never honored, which is
        // worse than not offering it at all.
        assert_eq!(ACCEPT_LARGE_OUTPUT_KEY, "accept_large_output");
    }
}
