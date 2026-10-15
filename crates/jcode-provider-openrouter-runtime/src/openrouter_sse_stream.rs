use super::*;
use jcode_provider_openrouter::stream::OpenRouterStream;

/// Models that require the OpenAI Responses API endpoint (`/responses`)
/// instead of chat completions when served through OpenCode Go.
/// See: <https://opencode.ai/docs/go/#endpoints>
const OPENCODE_GO_RESPONSES_MODELS: &[&str] = &[
    "grok-4.6",
    "gpt-5.6-luna",
    "muse-spark-1.3-contributor",
    "muse-spark-1.2-contributor",
];

/// Check if this model+endpoint combination should use the Responses API.
fn needs_responses_api(model: &str, api_base: &str) -> bool {
    if !is_opencode_api_base(api_base) {
        return false;
    }
    OPENCODE_GO_RESPONSES_MODELS.contains(&model)
}

/// Convert a chat-completions request body to Responses API format.
/// The Responses API accepts simple `{role, content}` messages directly as `input`,
/// so for plain messages we pass them through unchanged. Only tool definitions
/// and parameter names need conversion.
/// See: https://platform.openai.com/docs/guides/responses-vs-chat-completions
fn convert_to_responses_format(mut request: Value) -> Value {
    if let Some(messages) = request.get("messages").cloned() {
        // Pass messages through as-is: the Responses API accepts
        // `{role, content}` format directly for input items.
        request["input"] = messages;
        if let Some(obj) = request.as_object_mut() {
            obj.remove("messages");
        }
    }

    // Convert tools from chat-completions format to Responses API format
    if let Some(tools) = request.get("tools").cloned()
        && let Some(tools_arr) = tools.as_array()
    {
        let converted: Vec<Value> = tools_arr
            .iter()
            .filter_map(|tool| {
                let tool_type = tool.get("type").and_then(|t| t.as_str())?;
                if tool_type == "function" {
                    let func = tool.get("function")?;
                    Some(serde_json::json!({
                        "type": "function",
                        "name": func.get("name").and_then(|n| n.as_str()).unwrap_or(""),
                        "description": func.get("description").and_then(|d| d.as_str()).unwrap_or(""),
                        "parameters": func.get("parameters").cloned().unwrap_or(Value::Object(Default::default())),
                        "strict": func.get("strict").cloned().unwrap_or(Value::Bool(false)),
                    }))
                } else {
                    Some(tool.clone())
                }
            })
            .collect();
        request["tools"] = Value::Array(converted);
    }

    // Map chat-completions `max_tokens` to Responses API `max_output_tokens`.
    // Preserve the caller's configured value instead of hardcoding a default.
    if let Some(obj) = request.as_object_mut() {
        let max_tokens_value = obj
            .remove("max_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(16384);
        obj.remove("stream_options");
        obj.insert(
            "max_output_tokens".to_string(),
            Value::Number(serde_json::Number::from(max_tokens_value)),
        );
    }

    request
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

    // OpenCode Go routes certain models to the Responses API instead of chat completions.
    let use_responses_api = needs_responses_api(&model, &api_base);
    let (url, request) = if use_responses_api {
        let converted = convert_to_responses_format(request);
        (format!("{}/responses", api_base), converted)
    } else {
        (format!("{}/chat/completions", api_base), request)
    };
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

    // Responses API models use a different SSE format than chat completions.
    if use_responses_api {
        return stream_responses_api_response(response, tx, &model).await;
    }

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

/// Parse and stream Responses API SSE events, converting them to StreamEvents.
/// The Responses API uses `event: <type>\ndata: <json>` format instead of
/// chat-completions' `data: <json>` format.
async fn stream_responses_api_response(
    response: reqwest::Response,
    tx: mpsc::Sender<Result<StreamEvent>>,
    _model: &str,
) -> Result<()> {
    use futures::StreamExt;
    let mut bytes_stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut _current_event_type = String::new();
    let mut in_thinking = false;
    let stream_idle_timeout = jcode_base::provider::stream_idle_timeout();

    loop {
        let chunk_result = match tokio::time::timeout(stream_idle_timeout, bytes_stream.next())
            .await
        {
            Ok(Some(result)) => result,
            Ok(None) => break, // stream ended
            Err(_) => {
                jcode_base::logging::warn(&format!(
                    "Responses API stream timed out (no data for {}s)\n  model: {}",
                    stream_idle_timeout.as_secs(),
                    _model
                ));
                anyhow::bail!(
                    "Responses API stream timeout\n  model: {}\n  timeout: no data received for {} seconds",
                    _model,
                    stream_idle_timeout.as_secs()
                );
            }
        };
        let chunk = chunk_result.map_err(|e| anyhow::anyhow!("Response stream error: {}", e))?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        // Process complete SSE events (terminated by \n\n or \r\n\r\n)
        while let Some(newline_pos) = buffer.find("\n\n").or_else(|| buffer.find("\r\n\r\n")) {
            let sep_len = if buffer[newline_pos..].starts_with("\r\n\r\n") {
                4
            } else {
                2
            };
            let event_block = buffer[..newline_pos].to_string();
            buffer = buffer[newline_pos + sep_len..].to_string();

            // Parse event type and data from the block
            let mut data = String::new();
            let mut event_type = String::new();

            for line in event_block.lines() {
                // Handle both "event: x" and "event:x" (no space after colon)
                if let Some(etype) = line
                    .strip_prefix("event: ")
                    .or_else(|| line.strip_prefix("event:"))
                {
                    event_type = etype.trim().to_string();
                } else if let Some(d) = line
                    .strip_prefix("data: ")
                    .or_else(|| line.strip_prefix("data:"))
                {
                    data = d.to_string();
                }
                // Ignore comment lines (e.g. ": ping") and other fields
            }

            // Skip keepalive/empty blocks that have no data payload
            if data.is_empty() {
                continue;
            }

            if data == "[DONE]" {
                // End-of-stream sentinel: close thinking if active, then finish
                if in_thinking {
                    let _ = tx.send(Ok(StreamEvent::ThinkingEnd)).await;
                }
                let _ = tx
                    .send(Ok(StreamEvent::MessageEnd { stop_reason: None }))
                    .await;
                return Ok(());
            }

            // Parse the JSON and convert to StreamEvent
            let json = match serde_json::from_str::<Value>(&data) {
                Ok(v) => v,
                Err(e) => {
                    jcode_base::logging::warn(&format!(
                        "Responses API: malformed JSON in event '{}': {} -- data: {}",
                        event_type,
                        e,
                        &data[..data.len().min(200)]
                    ));
                    continue; // skip malformed blocks instead of ending the stream
                }
            };

            match event_type.as_str() {
                "response.output_text.delta" => {
                    // If we were in thinking, close it first
                    if in_thinking {
                        let _ = tx.send(Ok(StreamEvent::ThinkingEnd)).await;
                        in_thinking = false;
                    }
                    if let Some(delta) = json.get("delta").and_then(|d| d.as_str()) {
                        let _ = tx.send(Ok(StreamEvent::TextDelta(delta.to_string()))).await;
                    }
                }
                "response.reasoning.delta" | "response.reasoning_summary_text.delta" => {
                    if !in_thinking {
                        let _ = tx.send(Ok(StreamEvent::ThinkingStart)).await;
                        in_thinking = true;
                    }
                    if let Some(delta) = json.get("delta").and_then(|d| d.as_str()) {
                        let _ = tx
                            .send(Ok(StreamEvent::ThinkingDelta(delta.to_string())))
                            .await;
                    }
                }
                "response.output_item.added" => {
                    if let Some(item) = json.get("item") {
                        match item.get("type").and_then(|v| v.as_str()) {
                            Some("reasoning") => {
                                let _ = tx.send(Ok(StreamEvent::ThinkingStart)).await;
                                in_thinking = true;
                            }
                            Some("function_call") => {
                                let id = item
                                    .get("call_id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let name = item
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let _ = tx.send(Ok(StreamEvent::ToolUseStart { id, name })).await;
                            }
                            _ => {}
                        }
                    }
                }
                "response.function_call_arguments.delta" => {
                    if let Some(delta) = json.get("delta").and_then(|d| d.as_str()) {
                        let _ = tx
                            .send(Ok(StreamEvent::ToolInputDelta(delta.to_string())))
                            .await;
                    }
                }
                "response.function_call_arguments.done" => {
                    let _ = tx.send(Ok(StreamEvent::ToolUseEnd)).await;
                }
                "response.completed" => {
                    // Close thinking if still active
                    if in_thinking {
                        let _ = tx.send(Ok(StreamEvent::ThinkingEnd)).await;
                    }
                    // Emit token usage if present
                    if let Some(usage) = json.get("usage") {
                        let _ = tx
                            .send(Ok(StreamEvent::TokenUsage {
                                input_tokens: usage.get("input_tokens").and_then(|v| v.as_u64()),
                                output_tokens: usage.get("output_tokens").and_then(|v| v.as_u64()),
                                cache_read_input_tokens: usage
                                    .get("input_tokens_cache_read")
                                    .and_then(|v| v.as_u64()),
                                cache_creation_input_tokens: usage
                                    .get("input_tokens_cache_creation")
                                    .and_then(|v| v.as_u64()),
                            }))
                            .await;
                    }
                    // Forward stop reason from response
                    let stop_reason = json
                        .get("stop_reason")
                        .and_then(|r| r.as_str())
                        .map(|s| s.to_string());
                    let _ = tx.send(Ok(StreamEvent::MessageEnd { stop_reason })).await;
                    return Ok(());
                }
                "response.incomplete" => {
                    if in_thinking {
                        let _ = tx.send(Ok(StreamEvent::ThinkingEnd)).await;
                    }
                    let _ = tx
                        .send(Ok(StreamEvent::MessageEnd {
                            stop_reason: Some("length".to_string()),
                        }))
                        .await;
                    return Ok(());
                }
                "response.failed" => {
                    if in_thinking {
                        let _ = tx.send(Ok(StreamEvent::ThinkingEnd)).await;
                    }
                    let error_msg = json
                        .get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(|m| m.as_str())
                        .unwrap_or("Response failed");
                    anyhow::bail!("Responses API error: {}", error_msg);
                }
                "response.in_progress" | "response.created" | "response.output_item.done" => {
                    // No-op events
                }
                "error" => {
                    if in_thinking {
                        let _ = tx.send(Ok(StreamEvent::ThinkingEnd)).await;
                    }
                    let error_msg = json
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("Stream error");
                    anyhow::bail!("Responses API stream error: {}", error_msg);
                }
                _ => {
                    // Unknown event type — log once for visibility, skip
                    jcode_base::logging::debug(&format!(
                        "Responses API: unknown event type '{}'",
                        event_type
                    ));
                }
            }
        }
    }

    // Stream ended without explicit completion event
    if in_thinking {
        let _ = tx.send(Ok(StreamEvent::ThinkingEnd)).await;
    }
    let _ = tx
        .send(Ok(StreamEvent::MessageEnd { stop_reason: None }))
        .await;
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
