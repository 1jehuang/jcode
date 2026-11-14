//! Uses a loopback HTTP fixture only. No live mobile API is contacted.

use super::*;
use std::sync::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Request {
    line: String,
    auth: String,
    body: serde_json::Value,
}

/// Answers each request with the next canned JSON body.
async fn api(
    replies: Vec<(u16, serde_json::Value)>,
) -> (String, tokio::task::JoinHandle<Vec<Request>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let mut seen = Vec::new();
        for (status, reply) in replies {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
                .await
                .unwrap()
                .unwrap();
            let mut raw = Vec::new();
            let (head_len, body_len) = loop {
                let mut chunk = [0u8; 4096];
                let n = socket.read(&mut chunk).await.unwrap();
                assert!(n > 0, "request ended early");
                raw.extend_from_slice(&chunk[..n]);
                if let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&raw[..end]).to_ascii_lowercase();
                    let len = head
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .map_or(0, |v| v.trim().parse::<usize>().unwrap());
                    break (end + 4, len);
                }
            };
            while raw.len() < head_len + body_len {
                let mut chunk = [0u8; 4096];
                let n = socket.read(&mut chunk).await.unwrap();
                assert!(n > 0, "body ended early");
                raw.extend_from_slice(&chunk[..n]);
            }
            let head = String::from_utf8_lossy(&raw[..head_len]).into_owned();
            seen.push(Request {
                line: head.lines().next().unwrap().to_string(),
                auth: head
                    .lines()
                    .find(|line| line.to_ascii_lowercase().starts_with("authorization:"))
                    .unwrap_or_default()
                    .to_string(),
                body: serde_json::from_slice(&raw[head_len..head_len + body_len]).unwrap(),
            });
            let body = reply.to_string();
            let response = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        }
        seen
    });
    (base, task)
}

#[derive(Default)]
struct FakeHost {
    live: Vec<LiveSession>,
    fail: Option<&'static str>,
    delivered: Mutex<Vec<(String, String)>>,
}

impl LocalSessionHost for FakeHost {
    fn live_sessions(&self) -> BoxFuture<'_, Vec<LiveSession>> {
        Box::pin(async { self.live.clone() })
    }
    fn deliver<'a>(&'a self, session_id: &'a str, text: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if let Some(message) = self.fail {
                anyhow::bail!(message);
            }
            self.delivered
                .lock()
                .unwrap()
                .push((session_id.to_string(), text.to_string()));
            Ok(())
        })
    }
}

fn live(id: &str, busy: bool) -> LiveSession {
    LiveSession {
        session_id: id.to_string(),
        busy,
    }
}

fn message(session: &str, text: &str) -> serde_json::Value {
    serde_json::json!({"command": {"command_id": "cmd-1", "kind": "message", "session_id": session,
        "text": text, "status": "running", "target": "local"}})
}

#[tokio::test]
async fn reports_live_sessions_and_delivers_the_claimed_message_once() {
    let (base, server) = api(vec![
        (200, message("session_b", "run the tests")),
        (200, serde_json::json!({"command": {"status": "done"}})),
    ])
    .await;
    let host = FakeHost {
        live: vec![live("session_a", false), live("session_b", true)],
        ..Default::default()
    };
    let ran = run_local_once(
        &reqwest::Client::new(),
        &base,
        "fixture-key",
        "laptop",
        &host,
    )
    .await
    .unwrap();
    assert!(ran);
    assert_eq!(
        *host.delivered.lock().unwrap(),
        vec![("session_b".to_string(), "run the tests".to_string())]
    );
    let requests = server.await.unwrap();
    assert_eq!(
        requests[0].line,
        "POST /mobile/commands/claim-local HTTP/1.1"
    );
    assert!(requests[0].auth.ends_with("Bearer fixture-key"));
    assert_eq!(
        requests[0].body,
        serde_json::json!({"machine": "laptop", "sessions": [
            {"session_id": "session_b", "busy": true},
            {"session_id": "session_a", "busy": false}]})
    );
    assert_eq!(
        requests[1].line,
        "POST /mobile/commands/cmd-1/complete HTTP/1.1"
    );
    assert_eq!(
        requests[1].body,
        serde_json::json!({"status": "done", "session_id": "session_b"})
    );
}

#[tokio::test]
async fn idle_heartbeat_claims_nothing_and_no_live_sessions_means_no_request() {
    let (base, server) = api(vec![(200, serde_json::json!({"command": null}))]).await;
    let host = FakeHost {
        live: vec![live("session_a", false)],
        ..Default::default()
    };
    let client = reqwest::Client::new();
    assert!(
        !run_local_once(&client, &base, "k", "laptop", &host)
            .await
            .unwrap()
    );
    assert_eq!(server.await.unwrap().len(), 1);
    assert!(host.delivered.lock().unwrap().is_empty());

    // An unroutable base proves no request is attempted without live sessions.
    let nothing = FakeHost::default();
    assert!(
        !run_local_once(&client, "http://127.0.0.1:1", "k", "laptop", &nothing)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn failed_delivery_is_reported_instead_of_dropped() {
    let (base, server) = api(vec![
        (200, message("session_a", "hello")),
        (200, serde_json::json!({"command": {"status": "failed"}})),
    ])
    .await;
    let host = FakeHost {
        live: vec![live("session_a", false)],
        fail: Some("that session cannot take a message right now"),
        ..Default::default()
    };
    assert!(
        run_local_once(&reqwest::Client::new(), &base, "k", "laptop", &host)
            .await
            .unwrap()
    );
    let requests = server.await.unwrap();
    assert_eq!(
        requests[1].body,
        serde_json::json!({"status": "failed", "error": "that session cannot take a message right now"})
    );
}

#[tokio::test]
async fn refuses_commands_for_other_sessions_and_other_kinds() {
    for (command, expected) in [
        (
            message("session_elsewhere", "hello"),
            "that session is not running on this computer",
        ),
        (
            serde_json::json!({"command": {"command_id": "cmd-1", "kind": "create", "text": "new"}}),
            "this computer only accepts messages for running sessions",
        ),
        (message("session_a", "   "), "empty message"),
    ] {
        let (base, server) = api(vec![(200, command), (200, serde_json::json!({}))]).await;
        let host = FakeHost {
            live: vec![live("session_a", false)],
            ..Default::default()
        };
        assert!(
            run_local_once(&reqwest::Client::new(), &base, "k", "laptop", &host)
                .await
                .unwrap()
        );
        assert!(host.delivered.lock().unwrap().is_empty());
        let requests = server.await.unwrap();
        assert_eq!(
            requests[1].body,
            serde_json::json!({"status": "failed", "error": expected})
        );
    }
}

#[tokio::test]
async fn api_errors_surface_without_delivering() {
    let (base, server) = api(vec![(
        402,
        serde_json::json!({"error": {"code": "subscription_required"}}),
    )])
    .await;
    let host = FakeHost {
        live: vec![live("session_a", false)],
        ..Default::default()
    };
    let error = run_local_once(&reqwest::Client::new(), &base, "k", "laptop", &host)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("402"));
    server.await.unwrap();
    assert!(host.delivered.lock().unwrap().is_empty());
}

#[test]
fn report_keeps_busy_sessions_under_the_cap_and_drops_invalid_ids() {
    let mut sessions: Vec<_> = (0..120)
        .map(|i| live(&format!("session_{i:03}"), false))
        .collect();
    sessions.push(live("session_busy", true));
    sessions.push(live("session_busy", true));
    sessions.push(live("session_busy", false));
    sessions.push(live("bad id", true));
    let reported = report(sessions);
    assert_eq!(reported.len(), MAX_REPORTED_SESSIONS);
    assert_eq!(reported[0], live("session_busy", true));
    assert_eq!(
        reported
            .iter()
            .filter(|s| s.session_id == "session_busy")
            .count(),
        1
    );
    assert!(reported.iter().all(|s| s.session_id != "bad id"));
}
