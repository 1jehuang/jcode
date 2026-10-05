use super::*;
use jcode_provider_openrouter::stream::OpenRouterStream;

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

/// Hint for an error response. Some providers send 429 for an outage of
/// their own upstream ("The model provider is temporarily unavailable"),
/// which is not the caller's rate limit: name the outage instead.
fn response_hint(status: u16, body: &str, api_base: &str, model: &str) -> &'static str {
    if status == 429
        && is_provider_overload_message(&body.to_lowercase())
        && !body.to_lowercase().contains("rate limit")
    {
        return http_status_hint(503, api_base, model);
    }
    http_status_hint(status, api_base, model)
}

/// Hint for a request the server answered with an error status. The network
/// is working (a response came back), so connectivity advice would mislead:
/// point at the rate limit, the key, the account balance, or the model.
fn http_status_hint(status: u16, api_base: &str, model: &str) -> &'static str {
    let endpoint_hint = local_endpoint_troubleshooting_hint(api_base, model);
    let is_local = !endpoint_hint.starts_with("Hint: check network");
    match status {
        // A 429 means the server answered: the request reached the provider
        // and was rejected for quota, not connectivity. Point at the limit.
        429 => RATE_LIMIT_HINT,
        401 | 403 => {
            "Hint: the provider rejected the API key. Check that the key is valid and allowed to use this model, or switch to another provider with /model."
        }
        402 => {
            "Hint: this is a billing limit, not a network problem. The key's balance or token allowance is used up: raise the limit or top up in the provider's dashboard, use another key, or switch to another provider with /model."
        }
        // Local servers (Ollama, LM Studio) answer 404 for a model that is
        // not installed or loaded, which their own hint already explains.
        404 if !is_local => {
            "Hint: the endpoint or model was not found. Check that the base URL includes the API version (usually /v1) and that the model exists on the provider."
        }
        // The provider answered, so this is server-side overload or failure,
        // not connectivity (#1596). These are retried before surfacing.
        500..=599 if !is_local => {
            "Hint: the provider is overloaded or having a temporary server problem, not a network problem. jcode already retried; try again shortly or switch to another provider with /model."
        }
        _ => endpoint_hint,
    }
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
    let mut last_error = None;
    let mut next_retry_delay = None;
    let config = jcode_base::config::config();
    let max_retries = config.provider.max_retries.max(1);
    let retry_backoff_cap =
        std::time::Duration::from_secs(config.provider.retry_backoff_cap_secs.max(1));

    for attempt in 0..max_retries {
        // The consumer drops its receiver to cancel the turn (e.g. on Esc or
        // /model switch). Retrying against a closed channel would burn the
        // remaining attempts, backoff waits included, for output nobody
        // will ever read, so stop as soon as cancellation is observed.
        if tx.is_closed() {
            return;
        }
        if attempt > 0 {
            let delay = retry_wait(attempt, next_retry_delay.take(), retry_backoff_cap);
            // Also wake mid-wait: a cancelled turn must not sit out a long
            // rate-limit or exponential backoff before noticing.
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = tx.closed() => return,
            }
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
        let attempt_client = if attempt == 0 {
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
                if is_retryable_error(&error_str) && attempt + 1 < max_retries {
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
        let hint = response_hint(status.as_u16(), &body, &api_base, &model);
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
            // An error event inside a 200 stream (a proxy relaying the
            // upstream's 503, or a provider's "deploying" notice) ends the
            // turn if forwarded. When it is transient, fail this attempt
            // instead so the retry loop resends it.
            Ok(Some(Ok(StreamEvent::Error {
                message,
                retry_after_secs,
            }))) if is_retryable_error(&message.to_lowercase()) => {
                let wait = retry_after_secs
                    .map(std::time::Duration::from_secs)
                    .or_else(|| {
                        is_deploy_message(&message.to_lowercase()).then_some(deploy_retry_wait())
                    });
                return Err(jcode_provider_core::retry_after::error_with_retry_after(
                    format!(
                        "OpenAI-compatible stream error\n  endpoint: {}\n  model: {}\n  auth: {}\n  error: {}",
                        url,
                        model,
                        auth.label(),
                        message
                    ),
                    wait.map(jcode_provider_core::retry_after::RetryAfter::after),
                ));
            }
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
    // `status: 503 ...` from this module, or `HTTP 503: ...` when a proxy
    // relays the upstream failure inside the stream.
    let rest = if let Some(idx) = lower.find("status:") {
        lower[idx + "status:".len()..].trim_start()
    } else {
        let idx = lower.find("http ")?;
        &lower[idx + "http ".len()..]
    };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.len() == 3 {
        digits.parse().ok()
    } else {
        None
    }
}

/// Wait before retry `attempt`. A provider-requested delay (Retry-After) is
/// honored as sent, up to [`jcode_provider_core::retry_after::MAX_RETRY_AFTER`]
/// (already applied when it was parsed): resending sooner only draws another
/// rate limit. The configured backoff cap limits jcode's own exponential
/// backoff, not the provider's request.
fn retry_wait(
    attempt: u32,
    server_hint: Option<std::time::Duration>,
    backoff_cap: std::time::Duration,
) -> std::time::Duration {
    match server_hint {
        Some(hint) => hint.min(jcode_provider_core::retry_after::MAX_RETRY_AFTER),
        None => jcode_provider_core::retry_after::retry_delay(attempt, RETRY_BASE_DELAY_MS, None)
            .min(backoff_cap),
    }
}

/// Hint for a 429. Kept next to [`retry_wait`] so the promise matches it.
const RATE_LIMIT_HINT: &str = "Hint: the provider rate limited this request (per-minute or quota cap), not a network problem. jcode waits as long as the provider asks (up to 60s per retry) and backs off within its retry budget; if it keeps failing, wait a minute, lower request frequency, or switch to another provider with /model.";

fn is_retryable_error(error_str: &str) -> bool {
    // Explicit non-retryable HTTP statuses take precedence over the loose
    // substring heuristics below. These are deterministic client-side failures
    // (auth, billing, malformed request) where retrying is futile and just
    // burns time/credits. 429 (rate limit) is classified explicitly so it does
    // not depend on provider-specific body wording.
    match parsed_http_status(error_str) {
        Some(400 | 401 | 402 | 403 | 404 | 405 | 406 | 422) => return false,
        // 429 rate limit, and every 5xx: the server answered but is overloaded
        // or failing on its side. This includes non-standard codes such as the
        // 529 "overloaded" some OpenAI-compatible providers send (#1596).
        Some(429 | 500..=599) => return true,
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
        || is_provider_overload_message(error_str)
        || is_deploy_message(error_str)
        || is_interrupted_stream_message(error_str)
}

/// Wait between retries while a provider says it is deploying. Deploys take
/// minutes, so the default exponential backoff would spend the whole retry
/// budget in under a minute. With 8 attempts this covers about 3.5 minutes.
const DEPLOY_RETRY_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

#[cfg(not(test))]
fn deploy_retry_wait() -> std::time::Duration {
    DEPLOY_RETRY_WAIT
}

/// Tests exercise the real retry loop against a local server; a 30s wait
/// would only slow them down.
#[cfg(test)]
fn deploy_retry_wait() -> std::time::Duration {
    std::time::Duration::from_millis(50)
}

/// Wording for a provider that is mid-deploy and asks to retry shortly
/// ("We're deploying an update right now ... Please try again shortly.",
/// `"code":"DEPLOYING"`). `error_str` is already lowercased by the caller.
fn is_deploy_message(error_str: &str) -> bool {
    [
        "deploying",
        "try again shortly",
        "under maintenance",
        "maintenance mode",
    ]
    .iter()
    .any(|marker| error_str.contains(marker))
}

/// Wording for a provider that dropped its own upstream mid-response and
/// reports it as an error event inside the 200 stream (Openference: "The
/// model provider's stream was interrupted. Please retry."). The request was
/// fine, so resending it works. `error_str` is already lowercased.
fn is_interrupted_stream_message(error_str: &str) -> bool {
    [
        "stream was interrupted",
        "stream interrupted",
        "response was interrupted",
        "connection was interrupted",
    ]
    .iter()
    .any(|marker| error_str.contains(marker))
}

/// Wording providers use for a temporary capacity problem, sometimes inside
/// a 200 SSE stream rather than as an HTTP status (e.g. Openference's
/// "We're experiencing heavy usage right now ... please try again in a
/// moment"). `error_str` is already lowercased by the caller.
fn is_provider_overload_message(error_str: &str) -> bool {
    [
        "heavy usage",
        "temporarily unavailable",
        "temporary unavailability",
        "try again in a moment",
        "server is busy",
        "at capacity",
        "capacity constraints",
    ]
    .iter()
    .any(|marker| error_str.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
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

    /// A 429 with a `Retry-After` header must be classified as retryable,
    /// carry the exact 2s to the retry loop's delay selection instead of
    /// falling back to jittered exponential backoff, and the hint must point
    /// at the rate limit, not the network.
    #[test]
    fn rate_limit_429_delay_flows_to_retry_loop() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        rt.block_on(async {
            let body = r#"{"error":"Rate limit exceeded. Too many requests per minute.","type":"rate_limit_error","code":"rate_limit_exceeded","retry_after_seconds":2,"max_rpm":25}"#;
            let response = format!(
                "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 2\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
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
                serde_json::json!({"model": "glm-5.3", "messages": [], "stream": true}),
                tx,
                Arc::new(Mutex::new(None)),
                "glm-5.3".to_string(),
            )
            .await
            .expect_err("429 must surface as an error");

            let error_str = format!("{err:#}");
            assert!(error_str.contains("status: 429"), "{error_str}");
            // The retry loop classifies this exact formatted error before
            // deciding.
            assert!(
                is_retryable_error(&error_str.to_lowercase()),
                "429 must be retried"
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

    /// Dropping the consumer's receiver cancels the turn: the retry loop must
    /// stop without burning the remaining attempts or sitting out the backoff
    /// wait against a server that keeps answering retryable 500s.
    #[test]
    fn cancelled_stream_stops_retrying_and_wakes_from_backoff() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        rt.block_on(async {
            let connections = Arc::new(AtomicUsize::new(0));
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind failing server");
            let addr = listener.local_addr().expect("failing server addr");
            let counted = Arc::clone(&connections);
            std::thread::spawn(move || {
                for _ in 0..64 {
                    let Ok((mut stream, _)) = listener.accept() else {
                        return;
                    };
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .expect("set read timeout");
                    let mut request = vec![0u8; 65536];
                    let _ = stream.read(&mut request);
                    let _ = stream.write_all(
                        b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                    counted.fetch_add(1, Ordering::SeqCst);
                }
            });
            let api_base = format!("http://{addr}/v1");

            let (tx, mut rx) = mpsc::channel(16);
            // Consume the first (pre-request) event, then drop the receiver
            // to cancel the turn while the first attempt is still in flight.
            tokio::spawn(async move {
                let _ = rx.recv().await;
            });

            let start = std::time::Instant::now();
            run_stream_with_retries(
                Client::new(),
                api_base,
                ProviderAuth::None {
                    label: "test".to_string(),
                },
                false,
                "conv".to_string(),
                serde_json::json!({"model": "glm-5.3", "messages": [], "stream": true}),
                tx,
                Arc::new(Mutex::new(None)),
                "glm-5.3".to_string(),
            )
            .await;

            assert!(
                start.elapsed() < Duration::from_secs(5),
                "cancelled turn must not sit out the retry budget"
            );
            assert!(
                connections.load(Ordering::SeqCst) <= 1,
                "cancelled turn must not retry the request"
            );
        });
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
    fn run_with_failing_first_attempt(first_body: String) -> RetryRun {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        rt.block_on(async {
            let connections = Arc::new(AtomicUsize::new(0));
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
            let addr = listener.local_addr().expect("test server addr");
            let counted = Arc::clone(&connections);
            std::thread::spawn(move || {
                for attempt in 0..2 {
                    let Ok((mut stream, _)) = listener.accept() else {
                        return;
                    };
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .expect("set read timeout");
                    let mut request = vec![0u8; 65536];
                    let _ = stream.read(&mut request);
                    let body = if attempt == 0 {
                        first_body.clone()
                    } else {
                        HEALTHY_ANSWER_SSE.to_string()
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(response.as_bytes());
                    counted.fetch_add(1, Ordering::SeqCst);
                }
            });

            let (tx, mut rx) = mpsc::channel(64);
            let api_base = format!("http://{addr}/v1");
            let run = tokio::spawn(async move {
                run_stream_with_retries(
                    Client::new(),
                    api_base,
                    ProviderAuth::None {
                        label: "test".to_string(),
                    },
                    false,
                    "conv".to_string(),
                    serde_json::json!({"model": "ntl/claude", "messages": [], "stream": true}),
                    tx,
                    Arc::new(Mutex::new(None)),
                    "ntl/claude".to_string(),
                )
                .await;
            });

            let mut result = RetryRun {
                text: String::new(),
                errors: Vec::new(),
                rollbacks: 0,
                connections: 0,
            };
            while let Some(item) = rx.recv().await {
                match item {
                    Ok(StreamEvent::TextDelta(delta)) => result.text.push_str(&delta),
                    Ok(StreamEvent::RetryRollback { .. }) => {
                        result.rollbacks += 1;
                        result.text.clear();
                    }
                    Ok(StreamEvent::Error { message, .. }) => result.errors.push(message),
                    Err(error) => result.errors.push(format!("{error:#}")),
                    Ok(_) => {}
                }
            }
            run.await.expect("retry loop task");
            result.connections = connections.load(Ordering::SeqCst);
            result
        })
    }

    /// End to end: the first attempt gets a 200 stream whose only event is
    /// the relayed deploy 503, the second gets a normal answer. The turn must
    /// see the answer, not the error.
    #[test]
    fn in_stream_deploying_503_is_retried_until_the_provider_answers() {
        let run = run_with_failing_first_attempt(format!(
            "data: {}\n\ndata: [DONE]\n\n",
            serde_json::json!({"error": {"message": PROXY_DEPLOYING_503}})
        ));
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
        let run = run_with_failing_first_attempt(format!(
            "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            serde_json::json!({"choices": [{"delta": {"content": "half an ans"}}]}),
            serde_json::json!({"error": {"message": INTERRUPTED_STREAM}})
        ));
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
        // Local servers keep their own advice for a 5xx.
        assert!(
            http_status_hint(503, "http://localhost:11434/v1", "llama3.2").contains("ollama serve")
        );
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
        // A real rate limit keeps the rate-limit hint.
        let limited = r#"{"error":"Rate limit exceeded. Too many requests per minute."}"#;
        assert_eq!(response_hint(429, limited, api, "glm-5.3"), RATE_LIMIT_HINT);
        assert_eq!(response_hint(429, "", api, "glm-5.3"), RATE_LIMIT_HINT);
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
        assert!(
            http_status_hint(404, "http://localhost:11434/v1", "llama3.2").contains("ollama pull")
        );
        // Server errors keep the endpoint-specific advice.
        assert!(
            http_status_hint(503, "http://localhost:11434/v1", "llama3.2").contains("ollama serve")
        );
    }
}
