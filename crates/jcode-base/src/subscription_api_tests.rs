use super::*;
use std::io::{Read, Write};

/// (status, extra headers, body) for one canned HTTP response.
type CannedResponse = (u16, Vec<(&'static str, &'static str)>, String);

fn spawn_server(responses: Vec<CannedResponse>) -> String {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    let addr = listener.local_addr().expect("addr");
    std::thread::spawn(move || {
        for (status, headers, body) in responses {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0u8; 8192];
            let _ = stream.read(&mut request);
            let extra = headers
                .into_iter()
                .map(|(name, value)| format!("{name}: {value}\r\n"))
                .collect::<String>();
            let response = format!(
                "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).expect("write");
        }
    });
    format!("http://{addr}/v1")
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("client")
}

#[test]
fn subscription_me_parses_expected_shape() {
    let json = r#"{
        "account_id": "acct_123", "email": "dev@example.com",
        "tier": "flagship", "status": "active",
        "usage": {"used_usd": 12.5, "budget_usd": 3000.0},
        "manage_url": "https://jcode.sh/account"
    }"#;
    let me: SubscriptionMe = serde_json::from_str(json).expect("parse");
    assert_eq!(me.parsed_tier(), Some(JcodeTier::Flagship));
    assert!(!me.capabilities.voice_transcription);
    assert!(!me.capabilities.remote_compile);
    assert!(me.has_active_paid_plan());
    assert_eq!(me.manage_url.as_deref(), Some("https://jcode.sh/account"));
}

#[test]
fn metered_account_parses_spending_limit_and_tranche_state_without_a_tier() {
    let json = r#"{
        "account_id": "acct_metered", "email": "dev@example.com",
        "tier": "none", "status": "active",
        "usage": {
            "used_usd": 34.5,
            "spend_limit_usd": 100.0,
            "billed_usd": 20.0,
            "next_charge_at_usd": 60.0
        }
    }"#;
    let me: SubscriptionMe = serde_json::from_str(json).expect("parse metered account");
    assert_eq!(me.parsed_tier(), None);
    assert!(me.has_active_paid_plan());
    assert_eq!(me.usage.budget_usd, 100.0);
    assert_eq!(me.usage.billed_usd, 20.0);
    assert_eq!(me.usage.next_charge_at_usd, Some(60.0));
}

#[test]
fn polling_backoff_is_deterministic_and_bounded() {
    let mut state = PollingBackoff::new(Duration::from_secs(3));
    assert_eq!(state.delay(), Duration::from_secs(3));
    state.on_slow_down(None);
    assert_eq!(state.delay(), Duration::from_secs(8));
    state.on_slow_down(Some(Duration::from_secs(12)));
    assert_eq!(state.delay(), Duration::from_secs(12));
    state.on_offline_error();
    assert_eq!(state.delay(), Duration::from_secs(24));
    state.on_offline_error();
    assert_eq!(state.delay(), Duration::from_secs(30));
    state.on_pending();
    assert_eq!(state.delay(), Duration::from_secs(3));
}

#[tokio::test]
async fn device_request_uses_new_contract_and_parses_public_urls() {
    let base = spawn_server(vec![(
        200,
        vec![],
        r#"{"device_code":"secret","flow_id":"public-flow","verification_uri":"https://jcode.sh/account","verification_uri_complete":"https://jcode.sh/account?flow=public-flow","verify_url":"https://jcode.sh/account?flow=public-flow","expires_in":600,"interval":3}"#.to_string(),
    )]);
    let result = request_device_authorization(&client(), &base, Some(JcodeTier::Pro))
        .await
        .expect("device auth");
    assert_eq!(result.device_code, "secret");
    assert_eq!(result.flow_id, "public-flow");
    assert!(!result.verification_uri_complete.contains("secret"));
}

#[tokio::test]
async fn metered_device_request_does_not_select_a_subscription_tier() {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    let addr = listener.local_addr().expect("addr");
    let (request_tx, request_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut request = [0u8; 8192];
        let count = stream.read(&mut request).expect("read request");
        request_tx
            .send(String::from_utf8_lossy(&request[..count]).into_owned())
            .expect("capture request");
        let body = r#"{"device_code":"secret","flow_id":"public-flow","verification_uri":"https://jcode.sh/account","verification_uri_complete":"https://jcode.sh/account?flow=public-flow","expires_in":600,"interval":3}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).expect("write");
    });

    request_device_authorization(&client(), &format!("http://{addr}/v1"), None)
        .await
        .expect("device auth");
    let request = request_rx.recv().expect("captured request");
    assert!(request.contains(r#"{"client_name":"jcode-cli"}"#));
    assert!(!request.contains("requested_tier"), "{request}");
}

#[tokio::test]
async fn legacy_device_response_is_explained_without_echoing_body() {
    let base = spawn_server(vec![(
        200,
        vec![],
        r#"{"device_code":"do-not-echo","verify_url":"https://old.example/login"}"#.to_string(),
    )]);
    let error = request_device_authorization(&client(), &base, Some(JcodeTier::Pro))
        .await
        .expect_err("legacy response rejected");
    assert_eq!(error, AccountApiError::LegacyBackend);
    assert!(!error.to_string().contains("do-not-echo"));
}

#[tokio::test]
async fn token_poll_handles_pending_slow_down_success_denied_and_replay() {
    let base = spawn_server(vec![
        (428, vec![], r#"{"error":"authorization_pending"}"#.to_string()),
        (429, vec![("Retry-After", "9")], r#"{"error":"slow_down"}"#.to_string()),
        (200, vec![], r#"{"api_key":"jck_live_test","account_id":"acct","email":"user@example.com","tier":"none","status":"active"}"#.to_string()),
        (400, vec![], r#"{"error":"access_denied"}"#.to_string()),
        (400, vec![], r#"{"error":"expired_token"}"#.to_string()),
    ]);
    let client = client();
    assert_eq!(
        poll_device_token_once(&client, &base, "secret")
            .await
            .unwrap(),
        TokenPollOutcome::Pending
    );
    assert_eq!(
        poll_device_token_once(&client, &base, "secret")
            .await
            .unwrap(),
        TokenPollOutcome::SlowDown {
            retry_after: Some(Duration::from_secs(9))
        }
    );
    assert!(matches!(
        poll_device_token_once(&client, &base, "secret")
            .await
            .unwrap(),
        TokenPollOutcome::Approved(_)
    ));
    assert_eq!(
        poll_device_token_once(&client, &base, "secret")
            .await
            .unwrap(),
        TokenPollOutcome::Denied
    );
    assert_eq!(
        poll_device_token_once(&client, &base, "secret")
            .await
            .unwrap(),
        TokenPollOutcome::Expired
    );
}

#[tokio::test]
async fn me_and_revoke_classify_revoked_keys_without_leaking_them() {
    let base = spawn_server(vec![
        (
            401,
            vec![],
            r#"{"error":"invalid_key","message":"jck_live_do-not-log"}"#.to_string(),
        ),
        (401, vec![], r#"{"error":"invalid_key"}"#.to_string()),
    ]);
    let client = client();
    let me_error = fetch_subscription_me_with(&client, &base, "jck_live_secret")
        .await
        .expect_err("revoked");
    assert_eq!(me_error, AccountApiError::Unauthorized);
    assert!(!me_error.to_string().contains("jck_live"));
    let revoke_error = revoke_current_key(&client, &base, "jck_live_secret")
        .await
        .expect_err("already revoked");
    assert_eq!(revoke_error, AccountApiError::Unauthorized);
}

#[tokio::test]
async fn billing_cap_and_portal_parse_and_reject_unsafe_urls() {
    let billing = r#"{"billing":{"currency":"usd","activation":{"state":"active"},
        "promotional_credit":{"original_microusd":5000000,"remaining_microusd":2500000},
        "monthly_hard_cap_cents":5000,
        "period":{"resets_at":"2026-11-01T00:00:00.000Z","billable_microusd":12340000,"remaining_cap_microusd":37660000}}}"#;
    let capped = billing.replace("5000,", "10000,");
    let base = spawn_server(vec![
        (200, vec![], billing.to_string()),
        (200, vec![], capped),
        (
            200,
            vec![],
            r#"{"url":"https://billing.stripe.com/p/session/x"}"#.into(),
        ),
        (200, vec![], r#"{"url":"http://evil.example/"}"#.into()),
        (
            409,
            vec![],
            r#"{"error":{"code":"no_billing_account"}}"#.into(),
        ),
    ]);
    let client = client();
    let status = fetch_billing_with(&client, &base, "k")
        .await
        .expect("billing");
    assert_eq!(status.monthly_hard_cap_usd(), 50.0);
    assert!((status.billable_usd() - 12.34).abs() < 1e-9);
    assert_eq!(status.promotional_credit_usd(), 2.5);
    assert_eq!(status.activation.state, "active");
    let updated = set_monthly_hard_cap_with(&client, &base, "k", 10_000)
        .await
        .expect("cap");
    assert_eq!(updated.monthly_hard_cap_cents, 10_000);
    assert_eq!(
        billing_portal_url_with(&client, &base, "k")
            .await
            .expect("portal"),
        "https://billing.stripe.com/p/session/x"
    );
    assert!(matches!(
        billing_portal_url_with(&client, &base, "k").await,
        Err(AccountApiError::InvalidResponse(_))
    ));
    assert_eq!(
        billing_portal_url_with(&client, &base, "k").await,
        Err(AccountApiError::Http {
            status: 409,
            code: Some("no_billing_account".into())
        })
    );
}

#[tokio::test]
async fn checkout_and_plan_change_only_accept_safe_replies() {
    let base = spawn_server(vec![
        (
            200,
            vec![],
            r#"{"url":"https://checkout.stripe.com/c/pay/x"}"#.into(),
        ),
        (
            200,
            vec![],
            r#"{"url":"https://billing.stripe.com/p/x"}"#.into(),
        ),
        (
            200,
            vec![],
            r#"{"plan_usd":100,"monthly_limit_usd":1000,"effective":"next_invoice"}"#.into(),
        ),
        (409, vec![], r#"{"error":{"code":"not_subscribed"}}"#.into()),
    ]);
    let client = client();
    assert_eq!(
        start_subscription_checkout_with(&client, &base, "k", 50)
            .await
            .expect("checkout"),
        "https://checkout.stripe.com/c/pay/x"
    );
    assert!(matches!(
        start_subscription_checkout_with(&client, &base, "k", 50).await,
        Err(AccountApiError::InvalidResponse(_))
    ));
    let change = change_plan_with(&client, &base, "k", 100)
        .await
        .expect("plan");
    assert_eq!(change.plan_usd, 100);
    assert_eq!(change.effective, "next_invoice");
    assert_eq!(
        change_plan_with(&client, &base, "k", 100).await,
        Err(AccountApiError::Http {
            status: 409,
            code: Some("not_subscribed".into())
        })
    );
}
