//! Loop regressions use local HTTP responses, not a synthetic retry classifier.
use super::*;
use std::io::{Read, Write};
use std::sync::atomic::AtomicUsize;
use std::time::{Duration, Instant};

struct FakeServer {
    url: String,
    requests: Arc<AtomicUsize>,
    bodies: Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl FakeServer {
    fn new(responses: Vec<String>) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let bodies = Arc::new(std::sync::Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (count, recorded, done) = (requests.clone(), bodies.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            while !done.load(Ordering::SeqCst) {
                let (mut socket, _) = match listener.accept() {
                    Ok(socket) => socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("fake HTTP accept: {error}"),
                };
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut buffer = [0; 4096];
                    let len = socket.read(&mut buffer).unwrap();
                    assert_ne!(len, 0, "incomplete request");
                    bytes.extend_from_slice(&buffer[..len]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]);
                        let length: usize = headers
                            .lines()
                            .find_map(|line| {
                                let (key, value) = line.split_once(':')?;
                                key.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse().unwrap())
                            })
                            .unwrap();
                        if bytes.len() >= end + 4 + length {
                            recorded
                                .lock()
                                .unwrap()
                                .push(bytes[end + 4..end + 4 + length].to_vec());
                            break;
                        }
                    }
                }
                let attempt = count.fetch_add(1, Ordering::SeqCst);
                let response = &responses[attempt.min(responses.len() - 1)];
                // Closing the consumer during a test may also close the socket.
                let _ = socket.write_all(response.as_bytes());
            }
        });
        Self {
            url,
            requests,
            bodies,
            stop,
            thread: Some(thread),
        }
    }
}
impl Drop for FakeServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.take().unwrap().join().unwrap();
    }
}

fn rejection(seconds: u64) -> String {
    let body = r#"{"error":{"type":"rate_limit_error","message":"test rate limit"}}"#;
    format!(
        "HTTP/1.1 429 Too Many Requests\r\nRetry-After: {seconds}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}
fn sse(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}
fn success() -> String {
    sse(concat!(
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    ))
}

fn settings(budget: Option<u64>) -> reasoning_request::RetrySettings {
    // Construct before entering Tokio so usage prewarming cannot make live HTTP calls.
    let provider = AnthropicProvider::new();
    let mut settings = reasoning_request::RetrySettings::from_provider(&provider);
    settings.retry_wait_budget_secs = budget;
    settings
}

fn spawn_loop(
    server: &FakeServer,
    settings: reasoning_request::RetrySettings,
) -> (
    tokio::task::JoinHandle<()>,
    mpsc::Receiver<Result<StreamEvent>>,
) {
    let transport = DirectTransportConfig {
        api_url: server.url.clone(),
        oauth_api_url: None,
        headers: Ok(HeaderMap::new()),
        auth_mode: "none".into(),
        auth_header: "x-api-key".into(),
    };
    let (tx, rx) = mpsc::channel(100);
    let task = tokio::spawn(run_stream_with_retries(
        Client::builder().no_proxy().build().unwrap(),
        "test-token".into(),
        false,
        ApiRequest {
            model: "claude-sonnet-4-6".into(),
            max_tokens: 1024,
            system: None,
            messages: vec![],
            tools: None,
            metadata: None,
            thinking: None,
            output_config: None,
            temperature: None,
            service_tier: None,
            stream: true,
        },
        tx,
        Arc::new(RwLock::new(None)),
        None,
        "claude-sonnet-4-6".into(),
        "test-session".into(),
        Arc::new(std::sync::RwLock::new("claude-sonnet-4-6".into())),
        transport,
        settings,
    ));
    (task, rx)
}

fn run_case(
    budget: Option<u64>,
    responses: Vec<String>,
) -> (Vec<StreamEvent>, Option<anyhow::Error>, usize, Duration) {
    let settings = settings(budget);
    let server = FakeServer::new(responses);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let start = Instant::now();
        let (task, mut rx) = spawn_loop(&server, settings);
        let result = tokio::time::timeout(Duration::from_secs(6), async {
            let mut events = Vec::new();
            let mut error = None;
            while let Some(event) = rx.recv().await {
                match event {
                    Ok(event) => events.push(event),
                    Err(e) => error = Some(e),
                }
            }
            task.await.unwrap();
            (events, error)
        })
        .await
        .expect("retry budget must not hide the provider error behind its server wait");
        let bodies = server.bodies.lock().unwrap();
        for body in bodies.iter().skip(1) {
            assert_eq!(
                body, &bodies[0],
                "retries do not synthesize conversation/tool messages"
            );
        }
        (
            result.0,
            result.1,
            server.requests.load(Ordering::SeqCst),
            start.elapsed(),
        )
    })
}
fn retries(events: &[StreamEvent]) -> Vec<&jcode_message_types::RetryMetadata> {
    events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::ConnectionPhase {
                phase: jcode_message_types::ConnectionPhase::RetryingWithMetadata { retry },
            } => Some(retry),
            _ => None,
        })
        .collect()
}

#[test]
fn retry_sleep_budget_two_long_waits_fail_before_first_retry_with_original_error() {
    let _lock = jcode_base::storage::lock_test_env();
    let (events, error, count, elapsed) =
        run_case(Some(30), vec![rejection(60), rejection(60), success()]);
    let error = error.expect("original provider rejection");
    assert!(error.to_string().contains("429 Too Many Requests"));
    assert!(error.to_string().contains("test rate limit"));
    assert!(jcode_provider_core::retry_after::retry_after_required_from_error(&error).is_some());
    assert_eq!(count, 1);
    assert!(retries(&events).is_empty(), "no retry was scheduled");
    assert!(elapsed < Duration::from_secs(2));
}

#[test]
fn retry_sleep_budget_uncapped_hint_cannot_retry_early() {
    let _lock = jcode_base::storage::lock_test_env();
    let (_, error, count, elapsed) = run_case(Some(90), vec![rejection(3600), success()]);
    assert!(error.unwrap().to_string().contains("429"));
    assert_eq!(count, 1);
    assert!(elapsed < Duration::from_secs(2));
}

#[test]
fn retry_sleep_budget_is_cumulative_across_failures() {
    let _lock = jcode_base::storage::lock_test_env();
    let (events, error, count, elapsed) =
        run_case(Some(1), vec![rejection(1), rejection(1), success()]);
    assert!(error.unwrap().to_string().contains("429"));
    assert_eq!(count, 2);
    assert_eq!(retries(&events).len(), 1);
    assert!(elapsed >= Duration::from_millis(900));
    assert!(elapsed < Duration::from_secs(2));
}

#[test]
fn retry_sleep_budget_short_retry_recovers_with_finite_metadata() {
    let _lock = jcode_base::storage::lock_test_env();
    let (events, error, count, elapsed) = run_case(Some(3), vec![rejection(1), success()]);
    assert!(error.is_none());
    assert_eq!(count, 2);
    let retries = retries(&events);
    assert_eq!(retries.len(), 1);
    assert_eq!(retries[0].attempt, 2);
    assert_eq!(retries[0].max, MAX_RETRIES);
    assert_eq!(
        retries[0].reason,
        jcode_message_types::RetryReason::RateLimit
    );
    assert!(retries[0].delay_ms > 900 && retries[0].delay_ms <= 1000);
    assert!(elapsed >= Duration::from_millis(900));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, StreamEvent::MessageEnd { .. }))
    );
}

#[test]
fn retry_sleep_budget_disabled_keeps_existing_retry_sequence() {
    let _lock = jcode_base::storage::lock_test_env();
    let (events, error, count, elapsed) =
        run_case(None, vec![rejection(1), rejection(1), success()]);
    assert!(error.is_none());
    assert_eq!(count, 3);
    assert_eq!(retries(&events).len(), 2);
    assert!(elapsed >= Duration::from_millis(1800));
}

#[test]
fn retry_sleep_budget_zero_refuses_positive_wait() {
    let _lock = jcode_base::storage::lock_test_env();
    let (events, error, count, _) = run_case(Some(0), vec![rejection(1), success()]);
    assert!(error.is_some());
    assert_eq!(count, 1);
    assert!(retries(&events).is_empty());
}

#[test]
fn retry_sleep_budget_partial_tool_attempt_is_rolled_back_before_execution() {
    let _lock = jcode_base::storage::lock_test_env();
    let tool = concat!(
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tool_1\",\"name\":\"test_tool\",\"input\":{}}}\n\n",
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{}\"}}\n\n",
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    );
    let failure = sse(&format!(
        "{tool}event: error\ndata: {{\"type\":\"error\",\"error\":{{\"type\":\"overloaded_error\",\"message\":\"overloaded\"}}}}\n\n"
    ));
    let success = sse(&format!(
        "{tool}event: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"tool_use\"}},\"usage\":{{\"output_tokens\":1}}}}\n\nevent: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n"
    ));
    let (events, error, count, _) = run_case(Some(10), vec![failure, success]);
    assert!(error.is_none());
    assert_eq!(count, 2);
    assert_eq!(
        retries(&events)[0].reason,
        jcode_message_types::RetryReason::Overloaded
    );
    // Match the consumer contract: tool calls are provisional until the stream
    // completes. Rollback removes failed-attempt calls, so only the final call
    // can reach the tool executor.
    let mut pending = Vec::new();
    let mut completed = Vec::new();
    let mut rollbacks = 0;
    for event in events {
        match event {
            StreamEvent::ToolUseStart { id, .. } => pending.push(id),
            StreamEvent::RetryRollback { .. } => {
                pending.clear();
                rollbacks += 1;
            }
            StreamEvent::MessageEnd { .. } => completed.append(&mut pending),
            _ => {}
        }
    }
    assert_eq!(rollbacks, 1);
    assert_eq!(completed, ["tool_1"]);
}

#[test]
fn retry_sleep_budget_cancel_before_attempt_makes_no_request() {
    let _lock = jcode_base::storage::lock_test_env();
    let settings = settings(Some(30));
    let server = FakeServer::new(vec![success()]);
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let (task, rx) = spawn_loop(&server, settings);
            drop(rx);
            tokio::time::timeout(Duration::from_millis(500), task)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(server.requests.load(Ordering::SeqCst), 0);
        });
}

#[test]
fn retry_sleep_budget_cancel_during_wait_exits_without_reissuing_request() {
    let _lock = jcode_base::storage::lock_test_env();
    let settings = settings(Some(120));
    let server = FakeServer::new(vec![rejection(60), success()]);
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let (task, mut rx) = spawn_loop(&server, settings);
            tokio::time::timeout(Duration::from_secs(2), async {
                while let Some(event) = rx.recv().await {
                    if matches!(
                        event,
                        Ok(StreamEvent::ConnectionPhase {
                            phase: jcode_message_types::ConnectionPhase::RetryingWithMetadata { .. }
                        })
                    ) {
                        break;
                    }
                }
            })
            .await
            .unwrap();
            drop(rx);
            tokio::time::timeout(Duration::from_millis(500), task)
                .await
                .expect("consumer cancellation interrupts retry sleep")
                .unwrap();
            assert_eq!(server.requests.load(Ordering::SeqCst), 1);
        });
}
