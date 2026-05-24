use std::sync::Arc;
use std::time::Duration;
#[allow(dead_code)]
use async_trait::async_trait;
use reqwest::Client;
use tokio_stream::StreamExt;
#[allow(dead_code)]
use carpai_internal::inference::*;
use carpai_internal::inference_backend::*;
use tracing::{info, debug, warn};

pub struct SidecarInferenceBackend {
    client: Client,
    endpoint: String,
    model: String,
    api_key: Option<String>,
    #[allow(dead_code)]
    timeout: Duration,
}

impl SidecarInferenceBackend {
    pub fn new(provider_config: &crate::config::ProviderConfig) -> Self {
        let endpoint = provider_config
            .endpoint
            .as_deref()
            .unwrap_or("http://localhost:11434");

        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(provider_config.timeout_secs))
                .build()
                .expect("Failed to build HTTP client"),
            endpoint: endpoint.to_string(),
            model: provider_config
                .model
                .clone()
                .unwrap_or_else(|| "default".to_string()),
            api_key: provider_config.api_key.clone(),
            timeout: Duration::from_secs(provider_config.timeout_secs),
        }
    }

    pub fn with_model(model: impl Into<String>) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("Failed to build HTTP client"),
            endpoint: "http://localhost:11434".into(),
            model: model.into(),
            api_key: None,
            timeout: Duration::from_secs(30),
        }
    }

    fn build_url(&self, path: &str) -> String {
        format!("{}/{}", self.endpoint, path)
    }

    fn build_request_body(&self, request: &ChatCompletionRequest) -> serde_json::Value {
        serde_json::json!({
            "model": request.model,
            "messages": request.messages,
            "temperature": request.temperature.unwrap_or(0.7),
            "max_tokens": request.max_tokens.unwrap_or(4096),
            "top_p": request.top_p,
            "frequency_penalty": request.frequency_penalty.unwrap_or(0.0),
            "presence_penalty": request.presence_penalty.unwrap_or(0.0),
            "stop": request.stop,
        })
    }

    async fn send_request(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<reqwest::Response, InferenceError> {
        let mut req_builder = self.client.post(url).json(body);

        if let Some(ref key) = self.api_key {
            req_builder = req_builder.header("Authorization", format!("Bearer {}", key));
        }

        req_builder.send().await.map_err(|e| {
            InferenceError::ApiError {
                status: 0,
                message: format!("Failed to send request to {}: {}", url, e),
            }
        })
    }

    fn parse_chat_response(
        &self,
        response: serde_json::Value,
        request_model: String,
    ) -> Result<ChatCompletionResponse, InferenceError> {
        let choice = response["choices"][0].clone();
        let message = choice["message"].clone();

        let text = message["content"]
            .as_str()
            .unwrap_or("")
            .to_string();

        let usage = response["usage"].clone();
        let token_usage = CompletionTokenUsage {
            prompt_tokens: usage["prompt_tokens"].as_u64().unwrap_or(0).try_into().unwrap_or(0),
            completion_tokens: usage["completion_tokens"].as_u64().unwrap_or(0).try_into().unwrap_or(0),
            total_tokens: usage["total_tokens"].as_u64().unwrap_or(0).try_into().unwrap_or(0),
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        };

        debug!(tokens = %token_usage.total_tokens, "Generation complete");

        let finish_reason_str = choice["finish_reason"]
            .as_str()
            .unwrap_or("stop");
        let finish_reason = match finish_reason_str {
            "length" => FinishReason::Length,
            "content_filter" => FinishReason::ContentFilter,
            "error" => FinishReason::Error,
            _ => FinishReason::Stop,
        };

        Ok(ChatCompletionResponse {
            id: response["id"].as_str().unwrap_or("local").to_string(),
            object: "chat.completion".to_string(),
            created: chrono::Utc::now().timestamp() as u64,
            model: request_model,
            choices: vec![Choice {
                index: 0,
                message: ChatMessage {
                    role: ChatRole::Assistant,
                    content: ChatContent::Text(text),
                    name: None,
                },
                finish_reason,
                logprobs: None,
            }],
            usage: token_usage,
            provider: None,
            fallback_info: None,
        })
    }
}

#[async_trait]
impl InferenceBackend for SidecarInferenceBackend {
    async fn complete_chat(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, InferenceError> {
        info!(
            model = %request.model,
            messages = request.messages.len(),
            "Complete chat generation"
        );

        let url = self.build_url("/v1/chat/completions");
        let body = self.build_request_body(&request);

        let resp = self.send_request(&url, &body).await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            warn!(%status, body = %text, "Inference API error");
            return Err(InferenceError::ApiError {
                status: status.as_u16(),
                message: text,
            });
        }

        let response: serde_json::Value = resp.json().await.map_err(|e| {
            InferenceError::InvalidRequest(format!("Failed to parse response: {}", e))
        })?;

        self.parse_chat_response(response, request.model.clone())
    }

    async fn stream_chat(
        &self,
        request: ChatCompletionRequest,
    ) -> Result<Box<dyn tokio_stream::Stream<Item = Result<StreamChunk, InferenceError>> + Send>, InferenceError> {
        info!(
            model = %request.model,
            messages = request.messages.len(),
            "Starting stream chat"
        );

        let url = self.build_url("/v1/chat/completions");
        let mut body = self.build_request_body(&request);
        body["stream"] = serde_json::json!(true);

        let mut req_builder = self.client.post(&url).json(&body);

        if let Some(ref key) = self.api_key {
            req_builder = req_builder.header("Authorization", format!("Bearer {}", key));
        }

        let resp = req_builder.send().await.map_err(|e| {
            InferenceError::ApiError {
                status: 0,
                message: format!("Failed to send stream request: {}", e),
            }
        })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(InferenceError::ApiError {
                status: status.as_u16(),
                message: format!("Stream API error {}: {}", status, text),
            });
        }

        let byte_stream = resp.bytes_stream();

        let stream = byte_stream.map(move |chunk_result| -> Result<StreamChunk, InferenceError> {
            let chunk: bytes::Bytes = chunk_result.map_err(|e| {
                InferenceError::ApiError {
                    status: 0,
                    message: format!("Stream chunk error: {}", e),
                }
            })?;
            let text = String::from_utf8_lossy(&chunk);

            if text.starts_with("data: ") && text != "[DONE]\n" {
                if let Ok(data) = serde_json::from_str::<serde_json::Value>(&text[6..]) {
                    let delta = &data["choices"][0]["delta"];
                    let content = delta["content"].as_str().unwrap_or("").to_string();

                    if content.is_empty() {
                        return Err(InferenceError::InvalidRequest("Empty content delta".into()));
                    }

                    let finish_reason = data["choices"][0]["finish_reason"]
                        .as_str()
                        .map(|s| match s {
                            "length" => FinishReason::Length,
                            "content_filter" => FinishReason::ContentFilter,
                            "error" => FinishReason::Error,
                            _ => FinishReason::Stop,
                        });

                    return Ok(StreamChunk {
                        chunk_type: if finish_reason.is_some() {
                            StreamChunkType::Finish
                        } else {
                            StreamChunkType::ContentDelta
                        },
                        index: 0,
                        delta: Some(content),
                        finish_reason,
                        usage: None,
                    });
                }
            }

            Err(InferenceError::InvalidRequest("Invalid stream data".into()))
        });

        Ok(Box::new(stream))
    }

    async fn list_models_with_routing(&self) -> Result<Vec<RoutedModelInfo>, InferenceError> {
        let url = self.build_url("/api/tags");

        match self.client.get(&url).timeout(Duration::from_secs(5)).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    let models: serde_json::Value = resp.json().await.map_err(|e| {
                        InferenceError::ApiError {
                            status: 0,
                            message: format!("Failed to parse models response: {}", e),
                        }
                    })?;

                    let ollama_models = models["models"]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|m| m["name"].as_str().map(|n| n.to_string()))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();

                    Ok(ollama_models
                        .iter()
                        .enumerate()
                        .map(|(i, name)| RoutedModelInfo {
                            model: ModelInfo {
                                id: name.clone(),
                                name: name.clone(),
                                context_length: 128000,
                                capabilities: vec![],
                                available: true,
                            },
                            providers: vec![ModelProviderEntry {
                                provider: "ollama".to_string(),
                                endpoint: Some(self.endpoint.clone()),
                                weight: 100,
                                healthy: true,
                            }],
                            cost_per_1k_input: 0.0,
                            cost_per_1k_output: 0.0,
                            avg_latency_ms: 100.0,
                            success_rate: 1.0,
                            routing_priority: (i + 10) as u32,
                            supports_function_calling: false,
                            supports_thinking: false,
                            context_window: 128000,
                        })
                        .collect())
                } else {
                    Ok(vec![self.default_routed_model_info()])
                }
            }
            Err(e) => {
                warn!(error = %e, "Failed to fetch models from Ollama API");
                Ok(vec![self.default_routed_model_info()])
            }
        }
    }

    async fn select_model(
        &self,
        constraints: &ModelSelectionConstraints,
    ) -> Result<String, InferenceError> {
        info!(
            min_context_window = ?constraints.min_context_window,
            "Selecting model"
        );

        Ok(self.model.clone())
    }

    async fn get_quota_usage(&self, _user_id: &str) -> Result<QuotaUsage, InferenceError> {
        Ok(QuotaUsage {
            user_id: _user_id.to_string(),
            tokens_used: 0,
            token_limit: 0,
            requests_used: 0,
            request_limit: 0,
            period_start: chrono::Utc::now(),
            period_end: chrono::Utc::now() + chrono::Duration::hours(24),
            reset_in_secs: 86400,
        })
    }

    async fn record_usage(
        &self,
        _user_id: &str,
        usage: &CompletionTokenUsage,
        _model: &str,
    ) -> Result<(), InferenceError> {
        info!(
            prompt_tokens = usage.prompt_tokens,
            completion_tokens = usage.completion_tokens,
            "Recording token usage"
        );
        Ok(())
    }

    fn base_engine(&self) -> Arc<dyn InferenceEngine> {
        unimplemented!("base_engine() not yet implemented for SidecarInferenceBackend")
    }
}

impl SidecarInferenceBackend {
    fn default_routed_model_info(&self) -> RoutedModelInfo {
        RoutedModelInfo {
            model: ModelInfo {
                id: self.model.clone(),
                name: self.model.clone(),
                context_length: 128000,
                capabilities: vec![],
                available: true,
            },
            providers: vec![ModelProviderEntry {
                provider: "sidecar".to_string(),
                endpoint: Some(self.endpoint.clone()),
                weight: 100,
                healthy: true,
            }],
            cost_per_1k_input: 0.0,
            cost_per_1k_output: 0.0,
            avg_latency_ms: 100.0,
            success_rate: 1.0,
            routing_priority: 1,
            supports_function_calling: true,
            supports_thinking: false,
            context_window: 128000,
        }
    }
}
