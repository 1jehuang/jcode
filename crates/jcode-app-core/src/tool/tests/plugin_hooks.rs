use super::*;

#[cfg(unix)]
#[tokio::test]
async fn registry_execute_pre_tool_hook_blocks_and_allows() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = crate::storage::lock_test_env();
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let registry = Registry::new(provider).await;
    let temp = tempfile::TempDir::new().expect("temp dir");

    // Policy script: block bash calls whose input mentions "secret".
    let policy = temp.path().join("policy.sh");
    std::fs::write(
        &policy,
        "#!/bin/sh\ninput=$(cat)\ncase \"$input\" in\n  *secret*) echo \"no secrets\" >&2; exit 2 ;;\nesac\nexit 0\n",
    )
    .expect("write policy");
    std::fs::set_permissions(&policy, std::fs::Permissions::from_mode(0o755))
        .expect("chmod policy");

    let prev = std::env::var_os("JCODE_HOOK_PRE_TOOL");
    crate::env::set_var("JCODE_HOOK_PRE_TOOL", policy.to_string_lossy().to_string());
    // jcode-base is compiled without cfg(test) here, so the config cache only
    // re-checks env every 500ms; force a reload so the hook is visible now.
    crate::config::invalidate_config_cache();

    let ctx = || ToolContext {
        session_id: "test-pre-tool-hook".to_string(),
        message_id: "test".to_string(),
        tool_call_id: "test".to_string(),
        working_dir: Some(std::env::temp_dir()),
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: ToolExecutionMode::Direct,
    };

    let blocked = registry
        .execute(
            "bash",
            serde_json::json!({
                "command": "echo secret"
            }),
            ctx(),
        )
        .await;
    let allowed = registry
        .execute(
            "bash",
            serde_json::json!({
                "command": "true"
            }),
            ctx(),
        )
        .await;

    match prev {
        Some(value) => crate::env::set_var("JCODE_HOOK_PRE_TOOL", value),
        None => crate::env::remove_var("JCODE_HOOK_PRE_TOOL"),
    }
    crate::config::invalidate_config_cache();

    let error = blocked.expect_err("pre_tool hook should block matching input");
    assert!(
        error.to_string().contains("no secrets"),
        "hook stderr should surface in the error: {error}"
    );
    assert!(allowed.is_ok(), "non-matching input should pass the gate");
}

#[tokio::test]
async fn registry_executes_an_enabled_project_plugin_policy_before_the_tool() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let project = root.join("project");
    let home = root.join("home");
    let market = root.join("market");
    let plugin = market.join("policy");
    for path in [
        &project,
        &market.join(".claude-plugin"),
        &plugin.join(".claude-plugin"),
        &plugin.join("hooks"),
    ] {
        std::fs::create_dir_all(path).unwrap();
    }
    std::fs::write(
        market.join(".claude-plugin/marketplace.json"),
        r#"{"name":"hook-test","plugins":[{"name":"policy","source":"./policy"}]}"#,
    )
    .unwrap();
    std::fs::write(
        plugin.join(".claude-plugin/plugin.json"),
        r#"{"name":"policy"}"#,
    )
    .unwrap();
    std::fs::write(plugin.join("hooks/hooks.json"), r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"echo plugin-policy >&2; exit 2"}]}]}}"#).unwrap();
    let _home = TestHomeGuard::new(&home);
    let manager = jcode_base::plugin::PluginManager::new(home, project.clone());
    manager.add_marketplace(market.to_str().unwrap()).unwrap();
    let installed = manager
        .install("policy", jcode_base::plugin::PluginScope::Project)
        .unwrap();
    crate::config::invalidate_config_cache();
    let registry = Registry::new(Arc::new(MockProvider)).await;
    let result = registry
        .execute(
            "bash",
            serde_json::json!({"command":"printf should-not-run"}),
            mcp_test_context(&project),
        )
        .await;
    assert!(result.unwrap_err().to_string().contains("plugin-policy"));

    std::fs::write(installed.root.join("hooks/hooks.json"), serde_json::json!({"hooks":{
        "SessionStart":[{"hooks":[{"type":"command","command":"cat > \"$CLAUDE_PLUGIN_ROOT/start.json\""}]}],
        "SessionEnd":[{"hooks":[{"type":"command","command":"cat > \"$CLAUDE_PLUGIN_ROOT/end.json\""}]}],
        "PostToolUse":[{"hooks":[{"type":"command","command":"cat > \"$CLAUDE_PLUGIN_ROOT/post.json\""}]}]
    }}).to_string()).unwrap();
    let mut agent = crate::agent::Agent::new(Arc::new(MockProvider), registry.clone());
    agent.set_working_dir(project.to_str().unwrap());
    agent.fire_session_lifecycle_hook("session_start", "plugin-test");
    let start = read_payload(&installed.root.join("start.json")).await;
    assert_eq!(start["hook_event_name"], "SessionStart");
    assert_eq!(start["session_id"], agent.session_id());
    assert_eq!(start["source"], "plugin-test");

    let command = format!("printf '%s' '{}tail'", "x".repeat(20_000));
    let input = serde_json::json!({"command":command});
    let output = registry
        .execute("bash", input.clone(), mcp_test_context(&project))
        .await
        .unwrap();
    let post = read_payload(&installed.root.join("post.json")).await;
    assert_eq!(post["hook_event_name"], "PostToolUse");
    assert_eq!(post["tool_name"], "Bash");
    assert_eq!(post["tool_input"], input);
    assert_eq!(post["tool_response"], output.output);
    assert!(post["tool_response"].as_str().unwrap().ends_with("tail\n"));
    agent.fire_session_lifecycle_hook("session_end", "plugin-test");
    let end = read_payload(&installed.root.join("end.json")).await;
    assert_eq!(end["hook_event_name"], "SessionEnd");
    assert_eq!(end["session_id"], agent.session_id());
}

async fn read_payload(path: &std::path::Path) -> Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let result = std::fs::read(path)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| Ok(serde_json::from_slice(&bytes)?));
        match result {
            Ok(value) => return value,
            Err(error) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "hook payload {}: {error}",
                    path.display()
                );
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }
    }
}
