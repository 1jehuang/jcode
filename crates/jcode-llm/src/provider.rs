//! LLM Provider implementations

use async_trait::async_trait;
use futures::Stream;
use futures::stream::StreamExt;
use reqwest::{Client, Response};
use std::pin::Pin;
use std::sync::Arc;
use tracing::debug;

use serde::Deserialize;

use crate::{
    config::LlmConfig,
    error::{LlmError, LlmResult},
    types::*,
};

/// Stream type alias for chat completion streaming
pub type ChatCompletionStream = Pin<Box<dyn Stream<Item = LlmResult<ChatCompletionChunk>> + Send>>;

/// Core trait for LLM providers
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Get provider type
    fn provider_type(&self) -> crate::ProviderType;

    /// Get model name
    fn model_name(&self) -> &str;

    /// Get current configuration
    fn config(&self) -> &LlmConfig;

    /// Non-streaming chat completion
    async fn chat_completion(
        &self,
        request: ChatCompletionRequest,
    ) -> LlmResult<ChatCompletionResponse>;

    /// Streaming chat completion
    async fn chat_completion_stream(
        &self,
        request: ChatCompletionRequest,
    ) -> LlmResult<ChatCompletionStream>;

    /// Generate embeddings for text(s)
    async fn embeddings(
        &self,
        request: EmbeddingRequest,
    ) -> LlmResult<EmbeddingResponse>;

    /// Count tokens in text
    async fn count_tokens(&self, text: &str) -> LlmResult<u32>;

    /// Check if the provider is available and responsive
    async fn health_check(&self) -> LlmResult<bool>;

    /// List available models
    async fn list_models(&self) -> LlmResult<Vec<ModelInfo>>;
}

/// HTTP client wrapper with common functionality
struct HttpClient {
    client: Client,
    config: LlmConfig,
}

impl HttpClient {
    fn new(config: LlmConfig) -> Self {
        Self {
            client: Client::builder()
                .timeout(config.timeout())
                .build()
                .expect("Failed to build HTTP client"),
            config,
        }
    }

    async fn get_api_key(&self) -> LlmResult<String> {
        self.config.get_api_key()
    }

    async fn post<T: serde::Serialize, U: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        body: &T,
    ) -> LlmResult<U> {
        let url = format!("{}/{}", self.config.get_api_base_url(), path);
        
        debug!(url = %url, "Making POST request");

        let api_key = self.get_api_key().await?;
        
        let mut req = self.client
            .post(&url)
            .json(body)
            .header("Content-Type", "application/json");
            
        // Add API key if not empty (for local deployments)
        if !api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = req.send().await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let error_body = response.text().await.unwrap_or_default();
            
            // Parse error message from response
            let error_msg = parse_error_message(&error_body);
            
            return Err(match status {
                401 => LlmError::AuthenticationFailed,
                429 => LlmError::RateLimited { retry_after_seconds: 60 },
                _ => LlmError::ApiError {
                    status,
                    message: error_msg,
                    code: None,
                },
            });
        }

        response.json::<U>().await.map_err(|e| LlmError::InvalidResponse(e.to_string()))
    }

    async fn post_stream(
        &self,
        path: &str,
        body: &ChatCompletionRequest,
    ) -> LlmResult<Response> {
        let url = format!("{}/{}", self.config.get_api_base_url(), path);
        
        debug!(url = %url, "Making streaming POST request");

        let api_key = self.get_api_key().await?;
        
        let mut req = self.client
            .post(&url)
            .json(body)
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream");
            
        if !api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = req.send().await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let error_body = response.text().await.unwrap_or_default();
            return Err(LlmError::ApiError {
                status,
                message: parse_error_message(&error_body),
                code: None,
            });
        }

        Ok(response)
    }
}

fn parse_error_message(body: &str) -> String {
    // Try to parse as JSON error object
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(error) = json.get("error") {
            if let Some(msg) = error.get("message").and_then(|m| m.as_str()) {
                return msg.to_string();
            }
            return error.to_string();
        }
        // Some APIs wrap errors differently
        if let Some(msg) = json.get("message").and_then(|m| m.as_str()) {
            return msg.to_string();
        }
    }
    
    // Fallback to raw body (truncated)
    if body.len() > 200 {
        format!("{}...", &body[..200])
    } else {
        body.to_string()
    }
}

// ============== Deepseek Provider ==============

/// Deepseek API provider (cloud-based)
pub struct DeepseekProvider {
    http: Arc<HttpClient>,
}

impl DeepseekProvider {
    pub fn new(config: LlmConfig) -> Self {
        assert_eq!(
            config.provider_type,
            crate::ProviderType::Deepseek,
            "Invalid provider type for DeepseekProvider"
        );
        
        Self {
            http: Arc::new(HttpClient::new(config)),
        }
    }
}

#[async_trait]
impl LlmProvider for DeepseekProvider {
    fn provider_type(&self) -> crate::ProviderType {
        crate::ProviderType::Deepseek
    }

    fn model_name(&self) -> &str {
        &self.http.config.model_name
    }

    fn config(&self) -> &LlmConfig {
        &self.http.config
    }

    async fn chat_completion(
        &self,
        mut request: ChatCompletionRequest,
    ) -> LlmResult<ChatCompletionResponse> {
        // Set default values if not provided
        if request.temperature.is_none() {
            request.temperature = Some(self.http.config.temperature);
        }
        if request.max_tokens.is_none() {
            request.max_tokens = Some(self.http.config.max_tokens);
        }
        if request.stream.is_none() {
            request.stream = Some(false);
        }

        self.http.post("chat/completions", &request).await
    }

    async fn chat_completion_stream(
        &self,
        mut request: ChatCompletionRequest,
    ) -> LlmResult<ChatCompletionStream> {
        request.stream = Some(true);
        if request.temperature.is_none() {
            request.temperature = Some(self.http.config.temperature);
        }
        if request.max_tokens.is_none() {
            request.max_tokens = Some(self.http.config.max_tokens);
        }

        let response = self.http.post_stream("chat/completions", &request).await?;
        
        // Use bytes_stream and map to create async stream
        let byte_stream = response.bytes_stream();
        
        let stream = byte_stream.map(|result| {
            match result {
                Ok(bytes) => {
                    let data = String::from_utf8_lossy(&bytes);
                    
                    // Parse SSE format: "data: {...}\n\n"
                    for line in data.lines() {
                        if let Some(json_str) = line.strip_prefix("data: ") {
                            if json_str.trim() == "[DONE]" {
                                return Err(LlmError::StreamingError("Stream ended".to_string()));
                            }
                            
                            return serde_json::from_str::<ChatCompletionChunk>(json_str)
                                .map_err(|e| LlmError::StreamingError(e.to_string()));
                        }
                    }
                    
                    Err(LlmError::StreamingError("Empty chunk".to_string()))
                }
                Err(e) => Err(LlmError::RequestFailed(e)),
            }
        });

        Ok(Box::pin(stream))
    }

    async fn embeddings(
        &self,
        request: EmbeddingRequest,
    ) -> LlmResult<EmbeddingResponse> {
        self.http.post("embeddings", &request).await
    }

    async fn count_tokens(&self, text: &str) -> LlmResult<u32> {
        // Approximate token count (4 chars per token average for English)
        // For Chinese/Japanese, it's closer to 1.5-2 chars per token
        let approx_tokens = (text.len() as f32 / 3.5).ceil() as u32;
        Ok(approx_tokens)
    }

    async fn health_check(&self) -> LlmResult<bool> {
        // Simple health check by listing models
        self.list_models().await.map(|_| true).or(Ok(false))
    }

    async fn list_models(&self) -> LlmResult<Vec<ModelInfo>> {
        // Deepseek doesn't have a public models endpoint, so we return known models
        Ok(vec![
            ModelInfo {
                id: "deepseek-chat".to_string(),
                owned_by: crate::ProviderType::Deepseek,
                max_context: 64000,
                supports_function_calling: true,
                supports_streaming: true,
                supports_embeddings: false,
            },
            ModelInfo {
                id: "deepseek-reasoner".to_string(),
                owned_by: crate::ProviderType::Deepseek,
                max_context: 64000,
                supports_function_calling: true,
                supports_streaming: true,
                supports_embeddings: false,
            },
        ])
    }
}

// ============== OpenAI-Compatible Provider (vLLM / llama.cpp) ==============

/// OpenAI-compatible API provider (supports vLLM, llama.cpp server, etc.)
pub struct OpenAiCompatibleProvider {
    http: Arc<HttpClient>,
}

impl OpenAiCompatibleProvider {
    pub fn new(config: LlmConfig) -> Self {
        Self {
            http: Arc::new(HttpClient::new(config)),
        }
    }
}

#[async_trait]
impl LlmProvider for OpenAiCompatibleProvider {
    fn provider_type(&self) -> crate::ProviderType {
        crate::ProviderType::OpenAiCompatible
    }

    fn model_name(&self) -> &str {
        &self.http.config.model_name
    }

    fn config(&self) -> &LlmConfig {
        &self.http.config
    }

    async fn chat_completion(
        &self,
        mut request: ChatCompletionRequest,
    ) -> LlmResult<ChatCompletionResponse> {
        if request.temperature.is_none() {
            request.temperature = Some(self.http.config.temperature);
        }
        if request.max_tokens.is_none() {
            request.max_tokens = Some(self.http.config.max_tokens);
        }
        if request.stream.is_none() {
            request.stream = Some(false);
        }

        self.http.post("chat/completions", &request).await
    }

    async fn chat_completion_stream(
        &self,
        mut request: ChatCompletionRequest,
    ) -> LlmResult<ChatCompletionStream> {
        request.stream = Some(true);
        if request.temperature.is_none() {
            request.temperature = Some(self.http.config.temperature);
        }
        if request.max_tokens.is_none() {
            request.max_tokens = Some(self.http.config.max_tokens);
        }

        let response = self.http.post_stream("chat/completions", &request).await?;
        
        let byte_stream = response.bytes_stream();
        
        let stream = byte_stream.map(|result| {
            match result {
                Ok(bytes) => {
                    let data = String::from_utf8_lossy(&bytes);
                    
                    for line in data.lines() {
                        if let Some(json_str) = line.strip_prefix("data: ") {
                            if json_str.trim() == "[DONE]" {
                                return Err(LlmError::StreamingError("Stream ended".to_string()));
                            }
                            
                            return serde_json::from_str::<ChatCompletionChunk>(json_str)
                                .map_err(|e| LlmError::StreamingError(e.to_string()));
                        }
                    }
                    
                    Err(LlmError::StreamingError("Empty chunk".to_string()))
                }
                Err(e) => Err(LlmError::RequestFailed(e)),
            }
        });

        Ok(Box::pin(stream))
    }

    async fn embeddings(
        &self,
        request: EmbeddingRequest,
    ) -> LlmResult<EmbeddingResponse> {
        self.http.post("embeddings", &request).await
    }

    async fn count_tokens(&self, text: &str) -> LlmResult<u32> {
        // Use tokenizer API if available, otherwise approximate
        let request = TokenCountRequest {
            model: self.http.config.model_name.clone(),
            input: text.to_string(),
        };
        
        match self.http.post::<TokenCountRequest, TokenCountResponse>("tokenize", &request).await {
            Ok(response) => Ok(response.tokens),
            Err(_) => {
                // Fallback to approximation
                Ok((text.len() as f32 / 3.5).ceil() as u32)
            }
        }
    }

    async fn health_check(&self) -> LlmResult<bool> {
        self.list_models().await.map(|models| !models.is_empty()).or(Ok(false))
    }

    async fn list_models(&self) -> LlmResult<Vec<ModelInfo>> {
        let response: serde_json::Value = self.http.get("/models").await?;
        
        let models = response["data"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| {
                        let id = m["id"].as_str()?.to_string();
                        Some(ModelInfo {
                            id,
                            owned_by: crate::ProviderType::OpenAiCompatible,
                            max_context: m["context_length"]
                                .as_u64()
                                .unwrap_or(8192) as usize,
                            supports_function_calling: true,
                            supports_streaming: true,
                            supports_embeddings: true,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(models)
    }
}

// Helper method for HTTP client (needed for list_models)
impl HttpClient {
    async fn get<U: for<'de> Deserialize<'de>>(&self, path: &str) -> LlmResult<U> {
        let url = format!("{}/{}", self.config.get_api_base_url(), path);
        
        let api_key = self.get_api_key().await?;
        
        let mut req = self.client.get(&url);
        
        if !api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = req.send().await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let error_body = response.text().await.unwrap_or_default();
            return Err(LlmError::ApiError {
                status,
                message: parse_error_message(&error_body),
                code: None,
            });
        }

        response.json::<U>().await.map_err(|e| LlmError::InvalidResponse(e.to_string()))
    }
}

// ============== Provider Factory ==============

/// Factory for creating LLM providers based on configuration
pub struct LlmProviderFactory;

impl LlmProviderFactory {
    /// Create a provider based on configuration
    pub fn create_provider(config: LlmConfig) -> Arc<dyn LlmProvider> {
        match config.provider_type {
            crate::ProviderType::Deepseek => Arc::new(DeepseekProvider::new(config)),
            crate::ProviderType::OpenAiCompatible | crate::ProviderType::Custom => {
                Arc::new(OpenAiCompatibleProvider::new(config))
            }
        }
    }

    /// Create provider from preset name
    pub fn from_preset(preset: &str) -> Option<Arc<dyn LlmProvider>> {
        let config = match preset {
            "deepseek-chat" => Some(crate::presets::deepseek_chat()),
            "deepseek-r1" => Some(crate::presets::deepseek_r1()),
            _ => None,
        }?;
        
        Some(Self::create_provider(config))
    }

    /// Create local vLLM provider
    pub fn local_vllm(model_name: impl Into<String>, port: u16) -> Arc<dyn LlmProvider> {
        Arc::new(OpenAiCompatibleProvider::new(LlmConfig::local_vllm(model_name, port)))
    }

    /// Create local llama.cpp provider
    pub fn local_llamacpp(model_name: impl Into<String>, port: u16) -> Arc<dyn LlmProvider> {
        Arc::new(OpenAiCompatibleProvider::new(LlmConfig::local_llamacpp(model_name, port)))
    }
}
