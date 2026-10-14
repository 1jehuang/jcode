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

    assert!(elapsed < Duration::from_secs(5), "took {elapsed:?}");
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

    let started = Instant::now();
    let err = refresh_openai_tokens_at(&url, Duration::from_millis(300), "rt_dummy", None)
        .await
        .expect_err("an incomplete body must fail");
    server.abort();

    assert!(started.elapsed() < Duration::from_secs(5));
    let message = format!("{err:#}");
    assert!(message.contains("response body"), "{message}");
    assert!(!message.contains("at_partial"), "{message}");
}

#[tokio::test]
async fn refresh_deadline_failure_leaves_stored_token_and_releases_lock() {
    let _sandbox = sandbox();
    let label = seed_account("rt_seed");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = token_url(&listener);
    let server = tokio::spawn(async move {
        // First exchange stalls. The second succeeds, proving the per-account
        // refresh lock was released by the timed-out transaction.
        let (mut stalled, _) = listener.accept().await.unwrap();
        read_request(&mut stalled).await;
        let (mut stream, _) = listener.accept().await.unwrap();
        let body = read_request(&mut stream).await;
        assert!(body.contains("refresh_token=rt_seed"), "{body}");
        stream
            .write_all(http_response(200, &success_body("at_new", "rt_new")).as_bytes())
            .await
            .unwrap();
        drop(stalled);
    });

    let err = refresh_openai_tokens_at(
        &url,
        Duration::from_millis(200),
        "rt_seed",
        Some(label.clone()),
    )
    .await
    .expect_err("stalled refresh must time out");
    assert!(format!("{err:#}").contains("timed out"));
    // Uncertain outcome: never reset or rewrite the stored credential.
    assert_eq!(stored_refresh_token(&label).as_deref(), Some("rt_seed"));

    let tokens = tokio::time::timeout(
        Duration::from_secs(5),
        refresh_openai_tokens_at(&url, Duration::from_secs(5), "rt_seed", Some(label.clone())),
    )
    .await
    .expect("lock must be released after a timed-out refresh")
    .unwrap();
    assert_eq!(tokens.refresh_token, "rt_new");
    assert_eq!(stored_refresh_token(&label).as_deref(), Some("rt_new"));
    server.await.unwrap();
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
