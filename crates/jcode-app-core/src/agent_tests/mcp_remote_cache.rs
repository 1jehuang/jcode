use super::*;
use crate::tool::Tool;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{RwLock, Semaphore};

/// A real HTTP peer whose initialize response is controlled by the test.
/// Dropping the fixture also cancels accepted sockets, including gated ones.
struct RemotePeer {
    url: String,
    initialized: Arc<Semaphore>,
    release: Arc<Semaphore>,
    tools_visible: Arc<std::sync::atomic::AtomicBool>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for RemotePeer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl RemotePeer {
    async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let initialized = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let tools_visible = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let started = initialized.clone();
        let gate = release.clone();
        let catalog = tools_visible.clone();
        let task = tokio::spawn(async move {
            let mut sockets = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (stream, _) = accepted.unwrap();
                        let started = started.clone();
                        let gate = gate.clone();
                        let catalog = catalog.clone();
                        sockets.spawn(async move {
                            serve_request(stream, started, gate, catalog).await;
                        });
                    }
                    done = sockets.join_next(), if !sockets.is_empty() => {
                        done.unwrap().unwrap();
                    }
                }
            }
        });
        Self {
            url,
            initialized,
            release,
            tools_visible,
            task,
        }
    }

    async fn wait_for_initialize(&self) {
        tokio::time::timeout(Duration::from_secs(5), self.initialized.acquire())
            .await
            .expect("peer must receive initialize")
            .unwrap()
            .forget();
    }
}

async fn serve_request(
    mut stream: tokio::net::TcpStream,
    started: Arc<Semaphore>,
    gate: Arc<Semaphore>,
    tools_visible: Arc<std::sync::atomic::AtomicBool>,
) {
    let mut bytes = Vec::new();
    let (headers, body_start, length) = loop {
        let mut chunk = [0; 4096];
        let n = stream.read(&mut chunk).await.unwrap();
        if n == 0 {
            return;
        }
        bytes.extend_from_slice(&chunk[..n]);
        assert!(bytes.len() < 128 * 1024);
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let headers = String::from_utf8(bytes[..end].to_vec()).unwrap();
            let length = headers
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            break (headers, end + 4, length);
        }
    };
    while bytes.len() < body_start + length {
        let mut chunk = [0; 4096];
        let n = stream.read(&mut chunk).await.unwrap();
        if n == 0 {
            return;
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
    let request: Value =
        serde_json::from_slice(&bytes[body_start..body_start + length]).unwrap_or(Value::Null);
    let (status, body) = if headers.starts_with("GET ") {
        ("405 Method Not Allowed", String::new())
    } else if headers.starts_with("DELETE ") || request.get("id").is_none() {
        ("204 No Content", String::new())
    } else {
        let result = match request["method"].as_str().unwrap() {
            "initialize" => {
                started.add_permits(1);
                gate.acquire().await.unwrap().forget();
                json!({"protocolVersion": request["params"]["protocolVersion"],
                    "capabilities": {"tools": {}}, "serverInfo": {"name": "cache-peer", "version": "1"}})
            }
            "tools/list" => {
                if tools_visible.load(std::sync::atomic::Ordering::SeqCst) {
                    json!({"tools": [{"name": "echo", "description": "Echo over HTTP",
                        "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}}]})
                } else {
                    json!({"tools": []})
                }
            }
            "tools/call" => {
                json!({"content": [{"type": "text", "text": request["params"]["arguments"]["text"]}]})
            }
            method => panic!("unexpected method {method}"),
        };
        (
            "200 OK",
            json!({"jsonrpc": "2.0", "id": request["id"], "result": result}).to_string(),
        )
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await.unwrap();
}

fn context(agent: &Agent) -> crate::tool::ToolContext {
    crate::tool::ToolContext {
        session_id: agent.session_id().into(),
        message_id: "cache-test".into(),
        tool_call_id: "connect-test".into(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: crate::tool::ToolExecutionMode::Direct,
    }
}

async fn wait_for_tool(registry: &Registry, name: &str) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if registry.tool_names().await.iter().any(|tool| tool == name) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fast remote tools must publish before slow initialize completes");
}

fn cache_tools_signature(tools: &[ToolDefinition]) -> (u64, usize, usize) {
    let ServerEvent::KvCacheRequest {
        tools_hash,
        tool_count,
        tools_json_chars,
        ..
    } = kv_cache_request_event(&[Message::user("cached message")], tools, "system", &[])
    else {
        unreachable!()
    };
    (tools_hash, tool_count, tools_json_chars)
}

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
    assert!(before.iter().any(|tool| tool.name == "mcp_call"));
    assert!(before.iter().any(|tool| tool.name == "mcp_search"));
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
    pool.disconnect_all().await;

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
    pool.disconnect_all().await;
}

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
    pool.disconnect_all().await;
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
            manager.read().await.disconnect_all().await;
        }
    }
}
