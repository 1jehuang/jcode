use super::*;
use crate::tool::Tool;
use std::fs;
use std::path::PathBuf;

fn create_test_tool() -> McpManagementTool {
    // Use an explicit empty config so tests are hermetic: McpManager::new()
    // would load the developer's real ~/.jcode/mcp.json, and list output
    // now includes configured-but-not-connected servers (issue #436).
    let manager = Arc::new(RwLock::new(McpManager::with_config(
        crate::mcp::McpConfig::default(),
    )));
    McpManagementTool::new(manager)
}

fn create_test_context() -> ToolContext {
    ToolContext {
        session_id: "test-session".to_string(),
        message_id: "test-message".to_string(),
        tool_call_id: "test-tool-call".to_string(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: crate::tool::ToolExecutionMode::Direct,
    }
}

struct LocalMcpConfigGuard {
    path: PathBuf,
    backup: Option<String>,
    created_dir: bool,
}

impl LocalMcpConfigGuard {
    fn new(content: &str) -> std::io::Result<Self> {
        let path = PathBuf::from(".jcode/mcp.json");
        let dir = path
            .parent()
            .ok_or_else(|| std::io::Error::other("missing parent"))?;
        let created_dir = if !dir.exists() {
            fs::create_dir_all(dir)?;
            true
        } else {
            false
        };
        let backup = if path.exists() {
            Some(fs::read_to_string(&path)?)
        } else {
            None
        };
        fs::write(&path, content)?;
        Ok(Self {
            path,
            backup,
            created_dir,
        })
    }
}

impl Drop for LocalMcpConfigGuard {
    fn drop(&mut self) {
        match &self.backup {
            Some(content) => {
                let _ = fs::write(&self.path, content);
            }
            None => {
                let _ = fs::remove_file(&self.path);
                if self.created_dir
                    && let Some(dir) = self.path.parent()
                {
                    let _ = fs::remove_dir(dir);
                }
            }
        }
    }
}

#[test]
fn test_tool_name() {
    let tool = create_test_tool();
    assert_eq!(tool.name(), "mcp");
}

#[test]
fn test_tool_description() {
    let tool = create_test_tool();
    assert!(tool.description().contains("MCP"));
    assert!(tool.description().contains("Model Context Protocol"));
}

#[test]
fn test_parameters_schema() {
    let tool = create_test_tool();
    let schema = tool.parameters_schema();
    assert_eq!(schema["type"], "object");
    assert!(schema["properties"]["action"].is_object());
    assert!(schema["properties"]["server"].is_object());
    assert!(schema["properties"]["command"].is_object());
    assert_eq!(schema["required"], json!(["action"]));
    assert!(
        schema["properties"]["action"]["enum"]
            .as_array()
            .unwrap()
            .contains(&json!("search"))
    );
    assert!(
        schema["properties"]["action"]["enum"]
            .as_array()
            .unwrap()
            .contains(&json!("call"))
    );
    assert_eq!(
        schema["properties"]["arguments"]["additionalProperties"],
        true
    );
}

async fn unified_registry() -> crate::tool::Registry {
    let registry = crate::tool::Registry::empty();
    let manager = Arc::clone(create_test_tool().manager());
    registry
        .register(
            "mcp".into(),
            Arc::new(McpManagementTool::new(manager.clone()).with_registry(registry.clone())),
        )
        .await;
    registry
        .register(
            "mcp_search".into(),
            Arc::new(McpSearchTool::new(manager.clone()).with_registry(registry.clone())),
        )
        .await;
    registry
        .register(
            "mcp_call".into(),
            Arc::new(McpCallTool::new(manager).with_registry(registry.clone())),
        )
        .await;
    registry
}

#[tokio::test]
async fn unified_mcp_hides_alias_schemas_but_preserves_execution() {
    let registry = unified_registry().await;
    let definitions = registry.definitions(None).await;
    assert_eq!(
        definitions
            .iter()
            .map(|d| d.name.as_str())
            .collect::<Vec<_>>(),
        vec!["mcp"]
    );
    let ctx = create_test_context();
    let unified = registry
        .execute("mcp", json!({"action":"search"}), ctx.clone())
        .await
        .unwrap();
    let legacy = registry
        .execute("mcp_search", json!({}), ctx.clone())
        .await
        .unwrap();
    assert_eq!(unified.output, "[]");
    assert_eq!(unified.output, legacy.output);
    assert_eq!(unified.metadata, legacy.metadata);
    for (name, input) in [("mcp", json!({"action":"call"})), ("mcp_call", json!({}))] {
        let error = registry
            .execute(name, input, ctx.clone())
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("missing field `server`"),
            "{error}"
        );
    }
    let script_defs = registry.definitions_for_session(&ctx.session_id).await;
    assert_eq!(
        script_defs
            .iter()
            .map(|d| d.name.as_str())
            .collect::<Vec<_>>(),
        vec!["mcp"]
    );
}

#[tokio::test]
async fn unified_mcp_respects_legacy_surface_and_management_policies() {
    use std::collections::HashSet;
    let registry = unified_registry().await;
    let mut ctx = create_test_context();
    ctx.session_id = "unified-mcp-action-policy".into();
    let _policy = crate::tool::register_session_tool_policy(
        &ctx.session_id,
        Some(HashSet::from(["mcp_search".into()])),
        HashSet::new(),
    );
    assert_eq!(
        registry
            .definitions(Some(&HashSet::from(["mcp_search".into()])))
            .await
            .len(),
        1
    );
    assert!(
        registry
            .execute("mcp", json!({"action":"search"}), ctx.clone())
            .await
            .is_ok()
    );
    for action in [
        "call",
        "list",
        "connect",
        "disconnect",
        "reload",
        "status",
        "logout",
    ] {
        let error = registry
            .execute("mcp", json!({"action":action}), ctx.clone())
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("not allowed"),
            "{action}: {error}"
        );
    }
    let _policy = crate::tool::register_session_tool_policy(
        &ctx.session_id,
        Some(HashSet::from(["mcp".into()])),
        HashSet::from(["mcp_call".into()]),
    );
    let error = registry
        .execute(
            "mcp",
            json!({"action":"call", "server":"x", "tool":"y"}),
            ctx.clone(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("disabled"), "{error}");
    assert!(
        registry
            .execute("mcp", json!({"action":"search"}), ctx.clone())
            .await
            .is_ok()
    );
    let _policy = crate::tool::register_session_tool_policy(
        &ctx.session_id,
        Some(HashSet::from(["mcp__x__y".into()])),
        HashSet::from(["mcp__x__y".into()]),
    );
    let error = registry
        .execute(
            "mcp",
            json!({"action":"call", "server":"x", "tool":"y"}),
            ctx,
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("not allowed"), "{error}");
}

#[test]
fn mcp_call_allows_dynamic_argument_keys_in_provider_schemas() {
    let tool = create_test_tool();
    let schema = tool.parameters_schema();
    assert_eq!(
        schema["properties"]["arguments"]["additionalProperties"],
        true
    );

    for spec in [
        &jcode_schema_dialect::registry::OPENROUTER,
        &jcode_schema_dialect::registry::OPENAI,
        &jcode_schema_dialect::registry::ANTHROPIC,
    ] {
        let normalized = jcode_schema_dialect::dialect::apply(&schema, spec);
        let arguments = &normalized["properties"]["arguments"];
        assert_eq!(arguments["type"], "object", "{}", spec.id);
        assert_eq!(arguments["additionalProperties"], true, "{}", spec.id);
        if spec.transforms.require_properties_on_objects {
            // Empty declared properties must not close the dynamic payload (#1214).
            assert_eq!(arguments["properties"], json!({}), "{}", spec.id);
        }
        assert_eq!(normalized["required"], schema["required"], "{}", spec.id);
    }
}

#[test]
fn mcp_call_dynamic_arguments_remain_ineligible_for_openai_strict_mode() {
    let tool = create_test_tool();
    let compatible =
        jcode_provider_core::openai_schema::openai_compatible_schema(&tool.parameters_schema());
    assert!(!jcode_provider_core::openai_schema::schema_supports_strict(
        &compatible
    ));
    assert_eq!(
        compatible["properties"]["arguments"]["additionalProperties"],
        true
    );
}

#[test]
fn search_name_matches_beat_description_matches() {
    let terms = vec!["calendar".to_string()];
    assert_eq!(
        match_rank(
            &terms,
            "mcp__g__calendar_events",
            "g",
            "calendar_events",
            ""
        ),
        Some(MatchRank::Name)
    );
    assert_eq!(
        match_rank(
            &terms,
            "mcp__rho__get_card",
            "rho",
            "get_card",
            "resets on calendar boundaries"
        ),
        Some(MatchRank::Description)
    );
    assert_eq!(match_rank(&terms, "mcp__x__y", "x", "y", "nothing"), None);
    // Every term must match, in any field.
    let terms = vec!["gmail".to_string(), "send".to_string()];
    assert_eq!(
        match_rank(&terms, "mcp__g__gmail_send", "g", "gmail_send", ""),
        Some(MatchRank::Name)
    );
    assert_eq!(
        match_rank(&terms, "mcp__g__gmail_read", "g", "gmail_read", ""),
        None
    );
    // An empty query lists everything.
    assert_eq!(match_rank(&[], "a", "b", "c", ""), Some(MatchRank::Name));
}

#[test]
fn search_descriptions_are_clipped() {
    assert_eq!(clip_description("  short  ", 10), "short");
    let long = "é".repeat(500);
    let clipped = clip_description(&long, 400);
    assert_eq!(clipped.chars().count(), 401);
    assert!(clipped.ends_with('…'));
}

fn search_result(n: usize, description_chars: usize) -> McpSearchResult {
    McpSearchResult {
        name: format!("mcp__s__tool_{n}"),
        server: "s".into(),
        tool: format!("tool_{n}"),
        description: "d".repeat(description_chars),
        input_schema: json!({"type": "object", "properties": {"q": {"type": "string"}}}),
        server_instructions: None,
    }
}

#[test]
fn search_results_fit_the_budget_adaptively() {
    // Small results come back whole, long descriptions included.
    let small = vec![search_result(0, 2_000)];
    let (out, detailed) =
        fit_search_results(&small, SEARCH_TOKEN_BUDGET).expect("serializable search results");
    assert_eq!(detailed, 1);
    assert!(out.contains(&"d".repeat(2_000)));

    // Moderately over budget: every match keeps its schema, descriptions shrink.
    let medium: Vec<_> = (0..10).map(|n| search_result(n, 4_000)).collect();
    let (out, detailed) =
        fit_search_results(&medium, SEARCH_TOKEN_BUDGET).expect("serializable search results");
    assert_eq!(detailed, 10);
    assert!(jcode_core::util::estimate_tokens(&out) <= SEARCH_TOKEN_BUDGET);
    assert!(!out.contains("more matches"));

    // Far over budget: leading matches keep detail, the rest are named.
    let large: Vec<_> = (0..400).map(|n| search_result(n, 1_000)).collect();
    let (out, detailed) =
        fit_search_results(&large, SEARCH_TOKEN_BUDGET).expect("serializable search results");
    assert!(detailed > 0 && detailed < 400, "{detailed}");
    assert!(out.contains(&format!("{} more matches", 400 - detailed)));
    assert!(out.contains("mcp__s__tool_399"));
    assert!(jcode_core::util::estimate_tokens(&out) <= SEARCH_TOKEN_BUDGET + 2_000);

    // Opting in returns everything.
    let (_, detailed) =
        fit_search_results(&large, usize::MAX).expect("serializable search results");
    assert_eq!(detailed, 400);
}

#[tokio::test]
async fn test_list_empty() {
    let tool = create_test_tool();
    let ctx = create_test_context();
    let input = json!({"action": "list"});

    let result = tool.execute(input, ctx).await.unwrap();
    assert!(result.output.contains("No MCP servers connected"));
}

#[tokio::test]
async fn test_list_shows_disabled_configured_server() {
    // Issue #436: disabled servers stay visible in the list with their
    // state, so users can see and enable them on demand.
    let mut config = crate::mcp::McpConfig::default();
    config.servers.insert(
        "off-server".to_string(),
        McpServerConfig {
            command: "some-bin".to_string(),
            args: vec![],
            env: HashMap::new(),
            shared: true,
            transport: None,
            url: None,
            headers: HashMap::new(),
            enabled: Some(false),
            disabled: None,
            timeout_secs: None,
        },
    );
    let manager = Arc::new(RwLock::new(McpManager::with_config(config)));
    let tool = McpManagementTool::new(manager);
    let ctx = create_test_context();

    let result = tool.execute(json!({"action": "list"}), ctx).await.unwrap();
    assert!(
        result.output.contains("off-server"),
        "disabled server must be listed: {}",
        result.output
    );
    assert!(
        result.output.contains("disabled in config"),
        "disabled state must be visible: {}",
        result.output
    );
}

#[tokio::test]
async fn test_connect_missing_server() {
    let tool = create_test_tool();
    let ctx = create_test_context();
    let input = json!({"action": "connect", "command": "/bin/test"});

    let result = tool.execute(input, ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("server"));
}

#[tokio::test]
async fn test_connect_missing_command() {
    let tool = create_test_tool();
    let ctx = create_test_context();
    let input = json!({"action": "connect", "server": "test"});

    let result = tool.execute(input, ctx).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("command"));
}

#[tokio::test]
async fn test_disconnect_not_connected() {
    let tool = create_test_tool();
    let ctx = create_test_context();
    let input = json!({"action": "disconnect", "server": "nonexistent"});

    let result = tool.execute(input, ctx).await.unwrap();
    assert!(result.output.contains("not connected"));
}

#[tokio::test]
async fn test_unknown_action() {
    let tool = create_test_tool();
    let ctx = create_test_context();
    let input = json!({"action": "invalid_action"});

    let result = tool.execute(input, ctx).await.unwrap();
    assert!(result.output.contains("Unknown action"));
}

#[tokio::test]
async fn test_reload_empty_config() {
    let _guard =
        LocalMcpConfigGuard::new("{\"servers\":{}}").expect("create temporary .jcode/mcp.json");
    let tool = create_test_tool();
    let ctx = create_test_context();
    let input = json!({"action": "reload"});

    let result = tool.execute(input, ctx).await.unwrap();
    // With config merging, global config may have servers.
    // If both are empty: "No servers found in config"
    // If global has servers: "Reloaded MCP config" (may show connection failures)
    assert!(
        result.output.contains("No servers")
            || result.output.contains("Empty config")
            || result.output.contains("Connected servers: 0")
            || result.output.contains("Reloaded MCP config")
    );
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn exact_selection_loads_only_advertised_allowed_tools() {
    struct RestoreHome(Option<std::ffi::OsString>);
    impl Drop for RestoreHome {
        fn drop(&mut self) {
            match self.0.take() {
                Some(home) => crate::env::set_var("JCODE_HOME", home),
                None => crate::env::remove_var("JCODE_HOME"),
            }
        }
    }
    let _guard = crate::storage::lock_test_env();
    let _restore = RestoreHome(std::env::var_os("JCODE_HOME"));
    let home = tempfile::tempdir().unwrap();
    crate::env::set_var("JCODE_HOME", home.path());
    let mut config = crate::mcp::McpConfig::default();
    let mut cache = crate::mcp::McpSchemaCache::default();
    for (server, names, enabled) in [
        (
            "alpha",
            vec!["lookup", "lookup_extra", "describe", "x-y", "x_y"],
            true,
        ),
        ("beta", vec!["lookup"], true),
        ("disabled", vec!["lookup"], false),
    ] {
        let server_config: McpServerConfig = serde_json::from_value(json!({
            "command":"must-not-start-exact-selection-fixture", "enabled":enabled
        }))
        .unwrap();
        let tools = names
            .into_iter()
            .map(|name| crate::mcp::McpToolDef {
                name: name.into(),
                description: Some("lookup information".into()),
                input_schema: json!({"type":"object","properties":{"value":{"type":"string"}}}),
            })
            .collect();
        cache.update(server, &server_config, tools);
        config.servers.insert(server.into(), server_config);
    }
    cache.save();
    let manager = Arc::new(RwLock::new(McpManager::with_config(config)));
    let registry = crate::tool::Registry::empty();
    registry
        .register(
            "mcp".into(),
            Arc::new(McpManagementTool::new(manager.clone()).with_registry(registry.clone())),
        )
        .await;
    let mut ctx = create_test_context();
    ctx.session_id = "exact-mcp-selection-fixture".into();
    let query = " select: mcp__alpha__lookup, mcp__alpha__describe, mcp__alpha__lookup ";
    let output = registry
        .execute("mcp", json!({"action":"search","query":query}), ctx.clone())
        .await
        .unwrap();
    let selected: Vec<Value> = serde_json::from_str(&output.output).unwrap();
    assert_eq!(
        selected
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["mcp__alpha__describe", "mcp__alpha__lookup"]
    );
    assert_eq!(
        output.metadata.unwrap()["tool_references"],
        json!(["mcp__alpha__describe", "mcp__alpha__lookup"])
    );
    let schema = McpManagementTool::new(manager.clone()).parameters_schema();
    assert!(
        schema["properties"]["query"]["description"]
            .as_str()
            .unwrap()
            .contains("select:")
    );
    for query in [
        "select:",
        "select: , ",
        "select:unknown",
        "select:mcp__disabled__lookup",
        "select:mcp__alpha__x_y",
    ] {
        let output = registry
            .execute("mcp", json!({"action":"search","query":query}), ctx.clone())
            .await
            .unwrap();
        assert_eq!(output.output, "[]", "query {query}");
        assert_eq!(output.metadata.unwrap()["tool_references"], json!([]));
    }
    let output = registry.execute("mcp", json!({"action":"search","server":"beta","query":"select:mcp__alpha__lookup,mcp__beta__lookup"}), ctx.clone()).await.unwrap();
    let filtered: Vec<Value> = serde_json::from_str(&output.output).unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0]["name"], "mcp__beta__lookup");
    let output = registry
        .execute(
            "mcp",
            json!({"action":"search","query":"lookup"}),
            ctx.clone(),
        )
        .await
        .unwrap();
    let fuzzy: Vec<Value> = serde_json::from_str(&output.output).unwrap();
    assert_eq!(fuzzy.len(), 3);
    let output = registry
        .execute("mcp", json!({"action":"search"}), ctx.clone())
        .await
        .unwrap();
    let advertised: Vec<Value> = serde_json::from_str(&output.output).unwrap();
    let canonical = advertised
        .iter()
        .find(|tool| tool["tool"] == "x-y")
        .unwrap()["name"]
        .as_str()
        .unwrap();
    let output = registry
        .execute(
            "mcp",
            json!({"action":"search","query":format!("select:{canonical}")}),
            ctx.clone(),
        )
        .await
        .unwrap();
    let exact: Vec<Value> = serde_json::from_str(&output.output).unwrap();
    assert_eq!(exact.len(), 1);
    assert_eq!(exact[0]["tool"], "x-y");
    let _policy = crate::tool::register_session_tool_policy(
        &ctx.session_id,
        Some(std::collections::HashSet::from([
            "mcp__alpha__lookup".into()
        ])),
        std::collections::HashSet::new(),
    );
    let output = registry
        .execute("mcp", json!({"action":"search","query":query}), ctx)
        .await
        .unwrap();
    let allowed: Vec<Value> = serde_json::from_str(&output.output).unwrap();
    assert_eq!(allowed.len(), 1);
    assert_eq!(allowed[0]["name"], "mcp__alpha__lookup");
    assert!(manager.read().await.connected_servers().await.is_empty());
}
