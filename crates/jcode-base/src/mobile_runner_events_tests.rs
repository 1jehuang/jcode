use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{WebSocketStream, accept_hdr_async};

const HINT: &str = r#"{"type":"commands_available"}"#;
const FALLBACK: Duration = Duration::from_secs(5);

async fn fixture() -> (Wakeups, WebSocketStream<TcpStream>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let mut wakeups = spawn_with(
        Arc::new(move || Some((base.clone(), "fixture-only-key".into()))),
        Duration::from_secs(30),
    );
    let (stream, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let socket = accept_hdr_async(
        stream,
        |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
            assert_eq!(
                request.headers()["authorization"],
                "Bearer fixture-only-key"
            );
            assert_eq!(request.uri().path(), "/mobile/commands/events");
            assert!(request.uri().query().is_none());
            Ok(response)
        },
    )
    .await
    .unwrap();
    // The subscription itself intentionally generates one queue recheck.
    tokio::time::timeout(Duration::from_secs(1), wakeups.wait(FALLBACK))
        .await
        .unwrap();
    (wakeups, socket)
}

#[tokio::test]
async fn changed_source_closes_old_socket_and_authenticates_only_new_endpoint() {
    use std::sync::Mutex;
    let old = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let new = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let source = Arc::new(Mutex::new(Some((
        format!("http://{}", old.local_addr().unwrap()),
        "fixture-old-account".to_string(),
    ))));
    let current = source.clone();
    let mut wakeups = spawn_with(
        Arc::new(move || current.lock().unwrap().clone()),
        Duration::from_secs(30),
    );
    let (stream, _) = tokio::time::timeout(Duration::from_secs(2), old.accept())
        .await
        .unwrap()
        .unwrap();
    let mut old_socket = accept_hdr_async(
        stream,
        |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
            assert_eq!(
                request.headers()["authorization"],
                "Bearer fixture-old-account"
            );
            Ok(response)
        },
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(1), wakeups.wait(FALLBACK))
        .await
        .unwrap();
    *source.lock().unwrap() = Some((
        format!("http://{}", new.local_addr().unwrap()),
        "fixture-new-account".into(),
    ));
    // Scope checks run every five seconds. The bound includes scheduling margin,
    // not an assertion about connection latency or throughput.
    let (stream, _) = tokio::time::timeout(Duration::from_secs(7), new.accept())
        .await
        .expect("changed source must leave the old subscription and its cooldown")
        .unwrap();
    let mut new_socket = accept_hdr_async(
        stream,
        |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
            assert_eq!(
                request.headers()["authorization"],
                "Bearer fixture-new-account"
            );
            assert!(request.uri().query().is_none());
            Ok(response)
        },
    )
    .await
    .unwrap();
    let end = tokio::time::timeout(Duration::from_secs(1), old_socket.next())
        .await
        .expect("old account socket must be closed");
    assert!(!matches!(end, Some(Ok(Message::Text(_)))));
    tokio::time::timeout(Duration::from_secs(1), wakeups.wait(FALLBACK))
        .await
        .unwrap();
    new_socket.send(Message::Text(HINT.into())).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), wakeups.wait(FALLBACK))
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(80), old.accept())
            .await
            .is_err()
    );
}

#[test]
fn request_uses_bearer_header_without_credentials_in_url() {
    let request = event_request("https://example.invalid/api/", "fixture-only-key").unwrap();
    assert_eq!(
        request.uri().to_string(),
        "wss://example.invalid/api/mobile/commands/events"
    );
    assert_eq!(
        request.headers()["Authorization"],
        "Bearer fixture-only-key"
    );
    assert!(!request.uri().to_string().contains("fixture-only-key"));
    for base in [
        "https://user:password@example.invalid",
        "https://example.invalid?key=secret",
        "https://example.invalid#secret",
        "ftp://example.invalid",
        "not a URL",
    ] {
        assert!(event_request(base, "fixture-only-key").is_err(), "{base}");
    }
    assert!(event_request("https://example.invalid", "bad\r\nInjected: value").is_err());
}

#[test]
fn hints_accept_only_the_exact_non_authoritative_shape() {
    assert!(available(HINT));
    for text in [
        "",
        "null",
        "[]",
        "{}",
        r#"{"type":"other"}"#,
        r#"{"type":"commands_available","command":"execute"}"#,
        r#"{"type":"commands_available","type":"commands_available"}"#,
    ] {
        assert!(!available(text), "{text}");
    }
}

#[tokio::test]
async fn notification_wakes_before_five_second_fallback() {
    let (mut wakeups, mut socket) = fixture().await;
    socket.send(Message::Text(HINT.into())).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), wakeups.wait(FALLBACK))
        .await
        .expect("notification should wake within one second, not the five-second fallback");
}

#[tokio::test]
async fn notification_drives_a_real_poll_loop_claim_without_waiting_five_seconds() {
    let (wakeups, mut socket) = fixture().await;
    let queued = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let calls = Arc::new(AtomicUsize::new(0));
    let claimed = Arc::new(tokio::sync::Notify::new());
    let queue = queued.clone();
    let count = calls.clone();
    let signal = claimed.clone();
    let task = tokio::spawn(super::super::poll_commands_with_wait(
        move || {
            count.fetch_add(1, Ordering::SeqCst);
            let ran = queue.swap(false, Ordering::SeqCst);
            if ran {
                signal.notify_one();
            }
            async move { Ok(ran) }
        },
        wakeups,
    ));
    tokio::time::timeout(Duration::from_secs(1), async {
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let started = tokio::time::Instant::now();
    queued.store(true, Ordering::SeqCst);
    socket.send(Message::Text(HINT.into())).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), claimed.notified())
        .await
        .unwrap();
    println!(
        "Controlled localhost WebSocket hint to polling-loop claim: {:?}, not production latency",
        started.elapsed()
    );
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn notification_is_buffered_and_bursts_coalesce() {
    let (mut wakeups, mut socket) = fixture().await;
    let initial = *wakeups.receiver.borrow();
    for _ in 0..128 {
        socket.send(Message::Text(HINT.into())).await.unwrap();
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        while *wakeups.receiver.borrow() != initial + 128 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // All frames arrived before wait. A watch channel stores only the latest u64.
    tokio::time::timeout(Duration::from_secs(1), wakeups.wait(FALLBACK))
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(80), wakeups.wait(FALLBACK))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn dropped_socket_keeps_timer_fallback() {
    let (mut wakeups, socket) = fixture().await;
    drop(socket);
    assert!(
        tokio::time::timeout(
            Duration::from_millis(60),
            wakeups.wait(Duration::from_millis(180))
        )
        .await
        .is_err()
    );
    tokio::time::timeout(
        Duration::from_secs(1),
        wakeups.wait(Duration::from_millis(100)),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn dropping_wakeups_cancels_connection_task_and_socket() {
    let (wakeups, mut socket) = fixture().await;
    let task = wakeups.task.abort_handle();
    drop(wakeups);
    let result = tokio::time::timeout(Duration::from_secs(1), socket.next())
        .await
        .expect("dropping wakeups must close its websocket");
    assert!(!matches!(result, Some(Ok(Message::Text(_)))));
    tokio::time::timeout(Duration::from_secs(1), async {
        while !task.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn malformed_and_oversize_frames_fail_closed() {
    for frame in [
        Message::Text("not JSON".into()),
        Message::Text("x".repeat(4097)),
        Message::Binary(vec![0; 16]),
    ] {
        let (mut wakeups, mut socket) = fixture().await;
        socket.send(frame).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(100), wakeups.wait(FALLBACK))
                .await
                .is_err()
        );
        let end = tokio::time::timeout(Duration::from_secs(1), socket.next())
            .await
            .expect("invalid frames must terminate the connection");
        assert!(!matches!(end, Some(Ok(Message::Text(_)))));
    }
}

#[tokio::test(start_paused = true)]
async fn error_backoff_cannot_be_bypassed_by_buffered_hint() {
    let (sender, receiver) = watch::channel(0u64);
    let task = tokio::spawn(std::future::pending());
    let mut wakeups = Wakeups { receiver, task };
    sender.send_modify(|v| *v += 1);
    let start = tokio::time::Instant::now();
    wakeups.wait(Duration::from_secs(20)).await;
    assert!(start.elapsed() >= Duration::from_secs(20));
    // Backoff did not consume the pending queue hint.
    assert!(wakeups.receiver.has_changed().unwrap());
}

#[tokio::test]
async fn unavailable_endpoint_does_not_hot_loop_and_fallback_remains_live() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let attempts = Arc::new(AtomicUsize::new(0));
    let count = attempts.clone();
    let server = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            count.fetch_add(1, Ordering::SeqCst);
            let mut request = [0; 4096];
            stream.read(&mut request).await.unwrap();
            stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
        }
    });
    let mut wakeups = spawn_with(
        Arc::new(move || Some((base.clone(), "fixture-only-key".into()))),
        Duration::from_millis(1),
    );
    tokio::time::timeout(Duration::from_secs(1), async {
        while attempts.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let start = tokio::time::Instant::now();
    wakeups.wait(Duration::from_millis(150)).await;
    assert!(start.elapsed() >= Duration::from_millis(150));
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    server.abort();
}
