//! Command Code mock-server integration tests.
//!
//! These tests execute the real provider HTTP client, request builder, SSE
//! parsers, tool-event lifecycle, and request continuation against a local
//! loopback server. They do not call Command Code.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use jcode_message_types::{ContentBlock, Message, Role, StreamEvent, ToolDefinition};
use jcode_provider_core::Provider;
use serde_json::json;

use super::OpenRouterProvider;

struct TestEnv {
    _guard: std::sync::MutexGuard<'static, ()>,
    previous_max_retries: Option<std::ffi::OsString>,
    previous_backoff_cap: Option<std::ffi::OsString>,
}

impl TestEnv {
    fn new() -> Self {
        Self::with_retries(1)
    }

    fn with_retries(max_retries: u32) -> Self {
        let guard = crate::tests::ENV_LOCK.lock();
        let previous_max_retries = std::env::var_os("JCODE_MAX_RETRIES");
        let previous_backoff_cap = std::env::var_os("JCODE_RETRY_BACKOFF_CAP_SECS");
        jcode_base::env::set_var("JCODE_MAX_RETRIES", max_retries.to_string());
        jcode_base::env::set_var("JCODE_RETRY_BACKOFF_CAP_SECS", "1");
        jcode_base::config::invalidate_config_cache();
        Self {
            _guard: guard,
            previous_max_retries,
            previous_backoff_cap,
        }
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        match self.previous_max_retries.take() {
            Some(value) => jcode_base::env::set_var("JCODE_MAX_RETRIES", value),
            None => jcode_base::env::remove_var("JCODE_MAX_RETRIES"),
        }
        match self.previous_backoff_cap.take() {
            Some(value) => jcode_base::env::set_var("JCODE_RETRY_BACKOFF_CAP_SECS", value),
            None => jcode_base::env::remove_var("JCODE_RETRY_BACKOFF_CAP_SECS"),
        }
        jcode_base::config::invalidate_config_cache();
    }
}

struct MockResponse {
    status: u16,
    body: String,
}

struct MockServer {
    base: String,
    requests: Arc<Mutex<Vec<String>>>,
}

struct HangingServer {
    base: String,
}

struct HomeGuard {
    _temp: tempfile::TempDir,
    previous: Option<std::ffi::OsString>,
    previous_key: Option<std::ffi::OsString>,
}

impl HomeGuard {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("temp home");
        let previous = std::env::var_os("JCODE_HOME");
        let previous_key = std::env::var_os("COMMAND_CODE_API_KEY");
        jcode_base::env::set_var("JCODE_HOME", temp.path());
        jcode_base::env::remove_var("COMMAND_CODE_API_KEY");
        Self {
            _temp: temp,
            previous,
            previous_key,
        }
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => jcode_base::env::set_var("JCODE_HOME", value),
            None => jcode_base::env::remove_var("JCODE_HOME"),
        }
        match self.previous_key.take() {
            Some(value) => jcode_base::env::set_var("COMMAND_CODE_API_KEY", value),
            None => jcode_base::env::remove_var("COMMAND_CODE_API_KEY"),
        }
    }
}

impl MockServer {
    fn start(responses: Vec<MockResponse>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let port = listener.local_addr().expect("local addr").port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        std::thread::spawn(move || {
            for response in responses {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let request = read_http_request(&mut stream);
                captured.lock().expect("requests lock").push(request);
                let reason = if response.status == 200 {
                    "OK"
                } else {
                    "ERROR"
                };
                let head = format!(
                    "HTTP/1.1 {} {}\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response.status,
                    reason,
                    response.body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(response.body.as_bytes());
                let _ = stream.flush();
            }
        });
        Self {
            base: format!("http://127.0.0.1:{port}/api.commandcode.ai/provider/v1"),
            requests,
        }
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("requests lock").clone()
    }
}

impl HangingServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind hanging server");
        let port = listener.local_addr().expect("local addr").port();
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let _ = read_http_request(&mut stream);
            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
            let body = "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n";
            let chunk = format!("{:x}\r\n{}\r\n", body.len(), body);
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(chunk.as_bytes());
            let _ = stream.flush();
            // Keep the response body open without a terminal event. The client
            // must be able to cancel the stream without waiting for this sleep.
            std::thread::sleep(Duration::from_secs(3));
        });
        Self {
            base: format!("http://127.0.0.1:{port}/api.commandcode.ai/provider/v1"),
        }
    }
}

fn read_http_request(stream: &mut std::net::TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                buffer.extend_from_slice(&chunk[..read]);
                if let Some(head_end) = find_head_end(&buffer) {
                    let head = String::from_utf8_lossy(&buffer[..head_end]).to_string();
                    let content_length = head.lines().find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        if name.eq_ignore_ascii_case("content-length") {
                            value.trim().parse::<usize>().ok()
                        } else {
                            None
                        }
                    });
                    if let Some(content_length) = content_length {
                        let body_start = head_end + 4;
                        while buffer.len() < body_start + content_length {
                            match stream.read(&mut chunk) {
                                Ok(0) => break,
                                Ok(read) => buffer.extend_from_slice(&chunk[..read]),
                                Err(_) => break,
                            }
                        }
                    }
                    break;
                }
            }
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&buffer).to_string()
}

fn find_head_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn collect(
    runtime: &tokio::runtime::Runtime,
    provider: &OpenRouterProvider,
    messages: &[Message],
    tools: &[ToolDefinition],
) -> Vec<StreamEvent> {
    runtime.block_on(async {
        let mut stream = provider
            .complete(messages, tools, "system", None)
            .await
            .expect("complete");
        let mut events = Vec::new();
        while let Some(item) = stream.next().await {
            events.push(item.expect("stream event"));
        }
        events
    })
}

fn bash_tool() -> ToolDefinition {
    ToolDefinition::new(
        "bash",
        "Run a shell command",
        json!({"type":"object","properties":{"command":{"type":"string"}},"required":["command"]}),
    )
}

fn tool_followup() -> Vec<Message> {
    tool_followup_with_id("call_1")
}

fn tool_followup_with_id(id: &str) -> Vec<Message> {
    vec![
        Message::user("run it"),
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: "bash".to_string(),
                input: json!({"command":"echo hi"}),
                thought_signature: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        },
        Message::tool_result(id, "hi", false),
    ]
}

fn chat_tool_stream() -> String {
    [
        r#"data: {"choices":[{"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"bash"}}]},"finish_reason":null}]}"#,
        r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"command\":\"echo hi\"}"}}]},"finish_reason":null}]}"#,
        r#"data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#,
        "data: [DONE]",
    ]
    .join("\n\n")
        + "\n\n"
}

fn chat_text_stream() -> String {
    [
        r#"data: {"choices":[{"delta":{"content":"DONE"}}]}"#,
        r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":12,"completion_tokens":2}}"#,
        "data: [DONE]",
    ]
    .join("\n\n")
        + "\n\n"
}

fn responses_tool_stream() -> String {
    [
        r#"data: {"type":"response.output_item.added","output_index":0,"item":{"id":"item_1","type":"function_call","call_id":"call_1","name":"bash"}}"#,
        r#"data: {"type":"response.function_call_arguments.delta","item_id":"item_1","delta":"{\"command\":\"echo hi\"}"}"#,
        r#"data: {"type":"response.function_call_arguments.done","item_id":"item_1","arguments":"{\"command\":\"echo hi\"}"}"#,
        r#"data: {"type":"response.completed","response":{"id":"resp_1","status":"completed","usage":{"input_tokens":10,"output_tokens":5,"input_tokens_details":{"cached_tokens":0}}}}"#,
    ]
    .join("\n\n")
        + "\n\n"
}

fn responses_text_stream() -> String {
    [
        r#"data: {"type":"response.output_text.delta","delta":"DONE"}"#,
        r#"data: {"type":"response.completed","response":{"id":"resp_2","status":"completed","usage":{"input_tokens":12,"output_tokens":2,"input_tokens_details":{"cached_tokens":8}}}}"#,
    ]
    .join("\n\n")
        + "\n\n"
}

fn messages_text_stream() -> String {
    concat!(
        "event: message_start\n",
        r#"data: {"type":"message_start","message":{"id":"msg_1","model":"claude-sonnet-5","usage":{"input_tokens":10,"output_tokens":1}}}"#,
        "\n\n",
        "event: content_block_start\n",
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        "\n\n",
        "event: content_block_delta\n",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"DONE"}}"#,
        "\n\n",
        "event: content_block_stop\n",
        r#"data: {"type":"content_block_stop","index":0}"#,
        "\n\n",
        "event: message_delta\n",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}"#,
        "\n\n",
        "event: message_stop\n",
        r#"data: {"type":"message_stop"}"#,
        "\n\n",
    )
    .to_string()
}

fn messages_tool_stream() -> String {
    concat!(
        "event: message_start\n",
        r#"data: {"type":"message_start","message":{"id":"msg_tool","model":"claude-sonnet-5","usage":{"input_tokens":10,"output_tokens":1}}}"#,
        "\n\n",
        "event: content_block_start\n",
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"bash","input":{}}}"#,
        "\n\n",
        "event: content_block_delta\n",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"command\":\"echo hi\"}"}}"#,
        "\n\n",
        "event: content_block_stop\n",
        r#"data: {"type":"content_block_stop","index":0}"#,
        "\n\n",
        "event: message_delta\n",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":5}}"#,
        "\n\n",
        "event: message_stop\n",
        r#"data: {"type":"message_stop"}"#,
        "\n\n",
    )
    .to_string()
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

#[test]
fn chat_tool_call_and_result_are_replayed() {
    let _env = TestEnv::new();
    let server = MockServer::start(vec![
        MockResponse {
            status: 200,
            body: chat_tool_stream(),
        },
        MockResponse {
            status: 200,
            body: chat_text_stream(),
        },
    ]);
    let provider = OpenRouterProvider::new_command_code_test(
        server.base.clone(),
        "test-key".into(),
        "deepseek/deepseek-v4-flash",
    );
    let runtime = runtime();
    let events = collect(
        &runtime,
        &provider,
        &[Message::user("run it")],
        &[bash_tool()],
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, StreamEvent::ToolUseStart { name, .. } if name == "bash"))
    );
    assert!(events.iter().any(|event| matches!(
        event,
        StreamEvent::ToolUseEnd | StreamEvent::ToolUseEndFor { .. }
    )));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, StreamEvent::MessageEnd { .. }))
    );

    let events = collect(&runtime, &provider, &tool_followup(), &[bash_tool()]);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, StreamEvent::TextDelta(text) if text == "DONE"))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, StreamEvent::MessageEnd { .. }))
    );

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].contains("\"role\":\"tool\""));
    assert!(requests[1].contains("\"tool_call_id\":\"call_1\""));
    assert!(requests[1].contains("hi"));
}

#[test]
fn responses_tool_call_and_result_are_replayed() {
    let _env = TestEnv::new();
    let server = MockServer::start(vec![
        MockResponse {
            status: 200,
            body: responses_tool_stream(),
        },
        MockResponse {
            status: 200,
            body: responses_text_stream(),
        },
    ]);
    let provider = OpenRouterProvider::new_command_code_test(
        server.base.clone(),
        "test-key".into(),
        "meta/muse-ember",
    );
    let runtime = runtime();
    let events = collect(
        &runtime,
        &provider,
        &[Message::user("run it")],
        &[bash_tool()],
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, StreamEvent::ToolUseStart { name, .. } if name == "bash"))
    );
    assert!(events.iter().any(|event| matches!(
        event,
        StreamEvent::ToolUseEnd | StreamEvent::ToolUseEndFor { .. }
    )));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, StreamEvent::MessageEnd { .. }))
    );

    let events = collect(&runtime, &provider, &tool_followup(), &[bash_tool()]);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, StreamEvent::TextDelta(text) if text == "DONE"))
    );

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].contains("\"type\":\"function_call_output\""));
    assert!(requests[1].contains("\"call_id\":\"call_1\""));
    assert!(requests[1].contains("hi"));
}

#[test]
fn messages_text_stream_uses_anthropic_wire() {
    let _env = TestEnv::new();
    let server = MockServer::start(vec![MockResponse {
        status: 200,
        body: messages_text_stream(),
    }]);
    let provider = OpenRouterProvider::new_command_code_test(
        server.base.clone(),
        "test-key".into(),
        "claude-sonnet-5",
    );
    let runtime = runtime();
    let events = collect(&runtime, &provider, &[Message::user("hi")], &[]);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, StreamEvent::TextDelta(text) if text == "DONE"))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, StreamEvent::MessageEnd { .. }))
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].contains("\"model\":\"claude-sonnet-5\""));
    assert!(requests[0].contains("\"stream\":true"));
}

#[test]
fn messages_tool_call_and_result_are_replayed() {
    let _env = TestEnv::new();
    let server = MockServer::start(vec![
        MockResponse {
            status: 200,
            body: messages_tool_stream(),
        },
        MockResponse {
            status: 200,
            body: messages_text_stream(),
        },
    ]);
    let provider = OpenRouterProvider::new_command_code_test(
        server.base.clone(),
        "test-key".into(),
        "claude-sonnet-5",
    );
    let runtime = runtime();
    let events = collect(
        &runtime,
        &provider,
        &[Message::user("run it")],
        &[bash_tool()],
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, StreamEvent::ToolUseStart { name, .. } if name == "bash"))
    );
    assert!(events.iter().any(|event| matches!(
        event,
        StreamEvent::ToolUseEnd | StreamEvent::ToolUseEndFor { .. }
    )));

    let events = collect(
        &runtime,
        &provider,
        &tool_followup_with_id("toolu_1"),
        &[bash_tool()],
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, StreamEvent::TextDelta(text) if text == "DONE"))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, StreamEvent::MessageEnd { .. }))
    );

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].contains("\"type\":\"tool_result\""));
    assert!(requests[1].contains("\"tool_use_id\":\"toolu_1\""));
    assert!(requests[1].contains("hi"));
}

#[test]
fn chat_incomplete_stream_is_an_error() {
    let _env = TestEnv::new();
    let server = MockServer::start(vec![MockResponse {
        status: 200,
        body: "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n".to_string(),
    }]);
    let provider = OpenRouterProvider::new_command_code_test(
        server.base.clone(),
        "test-key".into(),
        "deepseek/deepseek-v4-flash",
    );
    let runtime = runtime();
    let result = runtime.block_on(async {
        let mut stream = provider
            .complete(&[Message::user("hi")], &[], "system", None)
            .await
            .expect("complete");
        let mut saw_error = false;
        while let Some(item) = stream.next().await {
            if item.is_err() {
                saw_error = true;
            }
        }
        saw_error
    });
    assert!(result, "incomplete chat stream must error");
}

#[test]
fn responses_incomplete_stream_is_an_error() {
    let _env = TestEnv::new();
    let server = MockServer::start(vec![MockResponse {
        status: 200,
        body: "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n"
            .to_string(),
    }]);
    let provider = OpenRouterProvider::new_command_code_test(
        server.base.clone(),
        "test-key".into(),
        "meta/muse-ember",
    );
    let runtime = runtime();
    let result = runtime.block_on(async {
        let mut stream = provider
            .complete(&[Message::user("hi")], &[], "system", None)
            .await
            .expect("complete");
        let mut saw_error = false;
        while let Some(item) = stream.next().await {
            if item.is_err() {
                saw_error = true;
            }
        }
        saw_error
    });
    assert!(result, "incomplete responses stream must error");
}

#[test]
fn entitlement_error_is_not_retried_as_transient() {
    let _env = TestEnv::new();
    let server = MockServer::start(vec![MockResponse {
        status: 403,
        body: r#"{"error":{"message":"MODEL_NOT_IN_PLAN","type":"permission_error"}}"#.to_string(),
    }]);
    let provider = OpenRouterProvider::new_command_code_test(
        server.base.clone(),
        "test-key".into(),
        "claude-sonnet-5",
    );
    let runtime = runtime();
    let error = runtime.block_on(async {
        let mut stream = provider
            .complete(&[Message::user("hi")], &[], "system", None)
            .await
            .expect("complete returns a stream");
        let mut error = None;
        while let Some(item) = stream.next().await {
            if let Err(item_error) = item {
                error = Some(item_error.to_string());
            }
        }
        error
    });
    let error = error.expect("403 must fail");
    assert!(error.contains("403"), "{error}");
    assert_eq!(server.requests().len(), 1, "403 must not be retried");
}

#[test]
fn rate_limit_is_retried_then_succeeds() {
    let _env = TestEnv::with_retries(2);
    let server = MockServer::start(vec![
        MockResponse {
            status: 429,
            body: r#"{"error":{"message":"rate limited","type":"rate_limit_error"}}"#.to_string(),
        },
        MockResponse {
            status: 200,
            body: chat_text_stream(),
        },
    ]);
    let provider = OpenRouterProvider::new_command_code_test(
        server.base.clone(),
        "test-key".into(),
        "deepseek/deepseek-v4-flash",
    );
    let runtime = runtime();
    let events = collect(&runtime, &provider, &[Message::user("hi")], &[]);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, StreamEvent::TextDelta(text) if text == "DONE"))
    );
    assert_eq!(server.requests().len(), 2, "429 must be retried once");
}

#[test]
fn server_error_exhausts_retry_and_reports_status() {
    let _env = TestEnv::with_retries(2);
    let server = MockServer::start(vec![
        MockResponse {
            status: 500,
            body: r#"{"error":{"message":"upstream failed","type":"server_error"}}"#.to_string(),
        },
        MockResponse {
            status: 500,
            body: r#"{"error":{"message":"upstream failed","type":"server_error"}}"#.to_string(),
        },
    ]);
    let provider = OpenRouterProvider::new_command_code_test(
        server.base.clone(),
        "test-key".into(),
        "deepseek/deepseek-v4-flash",
    );
    let runtime = runtime();
    let error = runtime.block_on(async {
        let mut stream = provider
            .complete(&[Message::user("hi")], &[], "system", None)
            .await
            .expect("complete returns a stream");
        let mut error = None;
        while let Some(item) = stream.next().await {
            if let Err(item_error) = item {
                error = Some(item_error.to_string());
            }
        }
        error
    });
    let error = error.expect("500 must fail after retry");
    assert!(error.contains("500"), "{error}");
    assert_eq!(server.requests().len(), 2, "one retry must be attempted");
}

#[test]
fn stalled_stream_can_be_cancelled_promptly() {
    let _env = TestEnv::new();
    let server = HangingServer::start();
    let provider = OpenRouterProvider::new_command_code_test(
        server.base.clone(),
        "test-key".into(),
        "deepseek/deepseek-v4-flash",
    );
    let runtime = runtime();
    let started = std::time::Instant::now();
    runtime.block_on(async {
        let mut stream = provider
            .complete(&[Message::user("hi")], &[], "system", None)
            .await
            .expect("complete");
        let partial = tokio::time::timeout(Duration::from_secs(1), async {
            while let Some(item) = stream.next().await {
                match item.expect("event") {
                    StreamEvent::TextDelta(text) if text == "partial" => return true,
                    _ => {}
                }
            }
            false
        })
        .await
        .expect("partial event must arrive before the idle timeout");
        assert!(partial, "partial text event not found");
        drop(stream);
    });
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "cancelling a stalled stream must not wait for the idle timeout"
    );
}

#[test]
fn concurrent_streams_do_not_cross_talk() {
    let _env = TestEnv::new();
    let server = MockServer::start(vec![
        MockResponse {
            status: 200,
            body: chat_text_stream(),
        },
        MockResponse {
            status: 200,
            body: chat_text_stream(),
        },
    ]);
    let provider = OpenRouterProvider::new_command_code_test(
        server.base.clone(),
        "test-key".into(),
        "deepseek/deepseek-v4-flash",
    );
    let runtime = runtime();
    let (first, second) = runtime.block_on(async {
        let first = async {
            let mut stream = provider
                .complete(&[Message::user("first")], &[], "system", None)
                .await
                .expect("first complete");
            let mut events = Vec::new();
            while let Some(item) = stream.next().await {
                events.push(item.expect("first event"));
            }
            events
        };
        let second = async {
            let mut stream = provider
                .complete(&[Message::user("second")], &[], "system", None)
                .await
                .expect("second complete");
            let mut events = Vec::new();
            while let Some(item) = stream.next().await {
                events.push(item.expect("second event"));
            }
            events
        };
        tokio::join!(first, second)
    });
    for events in [first, second] {
        assert!(
            events
                .iter()
                .any(|event| matches!(event, StreamEvent::TextDelta(text) if text == "DONE"))
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, StreamEvent::MessageEnd { .. }))
        );
    }
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn saved_account_pin_survives_global_active_switch() {
    let _env = TestEnv::new();
    let _home = HomeGuard::new();
    use jcode_base::provider_catalog::command_code_accounts;
    command_code_accounts::save_account(Some("alpha"), "alpha-key").expect("save alpha");
    command_code_accounts::save_account(Some("beta"), "beta-key").expect("save beta");
    command_code_accounts::set_active("alpha").expect("select alpha");

    let server = MockServer::start(vec![
        MockResponse {
            status: 200,
            body: chat_text_stream(),
        },
        MockResponse {
            status: 200,
            body: chat_text_stream(),
        },
        MockResponse {
            status: 200,
            body: chat_text_stream(),
        },
    ]);
    let provider = OpenRouterProvider::new_command_code_test(
        server.base.clone(),
        "fallback-key".into(),
        "deepseek/deepseek-v4-flash",
    );
    assert_eq!(
        provider.bind_session_account(None).expect("pin alpha"),
        Some("alpha".to_string())
    );

    command_code_accounts::set_active("beta").expect("switch global active");
    let runtime = runtime();
    for prompt in ["first", "second"] {
        collect(&runtime, &provider, &[Message::user(prompt)], &[]);
    }

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        let lower = request.to_ascii_lowercase();
        assert!(
            lower.contains("authorization: bearer alpha-key"),
            "pinned session lost alpha: {request}"
        );
        assert!(!lower.contains("beta-key"), "session rebound to beta");
    }

    let new_provider = OpenRouterProvider::new_command_code_test(
        server.base.clone(),
        "fallback-key".into(),
        "deepseek/deepseek-v4-flash",
    );
    assert_eq!(
        new_provider.bind_session_account(None).expect("pin beta"),
        Some("beta".to_string())
    );
    collect(&runtime, &new_provider, &[Message::user("new")], &[]);
    let last = server.requests().pop().expect("third request");
    assert!(
        last.to_ascii_lowercase()
            .contains("authorization: bearer beta-key"),
        "new session must use the current active account: {last}"
    );
}
