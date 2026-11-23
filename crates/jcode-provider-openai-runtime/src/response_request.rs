use super::*;

impl OpenAIProvider {
    /// Shared request settings keep speculative warmup and foreground generation
    /// identical even when reasoning, tools, cache policy, or service tier change.
    pub(super) async fn response_request<'a>(
        &self,
        input: impl Into<std::borrow::Cow<'a, [Value]>>,
        tools: &[ToolDefinition],
        system: &str,
    ) -> Value {
        let model_id = self.model_id().await;
        let is_chatgpt_mode = Self::is_chatgpt_mode(&*self.credentials.read().await);
        self.response_request_for_model(&model_id, input, tools, system, is_chatgpt_mode)
    }

    pub(super) fn response_request_for_model<'a>(
        &self,
        model_id: &str,
        input: impl Into<std::borrow::Cow<'a, [Value]>>,
        tools: &[ToolDefinition],
        system: &str,
        is_chatgpt_mode: bool,
    ) -> Value {
        let hosted_tools =
            native_web_search::hosted_tools_for_request(model_id, is_chatgpt_mode, tools);
        let tools = native_web_search::without_local_websearch(tools, &hosted_tools);
        let mut api_tools = build_tools(&tools);
        api_tools.extend(hosted_tools);
        let reasoning_effort = self
            .reasoning_effort
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
            // No explicit user effort: fall back to the model's jcode-side
            // default (e.g. `low` for GPT-5.6 Sol).
            .or_else(|| Self::default_reasoning_effort_for_model(model_id));
        // Map the `swarm` sentinel (and any future aliases) to the real effort
        // value the API understands.
        let api_reasoning_effort = self.api_reasoning_effort(reasoning_effort.as_deref());
        let service_tier = self.effective_service_tier(model_id);
        let native_compaction_threshold =
            self.native_compaction_threshold_for_context_window(self.context_window());
        Self::build_response_request(
            model_id,
            system.to_string(),
            input,
            &api_tools,
            is_chatgpt_mode,
            self.max_output_tokens,
            api_reasoning_effort.as_deref(),
            service_tier.as_deref(),
            Some(&self.cache_routing_key()),
            self.prompt_cache_retention.as_deref(),
            native_compaction_threshold,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "request construction threads explicit per-request OpenAI settings without hidden state"
    )]
    pub(super) fn build_response_request<'a>(
        model_id: &str,
        instructions: String,
        input: impl Into<std::borrow::Cow<'a, [Value]>>,
        api_tools: &[Value],
        is_chatgpt_mode: bool,
        max_output_tokens: Option<u32>,
        reasoning_effort: Option<&str>,
        service_tier: Option<&str>,
        prompt_cache_key: Option<&str>,
        prompt_cache_retention: Option<&str>,
        native_compaction_threshold: Option<usize>,
    ) -> Value {
        let mut tools = api_tools.to_vec();
        // The hosted `image_generation` tool is only available to general
        // ChatGPT/GPT models on the Responses backend. Codex models
        // (`*-codex*`) reject unknown hosted tools, so don't attach it for them.
        if is_chatgpt_mode && model_supports_image_generation(model_id) {
            tools.push(serde_json::json!({ "type": "image_generation" }));
        }

        let mut input = input.into();
        // GPT-6.2 renders top-level instructions after the cacheable history.
        // Keep stable guidance in history, including on previous_response_id
        // continuations. Retain the string field for the ChatGPT endpoint.
        let instructions = if !instructions.is_empty()
            && ["gpt-6.2-astra", "gpt-6.2-sol"].iter().any(|family| {
                model_id == *family
                    || model_id
                        .strip_prefix(family)
                        .is_some_and(|s| s.starts_with('-'))
            }) {
            input.to_mut().insert(
                0,
                serde_json::json!({"role": "developer", "content": instructions}),
            );
            String::new()
        } else {
            instructions
        };
        // Move the input and tool arrays directly into the envelope. json!
        // would serialize owned Values and clone the entire history.
        let mut request = Value::Object(
            [
                ("model", Value::from(model_id)),
                ("instructions", Value::String(instructions)),
                ("input", Value::Array(input.into_owned())),
                ("tools", Value::Array(tools)),
                ("tool_choice", Value::from("auto")),
                ("parallel_tool_calls", Value::Bool(false)),
                ("stream", Value::Bool(true)),
                ("store", Value::Bool(false)),
                (
                    "include",
                    serde_json::json!(["reasoning.encrypted_content"]),
                ),
            ]
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
        );

        if !is_chatgpt_mode && let Some(max_output_tokens) = max_output_tokens {
            request["max_output_tokens"] = serde_json::json!(max_output_tokens);
        }

        if let Some(effort) = reasoning_effort {
            request["reasoning"] = openai_stream_runtime::reasoning_payload(effort);
        }

        // The provider filters against the actual request model before building.
        if let Some(service_tier) = service_tier {
            request["service_tier"] = serde_json::json!(service_tier);
        }

        if let Some(compact_threshold) = native_compaction_threshold {
            request["context_management"] = serde_json::json!([
                {
                    "type": "compaction",
                    "compact_threshold": compact_threshold,
                }
            ]);
        }

        // Codex sends this for OAuth too. ChatGPT additionally derives cache
        // affinity from the matching session-id transport header.
        if let Some(key) = prompt_cache_key {
            request["prompt_cache_key"] = serde_json::json!(key);
        }
        if !is_chatgpt_mode
            && let Some(retention) =
                Self::effective_prompt_cache_retention(model_id, prompt_cache_retention)
        {
            request["prompt_cache_retention"] = serde_json::json!(retention);
        }

        request
    }
}
