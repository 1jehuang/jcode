//! Wire-level coverage for the default client shared by the daemon mobile loops.
use super::*;
use crate::mobile_runner::{LiveSession, LocalSessionHost};
use std::future::Future;
use std::pin::Pin;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Host;
impl LocalSessionHost for Host {
    fn live_sessions(&self) -> Pin<Box<dyn Future<Output = Vec<LiveSession>> + Send + '_>> {
        Box::pin(async {
            vec![LiveSession {
                session_id: "session-mobile".into(),
                busy: true,
            }]
        })
    }
    fn deliver<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async { panic!("server never supplies a command") })
    }
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
    let mut bytes = Vec::new();
    loop {
        let mut buf = [0; 4096];
        let n = socket.read(&mut buf).await.unwrap();
        assert_ne!(n, 0);
        bytes.extend_from_slice(&buf[..n]);
        if let Some(end) = bytes.windows(4).position(|s| s == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
            let length = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
                .map_or(0, |n| n.parse::<usize>().unwrap());
            if bytes.len() >= end + 4 + length {
                return String::from_utf8(bytes).unwrap();
            }
        }
    }
}

// No response is sent until both requests arrive. A lock held across send/await
// would deadlock here, so this also exercises independent concurrent requests.
async fn pair(sync: reqwest::Client, runner: reqwest::Client) -> Vec<String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        let first_request = read_request(&mut first).await;
        let (mut second, _) = listener.accept().await.unwrap();
        let second_request = read_request(&mut second).await;
        let mut requests = Vec::new();
        for (mut socket, request) in [(first, first_request), (second, second_request)] {
            let body = if request.starts_with("GET ") {
                "{\"sessions\":[]}"
            } else {
                "{\"command\":null}"
            };
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nSet-Cookie: mobile=secret\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            // Host is the only intentionally varying part of the wire request.
            requests.push(
                request
                    .lines()
                    .filter(|line| !line.starts_with("host:"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
        requests.sort();
        requests
    });
    let requests = async {
        let (listing, claim) = tokio::join!(
            list_cloud_sessions_with_endpoint(&sync, &base, "sync-only-key"),
            crate::mobile_runner::run_local_once(
                &runner,
                &base,
                "runner-only-key",
                "test-machine",
                &Host
            ),
        );
        assert!(listing.unwrap().is_empty());
        assert!(!claim.unwrap());
        server.await.unwrap()
    };
    tokio::time::timeout(Duration::from_secs(5), requests)
        .await
        .unwrap()
}

#[tokio::test]
async fn mobile_http_defaults_preserve_wire_concurrency_and_credential_isolation() {
    let baseline = pair(reqwest::Client::new(), reqwest::Client::new()).await;
    let sync = daemon_http_client();
    let runner = daemon_http_client();
    for _ in 0..3 {
        let shared = pair(sync.clone(), runner.clone()).await;
        assert_eq!(shared, baseline);
        assert!(shared[0].starts_with("GET /mobile/sessions?limit=500 HTTP/1.1\n"));
        assert!(shared[0].contains("authorization: Bearer sync-only-key\n"));
        assert!(!shared[0].contains("runner-only-key"));
        assert!(shared[1].starts_with("POST /mobile/commands/claim-local HTTP/1.1\n"));
        assert!(shared[1].contains("authorization: Bearer runner-only-key\n"));
        assert!(!shared[1].contains("sync-only-key"));
        assert!(shared[1].contains("content-type: application/json\n"));
        assert!(shared[1].ends_with(r#"{"machine":"test-machine","sessions":[{"busy":true,"session_id":"session-mobile"}],"supports_images":true}"#));
        assert!(shared.iter().all(|request| !request.contains("cookie:")));
    }
    for client in [sync, runner] {
        let request = client.get("http://127.0.0.1/").build().unwrap();
        assert!(request.timeout().is_none());
        assert!(
            !request
                .headers()
                .contains_key(reqwest::header::AUTHORIZATION)
        );
        assert!(!request.headers().contains_key(reqwest::header::COOKIE));
        let request = client
            .get("http://127.0.0.1/")
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap();
        assert_eq!(request.timeout(), Some(&Duration::from_secs(30)));
    }
}

#[tokio::test]
async fn mobile_http_clones_share_pool_but_not_auth_or_response_cookies() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mobile", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut requests = Vec::new();
        for _ in 0..3 {
            requests.push(read_request(&mut socket).await);
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nSet-Cookie: account=must-not-leak; Path=/\r\n\r\n").await.unwrap();
        }
        requests
    });
    let check = async {
        let sync = daemon_http_client();
        let runner = daemon_http_client();
        sync.get(&url)
            .bearer_auth("sync-secret")
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        runner
            .get(&url)
            .bearer_auth("runner-secret")
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        sync.get(&url).send().await.unwrap().bytes().await.unwrap();
        let requests = server.await.unwrap();
        assert!(requests[0].contains("authorization: Bearer sync-secret\r\n"));
        assert!(!requests[0].contains("runner-secret"));
        assert!(requests[1].contains("authorization: Bearer runner-secret\r\n"));
        assert!(!requests[1].contains("sync-secret"));
        assert!(!requests[2].to_ascii_lowercase().contains("authorization:"));
        assert!(
            requests
                .iter()
                .all(|request| !request.to_ascii_lowercase().contains("cookie:"))
        );
    };
    // A regression to separate default clients cannot reuse the first socket
    // and therefore cannot complete the second request against this fixture.
    tokio::time::timeout(Duration::from_secs(5), check)
        .await
        .unwrap();
}
