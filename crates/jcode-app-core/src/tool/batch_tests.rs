use super::*;
use serde_json::json;
use std::sync::Arc;

struct EchoTool;

struct McpRoutingProbe;

struct DispatchCounter(Arc<std::sync::atomic::AtomicUsize>);

#[async_trait::async_trait]
impl Tool for DispatchCounter {
    fn name(&self) -> &str {
        "dispatch_counter"
    }

    fn description(&self) -> &str {
        "Count child executions in batch tests"
    }

    fn parameters_schema(&self) -> Value {
        json!({"type": "object", "properties": {"tool": {"type": "string"}}})
    }

    async fn execute(&self, _input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(ToolOutput::new("dispatched"))
    }
}

fn mcp_probe_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "server": {"type": "string"},
            "tool": {"type": "string"},
            "arguments": {"type": "object", "additionalProperties": true}
        },
        "required": ["server", "tool", "arguments"]
    })
}

#[async_trait::async_trait]
impl Tool for McpRoutingProbe {
    fn name(&self) -> &str {
        "mcp_call"
    }
    fn description(&self) -> &str {
        "Offline MCP routing probe"
    }
    fn parameters_schema(&self) -> Value {
        mcp_probe_schema()
    }
    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        #[derive(serde::Deserialize)]
        struct Input {
            server: String,
            tool: String,
            arguments: Value,
        }
        let parsed: Input = serde_json::from_value(input)?;
        Ok(ToolOutput::new(format!(
            "{}:{}:{}",
            parsed.server, parsed.tool, parsed.arguments
        )))
    }
}

#[async_trait::async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }

    fn description(&self) -> &str {
        "Echo test input"
    }

    fn parameters_schema(&self) -> Value {
        json!({"type": "object"})
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        Ok(ToolOutput::new(input["text"].as_str().unwrap_or_default()))
    }
}

fn test_context() -> ToolContext {
    ToolContext {
        session_id: "batch-registry-lifetime".to_string(),
        message_id: "message".to_string(),
        tool_call_id: "batch-call".to_string(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: super::super::ToolExecutionMode::Direct,
    }
}

async fn registry_with_batch_and_echo() -> Registry {
    let registry = Registry::empty();
    let mut tools = registry.tools.write().await;
    tools.insert("echo".to_string(), Arc::new(EchoTool));
    tools.insert(
        "batch".to_string(),
        Arc::new(BatchTool::new(registry.downgrade())),
    );
    drop(tools);
    registry
}

#[tokio::test]
async fn registry_tool_map_drops_after_external_owners_are_dropped() {
    let registry = Registry::empty();
    let tools = Arc::downgrade(&registry.tools);

    registry.tools.write().await.insert(
        "batch".to_string(),
        Arc::new(BatchTool::new(registry.downgrade())) as Arc<dyn Tool>,
    );

    drop(registry);

    assert!(
        tools.upgrade().is_none(),
        "BatchTool must not strongly retain the registry tool map that owns it"
    );
}

#[tokio::test]
async fn batch_executes_through_surviving_registry_clone() {
    let registry = registry_with_batch_and_echo().await;
    let surviving_clone = registry.clone();
    drop(registry);

    let output = surviving_clone
        .execute(
            "batch",
            json!({
                "tool_calls": [{
                    "tool": "echo",
                    "intent": "Verify the surviving registry clone",
                    "parameters": {"text": "still alive"}
                }]
            }),
            test_context(),
        )
        .await
        .expect("batch should use the surviving registry clone's tool map");

    assert!(output.output.contains("still alive"));
    assert!(output.output.contains("Completed: 1 succeeded, 0 failed"));
}

#[tokio::test]
async fn batch_fails_cleanly_after_registry_tool_map_is_dropped() {
    let registry = registry_with_batch_and_echo().await;
    let batch = registry
        .tools
        .read()
        .await
        .get("batch")
        .cloned()
        .expect("batch tool should be registered");
    drop(registry);

    let error = batch
        .execute(
            json!({
                "tool_calls": [{
                    "tool": "echo",
                    "intent": "Verify clean teardown",
                    "parameters": {"text": "unreachable"}
                }]
            }),
            test_context(),
        )
        .await
        .expect_err("batch should reject execution after its registry is gone");

    assert_eq!(
        error.to_string(),
        "Batch tool registry is no longer available"
    );
}

#[test]
fn description_includes_parallel_tool_call_example() {
    assert!(BATCH_DESCRIPTION.contains("Run independent tool calls in parallel"));
    assert!(BATCH_DESCRIPTION.contains(r#""tool_calls": ["#));
    assert!(BATCH_DESCRIPTION.contains(r#""tool": "read""#));
    assert!(BATCH_DESCRIPTION.contains(r#""tool": "agentgrep""#));
}

#[test]
fn batch_schema_exposes_collision_free_parameters() {
    let schema = generic_batch_schema();
    let parameters = &schema["properties"]["tool_calls"]["items"]["properties"]["parameters"];
    assert_eq!(parameters["type"], "object");
    assert_eq!(parameters["additionalProperties"], true);
    assert!(crate::util::estimate_tokens(parameters["description"].as_str().unwrap()) <= 25);
    assert!(BATCH_DESCRIPTION.contains(r#""parameters""#));
    assert!(BATCH_DESCRIPTION.contains(r#""tool":"mcp_call""#));
}

#[test]
fn normalization_preserves_flat_arguments_and_routing_siblings() {
    let input = json!({"tool_calls": [{
        "tool": "mcp_call",
        "server": "fixture",
        "tool_name": "probe",
        "arguments": {"id": "synthetic"}
    }]});
    let parsed: BatchInput = serde_json::from_value(normalize_batch_input(input)).unwrap();
    let parameters = parsed.tool_calls[0].parameters.as_ref().unwrap();
    assert_eq!(parameters["server"], "fixture");
    assert_eq!(parameters["arguments"]["id"], "synthetic");
}

#[tokio::test]
async fn batch_preserves_flat_native_arguments_at_dispatch() {
    let registry = registry_with_batch_and_echo().await;
    let output = registry
        .execute(
            "batch",
            json!({"tool_calls": [{
                "tool": "echo",
                "intent": "Check native arguments remain child input",
                "text": "native arguments preserved",
                "arguments": {"id": "synthetic"}
            }]}),
            test_context(),
        )
        .await
        .unwrap();
    assert!(output.output.contains("native arguments preserved"));
    assert!(output.output.contains("Completed: 1 succeeded, 0 failed"));
}

#[tokio::test]
async fn batch_recovers_name_routed_mcp_without_duplicate_tool() {
    let registry = registry_with_batch_and_echo().await;
    registry
        .tools
        .write()
        .await
        .insert("mcp_call".into(), Arc::new(McpRoutingProbe));
    for wrapper in ["mcp_call", "functions.mcp_call"] {
        let output = registry
            .execute(
                "batch",
                json!({"tool_calls": [{
                    "name": wrapper,
                    "tool": "probe",
                    "server": "fixture",
                    "arguments": {"id": "synthetic"},
                    "intent": "Check independent wrapper and raw MCP selectors"
                }]}),
                test_context(),
            )
            .await
            .unwrap();
        assert!(
            output
                .output
                .contains("fixture:probe:{\"id\":\"synthetic\"}")
        );
        assert!(output.output.contains("Completed: 1 succeeded, 0 failed"));
    }
}

#[tokio::test]
async fn batch_nested_mcp_keeps_all_routing_fields() {
    let registry = registry_with_batch_and_echo().await;
    registry
        .tools
        .write()
        .await
        .insert("mcp_call".into(), Arc::new(McpRoutingProbe));
    let output = registry.execute("batch", json!({"tool_calls": [{
        "tool": "mcp_call",
        "intent": "Check canonical MCP input envelope",
        "parameters": {"server": "fixture", "tool": "probe", "arguments": {"id": "synthetic"}}
    }]}), test_context()).await.unwrap();
    assert!(
        output
            .output
            .contains("fixture:probe:{\"id\":\"synthetic\"}")
    );
    assert!(output.output.contains("Completed: 1 succeeded, 0 failed"));
}

#[tokio::test]
async fn batch_rejects_ambiguous_selectors_before_dispatching_any_child() {
    let registry = registry_with_batch_and_echo().await;
    let executions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    registry.tools.write().await.insert(
        "dispatch_counter".into(),
        Arc::new(DispatchCounter(executions.clone())),
    );
    registry
        .tools
        .write()
        .await
        .insert("mcp_call".into(), Arc::new(McpRoutingProbe));

    let error = registry
        .execute(
            "batch",
            json!({"tool_calls": [
                {
                    "tool": "mcp_call",
                    "name": "dispatch_counter",
                    "server": "fixture",
                    "arguments": {},
                    "intent": "Ambiguous first call"
                },
                {
                    "tool": "dispatch_counter",
                    "parameters": {},
                    "intent": "Must not run before batch validation"
                }
            ]}),
            test_context(),
        )
        .await
        .expect_err("two callable selectors must be rejected");

    assert!(
        error
            .to_string()
            .contains("Ambiguous batch tool/name selectors")
    );
    assert_eq!(executions.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn batch_preserves_alias_declared_by_session_custom_schema() {
    use crate::protocol::{ServerEvent, SessionToolConfig, SessionToolDefinition};

    struct Cleanup(String);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            crate::tool::sdk::remove_session(&self.0);
            crate::tool::clear_session_tool_policy(&self.0);
        }
    }

    let session = format!("batch-custom-schema-{}", uuid::Uuid::new_v4());
    let _cleanup = Cleanup(session.clone());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    crate::tool::sdk::configure(
        &session,
        "batch-custom-schema-owner",
        SessionToolConfig {
            custom: vec![SessionToolDefinition {
                name: "echo".into(),
                description: "Custom echo schema".into(),
                parameters: json!({"type": "object", "properties": {
                    "arguments": {"type": "object"}
                }}),
            }],
            ..Default::default()
        },
        tx,
    )
    .unwrap();

    let registry = registry_with_batch_and_echo().await;
    let call_session = session.clone();
    let task = tokio::spawn(async move {
        registry
            .execute(
                "batch",
                json!({"tool_calls": [{
                    "tool": "functions.echo",
                    "arguments": {"id": "custom-schema"},
                    "intent": "Preserve the custom tool's native arguments field"
                }]}),
                ToolContext {
                    session_id: call_session,
                    ..test_context()
                },
            )
            .await
    });

    let ServerEvent::ToolCall {
        call_id,
        name,
        input,
        session_id: callback_session,
    } = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .expect("custom tool callback should arrive")
        .expect("callback channel should remain open")
    else {
        panic!("expected custom tool callback");
    };
    assert_eq!(callback_session, session);
    assert_eq!(name, "echo");
    assert_eq!(input["arguments"]["id"], "custom-schema");
    crate::tool::sdk::complete(
        "batch-custom-schema-owner",
        &session,
        &call_id,
        "callback output".into(),
        None,
    )
    .unwrap();
    let output = task.await.unwrap().unwrap();
    assert!(output.output.contains("callback output"));
    assert!(output.output.contains("Completed: 1 succeeded, 0 failed"));
}

#[test]
fn normalization_keeps_schema_declared_alias_fields_even_without_siblings() {
    for alias in ["arguments", "args", "input"] {
        let schemas = HashMap::from([(
            "native".into(),
            json!({"properties": {alias: {"type": "object"}}}),
        )]);
        let input = json!({"tool_calls": [{"tool": "native", alias: {"id": "synthetic"}}]});
        let parsed: BatchInput =
            serde_json::from_value(normalize_batch_input_with_schemas(input, &schemas).unwrap())
                .unwrap();
        assert_eq!(
            parsed.tool_calls[0].parameters.as_ref().unwrap()[alias]["id"],
            "synthetic"
        );
    }
}

#[test]
fn normalization_preserves_alias_fields_declared_through_combiners_and_refs() {
    for schema in [
        json!({"allOf": [{"properties": {"arguments": {"type": "object"}}}]}),
        json!({"anyOf": [{"type": "null"}, {"properties": {"arguments": {"type": "object"}}}]}),
        json!({"$ref": "#/$defs/Input", "$defs": {"Input": {"properties": {"arguments": {"type": "object"}}}}}),
        json!({"$ref": "#"}),
        json!({"$ref": "https://example.invalid/input.schema.json"}),
    ] {
        let schemas = HashMap::from([("native".into(), schema)]);
        let input = json!({"tool_calls": [{"tool": "native", "arguments": {"id": "synthetic"}}]});
        let parsed: BatchInput =
            serde_json::from_value(normalize_batch_input_with_schemas(input, &schemas).unwrap())
                .unwrap();
        assert_eq!(
            parsed.tool_calls[0].parameters.as_ref().unwrap()["arguments"]["id"],
            "synthetic"
        );
    }
}

#[test]
fn unresolved_schema_does_not_authorize_name_based_rerouting() {
    let schemas = HashMap::from([("possible_wrapper".into(), json!({"$ref": "#"}))]);
    let input = json!({"tool_calls": [{"tool": "unknown_route", "name": "possible_wrapper", "arguments": {}}]});
    let parsed: BatchInput =
        serde_json::from_value(normalize_batch_input_with_schemas(input, &schemas).unwrap())
            .unwrap();
    assert_eq!(parsed.tool_calls[0].tool, "unknown_route");
    assert_eq!(
        parsed.tool_calls[0].parameters.as_ref().unwrap()["name"],
        "possible_wrapper"
    );
}

#[test]
fn nested_schema_properties_are_not_root_argument_fields() {
    let schema =
        json!({"properties": {"payload": {"properties": {"arguments": {"type": "object"}}}}});
    assert_eq!(
        schema_property_declaration(&schema, "arguments"),
        Some(false)
    );
}

#[test]
fn normalization_preserves_native_name_field() {
    let input = json!({"tool_calls": [{"tool": "echo", "name": "project", "text": "value"}]});
    let parsed: BatchInput = serde_json::from_value(normalize_batch_input(input)).unwrap();
    assert_eq!(parsed.tool_calls[0].tool, "echo");
    assert_eq!(
        parsed.tool_calls[0].parameters.as_ref().unwrap()["name"],
        "project"
    );
}

#[test]
fn normalization_rejects_two_callable_routes_before_dispatch() {
    let schemas = HashMap::from([
        ("mcp_call".into(), mcp_probe_schema()),
        ("echo".into(), json!({"type": "object"})),
    ]);
    let input = json!({"tool_calls": [{"name": "mcp_call", "tool": "echo", "server": "fixture", "arguments": {}}]});
    let error = normalize_batch_input_with_schemas(input, &schemas).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Ambiguous batch tool/name selectors")
    );
    assert!(error.to_string().contains("parameters"));
}

#[test]
fn normalization_preserves_flat_input_with_null_parameters() {
    let input = json!({"tool_calls": [{"tool": "echo", "parameters": null, "text": "retained"}]});
    let parsed: BatchInput = serde_json::from_value(normalize_batch_input(input)).unwrap();
    assert_eq!(
        parsed.tool_calls[0].parameters.as_ref().unwrap()["text"],
        "retained"
    );
}

#[test]
fn normalization_drops_only_redundant_non_native_name_selector() {
    for native_name in [false, true] {
        let properties = if native_name {
            json!({"name": {"type": "string"}})
        } else {
            json!({})
        };
        let schemas = HashMap::from([("echo".into(), json!({"properties": properties}))]);
        let input = json!({"tool_calls": [{"tool": "echo", "name": "echo", "text": "retained"}]});
        let parsed: BatchInput =
            serde_json::from_value(normalize_batch_input_with_schemas(input, &schemas).unwrap())
                .unwrap();
        let parameters = parsed.tool_calls[0].parameters.as_ref().unwrap();
        assert_eq!(parameters.get("name").is_some(), native_name);
        assert_eq!(parameters["text"], "retained");
    }
}

#[test]
fn normalization_keeps_native_name_that_matches_another_registered_route() {
    let native_schema = json!({
        "type": "object",
        "properties": {"name": {"type": "string"}}
    });
    for schema in [
        native_schema,
        json!({"allOf": [{"properties": {"name": {"type": "string"}}}]}),
        json!({
            "$ref": "#/$defs/Input",
            "$defs": {"Input": {"properties": {"name": {"type": "string"}}}}
        }),
    ] {
        let schemas = HashMap::from([
            ("native".into(), schema),
            ("mcp_call".into(), mcp_probe_schema()),
        ]);
        let input = json!({"tool_calls": [{
            "tool": "native",
            "name": "mcp_call",
            "payload": {"id": "fixture"}
        }]});
        let parsed: BatchInput =
            serde_json::from_value(normalize_batch_input_with_schemas(input, &schemas).unwrap())
                .unwrap();
        let call = &parsed.tool_calls[0];
        assert_eq!(call.tool, "native");
        let parameters = call.parameters.as_ref().unwrap();
        assert_eq!(parameters["name"], "mcp_call");
        assert_eq!(parameters["payload"]["id"], "fixture");
    }
}

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
fn test_normalize_forwards_top_level_intent_into_nested_parameters() {
    let input = json!({
        "tool_calls": [{
            "tool": "read",
            "intent": "Inspect the batch renderer",
            "parameters": {"file_path": "src/tui/ui_messages.rs"}
        }]
    });

    let normalized = normalize_batch_input(input);
    let parsed: BatchInput = serde_json::from_value(normalized).unwrap();
    let params = parsed.tool_calls[0].parameters.as_ref().unwrap();

    assert_eq!(params["intent"], "Inspect the batch renderer");
    assert_eq!(params["file_path"], "src/tui/ui_messages.rs");
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
    let registry = Registry {
        mcp_policy: Arc::default(),
        tools: std::sync::Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        skills: std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::skill::SkillRegistry::default(),
        )),
        compaction: std::sync::Arc::new(tokio::sync::RwLock::new(
            crate::compaction::CompactionManager::new(),
        )),
    };
    let schema = BatchTool::new(registry.downgrade()).parameters_schema();

    assert_eq!(
        schema["properties"]["tool_calls"]["items"]["required"],
        // Nested batch entries require `intent` alongside `tool` so every
        // fanned-out call carries a display label, matching the central
        // intent requirement in `ensure_intent_in_schema` (8505080a6).
        json!(["tool", "intent"])
    );
    assert_eq!(
        schema["properties"]["tool_calls"]["items"]["additionalProperties"],
        json!(true)
    );
    assert_eq!(
        schema["properties"]["tool_calls"]["items"]["properties"]["tool"]["description"],
        json!("Tool name.")
    );
    assert!(schema["properties"]["tool_calls"]["items"]["properties"]["intent"].is_object());
    assert_eq!(
        schema["properties"]["tool_calls"]["items"]["properties"]["parameters"]["type"],
        "object"
    );
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
        Some(3)
    );
    assert!(schema["properties"]["tool_calls"]["items"]["oneOf"].is_null());
}

#[test]
fn subcall_level_accept_large_output_is_forwarded_into_parameters() {
    // Models place the flag beside `tool` rather than inside `parameters`, the
    // same mistake they already make with `intent`. The guard runs per sub-call
    // on that sub-call's parameters, so a flag left at the wrong level is
    // silently dropped and the sub-call withheld again.
    let input = serde_json::json!({
        "tool_calls": [{
            "tool": "agentgrep",
            "accept_large_output": true,
            "parameters": { "query": "x" },
        }]
    });
    let out = super::normalize_batch_input(input);
    assert_eq!(
        out["tool_calls"][0]["parameters"][jcode_tool_core::ACCEPT_LARGE_OUTPUT_KEY],
        serde_json::json!(true),
        "flag beside `tool` must reach the sub-call parameters"
    );
}

#[test]
fn subcall_level_accept_large_output_does_not_override_an_explicit_value() {
    // An explicit `false` inside parameters is a deliberate choice for that one
    // sub-call and must win over a blanket flag beside `tool`.
    let input = serde_json::json!({
        "tool_calls": [{
            "tool": "agentgrep",
            "accept_large_output": true,
            "parameters": { "query": "x", "accept_large_output": false },
        }]
    });
    let out = super::normalize_batch_input(input);
    assert_eq!(
        out["tool_calls"][0]["parameters"][jcode_tool_core::ACCEPT_LARGE_OUTPUT_KEY],
        serde_json::json!(false),
        "explicit per-subcall value must win"
    );
}

struct ImageTool;

#[async_trait::async_trait]
impl Tool for ImageTool {
    fn name(&self) -> &str {
        "test_image"
    }
    fn description(&self) -> &str {
        "Image fixture"
    }
    fn parameters_schema(&self) -> Value {
        json!({"type": "object"})
    }
    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        if input["fail"] == true {
            anyhow::bail!("fixture failure");
        }
        if input["slow"] == true {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let data = input["data"].as_str().unwrap();
        Ok(ToolOutput::new("image").with_labeled_image("image/png", data, format!("{data}.png")))
    }
}

#[tokio::test]
async fn batch_preserves_images_in_input_order_across_failures() {
    let registry = registry_with_batch_and_echo().await;
    registry
        .tools
        .write()
        .await
        .insert("test_image".into(), Arc::new(ImageTool));
    let output = registry
        .execute(
            "batch",
            json!({"tool_calls": [
                {"tool": "test_image", "parameters": {"data": "first", "slow": true}},
                {"tool": "test_image", "parameters": {"fail": true}},
                {"tool": "test_image", "parameters": {"data": "last"}}
            ]}),
            test_context(),
        )
        .await
        .unwrap();
    assert_eq!(
        output
            .images
            .iter()
            .map(|i| i.data.as_str())
            .collect::<Vec<_>>(),
        ["first", "last"]
    );
    assert_eq!(output.images[0].label.as_deref(), Some("first.png"));
    assert_eq!(output.images[1].media_type, "image/png");
    assert!(output.output.contains("Completed: 2 succeeded, 1 failed"));
}
