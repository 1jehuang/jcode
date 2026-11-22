use super::tests::EnvVarGuard;
use super::*;

#[test]
fn available_models_include_gemini_defaults() {
    let provider = GeminiProvider::new();
    let models = provider.available_models();
    assert!(models.contains(&"gemini-3-pro-preview"));
    assert!(models.contains(&"gemini-3.1-pro-preview"));
    assert!(models.contains(&"gemini-2.5-pro"));
    assert!(models.contains(&"gemini-2.5-flash"));
}

#[test]
fn set_model_accepts_gemini_models() {
    let provider = GeminiProvider::new();
    provider.set_model("gemini-2.5-flash").unwrap();
    assert_eq!(provider.model(), "gemini-2.5-flash");
}

#[test]
fn detects_model_not_found_errors() {
    let err = anyhow::anyhow!(
        "Gemini request generateContent failed (HTTP 404 Not Found): {{\"error\":{{\"status\":\"NOT_FOUND\",\"message\":\"Requested entity was not found.\"}}}}"
    );
    assert!(is_gemini_model_not_found_error(&err));
}

#[test]
fn extract_gemini_model_ids_discovers_nested_models() {
    let response = json!({
        "routing": {
            "manual": {
                "models": [
                    {"id": "gemini-3-pro-preview"},
                    {"name": "gemini-3.1-pro-preview"}
                ]
            },
            "auto": ["gemini-3-flash-preview", "not-a-model"]
        }
    });

    assert_eq!(
        extract_gemini_model_ids(&response),
        vec![
            "gemini-3.1-pro-preview".to_string(),
            "gemini-3-pro-preview".to_string(),
            "gemini-3-flash-preview".to_string(),
        ]
    );
}

#[test]
fn available_models_display_prefers_discovered_models_and_current_model() {
    let provider = GeminiProvider::new();
    provider.set_model("gemini-4-pro-preview").unwrap();
    *provider.fetched_models.write().unwrap() = vec![
        "gemini-3-flash-preview".to_string(),
        "gemini-3-pro-preview".to_string(),
    ];

    assert_eq!(
        provider.available_models_display(),
        vec![
            "gemini-3-pro-preview".to_string(),
            "gemini-3-flash-preview".to_string(),
            "gemini-4-pro-preview".to_string(),
        ]
    );
}

#[test]
fn available_models_display_without_discovery_uses_current_model_only() {
    let _guard = jcode_base::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("tempdir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let provider = GeminiProvider::new();
    provider.set_model("gemini-4-pro-preview").unwrap();

    assert_eq!(
        provider.available_models_display(),
        vec!["gemini-4-pro-preview".to_string()]
    );
}

#[test]
fn available_models_display_seeds_from_persisted_catalog() {
    let _guard = jcode_base::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("tempdir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let path = GeminiProvider::persisted_catalog_path().expect("catalog path");
    jcode_base::storage::write_json(
        &path,
        &PersistedCatalog {
            models: vec!["gemini-3-pro-preview".to_string()],
            fetched_at_rfc3339: chrono::Utc::now().to_rfc3339(),
        },
    )
    .expect("write persisted catalog");

    let provider = GeminiProvider::new();
    assert!(
        provider
            .available_models_display()
            .contains(&"gemini-3-pro-preview".to_string())
    );
}

#[test]
fn direct_neon_404_preserves_selected_model_and_original_error() {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio_stream::StreamExt;
    let _lock = jcode_base::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", home.path());
    let _key = EnvVarGuard::set_value("GEMINI_API_KEY", "loopback-key");
    let _force = EnvVarGuard::set_value("JCODE_GEMINI_FORCE_OAUTH", "false");
    let _version = EnvVarGuard::set_value("GEMINI_API_VERSION", "v1beta");
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            let _endpoint = EnvVarGuard::set_value("GEMINI_API_ENDPOINT", &endpoint);
            let server = tokio::spawn(async move {
                let mut reader = BufReader::new(listener.accept().await.unwrap().0);
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                assert!(
                    line.starts_with(
                        "POST /v1beta/models/gemini-4-neon:streamGenerateContent?alt=sse "
                    ),
                    "unexpected request line: {line}"
                );
                let mut length = 0;
                loop {
                    line.clear();
                    assert!(reader.read_line(&mut line).await.unwrap() > 0);
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(n) = line.to_lowercase().strip_prefix("content-length:") {
                        length = n.trim().parse().unwrap();
                    }
                }
                reader.read_exact(&mut vec![0; length]).await.unwrap();
                reader
                    .get_mut()
                    .write_all(
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await
                    .unwrap();
            });
            let mut provider = GeminiProvider::new();
            provider.client = reqwest::Client::builder().no_proxy().build().unwrap();
            provider.set_model("gemini:gemini-4-neon").unwrap();
            let mut events = provider.complete(&[], &[], "test", None).await.unwrap();
            let err = loop {
                if let Err(err) = events.next().await.expect("terminal model error") {
                    break err;
                }
            };
            assert!(err.to_string().contains("gemini-4-neon"));
            assert!(format!("{err:#}").contains("HTTP 404"));
            assert_eq!(provider.model(), "gemini-4-neon");
            server.await.unwrap();
        });
}

#[test]
fn developer_catalog_paginates_and_preserves_cache_on_failure() {
    use std::io::{BufRead, Write};
    let _lock = jcode_base::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", home.path());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let _endpoint = EnvVarGuard::set_value("GEMINI_API_ENDPOINT", &endpoint);
    let _version = EnvVarGuard::set_value("GEMINI_API_VERSION", "v1beta");
    let server = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let responses = [
            (
                "200 OK",
                r#"{"models":[{"name":"models/gemini-new-pro","supportedGenerationMethods":["generateContent"]},{"name":"models/gemini-embedding-only","supportedGenerationMethods":["embedContent"]}],"nextPageToken":"opaque+/= token"}"#,
            ),
            (
                "200 OK",
                r#"{"models":[{"name":"models/gemini-new-flash"}]}"#,
            ),
            ("401 Unauthorized", r#"{"error":{"message":"invalid key"}}"#),
            (
                "200 OK",
                r#"{"models":[{"name":"models/gemini-partial"}],"nextPageToken":"repeat"}"#,
            ),
            ("200 OK", r#"{"models":[],"nextPageToken":"repeat"}"#),
        ];
        for (index, (status, body)) in responses.into_iter().enumerate() {
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "catalog request timed out"
                        );
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(err) => panic!("accept: {err}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut reader = std::io::BufReader::new(&stream);
            let mut request = String::new();
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
                request.push_str(&line);
            }
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("x-goog-api-key: local-key")
            );
            let path = request
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap();
            let url = reqwest::Url::parse(&format!("http://localhost{path}")).unwrap();
            assert_eq!(url.path(), "/v1beta/models");
            if index == 1 {
                assert_eq!(
                    url.query_pairs()
                        .find(|(key, _)| key == "pageToken")
                        .unwrap()
                        .1,
                    "opaque+/= token"
                );
            }
            write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut provider = GeminiProvider::new();
        provider.client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let models = provider
            .refresh_available_models_api_key("local-key")
            .await
            .unwrap();
        assert!(models.contains(&"gemini-new-pro".to_string()));
        assert!(models.contains(&"gemini-new-flash".to_string()));
        assert!(!models.contains(&"gemini-embedding-only".to_string()));
        assert!(
            provider
                .refresh_available_models_api_key("local-key")
                .await
                .is_err()
        );
        assert_eq!(*provider.fetched_models.read().unwrap(), models);
        assert!(
            provider
                .refresh_available_models_api_key("local-key")
                .await
                .is_err()
        );
        assert_eq!(*provider.fetched_models.read().unwrap(), models);
    });
    server.join().unwrap();
}

#[test]
fn developer_api_streams_thoughts_live_over_sse() {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio_stream::StreamExt;
    let _lock = jcode_base::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", home.path());
    let _key = EnvVarGuard::set_value("GEMINI_API_KEY", "loopback-key");
    let _force = EnvVarGuard::set_value("JCODE_GEMINI_FORCE_OAUTH", "false");
    let _version = EnvVarGuard::set_value("GEMINI_API_VERSION", "v1beta");
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            let _endpoint = EnvVarGuard::set_value("GEMINI_API_ENDPOINT", &endpoint);
            let (release_chunk2_tx, release_chunk2_rx) = tokio::sync::oneshot::channel::<()>();
            let server = tokio::spawn(async move {
                let mut reader = BufReader::new(listener.accept().await.unwrap().0);
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                assert!(
                    line.starts_with(
                        "POST /v1beta/models/gemini-2.5-pro:streamGenerateContent?alt=sse "
                    ),
                    "unexpected request line: {line}"
                );
                let mut length = 0usize;
                loop {
                    line.clear();
                    assert!(reader.read_line(&mut line).await.unwrap() > 0);
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(n) = line.to_lowercase().strip_prefix("content-length:") {
                        length = n.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0u8; length];
                reader.read_exact(&mut body).await.unwrap();
                let req_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(
                    req_json["generationConfig"]["thinkingConfig"]["includeThoughts"],
                    serde_json::json!(true),
                    "request must enable includeThoughts: {req_json}"
                );

                let stream = reader.get_mut();
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                    )
                    .await
                    .unwrap();
                stream
                    .write_all(
                        b"data: {\"modelVersion\":\"gemini-2.5-pro\",\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"First thought chunk.\",\"thought\":true}]}}]}\r\n\r\n",
                    )
                    .await
                    .unwrap();
                stream.flush().await.unwrap();

                // Hold the stream open until the client actually receives the
                // first ThinkingDelta! Proves incremental live streaming (not
                // buffered until EOF).
                release_chunk2_rx.await.unwrap();

                stream
                    .write_all(
                        b"data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\" Second thought chunk.\",\"thought\":true}]}}]}\r\n\r\n",
                    )
                    .await
                    .unwrap();
                stream
                    .write_all(
                        b"data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"Final answer.\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":12,\"candidatesTokenCount\":4,\"thoughtsTokenCount\":20}}\r\n\r\n",
                    )
                    .await
                    .unwrap();
                stream.flush().await.unwrap();
            });

            let mut provider = GeminiProvider::new();
            provider.client = reqwest::Client::builder().no_proxy().build().unwrap();
            provider.set_model("gemini:gemini-2.5-pro").unwrap();
            let mut events = provider.complete(&[], &[], "test", None).await.unwrap();

            let mut release_tx = Some(release_chunk2_tx);
            let mut thoughts = Vec::new();
            let mut texts = Vec::new();
            let mut saw_thinking_start = false;
            let mut saw_thinking_end = false;
            let mut saw_thinking_done = false;
            let mut saw_message_end = false;

            while let Some(ev) = events.next().await {
                match ev.unwrap() {
                    StreamEvent::ThinkingStart => saw_thinking_start = true,
                    StreamEvent::ThinkingDelta(delta) => {
                        thoughts.push(delta);
                        if let Some(tx) = release_tx.take() {
                            // Received live BEFORE server wrote chunk 2!
                            let _ = tx.send(());
                        }
                    }
                    StreamEvent::ThinkingEnd => saw_thinking_end = true,
                    StreamEvent::ThinkingDone { .. } => saw_thinking_done = true,
                    StreamEvent::TextDelta(delta) => texts.push(delta),
                    StreamEvent::MessageEnd { stop_reason } => {
                        assert_eq!(stop_reason.as_deref(), Some("stop"));
                        saw_message_end = true;
                    }
                    _ => {}
                }
            }

            assert!(saw_thinking_start);
            assert!(saw_thinking_end);
            assert!(saw_thinking_done);
            assert!(saw_message_end);
            assert_eq!(
                thoughts,
                vec!["First thought chunk.", " Second thought chunk."]
            );
            assert_eq!(texts, vec!["Final answer."]);
            server.await.unwrap();
        });
}

#[test]
fn developer_api_retries_without_thinking_config_when_rejected() {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio_stream::StreamExt;
    let _lock = jcode_base::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", home.path());
    let _key = EnvVarGuard::set_value("GEMINI_API_KEY", "loopback-key");
    let _force = EnvVarGuard::set_value("JCODE_GEMINI_FORCE_OAUTH", "false");
    let _version = EnvVarGuard::set_value("GEMINI_API_VERSION", "v1beta");
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            let _endpoint = EnvVarGuard::set_value("GEMINI_API_ENDPOINT", &endpoint);
            let server = tokio::spawn(async move {
                for attempt in 0..2 {
                    let mut reader = BufReader::new(listener.accept().await.unwrap().0);
                    let mut line = String::new();
                    reader.read_line(&mut line).await.unwrap();
                    let mut length = 0usize;
                    loop {
                        line.clear();
                        assert!(reader.read_line(&mut line).await.unwrap() > 0);
                        if line == "\r\n" {
                            break;
                        }
                        if let Some(n) = line.to_lowercase().strip_prefix("content-length:") {
                            length = n.trim().parse().unwrap();
                        }
                    }
                    let mut body = vec![0u8; length];
                    reader.read_exact(&mut body).await.unwrap();
                    let req_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
                    let stream = reader.get_mut();
                    if attempt == 0 {
                        assert_eq!(
                            req_json["generationConfig"]["thinkingConfig"]["includeThoughts"],
                            serde_json::json!(true)
                        );
                        let err_body = r#"{"error":{"code":400,"message":"Invalid JSON payload received. Unknown name \"thinkingConfig\" at 'generation_config'"}}"#;
                        let resp = format!(
                            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            err_body.len(),
                            err_body
                        );
                        stream.write_all(resp.as_bytes()).await.unwrap();
                    } else {
                        assert!(
                            req_json.get("generationConfig").is_none(),
                            "retry must omit generationConfig: {req_json}"
                        );
                        stream
                            .write_all(
                                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"Fallback ok\"}]},\"finishReason\":\"STOP\"}]}\r\n\r\n",
                            )
                            .await
                            .unwrap();
                    }
                    stream.flush().await.unwrap();
                }
            });

            let mut provider = GeminiProvider::new();
            provider.client = reqwest::Client::builder().no_proxy().build().unwrap();
            provider.set_model("gemini:gemini-2.5-flash").unwrap();
            let mut events = provider.complete(&[], &[], "test", None).await.unwrap();
            let mut texts = Vec::new();
            while let Some(ev) = events.next().await {
                if let StreamEvent::TextDelta(delta) = ev.unwrap() {
                    texts.push(delta);
                }
            }
            assert_eq!(texts, vec!["Fallback ok"]);
            server.await.unwrap();
        });
}
