//! Real HTTP regressions for the built-in Azure Responses retry boundary.
use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const COMPLETE: &str = concat!(
    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"final\"}\n\n",
    "data: {\"type\":\"response.completed\",\"response\":{}}\n\n",
);

struct Server {
    base: String,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<Vec<Instant>>>,
}

impl Server {
    fn new(bodies: Vec<String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/openai/v1", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            let mut requests = Vec::new();
            let deadline = Instant::now() + Duration::from_secs(12);
            while !thread_stop.load(Ordering::SeqCst) && Instant::now() < deadline {
                let (mut socket, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("accept: {error}"),
                };
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut buffer = [0; 4096];
                    let count = socket.read(&mut buffer).unwrap();
                    assert_ne!(count, 0, "request must have a body");
                    bytes.extend_from_slice(&buffer[..count]);
                    if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                        let length: usize = headers
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length: "))
                            .unwrap()
                            .parse()
                            .unwrap();
                        if bytes.len() >= end + 4 + length {
                            assert!(headers.starts_with("post /openai/v1/responses "));
                            break;
                        }
                    }
                }
                let index = requests.len();
                requests.push(Instant::now());
                let body = bodies.get(index).map(String::as_str).unwrap_or(COMPLETE);
                // Content-Length is deliberately correct, including for the
                // truncated SSE fixture. This is a clean HTTP EOF, not a
                // reqwest transport error that was already retryable.
                write!(socket,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(), body).unwrap();
            }
            requests
        });
        Self {
            base,
            stop,
            thread: Some(thread),
        }
    }

    fn finish(mut self) -> Vec<Instant> {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.take().unwrap().join().unwrap()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run_azure(base: String) -> Vec<Result<StreamEvent>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (tx, mut rx) = mpsc::channel(64);
        tokio::time::timeout(
            Duration::from_secs(10),
            run_stream_with_retries(
                reqwest::Client::new(),
                base,
                ProviderAuth::None {
                    label: "test".into(),
                },
                false,
                "azure-retry-test".into(),
                serde_json::json!({"model":"deployment", "input":[], "stream":true}),
                true,
                tx,
                Arc::new(Mutex::new(None)),
                "deployment".into(),
                false,
                "openrouter::test-scope".to_string(),
            ),
        )
        .await
        .expect("retry loop must finish");
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        events
    })
}

#[test]
fn azure_clean_eof_retries_with_rollback_before_replayed_output() {
    let partial = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"id\":\"rs_1\",\"type\":\"reasoning\",\"summary\":[],\"encrypted_content\":\"cipher\"}}\n\n",
        "data: {\"type\":\"response.output_item.added\",\"item\":{\"id\":\"fc_1\",\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"bash\",\"arguments\":\"\"}}\n\n",
        "data: {\"type\":\"response.function_call_arguments.done\",\"item_id\":\"fc_1\",\"arguments\":\"{}\"}\n\n",
    );
    let server = Server::new(vec![partial.into(), COMPLETE.into()]);
    let events: Vec<_> = run_azure(server.base.clone())
        .into_iter()
        .map(Result::unwrap)
        .collect();
    assert_eq!(server.finish().len(), 2);
    let rollback = events
        .iter()
        .position(|event| matches!(event, StreamEvent::RetryRollback { .. }))
        .unwrap();
    assert!(
        events[..rollback]
            .iter()
            .any(|event| matches!(event, StreamEvent::TextDelta(text) if text == "partial"))
    );
    assert!(
        events[..rollback]
            .iter()
            .any(|event| matches!(event, StreamEvent::OpenAIReasoning { .. }))
    );
    assert!(
        events[..rollback]
            .iter()
            .any(|event| matches!(event, StreamEvent::ToolUseStart { .. }))
    );
    assert!(
        events[rollback + 1..]
            .iter()
            .any(|event| matches!(event, StreamEvent::TextDelta(text) if text == "final"))
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, StreamEvent::MessageEnd { .. }))
            .count(),
        1
    );
}

#[test]
fn azure_permanent_structured_failure_is_not_retried_and_keeps_hint() {
    let body = "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"type\":\"invalid_request_error\",\"code\":\"content_policy_violation\",\"message\":\"Rejected even though text says timeout\",\"retry_after\":7}}}\n\n";
    let server = Server::new(vec![body.into(), COMPLETE.into()]);
    let events = run_azure(server.base.clone());
    assert_eq!(server.finish().len(), 1, "permanent failure must not retry");
    let error = events
        .into_iter()
        .find_map(Result::err)
        .expect("must surface error");
    assert!(error.to_string().contains("invalid_request_error"));
    assert!(!retryable_stream_failure(&error));
    let remaining = jcode_provider_core::retry_after::retry_after_from_error(&error)
        .expect("retry hint retained through typed wrapper");
    assert!(remaining <= Duration::from_secs(7) && remaining > Duration::from_secs(5));
}

#[test]
fn azure_transient_structured_failure_honors_retry_after() {
    let body = "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"type\":\"server_error\",\"message\":\"Try again\",\"retry_after\":2}}}\n\n";
    let server = Server::new(vec![body.into(), COMPLETE.into()]);
    let events = run_azure(server.base.clone());
    let requests = server.finish();
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1].duration_since(requests[0]) >= Duration::from_millis(1900),
        "must honor two-second hint rather than one-second backoff"
    );
    assert!(events.iter().all(Result::is_ok));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Ok(StreamEvent::MessageEnd { .. })))
    );
}

#[test]
fn azure_unknown_failure_does_not_inherit_stream_error_retry_heuristic() {
    let error = azure_response_failure("Unrecognized deployment rejection", None);
    assert!(!retryable_stream_failure(&error));
    // Non-Azure errors still use exactly the legacy heuristic.
    let legacy = anyhow::anyhow!("OpenAI-compatible stream error: unknown");
    assert!(retryable_stream_failure(&legacy));
    let error = azure_response_failure("rate_limit_exceeded", Some(u64::MAX));
    assert!(retryable_stream_failure(&error));
    assert!(
        jcode_provider_core::retry_after::retry_after_from_error(&error).unwrap()
            <= jcode_provider_core::retry_after::MAX_RETRY_AFTER
    );
}
