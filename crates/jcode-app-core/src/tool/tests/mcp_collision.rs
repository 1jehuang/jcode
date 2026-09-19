use super::*;

fn collision_catalog() -> Vec<(String, crate::mcp::McpToolDef)> {
    [("server-a", "query-docs"), ("server_a", "query_docs")]
        .into_iter()
        .map(|(server, name)| {
            (
                server.to_string(),
                crate::mcp::McpToolDef {
                    name: name.to_string(),
                    description: Some(server.to_string()),
                    input_schema: serde_json::json!({"type": "object"}),
                },
            )
        })
        .collect()
}

#[tokio::test]
async fn mcp_collision_refresh_removes_cached_alias_and_schema() {
    let registry = Registry::new(Arc::new(MockProvider)).await;
    let manager = Arc::new(RwLock::new(crate::mcp::McpManager::with_config(
        crate::mcp::McpConfig::default(),
    )));
    let mut catalog = collision_catalog();
    let proxies = crate::mcp::create_mcp_tools_from_cached_many(&catalog, Arc::clone(&manager));
    assert_eq!(
        proxies[0].1.mcp_identity(),
        Some(("server-a", "query-docs")),
        "MCP proxy lost identity: ensure base/tool-core artifacts came from this worktree"
    );
    let legacy = crate::mcp::dispatch_name(&catalog[0].0, &catalog[0].1.name);
    registry
        .reconcile_mcp_tools(crate::mcp::create_mcp_tools_from_cached_many(
            &catalog[..1],
            Arc::clone(&manager),
        ))
        .await;
    assert!(registry.tool_names().await.contains(&legacy));
    catalog[0].1.input_schema =
        serde_json::json!({"type":"object", "properties":{"fresh":{"type":"string"}}});
    let aliases = crate::mcp::dispatch_names(&catalog);
    registry
        .reconcile_mcp_tools(crate::mcp::create_mcp_tools_from_cached_many(
            &catalog, manager,
        ))
        .await;
    let defs = registry.definitions(None).await;
    assert!(!defs.iter().any(|def| def.name == legacy));
    for alias in &aliases {
        assert!(defs.iter().any(|def| &def.name == alias));
    }
    let fresh = defs.iter().find(|def| def.name == aliases[0]).unwrap();
    assert!(fresh.input_schema["properties"]["fresh"].is_object());
    assert!(
        registry
            .tool_names()
            .await
            .iter()
            .any(|name| name == "bash")
    );
    let removed = registry.unregister_mcp_server("server-a").await;
    assert_eq!(removed, vec![aliases[0].clone()]);
    assert!(registry.tool_names().await.contains(&aliases[1]));
    assert!(!registry.tool_names().await.contains(&aliases[0]));
}

#[tokio::test]
async fn mcp_collision_legacy_deny_blocks_eager_and_deferred_dispatch() {
    let registry = Registry::new(Arc::new(MockProvider)).await;
    let manager = Arc::new(RwLock::new(crate::mcp::McpManager::with_config(
        crate::mcp::McpConfig::default(),
    )));
    let catalog = collision_catalog();
    let aliases = crate::mcp::dispatch_names(&catalog);
    registry
        .reconcile_mcp_tools(crate::mcp::create_mcp_tools_from_cached_many(
            &catalog,
            Arc::clone(&manager),
        ))
        .await;
    let legacy = crate::mcp::dispatch_name(&catalog[0].0, &catalog[0].1.name);
    let mut ctx = mcp_test_context(std::path::Path::new("."));
    ctx.session_id = "mcp-collision-legacy-deny".to_string();
    set_session_tool_policy(&ctx.session_id, None, HashSet::from([legacy.clone()]));
    for (index, alias) in aliases.iter().enumerate() {
        let error = registry
            .execute(alias, serde_json::json!({}), ctx.clone())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("disabled"), "{error}");
        let error = mcp::McpCallTool::new(Arc::clone(&manager)).execute(
            serde_json::json!({"server":catalog[index].0,"tool":catalog[index].1.name,"arguments":{}}), ctx.clone(),
        ).await.unwrap_err();
        assert!(error.to_string().contains("not allowed"), "{error}");
    }
    clear_session_tool_policy(&ctx.session_id);
}

#[test]
fn mcp_collision_alias_policy_preserves_deny_precedence() {
    let catalog = collision_catalog();
    let aliases = crate::mcp::dispatch_names(&catalog);
    let legacy = crate::mcp::dispatch_name(&catalog[0].0, &catalog[0].1.name);
    let session = "mcp-collision-alias-policy";
    for denied in [&legacy, &aliases[0]] {
        set_session_tool_policy(
            session,
            Some(HashSet::from(["mcp_call".into()])),
            HashSet::from([denied.clone()]),
        );
        assert!(!session_mcp_alias_is_allowed(
            session,
            &aliases[0],
            &legacy,
            "mcp_call"
        ));
    }
    set_session_tool_policy(
        session,
        Some(HashSet::from([aliases[0].clone()])),
        HashSet::new(),
    );
    assert!(session_mcp_alias_is_allowed(
        session,
        &aliases[0],
        &legacy,
        "mcp_call"
    ));
    assert!(!session_mcp_alias_is_allowed(
        session,
        &aliases[1],
        &legacy,
        "mcp_call"
    ));
    clear_session_tool_policy(session);
}

// Stdio protocol fixture shared in spirit with jcode-base/tests/mcp_stdio_collision_integration.rs.
const SERVER: &str = r#"
import json, os, sys
server, names = sys.argv[1], json.loads(sys.argv[2])
for line in sys.stdin:
    req = json.loads(line)
    method = req.get('method')
    if method == 'shutdown':
        break
    if 'id' not in req:
        continue
    reply = {'jsonrpc': '2.0', 'id': req['id']}
    if method == 'initialize':
        result = {'protocolVersion': '2024-11-05', 'capabilities': {'tools': {}},
                  'serverInfo': {'name': server, 'version': '1'}}
    elif method == 'tools/list':
        result = {'tools': [{'name': name, 'description': server + ':' + name,
                            'inputSchema': {'type': 'object', 'properties': {
                                'token': {'type': 'string'}}}} for name in names]}
    elif method == 'tools/call' and req['params']['name'] in names:
        value = {'server': server, 'tool': req['params']['name'],
                 'arguments': req['params']['arguments'], 'pid': os.getpid()}
        result = {'content': [{'type': 'text', 'text': json.dumps(value)}], 'isError': False}
    else:
        reply['error'] = {'code': -32602, 'message': 'unknown method or original tool name'}
        print(json.dumps(reply), flush=True)
        continue
    reply['result'] = result
    print(json.dumps(reply), flush=True)
"#;

#[test]
fn mcp_collision_manual_lifecycle_real_stdio() {
    const MARKER: &str = "JCODE_MCP_MANAGEMENT_TEST_CHILD";
    if std::env::var_os(MARKER).is_none() {
        if !std::process::Command::new("python3")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
        {
            eprintln!("SKIP: python3 unavailable for real MCP stdio test");
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child.env_clear();
        for key in ["PATH", "LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH", "SYSTEMROOT"] {
            if let Some(value) = std::env::var_os(key) {
                child.env(key, value);
            }
        }
        for key in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "XDG_CONFIG_HOME",
        ] {
            child.env(key, home.path());
        }
        let status = child
            .env(MARKER, "1")
            .env("JCODE_HOME", home.path().join("jcode"))
            .env("JCODE_RUNTIME_DIR", home.path().join("runtime"))
            .current_dir(home.path())
            .args([
                "--exact",
                "tool::tests::mcp_collision::mcp_collision_manual_lifecycle_real_stdio",
                "--nocapture",
            ])
            .status()
            .unwrap();
        assert!(
            status.success(),
            "isolated management test failed: {status}"
        );
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            let registry = Registry::new(Arc::new(MockProvider)).await;
            let manager = Arc::new(RwLock::new(crate::mcp::McpManager::with_config(
                crate::mcp::McpConfig::default(),
            )));
            let management = mcp::McpManagementTool::new(Arc::clone(&manager)).with_registry(registry.clone());
            let ctx = mcp_test_context(std::path::Path::new("."));
            let catalog = collision_catalog();
            let aliases = crate::mcp::dispatch_names(&catalog);
            let legacy = crate::mcp::dispatch_name(&catalog[0].0, &catalog[0].1.name);
            for (index, (server, tool)) in catalog.iter().enumerate() {
                let output = management.execute(serde_json::json!({
                    "action":"connect", "server":server, "command":"python3",
                    "args":["-I", "-S", "-u", "-c", SERVER, server, serde_json::to_string(&vec![&tool.name]).unwrap()]
                }), ctx.clone()).await.unwrap();
                assert!(output.output.contains("Connected to MCP server"), "{}", output.output);
                if index == 1 { assert!(output.output.contains(&aliases[1]), "{}", output.output); }
            }
            assert!(!registry.tool_names().await.contains(&legacy));
            let listed = management.execute(serde_json::json!({"action":"list"}), ctx.clone()).await.unwrap();
            for (index, alias) in aliases.iter().enumerate() {
                assert!(listed.output.contains(alias));
                let output = registry.execute(alias, serde_json::json!({"token":"marker"}), ctx.clone()).await.unwrap();
                let value: Value = serde_json::from_str(&output.output).unwrap();
                assert_eq!(value["server"], catalog[index].0);
                assert_eq!(value["tool"], catalog[index].1.name);
            }
            set_session_tool_policy(&ctx.session_id, None, HashSet::from([aliases[0].clone()]));
            let deferred = mcp::McpCallTool::new(Arc::clone(&manager)).with_registry(registry.clone());
            let denied = deferred.execute(serde_json::json!({"server":"server-a","tool":"query-docs","arguments":{}}), ctx.clone()).await.unwrap_err();
            assert!(denied.to_string().contains("not allowed"));
            let peer = deferred.execute(serde_json::json!({"server":"server_a","tool":"query_docs","arguments":{}}), ctx.clone()).await.unwrap();
            assert_eq!(serde_json::from_str::<Value>(&peer.output).unwrap()["server"], "server_a");
            set_session_tool_policy(&ctx.session_id, None, HashSet::from([aliases[1].clone()]));
            management.execute(serde_json::json!({"action":"disconnect","server":"server-a"}), ctx.clone()).await.unwrap();
            let names = registry.tool_names().await;
            assert!(names.contains(&legacy));
            assert!(!names.contains(&aliases[0]));
            assert!(!names.contains(&aliases[1]));
            let denied = registry.execute(&legacy, serde_json::json!({}), ctx.clone()).await.unwrap_err();
            assert!(denied.to_string().contains("disabled"));
            let denied = deferred.execute(serde_json::json!({"server":"server_a","tool":"query_docs","arguments":{}}), ctx.clone()).await.unwrap_err();
            assert!(denied.to_string().contains("not allowed"));
            clear_session_tool_policy(&ctx.session_id);
            let peer = registry.execute(&legacy, serde_json::json!({}), ctx.clone()).await.unwrap();
            assert_eq!(serde_json::from_str::<Value>(&peer.output).unwrap()["server"], "server_a");
            management.execute(serde_json::json!({"action":"disconnect","server":"server_a"}), ctx).await.unwrap();
            assert!(!registry.tool_names().await.iter().any(|name| name.starts_with("mcp__")));
        }).await.expect("management lifecycle exceeded 30 seconds");
    });
}

#[tokio::test]
async fn mcp_collision_refresh_preserves_offline_cache_and_legacy_allowlists() {
    let registry = Registry::empty();
    let manager = Arc::new(RwLock::new(crate::mcp::McpManager::with_config(
        crate::mcp::McpConfig::default(),
    )));
    let catalog = collision_catalog();
    registry
        .reconcile_mcp_tools(crate::mcp::create_mcp_tools_from_cached_many(
            &catalog,
            Arc::clone(&manager),
        ))
        .await;
    let names = crate::mcp::dispatch_names(&catalog);
    let legacy = crate::mcp::dispatch_name(&catalog[0].0, &catalog[0].1.name);
    let allowed = HashSet::from([legacy.clone()]);
    for name in &names {
        assert!(registry.tool_is_allowed(&allowed, name));
    }
    assert_eq!(registry.definitions(Some(&allowed)).await.len(), 2);
    registry
        .refresh_mcp_tools(
            crate::mcp::create_mcp_tools_from_cached_many(&catalog[..1], Arc::clone(&manager)),
            &["server-a".into()],
        )
        .await;
    assert_eq!(registry.tool_names().await.len(), 2);
    for name in &names {
        assert!(registry.tool_names().await.contains(name));
    }
    // A connected server that now advertises zero tools must lose stale schemas.
    registry
        .refresh_mcp_tools(Vec::new(), &["server-a".into()])
        .await;
    assert_eq!(registry.tool_names().await, vec![legacy.clone()]);
    assert!(registry.tool_is_disabled(&HashSet::from([names[1].clone()]), &legacy));
    assert!(registry.tool_is_allowed(&HashSet::from([names[1].clone()]), &legacy));
    registry.unregister_mcp_server("server_a").await;
    registry.refresh_mcp_tools(Vec::new(), &[]).await;
    assert!(registry.tool_names().await.is_empty());
}
