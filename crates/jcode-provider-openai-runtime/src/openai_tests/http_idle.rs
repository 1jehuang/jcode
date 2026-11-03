// Real HTTP/SSE regressions: the server deliberately keeps its body open.
async fn http_transport_idle_case(heartbeats: bool, complete: bool) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let _env_lock = jcode_base::storage::lock_test_env();
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _base = EnvVarGuard::set("JCODE_OPENAI_API_BASE", &format!("http://{addr}/v1"));
    let _idle = EnvVarGuard::set("JCODE_STREAM_IDLE_TIMEOUT_SECS", "1");
    jcode_base::config::invalidate_config_cache();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let (mut tcp, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let mut buf = [0; 4096];
            let n = tcp.read(&mut buf).await.unwrap();
            assert_ne!(n, 0);
            request.extend_from_slice(&buf[..n]);
            if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..end]);
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")?
                            .parse()
                            .ok()
                    })
                    .unwrap();
                if request.len() >= end + 4 + length {
                    break;
                }
            }
        }
        tcp.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n").await.unwrap();
        if heartbeats {
            // Longer than the idle budget, without any parsed StreamEvent.
            for _ in 0..5 {
                let frame = ": keepalive\n\ndata: {\"type\":\"response.in_progress\"}\n\n";
                tcp.write_all(format!("{:x}\r\n{}\r\n", frame.len(), frame).as_bytes())
                    .await
                    .unwrap();
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
        }
        if complete {
            let frame = "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":7,\"output_tokens\":2}}}\n\n";
            tcp.write_all(format!("{:x}\r\n{}\r\n", frame.len(), frame).as_bytes())
                .await
                .unwrap();
        }
        // No EOF or final HTTP chunk until the provider has returned.
        let _ = release_rx.await;
    });
    let creds = Arc::new(RwLock::new(CodexCredentials {
        access_token: "fixture-key".into(),
        refresh_token: String::new(),
        id_token: None,
        account_id: None,
        expires_at: None,
    }));
    let (tx, mut rx) = mpsc::channel(64);
    let started = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        openai_stream_runtime::stream_response(
            Client::new(),
            creds,
            None,
            serde_json::json!({"model":"fixture-model", "stream":true, "input":[]}),
            "fixture".into(),
            tx,
        ),
    )
    .await
    .expect("HTTP response must finish or time out without waiting for EOF");
    let _ = release_tx.send(());
    server.await.unwrap();
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event.unwrap());
    }
    if complete {
        assert!(result.is_ok(), "{result:?}");
        assert!(
            events
                .iter()
                .any(|e| matches!(e, StreamEvent::MessageEnd { .. }))
        );
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::TokenUsage {
                input_tokens: Some(7),
                output_tokens: Some(2),
                ..
            }
        )));
    } else {
        let error = result.unwrap_err();
        assert!(format!("{error:?}").contains("Stream read timeout"));
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, StreamEvent::MessageEnd { .. }))
        );
    }
    if heartbeats {
        assert!(started.elapsed() >= Duration::from_millis(1400));
    }
}

#[tokio::test]
async fn http_idle_heartbeats_keep_response_alive_until_completion() {
    http_transport_idle_case(true, true).await;
}

#[tokio::test]
async fn http_idle_completion_does_not_wait_for_eof() {
    http_transport_idle_case(false, true).await;
}

#[tokio::test]
async fn http_idle_silent_transport_still_times_out() {
    http_transport_idle_case(false, false).await;
}

#[tokio::test]
async fn http_idle_stopped_heartbeats_still_time_out() {
    http_transport_idle_case(true, false).await;
}
