#![cfg_attr(test, allow(clippy::await_holding_lock))]

use crate::mcp::{McpConfig, McpManager, SharedMcpPool};
use crate::message::{Message, ToolDefinition};
use crate::provider::{EventStream, Provider};
use crate::tool::mcp::{McpCallTool, McpManagementTool, McpSearchTool};
use crate::tool::{Registry, Tool};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::RwLock;

struct SchemaSweepMockProvider;

#[async_trait]
impl Provider for SchemaSweepMockProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> anyhow::Result<EventStream> {
        Err(anyhow::anyhow!(
            "SchemaSweepMockProvider does not stream completions"
        ))
    }

    fn name(&self) -> &str {
        "mock"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(SchemaSweepMockProvider)
    }
}

fn contains_array_type_keyword(schema: &Value) -> bool {
    match schema {
        Value::Object(map) => {
            matches!(map.get("type"), Some(Value::Array(_)))
                || map.iter().any(|(k, v)| {
                    if matches!(
                        jcode_schema_dialect::keyword_role(k),
                        jcode_schema_dialect::KeywordRole::Data
                    ) {
                        false
                    } else {
                        contains_array_type_keyword(v)
                    }
                })
        }
        Value::Array(items) => items.iter().any(contains_array_type_keyword),
        _ => false,
    }
}

#[tokio::test]
async fn every_builtin_and_optional_tool_schema_conforms_to_gemini_and_antigravity_dialects() {
    let _env_lock = crate::storage::lock_test_env();
    let temp_dir = tempfile::tempdir().expect("isolated temp dir");

    let provider: Arc<dyn Provider> = Arc::new(SchemaSweepMockProvider);
    let registry = Registry::new(provider).await;
    registry.register_selfdev_tools().await;
    registry.register_ambient_tools().await;
    registry.set_desktop_ui_enabled(true).await;

    let pool = Arc::new(SharedMcpPool::new(McpConfig::default()));
    registry
        .register_mcp_tools_for_dir(
            None,
            Some(pool),
            Some("schema-sweep".to_string()),
            Some(temp_dir.path().to_path_buf()),
        )
        .await;

    let mut by_name: BTreeMap<String, ToolDefinition> = registry
        .definitions(None)
        .await
        .into_iter()
        .map(|def| (def.name.clone(), def))
        .collect();

    // Include fixed MCP surface tools directly even when hidden from default
    // model-visible listings so deferred/legacy MCP surfaces never regress.
    let mcp_manager = Arc::new(RwLock::new(McpManager::new()));
    for extra in [
        McpManagementTool::new(Arc::clone(&mcp_manager)).to_definition(),
        McpSearchTool::new(Arc::clone(&mcp_manager)).to_definition(),
        McpCallTool::new(Arc::clone(&mcp_manager)).to_definition(),
    ] {
        by_name.entry(extra.name.clone()).or_insert(extra);
    }

    for expected in [
        "todo",
        "swarm",
        "mcp",
        "mcp_search",
        "mcp_call",
        "applet",
        "user_response",
        "selfdev",
        "debug_socket",
        "end_ambient_cycle",
        "schedule_ambient",
        "request_permission",
        "send_message",
    ] {
        assert!(
            by_name.contains_key(expected),
            "expected built-in/optional tool `{expected}` in schema sweep"
        );
    }

    let mut failures = Vec::new();
    for spec in [
        &jcode_schema_dialect::registry::GEMINI,
        &jcode_schema_dialect::registry::ANTIGRAVITY_CLAUDE,
        &jcode_schema_dialect::registry::ANTIGRAVITY_BRIDGE,
    ] {
        for def in by_name.values() {
            let normalized = jcode_schema_dialect::dialect::apply(&def.input_schema, spec);
            for err in
                jcode_schema_dialect::must_not_contain_unsupported_constructs(&normalized, spec)
            {
                failures.push(format!("[{}] tool `{}` {err}", spec.id, def.name));
            }
            for err in jcode_schema_dialect::must_preserve_meaning(&def.input_schema, &normalized) {
                failures.push(format!(
                    "[{}] tool `{}` lost meaning: {err}",
                    spec.id, def.name
                ));
            }
            if contains_array_type_keyword(&normalized) {
                failures.push(format!(
                    "[{}] tool `{}` retained an array `type`: {normalized}",
                    spec.id, def.name
                ));
            }
            if jcode_schema_dialect::dialect::apply(&normalized, spec) != normalized {
                failures.push(format!(
                    "[{}] tool `{}` normalization is not idempotent",
                    spec.id, def.name
                ));
            }
        }

        // Pin `todo` and `swarm` invariants that previously caused live HTTP 400s.
        let todo_norm = jcode_schema_dialect::dialect::apply(&by_name["todo"].input_schema, spec);
        let add_item = &todo_norm["properties"]["add"]["items"];
        assert_eq!(add_item["type"], json!("object"), "{}", spec.id);
        assert!(
            add_item["properties"].is_object()
                && !add_item["properties"].as_object().unwrap().is_empty(),
            "{}: todo.add.items.properties must be populated: {add_item}",
            spec.id
        );

        let swarm_norm = jcode_schema_dialect::dialect::apply(&by_name["swarm"].input_schema, spec);
        assert_eq!(swarm_norm["type"], json!("object"), "{}", spec.id);
        assert!(swarm_norm.get("anyOf").is_none(), "{}", spec.id);
        assert!(swarm_norm.get("oneOf").is_none(), "{}", spec.id);
        assert!(swarm_norm.get("allOf").is_none(), "{}", spec.id);
    }

    assert!(
        failures.is_empty(),
        "built-in/optional tool schemas failed Gemini/Antigravity conformance:\n{}",
        failures.join("\n")
    );
}
