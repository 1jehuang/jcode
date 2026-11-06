#[test]
fn remote_deferred_tool_changes_do_not_change_cache_telemetry() {
    let eager = ToolDefinition {
        name: "mcp_search".into(),
        description: "search".into(),
        input_schema: json!({"type": "object"}),
        defer_loading: false,
    };
    let placeholder = ToolDefinition {
        name: "mcp_call".into(),
        description: "call".into(),
        input_schema: json!({"type": "object"}),
        defer_loading: true,
    };
    let before = vec![eager.clone(), placeholder.clone()];
    let mut after = before.clone();
    after.push(ToolDefinition {
        name: "mcp__remote__echo".into(),
        description: "late remote tool".into(),
        input_schema: json!({"type": "object", "properties": {"text": {"type": "string"}}}),
        defer_loading: true,
    });
    assert_eq!(
        cache_tools_signature(&before),
        cache_tools_signature(&after),
        "deferred catalog additions must not masquerade as cache-prefix changes"
    );
    after[2].description = "refreshed remote schema".into();
    assert_eq!(
        cache_tools_signature(&before),
        cache_tools_signature(&after)
    );
    assert_eq!(
        cache_tools_signature(&before),
        cache_tools_signature(&[eager])
    );
    after[0].description = "actually changed eager schema".into();
    assert_ne!(
        cache_tools_signature(&before),
        cache_tools_signature(&after),
        "a real eager-prefix change must remain visible"
    );
}

#[tokio::test]
async fn remote_startup_does_not_wait_and_publishes_fast_tools_before_slow_peer() {
    for shared in [false, true] {
        remote_startup_case(shared).await;
    }
}

async fn remote_startup_case(shared: bool) {
    let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let slow = RemotePeer::new().await;
    let fast = RemotePeer::new().await;
    fast.release.add_permits(1);
    let config = json!({"servers": {
        "slow": {"url": slow.url, "shared": shared},
        "fast": {"url": fast.url, "shared": shared}
    }});
    std::fs::write(sandbox.root().join("mcp.json"), config.to_string()).unwrap();
    let registry = Registry::empty();
    let pool = Arc::new(crate::mcp::SharedMcpPool::new(
        serde_json::from_value(config).unwrap(),
    ));
    let (tx, mut rx) = tokio_mpsc::unbounded_channel();
    let cold_started = std::time::Instant::now();
    tokio::time::timeout(
        Duration::from_secs(1),
        registry.register_mcp_tools_for_dir(
            Some(tx),
            Some(pool.clone()),
            Some("remote-startup-cache".into()),
            Some(sandbox.root().into()),
        ),
    )
    .await
    .expect("startup cannot await network handshakes");
    let cold_registration = cold_started.elapsed();
    let provider: Arc<dyn Provider> = Arc::new(NativeAutoCompactionProvider);
    let mut agent = Agent::new(provider, registry.clone());
    agent.mcp_tools_mode = crate::config::McpToolsMode::Auto;
    let before = agent.tool_definitions().await;
    assert!(before.iter().any(|tool| tool.name == "mcp"));
    assert!(
        !before
            .iter()
            .any(|tool| tool.name == "mcp_call" || tool.name == "mcp_search")
    );
    slow.wait_for_initialize().await;
    fast.wait_for_initialize().await;
    wait_for_tool(&registry, "mcp__fast__echo").await;
    for (name, input) in [
        ("mcp", json!({"action": "list"})),
        ("mcp_search", json!({"server": "fast"})),
        (
            "mcp__fast__echo",
            json!({"text": "usable before slow peer"}),
        ),
    ] {
        let output = tokio::time::timeout(
            Duration::from_secs(1),
            registry.execute(name, input, context(&agent)),
        )
        .await
        .expect("slow initialization must not block management or fast remote calls")
        .unwrap();
        assert!(!output.output.is_empty());
        if name == "mcp__fast__echo" {
            assert!(output.output.contains("usable before slow peer"));
        }
    }
    assert!(
        !registry
            .tool_names()
            .await
            .iter()
            .any(|name| name == "mcp__slow__echo")
    );
    assert_eq!(
        serde_json::to_value(&before).unwrap(),
        serde_json::to_value(agent.tool_definitions().await).unwrap()
    );
    slow.release.add_permits(1);
    wait_for_tool(&registry, "mcp__slow__echo").await;
    // Drain the completion event before dropping the sandbox: the background
    // task writes its schema cache before sending the final status.
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = rx.recv().await {
            if let ServerEvent::McpStatus { servers } = event
                && servers.iter().any(|s| s == "slow:1")
                && servers.iter().any(|s| s == "fast:1")
            {
                return;
            }
        }
        panic!("startup task closed without publishing complete MCP status");
    })
    .await
    .unwrap();
    pool.disconnect_all().await.unwrap();

    // With the schema cache populated, even explicit eager mode advertises both
    // servers before their new handshakes are allowed to finish.
    let warm_registry = Registry::empty();
    let (warm_tx, mut warm_rx) = tokio_mpsc::unbounded_channel();
    let warm_started = std::time::Instant::now();
    tokio::time::timeout(
        Duration::from_secs(1),
        warm_registry.register_mcp_tools_for_dir(
            Some(warm_tx),
            Some(pool.clone()),
            Some("remote-warm-cache".into()),
            Some(sandbox.root().into()),
        ),
    )
    .await
    .expect("warm startup cannot await network handshakes");
    let warm_registration = warm_started.elapsed();
    for name in ["mcp__fast__echo", "mcp__slow__echo"] {
        assert!(
            warm_registry
                .tool_names()
                .await
                .iter()
                .any(|tool| tool == name)
        );
    }
    let mut warm_agent = Agent::new(
        Arc::new(SwitchableDeferredProvider(Arc::new(
            std::sync::atomic::AtomicBool::new(false),
        ))),
        warm_registry,
    );
    warm_agent.mcp_tools_mode = crate::config::McpToolsMode::Eager;
    let warm_before = serde_json::to_value(warm_agent.tool_definitions().await).unwrap();
    slow.wait_for_initialize().await;
    fast.wait_for_initialize().await;
    slow.release.add_permits(1);
    fast.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = warm_rx.recv().await {
            if let ServerEvent::McpStatus { servers } = event
                && servers.iter().any(|s| s == "slow:1")
                && servers.iter().any(|s| s == "fast:1")
            {
                return;
            }
        }
        panic!("warm startup closed without publishing complete MCP status");
    })
    .await
    .unwrap();
    assert_eq!(
        warm_before,
        serde_json::to_value(warm_agent.tool_definitions().await).unwrap(),
        "warm cached schemas keep even the eager tool snapshot unchanged"
    );
    eprintln!(
        "MCP_STARTUP shared={shared} cold_registration_us={} warm_registration_us={}",
        cold_registration.as_micros(),
        warm_registration.as_micros()
    );
    pool.disconnect_all().await.unwrap();
}
