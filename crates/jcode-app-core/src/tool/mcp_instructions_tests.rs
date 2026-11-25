use super::*;

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn connected_server_guidance_arrives_once_with_selected_schemas() {
    if !std::process::Command::new("python3")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
    {
        eprintln!("SKIP: python3 unavailable for MCP instructions fixture");
        return;
    }

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
    for mode in ["normal", "blank", "huge", "boundary"] {
        config.servers.insert(mode.into(), serde_json::from_value(json!({
            "command":"python3","args":["-u","-c",include_str!("mcp_instructions_fixture.py"),mode],
            "shared":false,"timeout_secs":5
        })).unwrap());
    }
    let manager = Arc::new(RwLock::new(McpManager::with_config(config.clone())));
    for (name, server) in &config.servers {
        manager.read().await.connect(name, server).await.unwrap();
    }
    let registry = crate::tool::Registry::empty();
    registry
        .register(
            "mcp".into(),
            Arc::new(McpManagementTool::new(manager.clone()).with_registry(registry.clone())),
        )
        .await;
    let context = ToolContext {
        session_id: "mcp-instructions-fixture".into(),
        message_id: "message".into(),
        tool_call_id: "guidance".into(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: crate::tool::ToolExecutionMode::Direct,
    };
    for mode in ["normal", "blank", "huge", "boundary"] {
        let query = format!("select:mcp__{mode}__lookup,mcp__{mode}__read");
        let output = registry
            .execute(
                "mcp",
                json!({"action":"search","query":query}),
                context.clone(),
            )
            .await
            .unwrap();
        let tools: Vec<Value> = serde_json::from_str(&output.output).unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(
            output.metadata.unwrap()["tool_references"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let guidance: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool["server_instructions"].as_str())
            .collect();
        if mode == "blank" {
            assert!(guidance.is_empty());
        } else {
            assert_eq!(guidance.len(), 1);
            if mode == "normal" {
                assert_eq!(
                    guidance[0],
                    "Use the documented workspace before requesting data."
                );
            } else {
                assert!(guidance[0].starts_with('雪'));
                if mode == "huge" {
                    assert!(guidance[0].ends_with('…'));
                }
            }
        }
        assert!(jcode_core::util::estimate_tokens(&output.output) <= SEARCH_TOKEN_BUDGET);
    }
    let retained = manager
        .read()
        .await
        .server_instructions("huge")
        .await
        .unwrap();
    assert_eq!(retained.chars().count(), 16_385);
    assert!(retained.ends_with('…'));
    let boundary = manager
        .read()
        .await
        .server_instructions("boundary")
        .await
        .unwrap();
    assert_eq!(boundary.chars().count(), 16_384);
    assert!(!boundary.ends_with('…'));
    assert!(
        manager
            .read()
            .await
            .server_instructions("blank")
            .await
            .is_none()
    );
    let _policy = crate::tool::register_session_tool_policy(
        &context.session_id,
        Some(std::collections::HashSet::from([
            "mcp__normal__lookup".into()
        ])),
        std::collections::HashSet::new(),
    );
    let selected = registry
        .execute(
            "mcp",
            json!({"action":"search","query":"select:mcp__normal__lookup,mcp__huge__lookup"}),
            context.clone(),
        )
        .await
        .unwrap();
    assert!(!selected.output.contains('雪'));
    assert!(selected.output.contains("documented workspace"));
    let output = registry
        .execute(
            "mcp",
            json!({"action":"search","query":"select:unknown"}),
            context,
        )
        .await
        .unwrap();
    assert_eq!(output.output, "[]");
}

#[test]
fn optional_guidance_preserves_schema_capacity_in_tight_budgets() {
    for count in [1usize, 20] {
        let mut matches: Vec<_> = (0..count)
            .map(|index| McpSearchResult {
                name: format!("mcp__s__tool_{index}"),
                server: "s".into(),
                tool: format!("tool_{index}"),
                description: "d".repeat(200),
                input_schema: json!({"type":"object","properties":{"q":{"type":"string"}}}),
                server_instructions: None,
            })
            .collect();
        let budget = if count == 1 {
            jcode_core::util::estimate_tokens(&serde_json::to_string(&matches).unwrap())
        } else {
            500
        };
        let (control, expected) = fit_search_results(&matches, budget).unwrap();
        assert!(expected > 0);
        assert!(jcode_core::util::estimate_tokens(&control) <= budget);
        for result in &mut matches {
            result.server_instructions = Some("雪".repeat(40000));
        }
        let (output, detailed) = fit_search_results(&matches, budget).unwrap();
        assert!(detailed >= expected);
        assert!(jcode_core::util::estimate_tokens(&output) <= budget);
    }
}
