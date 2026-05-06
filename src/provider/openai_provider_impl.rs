use super::openai_stream_runtime::{
    stream_response, stream_response_websocket_persistent, try_persistent_ws_continuation,
};
use super::*;

#[async_trait]
impl Provider for OpenAIProvider {
    async fn complete(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
        system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let input = build_responses_input(messages);
        let input_item_count = input.len();
        let api_tools = build_tools(tools);
        let model_id = self.model_id().await;
        let (instructions, is_chatgpt_mode) = {
            let credentials = self.credentials.read().await;
            let is_chatgpt = Self::is_chatgpt_mode(&credentials);
            let instructions = if is_chatgpt {
                Self::chatgpt_instructions_with_selfdev(system)
            } else {
                system.to_string()
            };
            (instructions, is_chatgpt)
        };
        let reasoning_effort = self
            .reasoning_effort
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone());
        let service_tier = self
            .service_tier
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone());
        let native_compaction_threshold =
            self.native_compaction_threshold_for_context_window(self.context_window());
        let request = Self::build_response_request(
            &model_id,
            instructions,
            &input,
            &api_tools,
            is_chatgpt_mode,
            self.max_output_tokens,
            reasoning_effort.as_deref(),
            service_tier.as_deref(),
            self.prompt_cache_key.as_deref(),
            self.prompt_cache_retention.as_deref(),
            native_compaction_threshold,
        );

        // --- Persistent WebSocket continuation path ---
        // Try to reuse an existing WebSocket connection with previous_response_id
        // to send only incremental input items instead of the full conversation.
        let persistent_ws = Arc::clone(&self.persistent_ws);
        let transport_mode_snapshot = self
            .transport_mode
            .try_read()
            .map(|g| *g)
            .unwrap_or(OpenAITransportMode::HTTPS);
        let use_websocket_transport = match transport_mode_snapshot {
            OpenAITransportMode::HTTPS => false,
            OpenAITransportMode::WebSocket => true,
            OpenAITransportMode::Auto => Self::should_prefer_websocket(&model_id),
        };
        let usage_snapshot = crate::usage::get_openai_usage_sync();
        crate::logging::info(&format!(
            "OpenAI limit diag: request start model={} transport_mode={} websocket_preferred={} usage=({}) provider=({})",
            model_id,
            transport_mode_snapshot.as_str(),
            use_websocket_transport,
            usage_snapshot.diagnostic_fields(),
            self.diagnostic_state_summary()
        ));

        let (tx, rx) = mpsc::channel::<Result<StreamEvent>>(100);

        let credentials = Arc::clone(&self.credentials);
        let transport_mode = transport_mode_snapshot;
        let websocket_cooldowns = Arc::clone(&self.websocket_cooldowns);
        let websocket_failure_streaks = Arc::clone(&self.websocket_failure_streaks);
        let model_for_transport = model_id.clone();
        let client = self.client.clone();
        let panic_tx = tx.clone();

        tokio::spawn(async move {
            let stream_task = async move {
                // Attempt persistent WebSocket continuation first
                if use_websocket_transport {
                    let continuation_result = try_persistent_ws_continuation(
                        &persistent_ws,
                        &request,
                        &input,
                        input_item_count,
                        &tx,
                    )
                    .await;

                    match continuation_result {
                        PersistentWsResult::Success => {
                            record_websocket_success(
                                &websocket_cooldowns,
                                &websocket_failure_streaks,
                                &model_for_transport,
                            )
                            .await;
                            return;
                        }
                        PersistentWsResult::NotAvailable => {
                            crate::logging::info(
                                "No persistent WS connection available; using fresh connection",
                            );
                        }
                        PersistentWsResult::Failed(err) => {
                            crate::logging::warn(&format!(
                                "Persistent WS continuation failed: {}; using fresh connection",
                                err
                            ));
                            let mut guard = persistent_ws.lock().await;
                            *guard = None;
                        }
                    }
                }

                // Normal path: fresh connection with full input (with retry logic)
                let mut last_error = None;
                let mut force_https_for_request = false;
                let mut skip_backoff_once = false;

                for attempt in 0..MAX_RETRIES {
                    if attempt > 0 {
                        emit_connection_phase(
                            &tx,
                            crate::message::ConnectionPhase::Retrying {
                                attempt: attempt + 1,
                                max: MAX_RETRIES,
                            },
                        )
                        .await;
                    }
                    if attempt > 0 && !skip_backoff_once {
                        let delay = RETRY_BASE_DELAY_MS * (1 << (attempt - 1));
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        crate::logging::info(&format!(
                            "Retrying OpenAI API request (attempt {}/{})",
                            attempt + 1,
                            MAX_RETRIES
                        ));
                    }
                    skip_backoff_once = false;

                    let transport = if force_https_for_request {
                        OpenAITransport::HTTPS
                    } else {
                        match transport_mode {
                            OpenAITransportMode::HTTPS => OpenAITransport::HTTPS,
                            OpenAITransportMode::WebSocket => OpenAITransport::WebSocket,
                            OpenAITransportMode::Auto => {
                                if !Self::should_prefer_websocket(&model_for_transport) {
                                    OpenAITransport::HTTPS
                                } else if let Some(remaining) = websocket_cooldown_remaining(
                                    &websocket_cooldowns,
                                    &model_for_transport,
                                )
                                .await
                                {
                                    crate::logging::info(&format!(
                                        "OpenAI websocket cooldown active for model='{}' ({}s remaining); using HTTPS",
                                        model_for_transport,
                                        remaining.as_secs()
                                    ));
                                    emit_status_detail(
                                        &tx,
                                        format!(
                                            "https cooldown {}",
                                            format_status_duration(remaining)
                                        ),
                                    )
                                    .await;
                                    OpenAITransport::HTTPS
                                } else {
                                    OpenAITransport::WebSocket
                                }
                            }
                        }
                    };

                    let transport_label = transport.as_str();
                    let attempt_started = Instant::now();
                    crate::logging::info(&format!(
                        "OpenAI stream attempt {}/{} using transport '{}'; model='{}'; mode='{}'",
                        attempt + 1,
                        MAX_RETRIES,
                        transport_label,
                        model_for_transport,
                        transport_mode.as_str()
                    ));

                    let use_websocket = matches!(transport, OpenAITransport::WebSocket);
                    let result = if use_websocket {
                        stream_response_websocket_persistent(
                            Arc::clone(&credentials),
                            request.clone(),
                            tx.clone(),
                            Arc::clone(&persistent_ws),
                            input_item_count,
                        )
                        .await
                    } else {
                        stream_response(
                            client.clone(),
                            Arc::clone(&credentials),
                            request.clone(),
                            if force_https_for_request {
                                let reason = last_error
                                    .as_ref()
                                    .map(|error: &anyhow::Error| {
                                        summarize_websocket_fallback_reason(&error.to_string())
                                    })
                                    .unwrap_or("websocket error");
                                format!("https fallback: {}", reason)
                            } else if let Some(remaining) = websocket_cooldown_remaining(
                                &websocket_cooldowns,
                                &model_for_transport,
                            )
                            .await
                            {
                                format!("https cooldown {}", format_status_duration(remaining))
                            } else {
                                "https".to_string()
                            },
                            tx.clone(),
                        )
                        .await
                    };

                    match result {
                        Ok(()) => {
                            if use_websocket {
                                record_websocket_success(
                                    &websocket_cooldowns,
                                    &websocket_failure_streaks,
                                    &model_for_transport,
                                )
                                .await;
                            }
                            return;
                        }
                        Err(OpenAIStreamFailure::FallbackToHttps(error)) => {
                            let elapsed_ms = attempt_started.elapsed().as_millis();
                            let reason = summarize_websocket_fallback_reason(&error.to_string());
                            let fallback_reason =
                                classify_websocket_fallback_reason(&error.to_string());
                            crate::logging::warn(&format!(
                                "WebSocket fallback after {}ms: {}",
                                elapsed_ms, error
                            ));
                            emit_status_detail(&tx, format!("https fallback: {}", reason)).await;
                            force_https_for_request = true;
                            skip_backoff_once = true;
                            if matches!(transport_mode, OpenAITransportMode::Auto) {
                                let (streak, cooldown) = record_websocket_fallback(
                                    &websocket_cooldowns,
                                    &websocket_failure_streaks,
                                    &model_for_transport,
                                    fallback_reason,
                                )
                                .await;
                                crate::logging::warn(&format!(
                                    "OpenAI websocket backoff for model='{}': reason='{}' streak={} cooldown={}s",
                                    model_for_transport,
                                    fallback_reason.summary(),
                                    streak,
                                    cooldown.as_secs()
                                ));
                            }
                            // Clear persistent state on fallback
                            {
                                let mut guard = persistent_ws.lock().await;
                                *guard = None;
                            }
                            last_error = Some(error);
                            continue;
                        }
                        Err(OpenAIStreamFailure::Other(error)) => {
                            let elapsed_ms = attempt_started.elapsed().as_millis();
                            let error_str = error.to_string().to_lowercase();
                            if is_retryable_error(&error_str) && attempt + 1 < MAX_RETRIES {
                                crate::logging::info(&format!(
                                    "Transient error after {}ms, will retry: {}",
                                    elapsed_ms, error
                                ));
                                last_error = Some(error);
                                continue;
                            }
                            let _ = tx.send(Err(error)).await;
                            return;
                        }
                    }
                }

                // All retries exhausted
                if let Some(e) = last_error {
                    let _ = tx
                        .send(Err(anyhow::anyhow!(
                            "Failed after {} retries: {}",
                            MAX_RETRIES,
                            e
                        )))
                        .await;
                }
            };

            let result = AssertUnwindSafe(stream_task).catch_unwind().await;

            if let Err(panic_payload) = result {
                let msg = if let Some(text) = panic_payload.downcast_ref::<&str>() {
                    (*text).to_string()
                } else if let Some(text) = panic_payload.downcast_ref::<String>() {
                    text.clone()
                } else {
                    "unknown panic".to_string()
                };
                crate::logging::error(&format!("OpenAI provider stream task panicked: {}", msg));
                let _ = panic_tx
                    .send(Err(anyhow::anyhow!(
                        "OpenAI provider stream task panicked: {}",
                        msg
                    )))
                    .await;
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    fn name(&self) -> &str {
        "openai"
    }

    fn on_auth_changed(&self) {
        self.reload_credentials_now();
    }

    fn model(&self) -> String {
        // Use try_read to avoid blocking - fall back to default if locked
        self.model
            .try_read()
            .map(|m| m.clone())
            .unwrap_or_else(|_| DEFAULT_MODEL.to_string())
    }

    fn supports_image_input(&self) -> bool {
        true
    }

    fn set_model(&self, model: &str) -> Result<()> {
        if !crate::provider::known_openai_model_ids()
            .iter()
            .any(|known| known == model)
        {
            anyhow::bail!(
                "Unsupported OpenAI model '{}'. Use /model to choose from the models available to your account.",
                model,
            );
        }
        let availability = crate::provider::model_availability_for_account(model);
        if availability.state == crate::provider::AccountModelAvailabilityState::Unavailable {
            let detail = crate::provider::format_account_model_availability_detail(&availability)
                .unwrap_or_else(|| "not available for your account".to_string());
            anyhow::bail!(
                "The '{}' model is not available for your account right now ({}). \
                 Use /model to see available models.",
                model,
                detail
            );
        }
        if let Ok(mut current) = self.model.try_write() {
            let changed = current.as_str() != model;
            *current = model.to_string();
            crate::provider::clear_model_unavailable_for_account(model);
            drop(current);
            if changed {
                self.clear_persistent_ws_try("manual OpenAI model change reset the response chain");
            }
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Cannot change model while a request is in progress"
            ))
        }
    }

    fn available_models(&self) -> Vec<&'static str> {
        crate::provider::ALL_OPENAI_MODELS.to_vec()
    }

    fn available_models_for_switching(&self) -> Vec<String> {
        crate::provider::cached_openai_model_ids().unwrap_or_else(|| vec![self.model()])
    }

    fn available_models_display(&self) -> Vec<String> {
        self.available_models_for_switching()
    }

    async fn prefetch_models(&self) -> Result<()> {
        let access_token = openai_access_token(&self.credentials).await?;
        let catalog = crate::provider::fetch_openai_model_catalog(&access_token).await?;
        crate::provider::persist_openai_model_catalog(&catalog);
        if !catalog.context_limits.is_empty() {
            crate::provider::populate_context_limits(catalog.context_limits);
        }
        if !catalog.available_models.is_empty() {
            crate::provider::populate_account_models(catalog.available_models);
        }
        Ok(())
    }

    fn reasoning_effort(&self) -> Option<String> {
        self.reasoning_effort
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }

    fn set_reasoning_effort(&self, effort: &str) -> Result<()> {
        let normalized = Self::normalize_reasoning_effort(effort);
        match self.reasoning_effort.write() {
            Ok(mut guard) => {
                *guard = normalized;
                Ok(())
            }
            Err(poisoned) => {
                *poisoned.into_inner() = normalized;
                Ok(())
            }
        }
    }

    fn available_efforts(&self) -> Vec<&'static str> {
        vec!["none", "low", "medium", "high", "xhigh"]
    }

    fn service_tier(&self) -> Option<String> {
        self.service_tier
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
    }

    fn native_compaction_mode(&self) -> Option<String> {
        Some(self.native_compaction_mode.as_str().to_string())
    }

    fn native_compaction_threshold_tokens(&self) -> Option<usize> {
        (self.native_compaction_mode != OpenAINativeCompactionMode::Off)
            .then_some(self.native_compaction_threshold_tokens)
    }

    fn set_service_tier(&self, service_tier: &str) -> Result<()> {
        let normalized = Self::normalize_service_tier(service_tier)?;
        let mut guard = self
            .service_tier
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = normalized;
        Ok(())
    }

    fn available_service_tiers(&self) -> Vec<&'static str> {
        vec!["priority", "flex"]
    }

    fn transport(&self) -> Option<String> {
        self.transport_mode
            .try_read()
            .ok()
            .map(|g| g.as_str().to_string())
    }

    fn set_transport(&self, transport: &str) -> Result<()> {
        let mode = match transport.trim().to_ascii_lowercase().as_str() {
            "auto" => OpenAITransportMode::Auto,
            "https" | "http" | "sse" => OpenAITransportMode::HTTPS,
            "websocket" | "ws" | "wss" => OpenAITransportMode::WebSocket,
            other => anyhow::bail!(
                "Unknown transport '{}'. Use: auto, https, or websocket.",
                other
            ),
        };
        match self.transport_mode.try_write() {
            Ok(mut guard) => {
                let clears_persistent_chain = matches!(mode, OpenAITransportMode::HTTPS);
                *guard = mode;
                drop(guard);
                if clears_persistent_chain {
                    self.clear_persistent_ws_try(
                        "switching OpenAI transport to HTTPS invalidated the websocket chain",
                    );
                }
                Ok(())
            }
            Err(_) => Err(anyhow::anyhow!(
                "Cannot change transport while a request is in progress"
            )),
        }
    }

    fn available_transports(&self) -> Vec<&'static str> {
        vec!["auto", "https", "websocket"]
    }

    fn supports_compaction(&self) -> bool {
        true
    }

    fn uses_jcode_compaction(&self) -> bool {
        self.native_compaction_mode != OpenAINativeCompactionMode::Auto
    }

    async fn native_compact(
        &self,
        messages: &[ChatMessage],
        existing_summary_text: Option<&str>,
        existing_openai_encrypted_content: Option<&str>,
    ) -> Result<crate::provider::NativeCompactionResult> {
        if self.native_compaction_mode != OpenAINativeCompactionMode::Explicit {
            anyhow::bail!(
                "OpenAI native explicit compaction is disabled (mode={})",
                self.native_compaction_mode.as_str()
            );
        }

        let access_token = openai_access_token(&self.credentials).await?;
        let creds = self.credentials.read().await;
        let is_chatgpt_mode = Self::is_chatgpt_mode(&creds);
        let account_id = creds.account_id.clone();
        let url = Self::responses_compact_url(&creds);
        drop(creds);

        let mut input = Vec::new();
        if let Some(encrypted_content) = existing_openai_encrypted_content {
            if !crate::provider::openai_request::openai_encrypted_content_is_sendable(
                encrypted_content,
            ) {
                anyhow::bail!(
                    "OpenAI native compaction payload is too large to replay ({} chars > safe limit {} chars)",
                    encrypted_content.len(),
                    crate::provider::openai_request::OPENAI_ENCRYPTED_CONTENT_SAFE_MAX_CHARS,
                );
            }
            input.push(serde_json::json!({
                "type": "compaction",
                "encrypted_content": encrypted_content,
            }));
        } else if let Some(summary_text) = existing_summary_text {
            input.push(serde_json::json!({
                "type": "message",
                "role": "user",
                "content": [{
                    "type": "input_text",
                    "text": format!("## Previous Conversation Summary\n\n{}\n", summary_text),
                }]
            }));
        }
        input.extend(build_responses_input(messages));

        let mut builder = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json");

        if is_chatgpt_mode {
            builder = builder.header("originator", ORIGINATOR);
            if let Some(account_id) = account_id.as_ref() {
                builder = builder.header("chatgpt-account-id", account_id);
            }
        }

        let response = builder
            .json(&serde_json::json!({
                "model": self.model_id().await,
                "input": input,
                "store": false,
            }))
            .send()
            .await
            .context("Failed to send OpenAI compact request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = crate::util::http_error_body(response, "HTTP error").await;
            anyhow::bail!("OpenAI compact error {}: {}", status, body);
        }

        let body: Value = response
            .json()
            .await
            .context("Failed to parse OpenAI compact response")?;
        let encrypted_content = body
            .get("output")
            .and_then(|v| v.as_array())
            .and_then(|items| {
                items.iter().find_map(|item| {
                    if item.get("type").and_then(|v| v.as_str()) == Some("compaction") {
                        item.get("encrypted_content")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    } else {
                        None
                    }
                })
            })
            .ok_or_else(|| anyhow::anyhow!("OpenAI compact response missing compaction item"))?;

        if !crate::provider::openai_request::openai_encrypted_content_is_sendable(
            &encrypted_content,
        ) {
            anyhow::bail!(
                "OpenAI compact response returned oversized encrypted_content ({} chars > safe limit {} chars)",
                encrypted_content.len(),
                crate::provider::openai_request::OPENAI_ENCRYPTED_CONTENT_SAFE_MAX_CHARS,
            );
        }

        Ok(crate::provider::NativeCompactionResult {
            summary_text: None,
            openai_encrypted_content: Some(encrypted_content),
        })
    }

    fn context_window(&self) -> usize {
        let model = self.model();
        crate::provider::context_limit_for_model_with_provider(&model, Some(self.name()))
            .unwrap_or(crate::provider::DEFAULT_CONTEXT_LIMIT)
    }

    fn fork(&self) -> Arc<dyn Provider> {
        let model = self.model();
        Arc::new(OpenAIProvider {
            client: self.client.clone(),
            credentials: Arc::clone(&self.credentials),
            model: Arc::new(RwLock::new(model)),
            prompt_cache_key: self.prompt_cache_key.clone(),
            prompt_cache_retention: self.prompt_cache_retention.clone(),
            max_output_tokens: self.max_output_tokens,
            reasoning_effort: Arc::new(StdRwLock::new(self.reasoning_effort())),
            service_tier: Arc::new(StdRwLock::new(self.service_tier())),
            native_compaction_mode: self.native_compaction_mode,
            native_compaction_threshold_tokens: self.native_compaction_threshold_tokens,
            transport_mode: Arc::clone(&self.transport_mode),
            websocket_cooldowns: Arc::clone(&self.websocket_cooldowns),
            websocket_failure_streaks: Arc::clone(&self.websocket_failure_streaks),
            persistent_ws: Arc::new(Mutex::new(None)),
        })
    }

    async fn invalidate_credentials(&self) {
        if let Ok(credentials) = crate::auth::codex::load_credentials() {
            let mut guard = self.credentials.write().await;
            *guard = credentials;
        }

        self.clear_persistent_ws("credentials invalidated").await;
    }
}
