use super::openrouter_sse_stream::run_stream_with_retries;
use super::*;
use crate::provider::{ModelCatalogRefreshSummary, summarize_model_catalog_refresh};

#[async_trait]
impl Provider for OpenRouterProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let model = self.model.read().await.clone();
        let reasoning_effort = self.reasoning_effort();
        let thinking_override = Self::thinking_override();
        // Moonshot's dedicated Kimi coding endpoint enables thinking server-side
        // by default and rejects any assistant tool-call message that lacks
        // `reasoning_content`, even though its model id (`kimi-for-coding`) is
        // not a moonshotai/kimi-k2 model and the profile runs without OpenRouter
        // provider features (issue #322). We must attach `reasoning_content` to
        // those messages, but must NOT add the OpenRouter-specific top-level
        // `thinking` field (the endpoint already manages thinking itself), so
        // this is kept separate from `thinking_enabled`.
        let kimi_coding_endpoint = self.is_kimi_coding_endpoint(&model);
        let thinking_enabled = thinking_override.or_else(|| {
            if Self::is_kimi_model(&model) {
                Some(true)
            } else {
                None
            }
        });
        let allow_reasoning = (self.supports_provider_features || kimi_coding_endpoint)
            && thinking_enabled != Some(false);
        let include_reasoning_content = thinking_enabled == Some(true)
            || (allow_reasoning && Self::is_kimi_model(&model))
            || kimi_coding_endpoint;

        // Some OpenAI-compatible providers (e.g. Mistral) strictly enforce the
        // OpenAI schema and reject the non-standard `reasoning_content` message
        // field and top-level `thinking` request field with a 422 error
        // ("Extra inputs are not permitted"). Suppress both for those endpoints
        // regardless of any thinking override (issue #261).
        let strict_openai_schema =
            Self::strict_openai_schema_endpoint(self.profile_id.as_deref(), &self.api_base);
        let allow_reasoning = allow_reasoning && !strict_openai_schema;
        let include_reasoning_content = include_reasoning_content && !strict_openai_schema;
        let allow_image_input = self.supports_image_input();

        let mut effective_messages: Vec<Message> = messages.to_vec();
        let cache_supported = self.model_supports_cache(&model).await;
        let cache_control_added = if cache_supported {
            add_cache_breakpoint(&mut effective_messages)
        } else {
            false
        };

        let api_messages = jcode_provider_openrouter::request::build_chat_messages(
            &effective_messages,
            system,
            allow_reasoning,
            include_reasoning_content,
            allow_image_input,
        );

        // Build tools in OpenAI format
        let api_tools: Vec<Value> = tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        // Prompt-visible. Approximate token cost for this field:
                        // t.description_token_estimate().
                        "description": t.description,
                        "parameters": t.input_schema,
                    }
                })
            })
            .collect();

        // Build request
        let mut request = serde_json::json!({
            "model": model,
            "messages": api_messages,
            "stream": true,
        });

        if let Some(max_tokens) = self.max_tokens {
            request["max_tokens"] = serde_json::json!(max_tokens);
        }

        let mut sent_reasoning_config = false;
        if let Some(effort) = reasoning_effort.as_deref() {
            if Self::profile_supports_reasoning_effort(self.profile_id.as_deref()) {
                if effort != "none" {
                    request["reasoning_effort"] = serde_json::json!(effort);
                    sent_reasoning_config = true;
                }
            } else if Self::profile_supports_unified_reasoning(
                self.profile_id.as_deref(),
                self.send_openrouter_headers,
            ) {
                request["reasoning"] = serde_json::json!({
                    "effort": effort,
                });
                sent_reasoning_config = true;
            }
        }

        if !api_tools.is_empty() {
            request["tools"] = serde_json::json!(api_tools);
            if self.profile_id.as_deref() != Some("fpt") && !self.api_base.contains("fptcloud.com")
            {
                request["tool_choice"] = serde_json::json!("auto");
            }
        }

        // Optional thinking override for OpenRouter (provider-specific).
        // Skip for strict OpenAI-schema endpoints (e.g. Mistral) which reject
        // the non-standard top-level `thinking` field with a 422 (issue #261).
        if let Some(enable) = thinking_enabled
            && !sent_reasoning_config
            && !strict_openai_schema
        {
            request["thinking"] = serde_json::json!({
                "type": if enable { "enabled" } else { "disabled" }
            });
        }

        // Add provider routing if configured and supported by backend.
        let mut provider_obj = None;
        if self.supports_provider_features {
            let routing = self.effective_routing(&model).await;
            if !routing.is_empty() {
                let mut obj = serde_json::json!({});
                if let Some(ref order) = routing.order {
                    obj["order"] = serde_json::json!(order);
                }
                if !routing.allow_fallbacks {
                    obj["allow_fallbacks"] = serde_json::json!(false);
                }
                if let Some(ref sort) = routing.sort {
                    obj["sort"] = serde_json::json!(sort);
                }
                if let Some(min_tp) = routing.preferred_min_throughput {
                    obj["preferred_min_throughput"] = serde_json::json!(min_tp);
                }
                if let Some(max_latency) = routing.preferred_max_latency {
                    obj["preferred_max_latency"] = serde_json::json!(max_latency);
                }
                if let Some(max_price) = routing.max_price {
                    obj["max_price"] = serde_json::json!(max_price);
                }
                if let Some(require_parameters) = routing.require_parameters {
                    obj["require_parameters"] = serde_json::json!(require_parameters);
                }
                provider_obj = Some(obj);
            }
        }

        if cache_control_added && self.supports_provider_features {
            let mut obj = provider_obj.unwrap_or_else(|| serde_json::json!({}));
            obj["require_parameters"] = serde_json::json!(true);
            provider_obj = Some(obj);
        }

        if let Some(obj) = provider_obj {
            request["provider"] = obj;
        }

        let message_items = request
            .get("messages")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let tools_value = request.get("tools").cloned();
        let system_value = message_items
            .first()
            .filter(|message| message.get("role").and_then(|role| role.as_str()) == Some("system"))
            .cloned();
        let tool_count = tools_value
            .as_ref()
            .and_then(|value| value.as_array())
            .map(|tools| tools.len())
            .unwrap_or(0);
        crate::provider::fingerprint::log_provider_canonical_input(
            if self.supports_provider_features {
                "openrouter"
            } else {
                "openai-compatible"
            },
            &model,
            "chat_completions",
            &request,
            &message_items,
            system_value.as_ref(),
            tools_value.as_ref(),
            Some(tool_count),
            &[
                ("cache_supported", cache_supported.to_string()),
                ("cache_control_added", cache_control_added.to_string()),
                ("thinking_enabled", format!("{:?}", thinking_enabled)),
                (
                    "provider_features",
                    self.supports_provider_features.to_string(),
                ),
            ],
        );

        // OpenRouter uses HTTPS/SSE transport only
        crate::logging::info("OpenRouter transport: HTTPS (SSE)");

        let (tx, rx) = mpsc::channel::<Result<StreamEvent>>(100);
        let client = self.client.clone();
        let api_base = self.api_base.clone();
        let auth = self.auth.clone();
        let send_openrouter_headers = self.send_openrouter_headers;
        let request_for_retries = request;
        let model_for_stream = model.clone();
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
                send_openrouter_headers,
                request_for_retries,
                tx,
                provider_pin,
                model_for_stream,
            )
            .await;
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    fn name(&self) -> &str {
        "openrouter"
    }

    fn display_name(&self) -> String {
        self.runtime_display_name()
    }

    fn model(&self) -> String {
        self.model
            .try_read()
            .map(|m| m.clone())
            .unwrap_or_else(|_| DEFAULT_MODEL.to_string())
    }

    fn supports_image_input(&self) -> bool {
        if Self::profile_rejects_image_input(self.profile_id.as_deref()) {
            return false;
        }

        // Direct OpenAI-compatible local providers such as Ollama and LM Studio
        // document image content support on /v1/chat/completions. We already
        // serialize image blocks using OpenAI's image_url content-part shape in
        // complete(), so advertise support for direct compatibility profiles.
        // Keep the legacy OpenRouter aggregator behavior unchanged here because
        // image availability is model/provider-route dependent there.
        !self.supports_provider_features
    }

    fn set_model(&self, model: &str) -> Result<()> {
        // OpenRouter accepts any model ID - validation happens at API call time
        // This allows using any model without needing to pre-fetch the list
        let trimmed = model.trim();
        if trimmed.is_empty() {
            anyhow::bail!("OpenRouter/OpenAI-compatible model cannot be empty");
        }

        let (model_id, provider) = if self.supports_provider_features {
            let (model_id, provider) = parse_model_spec(trimmed);
            let model_id = if provider.is_some() {
                crate::provider::openrouter_catalog_model_id(&model_id).unwrap_or(model_id)
            } else {
                model_id
            };
            (model_id, provider)
        } else {
            // Generic OpenAI-compatible backends often use arbitrary model IDs.
            // Only real OpenRouter supports the model@provider pin syntax, so
            // preserve the caller's model string exactly for custom endpoints.
            (trimmed.to_string(), None)
        };
        if let Some(profile_id) = self.profile_id.as_deref()
            && !crate::provider_catalog::openai_compatible_profile_model_supports_chat(
                profile_id, &model_id,
            )
        {
            anyhow::bail!(
                "Model '{}' is listed by the provider catalog but is not currently usable for chat completions through this direct provider. Choose another model from `/model`.",
                model_id
            );
        }
        if let Ok(mut current) = self.model.try_write() {
            *current = model_id.clone();
        } else {
            return Err(anyhow::anyhow!(
                "Cannot change model while a request is in progress"
            ));
        }

        if self.supports_provider_features {
            if let Some(provider) = provider {
                self.set_explicit_pin(&model_id, provider);
            } else {
                self.clear_pin_if_model_changed(&model_id, true);
            }
        } else {
            self.clear_pin_if_model_changed(&model_id, true);
        }

        Ok(())
    }

    fn reasoning_effort(&self) -> Option<String> {
        if !Self::supports_reasoning_effort_for_profile(
            self.profile_id.as_deref(),
            self.send_openrouter_headers,
        ) {
            return None;
        }
        self.reasoning_effort
            .try_read()
            .ok()
            .and_then(|effort| effort.clone())
    }

    fn set_reasoning_effort(&self, effort: &str) -> Result<()> {
        if !Self::supports_reasoning_effort_for_profile(
            self.profile_id.as_deref(),
            self.send_openrouter_headers,
        ) {
            anyhow::bail!(
                "Reasoning effort is only supported for OpenRouter and DeepSeek direct profiles"
            );
        }
        let normalized =
            Self::normalize_reasoning_effort_for_profile(self.profile_id.as_deref(), effort);
        let mut current = self.reasoning_effort.try_write().map_err(|_| {
            anyhow::anyhow!("Cannot change reasoning effort while a request is in progress")
        })?;
        *current = normalized;
        Ok(())
    }

    fn available_efforts(&self) -> Vec<&'static str> {
        if Self::profile_supports_reasoning_effort(self.profile_id.as_deref()) {
            vec!["none", "low", "medium", "high", "max"]
        } else if Self::profile_supports_unified_reasoning(
            self.profile_id.as_deref(),
            self.send_openrouter_headers,
        ) {
            vec!["none", "low", "medium", "high", "xhigh"]
        } else {
            vec![]
        }
    }

    fn available_models(&self) -> Vec<&'static str> {
        // OpenRouter models are fetched dynamically from the API.
        // Static list is empty; use available_models_display for cached list.
        vec![]
    }

    fn available_models_display(&self) -> Vec<String> {
        let finalize = |models: Vec<String>| self.filter_profile_chat_supported_models(models);
        let with_current_model = |mut models: Vec<String>| {
            let current = self.model();
            if !current.trim().is_empty() && !models.iter().any(|model| model == &current) {
                models.insert(0, current);
            }
            models
        };

        let should_merge_static_models = self.should_merge_static_models_with_live_catalog();
        let merge_static_models = |mut models: Vec<String>| {
            if !should_merge_static_models {
                return with_current_model(models);
            }
            for model in &self.static_models {
                if !model.trim().is_empty() && !models.iter().any(|existing| existing == model) {
                    models.push(model.clone());
                }
            }
            with_current_model(models)
        };

        if !self.supports_model_catalog {
            if !self.static_models.is_empty() {
                return finalize(with_current_model(self.static_models.clone()));
            }
            let model = self.model();
            return finalize(if model.trim().is_empty() {
                Vec::new()
            } else {
                vec![model]
            });
        }

        if let Ok(cache) = self.models_cache.try_read()
            && cache.fetched
            && !cache.models.is_empty()
        {
            if let Some(cache_age) = cache
                .cached_at
                .and_then(|cached_at| current_unix_secs().map(|now| now.saturating_sub(cached_at)))
            {
                self.maybe_schedule_model_catalog_refresh(cache_age, "display memory cache");
            }
            return finalize(merge_static_models(
                cache.models.iter().map(|m| m.id.clone()).collect(),
            ));
        }

        if let Some(cache_entry) = self.load_usable_model_disk_cache_entry() {
            let cache_age = current_unix_secs()
                .map(|now| now.saturating_sub(cache_entry.cached_at))
                .unwrap_or(0);
            if let Ok(mut cache) = self.models_cache.try_write() {
                cache.models = cache_entry.models.clone();
                cache.fetched = true;
                cache.cached_at = Some(cache_entry.cached_at);
            }
            self.maybe_schedule_model_catalog_refresh(cache_age, "display disk cache");
            return finalize(merge_static_models(
                cache_entry.models.into_iter().map(|m| m.id).collect(),
            ));
        }

        // No memory or disk catalog yet. This commonly happens immediately after
        // adding a new OpenAI-compatible endpoint from `/login`: the provider is
        // hot-initialized, but the picker may render before the post-auth
        // prefetch has completed. Make the picker path self-healing by starting
        // the first `/models` fetch here, then return the best immediate
        // fallback. The background refresh publishes ModelsUpdated, which
        // invalidates/reopens the picker with the newly discovered models.
        self.maybe_schedule_model_catalog_refresh(u64::MAX, "display cache miss");

        if !self.static_models.is_empty() {
            return finalize(with_current_model(self.static_models.clone()));
        }

        let model = self.model();
        finalize(if model.trim().is_empty() {
            Vec::new()
        } else {
            vec![model]
        })
    }

    fn available_models_for_switching(&self) -> Vec<String> {
        self.available_models_display()
    }

    fn model_routes(&self) -> Vec<crate::provider::ModelRoute> {
        let (provider_label, api_method, detail) = self
            .direct_openai_compatible_route_parts()
            .unwrap_or_else(|| {
                (
                    "OpenRouter".to_string(),
                    "openrouter".to_string(),
                    String::new(),
                )
            });
        let live_model_ids = self.cached_live_model_ids_for_display();
        let static_model_ids: HashSet<String> = self.static_models.iter().cloned().collect();
        let is_direct_profile = self.profile_id.is_some();

        self.available_models_display()
            .into_iter()
            .filter(|model| crate::provider::is_listable_model_name(model))
            .map(|model| {
                let fallback_not_live = is_direct_profile
                    && live_model_ids
                        .as_ref()
                        .map(|live| !live.contains(&model))
                        .unwrap_or_else(|| static_model_ids.contains(&model));
                let route_detail = if fallback_not_live {
                    if detail.trim().is_empty() {
                        "fallback: static provider model list".to_string()
                    } else {
                        format!("{}; fallback: static provider model list", detail)
                    }
                } else {
                    detail.clone()
                };
                crate::provider::ModelRoute {
                    model,
                    provider: provider_label.clone(),
                    api_method: api_method.clone(),
                    available: true,
                    detail: route_detail,
                    cheapness: None,
                }
            })
            .collect()
    }

    async fn prefetch_models(&self) -> Result<()> {
        if !self.supports_model_catalog {
            return Ok(());
        }

        let _ = self.fetch_models().await?;
        if self.supports_provider_features {
            // Also prefetch endpoints for the current model so preferred_provider() works immediately.
            let model = self.model();
            if load_endpoints_disk_cache(&model).is_none() {
                let _ = self.fetch_endpoints(&model).await;
            }
        }
        Ok(())
    }

    async fn refresh_model_catalog(&self) -> Result<ModelCatalogRefreshSummary> {
        let before_models = self.available_models_display();
        let before_routes = self.model_routes();

        let refreshed_models = self.refresh_models().await?;

        if self.supports_provider_features {
            let mut targets = Vec::new();
            let mut seen = HashSet::new();
            let push_target =
                |targets: &mut Vec<String>, seen: &mut HashSet<String>, model: String| {
                    if !model.trim().is_empty() && seen.insert(model.clone()) {
                        targets.push(model);
                    }
                };

            push_target(&mut targets, &mut seen, self.model());

            for model in refreshed_models.iter().map(|info| info.id.clone()).take(16) {
                push_target(&mut targets, &mut seen, model);
            }

            for model in refreshed_models.iter().map(|info| info.id.clone()) {
                if load_endpoints_disk_cache_public(&model).is_some() {
                    push_target(&mut targets, &mut seen, model);
                }
                if targets.len() >= 24 {
                    break;
                }
            }

            futures::stream::iter(targets)
                .for_each_concurrent(4, |model| async move {
                    let _ = self.refresh_endpoints(&model).await;
                })
                .await;
        }

        let after_models = self.available_models_display();
        let after_routes = self.model_routes();
        Ok(summarize_model_catalog_refresh(
            before_models,
            after_models,
            before_routes,
            after_routes,
        ))
    }

    fn supports_compaction(&self) -> bool {
        true
    }

    fn preferred_provider(&self) -> Option<String> {
        self.preferred_provider()
    }

    fn context_window(&self) -> usize {
        let model_id = self.model();
        // Try cached model data from OpenRouter API
        let cache = self.models_cache.try_read();
        if let Ok(cache) = cache
            && let Some(model) = cache.models.iter().find(|m| m.id == model_id)
            && let Some(ctx) = model.context_length
        {
            return ctx as usize;
        }
        // A background/profile catalog refresh may have already persisted live
        // /models metadata before this provider instance has hydrated its
        // in-memory cache. Use that live catalog context length before falling
        // back to static defaults.
        if let Some(cache_entry) = self.load_usable_model_disk_cache_entry()
            && let Some(model) = cache_entry.models.iter().find(|m| m.id == model_id)
            && let Some(ctx) = model.context_length
        {
            return ctx as usize;
        }
        let normalized_model_id = model_id.trim().to_ascii_lowercase();
        if let Some(limit) = self.static_context_limits.get(&normalized_model_id) {
            return *limit;
        }
        if let Some(profile_id) = self.profile_id.as_deref()
            && let Some(limit) = crate::provider_catalog::openai_compatible_profile_context_limit(
                profile_id, &model_id,
            )
        {
            return limit;
        }
        crate::provider::context_limit_for_model_with_provider(&model_id, Some(self.name()))
            .unwrap_or(crate::provider::DEFAULT_CONTEXT_LIMIT)
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            client: self.client.clone(),
            model: Arc::new(RwLock::new(
                self.model.try_read().map(|m| m.clone()).unwrap_or_default(),
            )),
            reasoning_effort: Arc::new(RwLock::new(self.reasoning_effort())),
            api_base: self.api_base.clone(),
            auth: self.auth.clone(),
            supports_provider_features: self.supports_provider_features,
            supports_model_catalog: self.supports_model_catalog,
            profile_id: self.profile_id.clone(),
            max_tokens: self.max_tokens,
            static_models: self.static_models.clone(),
            static_context_limits: self.static_context_limits.clone(),
            send_openrouter_headers: self.send_openrouter_headers,
            models_cache: Arc::clone(&self.models_cache),
            model_catalog_refresh: Arc::clone(&self.model_catalog_refresh),
            provider_routing: Arc::new(RwLock::new(
                self.provider_routing
                    .try_read()
                    .map(|r| r.clone())
                    .unwrap_or_default(),
            )),
            provider_pin: Arc::new(Mutex::new(None)),
            endpoints_cache: Arc::clone(&self.endpoints_cache),
            endpoint_refresh: Arc::clone(&self.endpoint_refresh),
        })
    }
}
