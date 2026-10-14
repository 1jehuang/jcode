// Synthetic loopback regressions. No live account, model, or transcript data.
#[tokio::test]
async fn persistent_cancellation_old_keepalive_preserves_replacement() {
    let _env_lock = jcode_base::storage::lock_test_env();
    let (state, server) = test_persistent_ws_state().await;
    let old_connection = state.connected_at - Duration::from_secs(1);
    let persistent = Arc::new(Mutex::new(Some(state)));
    let keepalive = spawn_persistent_ws_keepalive_with_interval(
        Arc::downgrade(&persistent), old_connection, "fixture".to_string(), Duration::from_millis(1),
    );
    tokio::time::timeout(Duration::from_secs(1), keepalive).await.unwrap().unwrap();
    assert_eq!(persistent.lock().await.as_ref().unwrap().last_response_id, "resp_test");
    server.abort();
}

#[tokio::test]
async fn persistent_cancellation_keepalive_abort_invalidates_before_handoff() {
    let _env_lock = jcode_base::storage::lock_test_env();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (ping_tx, ping_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        assert!(matches!(ws.next().await.unwrap().unwrap(), WsMessage::Ping(_)));
        ping_tx.send(()).unwrap();
        std::future::pending::<()>().await;
        drop(ws);
    });
    let (ws_stream, _) = connect_async(format!("ws://{addr}")).await.unwrap();
    let connected_at = Instant::now();
    let persistent = Arc::new(Mutex::new(Some(PersistentWsState {
        ws_stream,
        identity: openai_websocket_prewarm::prewarm_identity(&prewarm_test_credentials()),
        last_response_id: "resp_previous".into(),
        connected_at,
        last_activity_at: connected_at - Duration::from_secs(20),
        last_response_completed_at: connected_at,
        message_count: 1,
        last_input_item_count: 0,
        last_input_item_hashes: vec![],
    })));
    let keepalive = spawn_persistent_ws_keepalive_with_interval(
        Arc::downgrade(&persistent), connected_at, "fixture".to_string(), Duration::from_millis(1),
    );
    tokio::time::timeout(Duration::from_secs(1), ping_rx).await.unwrap().unwrap();
    keepalive.abort();
    assert!(matches!(keepalive.await, Err(error) if error.is_cancelled()));
    assert!(persistent.lock().await.is_none(), "aborted ping round trip must not leave a reusable socket");
    server.abort();
}

#[tokio::test]
async fn persistent_cancellation_waiting_lock_preserves_current_owner() {
    let _env_lock = jcode_base::storage::lock_test_env();
    let (state, server) = test_persistent_ws_state().await;
    let provider = OpenAIProvider::new(prewarm_test_credentials());
    *provider.credentials.write().await = prewarm_test_credentials();
    provider.set_model("gpt-5.6-sol").unwrap();
    provider.set_transport("websocket").unwrap();
    jcode_base::provider::populate_account_models_for_scope(&provider.catalog_scope(),
        vec!["gpt-5.6-sol".to_string()]);
    let mut owner = provider.persistent_ws.lock().await;
    *owner = Some(state);
    let stream = provider.complete(&[ChatMessage::user("fixture")], &[], "fixture", None)
        .await.unwrap();
    tokio::task::yield_now().await;
    assert_eq!(Arc::strong_count(&provider.persistent_ws), 2);
    drop(stream);
    tokio::time::timeout(Duration::from_secs(1), async {
        while Arc::strong_count(&provider.persistent_ws) > 1 {
            tokio::task::yield_now().await;
        }
    }).await.expect("cancelled producer must stop even while another turn owns the mutex");
    assert_eq!(owner.as_ref().unwrap().last_response_id, "resp_test");
    server.abort();
}

#[tokio::test]
async fn persistent_cancellation_public_drop_releases_chain_and_reconnects() {
    let _env_lock = jcode_base::storage::lock_test_env();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _base = EnvVarGuard::set("JCODE_OPENAI_API_BASE", &format!("http://{addr}/v1"));
    let messages = vec![ChatMessage::user("original"), ChatMessage::assistant_text("earlier"),
        ChatMessage::user("next")];
    let input = build_responses_input(&messages);
    let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut stalled = tokio_tungstenite::accept_async(tcp).await.unwrap();
        let request = stalled.next().await.unwrap().unwrap();
        assert!(request.is_text());
        let request: Value = serde_json::from_str(request.to_text().unwrap()).unwrap();
        assert_eq!(request["previous_response_id"], "resp_previous", "{request}");
        stalled.send(WsMessage::Text(serde_json::json!({"type":"response.created",
            "response":{"id":"resp_incomplete"}}).to_string())).await.unwrap();
        stalled.send(WsMessage::Text(serde_json::json!({"type":"response.output_text.delta",
            "delta":"partial"}).to_string())).await.unwrap();
        accepted_tx.send(()).unwrap();
        // Retain the socket without returning an event or closing it.
        let (tcp, _) = listener.accept().await.unwrap();
        let mut fresh = tokio_tungstenite::accept_async(tcp).await.unwrap();
        let request: Value = serde_json::from_str(
            fresh.next().await.unwrap().unwrap().to_text().unwrap(),
        ).unwrap();
        assert!(request.get("previous_response_id").is_none());
        for event in [
            serde_json::json!({"type":"response.created","response":{"id":"resp_recovered"}}),
            serde_json::json!({"type":"response.output_text.delta","delta":"recovered"}),
            serde_json::json!({"type":"response.completed","response":{"id":"resp_recovered","status":"completed","output":[]}}),
        ] {
            fresh.send(WsMessage::Text(event.to_string())).await.unwrap();
        }
        while fresh.next().await.is_some() {}
        drop(stalled);
    });
    let (ws_stream, _) = connect_async(format!("ws://{addr}/v1/responses")).await.unwrap();
    let provider = OpenAIProvider::new(prewarm_test_credentials());
    *provider.credentials.write().await = prewarm_test_credentials();
    provider.set_model("gpt-5.6-sol").unwrap();
    provider.set_transport("websocket").unwrap();
    jcode_base::provider::populate_account_models_for_scope(&provider.catalog_scope(),
        vec!["gpt-5.6-sol".to_string()]);
    *provider.persistent_ws.lock().await = Some(PersistentWsState {
        ws_stream,
        identity: openai_websocket_prewarm::prewarm_identity(&prewarm_test_credentials()),
        last_response_id: "resp_previous".into(),
        connected_at: Instant::now(),
        last_activity_at: Instant::now(),
        last_response_completed_at: Instant::now(),
        message_count: 1,
        last_input_item_count: 1,
        last_input_item_hashes: persistent_ws_input_item_hashes(&input[..1]),
    });
    let mut stream = provider.complete(&messages, &[], "fixture", None).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while let Some(event) = stream.next().await {
            if matches!(event.unwrap(), StreamEvent::TextDelta(delta) if delta == "partial") {
                break;
            }
        }
        accepted_rx.await.unwrap();
    }).await.expect("fixture must accept the request");
    tokio::task::yield_now().await;
    drop(stream);
    tokio::time::timeout(Duration::from_secs(1), async {
        assert!(provider.persistent_ws.lock().await.is_none(),
            "cancelled response must be invalidated before mutex handoff");
    }).await.expect("dropping the consumer must release the persistent lock promptly");
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut next = provider.complete(&messages, &[], "fixture", None).await.unwrap();
        let mut text = String::new();
        while let Some(event) = next.next().await {
            if let StreamEvent::TextDelta(delta) = event.unwrap() {
                text.push_str(&delta);
            }
        }
        assert_eq!(text, "recovered");
        assert_eq!(provider.persistent_ws.lock().await.as_ref().unwrap().last_response_id,
            "resp_recovered");
    }).await.expect("next request must reconnect rather than reuse the cancelled response");
    server.abort();
}

#[tokio::test]
async fn persistent_cancellation_aborted_future_invalidates_before_handoff() {
    let _env_lock = jcode_base::storage::lock_test_env();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(tcp).await.unwrap();
        socket.next().await.unwrap().unwrap();
        accepted_tx.send(()).unwrap();
        std::future::pending::<()>().await;
    });
    let (ws_stream, _) = connect_async(format!("ws://{addr}")).await.unwrap();
    let input = vec![serde_json::json!({"role":"user","content":"original"}),
        serde_json::json!({"role":"user","content":"next"})];
    let persistent = Arc::new(Mutex::new(Some(PersistentWsState {
        ws_stream,
        identity: openai_websocket_prewarm::prewarm_identity(&prewarm_test_credentials()),
        last_response_id: "resp_previous".into(),
        connected_at: Instant::now(),
        last_activity_at: Instant::now(),
        last_response_completed_at: Instant::now(),
        message_count: 1,
        last_input_item_count: 1,
        last_input_item_hashes: persistent_ws_input_item_hashes(&input[..1]),
    })));
    let (tx, _rx) = mpsc::channel(10);
    let socket = Arc::clone(&persistent);
    let task = tokio::spawn(async move {
        try_persistent_ws_continuation(&socket,
            &Arc::new(RwLock::new(prewarm_test_credentials())),
            &serde_json::json!({"model":"gpt-5.6-sol"}), &input, input.len(), &tx).await
    });
    tokio::time::timeout(Duration::from_secs(1), accepted_rx).await.unwrap().unwrap();
    let socket = Arc::clone(&persistent);
    let waiter = tokio::spawn(async move {
        assert!(socket.lock().await.is_none(), "a cancelled in-flight socket must not be handed off");
    });
    tokio::task::yield_now().await;
    task.abort();
    assert!(matches!(task.await, Err(error) if error.is_cancelled()));
    tokio::time::timeout(Duration::from_secs(1), waiter).await.unwrap().unwrap();
    server.abort();
}
