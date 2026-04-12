use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

pub struct InvalidTool;

impl InvalidTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct InvalidInput {
    tool: String,
    error: String,
}

#[async_trait]
impl Tool for InvalidTool {
    fn name(&self) -> &str {
        "invalid"
    }

    fn description(&self) -> &str {
        "Report invalid tool usage. Use only when a tool call is malformed."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["tool", "error"],
            "properties": {
                "tool": {
                    "type": "string",
                    "description": "Tool name."
                },
                "error": {
                    "type": "string",
                    "description": "Validation error."
                }
            }
        })
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let params: InvalidInput = serde_json::from_value(input)?;
        Ok(ToolOutput::new(format!(
            "Invalid tool invocation for '{}': {}",
            params.tool, params.error
        )))
    }
}
