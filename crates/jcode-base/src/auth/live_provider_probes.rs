//! Live OpenAI-compatible provider probes shared by the auth lifecycle driver
//! and the provider doctor. These are pure HTTP/JSON checks with no test-only
//! dependencies, so they compile into the shipping binary.

use anyhow::{Context, ensure};
use serde::Deserialize;

use crate::provider_catalog::{OpenAiCompatibleProfile, ResolvedOpenAiCompatibleProfile};

/// Apply the right auth headers for a resolved OpenAI-compatible profile.
///
/// Most providers use `Authorization: Bearer <key>`. Anthropic's
/// OpenAI-compatible endpoints authenticate with `x-api-key` plus a required
/// `anthropic-version` header and reject Bearer auth (401), so key off the
/// resolved host.
fn apply_provider_auth(
    request: reqwest::RequestBuilder,
    resolved: &ResolvedOpenAiCompatibleProfile,
    api_key: &str,
) -> reqwest::RequestBuilder {
    if resolved
        .api_base
        .to_ascii_lowercase()
        .contains("api.anthropic.com")
    {
        return request
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01");
    }
    request.bearer_auth(api_key)
}

/// Set an output-token cap on a chat-completions body using the parameter name
/// the provider accepts. OpenAI's newer models (gpt-5.x) reject the legacy
/// `max_tokens` and require `max_completion_tokens`; most OpenAI-compatible and
/// Anthropic endpoints still take `max_tokens`. Keying off the resolved host
/// keeps the live probes one round-trip without provider-specific retries.
fn set_output_token_cap(
    body: &mut serde_json::Value,
    resolved: &ResolvedOpenAiCompatibleProfile,
    cap: u32,
) {
    let key = if resolved
        .api_base
        .to_ascii_lowercase()
        .contains("api.openai.com")
    {
        "max_completion_tokens"
    } else {
        "max_tokens"
    };
    body[key] = serde_json::json!(cap);
}

#[derive(Debug, Deserialize)]
struct OpenAiCompatibleModelsResponse {
    #[serde(default)]
    data: Vec<OpenAiCompatibleModelInfo>,
}

#[derive(Debug, Deserialize)]
struct OpenAiCompatibleModelInfo {
    id: String,
}

pub async fn fetch_live_openai_compatible_models(
    profile: OpenAiCompatibleProfile,
    api_key: &str,
) -> anyhow::Result<Vec<String>> {
    let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
    let url = format!("{}/models", resolved.api_base.trim_end_matches('/'));
    let request = crate::provider::shared_http_client().get(&url);
    let request = apply_provider_auth(request, &resolved, api_key);
    let response = tokio::time::timeout(std::time::Duration::from_secs(20), request.send())
        .await
        .context("timed out fetching live model catalog")?
        .with_context(|| {
            format!(
                "fetch live {} model catalog from {url}",
                resolved.display_name
            )
        })?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    ensure!(
        status.is_success(),
        "{} live model catalog failed (HTTP {}): {}",
        resolved.display_name,
        status,
        body.trim()
    );

    let parsed: OpenAiCompatibleModelsResponse = serde_json::from_str(&body)
        .with_context(|| format!("parse live {} model catalog", resolved.display_name))?;
    let models = parsed
        .data
        .into_iter()
        .map(|model| model.id.trim().to_string())
        .filter(|model| {
            !model.is_empty()
                && crate::provider_catalog::openai_compatible_profile_model_supports_chat(
                    resolved.id.as_str(),
                    model,
                )
        })
        .collect::<Vec<_>>();
    ensure!(
        !models.is_empty(),
        "{} live model catalog returned no models",
        resolved.display_name
    );
    Ok(models)
}

pub async fn run_live_openai_compatible_smoke(
    profile: OpenAiCompatibleProfile,
    api_key: &str,
    model: &str,
) -> anyhow::Result<crate::live_tests::LiveVerificationStage> {
    let started = std::time::Instant::now();
    let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
    let url = format!(
        "{}/chat/completions",
        resolved.api_base.trim_end_matches('/')
    );
    let body = serde_json::json!({
        "model": model,
        "messages": [
            {"role": "user", "content": "Reply with exactly AUTH_TEST_OK and nothing else."}
        ],
        "stream": false
    });
    let request = crate::provider::shared_http_client().post(&url).json(&body);
    let request = apply_provider_auth(request, &resolved, api_key);
    let response = tokio::time::timeout(std::time::Duration::from_secs(30), request.send())
        .await
        .context("timed out running live smoke completion")?
        .with_context(|| format!("run live {} smoke completion", resolved.display_name))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    ensure!(
        status.is_success(),
        "{} live smoke failed (HTTP {}): {}",
        resolved.display_name,
        status,
        text.trim()
    );
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .with_context(|| format!("parse live {} smoke response", resolved.display_name))?;
    let content = parsed
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(|content| content.as_str())
        .unwrap_or_default()
        .trim();
    ensure!(
        content.contains("AUTH_TEST_OK"),
        "{} live smoke returned unexpected content: {:?}",
        resolved.display_name,
        content
    );
    let mut stage = crate::live_tests::LiveVerificationStage::passed(
        crate::live_tests::checkpoints::NON_STREAMING_CHAT_COMPLETION,
    )
    .with_duration_ms(started.elapsed().as_millis() as u64)
    .with_evidence("http_status", serde_json::json!(status.as_u16()))
    .with_evidence("matched_expected_content", serde_json::json!(true));
    for key in ["id", "model", "usage", "cost"] {
        if let Some(value) = parsed.get(key) {
            stage = stage.with_evidence(key, value.clone());
        }
    }
    Ok(stage)
}

pub async fn run_live_openai_compatible_stream_smoke(
    profile: OpenAiCompatibleProfile,
    api_key: &str,
    model: &str,
) -> anyhow::Result<crate::live_tests::LiveVerificationStage> {
    let started = std::time::Instant::now();
    let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
    let url = format!(
        "{}/chat/completions",
        resolved.api_base.trim_end_matches('/')
    );
    let body = serde_json::json!({
        "model": model,
        "messages": [
            {"role": "user", "content": "Reply with exactly STREAM_TEST_OK and nothing else."}
        ],
        "stream": true,
        "stream_options": {"include_usage": true}
    });
    let request = crate::provider::shared_http_client().post(&url).json(&body);
    let request = apply_provider_auth(request, &resolved, api_key);
    let response = tokio::time::timeout(std::time::Duration::from_secs(45), request.send())
        .await
        .context("timed out running live stream smoke completion")?
        .with_context(|| format!("run live {} stream smoke completion", resolved.display_name))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    ensure!(
        status.is_success(),
        "{} live stream smoke failed (HTTP {}): {}",
        resolved.display_name,
        status,
        text.trim()
    );

    let mut content = String::new();
    let mut chunk_count = 0usize;
    let mut finish_reason = serde_json::Value::Null;
    let mut usage = serde_json::Value::Null;
    for line in text.lines() {
        let Some(data) = line.trim().strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data == "[DONE]" {
            break;
        }
        if data.is_empty() {
            continue;
        }
        let parsed: serde_json::Value = serde_json::from_str(data)
            .with_context(|| format!("parse live {} stream chunk", resolved.display_name))?;
        chunk_count += 1;
        if let Some(reported) = parsed.get("usage").filter(|usage| !usage.is_null()) {
            usage = reported.clone();
        }
        if let Some(delta) = parsed
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("delta"))
            && let Some(part) = delta.get("content").and_then(|content| content.as_str())
        {
            content.push_str(part);
        }
        if let Some(reason) = parsed
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("finish_reason"))
            .filter(|reason| !reason.is_null())
        {
            finish_reason = reason.clone();
        }
    }
    ensure!(
        content.contains("STREAM_TEST_OK"),
        "{} live stream smoke returned unexpected content: {:?}",
        resolved.display_name,
        content
    );
    let mut stage = crate::live_tests::LiveVerificationStage::passed(
        crate::live_tests::checkpoints::STREAMING_CHAT_COMPLETION,
    )
    .with_duration_ms(started.elapsed().as_millis() as u64)
    .with_evidence("http_status", serde_json::json!(status.as_u16()))
    .with_evidence("chunk_count", serde_json::json!(chunk_count))
    .with_evidence("finish_reason", finish_reason)
    .with_evidence("matched_expected_content", serde_json::json!(true));
    if !usage.is_null() {
        stage = stage.with_evidence("usage", usage);
    }
    Ok(stage)
}

pub async fn run_live_openai_compatible_tool_smoke(
    profile: OpenAiCompatibleProfile,
    api_key: &str,
    model: &str,
) -> anyhow::Result<crate::live_tests::LiveVerificationStage> {
    let started = std::time::Instant::now();
    let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
    let url = format!(
        "{}/chat/completions",
        resolved.api_base.trim_end_matches('/')
    );
    let tool_name = "auth_tool_probe";
    let mut body = serde_json::json!({
        "model": model,
        "messages": [
            {"role": "user", "content": "Call the auth_tool_probe tool now. Do not answer in text."}
        ],
        "tools": [
            {
                "type": "function",
                "function": {
                    "name": tool_name,
                    "description": "A no-op live auth/tool-call smoke-test tool.",
                    "parameters": {
                        "type": "object",
                        "properties": {},
                        "additionalProperties": false
                    }
                }
            }
        ],
        "stream": false
    });
    set_output_token_cap(&mut body, &resolved, 256);
    if !resolved.api_base.contains("fptcloud.com") {
        body["tool_choice"] = serde_json::json!("auto");
    }
    let request = crate::provider::shared_http_client().post(&url).json(&body);
    let request = apply_provider_auth(request, &resolved, api_key);
    let response = tokio::time::timeout(std::time::Duration::from_secs(45), request.send())
        .await
        .context("timed out running live tool-call smoke completion")?
        .with_context(|| format!("run live {} tool-call smoke", resolved.display_name))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    ensure!(
        status.is_success(),
        "{} live tool-call smoke failed (HTTP {}): {}",
        resolved.display_name,
        status,
        text.trim()
    );
    let parsed: serde_json::Value = serde_json::from_str(&text).with_context(|| {
        format!(
            "parse live {} tool-call smoke response",
            resolved.display_name
        )
    })?;
    let tool_calls = parsed
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("tool_calls"))
        .and_then(|tool_calls| tool_calls.as_array())
        .cloned()
        .unwrap_or_default();
    ensure!(
        !tool_calls.is_empty(),
        "{} live tool-call smoke returned no tool calls: {}",
        resolved.display_name,
        crate::util::truncate_str(text.trim(), 1200)
    );
    let function = tool_calls[0]
        .get("function")
        .and_then(|function| function.as_object())
        .context("live tool-call smoke response missing function object")?;
    let returned_name = function
        .get("name")
        .and_then(|name| name.as_str())
        .unwrap_or_default();
    ensure!(
        returned_name == tool_name,
        "{} live tool-call smoke returned unexpected tool name {:?}",
        resolved.display_name,
        returned_name
    );
    let arguments = function
        .get("arguments")
        .and_then(|arguments| arguments.as_str())
        .context("live tool-call smoke response missing string arguments")?;
    let parsed_arguments = crate::message::ToolCall::parse_streamed_input_to_object(arguments);
    ensure!(
        parsed_arguments.is_object(),
        "{} live tool-call smoke returned non-object tool arguments: {:?}",
        resolved.display_name,
        arguments
    );
    let choice = parsed
        .get("choices")
        .and_then(|choices| choices.get(0))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let mut stage = crate::live_tests::LiveVerificationStage::passed(
        crate::live_tests::checkpoints::TOOL_CALL_PARSE,
    )
    .with_duration_ms(started.elapsed().as_millis() as u64)
    .with_evidence("http_status", serde_json::json!(status.as_u16()))
    .with_evidence("tool_name", serde_json::json!(returned_name))
    .with_evidence("tool_arguments", parsed_arguments)
    .with_evidence(
        "finish_reason",
        choice
            .get("finish_reason")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    );
    for key in ["id", "model", "usage", "cost"] {
        if let Some(value) = parsed.get(key) {
            stage = stage.with_evidence(key, value.clone());
        }
    }
    Ok(stage)
}
