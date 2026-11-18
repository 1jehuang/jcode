use super::*;

#[test]
fn base_tools_do_not_expose_invalid_reporting_tool() {
    let skills = Arc::new(RwLock::new(crate::skill::SkillRegistry::default()));
    let tools = Registry::base_tools(&skills);
    assert!(!tools.contains_key("invalid"));
    assert!(tools.contains_key("bash"));
}

#[test]
fn base_tools_exclude_replace_and_keep_other_editing_tools() {
    let skills = Arc::new(RwLock::new(crate::skill::SkillRegistry::default()));
    let tools = Registry::base_tools(&skills);
    assert!(!tools.contains_key("replace"));
    for name in ["edit", "write", "apply_patch"] {
        assert!(tools.contains_key(name), "missing editing tool: {name}");
    }
}

#[tokio::test]
async fn mcp_list_remains_available_while_background_connect_is_handshaking() {
    let _env_lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().expect("create isolated JCODE_HOME");
    let _home_guard = TestHomeGuard::new(home.path());
    let working_dir = tempfile::tempdir().expect("create isolated MCP working directory");

    std::fs::write(
        home.path().join("mcp.json"),
        r#"{
            "mcpServers": {
                "slow": {
                    "command": "/bin/sh",
                    "args": ["-c", "sleep 2"],
                    "timeout_secs": 86400
                }
            }
        }"#,
    )
    .expect("write slow MCP config");

    let registry = Registry::empty();
    registry
        .register_mcp_tools_for_dir(None, None, None, Some(working_dir.path().to_path_buf()))
        .await;

    // Let the background connection task acquire its read guard and enter the
    // slow initialize handshake before asking the management tool to list.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let output = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        registry.execute(
            "mcp",
            serde_json::json!({"action": "list"}),
            mcp_test_context(working_dir.path()),
        ),
    )
    .await
    .expect("mcp list must not wait for a slow background handshake")
    .expect("mcp list should succeed");

    assert!(
        output.output.contains("slow"),
        "unexpected output: {}",
        output.output
    );
}

#[tokio::test]
async fn real_mcp_registration_does_not_retain_registry_tool_map() {
    let _env_lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().expect("create isolated JCODE_HOME");
    let _home_guard = TestHomeGuard::new(home.path());
    let working_dir = tempfile::tempdir().expect("create isolated MCP working directory");
    let registry = Registry::empty();
    let tools = Arc::downgrade(&registry.tools);

    register_empty_mcp_tools(&registry, working_dir.path()).await;
    assert!(registry.tool_names().await.iter().any(|name| name == "mcp"));

    drop(registry);

    assert!(
        tools.upgrade().is_none(),
        "McpManagementTool must not strongly retain the registry tool map that owns it"
    );
}

#[tokio::test]
async fn mcp_management_upgrades_registry_through_surviving_clone() {
    let _env_lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().expect("create isolated JCODE_HOME");
    let _home_guard = TestHomeGuard::new(home.path());
    let working_dir = tempfile::tempdir().expect("create isolated MCP working directory");
    let registry = Registry::empty();
    let tools = Arc::downgrade(&registry.tools);

    register_empty_mcp_tools(&registry, working_dir.path()).await;
    let surviving_clone = registry.clone();
    drop(registry);

    let stale_tool = surviving_clone
        .tools
        .read()
        .await
        .get("mcp")
        .cloned()
        .expect("MCP management tool should be registered");
    surviving_clone
        .register("mcp__lifetime__sentinel".to_string(), stale_tool)
        .await;

    let output = surviving_clone
        .execute(
            "mcp",
            serde_json::json!({"action": "reload"}),
            mcp_test_context(working_dir.path()),
        )
        .await
        .expect("MCP management should upgrade through the surviving registry clone");
    assert!(output.output.contains("No servers found in config"));
    assert!(
        !surviving_clone
            .tool_names()
            .await
            .iter()
            .any(|name| name == "mcp__lifetime__sentinel"),
        "reload should mutate the surviving registry through the weak handle"
    );
    assert!(
        surviving_clone
            .tool_names()
            .await
            .iter()
            .any(|name| name == "mcp"),
        "reload should preserve the MCP management tool"
    );
    assert!(tools.upgrade().is_some());

    drop(surviving_clone);
    assert!(tools.upgrade().is_none());
}
