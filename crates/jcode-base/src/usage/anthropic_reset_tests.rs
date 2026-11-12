use super::*;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Serve canned responses in order and return the raw requests received.
async fn server(responses: Vec<(u16, Value)>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
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
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
        requests
    });
    (base, task)
}

fn status(available: bool, next: Option<&str>) -> Value {
    json!({
        "five_hour": {"utilization": 100.0, "resets_at": "2099-01-01T00:00:00Z"},
        "juniper_tide": {
            "eligible": true, "arm": "reset", "available": available,
            "next_available_at": next, "resets_per_week": 1
        }
    })
}

fn profile() -> Value {
    json!({
        "account": {"email": "person@example.com"},
        "organization": {"uuid": "0b5c7e36-1d1a-4a4c-9f49-5f7d5e6b8a10"}
    })
}

async fn prepare(
    base: &str,
) -> Result<std::result::Result<PendingAnthropicLimitReset, AnthropicLimitResetUnavailable>> {
    prepare_with_token(
        &reset_client().unwrap(),
        base,
        "test-access-secret".into(),
        Some("claude-otter".into()),
    )
    .await
}

#[test]
fn offer_requires_an_eligible_reset_arm() {
    let offer = offer_from_status(&status(true, None), Some("work")).unwrap();
    assert!(offer.available);
    assert_eq!(offer.account_label.as_deref(), Some("work"));
    assert_eq!(offer.resets_per_week, 1);
    for block in [
        json!({"eligible": false, "arm": "reset", "available": true}),
        json!({"eligible": true, "arm": "control", "available": true}),
        json!({"eligible": true, "available": true}),
    ] {
        assert!(offer_from_status(&json!({ "juniper_tide": block }), None).is_none());
    }
    assert!(offer_from_status(&json!({}), None).is_none());
    assert!(offer_from_status(&json!({"juniper_tide": null}), None).is_none());
    // A malformed next-available time is dropped rather than displayed.
    let spent = offer_from_status(&status(false, Some("soon")), None).unwrap();
    assert!(!spent.available);
    assert!(spent.next_available_at.is_none());
}

#[tokio::test]
async fn preparation_is_read_only_and_pins_the_organization() {
    let (base, task) = server(vec![(200, status(true, None)), (200, profile())]).await;
    let pending = prepare(&base).await.unwrap().unwrap();
    assert_eq!(
        pending.organization_uuid,
        "0b5c7e36-1d1a-4a4c-9f49-5f7d5e6b8a10"
    );
    assert_eq!(pending.account_label(), Some("claude-otter"));
    assert!(pending.account_display().contains("person@example.com"));
    assert!(!format!("{pending:?}").contains("secret"));
    let requests = task.await.unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("GET /api/oauth/usage?at_wall=1&skip_spend=1 HTTP/1.1"));
    assert!(requests[1].starts_with("GET /api/oauth/profile HTTP/1.1"));
    for request in &requests {
        assert!(request.contains("authorization: Bearer test-access-secret"));
    }
}

#[tokio::test]
async fn spent_or_absent_offers_never_reach_the_profile_or_claim() {
    let (base, task) = server(vec![(200, status(false, Some("2099-01-08T00:00:00Z")))]).await;
    assert_eq!(
        prepare(&base).await.unwrap().unwrap_err(),
        AnthropicLimitResetUnavailable::Spent {
            next_available_at: Some("2099-01-08T00:00:00Z".into())
        }
    );
    assert_eq!(task.await.unwrap().len(), 1);

    let (base, task) = server(vec![(200, json!({"five_hour": {"utilization": 100.0}}))]).await;
    assert_eq!(
        prepare(&base).await.unwrap().unwrap_err(),
        AnthropicLimitResetUnavailable::NotOffered
    );
    assert_eq!(task.await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_login_without_an_organization_cannot_be_prepared() {
    for organization in [
        json!(null),
        json!({"uuid": ""}),
        json!({"uuid": "../escape"}),
    ] {
        let (base, task) = server(vec![
            (200, status(true, None)),
            (200, json!({ "organization": organization })),
        ])
        .await;
        assert!(prepare(&base).await.is_err());
        task.await.unwrap();
    }
}

#[tokio::test]
async fn claim_posts_the_program_to_the_pinned_organization() {
    let (base, task) = server(vec![
        (200, status(true, None)),
        (200, profile()),
        (503, json!({"sensitive": "do-not-display"})),
        (
            200,
            json!({"result": "already_used", "next_available_at": "2099-01-08T00:00:00Z"}),
        ),
    ])
    .await;
    let client = reset_client().unwrap();
    let pending = prepare(&base).await.unwrap().unwrap();
    let error = consume_with_client(&client, &base, &pending)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("uncertain"));
    assert!(!error.contains("do-not-display"));
    let outcome = consume_with_client(&client, &base, &pending).await.unwrap();
    assert!(!outcome.limits_cleared());
    assert!(outcome.message().contains("already used"));
    let requests = task.await.unwrap();
    for claim in &requests[2..] {
        assert!(claim.starts_with(
            "POST /api/organizations/0b5c7e36-1d1a-4a4c-9f49-5f7d5e6b8a10/reset_rate_limits HTTP/1.1"
        ));
        assert!(claim.ends_with(r#"{"program":"juniper_tide"}"#));
    }
}

#[test]
fn outcomes_distinguish_cleared_limits_and_unknown_results() {
    let parse = |value: Value| serde_json::from_value::<AnthropicLimitResetOutcome>(value).unwrap();
    let reset = parse(json!({"result": "reset"}));
    assert!(reset.limits_cleared());
    assert!(reset.message().contains("weekly limit still applies"));
    assert!(parse(json!({"result": "not_limited"})).limits_cleared());
    for result in ["ineligible", "unavailable", "some_future_result"] {
        assert!(!parse(json!({ "result": result })).limits_cleared());
    }
}
