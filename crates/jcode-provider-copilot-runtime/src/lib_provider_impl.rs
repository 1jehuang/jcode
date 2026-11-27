use super::*;

#[async_trait]
impl Provider for CopilotApiProvider {
    async fn complete(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDefinition],
        system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        self.wait_for_init().await;

        self.get_bearer_token().await.map_err(|e| {
            jcode_base::logging::warn(&format!(
                "Copilot bearer token acquisition failed (will trigger fallback): {}",
                e
            ));
            e
        })?;

        let is_user_initiated = self.is_user_initiated(messages);
        if is_user_initiated {
            self.user_turn_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        let model_for_fingerprint = self.model();
        let uses_responses_api = copilot_model_uses_responses_api(&model_for_fingerprint);
        let canonical_payload = self.canonical_payload(
            &model_for_fingerprint,
            uses_responses_api,
            messages,
            tools,
            system,
        );
        let (fingerprint_input, system_value, tools_value) =
            canonical_diagnostic_parts(&canonical_payload, uses_responses_api);
        jcode_provider_core::fingerprint::log_provider_canonical_input(
            "copilot",
            &model_for_fingerprint,
            if uses_responses_api {
                "responses"
            } else {
                "chat_completions"
            },
            &canonical_payload,
            fingerprint_input,
            system_value,
            tools_value,
            Some(tools_value.and_then(Value::as_array).map_or(0, Vec::len)),
            &[("user_initiated", is_user_initiated.to_string())],
        );

        let (tx, rx) = mpsc::channel::<Result<StreamEvent>>(100);

        let provider = CopilotApiProvider {
            client: self.client.clone(),
            model: self.model.clone(),
            github_token: self.github_token.clone(),
            bearer_token: self.bearer_token.clone(),
            fetched_models: self.fetched_models.clone(),
            catalog_source: self.catalog_source.clone(),
            session_id: self.session_id.clone(),
            machine_id: self.machine_id.clone(),
            init_ready: self.init_ready.clone(),
            init_done: self.init_done.clone(),
            premium_mode: self.premium_mode.clone(),
            user_turn_count: self.user_turn_count.clone(),
            reasoning_effort: self.reasoning_effort.clone(),
            model_efforts: self.model_efforts.clone(),
            created_at: self.created_at,
        };

        tokio::spawn(async move {
            provider
                .stream_request(canonical_payload, uses_responses_api, is_user_initiated, tx)
                .await;
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    fn name(&self) -> &str {
        "copilot"
    }

    fn model(&self) -> String {
        self.model
            .try_read()
            .map(|m| m.clone())
            .unwrap_or_else(|_| DEFAULT_MODEL.to_string())
    }

    fn set_model(&self, model: &str) -> Result<()> {
        // See `strip_own_model_prefix`: `--provider copilot` routes through this
        // runtime directly, so session restore hands it `copilot:<model>`.
        let trimmed = jcode_provider_core::strip_own_model_prefix(model, "copilot:");
        if trimmed.is_empty() {
            anyhow::bail!("Copilot model cannot be empty");
        }
        if trimmed.contains("[1m]") {
            anyhow::bail!(
                "1M context window models are not supported via Copilot. Use the Anthropic API directly."
            );
        }
        if let Ok(mut current) = self.model.try_write() {
            *current = trimmed.to_string();
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Cannot change model while a request is in progress"
            ))
        }
    }

    fn available_models(&self) -> Vec<&'static str> {
        FALLBACK_MODELS.to_vec()
    }

    fn available_models_display(&self) -> Vec<String> {
        if let Ok(models) = self.fetched_models.read()
            && !models.is_empty()
        {
            return models.clone();
        }
        FALLBACK_MODELS
            .iter()
            .map(|model| model.to_string())
            .collect()
    }

    fn available_models_for_switching(&self) -> Vec<String> {
        self.available_models_display()
    }

    async fn prefetch_models(&self) -> Result<()> {
        let grace_ms = Self::startup_prefetch_grace_ms();
        if self.created_at.elapsed().as_millis() < u128::from(grace_ms) {
            jcode_base::logging::info(&format!(
                "Skipping Copilot model prefetch during startup grace window ({}ms)",
                grace_ms
            ));
            return Ok(());
        }
        let result = self.refresh_live_catalog().await;
        self.mark_init_done();
        result
    }

    async fn refresh_model_catalog(
        &self,
    ) -> Result<jcode_provider_core::ModelCatalogRefreshSummary> {
        let before_models = self.available_models_display();
        let before_routes = self.model_routes();
        // An explicit refresh must not be skipped by the startup grace window.
        let result = self.refresh_live_catalog().await;
        self.mark_init_done();
        result?;
        Ok(jcode_provider_core::summarize_model_catalog_refresh(
            before_models,
            self.available_models_display(),
            before_routes,
            self.model_routes(),
        ))
    }

    fn supports_compaction(&self) -> bool {
        true
    }

    fn model_catalog_detail(&self) -> String {
        self.model_catalog_detail_impl()
    }

    fn set_premium_mode(&self, mode: PremiumMode) {
        CopilotApiProvider::set_premium_mode(self, mode);
    }

    fn premium_mode(&self) -> PremiumMode {
        CopilotApiProvider::get_premium_mode(self)
    }

    fn context_window(&self) -> usize {
        jcode_provider_core::context_limit_for_model_with_provider(&self.model(), Some(self.name()))
            .unwrap_or(128_000)
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(CopilotApiProvider {
            client: self.client.clone(),
            model: Arc::new(RwLock::new(self.model())),
            github_token: self.github_token.clone(),
            bearer_token: self.bearer_token.clone(),
            fetched_models: self.fetched_models.clone(),
            catalog_source: self.catalog_source.clone(),
            session_id: self.session_id.clone(),
            machine_id: self.machine_id.clone(),
            init_ready: self.init_ready.clone(),
            init_done: self.init_done.clone(),
            premium_mode: self.premium_mode.clone(),
            user_turn_count: self.user_turn_count.clone(),
            reasoning_effort: self.reasoning_effort.clone(),
            model_efforts: self.model_efforts.clone(),
            created_at: self.created_at,
        })
    }

    fn reasoning_effort(&self) -> Option<String> {
        self.effective_reasoning_effort(&self.model())
    }

    fn set_reasoning_effort(&self, effort: &str) -> Result<()> {
        let model = self.model();
        let supported = self.efforts_for(&model);
        if supported.is_empty() {
            anyhow::bail!(
                "Reasoning effort is not supported for Copilot model '{}'",
                model
            );
        }
        let normalized = effort.trim().to_lowercase();
        if !supported.contains(&normalized) {
            anyhow::bail!(
                "Unsupported reasoning effort '{}' for Copilot model '{}'. Supported: {}",
                effort,
                model,
                supported.join(", ")
            );
        }
        let mut guard = self
            .reasoning_effort
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = Some(normalized);
        Ok(())
    }

    fn available_efforts(&self) -> Vec<&'static str> {
        const KNOWN: [&str; 7] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];
        let supported = self.efforts_for(&self.model());
        KNOWN
            .into_iter()
            .filter(|e| supported.iter().any(|s| s == e))
            .collect()
    }
}
