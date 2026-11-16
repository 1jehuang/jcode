use super::{tests::*, *};

#[tokio::test]
async fn browser_subscription_checks_browser_capability_and_posts_choice() {
    let answer =
        json!({"answers": {"action": {"type": "choice", "choice": "click", "confidence": 0.9}}});
    let (base, worker) = mock_server(vec![
        (
            200,
            json!({"capabilities": {"browser_jev": true, "memory_jev": false}}).to_string(),
            vec![],
        ),
        (200, answer.to_string(), vec![]),
    ]);
    let mut client = mock_client(&base, JevProvider::Jcode);
    client.purpose = JevPurpose::Browser;
    assert_eq!(client.provider_name(), "jcode");
    assert_eq!(client.model_id(), "typesafe/jev-1.13");
    assert_eq!(
        client
            .evaluate(json!({"page": "private-page"}), browser_questions())
            .await
            .unwrap(),
        answer
    );
    let requests = worker.join().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("GET /v1/me "));
    assert!(!requests[0].contains("private-page"));
    assert!(requests[1].starts_with("POST /v1/decisions "));
    assert!(
        requests[1]
            .to_ascii_lowercase()
            .contains("x-jcode-jev-purpose: browser\r\n"),
        "browser decisions must use the gateway's separate browser budget"
    );
    let body: Value = serde_json::from_str(requests[1].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(body["questions"], Value::Object(browser_questions()));
    assert!(body["state"].is_string());
}

#[tokio::test]
async fn browser_capability_denial_prevents_page_upload() {
    for me in [
        json!({"capabilities": {"memory_jev": true}}),
        json!({"capabilities": {"browser_jev": false}}),
        json!({"capabilities": {"browser_jev": "true"}}),
    ] {
        let (base, worker) = mock_server(vec![(200, me.to_string(), vec![])]);
        let mut client = mock_client(&base, JevProvider::Jcode);
        client.purpose = JevPurpose::Browser;
        let error = client
            .evaluate(json!("private-page"), browser_questions())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("browser_jev"));
        let requests = worker.join().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("GET /v1/me "));
        assert!(!requests[0].contains("private-page"));
    }
}

#[tokio::test]
async fn browser_auth_billing_and_redirect_failures_never_retry_or_fallback() {
    for provider in [JevProvider::Jcode, JevProvider::OpenRouter] {
        for preflight in [false, true] {
            if preflight && provider != JevProvider::Jcode {
                continue;
            }
            for status in [401, 402, 403, 302, 307] {
                let mut replies = Vec::new();
                if provider == JevProvider::Jcode && !preflight {
                    replies.push((
                        200,
                        json!({"capabilities": {"browser_jev": true}}).to_string(),
                        vec![],
                    ));
                }
                replies.push((
                    status,
                    "private-provider-error test-route-secret".into(),
                    vec![("Location".into(), "http://127.0.0.1:1/never-follow".into())],
                ));
                let expected_requests = replies.len();
                let (base, worker) = mock_server(replies);
                let mut client = mock_client(&base, provider);
                client.purpose = JevPurpose::Browser;
                let error = client
                    .evaluate(json!("private-page"), browser_questions())
                    .await
                    .unwrap_err();
                let detail = format!("{error:#}");
                assert!(detail.contains(&status.to_string()));
                for secret in [
                    "private-provider-error",
                    "test-route-secret",
                    "private-page",
                ] {
                    assert!(!detail.contains(secret));
                }
                let requests = worker.join().unwrap();
                assert_eq!(requests.len(), expected_requests);
                if preflight {
                    assert!(!requests[0].contains("private-page"));
                }
            }
        }
    }
}

#[tokio::test]
async fn subscription_requires_live_capability_then_sends_bound_bearer() {
    let (base, worker) = mock_server(vec![
        (
            200,
            json!({"capabilities": {"memory_jev": true}}).to_string(),
            vec![],
        ),
        (200, response().to_string(), vec![]),
    ]);
    let client = mock_client(&base, JevProvider::Jcode);
    let value = client.evaluate(json!("state"), questions()).await.unwrap();
    assert_eq!(value, response());
    let requests = worker.join().unwrap();
    assert!(requests[0].starts_with("GET /v1/me "));
    assert!(requests[1].starts_with("POST /v1/decisions "));
    for request in requests {
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer test-route-secret\r\n")
        );
        assert!(!request.lines().next().unwrap().contains("secret"));
    }
}

#[tokio::test]
async fn missing_or_false_capability_never_posts_decisions() {
    for me in [
        json!({"tier": "flagship", "status": "active"}),
        json!({"capabilities": {"memory_jev": false}}),
        json!({"capabilities": {"memory_jev": "true"}}),
    ] {
        let (base, worker) = mock_server(vec![(200, me.to_string(), vec![])]);
        let client = mock_client(&base, JevProvider::Jcode);
        let error = client
            .evaluate(json!("state"), questions())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("memory_jev"));
        assert_eq!(worker.join().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn transient_overload_is_retried_on_same_route_then_succeeds() {
    for status in [429, 502, 503, 504, 529] {
        let (base, worker) = mock_server(vec![
            (
                status,
                "{}".into(),
                vec![("Retry-After".into(), "1".into())],
            ),
            (status, "{}".into(), vec![]),
            (200, response().to_string(), vec![]),
        ]);
        let client = mock_client(&base, JevProvider::OpenRouter);
        let value = client.evaluate(json!("state"), questions()).await.unwrap();
        assert_eq!(value, response());
        let requests = worker.join().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(
            requests
                .iter()
                .all(|r| r.starts_with("POST /v1/decisions "))
        );
    }
}

#[tokio::test]
async fn gateway_plan_quota_is_not_retried_and_becomes_upgrade_prompt() {
    let quota = json!({"error": {
        "code": "quota_exceeded", "message": "server text", "purpose": "browser", "scope": "day",
        "limit": 2000, "tier": "plus", "upgrade_tier": "pro",
        "upgrade_url": "https://jcode.sh/pricing", "resets_at": "2026-09-27T00:00:00.000Z"
    }});
    let (base, worker) = mock_server(vec![
        (
            200,
            json!({"capabilities": {"browser_jev": true}}).to_string(),
            vec![],
        ),
        (
            429,
            quota.to_string(),
            vec![("Retry-After".into(), "80000".into())],
        ),
    ]);
    let mut client = mock_client(&base, JevProvider::Jcode);
    client.purpose = JevPurpose::Browser;
    let error = client
        .evaluate(json!({"page": "p"}), browser_questions())
        .await
        .unwrap_err();
    let notice = crate::subscription_notice::from_error(&error).expect("typed quota notice");
    assert_eq!(notice.feature, "browser");
    assert_eq!(notice.upgrade_tier.as_deref(), Some("pro"));
    assert!(error.to_string().contains("Upgrade to Pro"));
    assert!(error.to_string().contains("https://jcode.sh/pricing"));
    // One /me preflight and exactly one decision: plan quotas are never retried.
    assert_eq!(worker.join().unwrap().len(), 2);
}

#[tokio::test]
async fn gateway_quota_rejects_non_jcode_upgrade_links() {
    let quota = json!({"error": {"code": "quota_exceeded", "purpose": "memory", "tier": "plus",
        "upgrade_tier": "pro", "upgrade_url": "https://evil.example/pay"}});
    let (base, worker) = mock_server(vec![
        (
            200,
            json!({"capabilities": {"memory_jev": true}}).to_string(),
            vec![],
        ),
        (
            429,
            quota.to_string(),
            vec![("Retry-After".into(), "80000".into())],
        ),
    ]);
    let client = mock_client(&base, JevProvider::Jcode);
    let error = client.evaluate(json!("s"), questions()).await.unwrap_err();
    let notice = crate::subscription_notice::from_error(&error).unwrap();
    assert_eq!(notice.upgrade_url, None);
    assert!(!error.to_string().contains("evil.example"));
    worker.join().unwrap();
}

#[tokio::test]
async fn persistent_overload_fails_after_bounded_retries_without_echo() {
    for status in [429, 529] {
        let replies = (0..=TRANSIENT_RETRY_DELAYS.len())
            .map(|_| {
                (
                    status,
                    "private-provider-error test-route-secret".into(),
                    vec![],
                )
            })
            .collect();
        let (base, worker) = mock_server(replies);
        let client = mock_client(&base, JevProvider::OpenRouter);
        let detail = format!(
            "{:#}",
            client
                .evaluate(json!("private-state"), questions())
                .await
                .unwrap_err()
        );
        assert!(detail.contains(&status.to_string()));
        assert!(detail.contains("overloaded"));
        assert!(!detail.contains("test-route-secret"));
        assert!(!detail.contains("private-state"));
        assert_eq!(
            worker.join().unwrap().len(),
            TRANSIENT_RETRY_DELAYS.len() + 1
        );
    }
}

#[tokio::test]
async fn auth_billing_and_redirect_errors_are_redacted_and_never_retried() {
    for status in [401, 402, 403, 404, 500, 302, 307] {
        let headers = vec![("Location".into(), "http://127.0.0.1:1/never-follow".into())];
        let (base, worker) = mock_server(vec![(
            status,
            "private-provider-error test-route-secret".into(),
            headers,
        )]);
        let client = mock_client(&base, JevProvider::OpenRouter);
        let error = client
            .evaluate(json!("private-state"), questions())
            .await
            .unwrap_err();
        let detail = format!("{error:#}");
        assert!(detail.contains(&status.to_string()));
        assert!(!detail.contains("test-route-secret"));
        assert!(!detail.contains("private-provider-error"));
        assert!(!detail.contains("private-state"));
        assert_eq!(worker.join().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn oversized_and_invalid_json_responses_are_rejected_without_echo() {
    for body in [
        "x".repeat(MAX_RESPONSE_BYTES + 1),
        "private-response-invalid-json".into(),
    ] {
        let (base, worker) = mock_server(vec![(200, body, vec![])]);
        let client = mock_client(&base, JevProvider::TypeSafe);
        let error = client
            .evaluate(json!("state"), questions())
            .await
            .unwrap_err();
        assert!(!format!("{error:#}").contains("private-response"));
        worker.join().unwrap();
    }
}

#[tokio::test]
async fn streamed_body_limit_is_enforced_without_content_length() {
    let (base, worker) = mock_server(vec![(
        200,
        "x".repeat(MAX_RESPONSE_BYTES + 1),
        vec![("Transfer-Encoding".into(), "chunked".into())],
    )]);
    let client = mock_client(&base, JevProvider::Aimlapi);
    let error = client
        .evaluate(json!("state"), questions())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("bounded response size"));
    worker.join().unwrap();
}

#[tokio::test]
async fn failed_preflight_does_not_send_decision_request() {
    for (status, body, headers) in [
        (401, "private-account-response".into(), vec![]),
        (200, "x".repeat(MAX_ME_BYTES + 1), vec![]),
        (
            307,
            "private-account-response".into(),
            vec![("Location".into(), "https://untrusted.example/me".into())],
        ),
    ] {
        let (base, worker) = mock_server(vec![(status, body, headers)]);
        let client = mock_client(&base, JevProvider::Jcode);
        let error = client
            .evaluate(json!("state"), questions())
            .await
            .unwrap_err();
        assert!(!format!("{error:#}").contains("private-account-response"));
        let requests = worker.join().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("GET /v1/me "));
    }
}

#[tokio::test]
async fn laya_local_and_private_requests_bypass_http_proxy_including_voice_hedges() {
    let _guard = crate::storage::lock_test_env();
    let proxy_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    proxy_listener.set_nonblocking(true).unwrap();
    let proxy_url = format!("http://{}", proxy_listener.local_addr().unwrap());

    let (base, seen) = concurrent_mock(vec![(Duration::from_secs(3), 200), (Duration::ZERO, 200)]);

    let prev_http_proxy = std::env::var_os("HTTP_PROXY");
    let prev_all_proxy = std::env::var_os("ALL_PROXY");
    let prev_no_proxy = std::env::var_os("NO_PROXY");
    let prev_voice_provider = std::env::var_os(VOICE_PROVIDER_ENV);
    let prev_laya_base = std::env::var_os(LAYA_BASE_URL_ENV);
    let prev_laya_key = std::env::var_os("LAYA_API_KEY");

    crate::env::set_var("HTTP_PROXY", &proxy_url);
    crate::env::set_var("ALL_PROXY", &proxy_url);
    crate::env::remove_var("NO_PROXY");
    crate::env::set_var(VOICE_PROVIDER_ENV, "laya");
    crate::env::set_var(LAYA_BASE_URL_ENV, &base);
    crate::env::set_var("LAYA_API_KEY", "laya-private-key");

    assert!(should_bypass_proxy(
        JevProvider::Laya,
        "http://127.0.0.1:8080/v1/systemone"
    ));
    assert!(should_bypass_proxy(
        JevProvider::Laya,
        "http://100.87.178.69:8000/v1/systemone"
    ));
    assert!(!should_bypass_proxy(
        JevProvider::TypeSafe,
        "https://api.typesafe.ai/v1/systemone"
    ));

    let client = JevClient::for_voice().unwrap();
    let value = client
        .evaluate(json!({"transcript": "private-voice-state"}), questions())
        .await;

    match prev_http_proxy {
        Some(v) => crate::env::set_var("HTTP_PROXY", v),
        None => crate::env::remove_var("HTTP_PROXY"),
    }
    match prev_all_proxy {
        Some(v) => crate::env::set_var("ALL_PROXY", v),
        None => crate::env::remove_var("ALL_PROXY"),
    }
    match prev_no_proxy {
        Some(v) => crate::env::set_var("NO_PROXY", v),
        None => crate::env::remove_var("NO_PROXY"),
    }
    match prev_voice_provider {
        Some(v) => crate::env::set_var(VOICE_PROVIDER_ENV, v),
        None => crate::env::remove_var(VOICE_PROVIDER_ENV),
    }
    match prev_laya_base {
        Some(v) => crate::env::set_var(LAYA_BASE_URL_ENV, v),
        None => crate::env::remove_var(LAYA_BASE_URL_ENV),
    }
    match prev_laya_key {
        Some(v) => crate::env::set_var("LAYA_API_KEY", v),
        None => crate::env::remove_var("LAYA_API_KEY"),
    }

    let value = value.unwrap();
    assert!(validate_answers(&value, &questions()).is_ok());
    assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert!(
        proxy_listener.accept().is_err(),
        "capture proxy must never receive local Laya primary or hedge requests"
    );
}

#[tokio::test]
async fn voice_later_batch_errors_never_return_partial_classification() {
    use crate::voice_intent::{SessionCandidate, classify_with_client, describe_questions};
    let offered: Vec<_> = (0..20)
        .map(|index| SessionCandidate {
            id: format!("session-{index}"),
            title: format!("Conversation {index}"),
            working_dir: None,
        })
        .collect();
    let questions = describe_questions("next conversation", &offered).unwrap();
    let first: Map<String, Value> = questions[..MAX_QUESTIONS]
        .iter()
        .map(|q| {
            (
                q.id.clone(),
                json!({"type": "noul", "noul": if q.id == "coding_agent" { 0.99 } else { 0.01 }}),
            )
        })
        .collect();
    let last: Map<String, Value> = questions[MAX_QUESTIONS..]
        .iter()
        .map(|q| (q.id.clone(), json!({"type": "noul", "noul": 0.01})))
        .collect();
    let mut failures = vec![
        (500, "{}".into(), vec![]),
        (200, json!({"answers": {}}).to_string(), vec![]),
    ];
    for invalid in [
        json!({"type": "noul", "noul": 1.1}),
        json!({"type": "noul", "noul": "0.99"}),
        json!({"type": "choice", "choice": "yes"}),
    ] {
        let mut bad = last.clone();
        bad.insert(questions[MAX_QUESTIONS].id.clone(), invalid);
        failures.push((200, json!({"answers": bad}).to_string(), vec![]));
    }
    let mut wrong_ids = last.clone();
    wrong_ids.remove(&questions[MAX_QUESTIONS].id);
    wrong_ids.insert("invented".into(), json!({"type": "noul", "noul": 0.99}));
    failures.push((200, json!({"answers": wrong_ids}).to_string(), vec![]));
    for failure in failures {
        // Batches run concurrently, so reply by which batch was requested.
        let later = questions[MAX_QUESTIONS].id.clone();
        let good = json!({"answers": first}).to_string();
        let mut failure = Some(failure);
        let (base, worker) = mock_server_with(2, move |request| {
            let body: Value =
                serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
            if body["questions"].get(&later).is_some() {
                failure.take().unwrap()
            } else {
                (200, good.clone(), vec![])
            }
        });
        assert!(
            classify_with_client(
                "next conversation",
                &offered,
                &mock_client(&base, JevProvider::OpenRouter)
            )
            .await
            .is_err()
        );
        assert_eq!(worker.join().unwrap().len(), 2);
    }
}

#[test]
fn suggestions_selector_and_openjev_route_are_isolated() {
    assert_eq!(
        JevPurpose::Suggestions
            .selector_with(
                |key| {
                    assert_eq!(key, SUGGESTIONS_PROVIDER_ENV);
                    Ok("   ".into())
                },
                || "laya".into(),
            )
            .unwrap(),
        "laya"
    );
    assert_eq!(
        JevPurpose::Suggestions
            .selector_with(|_| Err(std::env::VarError::NotPresent), || "  ".into())
            .unwrap(),
        "auto"
    );
    assert!(
        JevPurpose::Suggestions
            .selector_with(
                |_| Err(std::env::VarError::NotUnicode("invalid".into())),
                || "auto".into(),
            )
            .is_err()
    );
    let (provider, key) = resolve_with("openjev", |env, file| {
        assert_eq!((env, file), ("OPENJEV_API_KEY", "openjev.env"));
        Some("oje_secret".into())
    })
    .unwrap();
    assert_eq!(provider, JevProvider::OpenJev);
    assert_eq!(key, "oje_secret");
    assert_eq!(
        provider.endpoint("").unwrap(),
        "https://api.openjev.sh/v1/systemone"
    );
    assert_eq!(provider.model(), "openjev");
}

#[test]
fn choice_answer_validation_checks_criteria_and_probabilities() {
    let q = browser_questions();
    assert!(
        validate_answers(
            &json!({
                "answers": {
                    "action": {
                        "type": "choice",
                        "choice": "click",
                        "confidence": 0.85,
                        "probabilities": {"click": 0.85, "stop": 0.15}
                    }
                }
            }),
            &q
        )
        .is_ok()
    );
    assert!(
        validate_answers(
            &json!({"answers": {"action": {"type": "choice", "choice": "unknown"}}}),
            &q
        )
        .is_err()
    );
    assert!(
        validate_answers(
            &json!({
                "answers": {
                    "action": {
                        "type": "choice",
                        "choice": "click",
                        "probabilities": {"invented": 0.5}
                    }
                }
            }),
            &q
        )
        .is_err()
    );
}
