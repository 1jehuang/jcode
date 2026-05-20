use super::*;

fn truncated_stream_payload_context(data: &str) -> String {
    crate::util::truncate_str(&data.trim().replace('\n', "\\n"), 240).to_string()
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
            format!(
                "Failed to send OpenAI-compatible chat request\n  endpoint: {}\n  model: {}\n  auth: {}\nHint: check network connectivity, DNS/TLS, and that the base URL includes the API version (usually /v1).",
                url,
                model,
                auth.label()
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
        anyhow::bail!(
            "OpenAI-compatible chat request failed\n  endpoint: {}\n  model: {}\n  auth: {}\n  status: {}\n  response: {}\nHint: verify the selected model exists in `/models`, your key has access, and the endpoint supports POST /chat/completions with streaming.",
            url,
            model,
            auth.label(),
            status,
            body
        );
    }

    let _ = tx
        .send(Ok(StreamEvent::ConnectionPhase {
            phase: ConnectionPhase::WaitingForResponse,
        }))
        .await;

    let mut stream = OpenRouterStream::new(response.bytes_stream(), model.clone(), provider_pin);

    const SSE_CHUNK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

    loop {
        let event = match tokio::time::timeout(SSE_CHUNK_TIMEOUT, stream.next()).await {
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
                crate::logging::warn("OpenRouter SSE stream timed out (no data for 180s)");
                anyhow::bail!(
                    "OpenAI-compatible stream timeout\n  endpoint: {}\n  model: {}\n  auth: {}\n  timeout: no data received for 180 seconds\nHint: the provider may not support streaming, the model may be overloaded, or the request may be stuck before emitting tokens.",
                    url,
                    model,
                    auth.label()
                );
            }
        };
        if tx.send(Ok(event)).await.is_err() {
            return Ok(());
        }
    }

    Ok(())
}

fn is_retryable_error(error_str: &str) -> bool {
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
        }
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

        if let Some(existing) = self.tool_call_accumulators.get(&index)
            && let Some(ref incoming_id) = incoming_id
            && !existing.id.is_empty()
            && existing.id != *incoming_id
        {
            if let Some(previous) = self.tool_call_accumulators.remove(&index) {
                self.push_completed_tool_call(previous);
            }
        }

        let tc = self
            .tool_call_accumulators
            .entry(index)
            .or_insert_with(ToolCallAccumulator::default);

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
                self.flush_tool_call_accumulators();
                self.pending
                    .push_back(StreamEvent::MessageEnd { stop_reason: None });
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
                    let delta = match choice.get("delta").or_else(|| choice.get("message")) {
                        Some(d) => d,
                        None => continue,
                    };

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
                            self.pending
                                .push_back(StreamEvent::ThinkingDelta(reasoning_delta.to_string()));
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
                    if let Some(tool_calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
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

                    // Check for finish reason
                    if let Some(_finish_reason) =
                        choice.get("finish_reason").and_then(|f| f.as_str())
                    {
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
        assert!(matches!(events[3], StreamEvent::MessageEnd { .. }));
        assert!(stream.tool_call_accumulators.is_empty());
    }
}
