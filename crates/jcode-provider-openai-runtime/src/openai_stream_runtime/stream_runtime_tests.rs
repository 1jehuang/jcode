use super::*;

fn dummy_credentials() -> Arc<RwLock<CodexCredentials>> {
    Arc::new(RwLock::new(CodexCredentials {
        access_token: "at_old".into(),
        refresh_token: "rt_old".into(),
        id_token: None,
        account_id: Some("acct_dummy".into()),
        expires_at: Some(1),
    }))
}

fn rotated_tokens() -> oauth::OAuthTokens {
    oauth::OAuthTokens {
        access_token: "at_new".into(),
        refresh_token: "rt_new".into(),
        expires_at: 42,
        id_token: None,
        scopes: Vec::new(),
    }
}

/// Issue #1757: once rotation has started, a cancelled caller must not
/// leave the consumed refresh token in memory.
#[tokio::test]
async fn refresh_commit_survives_caller_cancellation() {
    let credentials = dummy_credentials();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let caller_credentials = Arc::clone(&credentials);
    let caller = tokio::spawn(async move {
        refresh_and_commit_openai_tokens(&caller_credentials, async move {
            started_tx.send(()).unwrap();
            release_rx.await.unwrap();
            Ok(rotated_tokens())
        })
        .await
    });
    started_rx.await.unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    release_tx.send(()).unwrap();

    tokio::time::timeout(Duration::from_secs(2), async {
        while credentials.read().await.refresh_token != "rt_new" {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("rotated tokens must be committed in memory after caller cancellation");
    let creds = credentials.read().await;
    assert_eq!(creds.access_token, "at_new");
    assert_eq!(creds.account_id.as_deref(), Some("acct_dummy"));
    assert_eq!(creds.expires_at, Some(42));
}

#[tokio::test]
async fn refresh_failure_leaves_in_memory_credentials_untouched() {
    let credentials = dummy_credentials();
    let err = refresh_and_commit_openai_tokens(&credentials, async {
        anyhow::bail!("OpenAI token refresh timed out after 30s waiting for response headers")
    })
    .await
    .unwrap_err();
    assert!(err.to_string().contains("timed out"));
    let creds = credentials.read().await;
    assert_eq!(creds.access_token, "at_old");
    assert_eq!(creds.refresh_token, "rt_old");
}

#[test]
fn unauthorized_triggers_token_refresh() {
    assert!(should_refresh_token(StatusCode::UNAUTHORIZED, ""));
}

#[test]
fn forbidden_triggers_refresh_only_for_token_bodies() {
    assert!(should_refresh_token(
        StatusCode::FORBIDDEN,
        "access token expired"
    ));
    assert!(!should_refresh_token(
        StatusCode::FORBIDDEN,
        "region not allowed"
    ));
}

#[test]
fn refreshed_token_marker_is_retryable() {
    // After a 401/403 we force-refresh the OpenAI token and surface this
    // marker so the retry loop reconnects with the new credentials.
    assert!(is_retryable_error(
        "openai token refreshed, retrying: 401 unauthorized"
    ));
}

#[test]
fn missing_or_failed_refresh_is_not_retryable() {
    assert!(!is_retryable_error(
        "openai rejected the access token and no refresh token is available; run /login to re-authenticate: 401"
    ));
    assert!(!is_retryable_error(
        "openai token refresh failed; run /login to re-authenticate: network error"
    ));
}

#[test]
fn tls_transient_errors_are_retryable() {
    // Regression for issue #338: transient TLS faults must be retried on
    // the OpenAI path, matching every other provider. Callers pass the
    // error string already lowercased.
    assert!(is_retryable_error(
        "stream error: io error: received fatal alert: badrecordmac"
    ));
    assert!(is_retryable_error("received fatal alert: badrecordmac"));
    assert!(is_retryable_error("decryption failed or bad record mac"));
    assert!(is_retryable_error("tls handshake eof"));
    assert!(is_retryable_error("connection aborted"));
    assert!(is_retryable_error("temporary failure in name resolution"));
    assert!(is_retryable_error("no route to host"));
    assert!(is_retryable_error("network is unreachable"));
    // A send-level cause that callers now surface via the full anyhow
    // chain ({:#}) instead of the masked top-level context alone.
    assert!(is_retryable_error(
        "failed to send request to openai api: error sending request: received fatal alert: badrecordmac"
    ));
}

#[test]
fn rate_limit_is_retryable() {
    // Regression for issue #338 (gap #2): 429s should be retried, unifying
    // behavior with Anthropic/Copilot.
    assert!(is_retryable_error("429 too many requests"));
    assert!(is_retryable_error("rate limit exceeded"));
    assert!(is_retryable_error("rate_limit_exceeded"));
}

#[test]
fn auth_errors_remain_non_retryable() {
    assert!(!is_retryable_error("401 unauthorized"));
    assert!(!is_retryable_error("invalid api key"));
}
#[test]
fn cache_affinity_https_matches_websocket_session_header() {
    let client = Client::new();
    let mut credentials = CodexCredentials {
        access_token: "fixture-token".into(),
        refresh_token: "fixture-refresh".into(),
        id_token: None,
        account_id: Some("fixture-account".into()),
        expires_at: None,
    };
    for oauth in [true, false] {
        if !oauth {
            credentials.refresh_token.clear();
        }
        let http = response_http_request(
            &client,
            "https://example.test/responses",
            &credentials.access_token,
            oauth,
            credentials.account_id.as_deref(),
            Some("persisted-session"),
        )
        .json(&serde_json::json!({"prompt_cache_key":"persisted-session"}))
        .build()
        .unwrap();
        let ws = openai_websocket_prewarm::websocket_request(
            &credentials,
            &credentials.access_token,
            Some("persisted-session"),
        )
        .unwrap();
        assert_eq!(
            http.headers().get("session-id"),
            ws.headers().get("session-id")
        );
        assert_eq!(http.headers().contains_key("session-id"), oauth);
        assert_eq!(
            http.headers().get("chatgpt-account-id"),
            ws.headers().get("chatgpt-account-id")
        );
        assert_eq!(http.headers()["authorization"], "Bearer fixture-token");
        if oauth {
            assert_eq!(http.headers()["session-id"], "persisted-session");
        }
    }
}
