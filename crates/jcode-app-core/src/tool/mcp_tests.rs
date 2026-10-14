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
}

#[test]
fn mcp_call_allows_dynamic_argument_keys_in_provider_schemas() {
    let tool = McpCallTool::new(Arc::clone(create_test_tool().manager()));
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
    let tool = McpCallTool::new(Arc::clone(create_test_tool().manager()));
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
    }
}

#[test]
fn search_results_fit_the_budget_adaptively() {
    // Small results come back whole, long descriptions included.
    let small = vec![search_result(0, 2_000)];
    let (out, detailed) = fit_search_results(&small, SEARCH_TOKEN_BUDGET);
    assert_eq!(detailed, 1);
    assert!(out.contains(&"d".repeat(2_000)));

    // Moderately over budget: every match keeps its schema, descriptions shrink.
    let medium: Vec<_> = (0..10).map(|n| search_result(n, 4_000)).collect();
    let (out, detailed) = fit_search_results(&medium, SEARCH_TOKEN_BUDGET);
    assert_eq!(detailed, 10);
    assert!(jcode_core::util::estimate_tokens(&out) <= SEARCH_TOKEN_BUDGET);
    assert!(!out.contains("more matches"));

    // Far over budget: leading matches keep detail, the rest are named.
    let large: Vec<_> = (0..400).map(|n| search_result(n, 1_000)).collect();
    let (out, detailed) = fit_search_results(&large, SEARCH_TOKEN_BUDGET);
    assert!(detailed > 0 && detailed < 400, "{detailed}");
    assert!(out.contains(&format!("{} more matches", 400 - detailed)));
    assert!(out.contains("mcp__s__tool_399"));
    assert!(jcode_core::util::estimate_tokens(&out) <= SEARCH_TOKEN_BUDGET + 2_000);

    // Opting in returns everything.
    let (_, detailed) = fit_search_results(&large, usize::MAX);
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
