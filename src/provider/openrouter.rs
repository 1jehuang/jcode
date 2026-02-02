//! OpenRouter API provider
//!
//! Uses OpenRouter's OpenAI-compatible API to access 200+ models from various providers.
//! Models are fetched dynamically from the API and cached to disk.
//!
//! Features:
//! - Provider pinning: Set JCODE_OPENROUTER_PROVIDER to pin to a specific provider (e.g., "Fireworks")
//! - Cache token parsing: Parses cached_tokens from OpenRouter responses for cache hit detection

use super::{EventStream, Provider};
use crate::message::{ContentBlock, Message, Role, StreamEvent, ToolDefinition};
use anyhow::{Context, Result};
use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};
use tokio::sync::RwLock;

/// OpenRouter API base URL
const API_BASE: &str = "https://openrouter.ai/api/v1";

/// Default model (Claude Sonnet via OpenRouter)
const DEFAULT_MODEL: &str = "anthropic/claude-sonnet-4";

fn model_requires_reasoning_content(model: &str) -> bool {
    let lower = model.to_lowercase();
    lower.contains("moonshot") || lower.contains("kimi")
}

/// Cache TTL in seconds (24 hours)
const CACHE_TTL_SECS: u64 = 24 * 60 * 60;

/// Model info from OpenRouter API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub context_length: Option<u64>,
    #[serde(default)]
    pub pricing: ModelPricing,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelPricing {
    pub prompt: Option<String>,
    pub completion: Option<String>,
}

/// Disk cache structure
#[derive(Debug, Serialize, Deserialize)]
struct DiskCache {
    /// Unix timestamp when cache was written
    cached_at: u64,
    /// Cached models
    models: Vec<ModelInfo>,
}

/// In-memory cache
#[derive(Debug, Default)]
struct ModelsCache {
    models: Vec<ModelInfo>,
    fetched: bool,
}

/// Get the cache file path
fn cache_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".jcode")
        .join("cache")
        .join("openrouter_models.json")
}

/// Load models from disk cache if valid
fn load_disk_cache() -> Option<Vec<ModelInfo>> {
    let path = cache_path();
    let content = std::fs::read_to_string(&path).ok()?;
    let cache: DiskCache = serde_json::from_str(&content).ok()?;

    // Check if cache is still valid
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();

    if now - cache.cached_at < CACHE_TTL_SECS {
        Some(cache.models)
    } else {
        None
    }
}

/// Save models to disk cache
fn save_disk_cache(models: &[ModelInfo]) {
    let path = cache_path();

    // Create cache directory if needed
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let cache = DiskCache {
        cached_at: now,
        models: models.to_vec(),
    };

    if let Ok(content) = serde_json::to_string(&cache) {
        let _ = std::fs::write(&path, content);
    }
}

/// Provider routing configuration
#[derive(Debug, Clone, Default)]
pub struct ProviderRouting {
    /// List of provider slugs to try in order (e.g., ["Fireworks", "Together"])
    pub order: Option<Vec<String>>,
    /// Whether to allow fallbacks to other providers (default: true)
    pub allow_fallbacks: bool,
}

pub struct OpenRouterProvider {
    client: Client,
    model: Arc<RwLock<String>>,
    api_key: String,
    models_cache: Arc<RwLock<ModelsCache>>,
    /// Provider routing preferences
    provider_routing: Arc<RwLock<ProviderRouting>>,
}

impl OpenRouterProvider {
    pub fn new() -> Result<Self> {
        let api_key = Self::get_api_key().ok_or_else(|| {
            anyhow::anyhow!(
                "OPENROUTER_API_KEY not found in environment or ~/.config/jcode/openrouter.env"
            )
        })?;

        let model =
            std::env::var("JCODE_OPENROUTER_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());

        // Parse provider routing from environment
        let provider_routing = Self::parse_provider_routing();

        Ok(Self {
            client: Client::new(),
            model: Arc::new(RwLock::new(model)),
            api_key,
            models_cache: Arc::new(RwLock::new(ModelsCache::default())),
            provider_routing: Arc::new(RwLock::new(provider_routing)),
        })
    }

    /// Parse provider routing configuration from environment variables
    fn parse_provider_routing() -> ProviderRouting {
        let mut routing = ProviderRouting {
            order: None,
            allow_fallbacks: true,
        };

        // JCODE_OPENROUTER_PROVIDER: comma-separated list of providers to prefer
        // e.g., "Fireworks" or "Fireworks,Together"
        if let Ok(providers) = std::env::var("JCODE_OPENROUTER_PROVIDER") {
            let order: Vec<String> = providers
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !order.is_empty() {
                routing.order = Some(order);
            }
        }

        // JCODE_OPENROUTER_NO_FALLBACK: disable fallbacks to other providers
        if std::env::var("JCODE_OPENROUTER_NO_FALLBACK").is_ok() {
            routing.allow_fallbacks = false;
        }

        routing
    }

    /// Set provider routing at runtime
    pub async fn set_provider_routing(&self, routing: ProviderRouting) {
        let mut current = self.provider_routing.write().await;
        *current = routing;
    }

    /// Get current provider routing
    pub async fn get_provider_routing(&self) -> ProviderRouting {
        self.provider_routing.read().await.clone()
    }

    /// Check if OPENROUTER_API_KEY is available (env var or config file)
    pub fn has_credentials() -> bool {
        Self::get_api_key().is_some()
    }

    /// Get API key from environment or config file
    fn get_api_key() -> Option<String> {
        // First check environment variable
        if let Ok(key) = std::env::var("OPENROUTER_API_KEY") {
            return Some(key);
        }

        // Fall back to config file
        let config_path = dirs::config_dir()?.join("jcode").join("openrouter.env");
        let content = std::fs::read_to_string(config_path).ok()?;

        for line in content.lines() {
            if let Some(key) = line.strip_prefix("OPENROUTER_API_KEY=") {
                let key = key.trim().trim_matches('"').trim_matches('\'');
                if !key.is_empty() {
                    return Some(key.to_string());
                }
            }
        }

        None
    }

    /// Fetch available models from OpenRouter API (with disk caching)
    pub async fn fetch_models(&self) -> Result<Vec<ModelInfo>> {
        // Check in-memory cache first
        {
            let cache = self.models_cache.read().await;
            if cache.fetched {
                return Ok(cache.models.clone());
            }
        }

        // Check disk cache
        if let Some(models) = load_disk_cache() {
            let mut cache = self.models_cache.write().await;
            cache.models = models.clone();
            cache.fetched = true;
            return Ok(models);
        }

        // Fetch from API
        let url = format!("{}/models", API_BASE);
        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await
            .context("Failed to fetch models from OpenRouter")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("OpenRouter API error ({}): {}", status, body);
        }

        #[derive(Deserialize)]
        struct ModelsResponse {
            data: Vec<ModelInfo>,
        }

        let models_response: ModelsResponse = response
            .json()
            .await
            .context("Failed to parse models response")?;

        // Save to disk cache
        save_disk_cache(&models_response.data);

        // Update in-memory cache
        {
            let mut cache = self.models_cache.write().await;
            cache.models = models_response.data.clone();
            cache.fetched = true;
        }

        Ok(models_response.data)
    }

    /// Force refresh the models cache from API
    pub async fn refresh_models(&self) -> Result<Vec<ModelInfo>> {
        // Clear in-memory cache
        {
            let mut cache = self.models_cache.write().await;
            cache.fetched = false;
            cache.models.clear();
        }

        // Delete disk cache
        let _ = std::fs::remove_file(cache_path());

        // Fetch fresh
        self.fetch_models().await
    }

    /// Get context length for a model
    pub async fn context_length_for_model(&self, model_id: &str) -> Option<u64> {
        if let Ok(models) = self.fetch_models().await {
            models
                .iter()
                .find(|m| m.id == model_id)
                .and_then(|m| m.context_length)
        } else {
            None
        }
    }
}

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
        let needs_reasoning_content = model_requires_reasoning_content(&model);

        // Build messages in OpenAI format
        let mut api_messages = Vec::new();

        // Add system message if provided
        if !system.is_empty() {
            api_messages.push(serde_json::json!({
                "role": "system",
                "content": system
            }));
        }

        // Convert messages
        for msg in messages {
            match msg.role {
                Role::User => {
                    for block in &msg.content {
                        match block {
                            ContentBlock::Text { text, .. } => {
                                api_messages.push(serde_json::json!({
                                    "role": "user",
                                    "content": text
                                }));
                            }
                            ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                is_error,
                            } => {
                                let output = if is_error == &Some(true) {
                                    format!("[Error] {}", content)
                                } else {
                                    content.clone()
                                };
                                api_messages.push(serde_json::json!({
                                    "role": "tool",
                                    "tool_call_id": tool_use_id,
                                    "content": output
                                }));
                            }
                            _ => {}
                        }
                    }
                }
                Role::Assistant => {
                    let mut text_content = String::new();
                    let mut tool_calls = Vec::new();
                    let mut reasoning_content = String::new();

                    for block in &msg.content {
                        match block {
                            ContentBlock::Text { text, .. } => {
                                text_content.push_str(text);
                            }
                            ContentBlock::Reasoning { text } => {
                                reasoning_content.push_str(text);
                            }
                            ContentBlock::ToolUse { id, name, input } => {
                                tool_calls.push(serde_json::json!({
                                    "id": id,
                                    "type": "function",
                                    "function": {
                                        "name": name,
                                        "arguments": serde_json::to_string(input).unwrap_or_default()
                                    }
                                }));
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

                    if !reasoning_content.is_empty()
                        || (needs_reasoning_content && !tool_calls.is_empty())
                    {
                        assistant_msg["reasoning_content"] = serde_json::json!(reasoning_content);
                    }

                    if !text_content.is_empty() || !tool_calls.is_empty() {
                        api_messages.push(assistant_msg);
                    }
                }
            }
        }

        // Build tools in OpenAI format
        let api_tools: Vec<Value> = tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
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

        // Add provider routing if configured
        let routing = self.provider_routing.read().await;
        if routing.order.is_some() || !routing.allow_fallbacks {
            let mut provider_obj = serde_json::json!({});
            if let Some(ref order) = routing.order {
                provider_obj["order"] = serde_json::json!(order);
            }
            if !routing.allow_fallbacks {
                provider_obj["allow_fallbacks"] = serde_json::json!(false);
            }
            request["provider"] = provider_obj;
        }
        drop(routing);

        // Send request
        let url = format!("{}/chat/completions", API_BASE);
        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .header("HTTP-Referer", "https://github.com/jcode")
            .header("X-Title", "jcode")
            .json(&request)
            .send()
            .await
            .context("Failed to send request to OpenRouter")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("OpenRouter API error ({}): {}", status, body);
        }

        let stream = OpenRouterStream::new(response.bytes_stream());
        Ok(Box::pin(stream))
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

    fn set_model(&self, model: &str) -> Result<()> {
        // OpenRouter accepts any model ID - validation happens at API call time
        // This allows using any model without needing to pre-fetch the list
        if let Ok(mut current) = self.model.try_write() {
            *current = model.to_string();
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Cannot change model while a request is in progress"
            ))
        }
    }

    fn available_models(&self) -> Vec<&'static str> {
        // OpenRouter models are fetched dynamically from the API.
        // Static list is empty; use available_models_display for cached list.
        vec![]
    }

    fn available_models_display(&self) -> Vec<String> {
        if let Ok(cache) = self.models_cache.try_read() {
            if cache.fetched && !cache.models.is_empty() {
                return cache.models.iter().map(|m| m.id.clone()).collect();
            }
        }

        if let Some(models) = load_disk_cache() {
            if let Ok(mut cache) = self.models_cache.try_write() {
                cache.models = models.clone();
                cache.fetched = true;
            }
            return models.into_iter().map(|m| m.id).collect();
        }

        Vec::new()
    }

    async fn prefetch_models(&self) -> Result<()> {
        let _ = self.fetch_models().await?;
        Ok(())
    }

    fn supports_compaction(&self) -> bool {
        true
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            client: self.client.clone(),
            model: Arc::new(RwLock::new(
                self.model.try_read().map(|m| m.clone()).unwrap_or_default(),
            )),
            api_key: self.api_key.clone(),
            models_cache: Arc::clone(&self.models_cache),
            provider_routing: Arc::new(RwLock::new(
                self.provider_routing
                    .try_read()
                    .map(|r| r.clone())
                    .unwrap_or_default(),
            )),
        })
    }
}

// ============================================================================
// SSE Stream Parser
// ============================================================================

struct OpenRouterStream {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    buffer: String,
    pending: VecDeque<StreamEvent>,
    current_tool_call: Option<ToolCallAccumulator>,
    /// Track if we've emitted the provider info (only emit once)
    provider_emitted: bool,
}

#[derive(Default)]
struct ToolCallAccumulator {
    id: String,
    name: String,
    arguments: String,
}

impl OpenRouterStream {
    fn new(stream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static) -> Self {
        Self {
            inner: Box::pin(stream),
            buffer: String::new(),
            pending: VecDeque::new(),
            current_tool_call: None,
            provider_emitted: false,
        }
    }

    fn parse_next_event(&mut self) -> Option<StreamEvent> {
        if let Some(event) = self.pending.pop_front() {
            return Some(event);
        }

        while let Some(pos) = self.buffer.find("\n\n") {
            let event_str = self.buffer[..pos].to_string();
            self.buffer = self.buffer[pos + 2..].to_string();

            // Parse SSE event
            let mut data = None;
            for line in event_str.lines() {
                if let Some(d) = line.strip_prefix("data: ") {
                    data = Some(d);
                }
            }

            let data = match data {
                Some(d) => d,
                None => continue,
            };

            if data == "[DONE]" {
                return Some(StreamEvent::MessageEnd { stop_reason: None });
            }

            let parsed: Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(_) => continue,
            };

            // Extract upstream provider info (only emit once)
            // OpenRouter returns "provider" field indicating which provider handled the request
            if !self.provider_emitted {
                if let Some(provider) = parsed.get("provider").and_then(|p| p.as_str()) {
                    self.provider_emitted = true;
                    self.pending.push_back(StreamEvent::UpstreamProvider {
                        provider: provider.to_string(),
                    });
                }
            }

            // Check for error
            if let Some(error) = parsed.get("error") {
                let message = error
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("OpenRouter error")
                    .to_string();
                return Some(StreamEvent::Error {
                    message,
                    retry_after_secs: None,
                });
            }

            // Parse choices
            if let Some(choices) = parsed.get("choices").and_then(|c| c.as_array()) {
                for choice in choices {
                    let delta = match choice.get("delta") {
                        Some(d) => d,
                        None => continue,
                    };

                    // Reasoning/thinking content (provider-specific)
                    let reasoning_delta = delta
                        .get("reasoning_content")
                        .and_then(|c| c.as_str())
                        .or_else(|| delta.get("reasoning").and_then(|c| c.as_str()))
                        .or_else(|| delta.get("thinking").and_then(|c| c.as_str()));
                    if let Some(reasoning) = reasoning_delta {
                        if !reasoning.is_empty() {
                            self.pending
                                .push_back(StreamEvent::ThinkingDelta(reasoning.to_string()));
                        }
                    }

                    // Text content
                    if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                        if !content.is_empty() {
                            return Some(StreamEvent::TextDelta(content.to_string()));
                        }
                    }

                    // Tool calls
                    if let Some(tool_calls) = delta.get("tool_calls").and_then(|t| t.as_array()) {
                        for tc in tool_calls {
                            let index = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0);

                            // Check if this is a new tool call
                            if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                                // Emit previous tool call if any
                                if let Some(prev) = self.current_tool_call.take() {
                                    if !prev.id.is_empty() {
                                        self.pending.push_back(StreamEvent::ToolUseStart {
                                            id: prev.id,
                                            name: prev.name,
                                        });
                                        self.pending
                                            .push_back(StreamEvent::ToolInputDelta(prev.arguments));
                                        self.pending.push_back(StreamEvent::ToolUseEnd);
                                    }
                                }

                                let name = tc
                                    .get("function")
                                    .and_then(|f| f.get("name"))
                                    .and_then(|n| n.as_str())
                                    .unwrap_or("")
                                    .to_string();

                                self.current_tool_call = Some(ToolCallAccumulator {
                                    id: id.to_string(),
                                    name,
                                    arguments: String::new(),
                                });
                            }

                            // Accumulate arguments
                            if let Some(args) = tc
                                .get("function")
                                .and_then(|f| f.get("arguments"))
                                .and_then(|a| a.as_str())
                            {
                                if let Some(ref mut tc) = self.current_tool_call {
                                    tc.arguments.push_str(args);
                                }
                            }
                        }
                    }

                    // Check for finish reason
                    if let Some(finish_reason) =
                        choice.get("finish_reason").and_then(|f| f.as_str())
                    {
                        // Emit any pending tool call
                        if let Some(tc) = self.current_tool_call.take() {
                            if !tc.id.is_empty() {
                                self.pending.push_back(StreamEvent::ToolUseStart {
                                    id: tc.id,
                                    name: tc.name,
                                });
                                self.pending
                                    .push_back(StreamEvent::ToolInputDelta(tc.arguments));
                                self.pending.push_back(StreamEvent::ToolUseEnd);
                            }
                        }

                        // Don't emit MessageEnd here - wait for [DONE]
                    }
                }
            }

            // Extract usage if present
            if let Some(usage) = parsed.get("usage") {
                let input_tokens = usage.get("prompt_tokens").and_then(|t| t.as_u64());
                let output_tokens = usage.get("completion_tokens").and_then(|t| t.as_u64());

                // OpenRouter returns cached tokens in various formats depending on provider:
                // - "cached_tokens" (OpenRouter's unified field)
                // - "prompt_tokens_details.cached_tokens" (OpenAI-style)
                // - "cache_read_input_tokens" (Anthropic-style, passed through)
                let cache_read_input_tokens = usage
                    .get("cached_tokens")
                    .and_then(|t| t.as_u64())
                    .or_else(|| {
                        usage
                            .get("prompt_tokens_details")
                            .and_then(|d| d.get("cached_tokens"))
                            .and_then(|t| t.as_u64())
                    })
                    .or_else(|| {
                        usage
                            .get("cache_read_input_tokens")
                            .and_then(|t| t.as_u64())
                    });

                // Cache creation tokens (Anthropic-style, passed through for some providers)
                let cache_creation_input_tokens = usage
                    .get("cache_creation_input_tokens")
                    .and_then(|t| t.as_u64());

                if input_tokens.is_some()
                    || output_tokens.is_some()
                    || cache_read_input_tokens.is_some()
                {
                    self.pending.push_back(StreamEvent::TokenUsage {
                        input_tokens,
                        output_tokens,
                        cache_read_input_tokens,
                        cache_creation_input_tokens,
                    });
                }
            }

            if let Some(event) = self.pending.pop_front() {
                return Some(event);
            }
        }

        None
    }
}

impl Stream for OpenRouterStream {
    type Item = Result<StreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(event) = self.parse_next_event() {
                return Poll::Ready(Some(Ok(event)));
            }

            match self.inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(bytes))) => {
                    if let Ok(text) = std::str::from_utf8(&bytes) {
                        self.buffer.push_str(text);
                    }
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Some(Err(anyhow::anyhow!("Stream error: {}", e))));
                }
                Poll::Ready(None) => {
                    // Stream ended - emit any pending tool call
                    if let Some(tc) = self.current_tool_call.take() {
                        if !tc.id.is_empty() {
                            self.pending.push_back(StreamEvent::ToolUseStart {
                                id: tc.id,
                                name: tc.name,
                            });
                            self.pending
                                .push_back(StreamEvent::ToolInputDelta(tc.arguments));
                            self.pending.push_back(StreamEvent::ToolUseEnd);
                        }
                    }
                    if let Some(event) = self.pending.pop_front() {
                        return Poll::Ready(Some(Ok(event)));
                    }
                    return Poll::Ready(None);
                }
                Poll::Pending => {
                    return Poll::Pending;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_has_credentials() {
        // has_credentials() checks both env var AND config file
        // So we just verify it returns a boolean without panicking
        let _has_creds = OpenRouterProvider::has_credentials();
        // If we got here, the function works
    }
}
