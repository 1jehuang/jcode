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
async fn test_toggle_enable_missing_server() {
    let tool = create_test_tool();
    let ctx = create_test_context();
    let input = json!({"action": "enable", "server": "nonexistent"});

    let result = tool.execute(input, ctx).await;
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("not found in MCP config")
    );
}

#[tokio::test]
async fn test_toggle_disable_missing_server() {
    let tool = create_test_tool();
    let ctx = create_test_context();
    let input = json!({"action": "disable", "server": "nonexistent"});

    let result = tool.execute(input, ctx).await;
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("not found in MCP config")
    );
}

#[tokio::test]
async fn test_toggle_enable_missing_server_field() {
    let tool = create_test_tool();
    let ctx = create_test_context();
    let input = json!({"action": "enable"});

    let result = tool.execute(input, ctx).await;
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("'server' is required")
    );
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

// --- Happy-path tests for toggle_server and emit_mcp_status ---

/// RAII guard that sets an env var for the duration of a test and restores
/// the previous value (or removes it) on drop. Mirrors the helper used in
/// `ambient/runner_tests.rs`; kept local so this module stays self-contained.
struct EnvVarGuard {
    key: &'static str,
    prev: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set_path(key: &'static str, value: &std::path::Path) -> Self {
        let prev = std::env::var_os(key);
        crate::env::set_var(key, value);
        Self { key, prev }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(prev) = self.prev.take() {
            crate::env::set_var(self.key, prev);
        } else {
            crate::env::remove_var(self.key);
        }
    }
}

/// Write a single-server MCP config to `<jcode_home>/mcp.json` (the global
/// config file jcode reads via `jcode_dir()`). `disabled=true` starts the
/// server disabled so an `enable` toggle has flags to flip; `disabled=false`
/// starts it enabled for a `disable` toggle.
///
/// The command is a deliberately nonexistent binary so the post-save
/// `connect` attempt in the enable path fails fast at `spawn()` (ENOENT)
/// instead of hanging on the 30s JSON-RPC request timeout a process that
/// exits without speaking MCP would trigger. The config save happens
/// before that connect attempt, so persistence is testable either way.
fn write_global_mcp_config(jcode_home: &std::path::Path, server_name: &str, disabled: bool) {
    let config = json!({
        "servers": {
            server_name: {
                "command": "jcode-mcp-test-nonexistent-server",
                "args": [],
                "env": {},
                "shared": false,
                "disabled": disabled,
            }
        }
    });
    fs::write(jcode_home.join("mcp.json"), config.to_string()).expect("write global mcp.json");
}

/// Read back the global `mcp.json` as a JSON value so tests can assert on
/// the persisted enabled/disabled flags after a toggle.
fn read_global_mcp_config(jcode_home: &std::path::Path) -> serde_json::Value {
    let content =
        fs::read_to_string(jcode_home.join("mcp.json")).expect("read global mcp.json after toggle");
    serde_json::from_str(&content).expect("mcp.json is valid JSON")
}

/// Enable a configured-but-disabled server and verify the enabled/disabled
/// flags are flipped and persisted to `~/.jcode/mcp.json` on disk. The
/// post-save connect uses `true` (exits immediately, no MCP handshake) so it
/// fails fast without hanging; the config save happens before that attempt.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_toggle_enable_persists_flags_to_disk() {
    let _env = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("temp jcode home");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let server_name = "toggle-enable-srv";
    write_global_mcp_config(temp.path(), server_name, true);

    let tool = create_test_tool();
    let ctx = create_test_context();
    let input = json!({"action": "enable", "server": server_name});

    let result = tokio::time::timeout(std::time::Duration::from_secs(15), tool.execute(input, ctx))
        .await
        .expect("toggle enable must not hang")
        .expect("toggle enable returns Ok even if the connect fails");

    // Config persistence is the key behavior: flags flipped to enabled.
    let saved = read_global_mcp_config(temp.path());
    let saved_server = saved
        .get("servers")
        .and_then(|s| s.get(server_name))
        .expect("server present after enable toggle");
    assert_eq!(saved_server["enabled"], json!(true));
    assert_eq!(saved_server["disabled"], json!(false));

    assert!(
        result.output.to_lowercase().contains("enabled"),
        "enable toggle should report enabled: {}",
        result.output
    );
}

/// Disable a configured-and-enabled server and verify the flags are flipped
/// and persisted to disk. The server is never connected, so the disable
/// path skips the disconnect/subprocess entirely.
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_toggle_disable_persists_flags_to_disk() {
    let _env = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("temp jcode home");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let server_name = "toggle-disable-srv";
    write_global_mcp_config(temp.path(), server_name, false);

    let tool = create_test_tool();
    let ctx = create_test_context();
    let input = json!({"action": "disable", "server": server_name});

    let result = tool
        .execute(input, ctx)
        .await
        .expect("toggle disable returns Ok");

    let saved = read_global_mcp_config(temp.path());
    let saved_server = saved
        .get("servers")
        .and_then(|s| s.get(server_name))
        .expect("server present after disable toggle");
    assert_eq!(saved_server["enabled"], json!(false));
    assert_eq!(saved_server["disabled"], json!(true));

    assert!(
        result.output.contains("Disabled MCP server"),
        "disable toggle should report disabled: {}",
        result.output
    );
}

/// A failed mcp.json save must stop the toggle: no (dis)connect, no registry
/// change, and honest reporting instead of a "Disabled ... Config NOT saved"
/// claim that `mcp_search`/`mcp_call` would immediately contradict by
/// re-advertising the server. (Greptile 4307539754)
///
/// The save is forced to fail by making JCODE_HOME read-only: the lock file
/// cannot be created, `persist_toggle` reports `save_ok=false`, and the tool
/// must bail out before any state mutation. Permissions are restored before
/// the asserts so the tempdir still cleans up on a panic.
#[cfg(unix)]
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn test_toggle_disable_reports_failure_and_keeps_state_on_save_error() {
    let _env = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("temp jcode home");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let server_name = "toggle-save-fail-srv";
    write_global_mcp_config(temp.path(), server_name, false);

    // Drop write permission on the jcode home so the toggle cannot persist.
    let mut perms = std::fs::metadata(temp.path())
        .expect("stat jcode home")
        .permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    {
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o555);
    }
    std::fs::set_permissions(temp.path(), perms).expect("chmod jcode home read-only");

    let manager = Arc::new(RwLock::new(McpManager::with_config(
        crate::mcp::McpConfig::default(),
    )));
    let tool = McpManagementTool::new(manager.clone());
    let ctx = create_test_context();
    let input = json!({"action": "disable", "server": server_name});

    let result = tokio::time::timeout(std::time::Duration::from_secs(15), tool.execute(input, ctx))
        .await
        .expect("failed toggle must not hang")
        .expect("save failure is reported in-band, not as an error");

    // Restore write permission before asserting so the tempdir can be removed.
    #[allow(clippy::permissions_set_readonly_false)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(temp.path())
            .expect("stat jcode home")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(temp.path(), perms).expect("chmod jcode home writable");
    }

    assert!(
        result.output.contains("Failed to disable MCP server"),
        "save failure must be reported as a failed toggle: {}",
        result.output
    );
    assert!(
        result.output.contains("config not saved"),
        "the user must learn why the toggle did not apply: {}",
        result.output
    );
    assert!(
        !result.output.contains("Disabled MCP server"),
        "a failed save must not be reported as a successful disable: {}",
        result.output
    );

    // On-disk state is untouched: the server stays enabled.
    let saved = read_global_mcp_config(temp.path());
    let saved_server = saved
        .get("servers")
        .and_then(|s| s.get(server_name))
        .expect("server still present after failed toggle");
    // The fixture writes only `disabled`; a failed toggle must leave it as-is.
    assert_eq!(saved_server["disabled"], json!(false));
    assert!(
        saved_server.get("enabled").is_none() || saved_server["enabled"] == json!(true),
        "a failed save must not flip the enabled flag on disk"
    );

    // The manager's in-memory config is never reloaded/mutated on failure.
    let configured = manager
        .read()
        .await
        .config()
        .servers
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    assert!(
        !configured.contains(&server_name.to_string()),
        "manager config must stay untouched when the save fails"
    );
}

/// With no connected servers, `emit_mcp_status` still sends an
/// `McpStatus` event with an empty list through the event channel.
#[tokio::test]
async fn test_emit_mcp_status_no_connected_servers() {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tool = create_test_tool().with_event_tx(Some(tx));

    tool.emit_mcp_status().await;

    let event = rx
        .try_recv()
        .expect("McpStatus event should be emitted on the channel");
    match event {
        crate::protocol::ServerEvent::McpStatus { servers } => {
            assert!(
                servers.is_empty(),
                "no connected servers -> empty status, got {servers:?}"
            );
        }
        other => panic!("expected McpStatus, got {other:?}"),
    }
}

/// Write a minimal stdio MCP server (a shell script) that answers
/// `initialize` and `tools/list` (exposing one tool) so a real connect via
/// `McpManager` succeeds and `all_tools()` is non-empty.
#[cfg(unix)]
fn write_fake_mcp_server(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("jcode-fake-mcp-server.sh");
    let script = r##"#!/bin/bash
while IFS= read -r line; do
  id=$(echo "$line" | grep -o '"id":[0-9]*' | grep -o '[0-9]*' | head -1)
  case "$line" in
*'"initialize"'*)
  echo '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"jcode-test-fake","version":"0.0.1"}}}'
  ;;
*'"tools/list"'*)
  echo '{"jsonrpc":"2.0","id":'"$id"',"result":{"tools":[{"name":"fake_tool","description":"fake tool for emit test","inputSchema":{"type":"object"}}]}}'
  ;;
*'"shutdown"'*)
  exit 0
  ;;
  esac
done
"##;
    fs::write(&path, script).expect("write fake mcp server script");
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
    path
}

/// Happy path for `emit_mcp_status`: a connected server with tools is
/// formatted as `"name:count"` in the `McpStatus` event payload.
#[tokio::test]
#[cfg(unix)]
async fn test_emit_mcp_status_formats_name_and_tool_count() {
    let temp = tempfile::tempdir().expect("temp dir for fake mcp server");
    let command = write_fake_mcp_server(temp.path());
    let server_name = "jcode-test-fake".to_string();

    let mut config = crate::mcp::McpConfig::default();
    config.servers.insert(
        server_name.clone(),
        McpServerConfig {
            command: command.to_string_lossy().to_string(),
            args: vec![],
            env: HashMap::new(),
            shared: false,
            transport: None,
            url: None,
            headers: HashMap::new(),
            enabled: None,
            disabled: None,
            timeout_secs: None,
        },
    );
    let manager = Arc::new(RwLock::new(McpManager::with_config(config.clone())));
    let server_config = config
        .servers
        .get(&server_name)
        .cloned()
        .expect("server config present");

    // Connect the fake server through the real manager so
    // connected_servers() and all_tools() report it.
    manager
        .read()
        .await
        .connect(&server_name, &server_config)
        .await
        .expect("fake MCP server must connect");

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tool = McpManagementTool::new(Arc::clone(&manager)).with_event_tx(Some(tx));
    tool.emit_mcp_status().await;

    let event = rx
        .try_recv()
        .expect("McpStatus event should be emitted after connect");
    match event {
        crate::protocol::ServerEvent::McpStatus { servers } => {
            assert_eq!(
                servers,
                vec![format!("{}:1", server_name)],
                "status should be 'name:count' for the single connected server, got {servers:?}"
            );
        }
        other => panic!("expected McpStatus, got {other:?}"),
    }

    // Clean up the subprocess.
    manager.read().await.disconnect_all().await;
}

/// `with_event_tx(Some(tx))` wires the event channel so
/// `emit_mcp_status` emits an `McpStatus` event. With no servers connected
/// the payload is an empty list, which still proves the channel was set.
#[tokio::test]
async fn test_with_event_tx_some_sets_event_channel() {
    let manager = Arc::new(RwLock::new(McpManager::with_config(
        crate::mcp::McpConfig::default(),
    )));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tool = McpManagementTool::new(Arc::clone(&manager)).with_event_tx(Some(tx));
    tool.emit_mcp_status().await;
    match rx
        .try_recv()
        .expect("McpStatus event should be emitted when event_tx is Some")
    {
        crate::protocol::ServerEvent::McpStatus { servers } => {
            assert!(
                servers.is_empty(),
                "no servers connected, expected an empty status list"
            );
        }
        other => panic!("expected McpStatus, got {other:?}"),
    }
}

/// `with_event_tx(None)` leaves the tool with no channel, so
/// `emit_mcp_status` returns early without emitting anything and without
/// panicking.
#[tokio::test]
async fn test_with_event_tx_none_emits_nothing() {
    let manager = Arc::new(RwLock::new(McpManager::with_config(
        crate::mcp::McpConfig::default(),
    )));
    let tool = McpManagementTool::new(Arc::clone(&manager)).with_event_tx(None);
    // With no event_tx the helper short-circuits; this must not panic and
    // emits no event (there is no channel to observe, so no-panic + return
    // is the contract).
    tool.emit_mcp_status().await;
}
