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

/// Body markers for a guardrail block. OpenRouter answers a blocked prompt with
/// `{"error":{"message":"Request blocked by content filter: [CREDIT_CARD]"}}`,
/// and OpenAI-style endpoints report the `content_filter` code.
fn names_content_filter(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("blocked by content filter") || lower.contains("content_filter")
}

/// A blocked prompt is neither a credential problem nor transient: the provider
/// read the text and refused it, so a retry, and any failover lane, resends the
/// blocked text. Say that instead of sending the user to check a key that was
/// never the problem (FU-149).
const CONTENT_FILTER_HINT: &str = "Hint: the provider's content filter blocked this request text, so the API key is not the problem. Retrying, or failing over to another provider, resends the same text and is likely to be blocked again. Remove or rewrite the flagged content and send the request again.";

/// Hint for a request the server answered with an error status. The network
/// is working (a response came back), so connectivity advice would mislead:
/// point at the key, the account balance, or the model instead.
fn http_status_hint(status: u16, api_base: &str, model: &str) -> &'static str {
    let endpoint_hint = local_endpoint_troubleshooting_hint(api_base, model);
    let is_local = !endpoint_hint.starts_with("Hint: check network");
    match status {
        // A local server is normally handed no hosted key at all (the error
        // block prints `auth: local endpoint (no auth)`), so blaming the API
        // key contradicts the evidence directly above the hint.
        401 | 403 if is_local => {
            "Hint: the local server refused the request, so the API key of a hosted provider is not the problem. Check the server's own auth and route settings, the base URL path, and - if this server proxies a hosted provider - that provider's key."
        }
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
        let hint = if names_content_filter(&body) {
            CONTENT_FILTER_HINT
        } else {
            http_status_hint(status.as_u16(), &api_base, &model)
        };
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

    /// FU-149: the measured OpenRouter guardrail body must never be reported as
    /// a rejected API key. Before the fix the hint read "the provider rejected
    /// the API key" two lines below a body naming a content filter.
    #[test]
    fn content_filter_body_is_not_reported_as_a_bad_api_key() {
        let body = r#"{"error": {"message": "Request blocked by content filter: [CREDIT_CARD]", "code": 403}}"#;
        assert!(names_content_filter(body));
        // OpenAI-style code instead of prose.
        assert!(names_content_filter(
            r#"{"error":{"code":"content_filter"}}"#
        ));
        assert!(!names_content_filter(
            r#"{"error":{"message":"Forbidden"}}"#
        ));

        let hint = if names_content_filter(body) {
            CONTENT_FILTER_HINT
        } else {
            http_status_hint(
                403,
                "https://openrouter.ai/api/v1",
                "deepseek/deepseek-v4-flash",
            )
        };
        assert_eq!(hint, CONTENT_FILTER_HINT);
        assert!(hint.contains("content filter"), "{hint}");
        assert!(!hint.contains("rejected the API key"), "{hint}");
        // The actionable instruction is to change the text, not the lane.
        assert!(hint.contains("Remove or rewrite"), "{hint}");
    }

    /// A 403 that names no filter still points at the key on a hosted endpoint.
    #[test]
    fn plain_403_still_blames_a_hosted_api_key() {
        assert!(!names_content_filter(
            r#"{"error":{"message":"Forbidden"}}"#
        ));
        let hint = http_status_hint(403, "https://api.example.com/v1", "m");
        assert!(hint.contains("rejected the API key"), "{hint}");
    }

    /// FU-149 also measured a local stub answering 403 while the error block
    /// printed `auth: local endpoint (no auth)`; the hint must not contradict
    /// that line.
    #[test]
    fn local_403_does_not_blame_a_hosted_api_key() {
        let hint = http_status_hint(403, "http://127.0.0.1:18765/v1", "m");
        assert!(hint.contains("local server refused"), "{hint}");
        assert!(!hint.contains("rejected the API key"), "{hint}");
        // A refused local request gets this advice for every local endpoint: the
        // Ollama/LM Studio "server is not running" hints would not explain a 403.
        let ollama = http_status_hint(403, "http://localhost:11434/v1", "llama3.2");
        assert!(ollama.contains("local server refused"), "{ollama}");
    }
}
