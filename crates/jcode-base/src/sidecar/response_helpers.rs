use super::*;

pub(super) fn resolve_openai_request_model(
    preferred_model: &str,
    is_chatgpt_mode: bool,
) -> (&str, Option<&'static str>) {
    if preferred_model != SIDECAR_OPENAI_MODEL {
        return (preferred_model, None);
    }

    match (
        is_chatgpt_mode,
        crate::provider::is_model_available_for_account(SIDECAR_OPENAI_MODEL),
    ) {
        (true, Some(false)) => (
            SIDECAR_OPENAI_OAUTH_FALLBACK_MODEL,
            Some(SIDECAR_OPENAI_OAUTH_FALLBACK_REASONING),
        ),
        _ => (SIDECAR_OPENAI_MODEL, Some(SIDECAR_OPENAI_REASONING)),
    }
}

pub(super) fn build_openai_request(
    model: &str,
    system: &str,
    user_message: &str,
    stream: bool,
    reasoning_effort: Option<&str>,
) -> serde_json::Value {
    let mut instructions = String::new();
    if !system.is_empty() {
        instructions.push_str(system);
    }

    let mut request = serde_json::json!({
        "model": model,
        "instructions": instructions,
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{
                "type": "input_text",
                "text": user_message,
            }],
        }],
        "stream": stream,
        "store": false,
    });

    if let Some(effort) = reasoning_effort {
        request["reasoning"] = serde_json::json!({ "effort": effort });
    }

    request
}

pub(super) fn classify_openai_model_unavailable(status: StatusCode, body: &str) -> Option<String> {
    let lower = body.to_ascii_lowercase();
    let mentions_model = lower.contains("model")
        || lower.contains("slug")
        || lower.contains("engine")
        || lower.contains("deployment");
    let unavailable = lower.contains("not available")
        || lower.contains("unavailable")
        || lower.contains("does not have access")
        || lower.contains("not enabled")
        || lower.contains("not found")
        || lower.contains("unknown model")
        || lower.contains("unsupported model")
        || lower.contains("invalid model");

    if !mentions_model || !unavailable {
        return None;
    }

    if matches!(
        status,
        StatusCode::NOT_FOUND
            | StatusCode::FORBIDDEN
            | StatusCode::BAD_REQUEST
            | StatusCode::UNPROCESSABLE_ENTITY
    ) {
        let trimmed = body.trim();
        return Some(if trimmed.is_empty() {
            format!("model denied by OpenAI API (status {})", status)
        } else {
            format!(
                "model denied by OpenAI API (status {}): {}",
                status, trimmed
            )
        });
    }

    None
}

pub(super) fn is_openai_model_unavailable(status: StatusCode, body: &str) -> bool {
    classify_openai_model_unavailable(status, body).is_some()
}

/// Collect text from an OpenAI Responses API SSE stream.
///
/// Parses `data: <json>` lines and accumulates text deltas from
/// `response.output_text.delta` events, stopping on completion/done.
pub(super) async fn collect_openai_sse_text(
    response: reqwest::Response,
    model: &str,
) -> Result<String> {
    use futures::StreamExt;
    let mut stream = response.bytes_stream();
    let mut text = String::new();
    let mut buf = String::new();
    let mut resolved_model = None;

    while let Some(chunk) = stream.next().await {
        let bytes = chunk.context("Error reading SSE stream")?;
        buf.push_str(&String::from_utf8_lossy(&bytes));

        // Process all complete lines in the buffer
        while let Some(newline_pos) = buf.find('\n') {
            let line = buf[..newline_pos].trim_end_matches('\r').to_string();
            buf = buf[newline_pos + 1..].to_string();

            if let Some(data) = crate::util::sse_data_line(&line) {
                if data == "[DONE]" {
                    return Ok(text);
                }
                if let Ok(event) = serde_json::from_str::<SseEvent>(data) {
                    if let Some(model) = openai_sidecar_response_model(event.response.as_ref()) {
                        resolved_model = Some(model.to_owned());
                    }
                    match event.kind.as_str() {
                        "response.output_text.delta" => {
                            if let Some(delta) = event.delta {
                                text.push_str(&delta);
                            }
                        }
                        "response.completed" | "response.incomplete" => {
                            record_openai_sidecar_usage(
                                model,
                                event.response.as_ref(),
                                resolved_model.as_deref(),
                            );
                            return Ok(text);
                        }
                        "response.failed" | "error" => {
                            let msg = event
                                .error
                                .as_ref()
                                .and_then(|e| e.as_str())
                                .unwrap_or("unknown error");
                            anyhow::bail!("OpenAI SSE error: {}", msg);
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    Ok(text)
}

/// Parsed OpenAI Responses API `usage` object. OpenAI reports cached prompt
/// tokens as a subset of `input_tokens` (`input_tokens_details.cached_tokens`),
/// so the total stays `input + output`; cache reads are surfaced separately for
/// pricing, the same convention the agent path uses for OpenAI providers.
pub(super) fn parse_openai_usage(
    usage: &serde_json::Value,
) -> Option<crate::telemetry::ProviderUsage> {
    let input = usage.get("input_tokens").and_then(|v| v.as_u64());
    let output = usage.get("output_tokens").and_then(|v| v.as_u64());
    if input.is_none() && output.is_none() {
        return None;
    }
    let cached = usage
        .get("input_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(|v| v.as_u64());
    Some(crate::telemetry::ProviderUsage {
        input_tokens: input.unwrap_or(0),
        output_tokens: output.unwrap_or(0),
        cache_read_input_tokens: cached,
        cache_creation_input_tokens: None,
    })
}

pub(super) fn openai_sidecar_response_model(response: Option<&serde_json::Value>) -> Option<&str> {
    jcode_provider_core::nonempty_response_model(
        response
            .and_then(|response| response.get("model"))
            .and_then(|model| model.as_str()),
    )
}

pub(super) fn record_openai_sidecar_usage(
    model: &str,
    response: Option<&serde_json::Value>,
    observed_model: Option<&str>,
) {
    if let Some(response) = response
        && let Some(parsed) = response.get("usage").and_then(parse_openai_usage)
    {
        let resolved_model = openai_sidecar_response_model(Some(response)).or(observed_model);
        crate::telemetry::record_provider_usage_with_model_attribution(
            None,
            "openai",
            model,
            resolved_model,
            crate::telemetry::UsageSource::Sidecar,
            parsed,
        );
    }
}

/// Extract text from a non-streaming OpenAI Responses API JSON response.
pub(super) fn extract_openai_response_text(result: &serde_json::Value) -> Result<String> {
    let mut text = String::new();
    if let Some(output) = result.get("output").and_then(|v| v.as_array()) {
        for item in output {
            let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if item_type == "message"
                && let Some(content) = item.get("content").and_then(|v| v.as_array())
            {
                for block in content {
                    let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    if (block_type == "output_text" || block_type == "text")
                        && let Some(t) = block.get("text").and_then(|v| v.as_str())
                    {
                        text.push_str(t);
                    }
                }
            }
        }
    }
    Ok(text)
}

pub(super) fn build_claude_system_param(system: &str) -> Option<ClaudeApiSystem<'_>> {
    let mut blocks = Vec::new();
    blocks.push(ClaudeApiSystemBlock {
        block_type: "text",
        text: CLAUDE_OAUTH_BILLING_BLOCK,
    });
    if !system.is_empty() {
        blocks.push(ClaudeApiSystemBlock {
            block_type: "text",
            text: system,
        });
    }
    Some(ClaudeApiSystem::Blocks(blocks))
}

/// Build the system param for the direct API-key path.
///
/// The OAuth billing block is only valid for the subscription endpoint; a
/// direct API key talks to the standard Messages API. So this only carries
/// the caller's own system prompt (if any).
pub(super) fn build_claude_api_key_system_param(system: &str) -> Option<ClaudeApiSystem<'_>> {
    if system.is_empty() {
        return None;
    }
    Some(ClaudeApiSystem::Blocks(vec![ClaudeApiSystemBlock {
        block_type: "text",
        text: system,
    }]))
}

/// Whether the sidecar's Claude backend should use the direct API key rather
/// than OAuth. True when the runtime is pinned to Anthropic API-key mode
/// (`claude-api`), or when no OAuth credentials are present at all. Mirrors the
/// main provider's resolution so memory features authenticate the same way the
/// agent does.
pub(super) fn anthropic_sidecar_prefers_api_key() -> bool {
    match jcode_provider_core::runtime_env_pinned_mode(
        jcode_provider_core::DualAuthProvider::Anthropic,
    ) {
        Some(jcode_provider_core::AuthMode::ApiKey) => true,
        Some(jcode_provider_core::AuthMode::Oauth) => false,
        None => auth::claude::load_credentials().is_err(),
    }
}

/// Recognize the Anthropic "OAuth not allowed for this organization" 403 so the
/// sidecar can transparently fall back to the API key.
pub(super) fn is_anthropic_oauth_forbidden(err: &anyhow::Error) -> bool {
    let msg = err.to_string();
    msg.contains("403")
        && (msg.contains("OAuth authentication is currently not allowed")
            || msg.contains("permission_error"))
}
