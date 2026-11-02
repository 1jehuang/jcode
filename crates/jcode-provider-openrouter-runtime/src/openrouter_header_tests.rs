use super::tests::make_custom_compatible_provider;
use super::tests::spawn_single_response_models_server;
use super::tests::{ENV_LOCK, EnvVarGuard};
use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

/// Spawns a single-request HTTP server that captures the raw request bytes and
/// replies with a canned SSE stream. Used by the header-override tests below:
/// the DNS override points the hostname at a local listener, so the full
/// stream path (including retries) is exercised.
fn spawn_header_capturing_server() -> (std::net::SocketAddr, std::sync::mpsc::Receiver<String>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("read timeout");
        let mut buf = vec![0u8; 65536];
        let n = stream.read(&mut buf).unwrap_or(0);
        let _ = tx.send(String::from_utf8_lossy(&buf[..n]).to_string());
        let body = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = stream.write_all(response.as_bytes());
    });
    (addr, rx)
}

#[test]
fn config_header_overrides_apply_on_the_wire_and_replace_built_in_user_agent() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(async {
        let (addr, rx) = spawn_header_capturing_server();
        let host = "override.example.test";
        let client = reqwest::Client::builder()
            .resolve(host, addr)
            .build()
            .expect("client");
        let api_base = format!("http://{host}:{}/zen/go/v1", addr.port());
        let mut overrides = HeaderMap::new();
        overrides.insert(
            reqwest::header::USER_AGENT,
            reqwest::header::HeaderValue::from_static("corp-agent/9.9"),
        );
        overrides.insert(
            reqwest::header::HeaderName::from_static("x-tenant-id"),
            reqwest::header::HeaderValue::from_static("tenant-42"),
        );
        let (tx, mut events) = tokio::sync::mpsc::channel::<anyhow::Result<StreamEvent>>(64);
        super::openrouter_sse_stream::run_stream_with_retries(
            client,
            api_base,
            ProviderAuth::None {
                label: "test".to_string(),
            },
            false,
            overrides,
            "conv-headers-1".to_string(),
            serde_json::json!({"model": "m", "messages": [], "stream": true}),
            false,
            tx,
            Arc::new(Mutex::new(None)),
            "m".to_string(),
            false,
        )
        .await;
        while events.recv().await.is_some() {}
        let raw = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("server captured request");

        let lines: Vec<&str> = raw.lines().take_while(|line| !line.is_empty()).collect();
        let user_agents: Vec<&str> = lines
            .iter()
            .filter(|line| line.to_ascii_lowercase().starts_with("user-agent:"))
            .map(|line| {
                line.split_once(':')
                    .map(|(_, value)| value.trim())
                    .unwrap_or("")
            })
            .collect();
        assert_eq!(
            user_agents,
            vec!["corp-agent/9.9"],
            "exactly one User-Agent, the configured one:\n{raw}"
        );
        assert!(
            lines.contains(&"x-tenant-id: tenant-42"),
            "custom header missing:\n{raw}"
        );
    });
}

#[test]
fn empty_config_header_overrides_keep_default_headers() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(async {
        let (addr, rx) = spawn_header_capturing_server();
        let host = "plain.example.test";
        let client = reqwest::Client::builder()
            .resolve(host, addr)
            .build()
            .expect("client");
        let api_base = format!("http://{host}:{}/zen/go/v1", addr.port());
        let (tx, mut events) = tokio::sync::mpsc::channel::<anyhow::Result<StreamEvent>>(64);
        super::openrouter_sse_stream::run_stream_with_retries(
            client,
            api_base,
            ProviderAuth::None {
                label: "test".to_string(),
            },
            false,
            HeaderMap::new(),
            "conv-headers-2".to_string(),
            serde_json::json!({"model": "m", "messages": [], "stream": true}),
            false,
            tx,
            Arc::new(Mutex::new(None)),
            "m".to_string(),
            false,
        )
        .await;
        while events.recv().await.is_some() {}
        let raw = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("server captured request");
        let user_agents: Vec<&str> = raw
            .lines()
            .take_while(|line| !line.is_empty())
            .filter(|line| line.to_ascii_lowercase().starts_with("user-agent:"))
            .map(|line| {
                line.split_once(':')
                    .map(|(_, value)| value.trim())
                    .unwrap_or("")
            })
            .collect();
        // Empty overrides must not inject anything: the bare test client has
        // no default UA, so zero is fine; there must never be duplicates.
        assert!(user_agents.len() <= 1, "duplicate User-Agent:\n{raw}");
        assert!(
            !raw.contains("tenant-42"),
            "empty overrides must not leak headers:\n{raw}"
        );
    });
}

#[test]
fn config_header_overrides_apply_to_model_catalog_requests() {
    let _lock = ENV_LOCK.lock();
    let home = tempfile::tempdir().expect("scratch JCODE_HOME");
    let _jcode_home = EnvVarGuard::set("JCODE_HOME", home.path());
    let _guard = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let (api_base, request_rx) = spawn_single_response_models_server(
        r#"{
            "object": "list",
            "data": [
                {"id": "catalog-model", "object": "model", "context_length": 8192}
            ]
        }"#,
    );
    let mut overrides = HeaderMap::new();
    overrides.insert(
        reqwest::header::USER_AGENT,
        reqwest::header::HeaderValue::from_static("corp-agent/9.9"),
    );
    overrides.insert(
        reqwest::header::HeaderName::from_static("x-tenant-id"),
        reqwest::header::HeaderValue::from_static("tenant-42"),
    );
    let provider = OpenRouterProvider {
        api_base,
        model: Arc::new(RwLock::new("catalog-model".to_string())),
        auth: Arc::new(|| {
            Ok(ProviderAuth::AuthorizationBearer {
                token: "sk-catalog".to_string(),
                label: "OPENAI_COMPAT_API_KEY".to_string(),
            })
        }),
        supports_provider_features: false,
        supports_model_catalog: true,
        profile_id: None,
        http_header_overrides: overrides,
        ..make_custom_compatible_provider()
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let fetched = rt
        .block_on(provider.refresh_models())
        .expect("refresh fake model catalog");
    assert_eq!(fetched[0].id, "catalog-model");

    let request = request_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("capture fake provider request");
    let lines: Vec<&str> = request
        .lines()
        .take_while(|line| !line.is_empty())
        .collect();
    let user_agents: Vec<&str> = lines
        .iter()
        .filter(|line| line.to_ascii_lowercase().starts_with("user-agent:"))
        .map(|line| {
            line.split_once(':')
                .map(|(_, value)| value.trim())
                .unwrap_or("")
        })
        .collect();
    assert_eq!(
        user_agents,
        vec!["corp-agent/9.9"],
        "catalog request must carry exactly the configured User-Agent:\n{request}"
    );
    assert!(
        lines.contains(&"x-tenant-id: tenant-42"),
        "catalog request must carry the custom header:\n{request}"
    );
}

#[test]
fn named_openai_compatible_provider_merges_global_and_profile_header_overrides() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let _key = EnvVarGuard::set("TEST_NAMED_COMPAT_KEY", "test-key");
    let home = tempfile::tempdir().expect("scratch JCODE_HOME");
    let _jcode_home = EnvVarGuard::set("JCODE_HOME", home.path());
    jcode_base::config::Config::invalidate_cache();
    std::fs::write(
        home.path().join("config.toml"),
        r#"
        [provider]
        user_agent = "global-agent/1.0"

        [provider.headers]
        x-tenant-id = "global-tenant"
        x-route = "global-route"
        "#,
    )
    .expect("write scratch config");

    let mut profile = jcode_base::config::NamedProviderConfig {
        base_url: "https://llm.example.com/v1".to_string(),
        api_key_env: Some("TEST_NAMED_COMPAT_KEY".to_string()),
        user_agent: Some("profile-agent/2.0".to_string()),
        ..jcode_base::config::NamedProviderConfig::default()
    };
    profile
        .headers
        .insert("x-route".to_string(), "profile-route".to_string());

    let provider = OpenRouterProvider::new_named_openai_compatible("override-compat", &profile)
        .expect("named profile should initialize");
    let overrides = &provider.http_header_overrides;
    assert_eq!(
        overrides.get("User-Agent").unwrap(),
        "profile-agent/2.0",
        "profile user_agent must beat global"
    );
    assert_eq!(
        overrides.get("x-route").unwrap(),
        "profile-route",
        "profile header must beat the same-named global header"
    );
    assert_eq!(
        overrides.get("x-tenant-id").unwrap(),
        "global-tenant",
        "global-only header must survive"
    );

    jcode_base::config::Config::invalidate_cache();
}

#[test]
fn config_header_overrides_apply_to_endpoint_requests() {
    let _lock = ENV_LOCK.lock();
    let home = tempfile::tempdir().expect("scratch JCODE_HOME");
    let _jcode_home = EnvVarGuard::set("JCODE_HOME", home.path());
    let _guard = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind endpoints server");
    let addr = listener.local_addr().expect("endpoints server addr");
    let (request_tx, request_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept endpoints request");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set read timeout");
        let mut request = vec![0u8; 8192];
        let n = stream.read(&mut request).unwrap_or(0);
        let _ = request_tx.send(String::from_utf8_lossy(&request[..n]).into_owned());
        let body = r#"{"data":{"endpoints":[]}}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = stream.write_all(response.as_bytes());
    });
    let api_base = format!("http://127.0.0.1:{}/v1", addr.port());
    let mut overrides = HeaderMap::new();
    overrides.insert(
        reqwest::header::USER_AGENT,
        reqwest::header::HeaderValue::from_static("corp-agent/9.9"),
    );
    overrides.insert(
        reqwest::header::HeaderName::from_static("x-tenant-id"),
        reqwest::header::HeaderValue::from_static("tenant-42"),
    );
    let provider = OpenRouterProvider {
        api_base,
        supports_provider_features: true,
        supports_model_catalog: true,
        http_header_overrides: overrides,
        ..make_custom_compatible_provider()
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    // The endpoints disk cache keys off the real ~/.jcode regardless of
    // JCODE_HOME, so a shared model id like "some/model" can resolve to a
    // file another run wrote and skip the HTTP request entirely. The
    // listener port makes this run's id unique.
    let model = format!("test-model-{}/headers-override", addr.port());
    let endpoints = rt
        .block_on(provider.fetch_endpoints(&model))
        .expect("fetch fake endpoints");
    assert!(endpoints.is_empty());

    let request = request_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("capture endpoints request");
    let lines: Vec<&str> = request
        .lines()
        .take_while(|line| !line.is_empty())
        .collect();
    let user_agents: Vec<&str> = lines
        .iter()
        .filter(|line| line.to_ascii_lowercase().starts_with("user-agent:"))
        .map(|line| {
            line.split_once(':')
                .map(|(_, value)| value.trim())
                .unwrap_or("")
        })
        .collect();
    assert_eq!(
        user_agents,
        vec!["corp-agent/9.9"],
        "endpoints request must carry exactly the configured User-Agent:\n{request}"
    );
    assert!(
        lines.contains(&"x-tenant-id: tenant-42"),
        "endpoints request must carry the custom header:\n{request}"
    );
}

#[test]
fn invalid_global_header_overrides_fail_provider_construction_with_labeled_error() {
    let _lock = ENV_LOCK.lock();
    let home = tempfile::tempdir().expect("scratch JCODE_HOME");
    let _jcode_home = EnvVarGuard::set("JCODE_HOME", home.path());
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    jcode_base::config::Config::invalidate_cache();
    std::fs::write(
        home.path().join("config.toml"),
        r#"
        [provider.headers]
        "bad header" = "x"
        "#,
    )
    .expect("write scratch config");

    let message = OpenRouterProvider::new_named_openai_compatible(
        "invalid-overrides",
        &jcode_base::config::NamedProviderConfig {
            base_url: "https://llm.example.com/v1".to_string(),
            auth: jcode_base::config::NamedProviderAuth::None,
            ..Default::default()
        },
    )
    .err()
    .map(|error| format!("{error:#}"))
    .expect("invalid global header must fail construction");
    assert!(
        message.contains("has invalid header overrides"),
        "error must explain the invalid header:\n{message}"
    );

    jcode_base::config::Config::invalidate_cache();
}

#[test]
fn invalid_profile_header_overrides_fail_provider_construction_with_labeled_error() {
    let _lock = ENV_LOCK.lock();
    let home = tempfile::tempdir().expect("scratch JCODE_HOME");
    let _jcode_home = EnvVarGuard::set("JCODE_HOME", home.path());
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    jcode_base::config::Config::invalidate_cache();

    let mut profile = jcode_base::config::NamedProviderConfig {
        base_url: "https://llm.example.com/v1".to_string(),
        auth: jcode_base::config::NamedProviderAuth::None,
        ..jcode_base::config::NamedProviderConfig::default()
    };
    profile
        .headers
        .insert("bad header".to_string(), "x".to_string());

    let message = OpenRouterProvider::new_named_openai_compatible("broken-profile", &profile)
        .err()
        .map(|error| format!("{error:#}"))
        .expect("invalid profile header must fail construction");
    assert!(
        message.contains("broken-profile has invalid header overrides"),
        "error must name the profile:\n{message}"
    );

    jcode_base::config::Config::invalidate_cache();
}
