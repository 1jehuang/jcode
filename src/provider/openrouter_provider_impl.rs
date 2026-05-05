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
        let thinking_override = Self::thinking_override();
        let thinking_enabled = thinking_override.or_else(|| {
            if Self::is_kimi_model(&model) {
                Some(true)
            } else {
                None
            }
        });
        let allow_reasoning = thinking_enabled != Some(false);
        let include_reasoning_content =
            thinking_enabled == Some(true) || (allow_reasoning && Self::is_kimi_model(&model));

        let mut effective_messages: Vec<Message> = messages.to_vec();
        let cache_supported = self.model_supports_cache(&model).await;
        let cache_control_added = if cache_supported {
            add_cache_breakpoint(&mut effective_messages)
        } else {
            false
        };

        // Build messages in OpenAI format
        let mut api_messages = Vec::new();

        // Add system message if provided
        if !system.is_empty() {
            api_messages.push(serde_json::json!({
                "role": "system",
                "content": system
            }));
        }

        let content_from_parts = |parts: Vec<Value>| -> Option<Value> {
            if parts.is_empty() {
                return None;
            }
            if parts.len() == 1 {
                let part = &parts[0];
                let has_cache = part.get("cache_control").is_some();
                if !has_cache && let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                    return Some(serde_json::json!(text));
                }
            }
            Some(Value::Array(parts))
        };

        let mut tool_result_last_pos: HashMap<String, usize> = HashMap::new();
        for (idx, msg) in effective_messages.iter().enumerate() {
            if let Role::User = msg.role {
                for block in &msg.content {
                    if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                        tool_result_last_pos.insert(tool_use_id.clone(), idx);
                    }
                }
            }
        }

        let missing_output = format!("[Error] {}", TOOL_OUTPUT_MISSING_TEXT);
        let mut injected_missing = 0usize;
        let mut delayed_results = 0usize;
        let mut skipped_results = 0usize;
        let mut tool_calls_seen: HashSet<String> = HashSet::new();
        let mut pending_tool_results: HashMap<String, String> = HashMap::new();
        let mut used_tool_results: HashSet<String> = HashSet::new();

        // Convert messages
        for (idx, msg) in effective_messages.iter().enumerate() {
            match msg.role {
                Role::User => {
                    let mut pending_user_parts: Vec<Value> = Vec::new();
                    for block in &msg.content {
                        match block {
                            ContentBlock::Text {
                                text,
                                cache_control,
                            } => {
                                let mut part = serde_json::json!({
                                    "type": "text",
                                    "text": text
                                });
                                if let Some(cache_control) = cache_control {
                                    part["cache_control"] =
                                        serde_json::to_value(cache_control).unwrap_or(Value::Null);
                                }
                                pending_user_parts.push(part);
                            }
                            ContentBlock::Image { media_type, data } => {
                                pending_user_parts.push(serde_json::json!({
                                    "type": "image_url",
                                    "image_url": {
                                        "url": format!("data:{};base64,{}", media_type, data)
                                    }
                                }));
                            }
                            ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                is_error,
                            } => {
                                if let Some(content) =
                                    content_from_parts(std::mem::take(&mut pending_user_parts))
                                {
                                    api_messages.push(serde_json::json!({
                                        "role": "user",
                                        "content": content
                                    }));
                                }

                                if used_tool_results.contains(tool_use_id) {
                                    skipped_results += 1;
                                    continue;
                                }
                                let output = if is_error == &Some(true) {
                                    format!("[Error] {}", content)
                                } else {
                                    content.clone()
                                };
                                if tool_calls_seen.contains(tool_use_id) {
                                    api_messages.push(serde_json::json!({
                                        "role": "tool",
                                        "tool_call_id": crate::message::sanitize_tool_id(tool_use_id),
                                        "content": output
                                    }));
                                    used_tool_results.insert(tool_use_id.clone());
                                } else if pending_tool_results.contains_key(tool_use_id) {
                                    skipped_results += 1;
                                } else {
                                    pending_tool_results.insert(tool_use_id.clone(), output);
                                    delayed_results += 1;
                                }
                            }
                            _ => {}
                        }
                    }

                    if let Some(content) =
                        content_from_parts(std::mem::take(&mut pending_user_parts))
                    {
                        api_messages.push(serde_json::json!({
                            "role": "user",
                            "content": content
                        }));
                    }
                }
                Role::Assistant => {
                    let mut text_content = String::new();
                    let mut reasoning_content = String::new();
                    let mut tool_calls = Vec::new();
                    let mut post_tool_outputs: Vec<(String, String)> = Vec::new();
                    let mut missing_tool_outputs: Vec<String> = Vec::new();

                    for block in &msg.content {
                        match block {
                            ContentBlock::Text { text, .. } => {
                                text_content.push_str(text);
                            }
                            ContentBlock::Reasoning { text } => {
                                reasoning_content.push_str(text);
                            }
                            ContentBlock::ToolUse { id, name, input } => {
                                let args = if input.is_object() {
                                    serde_json::to_string(input).unwrap_or_default()
                                } else {
                                    "{}".to_string()
                                };
                                tool_calls.push(serde_json::json!({
                                    "id": crate::message::sanitize_tool_id(id),
                                    "type": "function",
                                    "function": {
                                        "name": name,
                                        "arguments": args
                                    }
                                }));
                                tool_calls_seen.insert(id.clone());
                                if let Some(output) = pending_tool_results.remove(id) {
                                    post_tool_outputs.push((id.clone(), output));
                                    used_tool_results.insert(id.clone());
                                } else {
                                    let has_future_output = tool_result_last_pos
                                        .get(id)
                                        .map(|pos| *pos > idx)
                                        .unwrap_or(false);
                                    if !has_future_output {
                                        missing_tool_outputs.push(id.clone());
                                        used_tool_results.insert(id.clone());
                                    }
                                }
                            }
                            _ => {}
                        }
                    }

                    let mut assistant_msg = serde_json::json!({
                        "role": "assistant",
                    });

                    if !text_content.is_empty() {
                        assistant_msg["content"] = serde_json::json!(text_content);
                    }

                    if !tool_calls.is_empty() {
                        assistant_msg["tool_calls"] = serde_json::json!(tool_calls);
                    }

                    let has_reasoning_content = !reasoning_content.is_empty();
                    if allow_reasoning
                        && (include_reasoning_content || has_reasoning_content)
                        && (has_reasoning_content || !tool_calls.is_empty())
                    {
                        let reasoning_payload = if has_reasoning_content {
                            reasoning_content.clone()
                        } else {
                            " ".to_string()
                        };
                        assistant_msg["reasoning_content"] = serde_json::json!(reasoning_payload);
                    }

                    if !text_content.is_empty() || !tool_calls.is_empty() || has_reasoning_content {
                        api_messages.push(assistant_msg);

                        for (tool_call_id, output) in post_tool_outputs {
                            api_messages.push(serde_json::json!({
                                "role": "tool",
                                "tool_call_id": crate::message::sanitize_tool_id(&tool_call_id),
                                "content": output
                            }));
                        }

                        if !missing_tool_outputs.is_empty() {
                            injected_missing += missing_tool_outputs.len();
                            for missing_id in missing_tool_outputs {
                                api_messages.push(serde_json::json!({
                                    "role": "tool",
                                    "tool_call_id": crate::message::sanitize_tool_id(&missing_id),
                                    "content": missing_output.clone()
                                }));
                            }
                        }
                    }
                }
            }
        }

        if delayed_results > 0 {
            crate::logging::info(&format!(
                "[openrouter] Delayed {} tool output(s) to preserve call ordering",
                delayed_results
            ));
        }

        if !pending_tool_results.is_empty() {
            skipped_results += pending_tool_results.len();
        }

        if injected_missing > 0 {
            crate::logging::info(&format!(
                "[openrouter] Injected {} synthetic tool output(s) to prevent API error",
                injected_missing
            ));
        }
        if skipped_results > 0 {
            crate::logging::info(&format!(
                "[openrouter] Filtered {} orphaned tool result(s) to prevent API error",
                skipped_results
            ));
        }

        // Safety pass: ensure tool-call messages include reasoning_content (when allowed)
        // and that every tool call has a matching tool output after it.
        let mut outputs_after: HashSet<String> = HashSet::new();
        let mut missing_by_index: Vec<Vec<String>> = vec![Vec::new(); api_messages.len()];

        for (idx, msg) in api_messages.iter().enumerate().rev() {
            let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
            if role == "tool" {
                if let Some(id) = msg.get("tool_call_id").and_then(|v| v.as_str()) {
                    outputs_after.insert(id.to_string());
                }
                continue;
            }

            if role == "assistant"
                && let Some(tool_calls) = msg.get("tool_calls").and_then(|v| v.as_array())
            {
                for call in tool_calls {
                    if let Some(id) = call.get("id").and_then(|v| v.as_str())
                        && !outputs_after.contains(id)
                    {
                        outputs_after.insert(id.to_string());
                        missing_by_index[idx].push(id.to_string());
                    }
                }
            }
        }

        let mut normalized = Vec::with_capacity(api_messages.len());
        let mut extra_outputs = 0usize;
        let mut missing_reasoning = 0usize;

        for (idx, mut msg) in api_messages.into_iter().enumerate() {
            let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
            if role == "assistant"
                && allow_reasoning
                && msg.get("tool_calls").and_then(|v| v.as_array()).is_some()
            {
                let needs_reasoning = match msg.get("reasoning_content") {
                    Some(value) => value.as_str().map(|s| s.trim().is_empty()).unwrap_or(true),
                    None => true,
                };
                if needs_reasoning {
                    msg["reasoning_content"] = serde_json::json!(" ");
                    missing_reasoning += 1;
                }
            }

            normalized.push(msg);

            if let Some(missing) = missing_by_index.get(idx) {
                for id in missing {
                    extra_outputs += 1;
                    normalized.push(serde_json::json!({
                        "role": "tool",
                        "tool_call_id": id,
                        "content": missing_output.clone()
                    }));
                }
            }
        }

        api_messages = normalized;

        if missing_reasoning > 0 {
            crate::logging::info(&format!(
                "[openrouter] Filled reasoning_content on {} tool-call message(s)",
                missing_reasoning
            ));
        }
        if extra_outputs > 0 {
            crate::logging::info(&format!(
                "[openrouter] Safety-injected {} missing tool output(s) at request build",
                extra_outputs
            ));
        }

        // Final safety pass: ensure every tool_call_id has at least one tool response after it.
        let mut tool_output_positions: HashMap<String, usize> = HashMap::new();
        for (idx, msg) in api_messages.iter().enumerate() {
            if msg.get("role").and_then(|v| v.as_str()) == Some("tool")
                && let Some(id) = msg.get("tool_call_id").and_then(|v| v.as_str())
            {
                tool_output_positions.entry(id.to_string()).or_insert(idx);
            }
        }

        let mut missing_after: HashSet<String> = HashSet::new();
        for (idx, msg) in api_messages.iter().enumerate() {
            if msg.get("role").and_then(|v| v.as_str()) != Some("assistant") {
                continue;
            }
            if let Some(tool_calls) = msg.get("tool_calls").and_then(|v| v.as_array()) {
                for call in tool_calls {
                    if let Some(id) = call.get("id").and_then(|v| v.as_str()) {
                        let has_after = tool_output_positions
                            .get(id)
                            .map(|pos| *pos > idx)
                            .unwrap_or(false);
                        if !has_after {
                            missing_after.insert(id.to_string());
                        }
                    }
                }
            }
        }

        if !missing_after.is_empty() {
            for id in missing_after.iter() {
                api_messages.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": id,
                    "content": missing_output.clone()
                }));
            }
            crate::logging::info(&format!(
                "[openrouter] Appended {} tool output(s) to satisfy call ordering",
                missing_after.len()
            ));
        }

        // Final pass: ensure tool outputs immediately follow assistant tool calls.
        let mut tool_output_map: HashMap<String, Value> = HashMap::new();
        for msg in &api_messages {
            if msg.get("role").and_then(|v| v.as_str()) == Some("tool")
                && let Some(id) = msg.get("tool_call_id").and_then(|v| v.as_str())
            {
                let is_missing = msg
                    .get("content")
                    .and_then(|v| v.as_str())
                    .map(|v| v == missing_output)
                    .unwrap_or(false);
                match tool_output_map.get(id) {
                    Some(existing) => {
                        let existing_missing = existing
                            .get("content")
                            .and_then(|v| v.as_str())
                            .map(|v| v == missing_output)
                            .unwrap_or(false);
                        if existing_missing && !is_missing {
                            tool_output_map.insert(id.to_string(), msg.clone());
                        }
                    }
                    None => {
                        tool_output_map.insert(id.to_string(), msg.clone());
                    }
                }
            }
        }

        let mut reordered: Vec<Value> = Vec::with_capacity(api_messages.len());
        let mut used_outputs: HashSet<String> = HashSet::new();
        let mut injected_ordered = 0usize;
        let mut dropped_orphans = 0usize;

        for msg in api_messages.into_iter() {
            let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
            if role == "assistant" {
                let tool_calls = msg.get("tool_calls").and_then(|v| v.as_array()).cloned();
                if let Some(tool_calls) = tool_calls {
                    if tool_calls.is_empty() {
                        reordered.push(msg);
                        continue;
                    }
                    reordered.push(msg);
                    for call in tool_calls {
                        if let Some(id) = call.get("id").and_then(|v| v.as_str()) {
                            if let Some(tool_msg) = tool_output_map.get(id) {
                                reordered.push(tool_msg.clone());
                                used_outputs.insert(id.to_string());
                            } else {
                                injected_ordered += 1;
                                reordered.push(serde_json::json!({
                                    "role": "tool",
                                    "tool_call_id": id,
                                    "content": missing_output.clone()
                                }));
                                used_outputs.insert(id.to_string());
                            }
                        }
                    }
                    continue;
                }
            }

            if role == "tool" {
                if let Some(id) = msg.get("tool_call_id").and_then(|v| v.as_str())
                    && used_outputs.contains(id)
                {
                    dropped_orphans += 1;
                    continue;
                }
                dropped_orphans += 1;
                continue;
            }

            reordered.push(msg);
        }

        api_messages = reordered;

        if injected_ordered > 0 {
            crate::logging::info(&format!(
                "[openrouter] Inserted {} tool output(s) to enforce call ordering",
                injected_ordered
            ));
        }
        if dropped_orphans > 0 {
            crate::logging::info(&format!(
                "[openrouter] Dropped {} orphaned tool output(s) during re-ordering",
                dropped_orphans
            ));
        }

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

        if !api_tools.is_empty() {
            request["tools"] = serde_json::json!(api_tools);
            request["tool_choice"] = serde_json::json!("auto");
        }

        // Optional thinking override for OpenRouter (provider-specific).
        if let Some(enable) = thinking_enabled {
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

        // OpenRouter uses HTTPS/SSE transport only
        crate::logging::info("OpenRouter transport: HTTPS (SSE)");

        let (tx, rx) = mpsc::channel::<Result<StreamEvent>>(100);
        let client = self.client.clone();
        let api_base = self.api_base.clone();
        let auth = self.auth.clone();
        let send_openrouter_headers = self.send_openrouter_headers;
        let extra_headers = self.extra_headers.clone();
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
                extra_headers,
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

    fn model(&self) -> String {
        self.model
            .try_read()
            .map(|m| m.clone())
            .unwrap_or_else(|_| DEFAULT_MODEL.to_string())
    }

    fn supports_image_input(&self) -> bool {
        false
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

    fn available_models(&self) -> Vec<&'static str> {
        // OpenRouter models are fetched dynamically from the API.
        // Static list is empty; use available_models_display for cached list.
        vec![]
    }

    fn available_models_display(&self) -> Vec<String> {
        let with_current_model = |mut models: Vec<String>| {
            let current = self.model();
            if !current.trim().is_empty() && !models.iter().any(|model| model == &current) {
                models.insert(0, current);
            }
            models
        };

        let merge_static_models = |mut models: Vec<String>| {
            for model in &self.static_models {
                if !model.trim().is_empty() && !models.iter().any(|existing| existing == model) {
                    models.push(model.clone());
                }
            }
            with_current_model(models)
        };

        if !self.supports_model_catalog {
            if !self.static_models.is_empty() {
                return with_current_model(self.static_models.clone());
            }
            let model = self.model();
            return if model.trim().is_empty() {
                Vec::new()
            } else {
                vec![model]
            };
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
            return merge_static_models(cache.models.iter().map(|m| m.id.clone()).collect());
        }

        if let Some(cache_entry) = load_disk_cache_entry() {
            let cache_age = current_unix_secs()
                .map(|now| now.saturating_sub(cache_entry.cached_at))
                .unwrap_or(0);
            if let Ok(mut cache) = self.models_cache.try_write() {
                cache.models = cache_entry.models.clone();
                cache.fetched = true;
                cache.cached_at = Some(cache_entry.cached_at);
            }
            self.maybe_schedule_model_catalog_refresh(cache_age, "display disk cache");
            return merge_static_models(cache_entry.models.into_iter().map(|m| m.id).collect());
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
            return with_current_model(self.static_models.clone());
        }

        let model = self.model();
        if model.trim().is_empty() {
            Vec::new()
        } else {
            vec![model]
        }
    }

    fn available_models_for_switching(&self) -> Vec<String> {
        self.available_models_display()
    }

    fn model_routes(&self) -> Vec<crate::provider::ModelRoute> {
        let provider_label = self
            .profile_id
            .as_deref()
            .and_then(openai_compatible_profile_by_id)
            .map(|profile| profile.display_name.to_string())
            .unwrap_or_else(|| {
                if self.supports_provider_features {
                    "OpenRouter".to_string()
                } else {
                    "OpenAI-compatible".to_string()
                }
            });
        let api_method = if self.supports_provider_features {
            "openrouter".to_string()
        } else if let Some(profile_id) = self.profile_id.as_deref() {
            format!("openai-compatible:{}", profile_id)
        } else {
            "openai-compatible".to_string()
        };
        let detail = if self.supports_provider_features {
            String::new()
        } else {
            self.api_base.clone()
        };

        self.available_models_display()
            .into_iter()
            .filter(|model| crate::provider::is_listable_model_name(model))
            .map(|model| crate::provider::ModelRoute {
                model,
                provider: provider_label.clone(),
                api_method: api_method.clone(),
                available: true,
                detail: detail.clone(),
                cheapness: None,
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

            for model in targets {
                let _ = self.refresh_endpoints(&model).await;
            }
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
            api_base: self.api_base.clone(),
            auth: self.auth.clone(),
            supports_provider_features: self.supports_provider_features,
            supports_model_catalog: self.supports_model_catalog,
            profile_id: self.profile_id.clone(),
            static_models: self.static_models.clone(),
            static_context_limits: self.static_context_limits.clone(),
            send_openrouter_headers: self.send_openrouter_headers,
            extra_headers: self.extra_headers.clone(),
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
