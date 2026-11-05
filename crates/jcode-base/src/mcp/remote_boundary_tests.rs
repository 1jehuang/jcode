#[tokio::test]
async fn initialization_and_notification_errors_do_not_return_live_client() {
    for plan in [
        Plan {
            notification_status: 500,
            ..Plan::default()
        },
        Plan {
            protocol_version: "2099-01-01".into(),
            ..Plan::default()
        },
    ] {
        let fixture = Fixture::start(plan).await;
        let err = McpClient::connect("bad-handshake".into(), &fixture.config("http"))
            .await
            .err()
            .unwrap();
        assert!(
            !fixture
                .requests()
                .iter()
                .any(|r| r.body["method"] == "tools/list"),
            "{err:#}"
        );
    }
}

#[tokio::test]
async fn static_authorization_401_never_falls_back_or_retries() {
    let fixture = Fixture::start(Plan {
        initialize_status: Some(401),
        ..Plan::default()
    })
    .await;
    let err = McpClient::connect("unauthorized".into(), &fixture.config("http"))
        .await
        .err()
        .unwrap();
    assert!(format!("{err:#}").contains("configured Authorization"));
    assert_eq!(fixture.requests().len(), 1);
}

#[tokio::test]
async fn legacy_endpoint_cross_origin_and_redirects_never_leak_headers() {
    let attacker = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let attacker_url = format!("http://{}/steal", attacker.local_addr().unwrap());
    for plan in [
        Plan {
            legacy: true,
            endpoint: Some(attacker_url.clone()),
            ..Plan::default()
        },
        Plan {
            initialize_status: Some(307),
            redirect: Some(attacker_url.clone()),
            ..Plan::default()
        },
    ] {
        let fixture = Fixture::start(plan.clone()).await;
        let result = McpClient::connect(
            "secure".into(),
            &fixture.config(if plan.legacy { "sse" } else { "http" }),
        )
        .await;
        assert!(result.is_err());
        assert!(
            tokio::time::timeout(Duration::from_millis(100), attacker.accept())
                .await
                .is_err(),
            "server-selected origin received a credential-bearing request"
        );
        if plan.legacy {
            wait_until(|| fixture.state.active_streams.load(Ordering::SeqCst) == 0).await;
        }
    }
}

#[tokio::test]
async fn endpoint_timeout_and_cancelled_connect_drop_legacy_reader() {
    for cancel in [true, false] {
        let fixture = Fixture::start(Plan {
            legacy: true,
            omit_endpoint: true,
            ..Plan::default()
        })
        .await;
        let mut config = fixture.config("sse");
        config.timeout_secs = Some(1);
        let connect =
            tokio::spawn(async move { McpClient::connect("no-endpoint".into(), &config).await });
        wait_until(|| fixture.state.active_streams.load(Ordering::SeqCst) == 1).await;
        if cancel {
            connect.abort();
        }
        let result = connect.await;
        if !cancel {
            assert!(result.unwrap().is_err());
        }
        wait_until(|| fixture.state.active_streams.load(Ordering::SeqCst) == 0).await;
        assert_eq!(fixture.state.disconnected_streams.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn remote_handshake_deadline_is_independent_of_long_tool_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let (started_tx, started_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let (request, mut socket) = parse_request(socket).await.unwrap();
        assert_eq!(request.body["method"], "initialize");
        open_stream(&mut socket, "text/event-stream").await.unwrap();
        started_tx.send(()).unwrap();
        // Never send a response. The client must close the socket on deadline.
        let mut bytes = Vec::new();
        socket.read_to_end(&mut bytes).await.unwrap();
    });
    let config: McpServerConfig = serde_json::from_value(json!({
        "type": "http", "url": url, "timeout_secs": 3600
    }))
    .unwrap();
    let connect =
        tokio::spawn(async move { McpClient::connect("bounded-handshake".into(), &config).await });
    tokio::time::timeout(Duration::from_secs(2), started_rx)
        .await
        .unwrap()
        .unwrap();

    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(29)).await;
    assert!(!connect.is_finished());
    tokio::time::advance(Duration::from_secs(2)).await;
    let err = connect.await.unwrap().err().unwrap();
    tokio::time::resume();
    assert!(format!("{err:#}").contains("connection timed out after 30s"));
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("deadline must close the response stream")
        .unwrap();
}

#[tokio::test]
async fn bounded_concurrency_applies_backpressure_and_queue_timeout() {
    let fixture = Fixture::start(Plan::default()).await;
    let mut config = fixture.config("http");
    config.timeout_secs = Some(1);
    let mut client = McpClient::connect("backpressure".into(), &config)
        .await
        .unwrap();
    let mut callers = JoinSet::new();
    for _ in 0..96 {
        let handle = client.handle();
        callers.spawn(async move { handle.call_tool("hang", json!({})).await });
    }
    wait_until(|| fixture.state.active_streams.load(Ordering::SeqCst) == MAX_CONCURRENT_REQUESTS)
        .await;
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(
        fixture.state.active_streams.load(Ordering::SeqCst),
        MAX_CONCURRENT_REQUESTS
    );
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(result) = callers.join_next().await {
            assert!(result.unwrap().is_err());
        }
    })
    .await
    .unwrap();
    wait_until(|| fixture.state.active_streams.load(Ordering::SeqCst) == 0).await;
    client.shutdown().await.unwrap();
}
