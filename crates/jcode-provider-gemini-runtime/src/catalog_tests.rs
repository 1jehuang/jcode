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
fn fallback_models_skip_current_model() {
    assert_eq!(
        gemini_fallback_models("gemini-2.5-flash"),
        vec![
            "gemini-pro-latest",
            "gemini-3.1-pro-preview",
            "gemini-3-pro-preview",
            "gemini-2.5-pro",
            "gemini-3-flash-preview",
            "gemini-2.0-flash",
        ]
    );
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
