use super::*;

#[cfg(all(windows, feature = "test-support"))]
#[tokio::test]
async fn mcp_reload_failed_pool_drain_reconciles_registry_and_empty_retry_drains() {
    let _env_lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _home_guard = TestHomeGuard::new(home.path());
    let record = home.path().join("slow-pool.jsonl");
    let configured: crate::mcp::McpServerConfig = serde_json::from_value(serde_json::json!({
        "command":"C:\\Python314\\python.exe", "args":[concat!(env!("CARGO_MANIFEST_DIR"), "/../jcode-base/tests/natural_mcp_fixture.py"), record.display().to_string(), "slow_shutdown"], "shared":true
    })).unwrap();
    let client = crate::mcp::McpClient::connect_natural_test("slow".into(), &configured)
        .await
        .unwrap();
    let pool = Arc::new(crate::mcp::SharedMcpPool::new(
        crate::mcp::McpConfig::default(),
    ));
    pool.test_adopt_natural_client("slow".into(), client).await;
    let manager = Arc::new(RwLock::new(crate::mcp::McpManager::with_shared_pool(
        Arc::clone(&pool),
        "reload-registry".into(),
    )));
    manager
        .read()
        .await
        .connect("slow", &configured)
        .await
        .unwrap();
    let registry = Registry::new(Arc::new(MockProvider)).await;
    let obsolete = crate::mcp::McpToolDef {
        name: "obsolete".into(),
        description: None,
        input_schema: serde_json::json!({"type":"object"}),
    };
    registry
        .reconcile_mcp_tools(crate::mcp::create_mcp_tools_from_cached(
            "slow",
            &[obsolete],
            Arc::clone(&manager),
        ))
        .await;
    assert!(
        registry
            .tool_names()
            .await
            .iter()
            .any(|name| name.starts_with("mcp__"))
    );
    let tool = crate::tool::mcp::McpManagementTool::new(Arc::clone(&manager))
        .with_registry(registry.clone());
    let error = tool
        .execute(
            serde_json::json!({"action":"reload"}),
            mcp_test_context(home.path()),
        )
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("shutdown incomplete"));
    assert!(manager.read().await.cleanup_pending());
    assert!(pool.cleanup_pending());
    let names = registry.tool_names().await;
    assert!(!names.iter().any(|name| name.starts_with("mcp__")));
    assert!(names.iter().any(|name| name == "bash"));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if std::fs::read_to_string(&record)
                .unwrap_or_default()
                .contains("natural_exit")
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    pool.disconnect_all().await.unwrap();
    let result = tool
        .execute(
            serde_json::json!({"action":"reload"}),
            mcp_test_context(home.path()),
        )
        .await
        .unwrap();
    assert!(result.output.contains("Connected: 0/0"));
    assert!(!manager.read().await.cleanup_pending());
    assert!(!pool.cleanup_pending());
    assert!(
        !registry
            .tool_names()
            .await
            .iter()
            .any(|name| name.starts_with("mcp__"))
    );
}
