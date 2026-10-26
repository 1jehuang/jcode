// Public Provider::complete tests use only a loopback WS/HTTP server.
const OPAQUE_REJECTION: &str =
    r#"{"type":"error","error":{"type":"invalid_request_error","message":"Invalid request"}}"#;

async fn opaque_recovery_public_case(
    persistent: bool,
    prefix: Option<Value>,
    specific: bool,
    http_failure: bool,
    cancel: bool,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let _env_lock = jcode_base::storage::lock_test_env();
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _base = EnvVarGuard::set("JCODE_OPENAI_API_BASE", &format!("http://{addr}/v1"));
    let should_recover = prefix.is_none() && !specific;
    let (http_seen_tx, http_seen_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let mut ws = loop {
            let (tcp, _) = listener.accept().await.unwrap();
            if let Ok(ws) = tokio_tungstenite::accept_async(tcp).await {
                break ws;
            }
        };
        let wire: Value =
            serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
        if let Some(prefix) = prefix {
            ws.send(WsMessage::Text(prefix.to_string())).await.unwrap();
        }
        let error = if specific {
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"Invalid request","code":"invalid_function_parameters","param":"tools"}}"#
        } else {
            OPAQUE_REJECTION
        };
        ws.send(WsMessage::Text(error.into())).await.unwrap();
        // Failure must invalidate the old socket, not reuse it for recovery.
        while let Some(Ok(frame)) = ws.next().await {
            assert!(!matches!(frame, WsMessage::Text(_)), "failed WS was reused");
            if let WsMessage::Ping(p) = frame {
                let _ = ws.send(WsMessage::Pong(p)).await;
            }
        }
        if should_recover {
            let (mut tcp, mut data) = loop {
                let (mut tcp, _) = listener.accept().await.unwrap();
                let mut data = Vec::new();
                while !data.windows(4).any(|w| w == b"\r\n\r\n") {
                    let mut buf = [0; 4096];
                    let n = tcp.read(&mut buf).await.unwrap();
                    assert_ne!(n, 0);
                    data.extend_from_slice(&buf[..n]);
                }
                if data.starts_with(b"GET /v1/models ") {
                    tcp.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"data\":[]}").await.unwrap();
                    continue;
                }
                break (tcp, data);
            };
            let (header_end, length) = loop {
                if let Some(pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&data[..pos]);
                    assert!(headers.starts_with("POST /v1/responses "), "{headers}");
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_lowercase()
                                .strip_prefix("content-length: ")
                                .and_then(|s| s.parse::<usize>().ok())
                        })
                        .unwrap();
                    break (pos + 4, length);
                }
            };
            while data.len() < header_end + length {
                let mut buf = [0; 4096];
                let n = tcp.read(&mut buf).await.unwrap();
                assert_ne!(n, 0);
                data.extend_from_slice(&buf[..n]);
            }
            let http: Value =
                serde_json::from_slice(&data[header_end..header_end + length]).unwrap();
            assert!(http.get("previous_response_id").is_none());
            assert_eq!(http["instructions"], wire["instructions"]);
            assert_eq!(http["tools"], wire["tools"]);
            assert_eq!(http["reasoning"], wire["reasoning"]);
            let serialized = http["input"].to_string();
            assert!(serialized.contains("encrypted-history"));
            assert!(serialized.contains("retained tool output"));
            if !persistent {
                let mut expected = wire.clone();
                expected.as_object_mut().unwrap().remove("type");
                // The WS framing and SSE transport flags are the only wire differences.
                expected["stream"] = Value::Bool(true);
                assert_eq!(http, expected);
            }
            let _ = http_seen_tx.send(http);
            if cancel {
                let mut buf = [0; 1];
                assert_eq!(
                    tcp.read(&mut buf).await.unwrap(),
                    0,
                    "cancelled HTTP socket remained open"
                );
            } else {
                let body = if http_failure {
                    format!("data: {OPAQUE_REJECTION}\n\n")
                } else {
                    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_recovered\"}}\n\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"recovered\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_recovered\",\"status\":\"completed\",\"output\":[]}}\n\n".into()
                };
                tcp.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            }
        }
        assert!(tokio::time::timeout(Duration::from_millis(150), async {
            loop {
                let (mut tcp, _) = listener.accept().await.unwrap();
                let mut data = Vec::new();
                while !data.windows(4).any(|w| w == b"\r\n\r\n") {
                    let mut buf = [0; 4096];
                    let n = tcp.read(&mut buf).await.unwrap();
                    assert_ne!(n, 0);
                    data.extend_from_slice(&buf[..n]);
                }
                assert!(data.starts_with(b"GET /v1/models "), "unexpected extra response request: {}", String::from_utf8_lossy(&data));
                tcp.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"data\":[]}").await.unwrap();
            }
        }).await.is_err());
    });
    let provider = OpenAIProvider::new(prewarm_test_credentials());
    *provider.credentials.write().await = prewarm_test_credentials();
    provider.set_model("gpt-5.6-sol").unwrap();
    provider.set_transport("websocket").unwrap();
    let messages = vec![
        ChatMessage::user("original"),
        ChatMessage {
            role: Role::Assistant,
            content: vec![ContentBlock::OpenAIReasoning {
                id: "rs_history".into(),
                summary: vec!["prior thought".into()],
                encrypted_content: Some("encrypted-history".into()),
                status: Some("completed".into()),
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        assistant_tool_use(
            "call_history",
            "read",
            serde_json::json!({"path":"fixture"}),
        ),
        ChatMessage::tool_result("call_history", "retained tool output", false),
        ChatMessage::user("next"),
    ];
    if persistent {
        let (ws_stream, _) = connect_ws(format!("ws://{addr}/v1/responses"))
            .await
            .unwrap();
        let input = build_responses_input(&messages);
        *provider.persistent_ws.lock().await = Some(PersistentWsState {
            ws_stream,
            identity: openai_websocket_prewarm::prewarm_identity(
                &prewarm_test_credentials(),
                None,
                None,
            ),
            cache_key: Some(provider.cache_routing_key()),
            last_response_id: "resp_previous".into(),
            connected_at: Instant::now(),
            last_activity_at: Instant::now(),
            last_response_completed_at: Instant::now(),
            message_count: 1,
            last_input_item_count: 1,
            last_response_output: Vec::new(),
            last_input_item_hashes: persistent_ws_input_item_hashes(&input[..1]),
        });
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut stream = provider
            .complete(&messages, &[], "fixture", None)
            .await
            .unwrap();
        if cancel {
            // Poll the stream while waiting for the recovery request.
            tokio::pin!(http_seen_rx);
            loop {
                tokio::select! {
                    seen = &mut http_seen_rx => { seen.unwrap(); break; },
                    event = stream.next() => { assert!(event.is_some()); }
                }
            }
            drop(stream);
        } else {
            let mut errors = Vec::new();
            let mut text = String::new();
            while let Some(event) = stream.next().await {
                match event.unwrap() {
                    StreamEvent::Error { message, .. } => errors.push(message),
                    StreamEvent::TextDelta(delta) => text.push_str(&delta),
                    _ => {}
                }
            }
            if should_recover {
                let http = http_seen_rx.await.unwrap();
                assert_eq!(
                    http["input"],
                    serde_json::json!(build_responses_input(&messages))
                );
            }
            if should_recover && !http_failure {
                assert!(errors.is_empty(), "{errors:?}");
                assert_eq!(text, "recovered");
            } else {
                assert_eq!(errors.len(), 1, "{errors:?}");
                assert!(errors[0].contains("Invalid request"));
            }
        }
        assert!(provider.persistent_ws.lock().await.is_none());
        server.await.unwrap();
    })
    .await
    .expect("recovery stalled");
}

#[tokio::test]
async fn opaque_recovery_fresh_success() {
    opaque_recovery_public_case(false, None, false, false, false).await;
}
#[tokio::test]
async fn opaque_recovery_persistent_success() {
    opaque_recovery_public_case(true, None, false, false, false).await;
}
#[tokio::test]
async fn opaque_recovery_repeated_rejection_bounded() {
    opaque_recovery_public_case(false, None, false, true, false).await;
}
#[tokio::test]
async fn opaque_recovery_specific_schema_untouched() {
    opaque_recovery_public_case(false, None, true, false, false).await;
}
#[tokio::test]
async fn opaque_recovery_after_created_forbidden() {
    opaque_recovery_public_case(
        false,
        Some(serde_json::json!({"type":"response.created","response":{"id":"resp_started"}})),
        false,
        false,
        false,
    )
    .await;
}
#[tokio::test]
async fn opaque_recovery_after_text_forbidden() {
    opaque_recovery_public_case(
        false,
        Some(serde_json::json!({"type":"response.output_text.delta","delta":"partial"})),
        false,
        false,
        false,
    )
    .await;
}
#[tokio::test]
async fn opaque_recovery_after_thinking_forbidden() {
    opaque_recovery_public_case(
        false,
        Some(serde_json::json!({"type":"response.reasoning_summary_text.delta","delta":"partial"})),
        false,
        false,
        false,
    )
    .await;
}
#[tokio::test]
async fn opaque_recovery_after_tool_forbidden() {
    opaque_recovery_public_case(true, Some(serde_json::json!({"type":"response.output_item.added","item":{"type":"function_call","id":"fc","call_id":"call","name":"read","arguments":""}})), false, false, false).await;
}
#[tokio::test]
async fn opaque_recovery_cancel_invalidates_sockets() {
    opaque_recovery_public_case(true, None, false, false, true).await;
}
