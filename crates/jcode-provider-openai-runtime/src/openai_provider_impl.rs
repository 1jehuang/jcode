use super::openai_stream_runtime::{
    stream_response, stream_response_websocket_persistent, try_persistent_ws_continuation,
};
use super::*;

/// Whether a model catalog fetch error is an auth rejection (401/403) that a
/// token force-refresh may fix, as opposed to a network/server failure.
fn catalog_error_is_auth_rejection(err: &anyhow::Error) -> bool {
    err.downcast_ref::<jcode_base::provider::ModelCatalogHttpStatus>()
        .is_some_and(|status| status.0 == 401 || status.0 == 403)
}

#[async_trait]
impl Provider for OpenAIProvider {
    fn reload_credentials(&self) {
        self.reload_credentials_now();
    }

    fn credential_mode(&self) -> jcode_provider_core::CredentialMode {
        self.credential_mode_snapshot()
    }

    fn set_credential_mode(&self, mode: jcode_provider_core::CredentialMode) -> anyhow::Result<()> {
        OpenAIProvider::set_credential_mode(self, mode)
    }

    async fn complete(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
        system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let selected_model = self.model();
        if is_chatgpt_web_model(&selected_model) {
            return Arc::clone(&self.chatgpt_web)
                .complete(messages, tools, system, &selected_model)
                .await;
        }

        let input = build_responses_input(messages);
        let input_item_count = input.len();
        let api_tools = build_tools(tools);
        let model_id = self.model_id().await;
        let (instructions, is_chatgpt_mode) = {
            let credentials = self.credentials.read().await;
            (system.to_string(), Self::is_chatgpt_mode(&credentials))
        };
        let reasoning_effort = self
            .reasoning_effort
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone());
        // Map the `swarm` sentinel (and any future aliases) to the real effort
        // value the API understands.
        let api_reasoning_effort = self.api_reasoning_effort(reasoning_effort.as_deref());
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
            api_reasoning_effort.as_deref(),
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
        let request_tools = request
            .get("tools")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([]));
        let request_instructions = request.get("instructions").cloned();
        let request_tool_count = request_tools
            .as_array()
            .map(|tools| tools.len())
            .unwrap_or(api_tools.len());
        let canonical_payload = serde_json::json!({
            "model": request.get("model"),
            "instructions": request.get("instructions"),
            "input": &input,
            "tools": request.get("tools"),
            "tool_choice": request.get("tool_choice"),
            "parallel_tool_calls": request.get("parallel_tool_calls"),
            "reasoning": request.get("reasoning"),
            "context_management": request.get("context_management"),
            "include": request.get("include"),
            "service_tier": request.get("service_tier"),
            "prompt_cache_key": request.get("prompt_cache_key"),
            "prompt_cache_retention": request.get("prompt_cache_retention"),
        });
        let prompt_cache_key_hash = request
            .get("prompt_cache_key")
            .map(jcode_provider_core::fingerprint::stable_hash_json);
        jcode_provider_core::fingerprint::log_provider_canonical_input(
            "openai",
            &model_id,
            "openai_responses_full",
            &canonical_payload,
            &input,
            request_instructions.as_ref(),
            Some(&request_tools),
            Some(request_tool_count),
            &[
                (
                    "transport_mode",
                    transport_mode_snapshot.as_str().to_string(),
                ),
                ("websocket_preferred", use_websocket_transport.to_string()),
                ("input_item_count", input_item_count.to_string()),
                ("chatgpt_mode", is_chatgpt_mode.to_string()),
                ("request_kind", "full".to_string()),
                ("cache_namespace", "full_request".to_string()),
                (
                    "prompt_cache_key_present",
                    request.get("prompt_cache_key").is_some().to_string(),
                ),
                (
                    "prompt_cache_key_hash",
                    format!("{:?}", prompt_cache_key_hash),
                ),
                (
                    "prompt_cache_retention",
                    request
                        .get("prompt_cache_retention")
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "null".to_string()),
                ),
                (
                    "service_tier",
                    request
                        .get("service_tier")
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "null".to_string()),
                ),
            ],
        );
        let usage_snapshot = jcode_base::usage::get_openai_usage_sync();
        log_openai_stream_lifecycle(
            jcode_base::logging::LogLevel::Info,
            "request_start",
            vec![
                ("model", model_id.clone()),
                (
                    "transport_mode",
                    transport_mode_snapshot.as_str().to_string(),
                ),
                ("websocket_preferred", use_websocket_transport.to_string()),
                ("input_item_count", input_item_count.to_string()),
                ("tool_count", request_tool_count.to_string()),
                ("chatgpt_mode", is_chatgpt_mode.to_string()),
                ("request_kind", "full".to_string()),
                (
                    "prompt_cache_key_present",
                    request.get("prompt_cache_key").is_some().to_string(),
                ),
                (
                    "prompt_cache_key_hash",
                    format!("{:?}", prompt_cache_key_hash),
                ),
                (
                    "prompt_cache_retention",
                    request
                        .get("prompt_cache_retention")
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "null".to_string()),
                ),
                (
                    "service_tier",
                    request
                        .get("service_tier")
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "null".to_string()),
                ),
            ],
        );
        jcode_base::logging::info(&format!(
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
                    // Track output: a continuation that streams partial output
                    // and then fails falls through to a fresh-connection replay
                    // from the top, which must roll the partial output back.
                    let (attempt_tx, attempt_guard) =
                        jcode_provider_core::attempt_tracker::track_attempt_output(tx.clone());
                    let continuation_result = try_persistent_ws_continuation(
                        &persistent_ws,
                        &request,
                        &input,
                        input_item_count,
                        &attempt_tx,
                    )
                    .await;
                    drop(attempt_tx);
                    let saw_output = attempt_guard.finish().await;

                    match continuation_result {
                        PersistentWsResult::Success => {
                            log_openai_stream_lifecycle(
                                jcode_base::logging::LogLevel::Info,
                                "persistent_reuse_success",
                                vec![
                                    ("model", model_for_transport.clone()),
                                    ("transport", "websocket".to_string()),
                                ],
                            );
                            record_websocket_success(
                                &websocket_cooldowns,
                                &websocket_failure_streaks,
                                &model_for_transport,
                            )
                            .await;
                            return;
                        }
                        PersistentWsResult::NotAvailable => {
                            log_openai_stream_lifecycle(
                                jcode_base::logging::LogLevel::Info,
                                "persistent_reuse_unavailable",
                                vec![
                                    ("model", model_for_transport.clone()),
                                    ("transport", "websocket".to_string()),
                                ],
                            );
                            jcode_base::logging::info(
                                "No persistent WS connection available; using fresh connection",
                            );
                        }
                        PersistentWsResult::Failed(err) => {
                            log_openai_stream_lifecycle(
                                jcode_base::logging::LogLevel::Warn,
                                "persistent_reuse_failed",
                                vec![
                                    ("model", model_for_transport.clone()),
                                    ("transport", "websocket".to_string()),
                                    ("error", err.clone()),
                                ],
                            );
                            jcode_base::logging::warn(&format!(
                                "Persistent WS continuation failed: {}; using fresh connection",
                                err
                            ));
                            if saw_output {
                                // The failed continuation already streamed
                                // partial output; the fresh connection below
                                // replays the response from the top, so roll
                                // the partial output back on the consumer.
                                let _ = tx
                                    .send(Ok(StreamEvent::RetryRollback {
                                        attempt: 1,
                                        max: MAX_RETRIES,
                                    }))
                                    .await;
                            }
                            let mut guard = persistent_ws.lock().await;
                            *guard = None;
                            log_openai_stream_lifecycle(
                                jcode_base::logging::LogLevel::Warn,
                                "persistent_state_reset",
                                vec![
                                    ("model", model_for_transport.clone()),
                                    ("reason", "persistent_reuse_failed".to_string()),
                                ],
                            );
                        }
                    }
                }

                // Normal path: fresh connection with full input (with retry logic)
                let mut last_error = None;
                let mut force_https_for_request = false;
                let mut skip_backoff_once = false;
                let mut next_retry_delay = None;

                for attempt in 0..MAX_RETRIES {
                    if attempt > 0 {
                        emit_connection_phase(
                            &tx,
                            jcode_message_types::ConnectionPhase::Retrying {
                                attempt: attempt + 1,
                                max: MAX_RETRIES,
                            },
                        )
                        .await;
                    }
                    if attempt > 0 && !skip_backoff_once {
                        let delay = jcode_provider_core::retry_after::retry_delay(
                            attempt,
                            RETRY_BASE_DELAY_MS,
                            next_retry_delay.take(),
                        );
                        tokio::time::sleep(delay).await;
                        jcode_base::logging::info(&format!(
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
                                    jcode_base::logging::info(&format!(
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
                    log_openai_stream_lifecycle(
                        jcode_base::logging::LogLevel::Info,
                        "attempt_start",
                        vec![
                            ("model", model_for_transport.clone()),
                            ("attempt", (attempt + 1).to_string()),
                            ("max_attempts", MAX_RETRIES.to_string()),
                            ("transport", transport_label.to_string()),
                            ("transport_mode", transport_mode.as_str().to_string()),
                            ("forced_https", force_https_for_request.to_string()),
                        ],
                    );
                    jcode_base::logging::info(&format!(
                        "OpenAI stream attempt {}/{} using transport '{}'; model='{}'; mode='{}'",
                        attempt + 1,
                        MAX_RETRIES,
                        transport_label,
                        model_for_transport,
                        transport_mode.as_str()
                    ));

                    let use_websocket = matches!(transport, OpenAITransport::WebSocket);
                    // Track whether this attempt streams replay-visible output
                    // so a mid-stream transport fault can roll the partial
                    // output back on the consumer before the retry (or HTTPS
                    // fallback) replays the response from the top.
                    let (attempt_tx, attempt_guard) =
                        jcode_provider_core::attempt_tracker::track_attempt_output(tx.clone());
                    let result = if use_websocket {
                        stream_response_websocket_persistent(
                            Arc::clone(&credentials),
                            request.clone(),
                            attempt_tx,
                            Arc::clone(&persistent_ws),
                            input_item_count,
                        )
                        .await
                    } else {
                        // Retries use a fresh unpooled client: the fault that
                        // broke attempt N (e.g. TLS BadRecordMac from a
                        // corrupting middlebox) may also have poisoned other
                        // idle pooled connections opened through the same
                        // path, so reusing the shared pool can fail
                        // identically. A fresh client guarantees a brand-new
                        // TCP+TLS connection. (Websocket attempts always dial
                        // a new connection already.)
                        let attempt_client = if attempt == 0 {
                            client.clone()
                        } else {
                            jcode_provider_core::fresh_transport_client()
                        };
                        stream_response(
                            attempt_client,
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
                            attempt_tx,
                        )
                        .await
                    };
                    let saw_output = attempt_guard.finish().await;

                    match result {
                        Ok(()) => {
                            log_openai_stream_lifecycle(
                                jcode_base::logging::LogLevel::Info,
                                "attempt_success",
                                vec![
                                    ("model", model_for_transport.clone()),
                                    ("attempt", (attempt + 1).to_string()),
                                    ("transport", transport_label.to_string()),
                                    (
                                        "elapsed_ms",
                                        attempt_started.elapsed().as_millis().to_string(),
                                    ),
                                ],
                            );
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
                            log_openai_stream_lifecycle(
                                jcode_base::logging::LogLevel::Warn,
                                "fallback_to_https",
                                vec![
                                    ("model", model_for_transport.clone()),
                                    ("attempt", (attempt + 1).to_string()),
                                    ("transport", transport_label.to_string()),
                                    ("reason", reason.to_string()),
                                    ("fallback_reason", fallback_reason.summary().to_string()),
                                    ("elapsed_ms", elapsed_ms.to_string()),
                                ],
                            );
                            jcode_base::logging::warn(&format!(
                                "WebSocket fallback after {}ms: {}",
                                elapsed_ms, error
                            ));
                            emit_status_detail(&tx, format!("https fallback: {}", reason)).await;
                            if saw_output {
                                // Partial output already reached the consumer
                                // before the websocket fault; roll it back so
                                // the HTTPS replay renders cleanly instead of
                                // duplicating.
                                let _ = tx
                                    .send(Ok(StreamEvent::RetryRollback {
                                        attempt: attempt + 2,
                                        max: MAX_RETRIES,
                                    }))
                                    .await;
                            }
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
                                jcode_base::logging::warn(&format!(
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
                            log_openai_stream_lifecycle(
                                jcode_base::logging::LogLevel::Warn,
                                "persistent_state_reset",
                                vec![
                                    ("model", model_for_transport.clone()),
                                    ("reason", "fallback_to_https".to_string()),
                                    ("attempt", (attempt + 1).to_string()),
                                ],
                            );
                            last_error = Some(error);
                            continue;
                        }
                        Err(OpenAIStreamFailure::Other(error)) => {
                            let elapsed_ms = attempt_started.elapsed().as_millis();
                            // Full anyhow chain ({:#}) so a send-level transport
                            // cause wrapped behind `.context("Failed to send
                            // request to OpenAI API")` (e.g. TLS BadRecordMac) is
                            // visible to the retry classifier.
                            let error_str = format!("{error:#}").to_lowercase();
                            if is_retryable_error(&error_str) && attempt + 1 < MAX_RETRIES {
                                if saw_output {
                                    // Partial output already reached the
                                    // consumer; roll it back so the retried
                                    // response replays cleanly instead of
                                    // duplicating.
                                    let _ = tx
                                        .send(Ok(StreamEvent::RetryRollback {
                                            attempt: attempt + 2,
                                            max: MAX_RETRIES,
                                        }))
                                        .await;
                                }
                                log_openai_stream_lifecycle(
                                    jcode_base::logging::LogLevel::Warn,
                                    "retry_scheduled",
                                    vec![
                                        ("model", model_for_transport.clone()),
                                        ("attempt", (attempt + 1).to_string()),
                                        ("next_attempt", (attempt + 2).to_string()),
                                        ("transport", transport_label.to_string()),
                                        ("error", error.to_string()),
                                        ("elapsed_ms", elapsed_ms.to_string()),
                                    ],
                                );
                                jcode_base::logging::info(&format!(
                                    "Transient error after {}ms, will retry: {}",
                                    elapsed_ms, error
                                ));
                                next_retry_delay =
                                    jcode_provider_core::retry_after::retry_after_from_error(
                                        &error,
                                    );
                                last_error = Some(error);
                                continue;
                            }
                            log_openai_stream_lifecycle(
                                jcode_base::logging::LogLevel::Error,
                                "attempt_failed",
                                vec![
                                    ("model", model_for_transport.clone()),
                                    ("attempt", (attempt + 1).to_string()),
                                    ("transport", transport_label.to_string()),
                                    ("will_retry", "false".to_string()),
                                    ("error", error.to_string()),
                                    ("elapsed_ms", elapsed_ms.to_string()),
                                ],
                            );
                            let _ = tx.send(Err(error)).await;
                            return;
                        }
                    }
                }

                // All retries exhausted
                if let Some(e) = last_error {
                    log_openai_stream_lifecycle(
                        jcode_base::logging::LogLevel::Error,
                        "retries_exhausted",
                        vec![
                            ("model", model_for_transport.clone()),
                            ("max_attempts", MAX_RETRIES.to_string()),
                            ("error", e.to_string()),
                        ],
                    );
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
                jcode_base::logging::error(&format!(
                    "OpenAI provider stream task panicked: {}",
                    msg
                ));
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
        !is_chatgpt_web_model(&self.model())
    }

    fn set_model(&self, model: &str) -> Result<()> {
        let model = model.trim();
        if self.is_browser_only() && !is_chatgpt_web_model(model) {
            anyhow::bail!(
                "OpenAI API credentials are not available for '{}'. The browser-only runtime can use '{}'; run `jcode login --provider openai` before selecting API models.",
                model,
                CHATGPT_WEB_MODEL,
            );
        }
        if !is_chatgpt_web_model(model)
            && !jcode_base::provider::known_openai_model_ids()
                .iter()
                .any(|known| known == model)
        {
            anyhow::bail!(
                "Unsupported OpenAI model '{}'. Use /model to choose from the models available to your account.",
                model,
            );
        }
        let availability = jcode_base::provider::model_availability_for_account(model);
        if !is_chatgpt_web_model(model)
            && availability.state
                == jcode_base::provider::AccountModelAvailabilityState::Unavailable
        {
            let detail =
                jcode_base::provider::format_account_model_availability_detail(&availability)
                    .unwrap_or_else(|| "not available for your account".to_string());
            anyhow::bail!(
                "The '{}' model is not available for your account right now ({}). \
                 Use /model to see available models.",
                model,
                detail
            );
        }
        // Platform-API-only GPT Pro models cannot run on ChatGPT/Codex OAuth
        // tokens. If the loaded credential is OAuth-shaped, switch to the
        // platform API key now so selecting the model from the picker just
        // works instead of failing at request time with a backend rejection.
        if jcode_provider_core::is_openai_api_only_pro_model(model) {
            let is_chatgpt_shaped = self
                .credentials
                .try_read()
                .map(|creds| Self::is_chatgpt_mode(&creds))
                .unwrap_or(false);
            if is_chatgpt_shaped {
                self.set_credential_mode(OpenAICredentialMode::ApiKey)
                    .map_err(|err| {
                        anyhow::anyhow!(
                            "'{}' is only available on the OpenAI platform API and needs an \
                             OPENAI_API_KEY (ChatGPT/Codex OAuth cannot run it): {}",
                            model,
                            err
                        )
                    })?;
            }
        }
        if let Ok(mut current) = self.model.try_write() {
            let changed = current.as_str() != model;
            *current = model.to_string();
            jcode_base::provider::clear_model_unavailable_for_account(model);
            drop(current);
            if changed {
                self.clear_persistent_ws_try("manual OpenAI model change reset the response chain");
                self.revalidate_reasoning_effort();
            }
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Cannot change model while a request is in progress"
            ))
        }
    }

    fn available_models(&self) -> Vec<&'static str> {
        if self.is_browser_only() {
            return vec![CHATGPT_WEB_MODEL];
        }
        jcode_provider_core::ALL_OPENAI_MODELS.to_vec()
    }

    fn available_models_for_switching(&self) -> Vec<String> {
        if self.is_browser_only() {
            return vec![CHATGPT_WEB_MODEL.to_string()];
        }
        let mut models =
            jcode_base::provider::cached_openai_model_ids().unwrap_or_else(|| vec![self.model()]);
        if !models.iter().any(|model| model == CHATGPT_WEB_MODEL) {
            models.insert(0, CHATGPT_WEB_MODEL.to_string());
        }
        // Platform-API-only GPT Pro models are absent from the Codex OAuth
        // catalog by design; surface them whenever an OPENAI_API_KEY exists.
        if jcode_base::provider::openai_platform_api_key_configured() {
            for pro in jcode_provider_core::OPENAI_API_ONLY_PRO_MODELS {
                if !models.iter().any(|model| model == pro) {
                    models.push((*pro).to_string());
                }
            }
        }
        models
    }

    fn available_models_display(&self) -> Vec<String> {
        self.available_models_for_switching()
    }

    async fn prefetch_models(&self) -> Result<()> {
        if self.is_browser_only() {
            return Ok(());
        }
        // The loaded credential's *shape* is authoritative for which catalog
        // endpoint to hit, not the requested credential mode. In Auto mode a
        // user with only an OPENAI_API_KEY loads an API-key-shaped credential
        // while the mode stays Auto; routing by mode would send that platform
        // key to the ChatGPT/Codex endpoint and get a 401.
        let account_label = jcode_base::auth::codex::active_account_label();
        let (access_token, is_chatgpt_mode, credential_identity) = {
            let creds = self.credentials.read().await;
            (
                creds.access_token.clone(),
                Self::is_chatgpt_mode(&creds),
                Self::catalog_credential_identity(&creds),
            )
        };
        let catalog = if is_chatgpt_mode {
            let access_token = openai_access_token(&self.credentials).await?;
            match jcode_base::provider::fetch_openai_model_catalog(&access_token).await {
                Ok(catalog) => catalog,
                // The server can reject a token that still looks fresh by its
                // local expiry (revoked/rotated). The chat path recovers by
                // force-refreshing; without the same recovery here the model
                // catalog silently stays stale and newly released models never
                // show up until the user happens to re-login (observed as
                // days of bootstrap 401s in the logs).
                Err(err) if catalog_error_is_auth_rejection(&err) => {
                    let refresh_token = {
                        let creds = self.credentials.read().await;
                        creds.refresh_token.clone()
                    };
                    if refresh_token.is_empty() {
                        return Err(err);
                    }
                    jcode_base::logging::info(
                        "OpenAI model catalog fetch rejected the access token; force-refreshing and retrying",
                    );
                    let refreshed = super::openai_stream_runtime::force_refresh_openai_token(
                        &self.credentials,
                        &refresh_token,
                    )
                    .await
                    .map_err(|refresh_err| {
                        err.context(format!(
                            "token force-refresh after catalog 401/403 also failed: {refresh_err:#}"
                        ))
                    })?;
                    jcode_base::provider::fetch_openai_model_catalog(&refreshed).await?
                }
                Err(err) => return Err(err),
            }
        } else {
            jcode_base::provider::fetch_openai_api_key_model_catalog(&access_token).await?
        };
        let current_credential_identity = {
            let credentials = self.credentials.read().await;
            Self::catalog_credential_identity(&credentials)
        };
        if current_credential_identity != credential_identity
            || jcode_base::auth::codex::active_account_label() != account_label
        {
            jcode_base::logging::info(
                "Discarding OpenAI model catalog fetched for credentials that are no longer active",
            );
            return Ok(());
        }
        match self.model_reasoning_efforts.write() {
            Ok(mut efforts) => *efforts = catalog.reasoning_efforts.clone(),
            Err(poisoned) => *poisoned.into_inner() = catalog.reasoning_efforts.clone(),
        }
        self.revalidate_reasoning_effort();
        jcode_base::provider::persist_openai_model_catalog(&catalog);
        if !catalog.context_limits.is_empty() {
            jcode_base::provider::populate_context_limits(catalog.context_limits);
        }
        if !catalog.available_models.is_empty() {
            jcode_base::provider::populate_account_models(catalog.available_models);
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
        let requested = effort.trim().to_ascii_lowercase();
        if !requested.is_empty()
            && jcode_provider_core::canonical_reasoning_effort(&requested).is_none()
            && !jcode_base::prompt::is_swarm_effort(&requested)
        {
            anyhow::bail!(
                "Unsupported OpenAI reasoning effort '{}'; expected none|minimal|low|medium|high|xhigh|max|swarm|swarm-deep",
                effort
            );
        }
        let normalized = Self::normalize_reasoning_effort(effort);
        if let Some(requested) = normalized.as_deref()
            && !jcode_base::prompt::is_swarm_effort(requested)
        {
            let available = self.available_efforts();
            if !available.contains(&requested) {
                anyhow::bail!(
                    "OpenAI reasoning effort '{}' is not supported by model '{}' (available: {})",
                    requested,
                    self.model(),
                    available
                        .into_iter()
                        .filter(|effort| !jcode_base::prompt::is_swarm_effort(effort))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
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
        let model = jcode_provider_core::model_id::canonical(&self.model());
        // Platform-API-only GPT Pro models never appear in the Codex catalog,
        // so their ladders are pinned from live Responses API behavior:
        // gpt-5-pro accepts only `high`; newer pro generations accept
        // medium/high/xhigh.
        if jcode_provider_core::is_openai_api_only_pro_model(&model) {
            return if model.starts_with("gpt-5-pro") {
                vec!["high", "swarm", "swarm-deep"]
            } else {
                vec!["medium", "high", "xhigh", "swarm", "swarm-deep"]
            };
        }
        let advertised = self
            .model_reasoning_efforts
            .read()
            .map(|efforts| efforts.get(&model).cloned())
            .unwrap_or_else(|poisoned| poisoned.into_inner().get(&model).cloned());
        if let Some(advertised) = advertised {
            let mut efforts: Vec<&'static str> = advertised
                .iter()
                .filter_map(|effort| jcode_provider_core::canonical_reasoning_effort(effort))
                .collect();
            efforts.extend(["swarm", "swarm-deep"]);
            return efforts;
        }
        jcode_provider_core::OPENAI_SELECTABLE_EFFORTS.to_vec()
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
        if is_chatgpt_web_model(&self.model()) {
            return Some("browser".to_string());
        }
        self.transport_mode
            .try_read()
            .ok()
            .map(|g| g.as_str().to_string())
    }

    fn set_transport(&self, transport: &str) -> Result<()> {
        if is_chatgpt_web_model(&self.model()) {
            if transport.trim().eq_ignore_ascii_case("browser") {
                return Ok(());
            }
            anyhow::bail!(
                "The '{}' model always uses the browser transport; available transport: browser",
                CHATGPT_WEB_MODEL
            );
        }
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
        if is_chatgpt_web_model(&self.model()) {
            return vec!["browser"];
        }
        vec!["auto", "https", "websocket"]
    }

    fn supports_compaction(&self) -> bool {
        true
    }

    fn uses_jcode_compaction(&self) -> bool {
        is_chatgpt_web_model(&self.model())
            || self.native_compaction_mode != OpenAINativeCompactionMode::Auto
    }

    async fn native_compact(
        &self,
        messages: &[ChatMessage],
        existing_summary_text: Option<&str>,
        existing_openai_encrypted_content: Option<&str>,
    ) -> Result<jcode_provider_core::NativeCompactionResult> {
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
            if !jcode_base::provider::openai_request::openai_encrypted_content_is_sendable(
                encrypted_content,
            ) {
                anyhow::bail!(
                    "OpenAI native compaction payload is too large to replay ({} chars > safe limit {} chars)",
                    encrypted_content.len(),
                    jcode_base::provider::openai_request::OPENAI_ENCRYPTED_CONTENT_SAFE_MAX_CHARS,
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
            let body = jcode_base::util::http_error_body(response, "HTTP error").await;
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

        if !jcode_base::provider::openai_request::openai_encrypted_content_is_sendable(
            &encrypted_content,
        ) {
            anyhow::bail!(
                "OpenAI compact response returned oversized encrypted_content ({} chars > safe limit {} chars)",
                encrypted_content.len(),
                jcode_base::provider::openai_request::OPENAI_ENCRYPTED_CONTENT_SAFE_MAX_CHARS,
            );
        }

        Ok(jcode_provider_core::NativeCompactionResult {
            summary_text: None,
            openai_encrypted_content: Some(encrypted_content),
        })
    }

    fn context_window(&self) -> usize {
        let model = self.model();
        jcode_provider_core::context_limit_for_model_with_provider(&model, Some(self.name()))
            .unwrap_or(jcode_provider_core::DEFAULT_CONTEXT_LIMIT)
    }

    fn fork(&self) -> Arc<dyn Provider> {
        let model = self.model();
        Arc::new(OpenAIProvider {
            client: self.client.clone(),
            credentials: Arc::clone(&self.credentials),
            credential_mode: Arc::clone(&self.credential_mode),
            model: Arc::new(RwLock::new(model)),
            prompt_cache_key: self.prompt_cache_key.clone(),
            prompt_cache_retention: self.prompt_cache_retention.clone(),
            max_output_tokens: self.max_output_tokens,
            reasoning_effort: Arc::new(StdRwLock::new(self.reasoning_effort())),
            model_reasoning_efforts: Arc::clone(&self.model_reasoning_efforts),
            service_tier: Arc::new(StdRwLock::new(self.service_tier())),
            native_compaction_mode: self.native_compaction_mode,
            native_compaction_threshold_tokens: self.native_compaction_threshold_tokens,
            transport_mode: Arc::clone(&self.transport_mode),
            websocket_cooldowns: Arc::clone(&self.websocket_cooldowns),
            websocket_failure_streaks: Arc::clone(&self.websocket_failure_streaks),
            persistent_ws: Arc::new(Mutex::new(None)),
            chatgpt_web: Arc::new(chatgpt_web::ChatGptWebState::new()),
            browser_only: Arc::clone(&self.browser_only),
        })
    }

    async fn invalidate_credentials(&self) {
        let mode = *self.credential_mode.read().await;
        if let Ok(credentials) = super::load_credentials_for_mode(mode) {
            let mut guard = self.credentials.write().await;
            *guard = credentials;
            self.browser_only.store(false, AtomicOrdering::Release);
            drop(guard);
            self.reload_cached_reasoning_efforts();
        }

        self.clear_persistent_ws("credentials invalidated").await;
    }
}
