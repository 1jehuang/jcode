//! Quota-free integration tests against a real `muse serve` host.
//!
//! These drive the actual `muse` binary, but with `--provider echo`, which is a
//! stub that answers locally. **No model call is made and no subscription usage
//! is spent**, which is what makes it safe to run this in CI or on a developer
//! machine without a Muse Code subscription.
//!
//! The tests skip (rather than fail) when `muse` is not installed, so the crate
//! stays buildable and testable without the dependency present.

use jcode_provider_muse_code_runtime::{
    MSP_CLIENT_NAME, MSP_CLIENT_VERSION, MspInbound, MspTransport, MuseHostConfig, new_command_id,
};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;

/// Whether a `muse` binary is on `PATH`.
fn muse_available() -> bool {
    std::process::Command::new("muse")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

macro_rules! require_muse {
    () => {
        if !muse_available() {
            eprintln!("skipping: no `muse` binary on PATH");
            return;
        }
    };
}

async fn connected() -> (MspTransport, Value) {
    let transport = MspTransport::spawn(&MuseHostConfig::echo()).expect("spawn muse serve");
    let init = transport
        .initialize(MSP_CLIENT_NAME, MSP_CLIENT_VERSION)
        .await
        .expect("MSP handshake should succeed against a real host");
    (transport, init)
}

/// Collect inbound messages for a bounded window, returning their method names.
async fn drain_methods(
    rx: &mut tokio::sync::broadcast::Receiver<MspInbound>,
    window: Duration,
) -> Vec<String> {
    let mut methods = Vec::new();
    let deadline = tokio::time::Instant::now() + window;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(inbound)) => {
                if let Some(method) = inbound.method() {
                    methods.push(method.to_string());
                }
            }
            Ok(Err(RecvError::Lagged(_))) => continue,
            Ok(Err(RecvError::Closed)) => break,
            Err(_) => break,
        }
    }
    methods
}

#[tokio::test]
async fn handshake_reports_server_identity_and_schema_fingerprint() {
    require_muse!();
    let (mut transport, init) = connected().await;

    assert_eq!(init["serverInfo"]["name"], json!("muse"));
    assert!(
        init["serverInfo"]["version"].is_string(),
        "serverInfo.version should be reported, got {init:#?}"
    );
    // The host publishes the wire-schema fingerprint at handshake time, which is
    // what lets a client fail loudly on drift instead of misbehaving silently.
    assert_eq!(init["schema"]["version"], json!(1));
    let fingerprint = init["schema"]["fingerprint"]
        .as_str()
        .expect("schema fingerprint must be a string");
    assert!(
        fingerprint.starts_with("sha256:"),
        "unexpected fingerprint shape: {fingerprint}"
    );
    assert!(init["museHome"].is_string());

    transport.shutdown();
}

#[tokio::test]
async fn methods_before_the_initialized_notification_are_rejected() {
    require_muse!();
    // Deliberately bypass MspTransport::initialize so the `initialized`
    // notification is never sent.
    let transport = MspTransport::spawn(&MuseHostConfig::echo()).expect("spawn muse serve");
    transport
        .request(
            "initialize",
            json!({"clientInfo": {"name": MSP_CLIENT_NAME, "version": "0.1.0"}}),
        )
        .await
        .expect("initialize itself should succeed");

    let err = transport
        .request("model/list", json!({}))
        .await
        .expect_err("model/list must be rejected before `initialized`");
    let text = format!("{err:#}");
    assert!(
        text.contains("notInitialized") || text.contains("-32600"),
        "expected a notInitialized rejection, got: {text}"
    );
}

#[tokio::test]
async fn echo_turn_streams_deltas_and_completes_without_spending_quota() {
    require_muse!();
    let (transport, _init) = connected().await;
    let mut events = transport.subscribe();

    let session = transport
        .request("session/start", json!({"commandId": new_command_id()}))
        .await
        .expect("session/start should succeed");
    let session_id = session["session"]["sessionId"]
        .as_str()
        .expect("session/start must return sessionId")
        .to_string();

    let command_id = new_command_id();
    let ack = transport
        .request(
            "turn/start",
            json!({
                "sessionId": session_id,
                "commandId": command_id,
                "input": [{"type": "text", "text": "say hello"}],
                "displayText": "say hello",
            }),
        )
        .await
        .expect("turn/start should succeed");

    assert_eq!(ack["status"], json!("accepted"));
    // `turnId` is derived from the client-minted commandId, so a client can
    // correlate its own turn without a separate lookup.
    assert_eq!(ack["turnId"], json!(command_id));

    let methods = drain_methods(&mut events, Duration::from_secs(25)).await;
    for expected in ["turn/started", "item/delta", "turn/completed"] {
        assert!(
            methods.iter().any(|m| m == expected),
            "expected `{expected}` in the turn stream; saw {methods:?}"
        );
    }
}

#[tokio::test]
async fn model_list_and_usage_are_readable() {
    require_muse!();
    let (transport, _init) = connected().await;

    let models = transport
        .request("model/list", json!({}))
        .await
        .expect("model/list should succeed");
    assert_eq!(models["providerId"], json!("meta"));
    assert!(
        models["models"].is_array(),
        "model/list should return a models array, got {models:#?}"
    );

    // `usage/read` reports truthful absence (an empty object) until the host has
    // itself observed usage. It must not error in a fresh ephemeral host.
    let usage = transport
        .request("usage/read", json!({}))
        .await
        .expect("usage/read should succeed even with nothing observed");
    assert!(usage.is_object());
}

#[tokio::test]
async fn mutations_require_a_command_id() {
    require_muse!();
    let (transport, _init) = connected().await;

    let session = transport
        .request("session/start", json!({"commandId": new_command_id()}))
        .await
        .expect("session/start should succeed");
    let session_id = session["session"]["sessionId"]
        .as_str()
        .unwrap()
        .to_string();

    // Omitting the idempotency handle must be rejected, and rejected *loudly*:
    // MSP answers with invalidParams rather than silently minting one.
    let err = transport
        .request(
            "session/setReasoningEffort",
            json!({"sessionId": session_id, "reasoningEffort": "high"}),
        )
        .await
        .expect_err("mutations without a commandId must be rejected");
    let text = format!("{err:#}");
    assert!(
        text.contains("commandId") || text.contains("invalidParams"),
        "expected a commandId rejection, got: {text}"
    );

    // With the handle, and using the runtime field name (`reasoningEffort`, not
    // the schema's `effort`), the same call is accepted.
    let ack = transport
        .request(
            "session/setReasoningEffort",
            json!({
                "sessionId": session_id,
                "commandId": new_command_id(),
                "reasoningEffort": "high",
            }),
        )
        .await
        .expect("session/setReasoningEffort should succeed with a commandId");
    assert_eq!(ack["status"], json!("accepted"));
}
