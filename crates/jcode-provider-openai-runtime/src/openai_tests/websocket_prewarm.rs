fn prewarm_test_credentials() -> CodexCredentials {
    CodexCredentials {
        access_token: "sk-prewarm-test".to_string(),
        refresh_token: String::new(),
        id_token: None,
        account_id: None,
        expires_at: None,
    }
}

fn prewarm_test_tool() -> ToolDefinition {
    ToolDefinition {
        name: "lookup".to_string(),
        description: "look up a value".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": { "key": { "type": "string" } },
            "required": ["key"]
        }),
    }
}

fn prewarm_user_message(text: &str) -> ChatMessage {
    ChatMessage {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: text.to_string(),
            cache_control: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }
}

async fn wait_for_prewarm(slot: &openai_websocket_prewarm::PrewarmSlot) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !slot.is_ready() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("prewarm should become ready");
}

#[tokio::test]
async fn websocket_v2_prewarm_is_adopted_by_complete_without_losing_request_state() {
    let _lock = jcode_base::storage::lock_test_env();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local websocket server");
    let addr = listener.local_addr().expect("local websocket address");
    let _base = EnvVarGuard::set("JCODE_OPENAI_API_BASE", &format!("http://{addr}/v1"));
    let _transport = EnvVarGuard::set("JCODE_OPENAI_TRANSPORT", "websocket");
    let _prewarm = EnvVarGuard::set("JCODE_OPENAI_PREWARM", "1");

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept prewarm connection");
        let mut socket = tokio_tungstenite::accept_hdr_async(
            stream,
            |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
             response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                assert_eq!(request.uri().path(), "/v1/responses");
                assert_eq!(
                    request.headers()["openai-beta"],
                    openai_websocket_prewarm::WEBSOCKET_V2_BETA
                );
                assert_eq!(request.headers()["authorization"], "Bearer sk-prewarm-test");
                Ok(response)
            },
        )
        .await
        .expect("complete prewarm handshake");

        let warm: Value = serde_json::from_str(
            socket
                .next()
                .await
                .expect("warm request")
                .expect("valid warm frame")
                .into_text()
                .expect("warm text frame")
                .as_ref(),
        )
        .expect("warm JSON");
        assert_eq!(warm["type"], "response.create");
        assert_eq!(warm["model"], "gpt-5.6-sol");
        assert_eq!(warm["instructions"], "preserve this system prompt");
        assert_eq!(warm["input"], serde_json::json!([]));
        assert_eq!(warm["generate"], false);
        assert_eq!(warm["reasoning"]["effort"], "high");
        assert_eq!(warm["tools"][0]["name"], "lookup");
        socket
            .send(WsMessage::Text(
                r#"{"type":"response.created","response":{"id":"resp_warm"}}"#.into(),
            ))
            .await
            .expect("send warm created");
        socket.send(WsMessage::Text(
            r#"{"type":"response.completed","response":{"id":"resp_warm","status":"completed","output":[]}}"#.into(),
        )).await.expect("send warm completed");

        let generated: Value = serde_json::from_str(
            socket
                .next()
                .await
                .expect("generated request")
                .expect("valid generated frame")
                .into_text()
                .expect("generated text frame")
                .as_ref(),
        )
        .expect("generated JSON");
        assert_eq!(generated["type"], "response.create");
        assert_eq!(generated["previous_response_id"], "resp_warm");
        assert_eq!(generated["model"], "gpt-5.6-sol");
        assert_eq!(generated["instructions"], "preserve this system prompt");
        assert_eq!(generated["reasoning"]["effort"], "high");
        assert_eq!(generated["tools"][0]["name"], "lookup");
        assert!(
            generated.get("generate").is_none(),
            "generate:false must not leak"
        );
        let input = generated["input"]
            .as_array()
            .expect("generated input array");
        assert!(input.iter().any(|item| item["type"] == "reasoning"
            && item["id"] == "rs_history"
            && item["encrypted_content"] == "encrypted-history"));
        assert!(input
            .iter()
            .any(|item| item.to_string().contains("first user turn")));
        assert!(input
            .iter()
            .any(|item| item.to_string().contains("current user turn")));

        socket
            .send(WsMessage::Text(
                r#"{"type":"response.created","response":{"id":"resp_generated"}}"#.into(),
            ))
            .await
            .expect("send generated created");
        socket
            .send(WsMessage::Text(
                r#"{"type":"response.output_text.delta","delta":"continued output"}"#.into(),
            ))
            .await
            .expect("send continuation output");
        socket.send(WsMessage::Text(
            r#"{"type":"response.completed","response":{"id":"resp_generated","status":"completed","output":[]}}"#.into(),
        )).await.expect("send generated completed");
    });

    let provider = OpenAIProvider::new(prewarm_test_credentials());
    provider.set_model("gpt-5.6-sol").expect("set test model");
    provider
        .set_reasoning_effort("high")
        .expect("set reasoning effort");
    let tools = vec![prewarm_test_tool()];
    provider
        .prewarm(&tools, "preserve this system prompt")
        .await;
    wait_for_prewarm(&provider.prewarm).await;

    let messages = vec![
        prewarm_user_message("first user turn"),
        ChatMessage {
            role: Role::Assistant,
            content: vec![
                ContentBlock::OpenAIReasoning {
                    id: "rs_history".to_string(),
                    summary: vec!["prior thought".to_string()],
                    encrypted_content: Some("encrypted-history".to_string()),
                    status: Some("completed".to_string()),
                },
                ContentBlock::Text {
                    text: "prior answer".to_string(),
                    cache_control: None,
                },
            ],
            timestamp: None,
            tool_duration_ms: None,
        },
        prewarm_user_message("current user turn"),
    ];
    let mut events = provider
        .complete(&messages, &tools, "preserve this system prompt", None)
        .await
        .expect("start warmed completion");
    let mut output = String::new();
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(event) = events.next().await {
            if let StreamEvent::TextDelta(text) = event.expect("valid stream event") {
                output.push_str(&text);
            }
        }
    })
    .await
    .expect("warmed completion should finish");
    assert_eq!(output, "continued output");
    server.await.expect("local websocket server");
}

#[tokio::test]
async fn unfinished_or_incompatible_prewarm_is_cancelled_without_foreground_wait() {
    let _lock = jcode_base::storage::lock_test_env();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _base = EnvVarGuard::set("JCODE_OPENAI_API_BASE", &format!("http://{addr}/v1"));
    let credentials = prewarm_test_credentials();
    let slot = Arc::new(openai_websocket_prewarm::PrewarmSlot::default());
    let request = serde_json::json!({
        "model": "gpt-5.6-sol", "instructions": "original", "tools": [],
        "input": [], "stream": true
    });
    let accepted = Arc::new(tokio::sync::Notify::new());
    let server_accepted = Arc::clone(&accepted);
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let _ = socket.next().await;
        server_accepted.notify_one();
        let closed = tokio::time::timeout(Duration::from_secs(1), socket.next()).await;
        assert!(matches!(
            closed,
            Ok(None) | Ok(Some(Ok(WsMessage::Close(_)))) | Ok(Some(Err(_)))
        ));
    });
    slot.start(Arc::new(RwLock::new(credentials.clone())), &request);
    accepted.notified().await;

    let incompatible = serde_json::json!({
        "model": "gpt-5.6-sol", "instructions": "changed", "tools": [],
        "input": [], "stream": true
    });
    let before = Instant::now();
    assert!(slot.take_ready(&incompatible, &credentials).is_none());
    assert!(
        before.elapsed() < Duration::from_millis(100),
        "foreground must not wait for warmup"
    );
    server.await.expect("unfinished server");
}

#[tokio::test]
async fn ready_prewarm_with_different_settings_is_invalidated() {
    let _lock = jcode_base::storage::lock_test_env();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _base = EnvVarGuard::set("JCODE_OPENAI_API_BASE", &format!("http://{addr}/v1"));
    let credentials = prewarm_test_credentials();
    let slot = Arc::new(openai_websocket_prewarm::PrewarmSlot::default());
    let request = serde_json::json!({
        "model": "gpt-5.6-sol", "instructions": "original", "tools": [], "input": []
    });
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let _ = socket.next().await;
        socket
            .send(WsMessage::Text(
                r#"{"type":"response.created","response":{"id":"resp_settings"}}"#.into(),
            ))
            .await
            .unwrap();
        socket.send(WsMessage::Text(
            r#"{"type":"response.completed","response":{"id":"resp_settings","status":"completed","output":[]}}"#.into(),
        )).await.unwrap();
        let _ = socket.next().await;
    });
    slot.start(Arc::new(RwLock::new(credentials.clone())), &request);
    wait_for_prewarm(&slot).await;
    let changed = serde_json::json!({
        "model": "gpt-5.6-sol", "instructions": "changed", "tools": [], "input": []
    });
    assert!(slot.take_ready(&changed, &credentials).is_none());
    server.await.expect("settings mismatch server");
}

#[tokio::test]
async fn rejected_warmup_is_not_adopted() {
    let _lock = jcode_base::storage::lock_test_env();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _base = EnvVarGuard::set("JCODE_OPENAI_API_BASE", &format!("http://{addr}/v1"));
    let credentials = prewarm_test_credentials();
    let slot = Arc::new(openai_websocket_prewarm::PrewarmSlot::default());
    let request =
        serde_json::json!({"model":"gpt-5.6-sol","instructions":"system","tools":[],"input":[]});
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let _ = socket.next().await;
        socket
            .send(WsMessage::Text(
                r#"{"type":"response.created","response":{"id":"resp_rejected"}}"#.into(),
            ))
            .await
            .unwrap();
        socket.send(WsMessage::Text(
            r#"{"type":"response.completed","response":{"id":"resp_rejected","status":"failed","output":[]}}"#.into(),
        )).await.unwrap();
    });
    slot.start(Arc::new(RwLock::new(credentials.clone())), &request);
    server.await.expect("rejection server");
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(slot.take_ready(&request, &credentials).is_none());
}
