use super::{Registry, Tool, ToolContext, ToolOutput};
use crate::bus::{BatchSubcallProgress, BatchSubcallState};
use crate::message::ToolCall;
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;

const MAX_PARALLEL: usize = 10;

pub(crate) fn generic_batch_schema() -> Value {
    json!({
        "type": "object",
        "required": ["tool_calls"],
        "properties": {
            "tool_calls": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["tool"],
                    "properties": {
                        "tool": {
                            "type": "string",
                            "description": "Tool name."
                        }
                    },
                    "additionalProperties": true
                },
                "minItems": 1,
                "maxItems": 10
            }
        }
    })
}

fn ordered_batch_subcalls(
    subcalls: &[(usize, String, Value)],
    running: &HashMap<usize, ToolCall>,
    failures: &HashMap<usize, bool>,
) -> Vec<BatchSubcallProgress> {
    let mut ordered: Vec<BatchSubcallProgress> = subcalls
        .iter()
        .map(|(i, tool_name, parameters)| {
            let tool_call = running.get(i).cloned().unwrap_or_else(|| ToolCall {
                id: format!("batch-{}-{}", i + 1, tool_name),
                name: tool_name.clone(),
                input: parameters.clone(),
                intent: None,
            });
            let state = if running.contains_key(i) {
                BatchSubcallState::Running
            } else if failures.get(i).copied().unwrap_or(false) {
                BatchSubcallState::Failed
            } else {
                BatchSubcallState::Succeeded
            };

            BatchSubcallProgress {
                index: i + 1,
                tool_call,
                state,
            }
        })
        .collect();
    ordered.sort_by_key(|entry| entry.index);
    ordered
}

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
    #[serde(alias = "name")]
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
/// - "name" used instead of "tool" for the tool name key
/// - "arguments", "args", or "input" used instead of "parameters"
fn normalize_batch_input(mut input: Value) -> Value {
    if let Some(calls) = input.get_mut("tool_calls").and_then(|v| v.as_array_mut()) {
        for call in calls.iter_mut() {
            if let Some(obj) = call.as_object_mut() {
                // Normalize "name" -> "tool" if the model used the wrong key
                if !obj.contains_key("tool") {
                    if let Some(name_val) = obj.remove("name") {
                        obj.insert("tool".to_string(), name_val);
                    }
                }

                if !obj.contains_key("parameters") {
                    for alias in ["arguments", "args", "input"] {
                        if let Some(alias_val) = obj.remove(alias) {
                            obj.insert("parameters".to_string(), alias_val);
                            break;
                        }
                    }
                }

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
        "Run tools in parallel."
    }

    fn parameters_schema(&self) -> Value {
        generic_batch_schema()
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

        // Execute all tools in parallel, emitting progress events as each completes
        let num_tools = params.tool_calls.len();
        use futures::StreamExt;
        let subcalls: Vec<(usize, String, Value)> = params
            .tool_calls
            .into_iter()
            .enumerate()
            .map(|(i, tc)| {
                let (tool_name, parameters) = tc.resolved_parameters();
                (i, tool_name, parameters)
            })
            .collect();

        let mut running: HashMap<usize, ToolCall> = subcalls
            .iter()
            .map(|(i, tool_name, parameters)| {
                (
                    *i,
                    ToolCall {
                        id: format!("batch-{}-{}", i + 1, tool_name),
                        name: tool_name.clone(),
                        input: parameters.clone(),
                        intent: None,
                    },
                )
            })
            .collect();

        crate::bus::Bus::global().publish(crate::bus::BusEvent::BatchProgress(
            crate::bus::BatchProgress {
                session_id: ctx.session_id.clone(),
                tool_call_id: ctx.tool_call_id.clone(),
                total: num_tools,
                completed: 0,
                last_completed: None,
                running: running.values().cloned().collect(),
                subcalls: ordered_batch_subcalls(&subcalls, &running, &HashMap::new()),
            },
        ));

        let mut stream: futures::stream::FuturesUnordered<_> = subcalls
            .iter()
            .cloned()
            .map(|(i, tool_name, parameters)| {
                let registry = self.registry.clone();
                let sub_ctx = ctx.for_subcall(format!("batch-{}-{}", i + 1, tool_name.clone()));
                async move {
                    let result = registry.execute(&tool_name, parameters, sub_ctx).await;
                    (i, tool_name, result)
                }
            })
            .collect();

        let mut results: Vec<(usize, String, Result<ToolOutput>)> = Vec::with_capacity(num_tools);
        let mut failures: HashMap<usize, bool> = HashMap::new();
        let mut completed_count = 0usize;
        while let Some((i, tool_name, result)) = stream.next().await {
            completed_count += 1;
            let failed = result.is_err();
            running.remove(&i);
            failures.insert(i, failed);
            crate::bus::Bus::global().publish(crate::bus::BusEvent::BatchProgress(
                crate::bus::BatchProgress {
                    session_id: ctx.session_id.clone(),
                    tool_call_id: ctx.tool_call_id.clone(),
                    total: num_tools,
                    completed: completed_count,
                    last_completed: Some(tool_name.clone()),
                    running: running.values().cloned().collect(),
                    subcalls: ordered_batch_subcalls(&subcalls, &running, &failures),
                },
            ));
            results.push((i, tool_name, result));
        }
        // Restore original order
        results.sort_by_key(|(i, _, _)| *i);

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_normalize_flat_params() {
        let input = json!({
            "tool_calls": [
                {"tool": "read", "file_path": "file1.txt"},
                {"tool": "read", "file_path": "file2.txt"}
            ]
        });

        let normalized = normalize_batch_input(input);
        let parsed: BatchInput = serde_json::from_value(normalized).unwrap();
        assert_eq!(parsed.tool_calls.len(), 2);
        assert_eq!(parsed.tool_calls[0].tool, "read");
        let params = parsed.tool_calls[0].parameters.as_ref().unwrap();
        assert_eq!(params["file_path"], "file1.txt");
    }

    #[test]
    fn test_normalize_already_nested() {
        let input = json!({
            "tool_calls": [
                {"tool": "read", "parameters": {"file_path": "file1.txt"}}
            ]
        });

        let normalized = normalize_batch_input(input);
        let parsed: BatchInput = serde_json::from_value(normalized).unwrap();
        assert_eq!(parsed.tool_calls.len(), 1);
        let params = parsed.tool_calls[0].parameters.as_ref().unwrap();
        assert_eq!(params["file_path"], "file1.txt");
    }

    #[test]
    fn test_normalize_name_key_to_tool() {
        let input = json!({
            "tool_calls": [
                {"name": "read", "parameters": {"file_path": "file1.txt"}},
                {"name": "grep", "pattern": "foo", "path": "src/"}
            ]
        });

        let normalized = normalize_batch_input(input);
        let parsed: BatchInput = serde_json::from_value(normalized).unwrap();
        assert_eq!(parsed.tool_calls.len(), 2);
        assert_eq!(parsed.tool_calls[0].tool, "read");
        let params0 = parsed.tool_calls[0].parameters.as_ref().unwrap();
        assert_eq!(params0["file_path"], "file1.txt");
        assert_eq!(parsed.tool_calls[1].tool, "grep");
        let params1 = parsed.tool_calls[1].parameters.as_ref().unwrap();
        assert_eq!(params1["pattern"], "foo");
    }

    #[test]
    fn test_normalize_mixed_tool_and_name_keys() {
        let input = json!({
            "tool_calls": [
                {"tool": "read", "parameters": {"file_path": "a.rs"}},
                {"name": "read", "parameters": {"file_path": "b.rs"}},
                {"tool": "grep", "pattern": "test"}
            ]
        });

        let normalized = normalize_batch_input(input);
        let parsed: BatchInput = serde_json::from_value(normalized).unwrap();
        assert_eq!(parsed.tool_calls.len(), 3);
        assert_eq!(parsed.tool_calls[0].tool, "read");
        assert_eq!(parsed.tool_calls[1].tool, "read");
        assert_eq!(parsed.tool_calls[2].tool, "grep");
    }

    #[test]
    fn test_normalize_arguments_aliases_to_parameters() {
        let input = json!({
            "tool_calls": [
                {"tool": "read", "arguments": {"file_path": "a.rs"}},
                {"tool": "read", "args": {"file_path": "b.rs"}},
                {"tool": "read", "input": {"file_path": "c.rs"}}
            ]
        });

        let normalized = normalize_batch_input(input);
        let parsed: BatchInput = serde_json::from_value(normalized).unwrap();

        assert_eq!(parsed.tool_calls.len(), 3);
        assert_eq!(
            parsed.tool_calls[0].parameters.as_ref().unwrap()["file_path"],
            "a.rs"
        );
        assert_eq!(
            parsed.tool_calls[1].parameters.as_ref().unwrap()["file_path"],
            "b.rs"
        );
        assert_eq!(
            parsed.tool_calls[2].parameters.as_ref().unwrap()["file_path"],
            "c.rs"
        );
    }

    #[test]
    fn test_schema_only_requires_tool() {
        let schema = BatchTool::new(Registry {
            tools: std::sync::Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            skills: std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::skill::SkillRegistry::default(),
            )),
            compaction: std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::compaction::CompactionManager::new(),
            )),
        })
        .parameters_schema();

        assert_eq!(
            schema["properties"]["tool_calls"]["items"]["required"],
            json!(["tool"])
        );
        assert_eq!(
            schema["properties"]["tool_calls"]["items"]["additionalProperties"],
            json!(true)
        );
        assert_eq!(
            schema["properties"]["tool_calls"]["items"]["properties"]["tool"]["description"],
            json!("Tool name.")
        );
        assert!(schema["properties"]["tool_calls"]["items"]["properties"]["parameters"].is_null());
    }

    #[test]
    fn test_schema_keeps_flat_generic_subcall_shape() {
        let schema = generic_batch_schema();

        assert!(schema["properties"]["tool_calls"]["description"].is_null());
        assert!(schema["properties"]["tool_calls"]["items"]["description"].is_null());
        assert_eq!(
            schema["properties"]["tool_calls"]["items"]["properties"]
                .as_object()
                .map(|props| props.len()),
            Some(1)
        );
        assert!(schema["properties"]["tool_calls"]["items"]["oneOf"].is_null());
    }
}
