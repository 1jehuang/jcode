use super::*;
use jcode_provider_gemini::interactions::{
    InteractionEvent, PROVIDER_NATIVE_GEMINI, interaction_events,
};

/// Deferred definitions need Interactions tool retrieval, and history holding
/// replay-only Interactions steps must keep using it so their signatures are
/// sent back unchanged.
fn uses_interactions(messages: &[Message], tools: &[ToolDefinition]) -> bool {
    tools.iter().any(|tool| tool.defer_loading)
        || messages.iter().flat_map(|m| m.content.iter()).any(|block| {
            matches!(
                block,
                jcode_message_types::ContentBlock::ProviderNative { provider, .. }
                    if provider == PROVIDER_NATIVE_GEMINI
            )
        })
}

async fn stream_interaction(
    provider: &GeminiProvider,
    tx: &mpsc::Sender<Result<StreamEvent>>,
    api_key: &str,
    model: &str,
    messages: &[Message],
    tools: &[ToolDefinition],
    system: &str,
) {
    let response = match provider
        .create_interaction(api_key, model, messages, tools, system)
        .await
    {
        Ok(response) => response,
        Err(err) => {
            let _ = tx.send(Err(err)).await;
            return;
        }
    };
    let _ = tx
        .send(Ok(StreamEvent::ConnectionPhase {
            phase: ConnectionPhase::Streaming,
        }))
        .await;
    if let Some(usage) = response.usage.as_ref() {
        let _ = tx
            .send(Ok(StreamEvent::TokenUsage {
                input_tokens: usage.total_input_tokens,
                output_tokens: usage.total_output_tokens,
                cache_read_input_tokens: usage.total_cached_tokens,
                cache_creation_input_tokens: None,
            }))
            .await;
    }
    let status = response.status.clone().unwrap_or_default();
    if matches!(status.as_str(), "failed" | "cancelled") {
        let detail = response
            .errors
            .as_ref()
            .map(|errors| format!(": {errors}"))
            .unwrap_or_default();
        let _ = tx
            .send(Err(anyhow::anyhow!("Gemini interaction {status}{detail}")))
            .await;
        return;
    }
    let events = interaction_events(&response);
    let produced_output = events.iter().any(|event| {
        matches!(
            event,
            InteractionEvent::Text(_) | InteractionEvent::FunctionCall { .. }
        )
    });
    if !produced_output {
        let _ = tx
            .send(Err(anyhow::anyhow!(
                "Gemini interaction returned no usable output (status={status})"
            )))
            .await;
        return;
    }
    for event in events {
        match event {
            InteractionEvent::Text(text) => {
                let _ = tx.send(Ok(StreamEvent::TextDelta(text))).await;
            }
            InteractionEvent::Thinking(text) => {
                let _ = tx.send(Ok(StreamEvent::ThinkingStart)).await;
                let _ = tx.send(Ok(StreamEvent::ThinkingDelta(text))).await;
                let _ = tx.send(Ok(StreamEvent::ThinkingEnd)).await;
                let _ = tx
                    .send(Ok(StreamEvent::ThinkingDone { duration_secs: 0.0 }))
                    .await;
            }
            InteractionEvent::Native(item) => {
                let _ = tx
                    .send(Ok(StreamEvent::ProviderNative {
                        provider: PROVIDER_NATIVE_GEMINI.to_string(),
                        item,
                    }))
                    .await;
            }
            InteractionEvent::FunctionCall {
                id,
                name,
                arguments,
                signature,
            } => {
                let raw_id = if id.is_empty() {
                    Uuid::new_v4().to_string()
                } else {
                    id
                };
                let _ = tx
                    .send(Ok(StreamEvent::ToolUseStart {
                        id: jcode_message_types::sanitize_tool_id(&raw_id),
                        name,
                    }))
                    .await;
                let _ = tx
                    .send(Ok(StreamEvent::ToolInputDelta(arguments.to_string())))
                    .await;
                let _ = tx.send(Ok(StreamEvent::ToolUseEnd)).await;
                if let Some(signature) = signature {
                    let _ = tx.send(Ok(StreamEvent::ToolUseSignature(signature))).await;
                }
            }
        }
    }
    let _ = tx
        .send(Ok(StreamEvent::MessageEnd {
            stop_reason: Some(status),
        }))
        .await;
}

async fn consume_gemini_sse_stream(
    response: reqwest::Response,
    envelope: GeminiSseEnvelope,
    tx: &mpsc::Sender<Result<StreamEvent>>,
) -> Result<GeminiStreamFinish> {
    let mut byte_stream = response.bytes_stream();
    let mut utf8 = jcode_base::util::Utf8StreamDecoder::new();
    let mut sse = GeminiSseParser::new();
    let mut acc = GeminiStreamAccumulator::new("Gemini", true, false);
    let mut next_call_id = || Uuid::new_v4().to_string();

    while let Some(chunk_res) = byte_stream.next().await {
        let bytes = chunk_res.context("Failed reading Gemini SSE stream")?;
        let text = utf8.decode(&bytes);
        for data in sse.push(&text) {
            let Some(resp) =
                decode_sse_chunk(envelope, &data).context("Failed to decode Gemini SSE chunk")?
            else {
                continue;
            };
            let result = acc.push_response(resp, &mut next_call_id);
            for event in result.events {
                if tx.send(Ok(event)).await.is_err() {
                    return Ok(GeminiStreamFinish::Completed { events: Vec::new() });
                }
            }
        }
    }

    let trailing = utf8.flush();
    for data in sse.push(&trailing).into_iter().chain(sse.finish()) {
        let Some(resp) = decode_sse_chunk(envelope, &data)
            .context("Failed to decode trailing Gemini SSE chunk")?
        else {
            continue;
        };
        let result = acc.push_response(resp, &mut next_call_id);
        for event in result.events {
            if tx.send(Ok(event)).await.is_err() {
                return Ok(GeminiStreamFinish::Completed { events: Vec::new() });
            }
        }
    }

    Ok(acc.finish())
}

#[async_trait]
impl Provider for GeminiProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let model = self.model();
        let messages = messages.to_vec();
        let tools = tools.to_vec();
        let system = system.to_string();
        let resume_session_id = resume_session_id.map(|value| value.to_string());
        let state_cache = self.state.clone();
        let provider = self.clone();
        let (tx, rx) = mpsc::channel::<Result<StreamEvent>>(100);

        tokio::spawn(async move {
            let _ = tx
                .send(Ok(StreamEvent::ConnectionType {
                    connection: "https".to_string(),
                }))
                .await;
            let _ = tx
                .send(Ok(StreamEvent::ConnectionPhase {
                    phase: ConnectionPhase::Authenticating,
                }))
                .await;

            let state = {
                let provider = GeminiProvider {
                    client: provider.client.clone(),
                    model: provider.model.clone(),
                    state: state_cache.clone(),
                    fetched_models: provider.fetched_models.clone(),
                };
                match provider.ensure_state().await {
                    Ok(state) => state,
                    Err(err) => {
                        let _ = tx.send(Err(err)).await;
                        return;
                    }
                }
            };

            let _ = tx
                .send(Ok(StreamEvent::SessionId(
                    resume_session_id
                        .clone()
                        .unwrap_or_else(|| state.session_id.clone()),
                )))
                .await;
            let _ = tx
                .send(Ok(StreamEvent::ConnectionPhase {
                    phase: ConnectionPhase::SendingRequest,
                }))
                .await;
            let _ = tx
                .send(Ok(StreamEvent::ConnectionPhase {
                    phase: ConnectionPhase::WaitingForResponse,
                }))
                .await;

            if let GeminiAuthMode::ApiKey(api_key) = GeminiProvider::auth_mode()
                && GeminiProvider::native_deferred_route(&model)
                && uses_interactions(&messages, &tools)
            {
                stream_interaction(&provider, &tx, &api_key, &model, &messages, &tools, &system)
                    .await;
                return;
            }

            let mut thinking_mode = GeminiThinkingRequestMode::Full;
            let (response, envelope) = loop {
                match provider
                    .stream_generate_content(
                        &state,
                        &model,
                        &messages,
                        &tools,
                        &system,
                        resume_session_id.as_deref(),
                        thinking_mode,
                    )
                    .await
                {
                    Ok(pair) => break pair,
                    Err(err) if is_gemini_model_not_found_error(&err) => {
                        let _ = tx.send(Err(err.context(format!(
                            "Gemini could not serve selected model '{model}' for this account. Choose an available model with /model"
                        )))).await;
                        return;
                    }
                    Err(err) => {
                        let err_str = format!("{err:#}");
                        if let Some(next_mode) = thinking_mode.downgrade_for_error(&model, &err_str)
                        {
                            jcode_base::logging::warn(&format!(
                                "Gemini rejected thinkingConfig ({thinking_mode:?}); retrying with {next_mode:?}"
                            ));
                            thinking_mode = next_mode;
                            continue;
                        }
                        // A tool schema `streamGenerateContent` rejects 400s
                        // every turn, so the provider is unusable until jcode
                        // ships a new keyword. Learn the rejected construct
                        // from the error, persist it, and retry this turn
                        // without it. See `jcode-schema-dialect`.
                        match provider
                            .retry_after_schema_rejection(
                                &err_str,
                                &state,
                                &model,
                                &messages,
                                &tools,
                                &system,
                                resume_session_id.as_deref(),
                                thinking_mode,
                            )
                            .await
                        {
                            Some(Ok(pair)) => break pair,
                            Some(Err(retry_err)) => {
                                let _ = tx.send(Err(retry_err)).await;
                                return;
                            }
                            None => {
                                let _ = tx.send(Err(err)).await;
                                return;
                            }
                        }
                    }
                }
            };

            let _ = tx
                .send(Ok(StreamEvent::ConnectionPhase {
                    phase: ConnectionPhase::Streaming,
                }))
                .await;

            let finish = match consume_gemini_sse_stream(response, envelope, &tx).await {
                Ok(finish) => finish,
                Err(err) => {
                    let _ = tx.send(Err(err)).await;
                    return;
                }
            };

            match finish {
                GeminiStreamFinish::Completed { events } => {
                    for event in events {
                        let _ = tx.send(Ok(event)).await;
                    }
                }
                GeminiStreamFinish::RetryableEmpty {
                    finish_reason,
                    finish_message,
                    events,
                } => {
                    for event in events {
                        let _ = tx.send(Ok(event)).await;
                    }
                    let detail = finish_message
                        .as_deref()
                        .filter(|msg| !msg.trim().is_empty())
                        .map(|msg| format!(": {}", jcode_base::util::truncate_str(msg.trim(), 300)))
                        .unwrap_or_default();
                    let _ = tx
                        .send(Err(anyhow::anyhow!(
                            "Gemini returned no usable output (finish_reason={finish_reason}){detail}"
                        )))
                        .await;
                }
                GeminiStreamFinish::PseudoToolCall { events } => {
                    for event in events {
                        let _ = tx.send(Ok(event)).await;
                    }
                    let _ = tx
                        .send(Err(anyhow::anyhow!(
                            "Gemini returned an invalid pseudo-tool call"
                        )))
                        .await;
                }
                GeminiStreamFinish::Error { message, events } => {
                    for event in events {
                        let _ = tx.send(Ok(event)).await;
                    }
                    let _ = tx.send(Err(anyhow::anyhow!(message))).await;
                }
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    fn name(&self) -> &'static str {
        "gemini"
    }

    fn model(&self) -> String {
        self.model
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn supports_image_input(&self) -> bool {
        true
    }

    fn supports_deferred_tools(&self) -> bool {
        Self::native_deferred_route(&self.model())
    }

    fn set_model(&self, model: &str) -> Result<()> {
        // See `strip_own_model_prefix`: `--provider gemini` routes through this
        // runtime directly, so session restore hands it `gemini:<model>`.
        let trimmed = jcode_provider_core::strip_own_model_prefix(model, "gemini:");
        if trimmed.is_empty() {
            anyhow::bail!("Gemini model cannot be empty");
        }
        *self
            .model
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = trimmed.to_string();
        Ok(())
    }

    fn available_models(&self) -> Vec<&'static str> {
        AVAILABLE_MODELS.to_vec()
    }

    fn available_models_display(&self) -> Vec<String> {
        let discovered = self
            .fetched_models
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        if discovered.is_empty() {
            return vec![self.model()];
        }

        merge_gemini_model_lists(
            discovered
                .into_iter()
                .chain(std::iter::once(self.model()))
                .collect(),
        )
    }

    fn available_models_for_switching(&self) -> Vec<String> {
        self.available_models_display()
    }

    fn model_routes(&self) -> Vec<jcode_provider_core::ModelRoute> {
        self.available_models_display()
            .into_iter()
            .map(|model| jcode_provider_core::ModelRoute {
                model,
                provider: "Gemini".to_string(),
                api_method: "code-assist-oauth".to_string(),
                available: true,
                detail: String::new(),
                usage: None,
                cheapness: None,
            })
            .collect()
    }

    async fn prefetch_models(&self) -> Result<()> {
        let _ = self.refresh_available_models().await?;
        Ok(())
    }

    fn supports_compaction(&self) -> bool {
        // No native server-side compaction exists for this provider, so jcode's
        // own summary compaction is the only thing standing between a long
        // session and a hard context-limit rejection. Returning `false` here
        // disabled the entire compaction block in `Agent::messages_for_provider`
        // — including the emergency hard-compact and payload truncation at the
        // critical threshold — leaving these sessions with no safety net at all.
        true
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            client: self.client.clone(),
            model: Arc::new(RwLock::new(self.model())),
            state: self.state.clone(),
            fetched_models: self.fetched_models.clone(),
        })
    }

    async fn invalidate_credentials(&self) {
        let mut guard = self.state.lock().await;
        *guard = None;
    }
}
