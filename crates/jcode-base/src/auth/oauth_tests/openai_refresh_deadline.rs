//! Issue #1757: OpenAI refresh network waits are bounded end to end, and a
//! cancelled caller cannot interrupt rotation -> persistence. Loopback servers
//! and dummy credentials only.
use super::*;
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

/// Read one HTTP request (headers and Content-Length body) from `stream`.
async fn read_request(stream: &mut tokio::net::TcpStream) -> String {
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
    reader.read_exact(&mut body).await.unwrap();
    String::from_utf8(body).unwrap()
}

fn token_url(listener: &TcpListener) -> String {
    format!("http://{}/oauth/token", listener.local_addr().unwrap())
}

fn success_body(access: &str, refresh: &str) -> String {
    serde_json::json!({
        "access_token": access,
        "refresh_token": refresh,
        "expires_in": 3600,
    })
    .to_string()
}

fn http_response(status: u16, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn sandbox() -> crate::auth::test_sandbox::AuthTestSandbox {
    crate::auth::test_sandbox::AuthTestSandbox::new().expect("auth test sandbox")
}

/// Store a dummy account and return its (possibly canonicalized) label.
fn seed_account(refresh: &str) -> String {
    crate::auth::codex::upsert_account_from_tokens("openai-1", "at_old", refresh, None, Some(1))
        .unwrap()
}

fn stored_refresh_token(label: &str) -> Option<String> {
    stored_openai_tokens(label).map(|tokens| tokens.refresh_token)
}

#[tokio::test]
async fn rejected_cached_token_does_not_block_newly_imported_account() {
    let _sandbox = sandbox();
    let label = seed_account("newly-imported-refresh");
    crate::auth::codex::update_account_tokens(
        &label,
        "old-access",
        "newly-imported-refresh",
        Some("original-id-token".into()),
        Some("original-account-id".into()),
        Some(1),
    )
    .unwrap();
    crate::auth::codex::update_account_profile(&label, Some("test@example.invalid".into()))
        .unwrap();
    crate::auth::refresh_state::record_permanent_rejection(
        "openai",
        "old-daemon-refresh",
        "invalid_grant",
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = token_url(&listener);
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        assert!(request.contains("newly-imported-refresh"));
        assert!(!request.contains("old-daemon-refresh"));
        let response = http_response(200, &success_body("new-access", "new-refresh"));
        stream.write_all(response.as_bytes()).await.unwrap();
    });
    let tokens = refresh_openai_tokens_at(
        &url,
        Duration::from_secs(2),
        "old-daemon-refresh",
        Some(label.clone()),
    )
    .await
    .unwrap();
    assert_eq!(tokens.access_token, "new-access");
    assert_eq!(stored_refresh_token(&label).as_deref(), Some("new-refresh"));
    let account = crate::auth::codex::list_accounts()
        .unwrap()
        .into_iter()
        .find(|account| account.label == label)
        .unwrap();
    assert_eq!(account.account_id.as_deref(), Some("original-account-id"));
    assert_eq!(account.email.as_deref(), Some("test@example.invalid"));
    assert_eq!(account.id_token.as_deref(), Some("original-id-token"));
    server.await.unwrap();
}

#[tokio::test]
async fn refresh_deadline_bounds_withheld_headers() {
    let _sandbox = sandbox();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = token_url(&listener);
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        // Accept the request, then never send a status line.
        std::future::pending::<()>().await;
        drop(stream);
    });

    let started = Instant::now();
    let err = refresh_openai_tokens_at(&url, Duration::from_millis(300), "rt_dummy", None)
        .await
        .expect_err("withheld headers must fail");
    let elapsed = started.elapsed();
    server.abort();

    // Hang detector: a broken deadline would never return while the server
    // stalls forever, so any bound far above the 300ms deadline still fails.
    // 30s leaves room for a starved runner under the parallel suite.
    assert!(elapsed < Duration::from_secs(30), "took {elapsed:?}");
    let message = format!("{err:#}");
    assert!(message.contains("timed out"), "{message}");
    assert!(message.contains("response headers"), "{message}");
    assert!(message.contains("rotation outcome is unknown"), "{message}");
    assert!(!message.contains("rt_dummy"), "{message}");
}

#[tokio::test]
async fn refresh_deadline_bounds_incomplete_body() {
    let _sandbox = sandbox();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = token_url(&listener);
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        // Promise a body longer than what is sent, then stall.
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 4096\r\n\r\n{\"access_token\":\"at_partial",
            )
            .await
            .unwrap();
        stream.flush().await.unwrap();
        std::future::pending::<()>().await;
        drop(stream);
    });

    // 2s instead of 300ms: the phase ("response body") only holds if the
    // server task got scheduled to write the partial response within the
    // deadline. 2s is still two orders of magnitude below the product's 30s
    // refresh bound, so an unbounded body read still fails the hang detector
    // below, but a starved runner no longer flips the phase to "response
    // headers".
    let started = Instant::now();
    let err = refresh_openai_tokens_at(&url, Duration::from_secs(2), "rt_dummy", None)
        .await
        .expect_err("an incomplete body must fail");
    server.abort();

    // Same hang detector as the withheld-headers case; the deadline above is
    // what is under test, this just proves the call returned at all.
    assert!(started.elapsed() < Duration::from_secs(30));
    let message = format!("{err:#}");
    assert!(message.contains("response body"), "{message}");
    assert!(!message.contains("at_partial"), "{message}");
}

#[tokio::test]
async fn refresh_deadline_failure_leaves_stored_token_and_releases_lock() {
    let _sandbox = sandbox();
    let label = seed_account("rt_seed");
    // One listener per exchange. A shared listener pairs connections by
    // arrival order, and under load the first exchange can miss its 200ms
    // deadline before its socket is ever accepted, so the second exchange's
    // request would pair with the stall branch and hang to its own deadline.
    // Separate ports make the pairing explicit.
    let stall_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ok_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let stall_url = token_url(&stall_listener);
    let ok_url = token_url(&ok_listener);
    let stalling = tokio::spawn(async move {
        // First exchange: read the request, then hold the socket open well
        // past the 200ms deadline without responding, then close.
        let (mut stalled, _) = stall_listener.accept().await.unwrap();
        read_request(&mut stalled).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        drop(stalled);
    });
    let serving = tokio::spawn(async move {
        // Second exchange succeeds, proving the per-account refresh lock was
        // released by the timed-out transaction.
        let (mut stream, _) = ok_listener.accept().await.unwrap();
        let body = read_request(&mut stream).await;
        assert!(body.contains("refresh_token=rt_seed"), "{body}");
        stream
            .write_all(http_response(200, &success_body("at_new", "rt_new")).as_bytes())
            .await
            .unwrap();
    });

    let err = refresh_openai_tokens_at(
        &stall_url,
        Duration::from_millis(200),
        "rt_seed",
        Some(label.clone()),
    )
    .await
    .expect_err("stalled refresh must time out");
    assert!(format!("{err:#}").contains("timed out"));
    // Uncertain outcome: never reset or rewrite the stored credential.
    assert_eq!(stored_refresh_token(&label).as_deref(), Some("rt_seed"));

    // Hang detector for a leaked refresh lock: the waiter would block on the
    // single-flight lock forever, so a generous bound still catches it while
    // tolerating a starved runner (first refresh teardown + round trip).
    // A leaked lock surfaces as the inner deadline firing first.
    let tokens = tokio::time::timeout(
        Duration::from_secs(30),
        refresh_openai_tokens_at(
            &ok_url,
            Duration::from_secs(15),
            "rt_seed",
            Some(label.clone()),
        ),
    )
    .await
    .expect("lock must be released after a timed-out refresh")
    .unwrap();
    assert_eq!(tokens.refresh_token, "rt_new");
    assert_eq!(stored_refresh_token(&label).as_deref(), Some("rt_new"));
    // Abort rather than await: if the first exchange timed out before its
    // socket was ever accepted (starved runner), `stalling` is still parked in
    // `accept()` and awaiting it would hang the test forever. All observable
    // effects were asserted above.
    stalling.abort();
    serving.abort();
}

#[tokio::test]
async fn refresh_cancelled_after_rotation_still_persists() {
    let _sandbox = sandbox();
    let label = seed_account("rt_seed");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = token_url(&listener);
    let (received_tx, received_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        // The server has now rotated: rt_seed is consumed.
        received_tx.send(()).unwrap();
        release_rx.await.unwrap();
        stream
            .write_all(http_response(200, &success_body("at_rotated", "rt_rotated")).as_bytes())
            .await
            .unwrap();
    });

    let caller_url = url.clone();
    let caller_label = label.clone();
    let caller = tokio::spawn(async move {
        refresh_openai_tokens_at(
            &caller_url,
            Duration::from_secs(5),
            "rt_seed",
            Some(caller_label),
        )
        .await
    });
    received_rx.await.unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    release_tx.send(()).unwrap();

    tokio::time::timeout(Duration::from_secs(5), async {
        while stored_refresh_token(&label).as_deref() != Some("rt_rotated") {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("rotated tokens must be persisted even though the caller was cancelled");
    server.await.unwrap();
}

#[tokio::test]
async fn refresh_waiter_cancellation_does_not_disturb_in_flight_owner() {
    let _sandbox = sandbox();
    let label = seed_account("rt_seed");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = token_url(&listener);
    let (received_tx, received_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        received_tx.send(()).unwrap();
        release_rx.await.unwrap();
        stream
            .write_all(http_response(200, &success_body("at_owner", "rt_owner")).as_bytes())
            .await
            .unwrap();
        // A second exchange would mean the waiter reused the consumed token.
        let second = tokio::time::timeout(Duration::from_millis(300), listener.accept()).await;
        assert!(second.is_err(), "waiter must not start its own refresh");
    });

    let owner_url = url.clone();
    let owner_label = label.clone();
    let owner = tokio::spawn(async move {
        refresh_openai_tokens_at(
            &owner_url,
            Duration::from_secs(5),
            "rt_seed",
            Some(owner_label),
        )
        .await
    });
    received_rx.await.unwrap();
    let waiter_url = url.clone();
    let waiter_label = label.clone();
    let waiter = tokio::spawn(async move {
        refresh_openai_tokens_at(
            &waiter_url,
            Duration::from_secs(5),
            "rt_seed",
            Some(waiter_label),
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    waiter.abort();
    let _ = waiter.await;
    release_tx.send(()).unwrap();

    let tokens = owner.await.unwrap().unwrap();
    assert_eq!(tokens.refresh_token, "rt_owner");
    assert_eq!(stored_refresh_token(&label).as_deref(), Some("rt_owner"));
    server.await.unwrap();
}

#[tokio::test]
async fn refresh_malformed_json_does_not_echo_body_or_persist() {
    let _sandbox = sandbox();
    let label = seed_account("rt_seed");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = token_url(&listener);
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        let body = r#"{"access_token":"at_secret_value","refresh_token":"#;
        stream
            .write_all(http_response(200, body).as_bytes())
            .await
            .unwrap();
    });

    let err =
        refresh_openai_tokens_at(&url, Duration::from_secs(5), "rt_seed", Some(label.clone()))
            .await
            .expect_err("malformed JSON must fail");
    server.await.unwrap();
    let message = format!("{err:#}");
    assert!(message.contains("malformed JSON"), "{message}");
    assert!(!message.contains("at_secret_value"), "{message}");
    assert_eq!(stored_refresh_token(&label).as_deref(), Some("rt_seed"));
}

#[tokio::test]
async fn refresh_error_response_is_reported_and_marks_rejection() {
    let _sandbox = sandbox();
    let label = seed_account("rt_seed");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = token_url(&listener);
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        stream
            .write_all(http_response(400, r#"{"error":"invalid_grant"}"#).as_bytes())
            .await
            .unwrap();
    });

    let err =
        refresh_openai_tokens_at(&url, Duration::from_secs(5), "rt_seed", Some(label.clone()))
            .await
            .expect_err("400 must fail");
    server.await.unwrap();
    assert!(format!("{err:#}").contains("invalid_grant"));
    assert_eq!(stored_refresh_token(&label).as_deref(), Some("rt_seed"));

    // A known-invalid refresh token is not retried against the network.
    let again =
        refresh_openai_tokens_at(&url, Duration::from_secs(5), "rt_seed", Some(label.clone()))
            .await
            .expect_err("rejected token must fail fast");
    assert!(!format!("{again:#}").contains("timed out"));
}
