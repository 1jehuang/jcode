//! Focused plan 01 tests: snapshot import once semantics, whoami persistence
//! gate, OAuth state gate and request parsing, canonical generate request,
//! and the minimal text-delta stream decode.

use crate::auth::{
    CommandCodeAccount, import_command_code_auth_snapshot, oauth_state_matches,
    parse_callback_from_request, persist_verified_account,
};
use crate::{CommandCodeProvider, decode_text_only_stream};
use futures::StreamExt;
use jcode_message_types::{ContentBlock, Message, Role, StreamEvent};
use jcode_provider_command_code::{COMMAND_CODE_VERSION, GENERATE_URL, WhoamiIdentity};
fn valid_identity(id: &str, name: &str) -> WhoamiIdentity {
    serde_json::from_value(serde_json::json!({
        "user": {"id": id, "userName": name, "orgId": "org-1"}
    }))
    .expect("identity fixture")
}

fn user_message(text: &str) -> Message {
    Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: text.to_string(),
            cache_control: None,
        }],
        timestamp: None,
        tool_duration_ms: None,
    }
}

#[test]
fn command_code_generate_request_uses_endpoint_and_stream() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let request = runtime.block_on(async {
        let provider = CommandCodeProvider::new(
            "key-1".to_string(),
            "sess-1".to_string(),
            "zai-org/GLM-5.3".to_string(),
        );
        provider
            .generate_request(&[user_message("hi")], &[], "system")
            .expect("request builder")
            .build()
            .expect("built request")
    });
    assert_eq!(request.url().as_str(), GENERATE_URL);
    assert_eq!(request.method(), reqwest::Method::POST);
    let headers = request.headers();
    assert_eq!(headers.get(reqwest::header::USER_AGENT).unwrap(), "cli");
    assert_eq!(headers.get("x-session-id").unwrap(), "sess-1");
    assert!(headers.get("x-command-code-version").is_some());
    assert_eq!(
        headers.get("x-command-code-version").unwrap(),
        COMMAND_CODE_VERSION
    );
    assert!(headers.get("x-project-slug").is_some());
    let body_bytes = request
        .body()
        .expect("json body")
        .as_bytes()
        .expect("body bytes");
    let body_json: serde_json::Value = serde_json::from_slice(body_bytes).expect("body json");
    assert_eq!(
        body_json
            .get("params")
            .and_then(|params| params.get("stream"))
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    assert_eq!(body_json["params"]["reasoning_effort"], "max");
    assert_eq!(
        body_json["config"]["workingDir"],
        std::env::current_dir().unwrap().display().to_string()
    );
}

#[tokio::test]
async fn command_code_stream_decodes_text_delta_and_error() {
    let lf_byte: u8 = 10;
    let mut chunk_a =
        bytes::Bytes::from_static(b"{\"type\":\"text-delta\",\"text\":\"hel\"}").to_vec();
    chunk_a.push(lf_byte);
    let chunk_a = bytes::Bytes::from(chunk_a);
    let mut chunk_b =
        bytes::Bytes::from_static(b"{\"type\":\"error\",\"message\":\"boom\"}").to_vec();
    chunk_b.push(lf_byte);
    let chunk_b = bytes::Bytes::from(chunk_b);
    let stream = futures::stream::iter(vec![Ok::<_, std::io::Error>(chunk_a), Ok(chunk_b)]);
    let framed = crate::FramedStringStream::new(Box::pin(stream));
    let mut events = decode_text_only_stream(Box::pin(framed)).expect("decoder");
    let mut collected = Vec::new();
    while let Some(event) = events.next().await {
        match event.expect("event ok") {
            StreamEvent::TextDelta(text) => collected.push(text),
            StreamEvent::Error { message, .. } => collected.push(format!("error:{}", message)),
            _ => {}
        }
    }
    assert_eq!(collected, vec!["hel".to_string(), "error:boom".to_string()]);
}

#[test]
fn command_code_import_snapshot_is_once_only() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = dir.path().join("accounts.json");
    let snapshot = dir.path().join("auth.json");
    std::fs::write(&snapshot, "{\"apiKey\":\"key-1\",\"keyName\":\"dev\"}").expect("snapshot");
    // First run: whoami gate rejects, fails closed, and still marks the
    // one-time flag (D-01) so a second call never invokes whoami again.
    let first = import_command_code_auth_snapshot(&store, &snapshot, |_| {
        anyhow::bail!("whoami unreachable offline")
    });
    assert!(first.is_err());
    let second = import_command_code_auth_snapshot(&store, &snapshot, |_| {
        panic!("must not be called again after the one-time flag")
    });
    assert!(matches!(second, Ok(None)));
}

#[test]
fn command_code_persistence_requires_verified_identity() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = dir.path().join("accounts.json");
    let account = CommandCodeAccount {
        label: None,
        api_key: "key-1".to_string(),
        user_id: "u1".to_string(),
        user_name: "u".to_string(),
        org_id: None,
        key_name: None,
    };
    let ok = persist_verified_account(
        &store,
        account.clone(),
        &valid_identity("u1", "user-1"),
        None,
    );
    assert!(ok.is_ok());
    let rejected = persist_verified_account(
        &store,
        account,
        &valid_identity("", ""),
        Some("cmd-1".to_string()),
    );
    assert!(rejected.is_err(), "empty whoami identity must fail closed");
}

#[test]
fn command_code_oauth_state_gate_and_request_parse() {
    assert!(oauth_state_matches("state-1", "state-1"));
    assert!(!oauth_state_matches("state-1", "state-2"));
    assert!(!oauth_state_matches("", "anything"));
    assert!(!oauth_state_matches("short", "state-1"));
    let head = "POST /callback HTTP/1.1\r\nHost: 127.0.0.1:5959\r\nContent-Type: application/json\r\n\r\n{\"apiKey\":\"k x\",\"state\":\"state-1\",\"userId\":\"u1\",\"userName\":\"dev\",\"keyName\":\"n\"}";
    let callback = parse_callback_from_request(head).expect("parsed callback");
    assert_eq!(callback.api_key, "k x");
    assert_eq!(callback.state, "state-1");
    assert_eq!(callback.user_id, "u1");
    assert_eq!(callback.user_name, "dev");
    assert_eq!(callback.key_name.as_deref(), Some("n"));
    assert!(parse_callback_from_request("GET / HTTP/1.1\r\n\r\n").is_none());
    assert!(
        parse_callback_from_request("GET /callback?apiKey=k&state=s&userName=u HTTP/1.1\r\n\r\n")
            .is_none()
    );
    assert!(
        parse_callback_from_request(
            "POST /callback HTTP/1.1\r\nContent-Type: text/plain\r\n\r\n{} "
        )
        .is_none()
    );
    assert!(
        parse_callback_from_request(
            "POST /wrong HTTP/1.1\r\nContent-Type: application/json\r\n\r\n{} "
        )
        .is_none()
    );
}
