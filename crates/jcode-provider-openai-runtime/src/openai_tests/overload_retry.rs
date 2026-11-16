// Exercise real Provider::complete retries, not a reconstructed diagnostic chain.
async fn overload_public_retry_case(recover: bool, partial: bool) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let _lock = jcode_base::storage::lock_test_env();
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _base = EnvVarGuard::set("JCODE_OPENAI_API_BASE", &format!("http://{addr}/v1"));
    let _idle = EnvVarGuard::set("JCODE_STREAM_IDLE_TIMEOUT_SECS", "1");
    jcode_base::config::invalidate_config_cache();
    let server = tokio::spawn(async move {
        let mut attempts = 0;
        loop {
            let (mut tcp, _) = listener.accept().await.unwrap();
            let mut data = Vec::new();
            loop {
                let mut buf = [0; 4096];
                let n = tcp.read(&mut buf).await.unwrap();
                assert_ne!(n, 0);
                data.extend_from_slice(&buf[..n]);
                if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&data[..end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")?
                                .parse::<usize>()
                                .ok()
                        })
                        .unwrap_or(0);
                    if data.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            if data.starts_with(b"GET /v1/models ") {
                tcp.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"data\":[]}").await.unwrap();
                continue;
            }
            assert!(data.starts_with(b"POST /v1/responses "));
            attempts += 1;
            if attempts == 1 {
                if partial {
                    let body = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"discard me\"}\n\ndata: {\"type\":\"error\",\"error\":{\"type\":\"service_unavailable_error\",\"code\":\"server_is_overloaded\",\"message\":\"Our servers are currently overloaded.\"}}\n\n";
                    tcp.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                } else {
                    let body = r#"{"error":{"type":"service_unavailable_error","code":"server_is_overloaded","message":"Our servers are currently overloaded. Please try again later."}}"#;
                    tcp.write_all(format!("HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\nContent-Length: {}\r\nRetry-After: 0\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                }
            } else if recover {
                let body = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"recovered\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n";
                tcp.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                return attempts;
            } else {
                tcp.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n").await.unwrap();
                let mut buf = [0; 1];
                assert_eq!(tcp.read(&mut buf).await.unwrap(), 0);
                if attempts == MAX_RETRIES {
                    return attempts;
                }
            }
        }
    });
    let provider = OpenAIProvider::new(prewarm_test_credentials());
    provider.set_transport("https").unwrap();
    // Keep the fixture independent of the user's cached model catalog.
    *provider.model.write().await = "gpt-5.6-sol".into();
    let messages = [ChatMessage::user("fixture")];
    let mut stream = provider
        .complete(&messages, &[], "fixture", None)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        let mut notices = 0;
        let mut errors = Vec::new();
        let mut text = String::new();
        let mut rolled_back = false;
        while let Some(event) = stream.next().await {
            match event {
                Ok(StreamEvent::StatusDetail { detail })
                    if detail.starts_with("OpenAI reported server overload") =>
                {
                    assert!(errors.is_empty(), "notice must precede terminal failure");
                    assert!(detail.contains("server_is_overloaded"));
                    if partial {
                        assert!(
                            rolled_back,
                            "durable notice must not fence partial output before rollback"
                        );
                    }
                    notices += 1;
                }
                Ok(StreamEvent::TextDelta(delta)) => text.push_str(&delta),
                Ok(StreamEvent::RetryRollback { .. }) => {
                    rolled_back = true;
                    text.clear();
                }
                Err(error) => errors.push(error.to_string()),
                _ => {}
            }
        }
        assert_eq!(notices, 1);
        let attempts = server.await.unwrap();
        if recover {
            assert!(errors.is_empty(), "{errors:?}");
            assert_eq!(text, "recovered");
            assert_eq!(attempts, 2);
        } else {
            assert_eq!(errors.len(), 1, "{errors:?}");
            assert!(errors[0].contains("server_is_overloaded"), "{errors:?}");
            assert!(errors[0].contains("Stream read timeout"), "{errors:?}");
            assert_eq!(attempts, MAX_RETRIES);
        }
    })
    .await
    .expect("public overload retry fixture stalled");
}

#[tokio::test]
async fn overload_public_retry_retains_report_before_later_timeout() {
    overload_public_retry_case(false, false).await;
}

#[tokio::test]
async fn overload_public_retry_notice_survives_successful_recovery() {
    overload_public_retry_case(true, false).await;
}

#[tokio::test]
async fn overload_public_retry_rolls_back_partial_output_before_notice() {
    overload_public_retry_case(true, true).await;
}
