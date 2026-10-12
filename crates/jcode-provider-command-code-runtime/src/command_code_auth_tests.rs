//! Focused plan 01 tests: snapshot import once semantics, whoami persistence
//! gate, OAuth state gate and request parsing, canonical generate request,
//! and the minimal text-delta stream decode.

use crate::auth::{
    CommandCodeAccount, CommandCodeStore, OAuthCallback, import_command_code_auth_snapshot,
    oauth_authorize_url, oauth_state_matches, parse_callback_from_request,
    persist_verified_account, verified_account_from_callback,
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

#[test]
fn command_code_reasoning_recovery_changes_the_retry_body() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let provider =
            CommandCodeProvider::new("key-1".into(), "sess-1".into(), "zai-org/GLM-5.3".into());
        let first = provider
            .generate_request(&[user_message("hi")], &[], "system")
            .unwrap()
            .build()
            .unwrap();
        let first: serde_json::Value =
            serde_json::from_slice(first.body().unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(first["params"]["reasoning_effort"], "max");

        provider.reasoning.classify_pre_stream_rejection(
            "zai-org/GLM-5.3",
            422,
            "reasoning_effort unsupported",
        );
        let retry = provider
            .generate_request(&[user_message("hi")], &[], "system")
            .unwrap()
            .build()
            .unwrap();
        let retry: serde_json::Value =
            serde_json::from_slice(retry.body().unwrap().as_bytes().unwrap()).unwrap();
        assert!(retry["params"].get("reasoning_effort").is_none());
    });
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
fn oauth_account_uses_whoami_identity_not_callback() {
    let account = verified_account_from_callback(
        OAuthCallback {
            api_key: "key-1".into(),
            state: "state-1".into(),
            user_id: "callback-user".into(),
            user_name: "Callback Name".into(),
            key_name: Some("dev".into()),
        },
        &valid_identity("verified-user", "Verified Name"),
    );
    assert_eq!(account.user_id, "verified-user");
    assert_eq!(account.user_name, "Verified Name");
    assert_eq!(account.org_id.as_deref(), Some("org-1"));
    assert_eq!(account.api_key, "key-1");
}

#[test]
fn overlapping_account_persists_keep_both_records() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = dir.path().join("accounts.json");
    let account = |key: &str, user: &str| CommandCodeAccount {
        label: None,
        api_key: key.into(),
        user_id: user.into(),
        user_name: user.into(),
        org_id: None,
        key_name: None,
    };
    let left = store.clone();
    let right = store.clone();
    let first = std::thread::spawn(move || {
        persist_verified_account(
            &left,
            account("key-1", "u1"),
            &valid_identity("u1", "user-1"),
            Some("one".into()),
        )
    });
    let second = std::thread::spawn(move || {
        persist_verified_account(
            &right,
            account("key-2", "u2"),
            &valid_identity("u2", "user-2"),
            Some("two".into()),
        )
    });
    first.join().expect("first persist").expect("saved");
    second.join().expect("second persist").expect("saved");
    let loaded = CommandCodeStore::load(&store);
    let mut labels = loaded
        .accounts
        .iter()
        .filter_map(|account| account.label.clone())
        .collect::<Vec<_>>();
    labels.sort();
    assert_eq!(labels, vec!["one".to_string(), "two".to_string()]);
}

#[test]
fn oauth_authorize_url_targets_studio_cli_callback() {
    let url = oauth_authorize_url("abc");
    assert!(url.starts_with("https://commandcode.ai/studio/auth/cli?"));
    assert!(url.contains("callback=http%3A%2F%2Flocalhost%3A5959%2Fcallback"));
    assert!(url.contains("state=abc"));
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
