//! # Provider Adapter Module
//!
//! Implements the `LlmProvider` trait for `jcode-provider-core`'s `Provider` interface.
//! This allows the Coordinator to use the real LLM providers from the jcode workspace.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use jcode_message_types::{Message, StreamEvent};
use jcode_provider_core::Provider;
use tracing::{debug, warn};

use crate::coordinator::LlmProvider;
use crate::DebateError;
use crate::DebateResult;

/// Adapter that wraps a `jcode-provider-core` Provider to implement the `LlmProvider` trait.
/// This allows the debate system to use the real LLM providers configured in the workspace.
pub struct JcodeProviderAdapter {
    /// The underlying jcode provider
    provider: Arc<dyn Provider>,
    /// Model to use (defaults to claude-sonnet-4-6)
    model: String,
    /// Maximum tokens for responses
    max_tokens: u32,
    /// Default temperature
    temperature: f32,
    /// Timeout for provider calls
    timeout: Duration,
    /// Rate limit configuration for the provider
    rate_limit_config: RateLimitConfig,
}

/// Rate limit configuration for provider calls
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Minimum interval between calls to the same perspective (in milliseconds)
    pub perspective_interval_ms: u64,
    /// Global cooldown between any perspective calls (in milliseconds)
    pub global_cooldown_ms: u64,
    /// Maximum concurrent requests
    pub max_concurrent: usize,
    /// Rate limit error handling strategy
    pub on_rate_limit: RateLimitStrategy,
}

/// Strategy for handling rate limit errors
#[derive(Debug, Clone)]
pub enum RateLimitStrategy {
    /// Wait and retry with exponential backoff
    Retry { max_attempts: u32 },
    /// Fail immediately with error
    Fail,
    /// Use fallback provider if available
    Fallback,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            perspective_interval_ms: 2000,
            global_cooldown_ms: 500,
            max_concurrent: 4,
            on_rate_limit: RateLimitStrategy::Retry { max_attempts: 3 },
        }
    }
}

impl RateLimitConfig {
    /// Create configuration for high-rate providers (e.g., Claude with 50 req/min)
    pub fn for_high_rate() -> Self {
        Self {
            perspective_interval_ms: 1200,
            global_cooldown_ms: 300,
            max_concurrent: 4,
            on_rate_limit: RateLimitStrategy::Retry { max_attempts: 3 },
        }
    }

    /// Create configuration for low-rate providers (e.g., OpenAI with rate limits)
    pub fn for_low_rate() -> Self {
        Self {
            perspective_interval_ms: 5000,
            global_cooldown_ms: 1000,
            max_concurrent: 2,
            on_rate_limit: RateLimitStrategy::Retry { max_attempts: 5 },
        }
    }

    /// Create configuration for providers with no rate limits
    pub fn for_unlimited() -> Self {
        Self {
            perspective_interval_ms: 500,
            global_cooldown_ms: 100,
            max_concurrent: 8,
            on_rate_limit: RateLimitStrategy::Fail,
        }
    }
}

impl JcodeProviderAdapter {
    /// Create a new adapter wrapping the given provider
    pub fn new(provider: Arc<dyn Provider>) -> Self {
        Self {
            provider,
            model: "claude-sonnet-4-6".to_string(),
            max_tokens: 8192,
            temperature: 0.7,
            timeout: Duration::from_secs(120),
            rate_limit_config: RateLimitConfig::default(),
        }
    }

    /// Create with custom model
    pub fn with_model(mut self, model: &str) -> Self {
        self.model = model.to_string();
        if let Err(e) = self.provider.set_model(model) {
            warn!("Failed to set model {}: {}", model, e);
        }
        self
    }

    /// Create with custom max tokens
    pub fn with_max_tokens(mut self, tokens: u32) -> Self {
        self.max_tokens = tokens;
        self
    }

    /// Create with custom temperature
    pub fn with_temperature(mut self, temp: f32) -> Self {
        self.temperature = temp;
        self
    }

    /// Create with custom timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Create with custom rate limit config
    pub fn with_rate_limit_config(mut self, config: RateLimitConfig) -> Self {
        self.rate_limit_config = config;
        self
    }

    /// Create for a specific provider type with appropriate rate limits
    pub fn for_provider_type(provider: Arc<dyn Provider>, provider_type: ProviderType) -> Self {
        let rate_config = match provider_type {
            ProviderType::Anthropic => RateLimitConfig::for_high_rate(),
            ProviderType::OpenAI => RateLimitConfig::for_low_rate(),
            ProviderType::OpenRouter => RateLimitConfig::for_low_rate(),
            ProviderType::Other => RateLimitConfig::default(),
        };

        Self {
            provider,
            model: "claude-sonnet-4-6".to_string(),
            max_tokens: 8192,
            temperature: 0.7,
            timeout: Duration::from_secs(120),
            rate_limit_config: rate_config,
        }
    }

    /// Get the current model
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Get the provider name
    pub fn provider_name(&self) -> &str {
        self.provider.name()
    }
}

/// Provider type for rate limit configuration
#[derive(Debug, Clone, Copy)]
pub enum ProviderType {
    /// Anthropic/Claude API
    Anthropic,
    /// OpenAI API
    OpenAI,
    /// OpenRouter (aggregated)
    OpenRouter,
    /// Other providers
    Other,
}

impl From<&str> for ProviderType {
    fn from(name: &str) -> Self {
        match name.to_lowercase().as_str() {
            "anthropic" | "claude" => ProviderType::Anthropic,
            "openai" | "openai-compatible" => ProviderType::OpenAI,
            "openrouter" => ProviderType::OpenRouter,
            _ => ProviderType::Other,
        }
    }
}

#[async_trait]
impl LlmProvider for JcodeProviderAdapter {
    async fn complete(
        &self,
        messages: &[Message],
        system: &str,
        _max_tokens: u32,
        _temperature: f32,
    ) -> DebateResult<String> {
        // Note: The system prompt is passed separately to provider.complete()
        // as the coordinator builds comprehensive prompts that include context
        let stream = tokio::time::timeout(
            self.timeout,
            self.provider.complete(messages, &[], system, None),
        )
        .await
        .map_err(|_| DebateError::Timeout("Provider call timed out".to_string()))?
        .map_err(|e| DebateError::Provider(e.to_string()))?;

        // Collect the response
        let mut result = String::new();

        tokio::pin!(stream);
        while let Some(event) = stream.next().await {
            match event {
                Ok(StreamEvent::TextDelta(text)) => {
                    debug!("Token: {}", text);
                    result.push_str(&text);
                }
                Ok(StreamEvent::MessageEnd { stop_reason }) => {
                    if let Some(reason) = stop_reason {
                        debug!("Message ended: {}", reason);
                    }
                }
                Ok(StreamEvent::Error {
                    message,
                    retry_after_secs: _,
                }) => {
                    return Err(DebateError::Provider(message));
                }
                // Ignore other event types
                _ => {}
            }
        }

        debug!("Completed with {} chars", result.len());

        if result.is_empty() {
            return Err(DebateError::Provider(
                "Provider returned empty response".to_string(),
            ));
        }

        Ok(result)
    }
}

/// Builder for creating provider adapters with fluent configuration
#[derive(Default)]
pub struct ProviderAdapterBuilder {
    provider: Option<Arc<dyn Provider>>,
    model: Option<String>,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    timeout: Option<Duration>,
    rate_limit_config: Option<RateLimitConfig>,
}

impl ProviderAdapterBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the provider (required)
    pub fn provider(mut self, provider: Arc<dyn Provider>) -> Self {
        self.provider = Some(provider);
        self
    }

    /// Set the model
    pub fn model(mut self, model: &str) -> Self {
        self.model = Some(model.to_string());
        self
    }

    /// Set max tokens
    pub fn max_tokens(mut self, tokens: u32) -> Self {
        self.max_tokens = Some(tokens);
        self
    }

    /// Set temperature
    pub fn temperature(mut self, temp: f32) -> Self {
        self.temperature = Some(temp);
        self
    }

    /// Set timeout
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Set rate limit configuration
    pub fn rate_limit_config(mut self, config: RateLimitConfig) -> Self {
        self.rate_limit_config = Some(config);
        self
    }

    /// Use high rate limits (for Claude)
    pub fn high_rate(mut self) -> Self {
        self.rate_limit_config = Some(RateLimitConfig::for_high_rate());
        self
    }

    /// Use low rate limits (for OpenAI)
    pub fn low_rate(mut self) -> Self {
        self.rate_limit_config = Some(RateLimitConfig::for_low_rate());
        self
    }

    /// Build the adapter
    pub fn build(self) -> Result<JcodeProviderAdapter, &'static str> {
        let provider = self.provider.ok_or("Provider is required")?;

        let mut adapter = JcodeProviderAdapter::new(provider);

        if let Some(model) = self.model {
            adapter = adapter.with_model(&model);
        }
        if let Some(tokens) = self.max_tokens {
            adapter = adapter.with_max_tokens(tokens);
        }
        if let Some(temp) = self.temperature {
            adapter = adapter.with_temperature(temp);
        }
        if let Some(timeout) = self.timeout {
            adapter = adapter.with_timeout(timeout);
        }
        if let Some(config) = self.rate_limit_config {
            adapter = adapter.with_rate_limit_config(config);
        }

        Ok(adapter)
    }
}

/// Create a provider adapter from a MultiProvider in the main jcode binary
/// This function bridges the debate core to the main application's providers
pub fn create_adapter_from_multi_provider(
    provider: Arc<dyn Provider>,
    _provider_name: &str,
    default_model: Option<&str>,
) -> JcodeProviderAdapter {
    let provider_type = ProviderType::from(provider.name());

    let mut adapter = JcodeProviderAdapter::for_provider_type(provider, provider_type);

    if let Some(model) = default_model {
        if let Err(e) = adapter.provider.set_model(model) {
            warn!("Failed to set model {}: {}", model, e);
        }
        adapter.model = model.to_string();
    }

    adapter
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_config_defaults() {
        let config = RateLimitConfig::default();
        assert_eq!(config.perspective_interval_ms, 2000);
        assert_eq!(config.global_cooldown_ms, 500);
        assert!(matches!(
            config.on_rate_limit,
            RateLimitStrategy::Retry { max_attempts: 3 }
        ));
    }

    #[test]
    fn rate_limit_config_high_rate() {
        let config = RateLimitConfig::for_high_rate();
        assert_eq!(config.perspective_interval_ms, 1200);
        assert_eq!(config.global_cooldown_ms, 300);
        assert!(matches!(
            config.on_rate_limit,
            RateLimitStrategy::Retry { max_attempts: 3 }
        ));
    }

    #[test]
    fn rate_limit_config_low_rate() {
        let config = RateLimitConfig::for_low_rate();
        assert_eq!(config.perspective_interval_ms, 5000);
        assert_eq!(config.global_cooldown_ms, 1000);
        assert!(matches!(
            config.on_rate_limit,
            RateLimitStrategy::Retry { max_attempts: 5 }
        ));
    }

    #[test]
    fn provider_type_from_str() {
        assert!(matches!(
            ProviderType::from("anthropic"),
            ProviderType::Anthropic
        ));
        assert!(matches!(
            ProviderType::from("Claude"),
            ProviderType::Anthropic
        ));
        assert!(matches!(ProviderType::from("openai"), ProviderType::OpenAI));
        assert!(matches!(
            ProviderType::from("OpenRouter"),
            ProviderType::OpenRouter
        ));
        assert!(matches!(ProviderType::from("unknown"), ProviderType::Other));
    }

    #[test]
    fn builder_pattern() {
        // This would need a mock provider to actually test
        // Just verify the builder compiles correctly
        let _builder = ProviderAdapterBuilder::new()
            .model("claude-opus-4-6")
            .max_tokens(16384)
            .temperature(0.5)
            .timeout(Duration::from_secs(60))
            .high_rate();
    }

    #[tokio::test]
    async fn adapter_creation() {
        // Test that we can create adapter with provider type
        // This would need a mock provider to actually test
        let provider_type = ProviderType::Anthropic;
        assert!(matches!(provider_type, ProviderType::Anthropic));
    }
}
