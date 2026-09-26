use super::OpenRouterProvider;
use futures::StreamExt;
use jcode_message_types::Message;
use jcode_provider_core::Provider;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;

#[test]
fn mistral_models_use_their_configured_reasoning_defaults() {
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
