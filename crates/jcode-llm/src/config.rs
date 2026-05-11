//! LLM Configuration

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// LLM configuration for different providers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// Provider type (Deepseek, vLLM, etc.)
    pub provider_type: crate::types::ProviderType,
    
    /// Model name (e.g., "deepseek-v4-20250527", "Qwen2.5-72B-Instruct")
    pub model_name: String,
    
    /// API base URL (None for default)
    pub api_base_url: Option<String>,
    
    /// API key environment variable name
    pub api_key_env: String,
    
    /// Maximum tokens in response
    pub max_tokens: u32,
    
    /// Temperature (0.0 - 2.0)
    pub temperature: f64,
    
    /// Top-p sampling (0.0 - 1.0)
    pub top_p: Option<f64>,
    
    /// Request timeout in seconds
    pub timeout_secs: u64,
    
    /// Enable streaming by default
    pub stream_default: bool,
    
    /// Number of retries on failure
    pub max_retries: u32,
    
    /// Retry delay base in seconds (exponential backoff)
    pub retry_delay_base_secs: f64,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider_type: crate::ProviderType::Deepseek,
            model_name: "deepseek-chat".to_string(),
            api_base_url: None,
            api_key_env: "DEEPSEEK_API_KEY".to_string(),
            max_tokens: 4096,
            temperature: 0.7,
            top_p: None,
            timeout_secs: 120,
            stream_default: false,
            max_retries: 3,
            retry_delay_base_secs: 1.0,
        }
    }
}

impl LlmConfig {
    /// Get the actual API key from environment
    pub fn get_api_key(&self) -> Result<String, crate::error::LlmError> {
        std::env::var(&self.api_key_env)
            .map_err(|_| crate::error::LlmError::ApiKeyNotFound(self.api_key_env.clone()))
    }

    /// Get the effective API base URL
    pub fn get_api_base_url(&self) -> String {
        match &self.api_base_url {
            Some(url) => url.clone(),
            None => self.provider_type.default_api_base_url().to_string(),
        }
    }

    /// Get timeout as Duration
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs)
    }

    /// Create config for local vLLM deployment
    pub fn local_vllm(model_name: impl Into<String>, port: u16) -> Self {
        Self {
            provider_type: crate::ProviderType::OpenAiCompatible,
            model_name: model_name.into(),
            api_base_url: Some(format!("http://localhost:{}/v1", port)),
            api_key_env: "EMPTY".to_string(), // Local doesn't need API key
            max_tokens: 8192,
            temperature: 0.7,
            ..Default::default()
        }
    }

    /// Create config for llama.cpp server
    pub fn local_llamacpp(model_name: impl Into<String>, port: u16) -> Self {
        // llama.cpp uses OpenAI-compatible API format
        Self::local_vllm(model_name, port)
    }

    /// Create config for Deepseek cloud API
    pub fn deepseek_chat() -> Self {
        Self {
            provider_type: crate::ProviderType::Deepseek,
            model_name: "deepseek-chat".to_string(),
            api_base_url: Some("https://api.deepseek.com/v1".to_string()),
            api_key_env: "DEEPSEEK_API_KEY".to_string(),
            max_tokens: 8192,
            temperature: 0.7,
            ..Default::default()
        }
    }

    /// Create config for Deepseek Reasoner (R1)
    pub fn deepseek_reasoner() -> Self {
        Self {
            provider_type: crate::ProviderType::Deepseek,
            model_name: "deepseek-reasoner".to_string(),
            api_base_url: Some("https://api.deepseek.com/v1".to_string()),
            api_key_env: "DEEPSEEK_API_KEY".to_string(),
            max_tokens: 16384,
            temperature: 0.0, // Deterministic for reasoning models
            ..Default::default()
        }
    }
}

impl crate::types::ProviderType {
    /// Get default API base URL for each provider type
    pub fn default_api_base_url(&self) -> &'static str {
        match self {
            Self::Deepseek => "https://api.deepseek.com",
            Self::OpenAiCompatible => "http://localhost:8000", // Default vLLM port
            Self::Custom => "http://localhost:8080",
        }
    }
}
