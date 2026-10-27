use super::*;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

struct StubServer {
    base_url: String,
    bodies: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    handle: tokio::task::JoinHandle<()>,
}

impl StubServer {
    async fn shutdown(self) {
        self.handle.abort();
        let _ = &self.base_url;
        let _ = &self.bodies;
    }
}

async fn read_http_body(stream: &mut tokio::net::TcpStream) -> String {
    let mut reader = BufReader::new(stream);
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        if let Some((key, value)) = trimmed.split_once(':')
            && key.trim().eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body).await.unwrap();
    }
    String::from_utf8(body).unwrap()
}

/// Serve `responses` in order on loopback, capturing request bodies.
async fn start_stub(responses: Vec<(u16, String)>) -> StubServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let bodies = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = bodies.clone();
    let handle = tokio::spawn(async move {
        for (status, body) in responses {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let request_body = read_http_body(&mut stream).await;
            captured.lock().unwrap().push(request_body);
            let response = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });
    StubServer {
        base_url,
        bodies,
        handle,
    }
}

fn last_body(server: &StubServer) -> String {
    server
        .bodies
        .lock()
        .unwrap()
        .last()
        .cloned()
        .unwrap_or_default()
}

fn sandbox_without_opt_in() -> (crate::auth::test_sandbox::AuthTestSandbox, Option<String>) {
    let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");
    let previous = std::env::var(EXPERIMENTAL_ENV_VAR).ok();
    crate::env::remove_var(EXPERIMENTAL_ENV_VAR);
    (sandbox, previous)
}

fn use_stub(server: &StubServer) -> TestEndpointGuard {
    set_test_endpoints(&server.base_url, &server.base_url)
}

fn restore_opt_in(previous: Option<String>) {
    if let Some(value) = previous {
        crate::env::set_var(EXPERIMENTAL_ENV_VAR, value);
    } else {
        crate::env::remove_var(EXPERIMENTAL_ENV_VAR);
    }
}

fn device_json(device_code: &str, verification_uri: &str) -> String {
    serde_json::json!({
        "device_code": device_code,
        "user_code": "ABCD-1234",
        "verification_uri": verification_uri,
        "expires_in": 900,
        "interval": 5,
    })
    .to_string()
}

fn mint_json(api_key: &str, subs_active: Option<bool>, require_payment: Option<bool>) -> String {
    let mut value = serde_json::json!({
        "api_key": api_key,
        "base_url": EXPECTED_API_BASE_URL,
        "user_email": "user@example.com",
    });
    if let Some(active) = subs_active {
        value["is_subs_active"] = serde_json::json!(active);
    }
    if let Some(required) = require_payment {
        value["require_payment"] = serde_json::json!(required);
    }
    value.to_string()
}

fn challenge_for(base_url: &str) -> DeviceChallenge {
    DeviceChallenge {
        device_code: "dev-code-1".to_string(),
        user_code: "ABCD-1234".to_string(),
        verification_uri: format!("{base_url}/verify"),
        verification_uri_complete: None,
        expires_in: 900,
        interval: 5,
    }
}

#[tokio::test]
async fn begin_login_parses_loopback_challenge() {
    let (_sandbox, previous) = sandbox_without_opt_in();
    let server = start_stub(vec![(
        200,
        device_json("dev-code-1", "https://auth.meta.com/device"),
    )])
    .await;
    let _endpoints = use_stub(&server);
    let client = http_client().unwrap();

    let challenge = begin_login(&client).await.expect("begin login");
    assert_eq!(challenge.device_code, "dev-code-1");
    assert_eq!(challenge.user_code, "ABCD-1234");
    assert_eq!(challenge.expires_in, 900);
    assert_eq!(challenge.interval, 5);
    assert!(last_body(&server).contains("client_id=1031625952748946"));

    server.shutdown().await;
    restore_opt_in(previous);
}

#[tokio::test]
async fn begin_login_wrong_type_body_leaks_no_values() {
    let (_sandbox, previous) = sandbox_without_opt_in();
    // Well-typed strings where numbers belong: serde's error would name
    // the offending value; assert it never surfaces.
    let body = r#"{"device_code":"dev-x","user_code":"U","verification_uri":"https://auth.meta.com/device","expires_in":"SECRET-NUMBER","interval":"SECRET-INTERVAL"}"#.to_string();
    let server = start_stub(vec![(200, body)]).await;
    let _endpoints = use_stub(&server);
    let client = http_client().unwrap();

    let err = begin_login(&client)
        .await
        .expect_err("wrong types must fail");
    let message = format!("{err:#}");
    assert!(message.contains("expected shape"), "{message}");
    assert!(!message.contains("SECRET-NUMBER"), "{message}");
    assert!(!message.contains("SECRET-INTERVAL"), "{message}");

    server.shutdown().await;
    restore_opt_in(previous);
}

#[tokio::test]
async fn begin_login_clamps_numbers_like_launcher() {
    let (_sandbox, previous) = sandbox_without_opt_in();
    let body = serde_json::json!({
        "device_code": "dev-code-2",
        "user_code": "WXYZ-9999",
        "verification_uri": "https://auth.meta.com/device",
        "expires_in": 5,
        "interval": 0,
    })
    .to_string();
    let server = start_stub(vec![(200, body)]).await;
    let _endpoints = use_stub(&server);
    let client = http_client().unwrap();

    let challenge = begin_login(&client).await.expect("begin login");
    assert_eq!(challenge.expires_in, 60);
    assert_eq!(challenge.interval, 5);

    server.shutdown().await;
    restore_opt_in(previous);
}

#[tokio::test]
async fn begin_login_rejects_non_first_party_verification_url() {
    let (_sandbox, previous) = sandbox_without_opt_in();
    let server = start_stub(vec![(
        200,
        device_json("dev-code-3", "http://evil.example/device"),
    )])
    .await;
    let _endpoints = use_stub(&server);
    let client = http_client().unwrap();

    let err = begin_login(&client)
        .await
        .expect_err("must reject evil URL");
    let message = format!("{err:#}");
    assert!(message.contains("first-party"), "{message}");
    assert!(!message.contains("evil.example"), "{message}");

    server.shutdown().await;
    restore_opt_in(previous);
}

#[tokio::test]
async fn begin_login_rejects_missing_codes() {
    let (_sandbox, previous) = sandbox_without_opt_in();
    let body = serde_json::json!({
        "device_code": "",
        "user_code": "",
        "verification_uri": "https://auth.meta.com/device",
    })
    .to_string();
    let server = start_stub(vec![(200, body)]).await;
    let _endpoints = use_stub(&server);
    let client = http_client().unwrap();

    begin_login(&client)
        .await
        .expect_err("must reject empty codes");

    server.shutdown().await;
    restore_opt_in(previous);
}

#[tokio::test]
async fn native_route_does_not_require_legacy_opt_in() {
    // GA: no opt-in variable needed. The legacy variable is accepted silently
    // whether set or not.
    let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");
    let previous = std::env::var(EXPERIMENTAL_ENV_VAR).ok();
    crate::env::remove_var(EXPERIMENTAL_ENV_VAR);
    assert!(ensure_enabled().is_ok(), "GA must not require opt-in");

    crate::env::set_var(EXPERIMENTAL_ENV_VAR, "1");
    assert!(ensure_enabled().is_ok(), "legacy opt-in still accepted");
    crate::env::set_var(EXPERIMENTAL_ENV_VAR, "0");
    assert!(ensure_enabled().is_ok(), "legacy opt-out value ignored");

    restore_opt_in(previous);
}

#[tokio::test]
async fn poll_pending_and_slowdown() {
    let (_sandbox, previous) = sandbox_without_opt_in();
    let server = start_stub(vec![
        (400, r#"{"error":"authorization_pending"}"#.to_string()),
        (400, r#"{"error":"slow_down"}"#.to_string()),
    ])
    .await;
    let _endpoints = use_stub(&server);
    let client = http_client().unwrap();
    let challenge = challenge_for(&server.base_url);

    assert!(matches!(
        poll_login(&client, &challenge).await.expect("poll"),
        PollOutcome::Pending
    ));
    match poll_login(&client, &challenge).await.expect("poll") {
        PollOutcome::SlowDown { retry_after } => assert_eq!(retry_after, 10),
        other => panic!("expected slowdown, got {other:?}"),
    }

    server.shutdown().await;
    restore_opt_in(previous);
}

#[tokio::test]
async fn poll_denied_and_expired() {
    let (_sandbox, previous) = sandbox_without_opt_in();
    let server = start_stub(vec![
        (400, r#"{"error":"access_denied"}"#.to_string()),
        (400, r#"{"error":"expired_token"}"#.to_string()),
    ])
    .await;
    let _endpoints = use_stub(&server);
    let client = http_client().unwrap();
    let challenge = challenge_for(&server.base_url);

    assert!(matches!(
        poll_login(&client, &challenge).await.expect("poll"),
        PollOutcome::Denied
    ));
    assert!(matches!(
        poll_login(&client, &challenge).await.expect("poll"),
        PollOutcome::Expired
    ));

    server.shutdown().await;
    restore_opt_in(previous);
}

#[tokio::test]
async fn poll_http_failure_reports_status_without_body() {
    let (_sandbox, previous) = sandbox_without_opt_in();
    let server = start_stub(vec![(
        500,
        r#"{"error":"server_error","detail":"SECRET-BODY-MUST-NOT-LEAK"}"#.to_string(),
    )])
    .await;
    let _endpoints = use_stub(&server);
    let client = http_client().unwrap();
    let challenge = challenge_for(&server.base_url);

    let err = poll_login(&client, &challenge)
        .await
        .expect_err("must fail");
    let message = format!("{err:#}");
    assert!(message.contains("500"), "{message}");
    assert!(!message.contains("SECRET-BODY-MUST-NOT-LEAK"), "{message}");

    server.shutdown().await;
    restore_opt_in(previous);
}

#[tokio::test]
async fn poll_approval_mints_without_persisting() {
    let (_sandbox, previous) = sandbox_without_opt_in();
    let server = start_stub(vec![
        (200, r#"{"access_token":"dca-token-1"}"#.to_string()),
        (200, mint_json("LLM_TEST_KEY_1", Some(true), Some(false))),
    ])
    .await;
    let _endpoints = use_stub(&server);
    let client = http_client().unwrap();
    let challenge = challenge_for(&server.base_url);

    // Pure poll: nothing persisted; the caller saves explicitly after its
    // own cancellation checks. Save here to prove the returned
    // credentials are complete and round-trip.
    assert!(!credentials_path().unwrap().exists());
    match poll_login(&client, &challenge).await.expect("poll") {
        PollOutcome::Approved(credentials) => {
            assert_eq!(credentials.api_key, "LLM_TEST_KEY_1");
            assert_eq!(credentials.base_url, EXPECTED_API_BASE_URL);
            save_credentials(&credentials).expect("explicit save");
        }
        other => panic!("expected approval, got {other:?}"),
    }
    let stored = load_credentials().expect("stored credentials");
    assert_eq!(stored.api_key, "LLM_TEST_KEY_1");

    server.shutdown().await;
    restore_opt_in(previous);
}

#[tokio::test]
async fn poll_rejects_unexpected_mint_base_url() {
    let (_sandbox, previous) = sandbox_without_opt_in();
    let mut mint = serde_json::json!({
        "api_key": "LLM_TEST_KEY_EVIL",
        "base_url": "https://evil.example/v1",
        "is_subs_active": true,
    })
    .to_string();
    // silence unused-mut warning path clarity
    mint.push_str("");
    let server = start_stub(vec![
        (200, r#"{"access_token":"dca-token-2"}"#.to_string()),
        (200, mint),
    ])
    .await;
    let _endpoints = use_stub(&server);
    let client = http_client().unwrap();
    let challenge = challenge_for(&server.base_url);

    let err = poll_login(&client, &challenge)
        .await
        .expect_err("must fail");
    assert!(
        format!("{err:#}").contains("unexpected base URL"),
        "{err:?}"
    );
    assert!(!credentials_path().unwrap().exists());

    server.shutdown().await;
    restore_opt_in(previous);
}

#[tokio::test]
async fn poll_fails_closed_without_active_subscription() {
    let (_sandbox, previous) = sandbox_without_opt_in();
    let server = start_stub(vec![
        (200, r#"{"access_token":"dca-token-3"}"#.to_string()),
        (200, mint_json("LLM_TEST_KEY_3", Some(false), Some(false))),
    ])
    .await;
    let _endpoints = use_stub(&server);
    let client = http_client().unwrap();
    let challenge = challenge_for(&server.base_url);

    let err = poll_login(&client, &challenge)
        .await
        .expect_err("must fail closed");
    assert!(format!("{err:#}").contains("not active"), "{err:?}");
    assert!(!credentials_path().unwrap().exists());

    server.shutdown().await;
    restore_opt_in(previous);
}

#[tokio::test]
async fn poll_fails_closed_when_payment_unverified() {
    let (_sandbox, previous) = sandbox_without_opt_in();
    // require_payment absent: must fail closed, not silently pass.
    let mint = serde_json::json!({
        "api_key": "LLM_TEST_KEY_4",
        "base_url": EXPECTED_API_BASE_URL,
        "is_subs_active": true,
    })
    .to_string();
    let server = start_stub(vec![
        (200, r#"{"access_token":"dca-token-4"}"#.to_string()),
        (200, mint),
    ])
    .await;
    let _endpoints = use_stub(&server);
    let client = http_client().unwrap();
    let challenge = challenge_for(&server.base_url);

    let err = poll_login(&client, &challenge)
        .await
        .expect_err("must fail closed");
    assert!(
        format!("{err:#}").contains("payment status unverified"),
        "{err:?}"
    );
    assert!(!credentials_path().unwrap().exists());

    server.shutdown().await;
    restore_opt_in(previous);
}

#[tokio::test]
async fn mint_revoked_login_fails_closed() {
    let (_sandbox, previous) = sandbox_without_opt_in();
    let server = start_stub(vec![(401, r#"{"error":"invalid_token"}"#.to_string())]).await;
    let _endpoints = use_stub(&server);
    let client = http_client().unwrap();

    let err = mint_credentials(&client, "dca-revoked")
        .await
        .expect_err("must fail");
    assert!(format!("{err:#}").contains("no longer valid"), "{err:?}");

    server.shutdown().await;
    restore_opt_in(previous);
}

#[tokio::test]
async fn mint_rejects_empty_token_without_network() {
    let (_sandbox, previous) = sandbox_without_opt_in();
    let client = http_client().unwrap();
    mint_credentials(&client, "   ")
        .await
        .expect_err("empty token must fail");
    restore_opt_in(previous);
}

#[test]
fn credentials_debug_redacts_secrets() {
    let credentials = MuseCredentials {
        api_key: "LLM_SUPER_SECRET_KEY".to_string(),
        access_token: "dca-super-secret-token".to_string(),
        base_url: EXPECTED_API_BASE_URL.to_string(),
        user_email: Some("user@example.com".to_string()),
        user_full_name: None,
        is_subs_active: true,
        minted_at_ms: 1,
        subs_usage: None,
    };
    let rendered = format!("{credentials:?}");
    assert!(!rendered.contains("LLM_SUPER_SECRET_KEY"), "{rendered}");
    assert!(!rendered.contains("dca-super-secret-token"), "{rendered}");
    assert!(rendered.contains("[REDACTED]"), "{rendered}");
}

#[test]
fn load_rejects_tampered_stored_credentials() {
    let (_sandbox, previous) = sandbox_without_opt_in();
    // Well-formed active record loads fine.
    let mut credentials = MuseCredentials {
        api_key: "LLM_GOOD_KEY".to_string(),
        access_token: "dca-good".to_string(),
        base_url: EXPECTED_API_BASE_URL.to_string(),
        user_email: None,
        user_full_name: None,
        is_subs_active: true,
        minted_at_ms: 1,
        subs_usage: None,
    };
    save_credentials(&credentials).expect("save");
    load_credentials().expect("load good record");

    // Wrong base URL must not load (never route to an arbitrary host).
    credentials.base_url = "https://evil.example/v1".to_string();
    assert!(
        validate_stored_credentials(&credentials).is_err(),
        "wrong base_url must fail validation"
    );
    // Inactive subscription must not load.
    credentials.base_url = EXPECTED_API_BASE_URL.to_string();
    credentials.is_subs_active = false;
    assert!(
        validate_stored_credentials(&credentials).is_err(),
        "inactive subscription must fail validation"
    );
    // Multi-line secrets must not load.
    credentials.is_subs_active = true;
    credentials.api_key = "line1\nline2".to_string();
    assert!(
        validate_stored_credentials(&credentials).is_err(),
        "multi-line api key must fail validation"
    );
    restore_opt_in(previous);
}

#[test]
fn device_challenge_debug_redacts_device_code() {
    let challenge = DeviceChallenge {
        device_code: "dev-super-secret".to_string(),
        user_code: "ABCD-1234".to_string(),
        verification_uri: "https://auth.meta.com/device".to_string(),
        verification_uri_complete: None,
        expires_in: 900,
        interval: 5,
    };
    let rendered = format!("{challenge:?}");
    assert!(!rendered.contains("dev-super-secret"), "{rendered}");
    assert!(rendered.contains("ABCD-1234"), "{rendered}");
    assert!(rendered.contains("[REDACTED]"), "{rendered}");
}

#[test]
fn save_load_clear_roundtrip_with_owner_only_file() {
    let (_sandbox, previous) = sandbox_without_opt_in();
    let credentials = MuseCredentials {
        api_key: "LLM_ROUNDTRIP_KEY".to_string(),
        access_token: "dca-roundtrip".to_string(),
        base_url: EXPECTED_API_BASE_URL.to_string(),
        user_email: None,
        user_full_name: None,
        is_subs_active: true,
        minted_at_ms: chrono::Utc::now().timestamp_millis(),
        subs_usage: Some(SubsUsage {
            window_used_percent: Some(12.0),
            window_duration_mins: Some(300),
            window_resets_at: None,
            weekly_used_percent: Some(34.0),
            weekly_resets_at: None,
            tier: Some("pro".to_string()),
        }),
    };
    save_credentials(&credentials).expect("save");
    assert!(has_cached_auth());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(credentials_path().unwrap())
            .expect("stat")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "credential file must be owner-only");
    }

    let loaded = load_credentials().expect("load");
    assert_eq!(loaded.api_key, "LLM_ROUNDTRIP_KEY");
    assert_eq!(loaded.access_token, "dca-roundtrip");

    let summary = usage_summary().expect("summary");
    assert!(summary.contains("snapshot"), "{summary}");
    assert!(summary.contains("12%"), "{summary}");
    assert!(!summary.contains("LLM_ROUNDTRIP_KEY"), "{summary}");

    clear_credentials().expect("clear");
    assert!(!has_cached_auth());
    assert!(usage_summary().is_none());
    restore_opt_in(previous);
}

#[test]
fn usage_summary_rejects_invalid_snapshot_values() {
    let (_sandbox, previous) = sandbox_without_opt_in();
    // Hostile snapshot: NaN/infinite/out-of-range percents, absurd duration,
    // control-char tier injection. None of it may render.
    let credentials = MuseCredentials {
        api_key: "LLM_SNAPSHOT_KEY".to_string(),
        access_token: "dca-snapshot".to_string(),
        base_url: EXPECTED_API_BASE_URL.to_string(),
        user_email: None,
        user_full_name: None,
        is_subs_active: true,
        minted_at_ms: 1,
        subs_usage: Some(SubsUsage {
            window_used_percent: Some(f64::NAN),
            window_duration_mins: Some(0),
            window_resets_at: None,
            weekly_used_percent: Some(101.0),
            weekly_resets_at: None,
            tier: Some("pro\x1b[2J\x07".to_string()),
        }),
    };
    save_credentials(&credentials).expect("save");
    let summary = usage_summary().expect("summary");
    assert!(summary.contains("quota unknown"), "{summary}");
    assert!(!summary.contains("NaN"), "{summary}");
    assert!(!summary.contains("101"), "{summary}");
    assert!(!summary.contains("\x1b"), "{summary}");
    assert!(!summary.contains("tier"), "{summary}");

    // Infinite percent is also rejected.
    let mut credentials = credentials;
    credentials.subs_usage = Some(SubsUsage {
        window_used_percent: Some(f64::INFINITY),
        window_duration_mins: Some(300),
        window_resets_at: None,
        weekly_used_percent: None,
        weekly_resets_at: None,
        tier: None,
    });
    save_credentials(&credentials).expect("save");
    let summary = usage_summary().expect("summary");
    assert!(summary.contains("quota unknown"), "{summary}");

    // Tier alone (no valid percent) still reports quota unknown.
    credentials.subs_usage = Some(SubsUsage {
        window_used_percent: None,
        window_duration_mins: None,
        window_resets_at: None,
        weekly_used_percent: None,
        weekly_resets_at: None,
        tier: Some("pro".to_string()),
    });
    save_credentials(&credentials).expect("save");
    let summary = usage_summary().expect("summary");
    assert!(summary.contains("quota unknown"), "{summary}");
    assert!(!summary.contains("tier pro"), "{summary}");
    restore_opt_in(previous);
}

#[tokio::test]
async fn refresh_if_needed_skips_network_when_fresh() {
    let (_sandbox, previous) = sandbox_without_opt_in();
    let credentials = MuseCredentials {
        api_key: "LLM_FRESH_KEY".to_string(),
        access_token: "dca-fresh".to_string(),
        base_url: EXPECTED_API_BASE_URL.to_string(),
        user_email: None,
        user_full_name: None,
        is_subs_active: true,
        minted_at_ms: chrono::Utc::now().timestamp_millis(),
        subs_usage: None,
    };
    save_credentials(&credentials).expect("save");
    // Closed port: any network use would fail loudly.
    let _endpoints = set_test_endpoints("http://127.0.0.1:1", "http://127.0.0.1:1");
    let client = http_client().unwrap();

    let current = refresh_if_needed(&client).await.expect("fresh creds");
    assert_eq!(current.api_key, "LLM_FRESH_KEY");

    restore_opt_in(previous);
}

#[tokio::test]
async fn refresh_if_needed_remints_when_stale() {
    let (_sandbox, previous) = sandbox_without_opt_in();
    let credentials = MuseCredentials {
        api_key: "LLM_STALE_KEY".to_string(),
        access_token: "dca-stale".to_string(),
        base_url: EXPECTED_API_BASE_URL.to_string(),
        user_email: None,
        user_full_name: None,
        is_subs_active: true,
        minted_at_ms: 1,
        subs_usage: None,
    };
    save_credentials(&credentials).expect("save");
    let server = start_stub(vec![(
        200,
        mint_json("LLM_ROTATED_KEY", Some(true), Some(false)),
    )])
    .await;
    let _endpoints = use_stub(&server);
    let client = http_client().unwrap();

    let current = refresh_if_needed(&client).await.expect("remint");
    assert_eq!(current.api_key, "LLM_ROTATED_KEY");
    assert_eq!(load_credentials().expect("load").api_key, "LLM_ROTATED_KEY");

    server.shutdown().await;
    restore_opt_in(previous);
}

/// Serve `responses` in order on loopback, invoking `on_request` synchronously
/// when each request is received before sending the response.
async fn start_stub_with_hook<F>(responses: Vec<(u16, String)>, on_request: F) -> StubServer
where
    F: Fn(&str) + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let bodies = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = bodies.clone();
    let on_request = std::sync::Arc::new(on_request);
    let handle = tokio::spawn(async move {
        for (status, body) in responses {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let request_body = read_http_body(&mut stream).await;
            captured.lock().unwrap().push(request_body.clone());
            on_request(&request_body);
            let response = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });
    StubServer {
        base_url,
        bodies,
        handle,
    }
}

#[tokio::test]
async fn refresh_credentials_fails_and_does_not_resurrect_when_deleted_concurrently() {
    let (_sandbox, previous) = sandbox_without_opt_in();
    let credentials = MuseCredentials {
        api_key: "LLM_INITIAL_KEY".to_string(),
        access_token: "dca-account-to-delete".to_string(),
        base_url: EXPECTED_API_BASE_URL.to_string(),
        user_email: None,
        user_full_name: None,
        is_subs_active: true,
        minted_at_ms: chrono::Utc::now().timestamp_millis(),
        subs_usage: None,
    };
    save_credentials(&credentials).expect("save initial credentials");

    // The server hook unlinks credentials between the mint request and the response/write.
    let server = start_stub_with_hook(
        vec![(200, mint_json("LLM_MINTED_KEY", Some(true), Some(false)))],
        |_| {
            clear_credentials().expect("clear credentials during mint");
        },
    )
    .await;
    let _endpoints = use_stub(&server);
    let client = http_client().unwrap();

    let err = refresh_credentials(&client)
        .await
        .expect_err("refresh must fail when credentials disappeared");
    assert!(
        err.to_string().contains("refusing to overwrite"),
        "unexpected error: {err:#}"
    );

    // Verify credentials file is NOT resurrected.
    assert!(
        load_credentials().is_err(),
        "credentials must not be resurrected after logout"
    );
    let path = credentials_path().expect("credentials path");
    assert!(!path.exists(), "credentials file must not exist on disk");

    server.shutdown().await;
    restore_opt_in(previous);
}

#[tokio::test]
async fn refresh_credentials_refuses_to_overwrite_different_account_saved_concurrently() {
    let (_sandbox, previous) = sandbox_without_opt_in();
    let credentials = MuseCredentials {
        api_key: "LLM_ORIGINAL_KEY".to_string(),
        access_token: "dca-account-original".to_string(),
        base_url: EXPECTED_API_BASE_URL.to_string(),
        user_email: Some("account1@example.com".to_string()),
        user_full_name: None,
        is_subs_active: true,
        minted_at_ms: chrono::Utc::now().timestamp_millis(),
        subs_usage: None,
    };
    save_credentials(&credentials).expect("save original credentials");

    // The server hook saves a different account between the mint request and the response/write.
    let different_account = MuseCredentials {
        api_key: "LLM_NEW_ACCOUNT_KEY".to_string(),
        access_token: "dca-account-different".to_string(),
        base_url: EXPECTED_API_BASE_URL.to_string(),
        user_email: Some("account2@example.com".to_string()),
        user_full_name: None,
        is_subs_active: true,
        minted_at_ms: chrono::Utc::now().timestamp_millis(),
        subs_usage: None,
    };
    let diff_clone = different_account.clone();
    let server = start_stub_with_hook(
        vec![(
            200,
            mint_json("LLM_OLD_ACCOUNT_NEW_MINT", Some(true), Some(false)),
        )],
        move |_| {
            save_credentials(&diff_clone).expect("save different account");
        },
    )
    .await;
    let _endpoints = use_stub(&server);
    let client = http_client().unwrap();

    let err = refresh_credentials(&client)
        .await
        .expect_err("refresh must fail when account token changed");
    assert!(
        err.to_string().contains("refusing to overwrite"),
        "unexpected error: {err:#}"
    );

    // Verify the new account credentials were NOT overwritten.
    let current = load_credentials().expect("load credentials");
    assert_eq!(current.access_token, "dca-account-different");
    assert_eq!(current.api_key, "LLM_NEW_ACCOUNT_KEY");
    assert_eq!(current.user_email.as_deref(), Some("account2@example.com"));

    server.shutdown().await;
    restore_opt_in(previous);
}

#[tokio::test]
async fn credentials_lock_provides_mutual_exclusion() {
    let (_sandbox, previous) = sandbox_without_opt_in();
    let credentials = MuseCredentials {
        api_key: "LLM_MUTEX_KEY".to_string(),
        access_token: "dca-mutex".to_string(),
        base_url: EXPECTED_API_BASE_URL.to_string(),
        user_email: None,
        user_full_name: None,
        is_subs_active: true,
        minted_at_ms: chrono::Utc::now().timestamp_millis(),
        subs_usage: None,
    };
    save_credentials(&credentials).expect("save credentials");

    let guard = lock_credentials().expect("acquire credentials lock");
    let lock_path = credentials_lock_path().expect("lock path");
    assert!(lock_path.exists(), "lock file should exist on disk");

    // A second handle must not be able to take the lock while the guard is held.
    // `try_lock` is the standard library's portable primitive, so this holds on
    // Windows as well as Unix, which is exactly why the credential lock no
    // longer uses a Unix-only flock.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("open lock file");
    assert!(
        file.try_lock().is_err(),
        "try_lock must fail while the credential lock guard is held"
    );

    // Dropping the guard releases it for another thread or process.
    drop(guard);
    file.try_lock()
        .expect("try_lock must succeed once the guard is dropped");
    let _ = file.unlock();

    restore_opt_in(previous);
}

/// Regression test for the refresh-vs-logout race.
///
/// `refresh_credentials` must hold the shared credential lock across the final
/// reload, comparison and write. The test hook runs inside that critical
/// section, and a concurrent logout must be unable to complete while it runs.
/// Without the lock, the logout lands between the comparison and the write and
/// the write then resurrects the cleared credentials.
///
/// The final-state assertion is the second half of the same guarantee: because
/// the logout is forced to wait, it lands strictly after the refresh write, so
/// the account must end up logged out rather than restored.
#[tokio::test]
async fn refresh_holds_lock_so_logout_cannot_interleave_between_compare_and_write() {
    let (_sandbox, previous) = sandbox_without_opt_in();
    let credentials = MuseCredentials {
        api_key: "LLM_RACE_KEY".to_string(),
        access_token: "dca-race".to_string(),
        base_url: EXPECTED_API_BASE_URL.to_string(),
        user_email: None,
        user_full_name: None,
        is_subs_active: true,
        minted_at_ms: 1,
        subs_usage: None,
    };
    save_credentials(&credentials).expect("save initial credentials");

    let server = start_stub(vec![(
        200,
        mint_json("LLM_RACE_MINTED_KEY", Some(true), Some(false)),
    )])
    .await;
    let _endpoints = use_stub(&server);
    let client = http_client().unwrap();

    // The logout thread waits for the hook's signal, so it is guaranteed to
    // attempt the delete only while refresh is inside the critical section.
    let (entered_tx, entered_rx) = std::sync::mpsc::channel::<()>();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<bool>();
    let logout = std::thread::spawn(move || {
        entered_rx
            .recv()
            .expect("critical section must be entered before logout runs");
        let cleared = clear_credentials().is_ok();
        let _ = done_tx.send(cleared);
    });

    let _hook = set_refresh_critical_section_hook(move || {
        let _ = entered_tx.send(());
        match done_rx.recv_timeout(std::time::Duration::from_millis(500)) {
            // Correct: the logout is blocked on the credential lock.
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Ok(cleared) => panic!(
                "logout completed inside the refresh critical section (cleared={cleared}); \
                 the compare-and-write window is not protected"
            ),
            Err(err) => panic!("unexpected logout channel state: {err:?}"),
        }
    });

    let refreshed = refresh_credentials(&client).await;
    drop(_hook);
    assert!(
        refreshed.is_ok(),
        "refresh must succeed while it holds the lock: {refreshed:?}"
    );

    logout.join().expect("logout thread must finish");
    assert!(
        load_credentials().is_err(),
        "logout must win: the refresh must not resurrect credentials"
    );
    assert!(
        !credentials_path().expect("credentials path").exists(),
        "credentials file must be gone after the logout"
    );

    server.shutdown().await;
    restore_opt_in(previous);
}
