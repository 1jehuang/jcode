// Public Provider::complete + EventStream regressions using a loopback Responses
// server. These are deterministic protocol fixtures, not live OpenAI acceptance.
async fn persistent_terminal_public_case(
    error_kind: &str,
    code: Option<&str>,
    next_on_error: bool,
) {
    let _env_lock = jcode_base::storage::lock_test_env();
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _base = EnvVarGuard::set("JCODE_OPENAI_API_BASE", &format!("http://{addr}/v1"));
    let error_kind = error_kind.to_owned();
    let code = code.map(str::to_owned);
    let recover = code.as_deref() == Some("previous_response_not_found");
    let messages = vec![
        ChatMessage::user("original"),
        ChatMessage::assistant_text("earlier"),
        ChatMessage::user("next"),
    ];
    let full_input = build_responses_input(&messages);
    let expected_input = full_input.clone();
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        let delta: Value =
            serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
        assert_eq!(delta["previous_response_id"], "resp_stale");
        assert!(delta["input"].as_array().unwrap().len() < expected_input.len());
        let error = serde_json::json!({"type":"invalid_request_error", "code":code,
            "message": if recover {"Previous response not found."} else {"No tool output found for function call call_fixture."}});
        let frame = if error_kind == "response.failed" {
            serde_json::json!({"type":error_kind,"response":{"status":"failed","error":error}})
        } else {
            serde_json::json!({"type":error_kind,"error":error})
        };
        ws.send(WsMessage::Text(frame.to_string())).await.unwrap();
        // Keep the failed socket open, just like the incident. A correct client
        // drops it rather than waiting for another frame or sending a new delta.
        while let Some(frame) = ws.next().await {
            match frame {
                Ok(WsMessage::Ping(p)) => {
                    let _ = ws.send(WsMessage::Pong(p)).await;
                }
                Ok(WsMessage::Text(text)) => panic!("reused failed socket: {text}"),
                _ => break,
            }
        }
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        let fresh: Value =
            serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
        assert!(fresh.get("previous_response_id").is_none(), "{fresh}");
        assert_eq!(fresh["input"], serde_json::json!(expected_input));
        for event in [
            serde_json::json!({"type":"response.created","response":{"id":"resp_fresh"}}),
            serde_json::json!({"type":"response.output_text.delta","delta":"recovered"}),
            serde_json::json!({"type":"response.completed","response":{"id":"resp_fresh","status":"completed","output":[]}}),
        ] {
            ws.send(WsMessage::Text(event.to_string())).await.unwrap();
        }
        // Retain a healthy connection until the test tears down the provider.
        while ws.next().await.is_some() {}
    });
    let (ws_stream, _) = connect_async(format!("ws://{addr}/v1/responses"))
        .await
        .unwrap();
    let provider = OpenAIProvider::new(prewarm_test_credentials());
    *provider.credentials.write().await = prewarm_test_credentials();
    provider.set_model("gpt-5.6-sol").unwrap();
    provider.set_transport("websocket").unwrap();
    *provider.persistent_ws.lock().await = Some(PersistentWsState {
        ws_stream,
        identity: openai_websocket_prewarm::prewarm_identity(&prewarm_test_credentials()),
        last_response_id: "resp_stale".into(),
        connected_at: Instant::now(),
        last_activity_at: Instant::now(),
        last_response_completed_at: Instant::now(),
        message_count: 1,
        last_input_item_count: 1,
    });
    let outcome = tokio::time::timeout(Duration::from_secs(2), async {
        let mut stream = provider
            .complete(&messages, &[], "fixture", None)
            .await
            .unwrap();
        let mut errors = 0;
        let mut text = String::new();
        while let Some(event) = stream.next().await {
            match event.unwrap() {
                StreamEvent::Error { message, .. } => {
                    errors += 1;
                    assert!(message.contains("No tool output found"), "{message}");
                    if next_on_error {
                        break;
                    }
                }
                StreamEvent::TextDelta(delta) => text.push_str(&delta),
                _ => {}
            }
        }
        if recover {
            assert_eq!(errors, 0);
        } else {
            assert_eq!(errors, 1);
            // Keep the old stream alive while the next call acquires the mutex.
            let mut next = provider
                .complete(&messages, &[], "fixture", None)
                .await
                .unwrap();
            while let Some(event) = next.next().await {
                match event.unwrap() {
                    StreamEvent::TextDelta(delta) => text.push_str(&delta),
                    StreamEvent::Error { message, .. } => panic!("next call failed: {message}"),
                    _ => {}
                }
            }
        }
        assert_eq!(text, "recovered");
        assert_eq!(
            provider
                .persistent_ws
                .lock()
                .await
                .as_ref()
                .unwrap()
                .last_response_id,
            "resp_fresh"
        );
    })
    .await;
    server.abort();
    assert!(
        outcome.is_ok(),
        "terminal error left stream or next complete() stalled"
    );
}

#[tokio::test]
async fn persistent_terminal_public_stream_ends() {
    persistent_terminal_public_case("error", None, false).await;
}

#[tokio::test]
async fn persistent_terminal_public_next_call_not_stalled() {
    persistent_terminal_public_case("response.failed", None, true).await;
}

#[tokio::test]
async fn persistent_terminal_public_missing_previous_full_replay() {
    persistent_terminal_public_case("error", Some("previous_response_not_found"), false).await;
}

// Synthetic concurrency regression: a mutex waiter is queued before failure,
// so a caller-side clear after the helper returns cannot hide a stale handoff.
#[tokio::test]
async fn persistent_terminal_failure_invalidates_before_mutex_handoff() {
    let _env_lock = jcode_base::storage::lock_test_env();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (request_seen_tx, request_seen_rx) = tokio::sync::oneshot::channel();
    let (fail_tx, fail_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
        assert!(matches!(
            ws.next().await.unwrap().unwrap(),
            WsMessage::Text(_)
        ));
        request_seen_tx.send(()).unwrap();
        fail_rx.await.unwrap();
        ws.send(WsMessage::Text(
            r#"{"type":"error","error":{"type":"server_error","message":"internal server error"}}"#
                .into(),
        ))
        .await
        .unwrap();
        while ws.next().await.is_some_and(|frame| frame.is_ok()) {}
    });
    let (ws_stream, _) = connect_async(format!("ws://{addr}")).await.unwrap();
    let credentials = Arc::new(RwLock::new(prewarm_test_credentials()));
    let persistent_ws = Arc::new(Mutex::new(Some(PersistentWsState {
        ws_stream,
        identity: openai_websocket_prewarm::prewarm_identity(&prewarm_test_credentials()),
        last_response_id: "resp_stale".into(),
        connected_at: Instant::now(),
        last_activity_at: Instant::now(),
        last_response_completed_at: Instant::now(),
        message_count: 1,
        last_input_item_count: 1,
    })));
    let state = Arc::clone(&persistent_ws);
    let (tx, mut rx) = mpsc::channel(100);
    let attempt = tokio::spawn(async move {
        try_persistent_ws_continuation(
            &state,
            &credentials,
            &serde_json::json!({"model":"gpt-5.6-sol"}),
            &[
                serde_json::json!({"role":"user","content":"old"}),
                serde_json::json!({"role":"user","content":"new"}),
            ],
            2,
            &tx,
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        request_seen_rx.await.unwrap();
        let waiter = persistent_ws.lock();
        tokio::pin!(waiter);
        assert!(futures::poll!(&mut waiter).is_pending());
        fail_tx.send(()).unwrap();
        assert!(
            waiter.await.is_none(),
            "queued request observed failed response chain"
        );
        assert!(matches!(
            attempt.await.unwrap(),
            PersistentWsResult::Failed(_)
        ));
        while let Some(event) = rx.recv().await {
            assert!(
                !matches!(event.unwrap(), StreamEvent::Error { .. }),
                "retryable failure must not leak a terminal error"
            );
        }
        server.await.unwrap();
    })
    .await
    .expect("failure should promptly release persistent state");
}
