//! Protocol adapters for different communication protocols

use crate::error::{CarpAiError, Result};
use crate::types::*;
use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;

/// Trait for protocol adapters
#[async_trait]
pub trait ProtocolAdapter: Send + Sync {
    /// Send a completion request and get a response
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse>;

    /// Send a chat completion request
    async fn chat_complete(&self, request: ChatCompletionRequest) -> Result<ChatCompletionResponse>;

    /// Stream a completion response
    #[allow(clippy::result_large_err)]
    fn stream_complete(
        &self,
        request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>>;

    /// Execute a code action
    async fn code_action(&self, request: CodeActionRequest) -> Result<CodeActionResponse>;

    /// Check server health
    async fn health_check(&self) -> Result<HealthCheckResponse>;

    /// Get the protocol name
    fn name(&self) -> &str;
}

/// REST/HTTP adapter
pub struct RestAdapter {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    #[allow(dead_code)]
    timeout: std::time::Duration,
}

impl RestAdapter {
    pub fn new(base_url: String, api_key: Option<String>, timeout_secs: u64) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| CarpAiError::Connection {
                message: format!("Failed to build HTTP client: {}", e),
                endpoint: base_url.clone(),
                source: Some(e.into()),
            })?;

        Ok(Self {
            client,
            base_url,
            api_key,
            timeout: std::time::Duration::from_secs(timeout_secs),
        })
    }

    fn build_headers(&self) -> Result<reqwest::header::HeaderMap> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("application/json"),
        );

        if let Some(ref key) = self.api_key {
            let auth_value = format!("Bearer {}", key);
            let header_value = reqwest::header::HeaderValue::from_str(&auth_value)
                .map_err(|_| CarpAiError::Validation {
                    message: "API key contains invalid characters".to_string(),
                    field: Some("api_key".to_string()),
                    suggestion: Some("Ensure API key only contains valid ASCII characters".to_string()),
                })?;
            headers.insert(reqwest::header::AUTHORIZATION, header_value);
        }

        Ok(headers)
    }
}

#[async_trait]
impl ProtocolAdapter for RestAdapter {
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse> {
        let url = format!("{}/v1/completions", self.base_url);
        let start = std::time::Instant::now();

        let headers = self.build_headers()?;
        let response = self
            .client
            .post(&url)
            .headers(headers)
            .json(&request)
            .send()
            .await
            .map_err(|e| CarpAiError::Connection {
                message: e.to_string(),
                endpoint: url.clone(),
                source: Some(e),
            })?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_else(|_| String::from("Failed to read error body"));
            return Err(CarpAiError::Server {
                status,
                message: body,
                code: None,
                request_id: None,
            });
        }

        let mut completion_response: CompletionResponse = response.json().await.map_err(|e| CarpAiError::InvalidResponse {
            message: format!("Failed to parse response: {}", e),
            raw_response: None,
        })?;
        completion_response.latency_ms = start.elapsed().as_millis() as f64;
        completion_response.cached = false;

        Ok(completion_response)
    }

    async fn chat_complete(&self, request: ChatCompletionRequest) -> Result<ChatCompletionResponse> {
        let url = format!("{}/v1/chat/completions", self.base_url);
        let start = std::time::Instant::now();

        let headers = self.build_headers()?;
        let response = self
            .client
            .post(&url)
            .headers(headers)
            .json(&request)
            .send()
            .await
            .map_err(|e| CarpAiError::Connection {
                message: e.to_string(),
                endpoint: url.clone(),
                source: Some(e),
            })?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_else(|_| String::from("Failed to read error body"));
            return Err(CarpAiError::Server {
                status,
                message: body,
                code: None,
                request_id: None,
            });
        }

        let mut chat_response: ChatCompletionResponse = response.json().await.map_err(|e| CarpAiError::InvalidResponse {
            message: format!("Failed to parse response: {}", e),
            raw_response: None,
        })?;
        chat_response.latency_ms = start.elapsed().as_millis() as f64;
        chat_response.cached = false;

        Ok(chat_response)
    }

    fn stream_complete(
        &self,
        request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>> {
        let url = format!("{}/v1/completions/stream", self.base_url);
        let client = self.client.clone();
        let headers = self.build_headers()?;

        let stream = async_stream::stream! {
            let response = client
                .post(&url)
                .headers(headers)
                .json(&request)
                .send()
                .await
                .map_err(|e| CarpAiError::Connection {
                    message: e.to_string(),
                    endpoint: url.clone(),
                    source: Some(e),
                })?;

            if !response.status().is_success() {
                let status = response.status().as_u16();
                let body = response.text().await.unwrap_or_else(|_| String::from("Failed to read error body"));
                yield Err(CarpAiError::Server {
                    status,
                    message: body,
                    code: None,
                    request_id: None,
                });
                return;
            }

            let byte_stream = response.bytes_stream();
            use futures::StreamExt;

            let mut buffer = String::new();
            let mut byte_stream = Box::pin(byte_stream);

            while let Some(chunk_result) = byte_stream.next().await {
                match chunk_result {
                    Ok(bytes) => {
                        let chunk_str = String::from_utf8_lossy(&bytes);
                        buffer.push_str(&chunk_str);

                        while let Some(newline_pos) = buffer.find('\n') {
                            let line = buffer[..newline_pos].to_string();
                            buffer = buffer[newline_pos + 1..].to_string();

                            if let Some(data) = line.strip_prefix("data: ") {
                                if data == "[DONE]" {
                                    return;
                                }

                                match serde_json::from_str::<StreamChunk>(data) {
                                    Ok(stream_chunk) => yield Ok(stream_chunk),
                                    Err(e) => yield Err(CarpAiError::Streaming {
                                        message: "Failed to parse stream chunk".to_string(),
                                        source: Some(Box::new(e)),
                                    }),
                                }
                            }
                        }
                    }
                    Err(e) => {
                        yield Err(CarpAiError::Streaming {
                            message: "Stream error".to_string(),
                            source: Some(Box::new(e)),
                        });
                        return;
                    }
                }
            }
        };

        Ok(Box::pin(stream))
    }

    async fn code_action(&self, request: CodeActionRequest) -> Result<CodeActionResponse> {
        let url = format!("{}/v1/code/actions", self.base_url);

        let response = self
            .client
            .post(&url)
            .headers(self.build_headers())
            .json(&request)
            .send()
            .await
            .map_err(|e| CarpAiError::Connection {
                message: e.to_string(),
                endpoint: url.clone(),
                source: Some(e),
            })?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(CarpAiError::Server {
                status,
                message: body,
                code: None,
                request_id: None,
            });
        }

        response.json().await.map_err(|e| CarpAiError::InvalidResponse {
            message: e.to_string(),
            raw_response: None,
        })
    }

    async fn health_check(&self) -> Result<HealthCheckResponse> {
        let url = format!("{}/health", self.base_url);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| CarpAiError::Connection {
                message: e.to_string(),
                endpoint: url.clone(),
                source: Some(e),
            })?;

        if !response.status().is_success() {
            return Err(CarpAiError::Server {
                status: response.status().as_u16(),
                message: "Health check failed".to_string(),
                code: None,
                request_id: None,
            });
        }

        response.json().await.map_err(|e| CarpAiError::InvalidResponse {
            message: e.to_string(),
            raw_response: None,
        })
    }

    fn name(&self) -> &str {
        "REST"
    }
}

/// gRPC adapter (placeholder for future implementation)
pub struct GrpcAdapter {
    // Would hold gRPC client connection
    _endpoint: String,
}

impl GrpcAdapter {
    pub fn new(endpoint: String) -> Self {
        Self { _endpoint: endpoint }
    }
}

#[async_trait]
impl ProtocolAdapter for GrpcAdapter {
    async fn complete(&self, _request: CompletionRequest) -> Result<CompletionResponse> {
        // TODO: Implement gRPC client
        Err(CarpAiError::FeatureNotAvailable {
            feature: "gRPC".to_string(),
            requirement: Some("gRPC support is not yet implemented in the SDK".to_string()),
        })
    }

    async fn chat_complete(&self, _request: ChatCompletionRequest) -> Result<ChatCompletionResponse> {
        Err(CarpAiError::FeatureNotAvailable {
            feature: "gRPC chat".to_string(),
            requirement: Some("gRPC support is not yet implemented in the SDK".to_string()),
        })
    }

    fn stream_complete(
        &self,
        _request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>> {
        Err(CarpAiError::FeatureNotAvailable {
            feature: "gRPC streaming".to_string(),
            requirement: Some("gRPC support is not yet implemented in the SDK".to_string()),
        })
    }

    async fn code_action(&self, _request: CodeActionRequest) -> Result<CodeActionResponse> {
        Err(CarpAiError::FeatureNotAvailable {
            feature: "gRPC code actions".to_string(),
            requirement: Some("gRPC support is not yet implemented in the SDK".to_string()),
        })
    }

    async fn health_check(&self) -> Result<HealthCheckResponse> {
        Err(CarpAiError::FeatureNotAvailable {
            feature: "gRPC health check".to_string(),
            requirement: Some("gRPC support is not yet implemented in the SDK".to_string()),
        })
    }

    fn name(&self) -> &str {
        "gRPC"
    }
}
