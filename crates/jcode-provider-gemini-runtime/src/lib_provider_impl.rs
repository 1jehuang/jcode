use super::*;

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

            let response = match provider
                .generate_content(
                    &state,
                    &model,
                    &messages,
                    &tools,
                    &system,
                    resume_session_id.as_deref(),
                )
                .await
            {
                Ok(response) => response,
                Err(err) if is_gemini_model_not_found_error(&err) => {
                    let mut fallback_response = None;
                    let mut last_err = err;
                    for fallback_model in gemini_fallback_models(&model) {
                        jcode_base::logging::warn(&format!(
                            "Gemini model '{}' was not found; retrying with fallback '{}'",
                            model, fallback_model
                        ));
                        match provider
                            .generate_content(
                                &state,
                                fallback_model,
                                &messages,
                                &tools,
                                &system,
                                resume_session_id.as_deref(),
                            )
                            .await
                        {
                            Ok(response) => {
                                let _ = provider.set_model(fallback_model);
                                fallback_response = Some(response);
                                break;
                            }
                            Err(err) => {
                                last_err = err;
                            }
                        }
                    }

                    match fallback_response {
                        Some(response) => response,
                        None => {
                            let _ = tx.send(Err(last_err)).await;
                            return;
                        }
                    }
                }
                Err(err) => {
                    // A tool schema `generateContent` rejects 400s every turn,
                    // so the provider is unusable until jcode ships a new
                    // keyword. Learn the rejected construct from the error,
                    // persist it, and retry this turn without it. See
                    // `jcode-schema-dialect`.
                    match provider
                        .retry_after_schema_rejection(
                            &err.to_string(),
                            &state,
                            &model,
                            &messages,
                            &tools,
                            &system,
                            resume_session_id.as_deref(),
                        )
                        .await
                    {
                        Some(Ok(response)) => response,
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
            };

            let _ = tx
                .send(Ok(StreamEvent::ConnectionPhase {
                    phase: ConnectionPhase::Streaming,
                }))
                .await;

            if let Some(event) = response.resolved_model_event() {
                let _ = tx.send(Ok(event)).await;
            }

            if let Some(usage) = response
                .response
                .as_ref()
                .and_then(|response| response.usage_metadata.as_ref())
            {
                let _ = tx
                    .send(Ok(StreamEvent::TokenUsage {
                        input_tokens: usage.input_token_count(),
                        output_tokens: usage.output_token_count(),
                        cache_read_input_tokens: usage.cached_content_token_count,
                        cache_creation_input_tokens: None,
                    }))
                    .await;
            }

            let response_body = response.response;

            let candidate = response_body
                .as_ref()
                .and_then(|response| response.candidates.as_ref())
                .and_then(|candidates| candidates.first())
                .cloned();

            if candidate.is_none() {
                if let Some(feedback) = response_body
                    .as_ref()
                    .and_then(|response| response.prompt_feedback.as_ref())
                {
                    let block_reason = feedback.block_reason.as_deref().unwrap_or("unspecified");
                    let detail = feedback
                        .block_reason_message
                        .as_deref()
                        .filter(|msg| !msg.trim().is_empty())
                        .map(|msg| format!(": {}", msg.trim()))
                        .unwrap_or_default();
                    let _ = tx
                        .send(Err(anyhow::anyhow!(
                            "Gemini blocked the prompt ({}){}",
                            block_reason,
                            detail
                        )))
                        .await;
                    return;
                }

                let _ = tx
                    .send(Err(anyhow::anyhow!(
                        "Gemini returned no candidates for generateContent"
                    )))
                    .await;
                return;
            }

            let mut stop_reason = None;
            if let Some(candidate) = candidate {
                stop_reason = candidate
                    .finish_reason
                    .clone()
                    .map(|reason| reason.to_lowercase());
                if candidate.content.is_none()
                    && matches!(
                        candidate.finish_reason.as_deref(),
                        Some("SAFETY" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "RECITATION")
                    )
                {
                    let reason = candidate.finish_reason.as_deref().unwrap_or("unknown");
                    let detail = candidate
                        .finish_message
                        .as_deref()
                        .filter(|msg| !msg.trim().is_empty())
                        .map(|msg| format!(": {}", msg.trim()))
                        .unwrap_or_default();
                    let _ = tx
                        .send(Err(anyhow::anyhow!(
                            "Gemini stopped without content ({}){}",
                            reason,
                            detail
                        )))
                        .await;
                    return;
                }
                // Track whether this candidate produced any usable output (text or
                // a tool call). Gemini-3 thinking models intermittently emit
                // Python-style pseudo-code instead of a clean functionCall and
                // finish with `MALFORMED_FUNCTION_CALL` and empty content; surface
                // that as a retryable error below rather than a silent empty turn.
                let mut produced_output = false;
                if let Some(content) = candidate.content {
                    // Gemini 3 attaches a `thoughtSignature` to function-call
                    // parts (and occasionally to a standalone preceding part).
                    // Replay it via a ToolUseSignature event so it is persisted
                    // on the ToolUse block and resent on later turns; the API
                    // rejects follow-up turns whose functionCall omits it
                    // ("Function call is missing a thought_signature").
                    let mut pending_signature: Option<String> = None;
                    for part in content.parts {
                        let part_signature = part
                            .thought_signature
                            .as_ref()
                            .filter(|sig| !sig.is_empty())
                            .cloned();
                        if let Some(text) = part.text
                            && !text.is_empty()
                        {
                            produced_output = true;
                            let _ = tx.send(Ok(StreamEvent::TextDelta(text))).await;
                        }
                        if let Some(function_call) = part.function_call {
                            produced_output = true;
                            let signature =
                                part_signature.clone().or_else(|| pending_signature.take());
                            let raw_call_id = function_call
                                .id
                                .clone()
                                .unwrap_or_else(|| Uuid::new_v4().to_string());
                            let call_id = jcode_message_types::sanitize_tool_id(&raw_call_id);
                            let _ = tx
                                .send(Ok(StreamEvent::ToolUseStart {
                                    id: call_id,
                                    name: function_call.name,
                                }))
                                .await;
                            let _ = tx
                                .send(Ok(StreamEvent::ToolInputDelta(
                                    function_call.args.to_string(),
                                )))
                                .await;
                            let _ = tx.send(Ok(StreamEvent::ToolUseEnd)).await;
                            if let Some(signature) = signature {
                                let _ = tx.send(Ok(StreamEvent::ToolUseSignature(signature))).await;
                            }
                        } else if let Some(signature) = part_signature {
                            // Standalone signature part; remember it for the next
                            // function call in this candidate.
                            pending_signature = Some(signature);
                        }
                    }
                    // A thought signature not consumed by a following function
                    // call (e.g. a pure-text reasoning turn) is still an opaque
                    // reasoning signal. Surface it as a ThinkingSignatureDelta
                    // instead of dropping it.
                    if let Some(signature) = pending_signature.take() {
                        let _ = tx
                            .send(Ok(StreamEvent::ThinkingSignatureDelta(signature)))
                            .await;
                    }
                }

                // An abnormal finish (typically Gemini-3's intermittent
                // `MALFORMED_FUNCTION_CALL`) that yielded no text and no tool call
                // is a dead turn: surface it as a retryable error instead of a
                // silent empty `MessageEnd`. `STOP`/`MAX_TOKENS` are normal.
                if !produced_output {
                    let abnormal = candidate
                        .finish_reason
                        .as_deref()
                        .map(|reason| {
                            !matches!(
                                reason.to_ascii_uppercase().as_str(),
                                "STOP" | "MAX_TOKENS" | "FINISH_REASON_UNSPECIFIED" | ""
                            )
                        })
                        .unwrap_or(false);
                    if abnormal {
                        let reason = candidate.finish_reason.as_deref().unwrap_or("unknown");
                        let detail = candidate
                            .finish_message
                            .as_deref()
                            .filter(|msg| !msg.trim().is_empty())
                            .map(|msg| {
                                format!(": {}", jcode_base::util::truncate_str(msg.trim(), 300))
                            })
                            .unwrap_or_default();
                        let _ = tx
                            .send(Err(anyhow::anyhow!(
                                "Gemini returned no usable output (finish_reason={reason}){detail}"
                            )))
                            .await;
                        return;
                    }
                }
            }

            let _ = tx.send(Ok(StreamEvent::MessageEnd { stop_reason })).await;
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
