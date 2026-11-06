#[tokio::test]
async fn public_json_connect_list_call_refresh_and_shutdown() {
    let fixture = Fixture::start(Plan::default()).await;
    let mut client = fixture.connect("http").await;
    assert!(client.is_remote());
    assert!(client.is_running());
    assert_eq!(client.server_info().unwrap().name, "localhost");
    assert_eq!(client.tools()[0].name, "echo");
    assert_eq!(
        text(
            client
                .call_tool("echo", json!({"word":"héllo"}))
                .await
                .unwrap()
        ),
        "{\"word\":\"héllo\"}"
    );
    assert_eq!(
        text(client.call_tool("echo", Value::Null).await.unwrap()),
        "{}"
    );
    client.refresh_tools().await.unwrap();
    client.call_tool("session-rotate", json!({})).await.unwrap();
    client.call_tool("echo", json!({})).await.unwrap();
    assert!(!fixture.state.ordering_violation.load(Ordering::SeqCst));
    let handle = client.handle();
    client.shutdown().await.unwrap();
    assert!(!client.is_running());
    assert!(handle.call_tool("echo", json!({})).await.is_err());
    let requests = fixture.requests();
    assert_eq!(requests[0].body["method"], "initialize");
    assert!(!requests[0].headers.contains_key(SESSION_HEADER));
    for request in requests.iter().skip(1) {
        assert_eq!(
            request.headers.get(SESSION_HEADER).map(String::as_str),
            Some("fixture-session")
        );
        assert_eq!(
            request.headers.get(PROTOCOL_HEADER).map(String::as_str),
            Some("2025-06-18")
        );
        assert_eq!(
            request.headers.get("authorization").map(String::as_str),
            Some("Bearer fixture-secret")
        );
    }
    assert_eq!(requests.last().unwrap().method, "DELETE");
}

#[tokio::test]
async fn public_post_sse_fragmentation_notifications_and_multiline_json() {
    let fixture = Fixture::start(Plan {
        post_sse: true,
        ..Plan::default()
    })
    .await;
    let mut client = fixture.connect("http").await;
    assert_eq!(
        text(
            client
                .call_tool("echo", json!({"word":"héllo 🦀"}))
                .await
                .unwrap()
        ),
        "{\"word\":\"héllo 🦀\"}"
    );
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn public_legacy_and_http_fallback_workflows() {
    for (transport, status) in [("sse", 405), ("http", 400), ("http", 404), ("http", 405)] {
        let fixture = Fixture::start(Plan {
            legacy: true,
            fallback_status: status,
            protocol_version: "2024-11-05".into(),
            ..Plan::default()
        })
        .await;
        let mut client = fixture.connect(transport).await;
        assert_eq!(
            text(client.call_tool("echo", json!({"ok":true})).await.unwrap()),
            "{\"ok\":true}"
        );
        client.refresh_tools().await.unwrap();
        assert!(!fixture.state.ordering_violation.load(Ordering::SeqCst));
        let legacy_initialize = fixture
            .requests()
            .into_iter()
            .find(|r| r.path.starts_with("/messages") && r.body["method"] == "initialize")
            .unwrap();
        assert_eq!(
            legacy_initialize.body["params"]["protocolVersion"],
            "2024-11-05"
        );
        client.shutdown().await.unwrap();
        wait_until(|| fixture.state.active_streams.load(Ordering::SeqCst) == 0).await;
        assert!(fixture.state.disconnected_streams.load(Ordering::SeqCst) >= 1);
    }
}

#[tokio::test]
async fn concurrent_calls_do_not_block_or_cross_correlate() {
    let fixture = Fixture::start(Plan::default()).await;
    let mut client = fixture.connect("http").await;
    let slow_handle = client.handle();
    let slow = tokio::spawn(async move {
        slow_handle
            .call_tool("slow", json!({"request":"slow"}))
            .await
            .unwrap()
    });
    wait_until(|| {
        fixture
            .requests()
            .iter()
            .any(|r| r.body.pointer("/params/name") == Some(&json!("slow")))
    })
    .await;
    let fast = tokio::time::timeout(
        Duration::from_millis(200),
        client.call_tool("echo", json!({"request":"fast"})),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(text(fast), "{\"request\":\"fast\"}");
    assert!(!slow.is_finished());
    assert_eq!(text(slow.await.unwrap()), "{\"request\":\"slow\"}");
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_and_drop_cancel_owned_http_streams_and_waiters() {
    for explicit in [true, false] {
        let fixture = Fixture::start(Plan::default()).await;
        let mut client = fixture.connect("http").await;
        let handle = client.handle();
        let request_handle = handle.clone();
        let request =
            tokio::spawn(async move { request_handle.call_tool("hang", json!({})).await });
        wait_until(|| fixture.state.active_streams.load(Ordering::SeqCst) == 1).await;
        if explicit {
            client.shutdown().await.unwrap();
        }
        drop(client);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), request)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        wait_until(|| fixture.state.active_streams.load(Ordering::SeqCst) == 0).await;
        assert_eq!(fixture.state.disconnected_streams.load(Ordering::SeqCst), 1);
        assert!(handle.call_tool("echo", json!({})).await.is_err());
    }
}

#[tokio::test]
async fn caller_cancellation_aborts_response_stream_without_shutdown() {
    let fixture = Fixture::start(Plan::default()).await;
    let mut client = fixture.connect("http").await;
    let handle = client.handle();
    let request = tokio::spawn(async move { handle.call_tool("hang", json!({})).await });
    wait_until(|| fixture.state.active_streams.load(Ordering::SeqCst) == 1).await;
    request.abort();
    let _ = request.await;
    wait_until(|| fixture.state.active_streams.load(Ordering::SeqCst) == 0).await;
    client.call_tool("echo", json!({})).await.unwrap();
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn session_404_fails_all_waiters_and_requires_fresh_connect() {
    let fixture = Fixture::start(Plan::default()).await;
    let mut client = fixture.connect("http").await;
    let handle = client.handle();
    let hung = tokio::spawn(async move { handle.call_tool("hang", json!({})).await });
    wait_until(|| fixture.state.active_streams.load(Ordering::SeqCst) == 1).await;
    let err = client
        .call_tool("expire", json!({}))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("session expired"), "{err}");
    assert!(!client.is_running());
    assert!(
        tokio::time::timeout(Duration::from_secs(1), hung)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
    wait_until(|| fixture.state.active_streams.load(Ordering::SeqCst) == 0).await;
    client.shutdown().await.unwrap();
    let mut fresh = fixture.connect("http").await;
    fresh.call_tool("echo", json!({})).await.unwrap();
    fresh.shutdown().await.unwrap();
}

#[tokio::test]
async fn invalid_responses_and_oversized_bodies_fail_promptly() {
    let fixture = Fixture::start(Plan::default()).await;
    let mut client = fixture.connect("http").await;
    for tool in [
        "invalid-json",
        "empty",
        "accepted",
        "wrong-type",
        "wrong-id",
        "invalid-envelope",
        "oversized-json",
        "oversized-chunked",
        "oversized-sse",
        "server-error",
    ] {
        let err = tokio::time::timeout(Duration::from_secs(2), client.call_tool(tool, json!({})))
            .await
            .expect("must fail without request timeout")
            .unwrap_err();
        assert!(
            !err.to_string().contains("Request timeout"),
            "{tool}: {err:#}"
        );
    }
    client.call_tool("echo", json!({})).await.unwrap();
    client.shutdown().await.unwrap();
}
