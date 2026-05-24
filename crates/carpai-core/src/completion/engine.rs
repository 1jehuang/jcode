//! Completion Engine — Multi-provider abstraction layer
//!
//! Provides a unified `CompletionEngine` that wraps multiple completion providers
//! with automatic fallback chaining. Integrates with `jcode-completion` through
//! an abstraction trait, and supports local (Ollama) and cloud providers.
//!
//! ## Provider Architecture
//!
//! ```text
//! CompletionEngine
//!   ├── LocalCompletionProvider  (Ollama / llama.cpp / OpenAI-compatible)
//!   ├── JcodeCompletionProvider  (jcode-completion crate integration stub)
//!   └── Fallback chain: provider[0] → provider[1] → ... → error
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::config::{CoreConfig, ProviderConfig};

// ========================================================================
// Types
// ========================================================================

/// Internal completion request used between engine and providers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionRequestInternal {
    pub file_path: String,
    pub content: String,
    pub cursor_offset: usize,
    pub language: String,
    pub max_tokens: u32,
    pub temperature: f64,
}

/// Internal completion response from a provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionResponseInternal {
    pub text: String,
    pub score: f64,
    pub provider_name: String,
    pub latency_ms: u64,
}

/// Final output from the CompletionEngine
#[derive(Debug, Clone)]
pub struct CompletionOutput {
    pub candidates: Vec<CompletionCandidateOutput>,
    pub provider_used: String,
    pub total_latency_ms: u64,
}

/// A single candidate in the final output
#[derive(Debug, Clone)]
pub struct CompletionCandidateOutput {
    pub text: String,
    pub score: f64,
}

// ========================================================================
// CompletionProvider Trait
// ========================================================================

/// Abstraction over different completion backends
///
/// Implement this trait to add new providers (e.g., Anthropic, Google, custom).
/// The engine will try providers in order until one succeeds.
#[async_trait]
pub trait CompletionProvider: Send + Sync {
    /// Generate a completion for the given request
    async fn complete(
        &self,
        req: &CompletionRequestInternal,
    ) -> Result<CompletionResponseInternal, CompletionEngineError>;

    /// Human-readable name of this provider (for logging/metrics)
    fn name(&self) -> &str;

    /// Check if this provider is available (quick health check)
    async fn is_available(&self) -> bool {
        true
    }
}

// ========================================================================
// Errors
// ========================================================================

#[derive(Debug, thiserror::Error)]
pub enum CompletionEngineError {
    #[error("All {0} providers failed")]
    AllProvidersFailed(usize),

    #[error("Provider '{provider}' error: {message}")]
    ProviderError { provider: String, message: String },

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Timeout after {0}ms")]
    Timeout(u64),

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

// ========================================================================
// LocalCompletionProvider — Ollama / OpenAI-compatible API
// ========================================================================

/// Local completion provider using Ollama or any OpenAI-compatible endpoint
///
/// Supports:
/// - Ollama's `/v1/completions` endpoint (for FIM models)
/// - Ollama's `/v1/chat/completions` endpoint (for chat models)
/// - Any OpenAI-compatible API (LM Studio, vLLM, etc.)
pub struct LocalCompletionProvider {
    client: Client,
    endpoint: String,
    model: String,
    timeout: Duration,
    api_key: Option<String>,
}

impl LocalCompletionProvider {
    pub fn new(config: &ProviderConfig) -> Self {
        let endpoint = config
            .endpoint
            .as_deref()
            .unwrap_or("http://localhost:11434");

        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(config.timeout_secs))
                .build()
                .expect("Failed to build HTTP client for LocalCompletionProvider"),
            endpoint: endpoint.to_string(),
            model: config
                .model
                .clone()
                .unwrap_or_else(|| "default".to_string()),
            timeout: Duration::from_secs(config.timeout_secs),
            api_key: config.api_key.clone(),
        }
    }

    pub fn with_endpoint(endpoint: impl Into<String>, model: impl Into<String>) -> Self {
        let ep = endpoint.into();
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("Failed to build HTTP client"),
            endpoint: ep.clone(),
            model: model.into(),
            timeout: Duration::from_secs(30),
            api_key: None,
        }
    }

    async fn send_fim_request(
        &self,
        fim_prompt: &str,
        max_tokens: u32,
        temperature: f64,
    ) -> Result<Option<String>, CompletionEngineError> {
        let body = serde_json::json!({
            "prompt": fim_prompt,
            "model": self.model,
            "max_tokens": max_tokens.min(128),
            "temperature": temperature,
            "stop": ["<|fim_end|>", "\n\n\n", "```"],
        });

        let url = format!("{}/v1/completions", self.endpoint);
        let mut req_builder = self.client.post(&url).json(&body);

        if let Some(ref key) = self.api_key {
            req_builder = req_builder.header("Authorization", format!("Bearer {}", key));
        }

        let start = Instant::now();
        let resp = req_builder.send().await?;
        let latency = start.elapsed().as_millis() as u64;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            warn!(%status, body = %text, provider = %self.endpoint, "FIM API error");
            return Err(CompletionEngineError::ProviderError {
                provider: self.name().to_string(),
                message: format!("HTTP {}: {}", status, text),
            });
        }

        let data: serde_json::Value = resp.json().await?;
        debug!(latency_ms = latency, provider = %self.name(), "FIM completion done");

        Ok(data["choices"][0]["text"].as_str().map(|s| s.to_string()))
    }
}

#[async_trait]
impl CompletionProvider for LocalCompletionProvider {
    async fn complete(
        &self,
        req: &CompletionRequestInternal,
    ) -> Result<CompletionResponseInternal, CompletionEngineError> {
        let fim_prompt = format!(
            "<|fim_prefix|>{}<|fim_suffix|>{}<|fim_middle|>",
            req.content[..req.cursor_offset.min(req.content.len())].to_string(),
            &req.content[req.cursor_offset.min(req.content.len())..]
        );

        let start = Instant::now();

        match self.send_fim_request(&fim_prompt, req.max_tokens, req.temperature).await {
            Ok(Some(text)) => {
                let latency = start.elapsed().as_millis() as u64;
                Ok(CompletionResponseInternal {
                    text,
                    score: 1.0,
                    provider_name: self.name().to_string(),
                    latency_ms: latency,
                })
            }
            Ok(None) => Err(CompletionEngineError::ProviderError {
                provider: self.name().to_string(),
                message: "Empty response from FIM API".to_string(),
            }),
            Err(e) => Err(e),
        }
    }

    fn name(&self) -> &str {
        "local-ollama"
    }

    async fn is_available(&self) -> bool {
        let url = format!("{}/api/tags", self.endpoint);
        match self.client.get(&url).timeout(Duration::from_secs(2)).send().await {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        }
    }
}

// ========================================================================
// JcodeCompletionProvider — jcode-completion integration stub
// ========================================================================
//
// NOTE: This is a stub/documentation placeholder. The real jcode-completion
// crate provides multi-provider completion with caching, quality scoring,
// and LSP integration. When carpai-core gains a direct dependency on
// jcode-completion, replace this stub with the real implementation.
//
// Integration points:
//   - jcode_completion::CodeCompletionEngine (main entry)
//   - jcode_completion::providers::CompletionProviderConfig (provider config)
//   - jcode_completion::cache::CompletionCache (response caching)
//   - jcode_completion::quality::QualityScorer (candidate ranking)

/// Stub provider documenting jcode-completion integration
///
/// In production, this would wrap `jcode_completion::CodeCompletionEngine`
/// and delegate to its multi-provider pipeline (Ollama → OpenAI → Anthropic).
pub struct JcodeCompletionProvider {
    _endpoint: String,
    _model: String,
}

impl JcodeCompletionProvider {
    pub fn new(endpoint: &str, model: &str) -> Self {
        Self {
            _endpoint: endpoint.to_string(),
            _model: model.to_string(),
        }
    }
}

#[async_trait]
impl CompletionProvider for JcodeCompletionProvider {
    async fn complete(
        &self,
        _req: &CompletionRequestInternal,
    ) -> Result<CompletionResponseInternal, CompletionEngineError> {
        Err(CompletionEngineError::ProviderError {
            provider: self.name().to_string(),
            message: "JcodeCompletionProvider is a stub — integrate jcode-completion crate for production use".to_string(),
        })
    }

    fn name(&self) -> &str {
        "jcode-completion"
    }

    async fn is_available(&self) -> bool {
        false
    }
}

// ========================================================================
// CompletionEngine — Main orchestrator
// ========================================================================

/// Multi-provider completion engine with fallback chain
///
/// Tries each provider in order; on failure, falls back to the next.
/// Reports metrics and supports both inline (cursor-position) and FIM completions.
pub struct CompletionEngine {
    providers: Vec<Arc<dyn CompletionProvider>>,
    default_max_tokens: u32,
    default_temperature: f64,
}

impl CompletionEngine {
    /// Create a new CompletionEngine from CoreConfig
    ///
    /// Automatically configures providers based on `config.completion_provider`.
    /// If provider_type is "local", adds a LocalCompletionProvider.
    /// Additional providers can be added via `add_provider()`.
    pub fn new(config: &CoreConfig) -> Self {
        let mut providers: Vec<Arc<dyn CompletionProvider>> = Vec::new();

        match config.completion_provider.provider_type.as_str() {
            "local" | "ollama" | "" => {
                info!(
                    endpoint = ?config.completion_provider.endpoint,
                    model = ?config.completion_provider.model,
                    "Creating LocalCompletionProvider"
                );
                providers.push(Arc::new(LocalCompletionProvider::new(
                    &config.completion_provider,
                )));
            }
            "jcode" => {
                info!("Creating JcodeCompletionProvider (stub)");
                let endpoint = config
                    .completion_provider
                    .endpoint
                    .as_deref()
                    .unwrap_or("http://localhost:8080");
                let model = config
                    .completion_provider
                    .model
                    .as_deref()
                    .unwrap_or("default");
                providers.push(Arc::new(JcodeCompletionProvider::new(endpoint, model)));
            }
            other => {
                warn!(provider_type = other, "Unknown provider type, falling back to local");
                providers.push(Arc::new(LocalCompletionProvider::new(
                    &config.completion_provider,
                )));
            }
        }

        Self {
            providers,
            default_max_tokens: 64,
            default_temperature: 0.5,
        }
    }

    /// Add an additional provider to the fallback chain (appended last)
    pub fn add_provider(mut self, provider: Arc<dyn CompletionProvider>) -> Self {
        self.providers.push(provider);
        self
    }

    /// Execute inline code completion at cursor position
    ///
    /// This is the main entry point for IDE/TUI integration.
    /// Tries each provider in order, returns the first successful result.
    pub async fn complete(
        &self,
        file_path: &str,
        content: &str,
        cursor_offset: usize,
        language: &str,
    ) -> Result<CompletionOutput, CompletionEngineError> {
        let req = CompletionRequestInternal {
            file_path: file_path.to_string(),
            content: content.to_string(),
            cursor_offset,
            language: language.to_string(),
            max_tokens: self.default_max_tokens,
            temperature: self.default_temperature,
        };

        self.complete_internal(req).await
    }

    /// Execute FIM (Fill-in-the-Middle) completion
    ///
    /// Explicit prefix/suffix mode for maximum control over context window.
    pub async fn complete_fim(
        &self,
        prefix: &str,
        suffix: &str,
        file_path: &str,
    ) -> Result<crate::completion::quality::FimCompletionResponse, CompletionEngineError> {
        let content = format!("{}{}", prefix, suffix);
        let cursor_offset = prefix.len();

        let req = CompletionRequestInternal {
            file_path: file_path.to_string(),
            content,
            cursor_offset,
            language: detect_language(file_path),
            max_tokens: self.default_max_tokens,
            temperature: self.default_temperature,
        };

        let output = self.complete_internal(req).await?;

        Ok(crate::completion::quality::FimCompletionResponse {
            items: output
                .candidates
                .into_iter()
                .map(|c| crate::completion::quality::FimCandidate {
                    text: c.text,
                    score: c.score,
                    syntax_valid: true,
                    prefix_overlap: String::new(),
                })
                .collect(),
        })
    }

    /// Internal: try all providers in fallback order
    async fn complete_internal(
        &self,
        req: CompletionRequestInternal,
    ) -> Result<CompletionOutput, CompletionEngineError> {
        let start = Instant::now();
        let mut errors: Vec<String> = Vec::new();

        for provider in &self.providers {
            if !provider.is_available().await {
                debug!(provider = provider.name(), "Skipping unavailable provider");
                continue;
            }

            match provider.complete(&req).await {
                Ok(resp) => {
                    info!(
                        provider = provider.name(),
                        latency_ms = resp.latency_ms,
                        score = resp.score,
                        "Completion successful"
                    );
                    return Ok(CompletionOutput {
                        candidates: vec![CompletionCandidateOutput {
                            text: resp.text,
                            score: resp.score,
                        }],
                        provider_used: resp.provider_name,
                        total_latency_ms: start.elapsed().as_millis() as u64,
                    });
                }
                Err(e) => {
                    warn!(provider = provider.name(), error = %e, "Provider failed, trying next");
                    errors.push(format!("{}: {}", provider.name(), e));
                }
            }
        }

        Err(CompletionEngineError::AllProvidersFailed(self.providers.len()))
    }

    /// Check if any provider is ready
    pub async fn is_ready(&self) -> bool {
        for provider in &self.providers {
            if provider.is_available().await {
                return true;
            }
        }
        false
    }

    /// List configured provider names (for diagnostics)
    pub fn provider_names(&self) -> Vec<&str> {
        self.providers.iter().map(|p| p.name()).collect()
    }
}

fn detect_language(file_path: &str) -> String {
    let ext = std::path::Path::new(file_path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    match ext {
        "rs" => "rust".into(),
        "ts" | "tsx" => "typescript".into(),
        "js" | "jsx" => "javascript".into(),
        "py" => "python".into(),
        "go" => "go".into(),
        "java" => "java".into(),
        _ => ext.to_string(),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock provider for testing
    struct MockCompletionProvider {
        name: String,
        should_fail: bool,
        response_text: String,
    }

    impl MockCompletionProvider {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                should_fail: false,
                response_text: "fn hello() { println!(\"hello\"); }".to_string(),
            }
        }

        fn failing(name: &str) -> Self {
            Self {
                name: name.to_string(),
                should_fail: true,
                response_text: String::new(),
            }
        }
    }

    #[async_trait]
    impl CompletionProvider for MockCompletionProvider {
        async fn complete(
            &self,
            _req: &CompletionRequestInternal,
        ) -> Result<CompletionResponseInternal, CompletionEngineError> {
            if self.should_fail {
                Err(CompletionEngineError::ProviderError {
                    provider: self.name.clone(),
                    message: "Mock failure".to_string(),
                })
            } else {
                Ok(CompletionResponseInternal {
                    text: self.response_text.clone(),
                    score: 0.95,
                    provider_name: self.name.clone(),
                    latency_ms: 10,
                })
            }
        }

        fn name(&self) -> &str {
            &self.name
        }

        async fn is_available(&self) -> bool {
            !self.should_fail
        }
    }

    #[tokio::test]
    async fn test_engine_single_provider_success() {
        let engine = CompletionEngine {
            providers: vec![Arc::new(MockCompletionProvider::new("mock-a"))],
            default_max_tokens: 64,
            default_temperature: 0.5,
        };

        let result = engine
            .complete("test.rs", "fn main() {\n    ", 14, "rust")
            .await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert_eq!(output.candidates.len(), 1);
        assert_eq!(output.provider_used, "mock-a");
    }

    #[tokio::test]
    async fn test_engine_fallback_to_second_provider() {
        let engine = CompletionEngine {
            providers: vec![
                Arc::new(MockCompletionProvider::failing("mock-fail")),
                Arc::new(MockCompletionProvider::new("mock-ok")),
            ],
            default_max_tokens: 64,
            default_temperature: 0.5,
        };

        let result = engine
            .complete("test.rs", "fn main() {\n    ", 14, "rust")
            .await;
        assert!(result.is_ok());
        let output = result.unwrap();
        assert_eq!(output.provider_used, "mock-ok");
    }

    #[tokio::test]
    async fn test_all_providers_fail() {
        let engine = CompletionEngine {
            providers: vec![
                Arc::new(MockCompletionProvider::failing("a")),
                Arc::new(MockCompletionProvider::failing("b")),
            ],
            default_max_tokens: 64,
            default_temperature: 0.5,
        };

        let result = engine
            .complete("test.rs", "fn main() {\n    ", 14, "rust")
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            CompletionEngineError::AllProvidersFailed(n) => assert_eq!(n, 2),
            other => panic!("Expected AllProvidersFailed, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_is_ready_with_no_providers() {
        let engine = CompletionEngine {
            providers: vec![],
            default_max_tokens: 64,
            default_temperature: 0.5,
        };
        assert!(!engine.is_ready().await);
    }

    #[test]
    fn test_local_provider_name() {
        let config = ProviderConfig::default();
        let provider = LocalCompletionProvider::new(&config);
        assert_eq!(provider.name(), "local-ollama");
    }

    #[test]
    fn test_jcode_stub_name() {
        let provider = JcodeCompletionProvider::new("http://localhost:8080", "model");
        assert_eq!(provider.name(), "jcode-completion");
    }
}
