use super::*;
use futures::Stream;
use jcode_provider_core::is_provider_overload_message;
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

/// Hint for an error response. Some providers send 429 for an outage of
/// their own upstream ("The model provider is temporarily unavailable"),
/// which is not the caller's rate limit: name the outage instead. A local
/// proxy relaying that outage gets the same hint, since the proxy is up.
fn response_hint(status: u16, body: &str, api_base: &str, model: &str) -> &'static str {
    let lower = body.to_lowercase();
    if status == 429 && is_provider_overload_message(&lower) && !lower.contains("rate limit") {
        return PROVIDER_OUTAGE_HINT;
    }
    http_status_hint(status, api_base, model)
}

const PROVIDER_OUTAGE_HINT: &str = "Hint: the provider is overloaded or having a temporary server problem. jcode retries automatically, or switch to another provider with /model.";

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
            "Hint: this is a billing limit, not a network problem. The key's balance or token allowance is used up: raise the limit or top up in the provider's dashboard, use another key, or switch to another provider with /model. If the key's allowance is resettable, it may recover after its reset time."
        }
        // A 429 means the server answered: the request reached the provider
        // and was rejected for quota, not connectivity. Point at the limit.
        429 => RATE_LIMIT_HINT,
        // Local servers (Ollama, LM Studio) answer 404 for a model that is
        // not installed or loaded, which their own hint already explains.
        404 if !is_local => {
            "Hint: the endpoint or model was not found. Check that the base URL includes the API version (usually /v1) and that the model exists on the provider."
        }
        // The server answered but is overloaded or failing on its side. The
        // request is retried automatically, so point at the provider, not the
        // network.
        500..=599 if !is_local => PROVIDER_OUTAGE_HINT,
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
            &request,
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
    request: &Value,
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
        req.json(request),
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
        // Some providers (e.g. Openference) send the requested delay only in
        // the JSON body (`retry_after_seconds`) with no `Retry-After` header,
        // so the header hint is preferred and the body is the fallback.
        let header_retry_after = jcode_provider_core::retry_after::retry_after(response.headers());
        let body = jcode_base::util::http_error_body(response, "HTTP error").await;
        let retry_after = header_retry_after
            .or_else(|| jcode_provider_core::retry_after::retry_after_body(&body));
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
            response_hint(status.as_u16(), &body, &api_base, &model)
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
            // An error event inside a 200 stream (a proxy relaying the
            // upstream's 503, or a provider's "deploying" notice) ends the
            // turn if forwarded. When it is transient, fail this attempt
            // instead so the retry loop resends it. Azure Responses errors are
            // classified by `azure_response_failure` below instead.
            Ok(Some(Ok(StreamEvent::Error {
                message,
                retry_after_secs,
            }))) if !azure_responses && is_retryable_error(&message.to_lowercase()) => {
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
    jcode_provider_core::parse_http_status_message(error_str)
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
/// Whether a (lowercased) 429 body says the account's allowance is used up,
/// as opposed to asking the client to slow down.
fn is_exhausted_quota_error(error_str: &str) -> bool {
    [
        "freeusagelimiterror",
        "free usage limit",
        "insufficient_quota",
        "insufficient quota",
        "exceeded your current quota",
        "billing",
    ]
    .iter()
    .any(|marker| error_str.contains(marker))
}

fn is_retryable_error(error_str: &str) -> bool {
    // Explicit non-retryable HTTP statuses take precedence over the loose
    // substring heuristics below. These are deterministic client-side failures
    // (auth, billing, malformed request) where retrying is futile and just
    // burns time/credits. 429 (rate limit) is classified explicitly so it does
    // not depend on provider-specific body wording.
    let status = parsed_http_status(error_str);
    let payload = error_str
        .lines()
        .filter(|line| {
            !["endpoint:", "model:", "auth:"]
                .iter()
                .any(|prefix| line.trim_start().starts_with(prefix))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let error_str = payload.as_str();
    match status {
        Some(400 | 401 | 402 | 403 | 404 | 405 | 406 | 422) => return false,
        // 429 rate limit, and every 5xx: the server answered but is overloaded
        // or failing on its side. This includes non-standard codes such as the
        // 529 "overloaded" some OpenAI-compatible providers send (#1596).
        // A 429 that reports an exhausted allowance is not a rate limit: the
        // same request fails identically until the plan changes. Retrying it
        // left `jcode run` silent for minutes on a free-tier limit.
        Some(429) if is_exhausted_quota_error(error_str) => return false,
        Some(429 | 500..=599) => return true,
        _ => {}
    }

    if jcode_provider_core::is_definitive_usage_exhaustion(error_str)
        || [
            "invalid_request_error",
            "authentication_error",
            "permission_error",
            "invalid_api_key",
            "model_not_found",
            "content_filter",
        ]
        .iter()
        .any(|marker| error_str.contains(marker))
    {
        return false;
    }
    jcode_provider_core::is_transient_transport_error(error_str)
        || error_str.contains("rate limit")
        || error_str.contains("rate_limit")
        || error_str.contains("stream error")
        || error_str.contains("eof")
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

#[cfg(test)]
#[path = "compatible_retry_tests.rs"]
mod tests;
