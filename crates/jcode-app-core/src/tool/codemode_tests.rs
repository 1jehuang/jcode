use super::*;
use crate::tool::Registry;
use std::collections::HashSet;

#[test]
fn description_is_compact_and_preserves_execution_contract() {
    let registry = Registry::empty();
    let tool = CodemodeTool::new(registry.downgrade());
    let description = tool.to_definition().description;
    for clause in [
        "async function body",
        "await/return",
        "tools.<name>(args)",
        "callTool(name, args)",
        "failures throw",
        "Promise.all",
        "listTools(filter?)",
        "describeTool(name)",
        "store(key, value)",
        "load(key)",
        "storedKeys()",
        "console.log",
        "No direct filesystem, network, timers, or modules",
        "64 MB heap, 300 s, 2000 tool calls",
    ] {
        assert!(description.contains(clause), "Missing contract: {clause}");
    }
    assert!(description.len() <= 750);
}

struct JsonTool;

#[async_trait::async_trait]
impl Tool for JsonTool {
    fn name(&self) -> &str {
        "lookup"
    }

    fn description(&self) -> &str {
        "Return structured data for an id\nMore detail."
    }

    fn parameters_schema(&self) -> Value {
        json!({"type": "object", "properties": {"id": {"type": "number"}}})
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        let id = input["id"].as_i64().unwrap_or_default();
        Ok(ToolOutput::new(
            json!({ "id": id, "square": id * id }).to_string(),
        ))
    }
}

struct TextTool;

#[async_trait::async_trait]
impl Tool for TextTool {
    fn name(&self) -> &str {
        "shout"
    }

    fn description(&self) -> &str {
        "Uppercase text"
    }

    fn parameters_schema(&self) -> Value {
        json!({"type": "object"})
    }

    async fn execute(&self, input: Value, _ctx: ToolContext) -> Result<ToolOutput> {
        Ok(ToolOutput::new(
            input["text"].as_str().unwrap_or_default().to_uppercase(),
        ))
    }
}

fn ctx(session: &str) -> ToolContext {
    ToolContext {
        session_id: session.to_string(),
        message_id: "message".to_string(),
        tool_call_id: "codemode-call".to_string(),
        working_dir: None,
        stdin_request_tx: None,
        decision_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: super::super::ToolExecutionMode::Direct,
    }
}

async fn registry() -> Registry {
    let registry = Registry::empty();
    let mut tools = registry.tools.write().await;
    tools.insert("lookup".to_string(), Arc::new(JsonTool));
    tools.insert("shout".to_string(), Arc::new(TextTool));
    tools.insert(
        "codemode".to_string(),
        Arc::new(CodemodeTool::new(registry.downgrade())),
    );
    drop(tools);
    registry
}

#[tokio::test(flavor = "multi_thread")]
async fn codemode_exposes_only_unified_mcp_and_can_search() {
    let registry = registry().await;
    let manager = Arc::new(tokio::sync::RwLock::new(
        crate::mcp::McpManager::with_config(Default::default()),
    ));
    registry
        .register(
            "mcp".into(),
            Arc::new(
                super::super::mcp::McpManagementTool::new(manager.clone())
                    .with_registry(registry.clone()),
            ),
        )
        .await;
    registry
        .register(
            "mcp_search".into(),
            Arc::new(
                super::super::mcp::McpSearchTool::new(manager.clone())
                    .with_registry(registry.clone()),
            ),
        )
        .await;
    registry
        .register(
            "mcp_call".into(),
            Arc::new(super::super::mcp::McpCallTool::new(manager).with_registry(registry.clone())),
        )
        .await;
    let out = registry
        .execute(
            "codemode",
            json!({"code": r#"
        const names = listTools().map(t => t.name).filter(n => n.startsWith('mcp'));
        const found = await tools.mcp({action: 'search'});
        return {names, found};
    "#}),
            ctx("codemode-unified-mcp"),
        )
        .await
        .unwrap();
    assert!(out.output.contains("\"mcp\""), "{}", out.output);
    assert!(!out.output.contains("mcp_search"), "{}", out.output);
    assert!(!out.output.contains("mcp_call"), "{}", out.output);
    assert!(out.output.contains("\"found\": []"), "{}", out.output);
}

#[tokio::test(flavor = "multi_thread")]
async fn script_composes_registry_tools() {
    let registry = registry().await;
    let out = registry
        .execute(
            "codemode",
            json!({
                "code": r#"
                    const rows = await Promise.all([1, 2, 3].map((id) => tools.lookup({ id })));
                    const loud = await tools.shout({ text: "done" });
                    console.log("rows", rows.length);
                    return { total: rows.reduce((s, r) => s + r.square, 0), loud };
                "#
            }),
            ctx("codemode-compose-test"),
        )
        .await
        .expect("codemode runs");
    assert!(out.output.contains("\"total\": 14"), "{}", out.output);
    assert!(out.output.contains("\"loud\": \"DONE\""), "{}", out.output);
    assert!(out.output.contains("console:\nrows 3"), "{}", out.output);
    assert!(out.output.contains("[4 tool calls"), "{}", out.output);
}

#[tokio::test(flavor = "multi_thread")]
async fn script_cannot_call_itself_or_batch() {
    let registry = registry().await;
    let out = registry
        .execute(
            "codemode",
            json!({ "code": "return listTools().map((t) => t.name);" }),
            ctx("codemode-exclusion-test"),
        )
        .await
        .expect("codemode runs");
    assert!(out.output.contains("lookup"), "{}", out.output);
    assert!(!out.output.contains("\"codemode\""), "{}", out.output);

    let err = registry
        .execute(
            "codemode",
            json!({ "code": "return await tools.codemode({ code: 'return 1' });" }),
            ctx("codemode-exclusion-test"),
        )
        .await;
    let text = match err {
        Ok(out) => out.output,
        Err(err) => err.to_string(),
    };
    assert!(text.contains("unknown tool 'codemode'"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn session_policy_applies_inside_scripts() {
    let session = "codemode-policy-test";
    crate::tool::set_session_tool_policy(session, None, HashSet::from(["shout".to_string()]));
    let registry = registry().await;
    let out = registry
        .execute(
            "codemode",
            json!({ "code": "return [listTools().map((t) => t.name), 'shout' in tools];" }),
            ctx(session),
        )
        .await;
    crate::tool::clear_session_tool_policy(session);
    let out = out.expect("codemode runs");
    assert!(!out.output.contains("\"shout\""), "{}", out.output);
    assert!(out.output.contains("false"), "{}", out.output);
}

#[tokio::test(flavor = "multi_thread")]
async fn uncaught_errors_without_side_effects_are_tool_errors() {
    let registry = registry().await;
    let err = registry
        .execute(
            "codemode",
            json!({ "code": "throw new Error('broken script')" }),
            ctx("codemode-error-test"),
        )
        .await
        .expect_err("pure failure is a tool error");
    assert!(err.to_string().contains("broken script"), "{err}");
}

#[test]
fn report_renders_strings_raw_and_lists_store_keys() {
    let mut store = Map::new();
    store.insert("k".into(), json!(1));
    let out = jcode_codemode::RunOutput {
        result: Some(json!("plain text")),
        tool_calls: 1,
        store,
        ..Default::default()
    };
    let report = render_report(&out);
    assert!(report.starts_with("result:\nplain text\n"), "{report}");
    assert!(report.contains("[1 tool call, "), "{report}");
    assert!(report.contains("stored: k]"), "{report}");
}
