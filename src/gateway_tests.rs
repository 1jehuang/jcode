use super::*;
use chrono::{DateTime, Utc};
use tokio_tungstenite::tungstenite::handshake::server::Request;

struct EnvVarGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let previous = std::env::var_os(key);
        crate::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.as_ref() {
            crate::env::set_var(self.key, previous);
        } else {
            crate::env::remove_var(self.key);
        }
    }
}

#[test]
fn test_device_registry_pairing() {
    let mut registry = DeviceRegistry::default();

    // Generate pairing code
    let code = registry.generate_pairing_code();
    assert_eq!(code.len(), 6);
    assert_eq!(registry.pending_codes.len(), 1);

    // Validate correct code
    assert!(registry.validate_code(&code));
    assert_eq!(registry.pending_codes.len(), 0); // consumed

    // Validate again should fail (consumed)
    assert!(!registry.validate_code(&code));
}

#[test]
fn test_device_registry_token_auth() {
    let mut registry = DeviceRegistry::default();

    // Pair a device
    let token = registry.pair_device("test-device-1".to_string(), "Test iPhone".to_string(), None);

    // Validate correct token
    assert!(registry.validate_token(&token).is_some());
    let device = registry.validate_token(&token).unwrap();
    assert_eq!(device.name, "Test iPhone");
    assert_eq!(device.id, "test-device-1");

    // Validate wrong token
    assert!(registry.validate_token("wrong-token").is_none());

    // Token hash should be stored, not raw token
    assert!(registry.devices[0].token_hash.starts_with("sha256:"));
}

#[test]
fn test_device_re_pairing() {
    let mut registry = DeviceRegistry::default();

    // Pair same device twice
    let token1 = registry.pair_device("device-1".to_string(), "iPhone v1".to_string(), None);
    let token2 = registry.pair_device("device-1".to_string(), "iPhone v2".to_string(), None);

    // Only one device entry (old one replaced)
    assert_eq!(registry.devices.len(), 1);
    assert_eq!(registry.devices[0].name, "iPhone v2");

    // Old token should be invalid
    assert!(registry.validate_token(&token1).is_none());
    // New token should be valid
    assert!(registry.validate_token(&token2).is_some());
}

#[test]
fn test_parse_bearer_token() {
    assert_eq!(parse_bearer_token("Bearer abc"), Some("abc"));
    assert_eq!(parse_bearer_token("bearer abc"), Some("abc"));
    assert_eq!(parse_bearer_token("BEARER abc"), Some("abc"));
    assert_eq!(parse_bearer_token("Bearer"), None);
    assert_eq!(parse_bearer_token("Basic abc"), None);
    assert_eq!(parse_bearer_token("Bearer abc def"), None);
}

#[test]
fn test_parse_query_token() {
    assert_eq!(parse_query_token("token=abc"), Some("abc"));
    assert_eq!(parse_query_token("foo=bar&token=abc123"), Some("abc123"));
    assert_eq!(parse_query_token("token="), None);
    assert_eq!(parse_query_token("foo=bar"), None);
}

#[test]
fn test_hex_token_validation() {
    assert!(is_valid_hex_token(
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    ));
    assert!(!is_valid_hex_token("abc"));
    assert!(!is_valid_hex_token(
        "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"
    ));
}

#[test]
fn test_extract_ws_auth_prefers_header_and_falls_back_to_query() {
    let token_a = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let token_b = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

    let header_request = Request::builder()
        .uri("ws://example.com/ws")
        .header("authorization", format!("Bearer {token_a}"))
        .body(())
        .expect("request");
    let header_auth = extract_ws_auth(&header_request).expect("header auth");
    assert_eq!(header_auth.token, token_a);
    assert_eq!(header_auth.source, WsAuthSource::Header);

    let query_request = Request::builder()
        .uri(format!("ws://example.com/ws?token={token_b}"))
        .body(())
        .expect("request");
    let query_auth = extract_ws_auth(&query_request).expect("query auth");
    assert_eq!(query_auth.token, token_b);
    assert_eq!(query_auth.source, WsAuthSource::Query);
}

#[test]
fn test_extract_ws_auth_rejects_conflicting_sources() {
    let token_a = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let token_b = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

    let request = Request::builder()
        .uri(format!("ws://example.com/ws?token={token_b}"))
        .header("authorization", format!("Bearer {token_a}"))
        .body(())
        .expect("request");
    assert!(extract_ws_auth(&request).is_err());
}

#[test]
fn test_route_harness_event_sse_run_id_decodes_path() {
    assert_eq!(
        route_harness_event_sse_run_id("/events/runs/run%20demo/stream").as_deref(),
        Some("run demo")
    );
    assert!(route_harness_event_sse_run_id("/events/runs/run_demo").is_none());
}

#[test]
fn test_extract_http_auth_token_accepts_bearer_and_rejects_missing() {
    let mut headers = std::collections::HashMap::new();
    headers.insert(
        "authorization".to_string(),
        "Bearer 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
    );

    let token = extract_http_auth_token(&headers, None).unwrap();

    assert_eq!(
        token,
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    );
    assert!(extract_http_auth_token(&std::collections::HashMap::new(), None).is_err());
}

#[tokio::test]
async fn test_gateway_http_sse_replays_tail_with_auth() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::Builder::new()
        .prefix("jcode-gateway-sse-")
        .tempdir()
        .unwrap();
    let _home = EnvVarGuard::set("JCODE_HOME", temp.path().join("home"));
    let _runtime = EnvVarGuard::set("JCODE_RUNTIME_DIR", temp.path().join("runtime"));
    std::fs::create_dir_all(crate::storage::jcode_dir().unwrap()).unwrap();

    let mut registry = DeviceRegistry::default();
    let token = registry.pair_device("device-sse".to_string(), "SSE Dashboard".to_string(), None);
    let registry = std::sync::Arc::new(tokio::sync::RwLock::new(registry));

    write_gateway_sse_test_event("hevt_start", "run_gateway_sse", 1);
    write_gateway_sse_test_event("hevt_tool", "run_gateway_sse", 2);
    write_gateway_sse_test_event("hevt_done", "run_gateway_sse", 3);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let registry_for_task = std::sync::Arc::clone(&registry);
    let server = tokio::spawn(async move {
        let (stream, peer_addr) = listener.accept().await.unwrap();
        handle_http(stream, peer_addr, registry_for_task)
            .await
            .unwrap();
    });

    let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
    let request = format!(
        "GET /events/runs/run_gateway_sse/stream?replay=only&retry_ms=1500 HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nLast-Event-ID: hevt_tool\r\n\r\n"
    );
    tokio::io::AsyncWriteExt::write_all(&mut client, request.as_bytes())
        .await
        .unwrap();
    let mut response = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut client, &mut response)
        .await
        .unwrap();
    server.await.unwrap();

    let response = String::from_utf8(response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert!(
        response.contains("Content-Type: text/event-stream"),
        "{response}"
    );
    assert!(response.contains("id: hevt_done"), "{response}");
    assert!(response.contains("retry: 1500"), "{response}");
    assert!(!response.contains("id: hevt_start"), "{response}");
    assert!(!response.contains("id: hevt_tool"), "{response}");
}

#[tokio::test]
async fn test_gateway_http_sse_streams_live_bus_events() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::Builder::new()
        .prefix("jcode-gateway-sse-live-")
        .tempdir()
        .unwrap();
    let _home = EnvVarGuard::set("JCODE_HOME", temp.path().join("home"));
    let _runtime = EnvVarGuard::set("JCODE_RUNTIME_DIR", temp.path().join("runtime"));
    std::fs::create_dir_all(crate::storage::jcode_dir().unwrap()).unwrap();

    let mut registry = DeviceRegistry::default();
    let token = registry.pair_device(
        "device-sse-live".to_string(),
        "SSE Live Dashboard".to_string(),
        None,
    );
    let registry = std::sync::Arc::new(tokio::sync::RwLock::new(registry));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let registry_for_task = std::sync::Arc::clone(&registry);
    let server = tokio::spawn(async move {
        let (stream, peer_addr) = listener.accept().await.unwrap();
        handle_http(stream, peer_addr, registry_for_task)
            .await
            .unwrap();
    });

    let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
    let request = format!(
        "GET /events/runs/run_gateway_sse_live/stream HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\n\r\n"
    );
    tokio::io::AsyncWriteExt::write_all(&mut client, request.as_bytes())
        .await
        .unwrap();

    let mut response = Vec::new();
    read_until_text(&mut client, &mut response, "\r\n\r\n").await;
    crate::harness_events::HarnessEventBus::global().publish(
        crate::harness_events::HarnessEventDraft::run_completed("run_gateway_sse_live"),
    );
    read_until_text(&mut client, &mut response, "event: run_completed").await;

    let response = String::from_utf8(response).unwrap();
    assert!(
        response.contains("Content-Type: text/event-stream"),
        "{response}"
    );
    assert!(response.contains("event: run_completed"), "{response}");
    assert!(
        response.contains("\"run_id\":\"run_gateway_sse_live\""),
        "{response}"
    );
    drop(client);
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn test_gateway_ws_control_text_audits_approval_command() {
    let bus = crate::harness_events::HarnessEventBus::with_capacity(8);
    let mut rx = bus.subscribe();
    let response = handle_harness_control_ws_text(
        r#"{
            "command":"resolve_human_approval",
            "run_id":"run_gateway_control",
            "approval_id":"approval_shell",
            "decision":"approved",
            "actor":"web-ui",
            "reason":"clicked approve",
            "client_command_id":"cmd_gateway_1"
        }"#,
        true,
        &bus,
    )
    .expect("control command should be intercepted");
    let event = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
        .await
        .unwrap()
        .unwrap();
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();

    assert_eq!(
        event.kind,
        crate::harness_events::HarnessEventKind::HumanApprovalResolved
    );
    assert_eq!(event.payload["approval_id"], "approval_shell");
    assert_eq!(event.payload["reason_present"], true);
    assert_eq!(response["type"], "harness_control_ack");
    assert_eq!(response["event"]["kind"], "human_approval_resolved");
    assert_eq!(response["event"]["payload"]["status"], "resolved");
}

#[test]
fn test_gateway_ws_control_text_ignores_regular_protocol_messages() {
    let bus = crate::harness_events::HarnessEventBus::with_capacity(8);
    assert!(
        handle_harness_control_ws_text(
            r#"{"type":"client_input","command":"bash","payload":{"text":"hello"}}"#,
            true,
            &bus,
        )
        .is_none()
    );
    assert!(handle_harness_control_ws_text("not json", true, &bus).is_none());
}

#[tokio::test]
async fn test_gateway_ws_control_text_rejects_unauthorized_write() {
    let bus = crate::harness_events::HarnessEventBus::with_capacity(8);
    let mut rx = bus.subscribe();
    let response = handle_harness_control_ws_text(
        r#"{"command":"cancel_run","run_id":"run_gateway_reject","actor":"web-ui","reason":"stop"}"#,
        false,
        &bus,
    )
    .expect("control command should be intercepted");
    let event = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
        .await
        .unwrap()
        .unwrap();
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();

    assert_eq!(
        event.kind,
        crate::harness_events::HarnessEventKind::ControlCommandRejected
    );
    assert_eq!(event.level, crate::harness_events::HarnessEventLevel::Warn);
    assert_eq!(event.payload["command"], "cancel_run");
    assert_eq!(event.payload["authorized"], false);
    assert_eq!(response["type"], "harness_control_rejected");
    assert_eq!(response["event"]["kind"], "control_command_rejected");
}

async fn read_until_text(client: &mut tokio::net::TcpStream, response: &mut Vec<u8>, needle: &str) {
    let deadline = std::time::Duration::from_secs(2);
    tokio::time::timeout(deadline, async {
        let mut buf = [0u8; 1024];
        loop {
            if String::from_utf8_lossy(response).contains(needle) {
                break;
            }
            let n = tokio::io::AsyncReadExt::read(client, &mut buf)
                .await
                .unwrap();
            assert!(n > 0, "connection closed before finding {needle}");
            response.extend_from_slice(&buf[..n]);
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {needle}"));
}

fn write_gateway_sse_test_event(event_id: &str, run_id: &str, sequence: u64) {
    let event = crate::harness_events::HarnessEvent::new(
        event_id,
        run_id,
        DateTime::parse_from_rfc3339("2026-05-08T04:56:00Z")
            .unwrap()
            .with_timezone(&Utc),
        sequence,
        crate::harness_events::HarnessEventLevel::Info,
        crate::harness_events::HarnessEventKind::ToolFinished,
        serde_json::json!({"status": "ok"}),
    );
    crate::harness_events::append_harness_event_ndjson(
        crate::harness_events::harness_event_log_path(run_id),
        &event,
    )
    .unwrap();
}
