use super::*;

impl OpenRouterProvider {
    pub(crate) async fn complete_azure_responses(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
    ) -> Result<EventStream> {
        use jcode_provider_openai::request::{
            build_responses_input, build_tools, downgrade_web_search_calls, insert_additional_tools,
        };

        let model = self.model.read().await.clone();
        let mut input = build_responses_input(messages);
        insert_additional_tools(&mut input, messages, tools);
        downgrade_web_search_calls(&mut input);
        let api_tools = build_tools(tools);
        let mut request = serde_json::json!({
            "model": model,
            "instructions": system,
            "input": input,
            "tools": api_tools,
            "stream": true,
            "store": false,
            "include": ["reasoning.encrypted_content"],
        });
        if !api_tools.is_empty() {
            request["tool_choice"] = serde_json::json!("auto");
        }
        if let Some(max_tokens) = self.max_tokens {
            request["max_output_tokens"] = serde_json::json!(max_tokens);
        }
        if let Some(effort) = self.reasoning_effort() {
            let effort =
                jcode_base::prompt::swarm_root_reasoning_effort(&effort).unwrap_or(effort.as_str());
            request["reasoning"] = serde_json::json!({"effort": effort});
        }
        if let Some(extra) = self.extra_body.as_ref()
            && let Some(request_obj) = request.as_object_mut()
        {
            for (key, value) in extra {
                request_obj.insert(key.clone(), value.clone());
            }
        }

        jcode_provider_core::fingerprint::log_provider_canonical_input(
            "azure-openai",
            &model,
            "openai_responses_full",
            &request,
            &input,
            request.get("instructions"),
            request.get("tools"),
            Some(api_tools.len()),
            &[],
        );
        let auth = (self.auth)()?;
        let (tx, rx) = mpsc::channel::<Result<StreamEvent>>(100);
        let client = self.client.clone();
        let api_base = self.api_base.clone();
        let conversation_id = self.conversation_id.clone();
        let provider_pin = Arc::clone(&self.provider_pin);
        tokio::spawn(async move {
            if tx
                .send(Ok(StreamEvent::ConnectionType {
                    connection: "https/sse".to_string(),
                }))
                .await
                .is_err()
            {
                return;
            }
            run_stream_with_retries(
                client,
                api_base,
                auth,
                false,
                conversation_id,
                request,
                true,
                tx,
                provider_pin,
                model,
                false,
            )
            .await;
        });
        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    /// The disk-cache namespace this provider's *foreground* catalog reads and
    /// writes should use.
    ///
    /// Every `new_named_openai_compatible()` constructor sets the process-global
    /// `JCODE_OPENROUTER_CACHE_NAMESPACE` env var, so with several named
    /// profiles in one process the last one constructed wins and all profiles
    /// collide on a single `<last-profile>_models.json`. The background refresh
    /// path already passes an explicit namespace; the foreground paths did not.
    /// See issue #607.
    ///
    /// Standard/direct OpenRouter and built-in profiles keep the existing
    /// env-var-driven `cache_path()` semantics.
    pub(crate) fn foreground_cache_namespace(&self) -> Option<String> {
        (self.is_user_named_profile() || self.profile_id.as_deref() == Some("grok-build"))
            .then(|| self.profile_id.clone())
            .flatten()
    }

    /// The disk cache entry usable for this provider, i.e. its own namespace
    /// (#607) and a `source_api_base` that matches this endpoint.
    pub(crate) fn load_usable_model_disk_cache_entry(
        &self,
    ) -> Option<jcode_provider_openrouter::DiskCache> {
        self.load_disk_cache_entry_for_this_profile()
            .filter(|entry| self.model_disk_cache_source_matches(entry))
    }

    /// Load this provider's own model disk cache, ignoring the process-global
    /// namespace env var for user-named profiles (#607).
    pub(crate) fn load_disk_cache_entry_for_this_profile(
        &self,
    ) -> Option<jcode_provider_openrouter::DiskCache> {
        match self.foreground_cache_namespace() {
            Some(namespace) => {
                jcode_provider_openrouter::load_disk_cache_entry_for_namespace(&namespace)
            }
            None => jcode_provider_openrouter::load_disk_cache_entry(),
        }
    }

    /// True when this instance was built from a user-declared
    /// `[providers.<name>]` profile in config.toml rather than a built-in
    /// OpenAI-compatible profile (Cerebras, NVIDIA NIM, ...).
    pub(crate) fn is_user_named_profile(&self) -> bool {
        let Some(id) = self.profile_id.as_deref() else {
            return false;
        };
        if self.builtin_azure {
            return false;
        }
        match jcode_base::provider_catalog::openai_compatible_profile_by_id(id) {
            // A `[providers.<name>]` block that shadows a built-in profile name
            // but points somewhere else is still a user-declared endpoint, so
            // its explicit model list must be preserved.
            Some(builtin) => normalize_api_base(builtin.api_base)
                .is_some_and(|builtin_base| builtin_base != self.api_base),
            None => true,
        }
    }

    pub(crate) fn should_merge_static_models_with_live_catalog(&self) -> bool {
        // Built-in OpenAI-compatible provider profiles use `static_models` as a
        // startup/pre-catalog fallback so `/model` is useful immediately after
        // login. Once a live `/models` catalog has been fetched, the live catalog
        // is more authoritative for access control. Keeping built-in fallback
        // entries after a successful fetch can advertise preview/stale models that
        // the provider rejects at chat time, which is especially confusing for
        // direct providers such as Cerebras.
        //
        // Preserve static models for OpenRouter itself and for custom/named
        // profiles, where the user supplied the list explicitly and there may be
        // no provider-side catalog contract. A profile id that is not in the
        // built-in OpenAI-compatible catalog comes from a user-declared
        // `[providers.<name>]` block in config.toml, so its
        // `[[providers.<name>.models]]` entries must survive background
        // `/models` catalog refreshes (issue #579).
        self.supports_provider_features || self.profile_id.is_none() || self.is_user_named_profile()
    }

    /// Whether the model catalog declares `image` as an accepted input modality
    /// for this model.
    ///
    /// Memory decides whenever it holds an opinion about the model, so a freshly
    /// fetched catalog is never overruled by a stale copy on disk. Only when
    /// memory is silent does the persisted catalog answer, and that fallback is
    /// what makes the first request after startup behave: the in-memory cache is
    /// initialised empty while the catalog is normally already on disk from the
    /// previous run, so memory alone would keep clamping images until the first
    /// refresh completed.
    ///
    /// `supports_image_input` is a sync trait method, so the in-memory read uses
    /// `try_read` rather than awaiting the tokio lock; a busy lock defers to the
    /// disk copy instead of blocking.
    pub(crate) fn catalog_declares_image_input(&self, model_id: &str) -> bool {
        if !self.supports_model_catalog {
            return false;
        }
        if let Ok(cache) = self.models_cache.try_read()
            && let Some(model) = cache
                .models
                .iter()
                .find(|model| model.id.trim().eq_ignore_ascii_case(model_id))
        {
            return declares_image_input(model);
        }
        self.load_usable_model_disk_cache_entry()
            .is_some_and(|entry| {
                entry
                    .models
                    .iter()
                    .find(|model| model.id.trim().eq_ignore_ascii_case(model_id))
                    .is_some_and(declares_image_input)
            })
    }
}

/// Whether one catalog entry declares `image` as an accepted input modality.
pub(crate) fn declares_image_input(model: &jcode_provider_openrouter::ModelInfo) -> bool {
    model
        .input
        .iter()
        .any(|modality| modality.eq_ignore_ascii_case("image"))
}

#[cfg(test)]
#[path = "openrouter_canonical_input_tests.rs"]
mod canonical_input_tests;

/// Append the swarm sentinels a ladder is missing. The shared constants
/// predate `swarm-deep`, and the client-side ladder advertises both, so the
/// runtime ladder must match or `set_reasoning_effort` rejects what the UI
/// offered.
pub(crate) fn append_swarm_sentinels(mut efforts: Vec<&'static str>) -> Vec<&'static str> {
    for sentinel in jcode_provider_core::reasoning::SWARM_EFFORTS {
        if !efforts.contains(sentinel) {
            efforts.push(sentinel);
        }
    }
    efforts
}
