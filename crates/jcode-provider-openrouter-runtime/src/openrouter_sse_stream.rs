use super::*;
use jcode_provider_openrouter::stream::OpenRouterStream;

/// Whether the parsed host is exactly `localhost` or a loopback address. A
/// prefix match would exempt remote hosts such as `localhost.example.com`.
fn is_loopback_base(api_base: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(api_base) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Client for loopback bases, which skip the PAN check: it never follows a
/// redirect and never goes through a system proxy, so the body cannot be
/// forwarded off the machine by the HTTP layer. `None` if it cannot be built,
/// in which case the loopback base loses its exemption.
fn loopback_client() -> Option<Client> {
    static CLIENT: OnceLock<Option<Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            let built = Client::builder()
                .user_agent(jcode_provider_core::JCODE_USER_AGENT)
                .connect_timeout(std::time::Duration::from_secs(15))
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .build();
            match built {
                Ok(client) => Some(client),
                Err(error) => {
                    jcode_base::logging::warn(&format!(
                        "loopback HTTP client unavailable: {error}"
                    ));
                    None
                }
            }
        })
        .clone()
}

/// Only the documented `=1` opts out; `0` or any other value keeps the check.
fn pan_check_opted_out(value: Option<&str>) -> bool {
    value.is_some_and(|value| value.trim() == "1")
}

/// Fail closed before a payment card number leaves the machine. A loopback
/// base sent through `loopback_client` (no redirects, no proxy) is exempt, and
/// `JCODE_DISABLE_PAN_CHECK=1` is the escape hatch for a false positive. The
/// error names the message, never the value, and keeps the gateway's wording so
/// the TUI stops auto-retry.
fn pan_pre_send_block(loopback_exempt: bool, request: &Value) -> Option<anyhow::Error> {
    if loopback_exempt
        || std::env::var("JCODE_DISABLE_PAN_CHECK")
            .is_ok_and(|value| pan_check_opted_out(Some(&value)))
    {
        return None;
    }
    let finding =
        jcode_provider_core::failover::pan_check::find_pan_in_messages(request.get("messages")?)?;
    Some(anyhow::anyhow!(
        "Local pre-send check: request blocked by content filter: [PAN]\n  \
         a payment card number (issuer prefix + Luhn) is in message #{} (role: {}); nothing was sent.\n  \
         Remove it from the conversation (compact or start a new session), or set \
         JCODE_DISABLE_PAN_CHECK=1 if this is a false positive.",
        finding.message_index,
        finding.role
    ))
}

fn local_endpoint_troubleshooting_hint(api_base: &str, model: &str) -> &'static str {
    let lower = api_base.to_ascii_lowercase();
    if lower.contains("localhost:11434") || lower.contains("127.0.0.1:11434") {
        return "Ollama hint: make sure `ollama serve` is running, the model is installed with `ollama pull <model>`, and run jcode with an installed model, for example `jcode --provider ollama --model llama3.2 run 'hello'`. If replies ignore earlier turns, Ollama is truncating the prompt to its serving context: restart it with a larger window, e.g. `OLLAMA_CONTEXT_LENGTH=65536 ollama serve`.";
    }

    if lower.contains("localhost:1234") || lower.contains("127.0.0.1:1234") {
        return "LM Studio hint: start the Local Server in LM Studio, load a chat model, and run jcode with the exact model id shown by LM Studio's /v1/models endpoint.";
    }

    if lower.contains("localhost") || lower.contains("127.0.0.1") || lower.contains("[::1]") {
        return "Local endpoint hint: make sure the server is running, the base URL includes /v1, the selected model is loaded, and the server supports streaming POST /chat/completions.";
    }

    let _ = model;
    "Hint: check network connectivity, DNS/TLS, that the base URL includes the API version (usually /v1), and that the model exists on the provider."
}

// ============================================================================
// SSE Stream Parser
// ============================================================================

#[expect(
    clippy::too_many_arguments,
    reason = "stream helpers thread transport, auth, request, event channel, and pin state explicitly"
)]
pub(super) async fn run_stream_with_retries(
    client: Client,
    api_base: String,
    auth: ProviderAuth,
    send_openrouter_headers: bool,
    conversation_id: String,
    request: Value,
    tx: mpsc::Sender<Result<StreamEvent>>,
    provider_pin: Arc<Mutex<Option<ProviderPin>>>,
    model: String,
) {
    let loopback = is_loopback_base(&api_base).then(loopback_client).flatten();
    if let Some(blocked) = pan_pre_send_block(loopback.is_some(), &request) {
        if tx.send(Err(blocked)).await.is_err() {
            jcode_base::logging::info("card-number block not delivered: stream receiver dropped");
        }
        return;
    }
    let mut last_error = None;
    let mut next_retry_delay = None;
    let config = jcode_base::config::config();
    let max_retries = config.provider.max_retries.max(1);
    let retry_backoff_cap =
        std::time::Duration::from_secs(config.provider.retry_backoff_cap_secs.max(1));

    for attempt in 0..max_retries {
        if attempt > 0 {
            let delay = jcode_provider_core::retry_after::retry_delay(
                attempt,
                RETRY_BASE_DELAY_MS,
                next_retry_delay.take(),
            )
            .min(retry_backoff_cap);
            tokio::time::sleep(delay).await;
            jcode_base::logging::info(&format!(
                "Retrying API request using {} (attempt {}/{})",
                auth.label(),
                attempt + 1,
                max_retries
            ));
        }

        jcode_base::logging::info(&format!(
            "API stream attempt {}/{} over HTTPS transport (model: {}, endpoint: {}, auth: {})",
            attempt + 1,
            max_retries,
            model,
            api_base,
            auth.label()
        ));

        // Track whether this attempt streams replay-visible output so a
        // mid-stream transport fault can roll the partial output back on the
        // consumer before the retry replays the response from the top.
        let (attempt_tx, attempt_guard) =
            jcode_provider_core::attempt_tracker::track_attempt_output(tx.clone());

        // Retries use a fresh unpooled client: the fault that broke attempt N
        // (e.g. TLS BadRecordMac from a corrupting middlebox) may also have
        // poisoned other idle pooled connections opened through the same path,
        // so reusing the shared pool can fail identically. A fresh client
        // guarantees a brand-new TCP+TLS connection.
        let attempt_client = if let Some(loopback) = &loopback {
            loopback.clone()
        } else if attempt == 0 {
            client.clone()
        } else {
            jcode_provider_core::fresh_transport_client()
        };

        match stream_response(
            attempt_client,
            api_base.clone(),
            auth.clone(),
            send_openrouter_headers,
            &conversation_id,
            request.clone(),
            attempt_tx,
            Arc::clone(&provider_pin),
            model.clone(),
        )
        .await
        {
            Ok(()) => {
                let _ = attempt_guard.finish().await;
                return;
            }
            Err(e) => {
                let saw_output = attempt_guard.finish().await;
                // Full anyhow chain ({:#}) so a `.context(...)`-wrapped transport
                // cause (e.g. TLS BadRecordMac) is visible to the classifier.
                let error_str = format!("{e:#}").to_lowercase();
                if should_retry(&error_str, attempt, max_retries) {
                    if saw_output {
                        // Partial output already reached the consumer; tell it
                        // to discard the partial attempt so the retried
                        // response replays cleanly instead of duplicating.
                        jcode_base::logging::warn(&format!(
                            "Transient API error after partial output; rolling back partial attempt and retrying: {}",
                            e
                        ));
                        let _ = tx
                            .send(Ok(StreamEvent::RetryRollback {
                                attempt: attempt + 2,
                                max: max_retries,
                            }))
                            .await;
                    } else {
                        jcode_base::logging::info(&format!(
                            "Transient API error, will retry: {}",
                            e
                        ));
                    }
                    next_retry_delay = jcode_provider_core::retry_after::retry_after_from_error(&e);
                    last_error = Some(e);
                    continue;
                }

                let _ = tx.send(Err(e)).await;
                return;
            }
        }
    }

    if let Some(e) = last_error {
        let _ = tx
            .send(Err(anyhow::anyhow!(
                "Failed after {} retries: {}",
                max_retries,
                e
            )))
            .await;
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "stream helpers thread transport, auth, request, event channel, and pin state explicitly"
)]
async fn stream_response(
    client: Client,
    api_base: String,
    auth: ProviderAuth,
    send_openrouter_headers: bool,
    conversation_id: &str,
    request: Value,
    tx: mpsc::Sender<Result<StreamEvent>>,
    provider_pin: Arc<Mutex<Option<ProviderPin>>>,
    model: String,
) -> Result<()> {
    use jcode_message_types::ConnectionPhase;
    let _ = tx
        .send(Ok(StreamEvent::ConnectionPhase {
            phase: ConnectionPhase::SendingRequest,
        }))
        .await;
    let connect_start = std::time::Instant::now();
    let stream_idle_timeout = jcode_base::provider::stream_idle_timeout();

    let url = format!("{}/chat/completions", api_base);
    let mut req = apply_kimi_coding_agent_headers(
        auth.apply(
            client
                .post(&url)
                .header("Content-Type", "application/json")
                .header("Accept-Encoding", "identity"),
        )
        .await?,
        &api_base,
        Some(&model),
    );

    if send_openrouter_headers {
        req = req
            .header("HTTP-Referer", "https://github.com/jcode")
            .header("X-Title", "jcode");
    }
    req = apply_opencode_session_header(req, &api_base, conversation_id);
    req = apply_grok_cli_turn_headers(req, &auth, &model, conversation_id);

    let response = jcode_provider_core::transport::send_with_initial_response_timeout(
        req.json(&request),
        stream_idle_timeout,
    )
    .await
    .with_context(|| {
        let hint = local_endpoint_troubleshooting_hint(&api_base, &model);
        format!(
            "Failed to send OpenAI-compatible chat request\n  endpoint: {}\n  model: {}\n  auth: {}\n{}",
            url,
            model,
            auth.label(),
            hint
        )
    })?;

    let connect_ms = connect_start.elapsed().as_millis();
    jcode_base::logging::info(&format!(
        "HTTP connection established in {}ms (status={})",
        connect_ms,
        response.status()
    ));

    if !response.status().is_success() {
        let status = response.status();
        let retry_after = jcode_provider_core::retry_after::retry_after(response.headers());
        let body = jcode_base::util::http_error_body(response, "HTTP error").await;
        let hint = local_endpoint_troubleshooting_hint(&api_base, &model);
        return Err(jcode_provider_core::retry_after::error_with_retry_after(
            format!(
                "OpenAI-compatible chat request failed\n  endpoint: {}\n  model: {}\n  auth: {}\n  status: {}\n  response: {}\n{}",
                url,
                model,
                auth.label(),
                status,
                body,
                hint
            ),
            retry_after,
        ));
    }

    let _ = tx
        .send(Ok(StreamEvent::ConnectionPhase {
            phase: ConnectionPhase::WaitingForResponse,
        }))
        .await;

    let mut stream = OpenRouterStream::new(response.bytes_stream(), model.clone(), provider_pin);

    // Idle timeout between streamed chunks. Configurable so slow reasoning
    // models (e.g. DeepSeek) that think silently for minutes before emitting
    // tokens don't trip a premature timeout (issue #196). Resolved from
    // `[provider] stream_idle_timeout_secs` / `JCODE_STREAM_IDLE_TIMEOUT_SECS`,
    // defaulting to 180s. Shared with the native provider paths (issue #434).
    let idle_timeout_secs = stream_idle_timeout.as_secs();

    loop {
        let event = match tokio::time::timeout(stream_idle_timeout, stream.next()).await {
            Ok(Some(Ok(event))) => event,
            Ok(Some(Err(e))) => anyhow::bail!(
                "OpenAI-compatible stream error\n  endpoint: {}\n  model: {}\n  auth: {}\n  error: {}",
                url,
                model,
                auth.label(),
                e
            ),
            Ok(None) => break, // stream ended normally
            Err(_) => {
                jcode_base::logging::warn(&format!(
                    "OpenRouter SSE stream timed out (no data for {}s)",
                    idle_timeout_secs
                ));
                anyhow::bail!(
                    "OpenAI-compatible stream timeout\n  endpoint: {}\n  model: {}\n  auth: {}\n  timeout: no data received for {} seconds\n{}",
                    url,
                    model,
                    auth.label(),
                    idle_timeout_secs,
                    local_endpoint_troubleshooting_hint(&api_base, &model)
                );
            }
        };
        if tx.send(Ok(event)).await.is_err() {
            return Ok(());
        }
    }

    Ok(())
}

/// Extract the HTTP status code reported in a formatted provider error string.
///
/// Error strings produced in this module embed the status as `status: <code>`
/// (e.g. `status: 402 Payment Required`). The input may be lowercased before
/// it reaches here, so matching is case-insensitive.
fn parsed_http_status(error_str: &str) -> Option<u16> {
    let lower = error_str.to_ascii_lowercase();
    let idx = lower.find("status:")?;
    let rest = lower[idx + "status:".len()..].trim_start();
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.len() == 3 {
        digits.parse().ok()
    } else {
        None
    }
}

/// Retries allowed for a 429 that OpenRouter attributes to the upstream
/// provider's shared key pool (`limit_source: upstream_provider_shared_pool`).
/// OpenRouter has already tried every provider it could route to before
/// answering, so a full backoff ladder mostly waits; measured 2026-09-24: 394
/// such 429s against `openrouter/auto`, each held for the whole budget.
const SHARED_POOL_429_MAX_RETRIES: u32 = 2;

/// Whether a failed zero-based `attempt` should be retried.
fn should_retry(error_str: &str, attempt: u32, max_retries: u32) -> bool {
    if !is_retryable_error(error_str) || attempt + 1 >= max_retries {
        return false;
    }
    let shared_pool_429 = parsed_http_status(error_str) == Some(429)
        && error_str.contains("upstream_provider_shared_pool");
    !(shared_pool_429 && attempt >= SHARED_POOL_429_MAX_RETRIES)
}

fn is_retryable_error(error_str: &str) -> bool {
    // Explicit non-retryable HTTP statuses take precedence over the loose
    // substring heuristics below. These are deterministic client-side failures
    // (auth, billing, malformed request) where retrying is futile and just
    // burns time/credits. 429 (rate limit) is classified explicitly so it does
    // not depend on provider-specific body wording.
    match parsed_http_status(error_str) {
        Some(400 | 401 | 402 | 403 | 404 | 405 | 406 | 422) => return false,
        Some(429) => return true,
        _ => {}
    }

    jcode_provider_core::is_transient_transport_error(error_str)
        || error_str.contains("stream error")
        || error_str.contains("eof")
        || error_str.contains("5")
            && (error_str.contains("50")
                || error_str.contains("502")
                || error_str.contains("503")
                || error_str.contains("504")
                || error_str.contains("internal server error"))
        || error_str.contains("overloaded")
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn shared_pool_429_retries_at_most_twice() {
        let pool = "status: 429 too many requests\n  response: {\"error\":{\"code\":429,\"metadata\":{\"limit_source\":\"upstream_provider_shared_pool\"}}}";
        assert!(should_retry(pool, 0, 8));
        assert!(should_retry(pool, 1, 8));
        assert!(!should_retry(pool, 2, 8), "three sends is the cap");
        // An ordinary rate limit keeps the configured ladder.
        assert!(should_retry("status: 429 too many requests", 5, 8));
        assert!(!should_retry("status: 429 too many requests", 7, 8));
        // The cap is for 429s only: a 503 naming the pool keeps its budget.
        let pool_503 = "status: 503 service unavailable\n  response: {\"error\":{\"metadata\":{\"limit_source\":\"upstream_provider_shared_pool\"}}}";
        assert!(should_retry(pool_503, 5, 8));
    }

    #[test]
    fn content_filter_block_is_never_retried() {
        let blocked = "status: 403 forbidden\n  response: {\"error\":{\"message\":\"request blocked by content filter: [credit_card]\",\"code\":403}}";
        assert!(!should_retry(blocked, 0, 8));
    }

    #[test]
    fn pan_pre_send_blocks_a_card_number_without_echoing_it() {
        let pan = "4242".repeat(4);
        let request = serde_json::json!({"messages": [
            {"role": "user", "content": "issues 1113 1114 1115 1116"},
            {"role": "assistant", "content": format!("card {pan}")},
        ]});
        let blocked = pan_pre_send_block(false, &request).expect("a PAN must block");
        let text = format!("{blocked:#}");
        assert!(text.contains("message #1 (role: assistant)"), "{text}");
        assert!(!text.contains(&pan), "the value must never be echoed");
        assert_eq!(
            jcode_provider_core::failover::content_filter_block_label(&text).as_deref(),
            Some("[PAN]"),
            "the TUI fail-fast path must recognise the local block"
        );
        let clean = serde_json::json!({"messages": [
            {"role": "user", "content": "for n in 1113 1114 1115 1116; do gh issue view $n; done"},
        ]});
        assert!(pan_pre_send_block(false, &clean).is_none());
        assert!(pan_pre_send_block(true, &request).is_none());
    }

    #[test]
    fn loopback_base_is_parsed_not_prefix_matched() {
        assert!(is_loopback_base("http://127.0.0.1:1234/v1"));
        assert!(is_loopback_base("http://localhost:11434/v1"));
        assert!(is_loopback_base("http://[::1]:8080/v1"));
        assert!(!is_loopback_base("https://localhost.example.com/v1"));
        assert!(!is_loopback_base("https://openrouter.ai/api/v1"));
    }

    /// A card number sent to a loopback base reaches the local server, but a
    /// redirect from it is not followed, so the body never leaves that server.
    #[test]
    fn loopback_pan_request_is_sent_but_redirect_is_not_followed() {
        use std::io::{Read, Write};
        let target = std::net::TcpListener::bind("127.0.0.1:0").expect("bind target");
        let target_port = target.local_addr().expect("target addr").port();
        let (target_tx, target_rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            for _ in target.incoming() {
                let _ = target_tx.send(());
            }
        });
        let local = std::net::TcpListener::bind("127.0.0.1:0").expect("bind local");
        let local_port = local.local_addr().expect("local addr").port();
        let (local_tx, local_rx) = std::sync::mpsc::channel::<bool>();
        std::thread::spawn(move || {
            for stream in local.incoming() {
                let Ok(mut stream) = stream else { continue };
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
                let mut buf = vec![0u8; 65536];
                let n = stream.read(&mut buf).unwrap_or(0);
                let raw = String::from_utf8_lossy(&buf[..n]).into_owned();
                let _ = local_tx.send(raw.contains("/v1/chat/completions"));
                let response = format!(
                    "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://127.0.0.1:{target_port}/v1/chat/completions\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });

        let pan = "4242".repeat(4);
        let request = serde_json::json!({"model": "m", "stream": true, "messages": [
            {"role": "user", "content": format!("card {pan}")},
        ]});
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let errors = rt.block_on(async {
            let (tx, mut events) = mpsc::channel::<Result<StreamEvent>>(64);
            run_stream_with_retries(
                Client::new(),
                format!("http://127.0.0.1:{local_port}/v1"),
                ProviderAuth::None {
                    label: "test".to_string(),
                },
                false,
                "conv-pan-loopback".to_string(),
                request,
                tx,
                Arc::new(Mutex::new(None)),
                "m".to_string(),
            )
            .await;
            let mut errors = Vec::new();
            while let Some(event) = events.recv().await {
                if let Err(e) = event {
                    errors.push(format!("{e:#}"));
                }
            }
            errors
        });
        assert!(
            local_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("the loopback server must receive the request"),
            "local server saw an unexpected request"
        );
        assert!(
            errors.iter().all(|e| !e.contains("[PAN]")),
            "loopback request was blocked by the pre-send check"
        );
        assert!(
            target_rx
                .recv_timeout(std::time::Duration::from_millis(500))
                .is_err(),
            "the redirect was followed, so the body left the loopback server"
        );
    }

    #[test]
    fn only_one_opts_out_of_the_pan_check() {
        assert!(pan_check_opted_out(Some("1")));
        assert!(!pan_check_opted_out(Some("0")));
        assert!(!pan_check_opted_out(Some("")));
        assert!(!pan_check_opted_out(None));
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
    fn payment_required_is_not_retryable() {
        let err = "openai-compatible chat request failed\n  endpoint: \
            https://openrouter.ai/api/v1/chat/completions\n  model: openai/gpt-5.4\n  \
            auth: openrouter_api_key\n  status: 402 payment required\n  response: \
            {\"error\":{\"message\":\"this request requires more credits, or fewer \
            max_tokens. you requested up to 65536 tokens, but can only afford 34424\"}}";
        assert!(!is_retryable_error(err));
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
}
