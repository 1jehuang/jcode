use super::*;

fn truncated_stream_payload_context(data: &str) -> String {
    crate::util::truncate_str(&data.trim().replace('\n', "\\n"), 240).to_string()
}

fn local_endpoint_troubleshooting_hint(api_base: &str, model: &str) -> &'static str {
    let lower = api_base.to_ascii_lowercase();
    if lower.contains("localhost:11434") || lower.contains("127.0.0.1:11434") {
        return "Ollama hint: make sure `ollama serve` is running, the model is installed with `ollama pull <model>`, and run jcode with an installed model, for example `jcode --provider ollama --model llama3.2 run 'hello'`.";
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
    request: Value,
    tx: mpsc::Sender<Result<StreamEvent>>,
    provider_pin: Arc<Mutex<Option<ProviderPin>>>,
    model: String,
) {
    let mut last_error = None;

    for attempt in 0..MAX_RETRIES {
        if attempt > 0 {
            let delay = RETRY_BASE_DELAY_MS * (1 << (attempt - 1));
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            crate::logging::info(&format!(
                "Retrying API request using {} (attempt {}/{})",
                auth.label(),
                attempt + 1,
                MAX_RETRIES
            ));
        }

        crate::logging::info(&format!(
            "API stream attempt {}/{} over HTTPS transport (model: {}, endpoint: {}, auth: {})",
            attempt + 1,
            MAX_RETRIES,
            model,
            api_base,
            auth.label()
        ));

        match stream_response(
            client.clone(),
            api_base.clone(),
            auth.clone(),
            send_openrouter_headers,
            request.clone(),
            tx.clone(),
            Arc::clone(&provider_pin),
            model.clone(),
        )
        .await
        {
            Ok(()) => return,
            Err(e) => {
                let error_str = e.to_string().to_lowercase();
                if is_retryable_error(&error_str) && attempt + 1 < MAX_RETRIES {
                    crate::logging::info(&format!("Transient API error, will retry: {}", e));
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
                MAX_RETRIES,
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
    request: Value,
    tx: mpsc::Sender<Result<StreamEvent>>,
    provider_pin: Arc<Mutex<Option<ProviderPin>>>,
    model: String,
) -> Result<()> {
    use crate::message::ConnectionPhase;
    let _ = tx
        .send(Ok(StreamEvent::ConnectionPhase {
            phase: ConnectionPhase::Connecting,
        }))
        .await;
    let connect_start = std::time::Instant::now();

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

    let response = req
        .json(&request)
        .send()
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
    crate::logging::info(&format!(
        "HTTP connection established in {}ms (status={})",
        connect_ms,
        response.status()
    ));

    if !response.status().is_success() {
        let status = response.status();
        let body = crate::util::http_error_body(response, "HTTP error").await;
        let hint = local_endpoint_troubleshooting_hint(&api_base, &model);
        anyhow::bail!(
            "OpenAI-compatible chat request failed\n  endpoint: {}\n  model: {}\n  auth: {}\n  status: {}\n  response: {}\n{}",
            url,
            model,
            auth.label(),
            status,
            body,
            hint
        );
    }

    let _ = tx
        .send(Ok(StreamEvent::ConnectionPhase {
            phase: ConnectionPhase::WaitingForResponse,
        }))
        .await;

    let mut stream = OpenRouterStream::new(response.bytes_stream(), model.clone(), provider_pin);

    let idle_timeout_secs = crate::provider::sse_timeout::chunk_timeout_secs(180);
    let sse_chunk_timeout = std::time::Duration::from_secs(idle_timeout_secs);

    loop {
        let event = match tokio::time::timeout(sse_chunk_timeout, stream.next()).await {
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
                crate::logging::warn(&format!(
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
    // burns time/credits. 429 (rate limit) is intentionally NOT listed here so
    // it can still be retried.
    if let Some(status) = parsed_http_status(error_str) {
        match status {
            400 | 401 | 402 | 403 | 404 | 405 | 406 | 422 => return false,
            _ => {}
        }
    }

    crate::provider::is_transient_transport_error(error_str)
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

pub(crate) struct OpenRouterStream {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    pub(crate) buffer: String,
    pending: VecDeque<StreamEvent>,
    tool_call_accumulators: std::collections::BTreeMap<u64, ToolCallAccumulator>,
    /// Track if we've emitted the provider info (only emit once)
    provider_emitted: bool,
    model: String,
    provider_pin: Arc<Mutex<Option<ProviderPin>>>,
    reasoning_buffer: String,
    finish_reason: Option<String>,
    message_end_emitted: bool,
}

#[derive(Default)]
struct ToolCallAccumulator {
    id: String,
    name: String,
    arguments: String,
}

impl OpenRouterStream {
    pub(crate) fn new(
        stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
        model: String,
        provider_pin: Arc<Mutex<Option<ProviderPin>>>,
    ) -> Self {
        Self {
            inner: Box::pin(stream),
            buffer: String::new(),
            pending: VecDeque::new(),
            tool_call_accumulators: std::collections::BTreeMap::new(),
            provider_emitted: false,
            model,
            provider_pin,
            reasoning_buffer: String::new(),
            finish_reason: None,
            message_end_emitted: false,
        }
    }

    fn queue_message_end(&mut self) {
        if self.message_end_emitted {
            return;
        }

        self.flush_tool_call_accumulators();
        self.message_end_emitted = true;
        self.pending.push_back(StreamEvent::MessageEnd {
            stop_reason: self.finish_reason.take(),
        });
    }

    fn observe_provider(&mut self, provider: &str) {
        let mut pin = self
            .provider_pin
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(existing) = pin.as_ref() {
            if existing.source == PinSource::Explicit && existing.model == self.model {
                return;
            }
            if existing.source == PinSource::Observed
                && existing.model == self.model
                && existing.provider == provider
            {
                return;
            }
        }

        *pin = Some(ProviderPin {
            model: self.model.clone(),
            provider: provider.to_string(),
            source: PinSource::Observed,
            allow_fallbacks: true,
            last_cache_read: None,
        });
    }

    fn refresh_cache_pin(&mut self, provider: &str) {
        let mut pin = self
            .provider_pin
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(existing) = pin.as_mut()
            && existing.model == self.model
            && existing.provider == provider
        {
            existing.last_cache_read = Some(Instant::now());
        }
    }

    fn push_completed_tool_call(&mut self, tc: ToolCallAccumulator) {
        if tc.id.trim().is_empty() {
            crate::logging::warn(&format!(
                "OpenRouter SSE dropped incomplete tool call for model {}: missing id (name={} args_len={})",
                self.model,
                tc.name,
                tc.arguments.len()
            ));
            return;
        }

        if tc.name.trim().is_empty() {
            crate::logging::warn(&format!(
                "OpenRouter SSE dropped incomplete tool call for model {}: missing name (id={} args_len={})",
                self.model,
                tc.id,
                tc.arguments.len()
            ));
            return;
        }

        self.pending.push_back(StreamEvent::ToolUseStart {
            id: tc.id,
            name: tc.name,
        });
        self.pending
            .push_back(StreamEvent::ToolInputDelta(tc.arguments));
        self.pending.push_back(StreamEvent::ToolUseEnd);
    }

    fn flush_tool_call_accumulators(&mut self) {
        let calls = std::mem::take(&mut self.tool_call_accumulators);
        for (_index, tc) in calls {
            self.push_completed_tool_call(tc);
        }
    }

    fn apply_tool_call_delta(
        &mut self,
        index: u64,
        id: Option<&str>,
        name: Option<&str>,
        arguments: Option<&str>,
    ) {
        let incoming_id = id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);

        if self
            .tool_call_accumulators
            .get(&index)
            .is_some_and(|existing| {
                incoming_id.as_ref().is_some_and(|incoming_id| {
                    !existing.id.is_empty() && existing.id != *incoming_id
                })
            })
            && let Some(previous) = self.tool_call_accumulators.remove(&index)
        {
            self.push_completed_tool_call(previous);
        }

        let tc = self.tool_call_accumulators.entry(index).or_default();

        if tc.id.is_empty()
            && let Some(incoming_id) = incoming_id
        {
            tc.id = incoming_id;
        }

        if tc.name.trim().is_empty()
            && let Some(incoming_name) = name.map(str::trim).filter(|value| !value.is_empty())
        {
            tc.name = incoming_name.to_string();
        }

        if let Some(args) = arguments {
            tc.arguments.push_str(args);
        }
    }

    pub(crate) fn parse_next_event(&mut self) -> Option<StreamEvent> {
        if let Some(event) = self.pending.pop_front() {
            return Some(event);
        }

        while let Some(pos) = self.buffer.find("\n\n") {
            let event_str = self.buffer[..pos].to_string();
            self.buffer = self.buffer[pos + 2..].to_string();

            // Parse SSE event
            let mut data = None;
            for line in event_str.lines() {
                if let Some(d) = crate::util::sse_data_line(line) {
                    data = Some(d);
                }
            }

            let data = match data {
                Some(d) => d,
                None => continue,
            };

            if data == "[DONE]" {
                self.queue_message_end();
                return self.pending.pop_front();
            }

            let parsed: Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(error) => {
                    crate::logging::warn(&format!(
                        "OpenRouter SSE JSON parse failed for model {}: {} payload={} ",
                        self.model,
                        error,
                        truncated_stream_payload_context(data)
                    ));
                    continue;
                }
            };

            // Extract upstream provider info (only emit once)
            // OpenRouter returns "provider" field indicating which provider handled the request
            if !self.provider_emitted
                && let Some(provider) = parsed.get("provider").and_then(|p| p.as_str())
            {
                self.provider_emitted = true;
                self.observe_provider(provider);
                self.pending.push_back(StreamEvent::UpstreamProvider {
                    provider: provider.to_string(),
                });
            }

            // Check for error
            if let Some(error) = parsed.get("error") {
                let message = error
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("OpenRouter error")
                    .to_string();
                return Some(StreamEvent::Error {
                    message,
                    retry_after_secs: None,
                });
            }

            // Parse choices
            if let Some(choices) = parsed.get("choices").and_then(|c| c.as_array()) {
                for choice in choices {
                    if let Some(delta) = choice.get("delta").or_else(|| choice.get("message")) {
                        if let Some(reasoning_content) = delta
                            .get("reasoning_content")
                            .or_else(|| delta.get("reasoning"))
                            .and_then(|c| c.as_str())
                            && !reasoning_content.is_empty()
                        {
                            let reasoning_delta =
                                if reasoning_content.starts_with(&self.reasoning_buffer) {
                                    &reasoning_content[self.reasoning_buffer.len()..]
                                } else {
                                    reasoning_content
                                };
                            self.reasoning_buffer = reasoning_content.to_string();
                            if !reasoning_delta.is_empty() {
                                self.pending.push_back(StreamEvent::ThinkingDelta(
                                    reasoning_delta.to_string(),
                                ));
                            }
                        }

                        // Text content
                        if let Some(content) = delta.get("content").and_then(|c| c.as_str())
                            && !content.is_empty()
                        {
                            self.pending
                                .push_back(StreamEvent::TextDelta(content.to_string()));
                        }

                        // Tool calls
                        if let Some(tool_calls) = delta.get("tool_calls").and_then(|t| t.as_array())
                        {
                            for tc in tool_calls {
                                let index = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
                                let function = tc.get("function");
                                self.apply_tool_call_delta(
                                    index,
                                    tc.get("id").and_then(|i| i.as_str()),
                                    function
                                        .and_then(|f| f.get("name"))
                                        .and_then(|n| n.as_str()),
                                    function
                                        .and_then(|f| f.get("arguments"))
                                        .and_then(|a| a.as_str()),
                                );
                            }
                        }
                    }

                    // Check for finish reason
                    if let Some(finish_reason) =
                        choice.get("finish_reason").and_then(|f| f.as_str())
                    {
                        let finish_reason = finish_reason.trim();
                        if !finish_reason.is_empty() {
                            self.finish_reason = Some(finish_reason.to_string());
                        }
                        // Emit any pending tool calls.
                        self.flush_tool_call_accumulators();

                        // Don't emit MessageEnd here - wait for [DONE]
                    }
                }
            }

            // Extract usage if present
            if let Some(usage) = parsed.get("usage") {
                let input_tokens = usage.get("prompt_tokens").and_then(|t| t.as_u64());
                let output_tokens = usage.get("completion_tokens").and_then(|t| t.as_u64());

                // OpenRouter returns cached tokens in various formats depending on provider:
                // - "cached_tokens" (OpenRouter's unified field)
                // - "prompt_tokens_details.cached_tokens" (OpenAI-style)
                // - "cache_read_input_tokens" (Anthropic-style, passed through)
                let cache_read_input_tokens = usage
                    .get("cached_tokens")
                    .and_then(|t| t.as_u64())
                    .or_else(|| {
                        usage
                            .get("prompt_tokens_details")
                            .and_then(|d| d.get("cached_tokens"))
                            .and_then(|t| t.as_u64())
                    })
                    .or_else(|| {
                        usage
                            .get("cache_read_input_tokens")
                            .and_then(|t| t.as_u64())
                    });

                // Cache creation tokens (Anthropic-style, passed through for some providers)
                let cache_creation_input_tokens = usage
                    .get("cache_creation_input_tokens")
                    .and_then(|t| t.as_u64());

                // Refresh cache pin when we see cache activity
                if (cache_read_input_tokens.is_some() || cache_creation_input_tokens.is_some())
                    && let Some(provider) = parsed.get("provider").and_then(|p| p.as_str())
                {
                    self.refresh_cache_pin(provider);
                }

                if input_tokens.is_some()
                    || output_tokens.is_some()
                    || cache_read_input_tokens.is_some()
                {
                    self.pending.push_back(StreamEvent::TokenUsage {
                        input_tokens,
                        output_tokens,
                        cache_read_input_tokens,
                        cache_creation_input_tokens,
                    });
                }
            }

            if let Some(event) = self.pending.pop_front() {
                return Some(event);
            }
        }

        None
    }
}

impl Stream for OpenRouterStream {
    type Item = Result<StreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(event) = self.parse_next_event() {
                return Poll::Ready(Some(Ok(event)));
            }

            match self.inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(bytes))) => {
                    if let Ok(text) = std::str::from_utf8(&bytes) {
                        self.buffer.push_str(text);
                    }
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(anyhow::anyhow!("Stream error: {}", e))));
                }
                Poll::Ready(None) => {
                    // Stream ended - emit any pending tool call
                    self.flush_tool_call_accumulators();
                    if let Some(event) = self.pending.pop_front() {
                        return Poll::Ready(Some(Ok(event)));
                    }
                    if !self.message_end_emitted {
                        self.message_end_emitted = true;
                        return Poll::Ready(Some(Ok(StreamEvent::MessageEnd {
                            stop_reason: self.finish_reason.take(),
                        })));
                    }
                    return Poll::Ready(None);
                }
                Poll::Pending => {
                    return Poll::Pending;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_next_event_ignores_malformed_json_chunks() {
        let provider_pin = Arc::new(std::sync::Mutex::new(None));
        let mut stream = OpenRouterStream::new(
            futures::stream::empty(),
            "test-model".to_string(),
            provider_pin,
        );
        stream.buffer = "data: {not-json}

"
        .to_string();

        let event = stream.parse_next_event();

        assert!(event.is_none());
        assert!(stream.pending.is_empty());
        assert!(stream.tool_call_accumulators.is_empty());
    }

    #[test]
    fn parse_next_event_accepts_reasoning_delta_alias() {
        let provider_pin = Arc::new(std::sync::Mutex::new(None));
        let mut stream = OpenRouterStream::new(
            futures::stream::empty(),
            "test-model".to_string(),
            provider_pin,
        );
        stream.buffer =
            "data: {\"choices\":[{\"delta\":{\"reasoning\":\"thinking\"}}]}\n\n".to_string();

        let event = stream.parse_next_event();

        assert!(matches!(event, Some(StreamEvent::ThinkingDelta(text)) if text == "thinking"));
    }

    #[test]
    fn parse_next_event_propagates_finish_reason_to_message_end() {
        let provider_pin = Arc::new(std::sync::Mutex::new(None));
        let mut stream = OpenRouterStream::new(
            futures::stream::empty(),
            "test-model".to_string(),
            provider_pin,
        );
        stream.buffer =
            "data: {\"choices\":[{\"finish_reason\":\"length\"}]}\n\ndata: [DONE]\n\n".to_string();

        let event = stream.parse_next_event();

        assert!(matches!(
            event,
            Some(StreamEvent::MessageEnd { stop_reason: Some(reason) }) if reason == "length"
        ));
    }

    #[test]
    fn stream_eof_emits_message_end_with_finish_reason_without_done() {
        let provider_pin = Arc::new(std::sync::Mutex::new(None));
        let bytes = Bytes::from_static(
            b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"max_tokens\"}]}\n\n",
        );
        let mut stream = OpenRouterStream::new(
            futures::stream::once(async move { Ok(bytes) }),
            "test-model".to_string(),
            provider_pin,
        );

        let event = futures::executor::block_on(stream.next());

        assert!(matches!(
            event,
            Some(Ok(StreamEvent::MessageEnd { stop_reason: Some(reason) })) if reason == "max_tokens"
        ));
        assert!(futures::executor::block_on(stream.next()).is_none());
    }

    #[test]
    fn parse_next_event_coalesces_repeated_tool_call_id_chunks() {
        let provider_pin = Arc::new(std::sync::Mutex::new(None));
        let mut stream =
            OpenRouterStream::new(futures::stream::empty(), "glm-5".to_string(), provider_pin);

        let chunk1 = serde_json::json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "bash", "arguments": ""}
                    }]
                }
            }]
        });
        let chunk2 = serde_json::json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": {"arguments": "{\"command\""}
                    }]
                }
            }]
        });
        let chunk3 = serde_json::json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_1",
                        "function": {"arguments": ":\"echo ok\"}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });
        stream.buffer =
            format!("data: {chunk1}\n\ndata: {chunk2}\n\ndata: {chunk3}\n\ndata: [DONE]\n\n");

        let mut events = Vec::new();
        for _ in 0..8 {
            if let Some(event) = stream.parse_next_event() {
                events.push(event);
            } else {
                break;
            }
        }

        assert_eq!(events.len(), 4, "events: {events:?}");
        assert!(matches!(
            &events[0],
            StreamEvent::ToolUseStart { id, name } if id == "call_1" && name == "bash"
        ));
        assert!(matches!(
            &events[1],
            StreamEvent::ToolInputDelta(args) if args == "{\"command\":\"echo ok\"}"
        ));
        assert!(matches!(events[2], StreamEvent::ToolUseEnd));
        assert!(matches!(
            &events[3],
            StreamEvent::MessageEnd { stop_reason } if stop_reason.as_deref() == Some("tool_calls")
        ));
        assert!(stream.tool_call_accumulators.is_empty());
    }

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
        // Rate limiting should still be retried.
        assert!(is_retryable_error("overloaded"));
    }
}
