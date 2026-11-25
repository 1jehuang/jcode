//! Offline regression tests through CalendarClient and Tool::execute. Each
//! server is bound to a fresh loopback port and receives only a synthetic token.
use super::*;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio::time::{Duration, timeout};

struct Fake {
    endpoint: String,
    requests: Arc<Mutex<Vec<(String, Value)>>>,
    task: JoinHandle<()>,
}

impl Fake {
    async fn new(responses: Vec<(u16, Value)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let task = tokio::spawn(async move {
            for (status, response) in responses {
                let (mut stream, _) = timeout(Duration::from_secs(5), listener.accept())
                    .await
                    .unwrap()
                    .unwrap();
                let mut bytes = Vec::new();
                let header_end = loop {
                    let mut buffer = [0; 4096];
                    let n = timeout(Duration::from_secs(5), stream.read(&mut buffer))
                        .await
                        .unwrap()
                        .unwrap();
                    assert_ne!(n, 0, "incomplete HTTP request");
                    bytes.extend_from_slice(&buffer[..n]);
                    if let Some(at) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        break at + 4;
                    }
                };
                let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
                let lower = headers.to_ascii_lowercase();
                assert!(lower.contains("authorization: bearer calendar-synthetic-only\r\n"));
                let len = lower
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .map(|value| value.trim().parse::<usize>().unwrap())
                    .unwrap_or(0);
                while bytes.len() < header_end + len {
                    let mut buffer = [0; 4096];
                    let n = timeout(Duration::from_secs(5), stream.read(&mut buffer))
                        .await
                        .unwrap()
                        .unwrap();
                    assert_ne!(n, 0);
                    bytes.extend_from_slice(&buffer[..n]);
                }
                let body = if len == 0 {
                    Value::Null
                } else {
                    serde_json::from_slice(&bytes[header_end..header_end + len]).unwrap()
                };
                captured
                    .lock()
                    .unwrap()
                    .push((headers.lines().next().unwrap().into(), body));
                let body = response.to_string();
                let reply = format!(
                    "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        Self {
            endpoint,
            requests,
            task,
        }
    }

    fn client(&self) -> CalendarClient {
        CalendarClient::for_loopback_tests(&self.endpoint, "calendar-synthetic-only").unwrap()
    }

    fn tool(&self) -> CalendarTool {
        CalendarTool {
            client: self.client(),
        }
    }

    async fn finish(self) -> Vec<(String, Value)> {
        timeout(Duration::from_secs(6), self.task)
            .await
            .unwrap()
            .unwrap();
        self.requests.lock().unwrap().clone()
    }
}

fn ctx() -> ToolContext {
    ToolContext {
        session_id: "calendar-offline".into(),
        message_id: "message".into(),
        tool_call_id: "call".into(),
        working_dir: None,
        stdin_request_tx: None,
        graceful_shutdown_signal: None,
        execution_mode: super::super::ToolExecutionMode::Direct,
    }
}

fn query(request: &str) -> std::collections::BTreeMap<String, String> {
    let target = request.split_whitespace().nth(1).unwrap();
    reqwest::Url::parse(&format!("http://127.0.0.1{target}"))
        .unwrap()
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

#[tokio::test]
async fn list_traverses_empty_and_short_pages_with_identical_filters() {
    let fake = Fake::new(vec![
        (
            200,
            json!({"items":[{"id":"one","summary":"First"}],"nextPageToken":"a +/&"}),
        ),
        (200, json!({"items":[],"nextPageToken":"b"})),
        (
            200,
            json!({"items":[{"id":"two","summary":"Later meeting"}]}),
        ),
    ])
    .await;
    let output = fake
        .tool()
        .execute(
            json!({"action":"list", "calendar_id":"team@example.test",
        "time_min":"2026-10-21T00:00:00Z", "time_max":"2026-10-28T00:00:00Z",
        "query":"a + b", "max_results":1}),
            ctx(),
        )
        .await
        .unwrap()
        .output;
    assert!(output.contains("First") && output.contains("Later meeting"));
    assert!(output.contains("(2)"));
    let requests = fake.finish().await;
    assert_eq!(requests.len(), 3);
    let first = query(&requests[0].0);
    assert_eq!(first["maxResults"], "1");
    assert_eq!(first["q"], "a + b");
    assert_eq!(first["singleEvents"], "true");
    assert_eq!(first["orderBy"], "startTime");
    for (index, (line, _)) in requests.iter().enumerate() {
        assert!(line.starts_with("GET /calendars/team%40example.test/events?"));
        let mut params = query(line);
        let token = params.remove("pageToken");
        assert_eq!(token.as_deref(), [None, Some("a +/&"), Some("b")][index]);
        assert_eq!(params, first);
    }
}

#[tokio::test]
async fn client_pagination_errors_do_not_return_partial_success() {
    for responses in [
        vec![
            (
                200,
                json!({"items":[{"id":"one"}],"nextPageToken":"repeat"}),
            ),
            (200, json!({"nextPageToken":"repeat"})),
        ],
        vec![
            (200, json!({"items":[{"id":"one"}],"nextPageToken":"next"})),
            (403, json!({"error":"denied"})),
        ],
    ] {
        let fake = Fake::new(responses).await;
        let error = fake
            .client()
            .list_events("primary", None, None, None, 25)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("repeated") || error.contains("403"),
            "{error}"
        );
        assert_eq!(fake.finish().await.len(), 2);
    }
}

#[tokio::test]
async fn empty_list_stops_without_a_token() {
    let fake = Fake::new(vec![(200, json!({"items":[],"nextPageToken":""}))]).await;
    let output = fake
        .tool()
        .execute(json!({"action":"list"}), ctx())
        .await
        .unwrap()
        .output;
    assert!(output.starts_with("No events"));
    assert_eq!(fake.finish().await.len(), 1);
}

#[test]
fn test_transport_rejects_non_loopback_destinations() {
    for endpoint in [
        "https://www.googleapis.com/calendar/v3",
        "http://example.test",
        "http://localhost",
        "http://127.0.0.1@evil.test",
    ] {
        assert!(CalendarClient::for_loopback_tests(endpoint, "calendar-synthetic-only").is_err());
    }
}
