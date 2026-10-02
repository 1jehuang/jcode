use super::*;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn credentials() -> auth::codex::CodexCredentials {
    auth::codex::CodexCredentials {
        access_token: "test-access-secret".into(),
        refresh_token: "test-refresh-secret".into(),
        account_id: Some("pinned-account".into()),
        id_token: None,
        expires_at: None,
    }
}

fn credit(id: &str, expiry: Option<&str>, status: &str) -> Value {
    json!({"id": id, "status": status, "reset_type": "codex_rate_limits",
        "expires_at": expiry, "title": "Full reset (Weekly + 5 hr)",
        "description": "Reset usage limits"})
}

async fn server(responses: Vec<(u16, Value)>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/wham/rate-limit-reset-credits",
        listener.local_addr().unwrap()
    );
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for (status, body) in responses {
            let (mut stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
                .await
                .unwrap()
                .unwrap();
            let mut request = Vec::new();
            loop {
                let mut chunk = [0; 4096];
                let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut chunk))
                    .await
                    .unwrap()
                    .unwrap();
                assert!(n > 0, "request ended early");
                request.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&request);
                if let Some((headers, body)) = text.split_once("\r\n\r\n") {
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .and_then(|n| n.parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if body.len() >= length {
                        break;
                    }
                }
            }
            requests.push(String::from_utf8(request).unwrap());
            let body = body.to_string();
            stream.write_all(format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        }
        requests
    });
    (url, task)
}

async fn prepare(client: &reqwest::Client, url: &str) -> Result<Option<PendingOpenAiUsageReset>> {
    prepare_with_credentials(
        client,
        url,
        credentials(),
        Some("openai-test".into()),
        "openai-test".into(),
    )
    .await
}

#[tokio::test]
async fn preparation_is_read_only_and_selects_earliest_expiring_available_credit() {
    let (url, task) = server(vec![(
        200,
        json!({"available_count": 3, "credits": [
            credit("no-expiry", None, "available"),
            credit("later", Some("2099-06-01T00:00:00Z"), "available"),
            credit("earliest", Some("2099-05-01T00:00:00Z"), "available"),
            credit("expired", Some("2000-01-01T00:00:00Z"), "available"),
            credit("redeemed", Some("2099-01-01T00:00:00Z"), "redeemed"),
            credit("invalid", Some("not-a-date"), "available")
        ]}),
    )])
    .await;
    let pending = prepare(&reset_client().unwrap(), &url)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pending.credit_id, "earliest");
    assert!(uuid::Uuid::parse_str(&pending.redeem_request_id).is_ok());
    let requests = task.await.unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("GET /wham/rate-limit-reset-credits HTTP/1.1"));
    assert!(requests[0].contains("authorization: Bearer test-access-secret"));
    assert!(requests[0].contains("chatgpt-account-id: pinned-account"));
    let message = pending.confirmation_message();
    assert!(message.contains("openai-test"));
    assert!(message.contains("3 banked"));
    assert!(message.contains("Reset 1 expires: unknown"));
    assert!(message.contains("Reset 2 expires: 2099-06-01"));
    assert!(message.contains("Reset 3 expires: 2099-05-01"));
    assert!(message.contains("cannot be undone"));
    assert!(message.contains("/reset usage limits openai confirm"));
    assert!(!message.contains("secret"));
    assert!(!format!("{pending:?}").contains("secret"));
}

#[tokio::test]
async fn no_available_credits_does_not_prepare_redemption() {
    let (url, task) = server(vec![(200, json!({"available_count": 0, "credits": []}))]).await;
    assert!(
        prepare(&reset_client().unwrap(), &url)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(task.await.unwrap().len(), 1);
}

#[tokio::test]
async fn inconsistent_or_unknown_payload_fails_closed() {
    for payload in [
        json!({"available_count": 1, "credits": []}),
        json!({"new_schema": true}),
        json!({"available_count": 1, "credits": [credit("old", Some("2000-01-01T00:00:00Z"), "available")]}),
    ] {
        let (url, task) = server(vec![(200, payload)]).await;
        assert!(prepare(&reset_client().unwrap(), &url).await.is_err());
        task.await.unwrap();
    }
}

#[tokio::test]
async fn consume_retries_pin_credit_account_and_idempotency_key() {
    let (url, task) = server(vec![
        (
            200,
            json!({"available_count": 1, "credits": [credit("one-credit", None, "available")]}),
        ),
        (503, json!({"sensitive": "do-not-display"})),
        (200, json!({"code": "already_redeemed"})),
    ])
    .await;
    let client = reset_client().unwrap();
    let pending = prepare(&client, &url).await.unwrap().unwrap();
    let error = consume_with_client(&client, &url, &pending)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("uncertain"));
    assert!(error.contains("SAME redemption"));
    assert!(!error.contains("do-not-display"));
    let outcome = consume_with_client(&client, &url, &pending.clone())
        .await
        .unwrap();
    assert_eq!(outcome.code, ResetCode::AlreadyRedeemed);
    let requests = task.await.unwrap();
    assert_eq!(requests.len(), 3);
    for request in &requests[1..] {
        assert!(request.starts_with("POST /wham/rate-limit-reset-credits/consume HTTP/1.1"));
        assert!(request.contains("chatgpt-account-id: pinned-account"));
        assert!(request.contains("authorization: Bearer test-access-secret"));
        let body: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(
            body,
            json!({"credit_id": "one-credit", "redeem_request_id": pending.redeem_request_id})
        );
    }
}

#[test]
fn all_official_outcomes_are_handled_and_unknown_codes_are_not_success() {
    for (code, text) in [
        ("reset", "2 window(s)"),
        ("nothing_to_reset", "nothing to reset"),
        ("no_credit", "no banked reset"),
        ("already_redeemed", "already redeemed"),
    ] {
        let outcome: OpenAiUsageResetOutcome =
            serde_json::from_value(json!({"code": code, "windows_reset": 2})).unwrap();
        assert!(outcome.message().contains(text));
    }
    assert!(
        serde_json::from_value::<OpenAiUsageResetOutcome>(json!({"code": "future_outcome"}))
            .is_err()
    );
}

#[tokio::test]
async fn auth_rate_limit_and_unsupported_errors_have_actionable_safe_messages() {
    for (status, hint) in [
        (401, "/login openai"),
        (403, "eligible"),
        (404, "not available"),
        (429, "rate limiting"),
    ] {
        let (url, task) = server(vec![(status, json!({"secret": "never-echo-this"}))]).await;
        let error = prepare(&reset_client().unwrap(), &url)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(hint), "{error}");
        assert!(!error.contains("never-echo-this"));
        task.await.unwrap();
    }
}

#[test]
fn reset_cache_invalidation_is_scoped_to_target_account() {
    let key = openai_usage_cache_key("test-token-reset", Some("reset-target-test"));
    let other_key = openai_usage_cache_key("test-token-other", Some("reset-other-test"));
    let data = OpenAIUsageData {
        fetched_at: Some(Instant::now()),
        ..Default::default()
    };
    store_openai_usage(key.clone(), data.clone());
    store_openai_usage(other_key.clone(), data);
    super::super::cache::invalidate_openai_usage_after_reset(
        "test-token-reset",
        Some("reset-target-test"),
    );
    assert!(cached_openai_usage(&key).is_none());
    assert!(cached_openai_usage(&other_key).is_some());
}

#[test]
fn provider_text_cannot_inject_terminal_controls() {
    assert_eq!(display_text("hello\n\x1b\rworld"), "helloworld");
    assert_eq!(display_text(&"a".repeat(1000)).len(), 500);
}

#[tokio::test]
async fn reset_generation_prevents_old_fetches_from_repopulating_usage() {
    let _guard = crate::storage::lock_test_env();
    let generation = openai_usage_generation();
    let key = openai_usage_cache_key("old-request", Some("reset-race-test"));
    let exhausted = OpenAIUsageData {
        hard_limit_reached: true,
        fetched_at: Some(Instant::now()),
        ..Default::default()
    };
    super::super::cache::invalidate_openai_usage_after_reset(
        "old-request",
        Some("reset-race-test"),
    );
    store_openai_usage_for_generation(generation, key.clone(), exhausted);
    assert!(cached_openai_usage(&key).is_none());

    let usage = get_openai_usage_cell().await;
    *usage.write().await = OpenAIUsageData::default();
    super::super::sync_openai_usage_from_reports(
        &[ProviderUsage {
            provider_name: "OpenAI".into(),
            hard_limit_reached: true,
            ..Default::default()
        }],
        generation,
    )
    .await;
    assert!(!usage.read().await.hard_limit_reached);

    store_openai_usage_for_generation(
        openai_usage_generation(),
        key.clone(),
        OpenAIUsageData {
            fetched_at: Some(Instant::now()),
            ..Default::default()
        },
    );
    assert!(cached_openai_usage(&key).is_some());
}

#[test]
fn reset_metadata_survives_cache_roundtrip_but_not_errors() {
    let report = ProviderUsage {
        provider_name: "OpenAI (ChatGPT)".into(),
        openai_reset_credits: Some(OpenAiResetCredits {
            available_count: 3,
            available_expirations: vec![Some("2099-06-01T00:00:00Z".into()), None],
            account_label: Some("openai-test".into()),
            ordinary_usage_allowed: Some(false),
        }),
        ..Default::default()
    };
    let data = openai_usage_data_from_provider_report(&report);
    let roundtrip = provider_report_from_openai_usage_data(report.provider_name.clone(), &data);
    assert_eq!(roundtrip.openai_reset_credits, report.openai_reset_credits);
    let failed = ProviderUsage {
        error: Some("HTTP 401".into()),
        ..report
    };
    assert!(
        openai_usage_data_from_provider_report(&failed)
            .openai_reset_credits
            .is_none()
    );
}

/// Opt-in read-only smoke check. This never redeems a reset or logs credentials.
#[tokio::test]
#[ignore = "requires a real ChatGPT OAuth login and network"]
async fn live_openai_banked_reset_availability_read_only() {
    match prepare_openai_usage_reset().await.unwrap() {
        Some(pending) => {
            println!(
                "Banked reset API accepted OAuth credentials: {} reset(s) available; selection ready (not consumed).",
                pending.available_count
            );
            let report = fetch_openai_usage_for_account(
                pending.account_display.clone(),
                pending.credentials.clone(),
                pending.account_label(),
            )
            .await;
            assert!(
                report.error.is_none(),
                "Read-only usage check failed: {:?}",
                report.error
            );
            let metadata = report
                .openai_reset_credits
                .expect("Expected banked reset summary from wham/usage");
            println!(
                "Read-only usage summary: {} banked reset(s), ordinary usage allowed: {:?}.",
                metadata.available_count, metadata.ordinary_usage_allowed
            );
        }
        None => println!(
            "Banked reset API accepted OAuth credentials: no resets available (not consumed)."
        ),
    }
}

#[tokio::test]
async fn expiry_metadata_lookup_is_read_only_and_filters_unavailable_credits() {
    let (url, task) = server(vec![(
        200,
        json!({"available_count": 4, "credits": [
            credit("known", Some("2099-05-01T00:00:00Z"), "available"),
            credit("expired", Some("2000-01-01T00:00:00Z"), "available"),
            credit("spent", Some("2099-05-01T00:00:00Z"), "redeemed"),
            credit("", Some("2099-05-01T00:00:00Z"), "available"),
            credit("unknown", None, "available"),
            credit("invalid", Some("not-a-date"), "available")
        ]}),
    )])
    .await;
    let expiries = fetch_available_expirations_at(&reset_client().unwrap(), &url, &credentials())
        .await
        .unwrap();
    assert_eq!(
        expiries,
        vec![Some("2099-05-01T00:00:00+00:00".into()), None, None]
    );
    let requests = task.await.unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("GET /wham/rate-limit-reset-credits HTTP/1.1"));
    assert!(requests[0].contains("chatgpt-account-id: pinned-account"));
}

#[tokio::test]
async fn expiry_metadata_lookup_failure_is_reported_without_redemption() {
    let (url, task) = server(vec![(503, json!({"error": "unavailable"}))]).await;
    assert!(
        fetch_available_expirations_at(&reset_client().unwrap(), &url, &credentials())
            .await
            .is_err()
    );
    let requests = task.await.unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("GET "));
}
