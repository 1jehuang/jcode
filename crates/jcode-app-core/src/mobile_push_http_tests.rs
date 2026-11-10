use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn fixture(
    status: u16,
    body: &'static str,
    delay: Duration,
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    fixture_with_body_delay(status, body, delay, Duration::ZERO).await
}

async fn fixture_with_body_delay(
    status: u16,
    body: &'static str,
    delay: Duration,
    body_delay: Duration,
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let connections = Arc::new(AtomicUsize::new(0));
    let count = connections.clone();
    let task = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            socket.set_nodelay(true).unwrap();
            count.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                // Artificial connection setup cost, not an APNs latency model.
                tokio::time::sleep(delay).await;
                loop {
                    let mut request = Vec::new();
                    let mut byte = [0];
                    while !request.ends_with(b"\r\n\r\n") {
                        if socket.read_exact(&mut byte).await.is_err() {
                            return;
                        }
                        request.push(byte[0]);
                    }
                    let headers = String::from_utf8(request).unwrap();
                    assert!(headers.starts_with("POST /push/send HTTP/1.1\r\n"));
                    assert!(
                        headers
                            .to_lowercase()
                            .contains("authorization: bearer fixture-key\r\n")
                    );
                    let length: usize = headers
                        .lines()
                        .find_map(|line| {
                            line.to_lowercase()
                                .strip_prefix("content-length: ")
                                .map(str::to_owned)
                        })
                        .unwrap()
                        .parse()
                        .unwrap();
                    let mut bytes = vec![0; length];
                    socket.read_exact(&mut bytes).await.unwrap();
                    let actual: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                    assert_eq!(actual, serde_json::to_value(payload()).unwrap());
                    let response = format!(
                        "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                        body.len()
                    );
                    if socket.write_all(response.as_bytes()).await.is_err() {
                        return;
                    }
                    tokio::time::sleep(body_delay).await;
                    if socket.write_all(body.as_bytes()).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    (base, connections, task)
}

fn payload() -> PushPayload {
    build_payload(
        PushKind::Wake,
        "fixture-session",
        Some("Fixture"),
        "Fixture body",
        Some(42),
    )
}

#[tokio::test]
async fn mobile_push_http_eight_push_connections() {
    // Control mirrors the original per-notification client construction.
    let (base, connections, task) =
        fixture(200, r#"{"sent":1,"devices":1}"#, Duration::from_millis(60)).await;
    let start = Instant::now();
    for _ in 0..8 {
        let reply = reqwest::Client::builder()
            .timeout(SEND_TIMEOUT)
            .build()
            .unwrap()
            .post(format!("{base}/push/send"))
            .bearer_auth("fixture-key")
            .json(&payload())
            .send()
            .await
            .unwrap()
            .json::<SendReply>()
            .await
            .unwrap();
        assert_eq!((reply.sent, reply.devices), (Some(1), Some(1)));
    }
    eprintln!(
        "artificial 60ms connection-delay fresh-client control: eight pushes elapsed_ms={} connections={}",
        start.elapsed().as_millis(),
        connections.load(Ordering::SeqCst)
    );
    assert_eq!(connections.load(Ordering::SeqCst), 8);
    task.abort();
    let (base, connections, task) =
        fixture(200, r#"{"sent":1,"devices":1}"#, Duration::from_millis(60)).await;
    let start = Instant::now();
    for _ in 0..8 {
        let result = send("fixture-key", &base, &payload()).await;
        assert_eq!(
            result,
            (
                "delivered",
                Some("sent=Some(1) devices=Some(1)".into()),
                Some(200)
            )
        );
    }
    eprintln!(
        "artificial 60ms connection-delay fixture: eight pushes elapsed_ms={} connections={}",
        start.elapsed().as_millis(),
        connections.load(Ordering::SeqCst)
    );
    assert_eq!(connections.load(Ordering::SeqCst), 1);
    for _ in 0..8 {
        assert!(
            send_mobile_resume("fixture-key", &base, &payload())
                .await
                .unwrap()
                .contains("sent to 1 device")
        );
    }
    assert_eq!(connections.load(Ordering::SeqCst), 1);
    task.abort();
}

#[tokio::test]
async fn mobile_push_http_reply_semantics() {
    for (status, body, outcome) in [
        (200, r#"{"sent":0,"devices":0}"#, "no_devices"),
        (200, r#"{"sent":0,"devices":1}"#, "undelivered"),
        (200, "invalid", "error"),
        (503, "fixture unavailable", "rejected"),
    ] {
        let (base, _, task) = fixture(status, body, Duration::ZERO).await;
        let result = send("fixture-key", &base, &payload()).await;
        assert_eq!(result.0, outcome);
        assert_eq!(result.2, Some(status));
        task.abort();
    }
}

#[tokio::test]
async fn mobile_push_http_deadline_and_cancellation() {
    let (base, _, task) = fixture(
        200,
        r#"{"sent":1,"devices":1}"#,
        SEND_TIMEOUT + Duration::from_secs(1),
    )
    .await;
    let start = Instant::now();
    let result = send("fixture-key", &base, &payload()).await;
    assert_eq!(result.0, "error");
    assert_eq!(result.2, None);
    assert!(start.elapsed() >= SEND_TIMEOUT);
    assert!(start.elapsed() < SEND_TIMEOUT + Duration::from_secs(1));
    let p = payload();
    let future = send("fixture-key", &base, &p);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), future)
            .await
            .is_err()
    );
    task.abort();
}

#[tokio::test]
async fn mobile_push_http_body_deadline() {
    let (base, _, task) = fixture_with_body_delay(
        200,
        r#"{"sent":1,"devices":1}"#,
        Duration::ZERO,
        SEND_TIMEOUT + Duration::from_secs(1),
    )
    .await;
    let p = payload();
    let (automatic, explicit) = tokio::join!(
        send("fixture-key", &base, &p),
        send_mobile_resume("fixture-key", &base, &p),
    );
    assert_eq!(automatic.0, "error");
    assert_eq!(automatic.2, Some(200));
    assert!(explicit.is_err());
    task.abort();
}
