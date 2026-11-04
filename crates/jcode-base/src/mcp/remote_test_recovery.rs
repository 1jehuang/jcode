#[tokio::test]
async fn owned_manager_reconnects_only_on_next_call_after_session_404() {
    use crate::mcp::manager::McpManager;
    use crate::mcp::protocol::McpConfig;
    let fixture = Fixture::start(Plan::default()).await;
    let mut server = fixture.config("http");
    server.shared = false;
    let config = McpConfig {
        servers: HashMap::from([("reconnect".to_string(), server)]),
    };
    let manager = McpManager::with_config(config);
    let error = manager
        .call_tool("reconnect", "expire", json!({"operation":"first"}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("session expired"));
    assert!(manager.connected_servers().await.is_empty());
    assert!(manager.all_tools().await.is_empty());
    let (first, second) = tokio::join!(
        manager.call_tool("reconnect", "echo", json!({"operation":"second"})),
        manager.call_tool("reconnect", "echo", json!({"operation":"third"})),
    );
    assert_eq!(text(first.unwrap()), "{\"operation\":\"second\"}");
    assert_eq!(text(second.unwrap()), "{\"operation\":\"third\"}");
    assert_eq!(manager.connected_servers().await, vec!["reconnect"]);
    assert_eq!(
        fixture
            .requests()
            .iter()
            .filter(|r| r.body["method"] == "initialize")
            .count(),
        2
    );
    assert_eq!(
        fixture
            .requests()
            .iter()
            .filter(|r| r.body.pointer("/params/name") == Some(&json!("expire")))
            .count(),
        1
    );
    for request in fixture
        .requests()
        .iter()
        .filter(|r| r.body["method"] == "initialize")
    {
        assert!(!request.headers.contains_key(SESSION_HEADER));
    }
    manager.disconnect_all().await.unwrap();
}

#[tokio::test]
async fn shared_manager_and_pool_replace_expired_handles_without_replaying_calls() {
    use crate::mcp::manager::McpManager;
    use crate::mcp::pool::SharedMcpPool;
    use crate::mcp::protocol::McpConfig;
    let _env_guard = crate::storage::lock_test_env();
    let fixture = Fixture::start(Plan::default()).await;
    let config = McpConfig {
        servers: HashMap::from([("reconnect".to_string(), fixture.config("http"))]),
    };
    let project = tempfile::tempdir().unwrap();
    std::fs::write(
        project.path().join(".mcp.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let pool = Arc::new(SharedMcpPool::new(config));
    let manager = McpManager::with_shared_pool_for_dir(
        Arc::clone(&pool),
        "one".into(),
        Some(project.path().to_path_buf()),
    );
    let other = McpManager::with_shared_pool_for_dir(
        Arc::clone(&pool),
        "two".into(),
        Some(project.path().to_path_buf()),
    );
    manager
        .call_tool("reconnect", "echo", json!({}))
        .await
        .unwrap();
    other
        .call_tool("reconnect", "echo", json!({}))
        .await
        .unwrap();
    let expired = pool.get_handle("reconnect").await.unwrap();
    let error = manager
        .call_tool("reconnect", "expire", json!({"operation":"first"}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("session expired"));
    assert!(expired.is_closed());
    assert!(manager.connected_servers().await.is_empty());
    assert!(other.connected_servers().await.is_empty());
    assert!(pool.connected_servers().await.is_empty());
    assert!(pool.get_handle("reconnect").await.is_none());
    assert!(!pool.has_connections().await);
    let (first, second) = tokio::join!(
        manager.call_tool("reconnect", "echo", json!({"operation":"second"})),
        other.call_tool("reconnect", "echo", json!({"operation":"third"})),
    );
    assert_eq!(text(first.unwrap()), "{\"operation\":\"second\"}");
    assert_eq!(text(second.unwrap()), "{\"operation\":\"third\"}");
    assert!(pool.has_connections().await);
    assert_eq!(
        fixture
            .requests()
            .iter()
            .filter(|r| r.body["method"] == "initialize")
            .count(),
        2
    );
    assert_eq!(
        fixture
            .requests()
            .iter()
            .filter(|r| r.body.pointer("/params/name") == Some(&json!("expire")))
            .count(),
        1
    );
    manager.disconnect_all().await.unwrap();
    other.disconnect_all().await.unwrap();
    pool.disconnect_all().await.unwrap();
}

#[test]
fn endpoint_origin_validation_accepts_relative_paths_only_on_configured_origin() {
    let base = reqwest::Url::parse("https://example.com/sse").unwrap();
    for endpoint in [
        "/messages?session=1",
        "messages",
        "https://example.com:443/messages",
    ] {
        assert!(validate_endpoint(&base, endpoint).is_ok(), "{endpoint}");
    }
    for endpoint in [
        "https://evil.com/message",
        "//evil.com/message",
        "http://example.com/message",
        "https://example.com:444/message",
        "https://user:pass@example.com/message",
        "/message#fragment",
        "file:///message",
    ] {
        assert!(validate_endpoint(&base, endpoint).is_err(), "{endpoint}");
    }
}

#[test]
fn sse_parser_arbitrary_byte_splits_bom_line_endings_and_empty_fields() {
    let data = "\u{feff}: comment\r\nevent: message\rdata: héllo 🦀\r\ndata: second\n\rdata\n\n";
    let mut parser = SseParser::default();
    let mut events = Vec::new();
    for byte in data.as_bytes() {
        events.extend(parser.push(&[*byte]).unwrap());
    }
    assert_eq!(
        events,
        vec![
            SseEvent {
                event: Some("message".into()),
                data: "héllo 🦀\nsecond".into()
            },
            SseEvent {
                event: None,
                data: String::new()
            }
        ]
    );
    assert!(
        parser
            .push(b"event: discarded\n\n: comment\n\n")
            .unwrap()
            .is_empty()
    );
    assert_eq!(parser.push(b"data: x\n\n").unwrap()[0].event, None);
}

#[test]
fn sse_parser_bounds_partial_lines_accumulated_events_and_chunk_output() {
    let mut parser = SseParser::default();
    for _ in 0..8 {
        parser.push(&vec![b'x'; 1024 * 1024]).unwrap();
    }
    assert!(parser.push(b"x").is_err());
    let mut parser = SseParser::default();
    let line = format!("data: {}\n", "x".repeat(1024 * 1024));
    for _ in 0..7 {
        parser.push(line.as_bytes()).unwrap();
    }
    assert!(parser.push(line.as_bytes()).is_err());
    let mut parser = SseParser::default();
    assert!(parser.push(&b"data\n\n".repeat(4097)).is_err());
}
