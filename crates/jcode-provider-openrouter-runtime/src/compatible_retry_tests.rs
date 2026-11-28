use super::*;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

#[test]
fn local_endpoint_hint_mentions_ollama_actions() {
    let hint = local_endpoint_troubleshooting_hint("http://localhost:11434/v1", "llama3.2");
    assert!(hint.contains("ollama serve"));
    assert!(hint.contains("ollama pull"));
    assert!(hint.contains("--provider ollama"));
}

#[test]
fn local_endpoint_hint_mentions_lm_studio_server() {
    let hint = local_endpoint_troubleshooting_hint("http://127.0.0.1:1234/v1", "local-model");
    assert!(hint.contains("LM Studio"));
    assert!(hint.contains("Local Server"));
    assert!(hint.contains("/v1/models"));
}

/// #1596: an OpenAI-compatible provider answering 529 "heavy usage" (or
/// any other 5xx) must be retried, not fail the turn on the first try.
#[test]
fn every_5xx_status_including_529_is_retryable() {
    let body = r#"data: {"error":{"message":"We're experiencing heavy usage right now, please try again in a moment.","type":"server_error"}}"#;
    for status in [
        "500 internal server error",
        "502 bad gateway",
        "503 service unavailable",
        "520 <unknown status code>",
        "529 <unknown status code>",
    ] {
        let err = format!(
            "openai-compatible chat request failed\n  endpoint: https://compat.example.test/v1/chat/completions\n  model: glm-5.3\n  auth: compat_api_key\n  status: {status}\n  response: {body}"
        )
        .to_lowercase();
        assert!(is_retryable_error(&err), "{status} must be retried");
    }
}

#[test]
fn server_error_hint_points_at_provider_not_network() {
    let hint = http_status_hint(529, "https://compat.example.test/v1", "glm-5.3");
    assert!(hint.contains("overloaded"), "{hint}");
    assert!(!hint.contains("check network"), "{hint}");
}

#[test]
fn parsed_http_status_extracts_code() {
    assert_eq!(
        parsed_http_status("status: 402 payment required"),
        Some(402)
    );
    assert_eq!(parsed_http_status("  status:404 not found"), Some(404));
    assert_eq!(parsed_http_status("no status here"), None);
    // Embedded numbers elsewhere must not be misread as a status.
    assert_eq!(parsed_http_status("you requested 65536 tokens"), None);
}

#[test]
fn shared_status_adapter_ignores_metadata_before_permanent_failure() {
    let error =
        "endpoint: HTTP 529\nmodel: status: 503\nauth: HTTP 402\nHTTP 401: temporarily unavailable";
    assert!(!is_retryable_error(error));
}

#[test]
fn chinese_gateway_unavailable_event_is_retried() {
    assert!(is_retryable_error(
        "服务暂时不可用。 原因：上游服务、网络链路或代理返回异常响应。 解决方案：请稍后重试。"
    ));
}

#[test]
fn shared_status_adapter_retries_relayed_529() {
    assert!(is_retryable_error("HTTP 529: upstream busy"));
}

#[test]
fn payment_required_is_not_retryable() {
    let err = "openai-compatible chat request failed\n  endpoint: \
        https://openrouter.ai/api/v1/chat/completions\n  model: openai/gpt-5.4\n  \
        auth: openrouter_api_key\n  status: 402 payment required\n  response: \
        {\"error\":{\"message\":\"this request requires more credits, or fewer \
        max_tokens. you requested up to 65536 tokens, but can only afford 34424\"}}";
    assert!(!is_retryable_error(err));
}

/// Field report: OpenCode Zen free models answer 429 FreeUsageLimitError.
/// That was retried 8 times with backoff, so `jcode run` printed nothing.
#[test]
fn exhausted_quota_429_fails_fast_but_rate_limit_429_retries() {
    let quota = "openai-compatible chat request failed\n  status: 429 Too Many Requests\n  response: {\"type\":\"error\",\"error\":{\"type\":\"FreeUsageLimitError\",\"message\":\"Free usage limit reached.\"}}"
        .to_lowercase();
    assert!(!is_retryable_error(&quota));
    let openai =
        "status: 429 too many requests\n  response: {\"error\":{\"code\":\"insufficient_quota\"}}";
    assert!(!is_retryable_error(openai));
    let rate = "status: 429 too many requests\n  response: {\"error\":{\"message\":\"rate limit exceeded, retry in 2s\"}}";
    assert!(is_retryable_error(rate));
}

#[test]
fn client_errors_are_not_retryable() {
    for status in [400u16, 401, 402, 403, 404, 405, 406, 422] {
        let err = format!("chat request failed\n  status: {status} client error");
        assert!(
            !is_retryable_error(&err),
            "status {status} should not be retryable"
        );
    }
}

#[test]
fn server_errors_remain_retryable() {
    assert!(is_retryable_error(
        "chat request failed\n  status: 503 service unavailable"
    ));
    assert!(is_retryable_error(
        "chat request failed\n  status: 500 internal server error"
    ));
    // Provider overload messages should still be retried.
    assert!(is_retryable_error("overloaded"));
}

#[test]
fn http_429_is_retryable_without_rate_limit_words_in_body() {
    assert!(is_retryable_error(
        "chat request failed\n  status: 429 unknown\n  response: {}"
    ));
}

/// Openference DNS failures reach the retry classifier as the full
/// context + cause chain (`{:#}`, then lowercased by the loop). The
/// shared transport classifier already knows these connect-phase faults;
/// these regressions lock the exact real-world resolver wordings (macOS
/// and Linux getaddrinfo, plus the temporary-resolution failure some
/// resolvers report) so a connect-phase DNS fault never surfaces as a
/// failed turn instead of a retry.
#[test]
fn dns_connect_chain_is_retryable_across_platform_wordings() {
    let chains = [
        // reqwest 0.12 + hyper-util on macOS
        "Failed to send OpenAI-compatible chat request\n  endpoint: https://api.openference.com/v1/chat/completions\n  model: glm-5.3\n  auth: api key\nHint: check network connectivity, DNS/TLS, that the base URL includes the API version (usually /v1), and that the model exists on the provider.: error sending request for url (https://api.openference.com/v1/chat/completions): client error (Connect): dns error: failed to lookup address information: nodename nor servname provided, or not known",
        // Linux glibc resolver wording
        "Failed to send OpenAI-compatible chat request\n  endpoint: https://api.openference.com/v1/chat/completions\n  model: glm-5.3\n  auth: api key\nHint: check network connectivity, DNS/TLS, that the base URL includes the API version (usually /v1), and that the model exists on the provider.: error sending request for url (https://api.openference.com/v1/chat/completions): client error (Connect): dns error: failed to lookup address information: Name or service not known",
        // Some resolvers report it as a temporary failure instead
        "error sending request for url (https://api.openference.com/v1/chat/completions): error trying to connect: dns error: temporary failure in name resolution",
    ];
    for chain in &chains {
        assert!(
            is_retryable_error(&chain.to_lowercase()),
            "DNS connect-phase chain must be retried: {chain}"
        );
    }
}

/// Openference answers 429 with the requested delay only in the JSON
/// body (`retry_after_seconds: 2`, `max_rpm: 25`) and no `Retry-After`
/// header. The stream path must classify the response as retryable and
/// carry the exact 2s to the retry loop's delay selection instead of
/// falling back to jittered exponential backoff, and the hint must point
/// at the rate limit, not the network.
#[test]
fn openference_429_body_delay_flows_to_retry_loop() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(async {
        let body = r#"{"error":"Rate limit exceeded. Too many requests per minute.","type":"rate_limit_error","code":"rate_limit_exceeded","retry_after_seconds":2,"max_rpm":25}"#;
        let response = format!(
            "HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind rate limit server");
        let addr = listener.local_addr().expect("rate limit server addr");
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept rate limited request");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("set read timeout");
            let mut request = vec![0u8; 65536];
            let _ = stream.read(&mut request);
            stream.write_all(response.as_bytes()).expect("write 429");
        });
        let api_base = format!("http://{addr}/v1");

        let (tx, _rx) = mpsc::channel(16);
        let err = stream_response(
            Client::new(),
            api_base,
            ProviderAuth::None {
                label: "test".to_string(),
            },
            false,
            "conv",
            &serde_json::json!({"model": "glm-5.3", "messages": [], "stream": true}),
            false, // Not Azure Responses.
            tx,
            Arc::new(Mutex::new(None)),
            "glm-5.3".to_string(),
            false, // Exercise chat-completions, not the Responses API.
        )
        .await
        .expect_err("429 must surface as an error");

        let error_str = format!("{err:#}");
        assert!(error_str.contains("status: 429"), "{error_str}");
        // The retry loop classifies this exact formatted error before
        // deciding, so the body-only delay must not change retryability.
        assert!(
            is_retryable_error(&error_str.to_lowercase()),
            "429 with body-only delay must be retried"
        );
        assert!(
            error_str.contains("rate limited"),
            "429 hint must name the rate limit: {error_str}"
        );
        assert!(
            !error_str.contains("network connectivity"),
            "429 is not a connectivity problem: {error_str}"
        );
        // The exact server-requested 2s must be recoverable by the outer
        // loop's retry_after_from_error, not replaced by default backoff.
        let delay =
            jcode_provider_core::retry_after::retry_after_from_error(&err).expect("hint");
        assert!(delay <= Duration::from_secs(2), "{delay:?}");
        assert!(delay > Duration::from_millis(1500), "{delay:?}");
    });
}

/// A resettable billing 402 must stay fatal for the retry loop (retrying
/// against an exhausted balance is futile), but the server's reset delay
/// and the billing hint must survive in the surfaced error so consumers
/// can hold the turn until the allowance resets.
#[test]
fn billing_402_stays_fatal_but_carries_reset_delay() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(async {
        let body = r#"{"error":"Insufficient credits. Your daily allowance resets soon.","retry_after_seconds":30}"#;
        let response = format!(
            "HTTP/1.1 402 Payment Required\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind billing server");
        let addr = listener.local_addr().expect("billing server addr");
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept billing request");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("set read timeout");
            let mut request = vec![0u8; 65536];
            let _ = stream.read(&mut request);
            stream.write_all(response.as_bytes()).expect("write 402");
        });
        let api_base = format!("http://{addr}/v1");

        let (tx, _rx) = mpsc::channel(16);
        let err = stream_response(
            Client::new(),
            api_base,
            ProviderAuth::None {
                label: "test".to_string(),
            },
            false,
            "conv",
            &serde_json::json!({"model": "glm-5.3", "messages": [], "stream": true}),
            false, // Not Azure Responses.
            tx,
            Arc::new(Mutex::new(None)),
            "glm-5.3".to_string(),
            false, // Exercise chat-completions, not the Responses API.
        )
        .await
        .expect_err("402 must surface as an error");

        let error_str = format!("{err:#}");
        assert!(error_str.contains("status: 402"), "{error_str}");
        assert!(
            !is_retryable_error(&error_str.to_lowercase()),
            "billing 402 must stay fatal for the retry loop"
        );
        assert!(error_str.contains("billing limit"), "{error_str}");
        assert!(
            error_str.contains("may recover after its reset time"),
            "{error_str}"
        );
        assert!(
            !error_str.contains("network connectivity"),
            "402 is not a connectivity problem: {error_str}"
        );
        // The reset delay survives in the chain for consumers that hold
        // the turn instead of surfacing it as a hard failure.
        let delay =
            jcode_provider_core::retry_after::retry_after_from_error(&err).expect("hint");
        assert!(delay <= Duration::from_secs(30), "{delay:?}");
        assert!(delay > Duration::from_secs(29), "{delay:?}");
    });
}

/// A provider asking for 60s must get 60s, not the 30s default backoff
/// cap, or the resend draws another 429. The hint promises exactly this.
#[test]
fn retry_after_is_honored_past_the_backoff_cap() {
    let cap = Duration::from_secs(30);
    assert_eq!(
        retry_wait(1, Some(Duration::from_secs(60)), cap),
        Duration::from_secs(60)
    );
    assert_eq!(
        retry_wait(1, Some(Duration::from_secs(600)), cap),
        jcode_provider_core::retry_after::MAX_RETRY_AFTER
    );
    assert!(retry_wait(10, None, cap) <= cap);
    assert_eq!(
        jcode_provider_core::retry_after::MAX_RETRY_AFTER,
        Duration::from_secs(60),
        "RATE_LIMIT_HINT names this bound"
    );
    assert!(RATE_LIMIT_HINT.contains("up to 60s"));
}

/// Cancel only after a visible failed attempt enters a server-requested wait.
#[test]
fn cancelled_stream_stops_retrying_and_wakes_from_backoff() {
    let run = run_with_failing_first_attempt(
        format!(
            "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            serde_json::json!({"choices":[{"delta":{"content":"partial"}}]}),
            serde_json::json!({"error":{"message":"temporarily unavailable","code":503,"retry_after_seconds":60}}),
        ),
        true,
    );
    assert_eq!(
        run.rollbacks, 1,
        "receiver stayed open through the failed attempt"
    );
    assert_eq!(
        run.connections, 1,
        "canceled backoff must not send another request"
    );
    assert!(run.errors.is_empty());
}

/// The exact error a local proxy relayed while its upstream was
/// deploying: a 200 SSE stream carrying `HTTP 503: {...DEPLOYING...}`.
const PROXY_DEPLOYING_503: &str = r#"HTTP 503: {"error":"We're deploying an update right now ⚙️ It only takes a few minutes and everything comes back on its own. Please try again shortly.","code":"DEPLOYING"}"#;

#[test]
fn in_stream_deploying_503_is_retryable_with_a_long_wait() {
    let lower = PROXY_DEPLOYING_503.to_lowercase();
    assert_eq!(parsed_http_status(&lower), Some(503));
    assert!(is_retryable_error(&lower));
    assert!(is_deploy_message(&lower));
    assert_eq!(DEPLOY_RETRY_WAIT, Duration::from_secs(30));
    assert!(DEPLOY_RETRY_WAIT <= jcode_provider_core::retry_after::MAX_RETRY_AFTER);
    // Wording alone, without a status, still retries.
    assert!(is_retryable_error(
        "we're deploying an update right now. please try again shortly."
    ));
    // An in-stream permanent failure keeps failing fast.
    assert!(!is_retryable_error(
        r#"http 401: {"error":"invalid api key"}"#
    ));
    // URLs never look like an `HTTP <code>` status.
    assert_eq!(
        parsed_http_status("endpoint: http://127.0.0.1:8317/v1 failed"),
        None
    );
}

/// The answer a provider streams once it is healthy again.
const HEALTHY_ANSWER_SSE: &str = concat!(
    "data: {\"choices\":[{\"delta\":{\"content\":\"back online\"}}]}\n\n",
    "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
    "data: [DONE]\n\n"
);

/// What the turn received from one `run_stream_with_retries` call.
struct RetryRun {
    /// Text deltas, with any rolled-back partial attempt discarded the way
    /// the agent discards it on `RetryRollback`.
    text: String,
    errors: Vec<String>,
    rollbacks: usize,
    connections: usize,
}

/// Serve `first_body` as a 200 SSE response to the first request and a
/// healthy answer to the second, then run the real retry loop against it.
fn run_with_failing_first_attempt(first_body: String, cancel_on_rollback: bool) -> RetryRun {
    let _lock = crate::tests::ENV_LOCK.lock();
    let home = tempfile::tempdir().unwrap();
    let _home = crate::tests::EnvVarGuard::set("JCODE_HOME", home.path());
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let connections = Arc::new(AtomicUsize::new(0));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let counted = Arc::clone(&connections);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let server = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) && counted.load(Ordering::SeqCst) < 2 {
                let (mut socket, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("accept: {error}"),
                };
                socket.set_nonblocking(false).unwrap();
                socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut buffer = [0; 4096];
                    let count = socket.read(&mut buffer).unwrap();
                    if count == 0 { return; }
                    bytes.extend_from_slice(&buffer[..count]);
                    assert!(bytes.len() <= 65_536);
                    if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                        let length: usize = headers.lines().find_map(|line| line.strip_prefix("content-length: ")).unwrap().parse().unwrap();
                        if bytes.len() >= end + 4 + length { break; }
                    }
                }
                let attempt = counted.fetch_add(1, Ordering::SeqCst);
                let body = if attempt == 0 { &first_body } else { HEALTHY_ANSWER_SSE };
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                let _ = socket.write_all(response.as_bytes());
            }
        });
        let (tx, mut rx) = mpsc::channel(64);
        let run = tokio::spawn(async move {
            run_stream_with_retries(
                Client::new(), format!("http://{addr}/v1"),
                ProviderAuth::None { label: "test".into() }, false, "conv".into(),
                serde_json::json!({"model":"ntl/claude","messages":[],"stream":true}),
                false, tx, Arc::new(Mutex::new(None)), "ntl/claude".into(), false,
            ).await;
        });
        let mut result = RetryRun { text: String::new(), errors: Vec::new(), rollbacks: 0, connections: 0 };
        while let Some(item) = rx.recv().await {
            match item {
                Ok(StreamEvent::TextDelta(delta)) => result.text.push_str(&delta),
                Ok(StreamEvent::RetryRollback { .. }) => {
                    result.rollbacks += 1;
                    result.text.clear();
                    if cancel_on_rollback {
                        // Let the producer park in backoff while the receiver remains open.
                        tokio::task::yield_now().await;
                        break;
                    }
                }
                Ok(StreamEvent::Error { message, .. }) => result.errors.push(message),
                Err(error) => result.errors.push(format!("{error:#}")),
                Ok(_) => {}
            }
        }
        drop(rx);
        let completion = if cancel_on_rollback {
            tokio::time::timeout(Duration::from_secs(1), run).await
        } else {
            Ok(run.await)
        };
        stop.store(true, Ordering::SeqCst);
        server.join().unwrap();
        completion.expect("cancellation must wake the 60-second backoff").expect("retry loop task");
        result.connections = connections.load(Ordering::SeqCst);
        result
    })
}

/// End to end: the first attempt gets a 200 stream whose only event is
/// the relayed deploy 503, the second gets a normal answer. The turn must
/// see the answer, not the error.
#[test]
fn in_stream_deploying_503_is_retried_until_the_provider_answers() {
    let run = run_with_failing_first_attempt(
        format!(
            "data: {}\n\ndata: [DONE]\n\n",
            serde_json::json!({"error": {"message": PROXY_DEPLOYING_503}})
        ),
        false,
    );
    assert!(
        run.errors.is_empty(),
        "deploy 503 must not reach the turn: {:?}",
        run.errors
    );
    assert_eq!(run.text, "back online");
    assert_eq!(run.connections, 2, "one retry");
}

/// Openference ends a long stream with this error event when its own
/// upstream drops mid-response.
const INTERRUPTED_STREAM: &str = "The model provider's stream was interrupted. Please retry.";

#[test]
fn interrupted_stream_message_is_retryable() {
    let lower = INTERRUPTED_STREAM.to_lowercase();
    assert!(is_interrupted_stream_message(&lower));
    assert!(is_retryable_error(&lower));
    // The retry loop sees it wrapped in the attempt's error context.
    assert!(is_retryable_error(
        &format!(
            "openai-compatible stream error\n  endpoint: https://api.openference.com/v1/chat/completions\n  model: glm-5.3\n  auth: key\n  error: {INTERRUPTED_STREAM}"
        )
        .to_lowercase()
    ));
}

/// End to end: the provider streams part of an answer, then reports the
/// interruption. The partial attempt is rolled back and the request
/// resent, so the turn sees one clean answer and no error.
#[test]
fn interrupted_stream_mid_answer_is_rolled_back_and_retried() {
    let run = run_with_failing_first_attempt(
        format!(
            "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            serde_json::json!({"choices": [{"delta": {"content": "half an ans"}}]}),
            serde_json::json!({"error": {"message": INTERRUPTED_STREAM}})
        ),
        false,
    );
    assert!(
        run.errors.is_empty(),
        "an interrupted stream must not end the turn: {:?}",
        run.errors
    );
    assert_eq!(run.rollbacks, 1, "partial attempt rolled back");
    assert_eq!(run.text, "back online");
    assert_eq!(run.connections, 2, "one retry");
}

/// Openference answered a busy period with a 529 and a server_error body
/// ("heavy usage ... please try again in a moment"). That is temporary:
/// it must be retried, not reported as a failed turn at once.
#[test]
fn provider_overload_529_is_retryable() {
    let err = "openai-compatible chat request failed\n  endpoint: \
        https://api.openference.com/v1/chat/completions\n  model: glm-5.3\n  \
        status: 529 <unknown status code>\n  response: data: {\"error\":{\"message\":\
        \"we're experiencing heavy usage right now, which may cause increased latency \
        or temporary unavailability. we're working on adding more capacity, please try \
        again in a moment.\",\"type\":\"server_error\"}}data: [done]";
    assert!(is_retryable_error(err));
    for status in [500u16, 501, 502, 503, 504, 520, 529, 599] {
        let err = format!("chat request failed\n  status: {status} whatever\n  response: {{}}");
        assert!(is_retryable_error(&err), "status {status} should retry");
    }
    // The same wording inside a stream error (no HTTP status) retries too.
    assert!(is_retryable_error(
        "openai-compatible stream error\n  error: we're experiencing heavy usage right now"
    ));
    // Hint names the provider, not the network.
    let hint = http_status_hint(529, "https://api.openference.com/v1", "glm-5.3");
    assert!(hint.contains("overloaded"), "{hint}");
    assert!(!hint.contains("network connectivity"), "{hint}");
}

/// Openference answers an outage of its own upstream with a 429. The
/// hint must name the outage, not tell the user they hit a rate limit.
#[test]
fn unavailable_429_gets_the_outage_hint_not_the_rate_limit_hint() {
    let api = "https://api.openference.com/v1";
    let outage = r#"data: {"error":{"message":"The model provider is temporarily unavailable. Please try again in a moment.","type":"server_error"}}data: [DONE]"#;
    let hint = response_hint(429, outage, api, "glm-5.3");
    assert!(hint.contains("temporary server problem"), "{hint}");
    assert!(!hint.contains("rate limited"), "{hint}");
    // Through a local proxy (127.0.0.1:8317) it is the same outage, not
    // a local server that is down.
    let via_proxy = response_hint(429, outage, "http://127.0.0.1:8317/v1", "glm-5.3");
    assert_eq!(via_proxy, PROVIDER_OUTAGE_HINT);
    // A real rate limit keeps the rate-limit hint.
    let limited = r#"{"error":"Rate limit exceeded. Too many requests per minute."}"#;
    let rate_limit_hint = http_status_hint(429, api, "glm-5.3");
    assert_eq!(response_hint(429, limited, api, "glm-5.3"), rate_limit_hint);
    assert_eq!(response_hint(429, "", api, "glm-5.3"), rate_limit_hint);
}

/// A 402 means the server answered: the hint must name the billing limit,
/// not send the user to check their network.
#[test]
fn status_hint_names_billing_for_402_not_network() {
    let hint = http_status_hint(402, "https://apiduck.servepics.com/v1", "glm-5.3");
    assert!(hint.contains("billing limit"), "{hint}");
    assert!(hint.contains("/model"), "{hint}");
    assert!(!hint.contains("network connectivity"), "{hint}");

    for status in [401u16, 403] {
        let hint = http_status_hint(status, "https://api.example.com/v1", "m");
        assert!(hint.contains("API key"), "{status}: {hint}");
        assert!(!hint.contains("network connectivity"), "{status}: {hint}");
    }
    assert!(http_status_hint(404, "https://api.example.com/v1", "m").contains("/v1"));
    // Local servers keep their own advice (e.g. `ollama pull` for a 404).
    assert!(http_status_hint(404, "http://localhost:11434/v1", "llama3.2").contains("ollama pull"));
    // Server errors keep the endpoint-specific advice.
    assert!(
        http_status_hint(503, "http://localhost:11434/v1", "llama3.2").contains("ollama serve")
    );
}

#[test]
fn metadata_model_and_endpoint_digits_do_not_make_a_failure_retryable() {
    for message in [
        "endpoint: https://api.test/503\nmodel: model-500\nerror: invalid model",
        "model529 failed",
        "model: status: 503\nendpoint: HTTP 529\nauth: HTTP 500",
    ] {
        assert!(!is_retryable_error(message), "{message}");
    }
}

#[test]
fn common_transient_sse_errors_retry_but_permanent_and_quota_errors_do_not() {
    for error in [
        serde_json::json!({"message":"Rate limit exceeded","type":"rate_limit_error"}),
        serde_json::json!({"message":"Internal server error","type":"server_error"}),
        serde_json::json!({"message":"Please try later","code":503}),
        serde_json::json!("Rate limit exceeded"),
    ] {
        let run = run_with_failing_first_attempt(
            format!(
                "data: {}\n\ndata: [DONE]\n\n",
                serde_json::json!({"error":error})
            ),
            false,
        );
        assert!(run.errors.is_empty(), "{:?}", run.errors);
        assert_eq!(run.text, "back online");
        assert_eq!(run.connections, 2);
    }
    for error in [
        serde_json::json!({"message":"Rate limit exceeded","code":"insufficient_quota"}),
        serde_json::json!({"message":"temporarily unavailable","type":"authentication_error"}),
        serde_json::json!({"message":"Internal server error","code":400}),
    ] {
        let run = run_with_failing_first_attempt(
            format!(
                "data: {}\n\ndata: [DONE]\n\n",
                serde_json::json!({"error":error})
            ),
            false,
        );
        assert_eq!(run.errors.len(), 1);
        assert_eq!(run.connections, 1);
    }
    assert!(is_retryable_error(
        "model: billing-503\nerror: rate limit exceeded"
    ));
}
