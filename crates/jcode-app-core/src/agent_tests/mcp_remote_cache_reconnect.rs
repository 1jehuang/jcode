#[tokio::test]
async fn remote_empty_catalog_replaces_obsolete_cached_schemas() {
    let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let peer = RemotePeer::new().await;
    peer.tools_visible
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let config: crate::mcp::McpConfig = serde_json::from_value(json!({"servers": {
        "remote": {"url": peer.url, "shared": true}
    }}))
    .unwrap();
    std::fs::write(
        sandbox.root().join("mcp.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let mut cache = crate::mcp::McpSchemaCache::default();
    cache.update(
        "remote",
        &config.servers["remote"],
        vec![crate::mcp::McpToolDef {
            name: "echo".into(),
            description: Some("Obsolete schema".into()),
            input_schema: json!({"type": "object"}),
        }],
    );
    cache.save();
    let registry = Registry::empty();
    let pool = Arc::new(crate::mcp::SharedMcpPool::new(config.clone()));
    let (tx, mut rx) = tokio_mpsc::unbounded_channel();
    registry
        .register_mcp_tools_for_dir(
            Some(tx),
            Some(pool.clone()),
            Some("empty-catalog".into()),
            Some(sandbox.root().into()),
        )
        .await;
    assert!(
        registry.has_mcp_tools_for_server("remote"),
        "cached proxy is advertised first"
    );
    peer.wait_for_initialize().await;
    peer.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = rx.recv().await {
            if let ServerEvent::McpStatus { servers } = event
                && servers.is_empty()
            {
                return;
            }
        }
        panic!("empty catalog did not complete registration");
    })
    .await
    .unwrap();
    assert!(!registry.has_mcp_tools_for_server("remote"));
    let updated = crate::mcp::McpSchemaCache::load();
    assert!(
        updated
            .tools_for("remote", &config.servers["remote"])
            .unwrap()
            .is_empty(),
        "next startup must not advertise the obsolete echo schema"
    );
    pool.disconnect_all().await.unwrap();
}

#[tokio::test]
async fn remote_connect_disconnect_reconnect_preserves_prompt_and_cache_baselines() {
    let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    for native in [false, true] {
        for mode in [
            crate::config::McpToolsMode::Auto,
            crate::config::McpToolsMode::Deferred,
        ] {
            let peer = RemotePeer::new().await;
            let registry = Registry::empty();
            let manager = Arc::new(RwLock::new(crate::mcp::McpManager::with_config(
                Default::default(),
            )));
            let management = Arc::new(
                crate::tool::mcp::McpManagementTool::new(manager.clone())
                    .with_registry(registry.clone()),
            );
            registry.register("mcp".into(), management.clone()).await;
            registry
                .register(
                    "mcp_search".into(),
                    Arc::new(
                        crate::tool::mcp::McpSearchTool::new(manager.clone())
                            .with_registry(registry.clone()),
                    ),
                )
                .await;
            registry
                .register(
                    "mcp_call".into(),
                    Arc::new(
                        crate::tool::mcp::McpCallTool::new(manager.clone())
                            .with_registry(registry.clone()),
                    ),
                )
                .await;
            let provider: Arc<dyn Provider> = Arc::new(SwitchableDeferredProvider(Arc::new(
                std::sync::atomic::AtomicBool::new(native),
            )));
            let mut agent = Agent::new(provider, registry);
            agent.mcp_tools_mode = mode;
            agent.add_message(
                Role::User,
                vec![ContentBlock::Text {
                    text: "existing cached conversation".into(),
                    cache_control: None,
                }],
            );
            let original_messages = serde_json::to_value(&agent.session.messages).unwrap();
            let before = agent.tool_definitions().await;
            let eager_before = serde_json::to_value(ToolDefinition::eager(&before)).unwrap();
            let static_before = agent.build_system_prompt_split(None).static_part;
            assert!(
                agent
                    .cache_tracker
                    .record_request(&agent.session.messages_for_provider_uncached())
                    .is_none()
            );
            let baseline = kv_cache_request_event(
                &agent.session.messages_for_provider_uncached(),
                &before,
                &static_before,
                &[],
            );
            agent.begin_kv_cache_monitor_request(&baseline, "cache-test-model");
            assert!(
                agent
                    .kv_cache_monitor
                    .finish_request(4096, Some(0))
                    .is_none()
            );
            let monitor_before = format!("{:?}", agent.kv_cache_monitor);
            for connected in [true, false, true] {
                let messages_before_action = serde_json::to_value(&agent.session.messages).unwrap();
                let input = if connected {
                    peer.release.add_permits(1);
                    json!({"action": "connect", "server": "remote", "url": peer.url, "headers": {"Authorization": "Bearer test-only"}})
                } else {
                    json!({"action": "disconnect", "server": "remote"})
                };
                let output = tokio::time::timeout(
                    Duration::from_secs(5),
                    management.execute(input, context(&agent)),
                )
                .await
                .unwrap()
                .unwrap();
                assert!(
                    output.output.contains(if connected {
                        "Connected to"
                    } else {
                        "Disconnected"
                    }),
                    "{}",
                    output.output
                );
                agent.unlock_tools_if_needed("mcp");
                let tools = agent.tool_definitions().await;
                assert_eq!(
                    eager_before,
                    serde_json::to_value(ToolDefinition::eager(&tools)).unwrap()
                );
                assert_eq!(
                    cache_tools_signature(&before),
                    cache_tools_signature(&tools)
                );
                assert_eq!(
                    static_before,
                    agent.build_system_prompt_split(None).static_part
                );
                assert_eq!(
                    agent.cache_tracker.turn_count(),
                    1,
                    "MCP lifecycle must not reset cache tracker"
                );
                assert_eq!(
                    monitor_before,
                    format!("{:?}", agent.kv_cache_monitor),
                    "MCP lifecycle must not reset KV monitor"
                );
                assert_eq!(
                    messages_before_action,
                    serde_json::to_value(&agent.session.messages).unwrap()
                );
                assert_eq!(
                    tools.iter().any(|tool| tool.name == "mcp__remote__echo"),
                    native && connected
                );
                if connected {
                    assert!(output.output.contains("input_schema"));
                    assert!(
                        crate::agent::tools::tool_output_references(&output)
                            .contains(&"mcp__remote__echo".into())
                    );
                    let called = manager
                        .read()
                        .await
                        .call_tool("remote", "echo", json!({"text": "still callable"}))
                        .await
                        .unwrap();
                    assert!(
                        matches!(&called.content[0], crate::mcp::ContentBlock::Text { text } if text == "still callable")
                    );
                    agent.announce_late_mcp_tools().await;
                    let messages = serde_json::to_value(&agent.session.messages).unwrap();
                    assert_eq!(
                        messages.as_array().unwrap()[..original_messages.as_array().unwrap().len()],
                        original_messages.as_array().unwrap()[..]
                    );
                    assert_eq!(
                        agent.session.messages.len(),
                        original_messages.as_array().unwrap().len() + usize::from(!native)
                    );
                    // Announcement is append-only and never repeats on reconnect.
                    agent.announce_late_mcp_tools().await;
                }
            }
            assert!(
                agent
                    .cache_tracker
                    .record_request(&agent.session.messages_for_provider_uncached())
                    .is_none()
            );
            manager.read().await.disconnect_all().await.unwrap();
        }
    }
}
