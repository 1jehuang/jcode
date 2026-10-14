use super::BatchTool;
use super::{Registry, Tool, ToolContext, ToolOutput};
use anyhow::Result;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct CountTool(&'static str, Arc<AtomicUsize>, Value);

#[async_trait::async_trait]
impl Tool for CountTool {
    fn name(&self) -> &str {
        self.0
    }
    fn description(&self) -> &str {
        "batch schema regression probe"
    }
    fn parameters_schema(&self) -> Value {
        self.2.clone()
    }
    async fn execute(&self, _input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        self.1.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutput::new("called"))
    }
}

fn context(session_id: String) -> ToolContext {
    ToolContext {
        session_id,
        message_id: "message".into(),
        tool_call_id: "batch-schema-regression".into(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: super::super::ToolExecutionMode::Direct,
    }
}

async fn registry() -> Registry {
    let registry = Registry::empty();
    registry.tools.write().await.insert(
        "batch".into(),
        Arc::new(BatchTool::new(registry.downgrade())),
    );
    registry
}

#[tokio::test]
async fn ordinary_alias_collision_rejects_batch_before_sibling_dispatch() {
    let registry = registry().await;
    let count = Arc::new(AtomicUsize::new(0));
    let ordinary_schema = json!({"type":"object", "properties":{"text":{"type":"string"}}});
    {
        let mut tools = registry.tools.write().await;
        tools.insert(
            "read".into(),
            Arc::new(CountTool("read", count.clone(), ordinary_schema.clone())),
        );
        tools.insert(
            "edit".into(),
            Arc::new(CountTool("edit", count.clone(), ordinary_schema)),
        );
    }
    let result = registry
        .execute(
            "batch",
            json!({"tool_calls":[
                {"tool":"read", "name":"edit", "text":"value"},
                {"tool":"edit", "parameters":{"text":"sibling"}}
            ]}),
            context("ordinary-alias-collision".into()),
        )
        .await;
    let error = result.expect_err("two ordinary registered routes must be ambiguous");
    assert!(
        error
            .to_string()
            .contains("Ambiguous batch tool/name selectors"),
        "{error:#}"
    );
    assert_eq!(
        count.load(Ordering::SeqCst),
        0,
        "validation must precede all dispatch"
    );
}

#[tokio::test]
async fn unknown_custom_schema_keeps_name_and_runs_independent_sdk_sibling() {
    for schema in [
        json!({"type":"object", "$ref":"https://example.invalid/schema.json"}),
        json!({"type":"object", "$ref":"#"}),
    ] {
        sdk_unknown_schema_case(schema).await;
    }
}

async fn sdk_unknown_schema_case(schema: Value) {
    sdk_native_name_case(schema, "mcp_call").await;
}

#[tokio::test]
async fn explicitly_open_schemas_preserve_native_names_in_sdk_callbacks() {
    for additional in [json!(true), json!({"type":"string"})] {
        for native_name in ["mcp_call", "opaque"] {
            sdk_native_name_case(
                json!({"type":"object", "additionalProperties":additional}),
                native_name,
            )
            .await;
        }
    }
}

async fn sdk_native_name_case(schema: Value, native_name: &'static str) {
    use crate::protocol::{ServerEvent, SessionToolConfig, SessionToolDefinition};
    struct Cleanup(String);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            crate::tool::sdk::remove_session(&self.0);
            crate::tool::clear_session_tool_policy(&self.0);
        }
    }

    let session = format!("batch-unknown-schema-{}", uuid::Uuid::new_v4());
    let _cleanup = Cleanup(session.clone());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    crate::tool::sdk::configure(
        &session,
        "batch-schema-tests",
        SessionToolConfig {
            custom: vec![SessionToolDefinition {
                name: "opaque".into(),
                description: "Unknown schema probe".into(),
                parameters: schema,
            }],
            ..Default::default()
        },
        tx,
    )
    .unwrap();
    let registry = registry().await;
    let count = Arc::new(AtomicUsize::new(0));
    registry.tools.write().await.insert(
        "probe".into(),
        Arc::new(CountTool(
            "probe",
            count.clone(),
            json!({"type":"object", "properties":{"value":{"type":"string"}}}),
        )),
    );
    let mcp_count = Arc::new(AtomicUsize::new(0));
    registry.tools.write().await.insert(
        "mcp_call".into(),
        Arc::new(CountTool(
            "mcp_call",
            mcp_count.clone(),
            json!({"type":"object", "properties":{
                "server":{"type":"string"}, "tool":{"type":"string"}, "arguments":{"type":"object"}
            }}),
        )),
    );
    let session_for_call = session.clone();
    let mut task = tokio::spawn(async move {
        registry
            .execute(
                "batch",
                json!({"tool_calls":[
                    {"tool":"opaque", "name":native_name, "value":"kept"},
                    {"tool":"probe", "parameters":{"value":"sibling"}}
                ]}),
                context(session_for_call),
            )
            .await
    });

    let event = tokio::select! {
        result = &mut task => match result {
            Ok(Ok(output)) => panic!("batch completed before SDK callback: {}", output.output),
            Ok(Err(error)) => panic!("batch failed before SDK callback: {error:#}"),
            Err(error) => panic!("batch task failed before SDK callback: {error}"),
        },
        event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv()) => {
            event.expect("SDK callback should arrive within two seconds")
                .expect("callback channel should remain open")
        }
    };
    let ServerEvent::ToolCall {
        call_id,
        name,
        input,
        session_id,
    } = event
    else {
        panic!("expected SDK custom-tool callback");
    };
    assert_eq!(name, "opaque");
    assert_eq!(session_id, session);
    assert_eq!(input["name"], native_name);
    assert_eq!(input["value"], "kept");
    crate::tool::sdk::complete(
        "batch-schema-tests",
        &session,
        &call_id,
        "sdk callback".into(),
        None,
    )
    .unwrap();
    let output = task.await.unwrap().unwrap();
    assert!(output.output.contains("sdk callback"));
    assert!(
        output.output.contains("Completed: 2 succeeded, 0 failed"),
        "{}",
        output.output
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(
        mcp_count.load(Ordering::SeqCst),
        0,
        "native name must not reroute the SDK custom tool"
    );
}

#[tokio::test]
async fn canonical_and_namespaced_aliases_for_same_tool_are_not_ambiguous() {
    let registry = registry().await;
    let count = Arc::new(AtomicUsize::new(0));
    registry.tools.write().await.insert(
        "read".into(),
        Arc::new(CountTool(
            "read",
            count.clone(),
            json!({"type":"object", "properties":{"text":{"type":"string"}}}),
        )),
    );

    let output = registry
        .execute(
            "batch",
            json!({"tool_calls":[{
                "tool":"functions.read", "name":"read", "text":"same route"
            }]}),
            context("canonical-alias-collision".into()),
        )
        .await
        .expect("canonical and namespaced selectors resolve to one actual tool");
    assert!(
        output.output.contains("Completed: 1 succeeded, 0 failed"),
        "{}",
        output.output
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn pattern_properties_preserve_full_native_arguments_in_sdk_callback() {
    sdk_native_arguments_case(
        json!({"type":"object", "patternProperties":{"^arguments$":{"type":"object"}}}),
    )
    .await;
}

#[tokio::test]
async fn explicitly_open_schemas_preserve_native_arguments_in_sdk_callbacks() {
    for additional in [json!(true), json!({"type":"object"})] {
        sdk_native_arguments_case(json!({"type":"object", "additionalProperties":additional}))
            .await;
    }
}

async fn sdk_native_arguments_case(schema: Value) {
    use crate::protocol::{ServerEvent, SessionToolConfig, SessionToolDefinition};
    struct Cleanup(String);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            crate::tool::sdk::remove_session(&self.0);
            crate::tool::clear_session_tool_policy(&self.0);
        }
    }

    let session = format!("batch-pattern-schema-{}", uuid::Uuid::new_v4());
    let _cleanup = Cleanup(session.clone());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    crate::tool::sdk::configure(
        &session,
        "batch-pattern-tests",
        SessionToolConfig {
            custom: vec![SessionToolDefinition {
                name: "pattern_tool".into(),
                description: "Pattern schema probe".into(),
                parameters: schema,
            }],
            ..Default::default()
        },
        tx,
    )
    .unwrap();
    let registry = registry().await;
    let call_session = session.clone();
    let task = tokio::spawn(async move {
        registry
            .execute(
                "batch",
                json!({"tool_calls":[{
                    "tool":"pattern_tool", "arguments":{"id":"native"}
                }]}),
                context(call_session),
            )
            .await
    });
    let ServerEvent::ToolCall {
        call_id,
        name,
        input,
        session_id,
    } = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap()
    else {
        panic!("expected SDK custom-tool callback");
    };
    assert_eq!(name, "pattern_tool");
    assert_eq!(session_id, session);
    assert_eq!(input, json!({"arguments":{"id":"native"}}));
    crate::tool::sdk::complete(
        "batch-pattern-tests",
        &session,
        &call_id,
        "pattern callback".into(),
        None,
    )
    .unwrap();
    let output = task.await.unwrap().unwrap();
    assert!(output.output.contains("pattern callback"));
}
