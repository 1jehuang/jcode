use super::*;
use futures::Stream;
use jcode_provider_openrouter::stream::OpenRouterStream;
use std::pin::Pin;

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

/// Hint for a request the server answered with an error status. The network
/// is working (a response came back), so connectivity advice would mislead:
/// point at the key, the account balance, or the model instead.
fn http_status_hint(status: u16, api_base: &str, model: &str) -> &'static str {
    let endpoint_hint = local_endpoint_troubleshooting_hint(api_base, model);
    let is_local = !endpoint_hint.starts_with("Hint: check network");
    match status {
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
    azure_responses: bool,
    tx: mpsc::Sender<Result<StreamEvent>>,
    provider_pin: Arc<Mutex<Option<ProviderPin>>>,
    model: String,
    responses_api: bool,
    capability_scope_key: String,
) {
    let mut request = request;
    // One modality retry: when the endpoint answers "this model cannot accept
    // image input" for a request that carried `image_url` parts, rewrite the
    // request with the images replaced by text markers and replay it once on
    // the SAME profile. This is the streaming twin of the failover-layer retry
    // in `MultiProvider::complete_with_failover`: HTTP errors surface here,
    // inside the async stream, because the stream handle is handed back before
    // the request is even sent.
    let mut image_modality_retry_available =
        jcode_provider_core::image_capability::request_contains_image_url_parts(&request);
    // Set by the inner replay loop when an attempt fails for a non-modality
    // (or after-partial-output) reason; consumed by the outer loop's
    // transient-vs-deterministic classification. Deferred-init: the only
    // read (`.take()` below) is always preceded by the assignment in the
    // same outer-loop iteration.
    let mut last_attempt_failure: Option<(anyhow::Error, bool, String)>;
    let mut last_error = None;
    let mut next_retry_delay = None;
    let config = jcode_base::config::config();
    let max_retries = config.provider.max_retries.max(1);
    let retry_backoff_cap =
        std::time::Duration::from_secs(config.provider.retry_backoff_cap_secs.max(1));

    // Outer loop: one iteration per user-visible retry attempt (config
    // max_retries). Replays happen inside the same `attempt` so the modality
    // retry never consumes one of the user's `max_retries` (with
    // `max_retries = 1` a `continue` would exit the loop and end the
    // stream silently).
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

        // Modality replay is an inner loop: a request that carried image
        // parts may be rejected for the image modality and replayed once,
        // rewritten with text markers, on the same attempt slot. Doing it
        // with `continue` on the outer loop would silently swallow the
        // failure when `max_retries == 1`: the loop would exit with
        // `last_error` unset and the consumer would see an empty stream.
        loop {
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
                azure_responses,
                attempt_tx,
                Arc::clone(&provider_pin),
                model.clone(),
                responses_api,
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
                    // Image-modality rejection: the endpoint is healthy, only the
                    // `image_url` parts are unsupported by this model. Checked
                    // BEFORE the transient-error classifier, which correctly
                    // classifies the HTTP 400 as a deterministic client error:
                    // here that is true for the pixel data, not for the request
                    // shape. Replay the same request once with images replaced
                    // by text markers. Errors after partial output skip this path
                    // on purpose: the consumer already rendered the tokens, a
                    // silent rewrite would duplicate them.
                    if image_modality_retry_available
                        && !saw_output
                        && jcode_provider_core::image_capability::error_indicates_image_rejection(
                            &error_str,
                        )
                    {
                        image_modality_retry_available = false;
                        // Record under the exact (capability-scope, model)
                        // pair that `MultiProvider::supports_image_input` and
                        // this runtime's own `supports_image_input` look up, so
                        // the NEXT request filters images at build time instead
                        // of paying another rejected call. The scope is the
                        // profile id or endpoint base, so a rejection on one
                        // OpenAI-compatible profile never suppresses images for
                        // another profile serving the same model name. The model
                        // here is the same raw string `Provider::model()`
                        // returns (the runtime's model id).
                        jcode_provider_core::image_capability::record_image_input_rejection(
                            Some(&capability_scope_key),
                            &model,
                            &error_str,
                        );
                        jcode_base::logging::warn(&format!(
                            "Endpoint rejected image input for model {model}; \
                             retrying once with images replaced by text markers"
                        ));
                        request = jcode_provider_core::image_capability::filter_image_url_parts_from_request(
                            &request,
                        );
                        // Replay inside the same attempt slot: no sleep (the
                        // failure is deterministic, not transient), no extra
                        // attempt consumed.
                        continue;
                    }
                    last_attempt_failure = Some((e, saw_output, error_str));
                    break;
                }
            }
        }

        // The inner loop broke: either the failure was not a modality
        // rejection, or it came after partial output. Classify transient
        // vs deterministic here, on the last error observed.
        if let Some((e, saw_output, _)) = last_attempt_failure.take() {
            if retryable_stream_failure(&e) && attempt + 1 < max_retries {
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
                    jcode_base::logging::info(&format!("Transient API error, will retry: {}", e));
                }
                next_retry_delay = jcode_provider_core::retry_after::retry_after_from_error(&e);
                last_error = Some(e);
                continue;
            }

            let _ = tx.send(Err(e)).await;
            return;
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
    azure_responses: bool,
    tx: mpsc::Sender<Result<StreamEvent>>,
    provider_pin: Arc<Mutex<Option<ProviderPin>>>,
    model: String,
    responses_api: bool,
) -> Result<()> {
    use jcode_message_types::ConnectionPhase;
    let _ = tx
        .send(Ok(StreamEvent::ConnectionPhase {
            phase: ConnectionPhase::SendingRequest,
        }))
        .await;
    let connect_start = std::time::Instant::now();
    let stream_idle_timeout = jcode_base::provider::stream_idle_timeout();

    let route = if azure_responses || responses_api {
        "responses"
    } else {
        "chat/completions"
    };
    let request_kind = if azure_responses {
        "Azure OpenAI Responses"
    } else {
        "OpenAI-compatible chat"
    };
    let url = format!("{}/{}", api_base, route);
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
        let hint = if azure_responses {
            "Hint: check the Azure deployment and that the endpoint supports /openai/v1/responses."
        } else {
            local_endpoint_troubleshooting_hint(&api_base, &model)
        };
        format!(
            "Failed to send {request_kind} request\n  endpoint: {}\n  model: {}\n  auth: {}\n{}",
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
        let hint = if azure_responses {
            match status.as_u16() {
                401 | 402 | 403 | 500..=599 => http_status_hint(status.as_u16(), &api_base, &model),
                429 => {
                    "Hint: Azure rate limited the request; jcode will retry before returning this error."
                }
                _ => {
                    "Hint: check the Azure deployment and that the endpoint supports /openai/v1/responses."
                }
            }
        } else {
            http_status_hint(status.as_u16(), &api_base, &model)
        };
        return Err(jcode_provider_core::retry_after::error_with_retry_after(
            format!(
                "{request_kind} request failed\n  endpoint: {}\n  model: {}\n  auth: {}\n  status: {}\n  response: {}\n{}",
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

    let mut stream: Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>> = if azure_responses {
        Box::pin(jcode_provider_openai::stream::OpenAIResponsesStream::new(
            response.bytes_stream(),
        ))
    } else if responses_api {
        Box::pin(super::opencode_go_responses::ResponsesStream::new(
            response.bytes_stream(),
        ))
    } else {
        Box::pin(OpenRouterStream::new(
            response.bytes_stream(),
            model.clone(),
            provider_pin,
        ))
    };

    // Idle timeout between streamed chunks. Configurable so slow reasoning
    // models (e.g. DeepSeek) that think silently for minutes before emitting
    // tokens don't trip a premature timeout (issue #196). Resolved from
    // `[provider] stream_idle_timeout_secs` / `JCODE_STREAM_IDLE_TIMEOUT_SECS`,
    // defaulting to 180s. Shared with the native provider paths (issue #434).
    let idle_timeout_secs = stream_idle_timeout.as_secs();

    let mut saw_message_end = false;
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
        if azure_responses {
            if let StreamEvent::Error {
                message,
                retry_after_secs,
            } = &event
            {
                return Err(azure_response_failure(message, *retry_after_secs));
            }
            if matches!(event, StreamEvent::MessageEnd { .. }) {
                saw_message_end = true;
            }
        } else if let StreamEvent::Error { message, .. } = &event {
            // A 200-status SSE `error` event saying the model rejects images
            // must reach the attempt loop's modality recovery, not be
            // forwarded and swallowed: forwarding made stream_response return
            // Ok(()) with the rejection neither recorded nor retried, so the
            // turn failed and every later turn repeated the same rejected
            // request. Return it as an error so the outer loop classifies it
            // (image rejection, no output yet) and replays once with images
            // filtered. Any other in-stream error stays forwarded: the
            // consumer surfaces it after the stream closes, as before.
            if jcode_provider_core::image_capability::error_indicates_image_rejection(message) {
                return Err(anyhow::anyhow!(
                    "OpenAI-compatible stream reported an image-modality rejection
  endpoint: {}
  model: {}
  auth: {}
  error: {}",
                    url,
                    model,
                    auth.label(),
                    message
                ));
            }
        }
        if tx.send(Ok(event)).await.is_err() {
            return Ok(());
        }
        if saw_message_end {
            break;
        }
    }

    if azure_responses && !saw_message_end {
        return Err(anyhow::Error::new(AzureResponseFailure {
            retryable: true,
            source: anyhow::anyhow!(
                "Azure OpenAI Responses stream ended before response.completed"
            ),
        }));
    }

    Ok(())
}

/// An Azure protocol failure is not necessarily a transient transport failure.
/// Keep its retry decision separate from the legacy string classifier, and
/// retain the shared retry-hint error as a source for deadline extraction.
#[derive(Debug)]
struct AzureResponseFailure {
    retryable: bool,
    source: anyhow::Error,
}

impl std::fmt::Display for AzureResponseFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.source, formatter)
    }
}

impl std::error::Error for AzureResponseFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

fn retryable_stream_failure(error: &anyhow::Error) -> bool {
    if let Some(failure) = error.downcast_ref::<AzureResponseFailure>() {
        return failure.retryable;
    }
    // Preserve the existing classifier and full transport context for every
    // non-Azure failure, including HTTP errors and reqwest body errors.
    is_retryable_error(&format!("{error:#}").to_lowercase())
}

fn azure_response_failure(message: &str, retry_after_secs: Option<u64>) -> anyhow::Error {
    let lower = message.to_ascii_lowercase();
    // The shared Responses parser preserves error type/code in the message.
    // Permanent failures must win even if their prose mentions a timeout or
    // overload, or the server supplies a retry hint.
    let permanent = [
        "invalid_request",
        "invalid_argument",
        "content_filter",
        "content_policy",
        "authentication",
        "permission_denied",
        "insufficient_quota",
        "billing",
        "unauthorized",
        "forbidden",
        "model_not_found",
        "deployment_not_found",
        "tool argument snapshot differs",
    ]
    .iter()
    .any(|marker| lower.contains(marker));
    let transient = [
        "rate_limit",
        "rate limit",
        "server_error",
        "internal_error",
        "internal server error",
        "service_unavailable",
        "overloaded",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
        || jcode_provider_core::is_transient_transport_error(&lower);
    let retry_after = retry_after_secs.and_then(|seconds| {
        jcode_provider_core::retry_after::retry_after_body(
            &serde_json::json!({"retry_after_seconds": seconds}).to_string(),
        )
    });
    anyhow::Error::new(AzureResponseFailure {
        retryable: !permanent && (transient || retry_after.is_some()),
        source: jcode_provider_core::retry_after::error_with_retry_after(
            format!("Azure OpenAI Responses stream error: {message}"),
            retry_after,
        ),
    })
}

#[cfg(test)]
#[path = "azure_retry_tests.rs"]
mod azure_retry_tests;

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
}
