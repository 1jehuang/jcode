// Exercise the public completion stream against a deterministic loopback server.
// No persistent state is fabricated: every cursor/hash is learned from a response.
type PrefixTestSocket = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

async fn prefix_test_request(socket: &mut PrefixTestSocket) -> Value {
    loop {
        match socket
            .next()
            .await
            .expect("request frame")
            .expect("valid frame")
        {
            WsMessage::Text(text) => return serde_json::from_str(&text).expect("request JSON"),
            WsMessage::Ping(payload) => socket.send(WsMessage::Pong(payload)).await.unwrap(),
            other => panic!("expected response.create, got {other:?}"),
        }
    }
}

async fn prefix_test_response(socket: &mut PrefixTestSocket, id: &str) {
    for event in [
        serde_json::json!({"type":"response.created","response":{"id":id}}),
        serde_json::json!({"type":"response.output_text.delta","delta":id}),
        serde_json::json!({"type":"response.completed","response":{"id":id,"status":"completed","output":[]}}),
    ] {
        socket
            .send(WsMessage::Text(event.to_string()))
            .await
            .unwrap();
    }
}

async fn prefix_test_complete(provider: &OpenAIProvider, messages: &[ChatMessage], id: &str) {
    let mut events = provider
        .complete(messages, &[], "prefix fixture", None)
        .await
        .unwrap();
    let mut text = String::new();
    while let Some(event) = events.next().await {
        match event.expect("valid completion event") {
            StreamEvent::TextDelta(delta) => text.push_str(&delta),
            StreamEvent::Error { message, .. } => panic!("completion failed: {message}"),
            _ => {}
        }
    }
    assert_eq!(text, id);
    let guard = provider.persistent_ws.lock().await;
    let state = guard.as_ref().expect("completed response retained");
    let input = build_responses_input(messages);
    assert_eq!(state.last_response_id, id);
    assert_eq!(state.last_input_item_count, input.len());
    assert_eq!(
        state.last_input_item_hashes,
        persistent_ws_input_item_hashes(&input)
    );
}

async fn prefix_test_provider() -> OpenAIProvider {
    let provider = OpenAIProvider::new(prewarm_test_credentials());
    *provider.credentials.write().await = prewarm_test_credentials();
    provider.set_model("gpt-5.6-sol").unwrap();
    provider.set_transport("websocket").unwrap();
    provider
}

async fn persistent_prefix_changed_output_case(had_real_output: bool) {
    let _lock = jcode_base::storage::lock_test_env();
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _base = EnvVarGuard::set("JCODE_OPENAI_API_BASE", &format!("http://{addr}/v1"));
    let _transport = EnvVarGuard::set("JCODE_OPENAI_TRANSPORT", "websocket");
    let _prewarm = EnvVarGuard::set("JCODE_OPENAI_PREWARM", "0");

    let mut original: Vec<_> = (0..58)
        .map(|i| ChatMessage::user(&format!("history {i}")))
        .collect();
    original.push(assistant_tool_use(
        "call_prefix",
        "bash",
        serde_json::json!({"command":"ls"}),
    ));
    if had_real_output {
        original.push(ChatMessage::tool_result("call_prefix", "old output", false));
    }
    original.push(ChatMessage::user("prior tail"));
    let mut changed = original.clone();
    if had_real_output {
        changed[59] = ChatMessage::tool_result("call_prefix", "real output preserved", false);
    } else {
        // A late result removes the synthetic missing-output item, shifting
        // the prior tail and placing the real output before the old cursor.
        changed.push(ChatMessage::tool_result(
            "call_prefix",
            "real output preserved",
            false,
        ));
    }
    changed.push(ChatMessage::user("new tail one"));
    changed.push(ChatMessage::user("new tail two"));
    let original_input = build_responses_input(&original);
    let changed_input = build_responses_input(&changed);
    assert_eq!(original_input.len(), 61);
    assert_eq!(changed_input.len(), 63);
    assert_eq!(
        original_input
            .iter()
            .zip(&changed_input)
            .take_while(|(a, b)| a == b)
            .count(),
        59
    );
    let output_pos = function_call_output_pos(&changed_input, "call_prefix").unwrap();
    assert!(
        output_pos < original_input.len(),
        "real output must precede old cursor"
    );
    assert_eq!(
        function_call_outputs(&changed_input, "call_prefix"),
        vec!["real output preserved"]
    );

    let mut server = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(tcp).await.unwrap();
        let first = prefix_test_request(&mut socket).await;
        assert!(first.get("previous_response_id").is_none());
        assert_eq!(first["input"], serde_json::json!(original_input));
        prefix_test_response(&mut socket, "resp_prefix_old").await;
        // A fresh socket is essential. Reject any delta on the old chain even
        // if a retry would later succeed and hide the original regression.
        while let Some(frame) = socket.next().await {
            match frame {
                Ok(WsMessage::Ping(payload)) => {
                    socket.send(WsMessage::Pong(payload)).await.unwrap()
                }
                Ok(WsMessage::Close(_)) | Err(_) => break,
                other => panic!("changed prefix reused old socket: {other:?}"),
            }
        }
        let (tcp, _) = listener.accept().await.unwrap();
        let mut fresh = tokio_tungstenite::accept_async(tcp).await.unwrap();
        let request = prefix_test_request(&mut fresh).await;
        assert_eq!(request["type"], "response.create");
        assert!(request.get("previous_response_id").is_none(), "{request}");
        assert_eq!(request["input"], serde_json::json!(changed_input));
        assert_eq!(
            request["input"][output_pos]["output"],
            "real output preserved"
        );
        prefix_test_response(&mut fresh, "resp_prefix_fresh").await;
    });
    let provider = prefix_test_provider().await;
    let outcome = tokio::time::timeout(Duration::from_secs(5), async {
        prefix_test_complete(&provider, &original, "resp_prefix_old").await;
        prefix_test_complete(&provider, &changed, "resp_prefix_fresh").await;
        (&mut server)
            .await
            .expect("prefix loopback server assertions");
    })
    .await;
    if outcome.is_err() {
        server.abort();
        let result = server.await;
        assert!(
            !matches!(&result, Err(error) if error.is_panic()),
            "loopback server panicked: {result:?}"
        );
    }
    outcome.expect("changed-prefix completions and server must finish");
}

#[tokio::test]
async fn persistent_prefix_late_tool_output_before_cursor_replays_full_input() {
    persistent_prefix_changed_output_case(false).await;
}

#[tokio::test]
async fn persistent_prefix_mutated_tool_output_with_growing_input_replays_full_input() {
    persistent_prefix_changed_output_case(true).await;
}

#[tokio::test]
async fn persistent_prefix_append_only_reuses_socket_and_refreshes_hashes_each_turn() {
    let _lock = jcode_base::storage::lock_test_env();
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _base = EnvVarGuard::set("JCODE_OPENAI_API_BASE", &format!("http://{addr}/v1"));
    let _transport = EnvVarGuard::set("JCODE_OPENAI_TRANSPORT", "websocket");
    let _prewarm = EnvVarGuard::set("JCODE_OPENAI_PREWARM", "0");
    let mut messages = vec![ChatMessage::user("first")];
    let mut turns = vec![messages.clone()];
    for i in 1..4 {
        messages.push(assistant_tool_use(
            &format!("call_append_{i}"),
            "bash",
            serde_json::json!({"command":"pwd"}),
        ));
        messages.push(ChatMessage::tool_result(
            &format!("call_append_{i}"),
            &format!("result {i}"),
            false,
        ));
        messages.push(ChatMessage::user(&format!("turn {i}")));
        turns.push(messages.clone());
    }
    let inputs: Vec<_> = turns
        .iter()
        .map(|turn| build_responses_input(turn))
        .collect();
    let mut server = tokio::spawn(async move {
        // Exactly one accept: reconnecting cannot satisfy this fixture.
        let (tcp, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(tcp).await.unwrap();
        let mut cursor = 0;
        for (i, input) in inputs.iter().enumerate() {
            let request = prefix_test_request(&mut socket).await;
            assert_eq!(request["type"], "response.create");
            if i == 0 {
                assert!(request.get("previous_response_id").is_none());
            } else {
                assert_eq!(
                    request["previous_response_id"],
                    format!("resp_append_{}", i - 1)
                );
            }
            assert_eq!(
                request["input"],
                serde_json::json!(&input[cursor..]),
                "turn {i}"
            );
            cursor = input.len();
            prefix_test_response(&mut socket, &format!("resp_append_{i}")).await;
        }
    });
    let provider = prefix_test_provider().await;
    let outcome = tokio::time::timeout(Duration::from_secs(5), async {
        for (i, turn) in turns.iter().enumerate() {
            prefix_test_complete(&provider, turn, &format!("resp_append_{i}")).await;
            assert_eq!(
                provider
                    .persistent_ws
                    .lock()
                    .await
                    .as_ref()
                    .unwrap()
                    .message_count,
                i + 1
            );
        }
        (&mut server)
            .await
            .expect("append-only loopback server assertions");
    })
    .await;
    if outcome.is_err() {
        server.abort();
        let result = server.await;
        assert!(
            !matches!(&result, Err(error) if error.is_panic()),
            "loopback server panicked: {result:?}"
        );
    }
    outcome.expect("append-only completions and server must finish");
}
