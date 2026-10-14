use super::{Registry, Tool, ToolContext, ToolOutput, WeakRegistry};
use crate::bus::{BatchSubcallProgress, BatchSubcallState};
use crate::message::ToolCall;
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;

const MAX_PARALLEL: usize = 10;

const BATCH_DESCRIPTION: &str = r#"Run independent tool calls in parallel instead of making them sequentially. Put each tool's input inside `parameters`, separate from the batch routing field `tool`. This is essential for mcp_call and other tools whose input also contains `tool`, `name`, or `arguments`. Flat inputs remain supported for compatibility. Example:
{
  "intent": "Inspect the relevant files in parallel",
  "tool_calls": [
    {
      "tool": "read",
      "intent": "Read the configuration",
      "parameters": {
        "file_path": "src/config.rs",
        "start_line": 1,
        "limit": 200
      }
    },
    {
      "tool": "agentgrep",
      "intent": "Find configuration usage",
      "parameters": {
        "query": "Config",
        "path": "src",
        "glob": "**/*.rs",
        "max_files": 20,
        "max_regions": 20
      }
    }
  ]
}
For an MCP call, keep the selectors at different levels:
{"tool_calls":[{"tool":"mcp_call","intent":"Call the discovered MCP tool","parameters":{"server":"example","tool":"raw_tool_name","arguments":{}}}]}"#;
pub(crate) fn generic_batch_schema() -> Value {
    json!({
        "type": "object",
        "required": ["tool_calls"],
        "properties": {
            "intent": super::intent_schema_property(),
            "tool_calls": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["tool", "intent"],
                    "properties": {
                        "tool": {
                            "type": "string",
                            "description": "Tool name."
                        },
                        "intent": super::intent_schema_property(),
                        "parameters": {
                            "type": "object",
                            "additionalProperties": true,
                            "description": "Complete child input. Nest native tool, name, arguments, args, and input fields here."
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
                intent: ToolCall::intent_from_input(parameters),
                thought_signature: None,
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
    registry: WeakRegistry,
}

impl BatchTool {
    pub(super) fn new(registry: WeakRegistry) -> Self {
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

fn schema_property_declaration(schema: &Value, property: &str) -> Option<bool> {
    fn declares(
        schema: &Value,
        root: &Value,
        property: &str,
        depth: usize,
        budget: &mut usize,
    ) -> Option<bool> {
        if depth > 16 || *budget == 0 {
            return None;
        }
        *budget -= 1;
        if schema
            .get("properties")
            .and_then(Value::as_object)
            .is_some_and(|properties| properties.contains_key(property))
        {
            return Some(true);
        }
        // Explicitly open schemas may use native fields not listed in properties.
        // Preserve those fields rather than guessing that they are batch aliases.
        let mut unknown = matches!(
            schema.get("additionalProperties"),
            Some(Value::Bool(true) | Value::Object(_))
        ) || match schema.get("patternProperties") {
            Some(Value::Object(patterns)) => !patterns.is_empty(),
            Some(_) => true,
            None => false,
        };
        if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
            match reference
                .strip_prefix('#')
                .and_then(|path| root.pointer(path))
                .and_then(|referenced| declares(referenced, root, property, depth + 1, budget))
            {
                Some(true) => return Some(true),
                Some(false) => {}
                None => unknown = true,
            }
        }
        for key in ["allOf", "anyOf", "oneOf", "if", "then", "else"] {
            if let Some(value) = schema.get(key) {
                let branches = value
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_else(|| std::slice::from_ref(value));
                for branch in branches {
                    match declares(branch, root, property, depth + 1, budget) {
                        Some(true) => return Some(true),
                        Some(false) => {}
                        None => unknown = true,
                    }
                }
            }
        }
        if unknown { None } else { Some(false) }
    }
    declares(schema, schema, property, 0, &mut 256)
}

fn schema_declares_property(schema: Option<&Value>, property: &str) -> bool {
    schema.is_some_and(|schema| schema_property_declaration(schema, property) == Some(true))
}

fn schema_may_declare_property(schema: Option<&Value>, property: &str) -> bool {
    // Unresolved or cyclic schemas justify preserving a child field, not
    // automatically treating a `name` value as a new routing selector.
    schema.is_some_and(|schema| schema_property_declaration(schema, property) != Some(false))
}

fn schema_confidently_lacks_property(schema: Option<&Value>, property: &str) -> bool {
    !schema_may_declare_property(schema, property)
}

#[derive(Clone)]
struct SelectorSchema {
    schema: Value,
    resolved_route: Option<String>,
}

/// Normalize legacy flat calls without consuming a child's native routing or
/// argument fields. Explicit `parameters` keeps wrapper and child names separate.
#[cfg(test)]
fn normalize_batch_input_with_schemas(
    input: Value,
    tool_schemas: &HashMap<String, Value>,
) -> Result<Value> {
    let selectors = tool_schemas
        .iter()
        .map(|(name, schema)| {
            (
                name.clone(),
                SelectorSchema {
                    schema: schema.clone(),
                    resolved_route: Some(
                        Registry::resolve_tool_name(
                            name.strip_prefix("functions.").unwrap_or(name),
                        )
                        .to_string(),
                    ),
                },
            )
        })
        .collect();
    normalize_batch_input_with_selectors(input, &selectors)
}

fn normalize_batch_input_with_selectors(
    mut input: Value,
    tool_schemas: &HashMap<String, SelectorSchema>,
) -> Result<Value> {
    if let Some(calls) = input.get_mut("tool_calls").and_then(|v| v.as_array_mut()) {
        for call in calls.iter_mut() {
            if let Some(obj) = call.as_object_mut() {
                if obj.get("parameters").is_some_and(Value::is_null) {
                    obj.remove("parameters");
                }
                let tool_name = obj.get("tool").and_then(Value::as_str).map(str::to_owned);
                let name_alias = obj.get("name").and_then(Value::as_str).map(str::to_owned);
                if let (Some(tool), Some(name)) = (&tool_name, &name_alias) {
                    let tool_selector = tool_schemas.get(tool);
                    let name_selector = tool_schemas.get(name);
                    let tool_schema = tool_selector.map(|selector| &selector.schema);
                    let same_route = match (
                        tool_selector.and_then(|selector| selector.resolved_route.as_deref()),
                        name_selector.and_then(|selector| selector.resolved_route.as_deref()),
                    ) {
                        (Some(tool_route), Some(name_route)) => tool_route == name_route,
                        _ => tool == name,
                    };

                    if !obj.contains_key("parameters")
                        && !same_route
                        && let (Some(tool_route), Some(name_route)) = (
                            tool_selector.and_then(|selector| selector.resolved_route.as_deref()),
                            name_selector.and_then(|selector| selector.resolved_route.as_deref()),
                        )
                        && tool_route != name_route
                        && schema_confidently_lacks_property(tool_schema, "name")
                    {
                        anyhow::bail!(
                            "Ambiguous batch tool/name selectors. Use {{\"tool\":\"<wrapper>\",\"parameters\":{{\"tool\":\"<child>\"}}}}."
                        );
                    }

                    if same_route && schema_confidently_lacks_property(tool_schema, "name") {
                        obj.remove("name");
                    } else if !obj.contains_key("parameters")
                        && schema_confidently_lacks_property(tool_schema, "name")
                        && name_selector.is_some_and(|selector| {
                            schema_declares_property(Some(&selector.schema), "tool")
                        })
                    {
                        // Recover a name-routed wrapper only when the current
                        // route is known to omit a native `name` field.
                        obj.remove("name");
                        let wrapper = Value::String(name.clone());
                        let parameters = std::mem::take(obj);
                        obj.insert("tool".to_string(), wrapper);
                        obj.insert("parameters".to_string(), Value::Object(parameters));
                    }
                }
                // Normalize "name" -> "tool" if the model used the wrong key
                if !obj.contains_key("tool")
                    && let Some(name_val) = obj.remove("name")
                {
                    obj.insert("tool".to_string(), name_val);
                }

                if !obj.contains_key("parameters") {
                    let schema = obj
                        .get("tool")
                        .and_then(Value::as_str)
                        .and_then(|name| tool_schemas.get(name))
                        .map(|selector| &selector.schema);
                    for alias in ["arguments", "args", "input"] {
                        // Do not steal a native child field, or drop its routing
                        // siblings by mistaking it for the whole input envelope.
                        if schema_may_declare_property(schema, alias)
                            || obj.keys().any(|key| {
                                !matches!(key.as_str(), "tool" | "intent" | "accept_large_output")
                                    && key != alias
                            })
                        {
                            continue;
                        }
                        if let Some(alias_val) = obj.remove(alias) {
                            obj.insert("parameters".to_string(), alias_val);
                            break;
                        }
                    }
                }

                // Canonical batch calls may keep the display intent beside
                // `parameters`. Forward it into the effective tool input so
                // live progress events and the nested tool execution retain it.
                let top_level_intent = obj
                    .get("intent")
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    .filter(|intent| !intent.is_empty())
                    .map(ToString::to_string);
                if let Some(intent) = top_level_intent
                    && let Some(params) = obj.get_mut("parameters").and_then(Value::as_object_mut)
                {
                    params.insert("intent".to_string(), Value::String(intent));
                }

                // Same forwarding for the oversized-output opt-in. The context
                // guard runs per sub-call inside registry.execute(), so a flag
                // left beside `parameters` would be silently dropped and the
                // sub-call withheld again. Models place it at either level.
                let top_level_accept = obj
                    .get(jcode_tool_core::ACCEPT_LARGE_OUTPUT_KEY)
                    .filter(|value| !value.is_null())
                    .cloned();
                if let Some(accept) = top_level_accept
                    && let Some(params) = obj.get_mut("parameters").and_then(Value::as_object_mut)
                    && !params.contains_key(jcode_tool_core::ACCEPT_LARGE_OUTPUT_KEY)
                {
                    params.insert(jcode_tool_core::ACCEPT_LARGE_OUTPUT_KEY.to_string(), accept);
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
    Ok(input)
}

#[cfg(test)]
fn normalize_batch_input(input: Value) -> Value {
    normalize_batch_input_with_schemas(input, &HashMap::new()).unwrap()
}

#[async_trait]
impl Tool for BatchTool {
    fn name(&self) -> &str {
        "batch"
    }

    fn description(&self) -> &str {
        BATCH_DESCRIPTION
    }

    fn parameters_schema(&self) -> Value {
        generic_batch_schema()
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let registry = self
            .registry
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("Batch tool registry is no longer available"))?;
        // Look up only the supplied routing candidates. Match dispatch's
        // namespace/alias resolution and session-local SDK replacements, and
        // release the registry lock before executing any child tool.
        let tool_schemas = {
            let tools = registry.tools.read().await;
            let custom = super::sdk::config(&ctx.session_id);
            let mut schemas = HashMap::new();
            if let Some(calls) = input.get("tool_calls").and_then(Value::as_array) {
                for call in calls {
                    for key in ["tool", "name"] {
                        if let Some(name) = call.get(key).and_then(Value::as_str) {
                            let resolved =
                                Registry::resolve_tool_name_for_session(&ctx.session_id, name);
                            let schema = custom
                                .as_ref()
                                .and_then(|config| {
                                    config.custom.iter().find(|tool| tool.name == resolved)
                                })
                                .map(|tool| tool.parameters.clone())
                                .or_else(|| {
                                    tools.get(resolved).map(|tool| tool.parameters_schema())
                                });
                            if let Some(schema) = schema {
                                schemas.insert(
                                    name.to_string(),
                                    SelectorSchema {
                                        schema,
                                        resolved_route: Some(resolved.to_string()),
                                    },
                                );
                            }
                        }
                    }
                }
            }
            schemas
        };
        let input = normalize_batch_input_with_selectors(input, &tool_schemas)?;
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
            if Registry::resolve_tool_name(&tc.tool) == "batch" {
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
                // Display and dispatch the canonical name (e.g. `functions.bash`
                // -> `bash`) while keeping SDK custom tool names intact.
                let tool_name =
                    Registry::resolve_tool_name_for_session(&ctx.session_id, &tool_name)
                        .to_string();
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
                        intent: ToolCall::intent_from_input(parameters),
                        thought_signature: None,
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
            .map(|(i, tool_name, parameters)| {
                let registry = registry.clone();
                let i = *i;
                let tool_name = tool_name.clone();
                let parameters = parameters.clone();
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
        let mut images = Vec::new();
        let mut success_count = 0;
        let mut error_count = 0;
        let mut failed_tools = Vec::new();

        for (i, tool_name, result) in results {
            output.push_str(&format!("--- [{}] {} ---\n", i + 1, tool_name));
            match result {
                Ok(out) => {
                    success_count += 1;
                    // Preserve attachments in subcall order, just like the text.
                    // The agent emits them against the visible parent batch call
                    // and persists them with its aggregate tool result.
                    images.extend(out.images);
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
                    failed_tools.push(tool_name.clone());
                    output.push_str(&format!("Error: {}", e));
                }
            }
            output.push_str("\n\n");
        }

        if error_count > 0 {
            crate::logging::warn(&format!(
                "[tool:batch] {} of {} subcalls failed for {} in session {}: {}",
                error_count,
                num_tools,
                ctx.tool_call_id,
                ctx.session_id,
                failed_tools.join(", ")
            ));
        }

        output.push_str(&format!(
            "Completed: {} succeeded, {} failed",
            success_count, error_count
        ));

        let mut result = ToolOutput::new(output);
        result.images = images;
        Ok(result)
    }
}

#[cfg(test)]
#[path = "batch_tests.rs"]
mod batch_tests;

#[cfg(all(test, unix))]
#[path = "batch_mcp_tests.rs"]
mod batch_mcp_tests;

#[cfg(test)]
#[path = "batch_schema_tests.rs"]
mod batch_schema_tests;
