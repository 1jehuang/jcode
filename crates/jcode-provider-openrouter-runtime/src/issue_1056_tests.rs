use super::OpenRouterProvider;
use super::tests::{ENV_LOCK, EnvVarGuard, make_custom_compatible_provider};
use futures::StreamExt;
use jcode_message_types::Message;
use jcode_provider_core::Provider;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;
use tempfile::TempDir;

#[test]
fn mistral_models_use_their_configured_reasoning_defaults() {
    // The constructor writes the process-global cache namespace; hold the env
    // lock so parallel tests never observe it, and restore it afterwards.
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let profile: jcode_base::config::NamedProviderConfig = toml::from_str(
        r#"
type = "openai-compatible"
base_url = "https://api.mistral.ai/v1"
auth = "Bearer"
api_key = "test"
disable_reasoning_heuristics = true

[[models]]
id = "mistral-small-latest"
reasoning = true
reasoning_effort = "high"

[[models]]
id = "mistral-medium-latest"
reasoning = true
reasoning_effort = "max"
"#,
    )
    .expect("issue 1056 Mistral profile parses");

    let provider = OpenRouterProvider::new_named_openai_compatible("mistral", &profile)
        .expect("Mistral provider constructs");
    provider.set_model("mistral-small-latest").unwrap();
    assert_eq!(provider.reasoning_effort().as_deref(), Some("high"));
    provider.set_model("mistral-medium-latest").unwrap();
    assert_eq!(provider.reasoning_effort().as_deref(), Some("max"));
}

#[test]
fn mistral_max_effort_is_sent_as_official_xhigh_value() {
    // The constructor writes the process-global cache namespace; hold the env
    // lock so parallel tests never observe it, and restore it afterwards.
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (request_tx, request_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut bytes = vec![0; 32 * 1024];
        let read = stream.read(&mut bytes).unwrap();
        request_tx
            .send(String::from_utf8_lossy(&bytes[..read]).into_owned())
            .unwrap();
        let body = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n\n";
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });

    let profile = jcode_base::config::NamedProviderConfig {
        base_url: format!("http://{address}/v1"),
        api_key: Some("test".to_string()),
        default_model: Some("mistral-medium-latest".to_string()),
        disable_reasoning_heuristics: true,
        models: vec![jcode_base::config::NamedProviderModelConfig {
            id: "mistral-medium-latest".to_string(),
            reasoning: Some(true),
            reasoning_effort: Some("max".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let provider = OpenRouterProvider::new_named_openai_compatible("mistral", &profile).unwrap();
    let messages = vec![Message::user("hello")];

    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let mut stream = provider.complete(&messages, &[], "", None).await.unwrap();
        while let Some(event) = stream.next().await {
            event.unwrap();
        }
    });

    let request = request_rx.recv().unwrap();
    assert!(request.contains(r#""reasoning_effort":"xhigh""#));
    assert!(!request.contains(r#""reasoning_effort":"max""#));
}

/// A named profile's `headers` table (for example a gateway metadata header)
/// is sent on the wire, including by forked providers; invalid entries are
/// skipped instead of disabling the provider.
#[test]
fn named_profile_headers_are_sent_on_the_wire() {
    // The constructor writes the process-global cache namespace; hold the env
    // lock so parallel tests never observe it, and restore it afterwards.
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (request_tx, request_rx) = mpsc::channel();
    std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            request_tx.send(read_request_head(&mut stream)).unwrap();
            let body = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n\n";
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        }
    });

    let mut headers = std::collections::BTreeMap::new();
    headers.insert(
        "cf-aig-metadata".to_string(),
        r#"{"agent_id":"jcode"}"#.to_string(),
    );
    headers.insert("bad header".to_string(), "x".to_string());
    let profile = jcode_base::config::NamedProviderConfig {
        base_url: format!("http://{address}/v1"),
        api_key: Some("test".to_string()),
        default_model: Some("m".to_string()),
        headers,
        ..Default::default()
    };
    let provider = OpenRouterProvider::new_named_openai_compatible("gateway", &profile).unwrap();
    let forked = provider.fork();
    let messages = vec![Message::user("hello")];

    let runtime = tokio::runtime::Runtime::new().unwrap();
    for p in [&provider as &dyn Provider, forked.as_ref()] {
        runtime.block_on(async {
            let mut stream = p.complete(&messages, &[], "", None).await.unwrap();
            while let Some(event) = stream.next().await {
                event.unwrap();
            }
        });
        let request = request_rx.recv().unwrap().to_ascii_lowercase();
        assert!(
            request.contains(r#"cf-aig-metadata: {"agent_id":"jcode"}"#),
            "missing profile header:\n{request}"
        );
        assert!(!request.contains("bad header"));
    }
}

/// Model discovery requests (`/models` and `/models/{id}/endpoints`) carry the
/// named profile's `headers` table, including for endpoints lookups.
#[test]
fn profile_headers_are_sent_on_model_discovery_requests() {
    let _lock = ENV_LOCK.lock();
    let temp = TempDir::new().expect("create temp home");
    let _home = EnvVarGuard::set("HOME", temp.path());
    let _appdata = EnvVarGuard::set("APPDATA", temp.path().join("AppData").join("Roaming"));
    let _namespace = EnvVarGuard::set(
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "test-profile-header-discovery",
    );
    let header = (
        reqwest::header::HeaderName::from_static("cf-aig-metadata"),
        reqwest::header::HeaderValue::from_static("gateway-profile"),
    );
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");

    let (api_base, request_rx) =
        spawn_single_response_models_server(r#"{"data":[{"id":"gateway-model"}]}"#);
    let provider = OpenRouterProvider {
        api_base,
        supports_model_catalog: true,
        extra_headers: Arc::new(vec![header.clone()]),
        ..make_custom_compatible_provider()
    };
    rt.block_on(provider.refresh_models())
        .expect("refresh fake model catalog");
    let request = request_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("capture catalog request")
        .to_ascii_lowercase();
    assert!(request.starts_with("get /v1/models "), "{request}");
    assert!(
        request.contains("cf-aig-metadata: gateway-profile"),
        "catalog request should carry profile headers: {request}"
    );

    let (api_base, request_rx) =
        spawn_single_response_models_server(r#"{"data":{"endpoints":[]}}"#);
    let provider = OpenRouterProvider {
        api_base,
        supports_provider_features: true,
        supports_model_catalog: true,
        extra_headers: Arc::new(vec![header]),
        ..make_custom_compatible_provider()
    };
    rt.block_on(provider.refresh_endpoints("vendor/gateway-model"))
        .expect("refresh fake endpoints");
    let request = request_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("capture endpoints request")
        .to_ascii_lowercase();
    assert!(
        request.starts_with("get /v1/models/vendor/gateway-model/endpoints "),
        "{request}"
    );
    assert!(
        request.contains("cf-aig-metadata: gateway-profile"),
        "endpoint request should carry profile headers: {request}"
    );
}

/// Read a request through the end of its headers, however many TCP reads the
/// client needs to deliver them.
fn read_request_head(stream: &mut std::net::TcpStream) -> String {
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut chunk).unwrap();
        assert!(
            read > 0,
            "connection closed before the request headers ended"
        );
        bytes.extend_from_slice(&chunk[..read]);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

pub(crate) fn spawn_single_response_models_server(
    body: &'static str,
) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake provider server");
    let addr = listener.local_addr().expect("fake provider addr");
    let (request_tx, request_rx) = mpsc::channel();

    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept fake provider request");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set read timeout");
        let mut request = vec![0u8; 8192];
        let n = stream.read(&mut request).unwrap_or(0);
        let request = String::from_utf8_lossy(&request[..n]).into_owned();
        let _ = request_tx.send(request);

        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .expect("write fake provider response");
    });

    (format!("http://{addr}/v1"), request_rx)
}
