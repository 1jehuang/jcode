use super::{Registry, Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

const MAX_PARALLEL: usize = 10;

pub struct BatchTool {
    registry: Registry,
}

impl BatchTool {
    pub fn new(registry: Registry) -> Self {
        Self { registry }
    }
}

#[derive(Deserialize)]
struct BatchInput {
    tool_calls: Vec<ToolCallInput>,
}

#[derive(Deserialize, Clone)]
struct ToolCallInput {
    tool: String,
    #[serde(default)]
    parameters: Option<Value>,
}

impl ToolCallInput {
    fn resolved_parameters(self) -> (String, Value) {
        if let Some(params) = self.parameters {
            return (self.tool, params);
        }
        (self.tool, Value::Object(Default::default()))
    }
}

/// Try to fix common LLM mistakes in batch tool_calls:
/// - Parameters placed at the same level as "tool" instead of nested under "parameters"
fn normalize_batch_input(mut input: Value) -> Value {
    if let Some(calls) = input.get_mut("tool_calls").and_then(|v| v.as_array_mut()) {
        for call in calls.iter_mut() {
            if let Some(obj) = call.as_object_mut() {
                if !obj.contains_key("parameters") && obj.contains_key("tool") {
                    let tool_name = obj.get("tool").cloned();
                    let mut params = serde_json::Map::new();
                    let keys: Vec<String> = obj.keys().filter(|k| *k != "tool").cloned().collect();
                    for key in keys {
                        if let Some(val) = obj.remove(&key) {
                            params.insert(key, val);
                        }
                    }
                    if !params.is_empty() {
                        obj.insert("parameters".to_string(), Value::Object(params));
                    }
                    if let Some(name) = tool_name {
                        obj.insert("tool".to_string(), name);
                    }
                }
            }
        }
    }
    input
}

#[async_trait]
impl Tool for BatchTool {
    fn name(&self) -> &str {
        "batch"
    }

    fn description(&self) -> &str {
        "Execute multiple tools in parallel. Maximum 10 tool calls. \
         Cannot batch the 'batch' tool itself. Returns results for each tool call."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["tool_calls"],
            "properties": {
                "tool_calls": {
                    "type": "array",
                    "description": "Array of tool calls to execute in parallel",
                    "items": {
                        "type": "object",
                        "required": ["tool", "parameters"],
                        "properties": {
                            "tool": {
                                "type": "string",
                                "description": "Name of the tool to execute"
                            },
                            "parameters": {
                                "type": "object",
                                "description": "Parameters for the tool"
                            }
                        }
                    },
                    "minItems": 1,
                    "maxItems": 10
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let input = normalize_batch_input(input);
        let params: BatchInput = serde_json::from_value(input)?;

        if params.tool_calls.is_empty() {
            return Err(anyhow::anyhow!("No tool calls provided"));
        }

        if params.tool_calls.len() > MAX_PARALLEL {
            return Err(anyhow::anyhow!(
                "Maximum {} parallel tool calls allowed",
                MAX_PARALLEL
            ));
        }

        // Check for disallowed tools
        for tc in &params.tool_calls {
            if tc.tool == "batch" {
                return Err(anyhow::anyhow!("Cannot batch the 'batch' tool"));
            }
        }

        // Execute all tools in parallel
        let num_tools = params.tool_calls.len();
        let futures: Vec<_> = params
            .tool_calls
            .into_iter()
            .enumerate()
            .map(|(i, tc)| {
                let registry = self.registry.clone();
                let (tool_name, parameters) = tc.resolved_parameters();
                let sub_ctx = ctx.for_subcall(format!("batch-{}-{}", i + 1, tool_name.clone()));
                async move {
                    let result = registry.execute(&tool_name, parameters, sub_ctx).await;
                    (i, tool_name, result)
                }
            })
            .collect();

        let results = futures::future::join_all(futures).await;

        // Format results
        let mut output = String::new();
        let mut success_count = 0;
        let mut error_count = 0;

        for (i, tool_name, result) in results {
            output.push_str(&format!("--- [{}] {} ---\n", i + 1, tool_name));
            match result {
                Ok(out) => {
                    success_count += 1;
                    let max_per_tool = 50_000 / num_tools.max(1);
                    if out.output.len() > max_per_tool {
                        output.push_str(crate::util::truncate_str(&out.output, max_per_tool));
                        output.push_str("...\n(truncated)");
                    } else {
                        output.push_str(&out.output);
                    }
                }
                Err(e) => {
                    error_count += 1;
                    output.push_str(&format!("Error: {}", e));
                }
            }
            output.push_str("\n\n");
        }

        output.push_str(&format!(
            "Completed: {} succeeded, {} failed",
            success_count, error_count
        ));

        Ok(ToolOutput::new(output))
    }
}
