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
    extra_headers: Vec<(reqwest::header::HeaderName, String)>,
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
            extra_headers.clone(),
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
    extra_headers: Vec<(reqwest::header::HeaderName, String)>,
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

    for (name, value) in &extra_headers {
        req = req.header(name, value);
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
    current_tool_call: Option<ToolCallAccumulator>,
    /// Track if we've emitted the provider info (only emit once)
    provider_emitted: bool,
    model: String,
    provider_pin: Arc<Mutex<Option<ProviderPin>>>,
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
            current_tool_call: None,
            provider_emitted: false,
            model,
            provider_pin,
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
                return Some(StreamEvent::MessageEnd { stop_reason: None });
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

                    if let Some(reasoning_content) =
                        delta.get("reasoning_content").and_then(|c| c.as_str())
                        && !reasoning_content.is_empty()
                    {
                        self.pending
                            .push_back(StreamEvent::ThinkingDelta(reasoning_content.to_string()));
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
                            let _index = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0);

                            // Check if this is a new tool call
                            if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                                // Emit previous tool call if any
                                if let Some(prev) = self.current_tool_call.take()
                                    && !prev.id.is_empty()
                                {
                                    self.pending.push_back(StreamEvent::ToolUseStart {
                                        id: prev.id,
                                        name: prev.name,
                                    });
                                    self.pending
                                        .push_back(StreamEvent::ToolInputDelta(prev.arguments));
                                    self.pending.push_back(StreamEvent::ToolUseEnd);
                                }

                                let name = tc
                                    .get("function")
                                    .and_then(|f| f.get("name"))
                                    .and_then(|n| n.as_str())
                                    .unwrap_or("")
                                    .to_string();

                                self.current_tool_call = Some(ToolCallAccumulator {
                                    id: id.to_string(),
                                    name,
                                    arguments: String::new(),
                                });
                            }

                            // Accumulate arguments
                            if let Some(args) = tc
                                .get("function")
                                .and_then(|f| f.get("arguments"))
                                .and_then(|a| a.as_str())
                                && let Some(ref mut tc) = self.current_tool_call
                            {
                                tc.arguments.push_str(args);
                            }
                        }
                    }

                    // Check for finish reason
                    if let Some(_finish_reason) =
                        choice.get("finish_reason").and_then(|f| f.as_str())
                    {
                        // Emit any pending tool call
                        if let Some(tc) = self.current_tool_call.take()
                            && !tc.id.is_empty()
                        {
                            self.pending.push_back(StreamEvent::ToolUseStart {
                                id: tc.id,
                                name: tc.name,
                            });
                            self.pending
                                .push_back(StreamEvent::ToolInputDelta(tc.arguments));
                            self.pending.push_back(StreamEvent::ToolUseEnd);
                        }

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
                    if let Some(tc) = self.current_tool_call.take()
                        && !tc.id.is_empty()
                    {
                        self.pending.push_back(StreamEvent::ToolUseStart {
                            id: tc.id,
                            name: tc.name,
                        });
                        self.pending
                            .push_back(StreamEvent::ToolInputDelta(tc.arguments));
                        self.pending.push_back(StreamEvent::ToolUseEnd);
                    }
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
        assert!(stream.current_tool_call.is_none());
    }
}
