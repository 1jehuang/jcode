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

fn timed_event() -> Value {
    json!({"id":"event", "start":{"dateTime":"2026-10-01T09:00:00-07:00",
        "timeZone":"America/Los_Angeles"}, "end":{"dateTime":"2026-10-01T12:15:00-07:00",
        "timeZone":"America/Los_Angeles"}})
}

async fn shifted(existing: Value, mut input: Value, expected_start: Value, expected_end: Value) {
    let fake = Fake::new(vec![(200, existing), (200, json!({"id":"event"}))]).await;
    input["action"] = json!("update");
    input["event_id"] = json!("event");
    let out = fake.tool().execute(input, ctx()).await.unwrap().output;
    assert!(out.starts_with("Event updated."));
    let requests = fake.finish().await;
    assert_eq!(
        requests[0].0,
        "GET /calendars/primary/events/event HTTP/1.1"
    );
    assert!(
        requests[1]
            .0
            .starts_with("PATCH /calendars/primary/events/event?sendUpdates=none")
    );
    assert_eq!(requests[1].1["start"], expected_start);
    assert_eq!(requests[1].1["end"], expected_end);
    assert_eq!(requests[1].1.as_object().unwrap().len(), 2);
}

#[tokio::test]
async fn start_only_preserves_long_duration_and_event_zone() {
    shifted(
        timed_event(),
        json!({"start":"2026-12-01T10:00:00-08:00"}),
        json!({"dateTime":"2026-12-01T18:00:00+00:00","timeZone":"America/Los_Angeles"}),
        json!({"dateTime":"2026-12-01T21:15:00+00:00","timeZone":"America/Los_Angeles"}),
    )
    .await;
}

#[tokio::test]
async fn start_only_all_day_preserves_exclusive_multiday_span() {
    shifted(
        json!({"start":{"date":"2026-10-01"},"end":{"date":"2026-10-04"}}),
        json!({"start":"2026-12-30"}),
        json!({"date":"2026-12-30"}),
        json!({"date":"2027-01-02"}),
    )
    .await;
}

#[tokio::test]
async fn local_start_uses_event_zone_and_elapsed_duration_across_dst() {
    shifted(
        timed_event(),
        json!({"start":"2026-11-01T00:30"}),
        json!({"dateTime":"2026-11-01T07:30:00+00:00","timeZone":"America/Los_Angeles"}),
        json!({"dateTime":"2026-11-01T10:45:00+00:00","timeZone":"America/Los_Angeles"}),
    )
    .await;
}

#[tokio::test]
async fn explicit_zone_overrides_event_zone() {
    shifted(
        timed_event(),
        json!({"start":"2026-12-01T10:00", "time_zone":"UTC"}),
        json!({"dateTime":"2026-12-01T10:00:00+00:00","timeZone":"UTC"}),
        json!({"dateTime":"2026-12-01T13:15:00+00:00","timeZone":"UTC"}),
    )
    .await;
}

#[tokio::test]
async fn missing_event_zone_looks_up_calendar_zone() {
    let fake = Fake::new(vec![
        (
            200,
            json!({"start":{"dateTime":"2026-01-01T09:00:00Z"},
        "end":{"dateTime":"2026-01-01T11:00:00Z"}}),
        ),
        (200, json!({"timeZone":"UTC"})),
        (200, json!({"id":"event"})),
    ])
    .await;
    fake.tool()
        .execute(
            json!({"action":"update","event_id":"event", "start":"2026-12-01T10:00"}),
            ctx(),
        )
        .await
        .unwrap();
    let requests = fake.finish().await;
    assert_eq!(requests[1].0, "GET /users/me/calendarList/primary HTTP/1.1");
    assert_eq!(
        requests[2].1["end"],
        json!({"dateTime":"2026-12-01T12:00:00+00:00","timeZone":"UTC"})
    );
}

#[tokio::test]
async fn explicit_end_and_metadata_update_do_not_fetch_existing_event() {
    for (input, expected) in [
        (
            json!({"start":"2026-10-21T10:00:00Z","end":"2026-10-21T15:00:00Z"}),
            json!({"start":{"dateTime":"2026-10-21T10:00:00+00:00"},"end":{"dateTime":"2026-10-21T15:00:00+00:00"}}),
        ),
        (json!({"summary":"renamed"}), json!({"summary":"renamed"})),
    ] {
        let fake = Fake::new(vec![(200, json!({"id":"event"}))]).await;
        let mut input = input;
        input["action"] = json!("update");
        input["event_id"] = json!("event");
        fake.tool().execute(input, ctx()).await.unwrap();
        let requests = fake.finish().await;
        assert!(requests[0].0.starts_with("PATCH "));
        assert_eq!(requests[0].1, expected);
    }
}

#[tokio::test]
async fn confirmation_preview_preserves_duration_without_patch() {
    let fake = Fake::new(vec![(200, timed_event())]).await;
    let output = fake
        .tool()
        .execute(
            json!({"action":"update", "event_id":"event",
        "start":"2026-12-01T10:00:00-08:00", "send_updates":"all"}),
            ctx(),
        )
        .await
        .unwrap()
        .output;
    assert!(output.starts_with("Confirmation required"));
    assert!(output.contains("2026-12-01T21:15:00+00:00"));
    assert_eq!(fake.finish().await.len(), 1);
}

#[tokio::test]
async fn unreadable_or_incompatible_duration_never_patches() {
    for (status, existing, start) in [
        (404, json!({"error":"missing"}), "2026-12-01"),
        (200, json!({"start":{"date":"2026-01-01"}}), "2026-12-01"),
        (200, timed_event(), "2026-12-01"),
        (
            200,
            json!({"start":{"date":"2026-01-01"},"end":{"date":"2026-01-03"}}),
            "2026-12-01T10:00:00Z",
        ),
        (
            200,
            json!({"start":{"date":"2026-01-02"},"end":{"date":"2026-01-01"}}),
            "2026-12-01",
        ),
        (200, timed_event(), "2026-03-08T02:30"),
        (200, timed_event(), "2026-11-01T01:30"),
    ] {
        let fake = Fake::new(vec![(status, existing)]).await;
        assert!(
            fake.tool()
                .execute(
                    json!({"action":"update","event_id":"event","start":start}),
                    ctx()
                )
                .await
                .is_err()
        );
        assert_eq!(fake.finish().await.len(), 1);
    }
}

#[tokio::test]
async fn create_still_defaults_to_thirty_minutes_without_get() {
    let fake = Fake::new(vec![(200, json!({"id":"created"}))]).await;
    fake.tool()
        .execute(
            json!({"action":"create","start":"2026-12-01T10:00:00Z"}),
            ctx(),
        )
        .await
        .unwrap();
    let requests = fake.finish().await;
    assert!(requests[0].0.starts_with("POST "));
    assert_eq!(
        requests[0].1["end"],
        json!({"dateTime":"2026-12-01T10:30:00+00:00"})
    );
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

#[tokio::test]
async fn duration_uses_old_instants_not_old_wall_clock_difference() {
    shifted(
        json!({"start":{"dateTime":"2026-11-01T01:30:00-07:00"},
            "end":{"dateTime":"2026-11-01T01:30:00-08:00"}}),
        json!({"start":"2026-12-01T10:00:00Z"}),
        json!({"dateTime":"2026-12-01T10:00:00+00:00"}),
        json!({"dateTime":"2026-12-01T11:00:00+00:00"}),
    )
    .await;
}

#[tokio::test]
async fn unspecified_or_invalid_timed_duration_never_patches() {
    for existing in [
        json!({"endTimeUnspecified":true}),
        json!({"start":{"dateTime":"bad"},"end":{"dateTime":"bad"}}),
        json!({"start":{"dateTime":"2026-01-01T12:00:00Z"},"end":{"dateTime":"2026-01-01T10:00:00Z"}}),
    ] {
        let fake = Fake::new(vec![(200, existing)]).await;
        assert!(
            fake.tool()
                .execute(
                    json!({"action":"update","event_id":"event",
            "start":"2026-12-01T10:00:00Z"}),
                    ctx()
                )
                .await
                .is_err()
        );
        assert_eq!(fake.finish().await.len(), 1);
    }
}

#[tokio::test]
async fn calendar_zone_lookup_failure_never_patches() {
    let fake = Fake::new(vec![
        (
            200,
            json!({"start":{"dateTime":"2026-01-01T09:00:00Z"},
        "end":{"dateTime":"2026-01-01T11:00:00Z"}}),
        ),
        (403, json!({"error":"denied"})),
    ])
    .await;
    let error = fake
        .tool()
        .execute(
            json!({"action":"update","event_id":"event",
        "start":"2026-12-01T10:00"}),
            ctx(),
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("403"), "{error}");
    assert_eq!(fake.finish().await.len(), 2);
}

#[tokio::test]
async fn confirmed_email_update_preserves_duration_and_propagates_patch_error() {
    for status in [200, 409] {
        let fake = Fake::new(vec![(200, timed_event()), (status, json!({"id":"event"}))]).await;
        let result = fake
            .tool()
            .execute(
                json!({"action":"update","event_id":"event",
            "start":"2026-12-01T10:00:00Z", "send_updates":"all", "confirmed":true}),
                ctx(),
            )
            .await;
        assert_eq!(result.is_ok(), status == 200);
        if let Err(error) = result {
            assert!(error.to_string().contains("409"));
        }
        let requests = fake.finish().await;
        assert!(requests[1].0.contains("sendUpdates=all"));
        assert_eq!(
            requests[1].1["end"]["dateTime"],
            "2026-12-01T13:15:00+00:00"
        );
    }
}

#[tokio::test]
async fn invalid_controls_fail_before_http() {
    let fake = Fake::new(vec![]).await;
    let tool = fake.tool();
    for (input, expected) in [
        (
            json!({"action":"update","start":"2026-12-01"}),
            "event_id is required",
        ),
        (
            json!({"action":"update","event_id":"event","start":"2026-12-01", "send_updates":"invalid"}),
            "Invalid send_updates",
        ),
        (
            json!({"action":"update","event_id":"event","end":"2026-12-01"}),
            "Pass start together with end",
        ),
    ] {
        let error = tool.execute(input, ctx()).await.unwrap_err().to_string();
        assert!(error.contains(expected), "{error}");
    }
    assert!(fake.finish().await.is_empty());
}
