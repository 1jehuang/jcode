//! LLM gRPC Service Implementation
//!
//! This module implements the `LlmService` gRPC service defined in jcode.proto.

use std::sync::Arc;
use std::pin::Pin;
use std::time::Instant;

use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use futures::{Stream, StreamExt};
use parking_lot::RwLock;
use tracing::{info, warn, error, instrument};
use uuid::Uuid;

pub mod proto {
    tonic::include_proto!("jcode");
}

use proto::llm_service_server::{LlmService, LlmServiceServer};
use proto::{
    LlmChatRequest, LlmChatResponse, LlmChatStreamChunk,
    StreamDelta, StreamToolCall,
    EmbeddingsRequest, EmbeddingsResponse, EmbeddingData, EmbeddingUsage,
    TokenCountRequest, TokenCountResponse,
    ListModelsRequest, ListModelsResponse, ModelInfo,
    HealthCheckRequest, HealthCheckResponse,
    Message, Usage, Tool, ToolCall,
};

use jcode_llm::{
    LlmProvider, 
    types::*,
};
use crate::error_handling::{LlmErrorCode, ErrorMetadata, errors};

/// LLM Server state
#[derive(Clone)]
pub struct LlmServerState {
    /// Default LLM provider
    pub default_provider: Arc<dyn LlmProvider>,
    
    /// Provider registry (for multi-provider support)
    providers: Arc<RwLock<std::collections::HashMap<String, Arc<dyn LlmProvider>>>>,
}

impl LlmServerState {
    pub fn new(provider: Arc<dyn LlmProvider>) -> Self {
        Self {
            default_provider: provider,
            providers: Arc::new(RwLock::new(std::collections::HashMap::new())),
        }
    }
    
    pub fn register_provider(&self, name: String, provider: Arc<dyn LlmProvider>) {
        self.providers.write().insert(name, provider);
    }
    
    fn get_provider(&self, provider_type: &str) -> Option<Arc<dyn LlmProvider>> {
        if provider_type.is_empty() || provider_type == "default" {
            Some(Arc::clone(&self.default_provider))
        } else {
            self.providers.read().get(provider_type).cloned()
        }
    }
}

/// LLM gRPC Service implementation
pub struct LlmServiceImpl {
    state: Arc<LlmServerState>,
}

impl LlmServiceImpl {
    pub fn new(state: Arc<LlmServerState>) -> Self {
        Self { state }
    }
    
    pub fn into_server(self) -> LlmServiceServer<Self> {
        LlmServiceServer::new(self)
    }
    
    /// Convert proto Message to internal ChatMessage
    fn convert_chat_message(msg: &Message) -> ChatMessage {
        let role = match msg.role.as_str() {
            "system" => MessageRole::System,
            "user" => MessageRole::User,
            "assistant" => MessageRole::Assistant,
            "tool" => MessageRole::Tool,
            _ => MessageRole::User,
        };
        
        ChatMessage {
            role,
            content: if msg.content.is_empty() { None } else { Some(msg.content.clone()) },
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }
    
    /// Convert proto Tool to internal ToolDefinition
    fn convert_tool(tool: &Tool) -> Option<ToolDefinition> {
        // For now, use empty parameters as default
        // TODO: Implement proper Struct -> JSON conversion when needed
        Some(ToolDefinition {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        })
    }
    
    /// Convert proto ToolCall to internal format (for response)
    fn convert_tool_call_response(tc: &jcode_llm::types::ToolCall) -> ToolCall {
        // Convert JSON string arguments to prost Struct
        let arguments = if !tc.arguments.is_empty() {
            // Parse JSON string and convert to prost Struct manually
            let mut struct_val = prost_types::Struct::default();
            
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&tc.arguments) {
                if let serde_json::Value::Object(map) = json {
                    for (key, value) in map {
                        let prost_value = match value {
                            serde_json::Value::String(s) => 
                                Some(prost_types::Value { kind: Some(prost_types::value::Kind::StringValue(s)) }),
                            serde_json::Value::Number(n) => 
                                if n.is_i64() {
                                    Some(prost_types::Value { kind: Some(prost_types::value::Kind::NumberValue(n.as_i64().unwrap_or(0) as f64)) })
                                } else if n.is_f64() {
                                    Some(prost_types::Value { kind: Some(prost_types::value::Kind::NumberValue(n.as_f64().unwrap_or(0.0))) })
                                } else {
                                    None
                                },
                            serde_json::Value::Bool(b) => 
                                Some(prost_types::Value { kind: Some(prost_types::value::Kind::BoolValue(b)) }),
                            _ => None,
                        };
                        
                        if let Some(v) = prost_value {
                            struct_val.fields.insert(key, v);
                        }
                    }
                }
            }
            
            Some(struct_val)
        } else {
            None
        };
        
        ToolCall {
            tool_name: tc.name.clone(),
            arguments,
        }
    }
    
    /// Convert internal response to proto response
    fn convert_response(response: ChatCompletionResponse) -> LlmChatResponse {
        // Extract tool calls first before moving choices
        let tool_calls: Vec<ToolCall> = response.choices.iter()
            .flat_map(|c| c.message.tool_calls.as_ref())
            .flatten()
            .map(|tc| Self::convert_tool_call_response(tc))
            .collect();
        
        let choices: Vec<Message> = response.choices.into_iter().map(|c| {
            Message {
                role: c.message.role.to_string(),
                content: c.message.content.unwrap_or_default(),
                parts: vec![],
            }
        }).collect();
        
        LlmChatResponse {
            id: response.id,
            model: response.model,
            choices,
            usage: Some(Usage {
                prompt_tokens: response.usage.prompt_tokens as i32,
                completion_tokens: response.usage.completion_tokens as i32,
                total_tokens: response.usage.total_tokens as i32,
            }),
            finish_reason: if tool_calls.is_empty() { "stop".to_string() } else { "tool_calls".to_string() },
            latency_ms: 0.0,
            tool_calls,
        }
    }
    
    /// Convert stream chunk to proto format
    fn convert_stream_chunk(chunk: &crate::streaming::StreamChunk) -> LlmChatStreamChunk {
        LlmChatStreamChunk {
            id: chunk.id.clone(),
            model: chunk.model.clone(),
            delta: Some(StreamDelta {
                role: "assistant".to_string(),
                content: chunk.content.clone(),
                tool_calls: vec![],
            }),
            finish_reason: if chunk.done { "stop".to_string() } else { String::new() },
            usage: chunk.usage.as_ref().map(|u| Usage {
                prompt_tokens: u.prompt_tokens as i32,
                completion_tokens: u.completion_tokens as i32,
                total_tokens: u.total_tokens as i32,
            }),
            done: chunk.done,
        }
    }
}

type LlmChatStreamResponse = Pin<Box<dyn Stream<Item = Result<LlmChatStreamChunk, Status>> + Send + 'static>>;

#[tonic::async_trait]
impl LlmService for LlmServiceImpl {
    
    #[instrument(skip(self), fields(session_id = %request.get_ref().session_id))]
    async fn llm_chat(
        &self,
        request: Request<LlmChatRequest>,
    ) -> Result<Response<LlmChatResponse>, Status> {
        let req = request.into_inner();
        let start = Instant::now();
        
        info!(
            session_id = %req.session_id,
            model = %req.model,
            messages = req.messages.len(),
            provider = %req.provider_type,
            "Received LlmChat request"
        );
        
        // Get appropriate provider
        let provider = self.state.get_provider(&req.provider_type)
            .ok_or_else(|| Status::not_found(format!("Provider not found: {}", req.provider_type)))?;
        
        // Convert request to internal format
        let internal_request = ChatCompletionRequest {
            model: req.model,
            messages: req.messages.iter().map(Self::convert_chat_message).collect(),
            temperature: Some(req.temperature as f64),
            max_tokens: Some(req.max_tokens as u32),
            top_p: Some(req.top_p as f64),
            tools: Some(req.tools.iter().filter_map(|t| Self::convert_tool(t)).collect()),
            stream: Some(false),
            stop: if req.stop_sequence.is_empty() { None } else { Some(vec![req.stop_sequence]) },
        };
        
        // Execute chat completion
        match provider.chat_completion(internal_request).await {
            Ok(response) => {
                let latency_ms = start.elapsed().as_millis() as f64;
                
                info!(
                    latency_ms = latency_ms,
                    tokens = ?response.usage,
                    "Chat completion successful"
                );
                
                let mut proto_response = Self::convert_response(response);
                proto_response.latency_ms = latency_ms;
                
                Ok(Response::new(proto_response))
            }
            Err(e) => {
                error!(error = %e, "Chat completion failed");
                
                let error_status = match &e {
                    jcode_llm::error::LlmError::AuthenticationFailed => {
                        errors::authentication_error(
                            e.to_string(),
                            provider.provider_type().to_string()
                        )
                    }
                    jcode_llm::error::LlmError::RateLimited { retry_after_seconds } => {
                        errors::rate_limited_error(
                            e.to_string(),
                            *retry_after_seconds,
                            provider.provider_type().to_string()
                        )
                    }
                    _ => {
                        errors::internal_error(
                            e.to_string(),
                            Some(format!("Provider: {}", provider.provider_type()))
                        )
                    }
                };
                
                Err(error_status)
            }
        }
    }
    
    type LlmChatStreamStream = LlmChatStreamResponse;
    
    #[instrument(skip(self), fields(session_id = %request.get_ref().session_id))]
    async fn llm_chat_stream(
        &self,
        request: Request<LlmChatRequest>,
    ) -> Result<Response<Self::LlmChatStreamStream>, Status> {
        let req = request.into_inner();
        
        info!(
            session_id = %req.session_id,
            model = %req.model,
            messages = req.messages.len(),
            provider = %req.provider_type,
            "Received LlmChatStream request"
        );
        
        // Get appropriate provider
        let provider = self.state.get_provider(&req.provider_type)
            .ok_or_else(|| Status::not_found(format!("Provider not found: {}", req.provider_type)))?;
        
        // Convert request to internal format
        let internal_request = ChatCompletionRequest {
            model: req.model.clone(),
            messages: req.messages.iter().map(Self::convert_chat_message).collect(),
            temperature: Some(req.temperature as f64),
            max_tokens: Some(req.max_tokens as u32),
            top_p: Some(req.top_p as f64),
            tools: Some(req.tools.iter().filter_map(|t| Self::convert_tool(t)).collect()),
            stream: Some(true),
            stop: if req.stop_sequence.is_empty() { None } else { Some(vec![req.stop_sequence]) },
        };
        
        // Create channel for streaming
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let request_id = Uuid::new_v4().to_string();
        let model_name = req.model.clone();
        
        // Spawn streaming task
        tokio::spawn(async move {
            match provider.chat_completion_stream(internal_request).await {
                Ok(mut stream) => {
                    while let Some(result) = stream.next().await {
                        match result {
                            Ok(chunk) => {
                                let proto_chunk = crate::streaming::StreamChunk {
                                    id: request_id.clone(),
                                    model: model_name.clone(),
                                    content: chunk.choices.first()
                                        .and_then(|c| c.delta.content.clone())
                                        .unwrap_or_default(),
                                    done: false,
                                    usage: None,
                                };
                                
                                if tx.send(Ok(proto_chunk)).await.is_err() {
                                    break; // Client disconnected
                                }
                            }
                            Err(e) => {
                                error!(error = %e, "Stream error");
                                
                                let error_chunk = crate::streaming::StreamChunk {
                                    id: request_id.clone(),
                                    model: model_name.clone(),
                                    content: format!("Error: {}", e),
                                    done: true,
                                    usage: None,
                                };
                                
                                let _ = tx.send(Ok(error_chunk)).await;
                                break;
                            }
                        }
                    }
                    
                    // Send final done signal
                    let final_chunk = crate::streaming::StreamChunk {
                        id: request_id,
                        model: model_name,
                        content: String::new(),
                        done: true,
                        usage: None,
                    };
                    
                    let _ = tx.send(Ok(final_chunk)).await;
                }
                Err(e) => {
                    error!(error = %e, "Failed to start stream");
                    
                    let error_chunk = crate::streaming::StreamChunk {
                        id: request_id,
                        model: model_name,
                        content: format!("Stream initialization failed: {}", e),
                        done: true,
                        usage: None,
                    };
                    
                    let _ = tx.send(Ok(error_chunk)).await;
                }
            }
        });
        
        // Convert our stream to proto stream
        let proto_stream = ReceiverStream::new(rx).map(move |result| {
            result.map(|chunk| Self::convert_stream_chunk(&chunk))
        });
        
        Ok(Response::new(Box::pin(proto_stream) as Self::LlmChatStreamStream))
    }
    
    #[instrument(skip(self))]
    async fn generate_embeddings(
        &self,
        request: Request<EmbeddingsRequest>,
    ) -> Result<Response<EmbeddingsResponse>, Status> {
        let req = request.into_inner();
        
        info!(
            session_id = %req.session_id,
            inputs = req.input.len(),
            model = %req.model,
            "Received GenerateEmbeddings request"
        );
        
        let provider = self.state.get_provider(&req.provider_type)
            .ok_or_else(|| Status::not_found(format!("Provider not found: {}", req.provider_type)))?;
        
        let internal_request = EmbeddingRequest {
            model: req.model,
            input: req.input,
            encoding_format: if req.encoding_format.is_empty() { 
                None 
            } else { 
                Some(req.encoding_format) 
            },
        };
        
        match provider.embeddings(internal_request).await {
            Ok(response) => {
                info!(embeddings = response.data.len(), "Embeddings generated successfully");
                
                let proto_data: Vec<EmbeddingData> = response.data.into_iter().map(|d| {
                    EmbeddingData {
                        object: "embedding".to_string(),
                        index: d.index as i32,
                        embedding: d.embedding,
                    }
                }).collect();
                
                Ok(Response::new(EmbeddingsResponse {
                    id: String::new(), // EmbeddingResponse doesn't have id field in jcode-llm
                    model: response.model,
                    data: proto_data,
                    usage: Some(EmbeddingUsage {
                        prompt_tokens: response.usage.prompt_tokens as i32,
                        total_tokens: response.usage.total_tokens as i32,
                    }),
                }))
            }
            Err(e) => {
                error!(error = %e, "Embedding generation failed");
                Err(Status::internal(e.to_string()))
            }
        }
    }
    
    #[instrument(skip(self))]
    async fn count_tokens(
        &self,
        request: Request<TokenCountRequest>,
    ) -> Result<Response<TokenCountResponse>, Status> {
        let req = request.into_inner();
        
        info!(
            input_length = req.input.len(),
            model = %req.model,
            "Received CountTokens request"
        );
        
        let provider = self.state.get_provider(&req.provider_type)
            .ok_or_else(|| Status::not_found(format!("Provider not found: {}", req.provider_type)))?;
        
        match provider.count_tokens(&req.input).await {
            Ok(tokens) => {
                Ok(Response::new(TokenCountResponse { tokens: tokens as i32 }))
            }
            Err(e) => {
                error!(error = %e, "Token counting failed");
                Err(Status::internal(e.to_string()))
            }
        }
    }
    
    #[instrument(skip(self))]
    async fn list_models(
        &self,
        request: Request<ListModelsRequest>,
    ) -> Result<Response<ListModelsResponse>, Status> {
        let req = request.into_inner();
        
        info!(provider = %req.provider_type, "Received ListModels request");
        
        // Clone providers to avoid holding lock across await points
        let default_provider = Arc::clone(&self.state.default_provider);
        let registered_providers: Vec<(String, Arc<dyn LlmProvider>)> = {
            self.state.providers.read()
                .iter()
                .map(|(k, v)| (k.clone(), Arc::clone(v)))
                .collect()
        };
        
        if req.provider_type.is_empty() || req.provider_type == "all" {
            // List from all providers (async without holding lock)
            let mut all_models = Vec::new();
            
            // Add default provider models
            if let Ok(models) = default_provider.list_models().await {
                for m in models {
                    all_models.push(ModelInfo {
                        id: m.id,
                        owned_by: m.owned_by.to_string(),
                        max_context: m.max_context as i32,
                        supports_function_calling: m.supports_function_calling,
                        supports_streaming: m.supports_streaming,
                        supports_embeddings: m.supports_embeddings,
                        metadata: Default::default(),
                    });
                }
            }
            
            // Add registered provider models
            for (_name, provider) in registered_providers {
                if let Ok(models) = provider.list_models().await {
                    for m in models {
                        all_models.push(ModelInfo {
                            id: m.id,
                            owned_by: m.owned_by.to_string(),
                            max_context: m.max_context as i32,
                            supports_function_calling: m.supports_function_calling,
                            supports_streaming: m.supports_streaming,
                            supports_embeddings: m.supports_embeddings,
                            metadata: Default::default(),
                        });
                    }
                }
            }
            
            let total = all_models.len() as i32;
            
            Ok(Response::new(ListModelsResponse {
                models: all_models,
                total_count: total,
            }))
        } else {
            // List from specific provider
            let provider = self.state.get_provider(&req.provider_type)
                .ok_or_else(|| Status::not_found(format!("Provider not found: {}", req.provider_type)))?;
            
            match provider.list_models().await {
                Ok(models) => {
                    let proto_models: Vec<ModelInfo> = models.into_iter().map(|m| ModelInfo {
                        id: m.id,
                        owned_by: m.owned_by.to_string(),
                        max_context: m.max_context as i32,
                        supports_function_calling: m.supports_function_calling,
                        supports_streaming: m.supports_streaming,
                        supports_embeddings: m.supports_embeddings,
                        metadata: Default::default(),
                    }).collect();
                    
                    let total = proto_models.len() as i32;
                    
                    Ok(Response::new(ListModelsResponse {
                        models: proto_models,
                        total_count: total,
                    }))
                }
                Err(e) => {
                    error!(error = %e, "Failed to list models");
                    Err(Status::internal(e.to_string()))
                }
            }
        }
    }
    
    #[instrument(skip(self))]
    async fn health_check(
        &self,
        request: Request<HealthCheckRequest>,
    ) -> Result<Response<HealthCheckResponse>, Status> {
        let req = request.into_inner();
        
        info!(provider = %req.provider_type, model = %req.model, "Received HealthCheck request");
        
        let start = Instant::now();
        
        let provider = if req.provider_type.is_empty() {
            Some(Arc::clone(&self.state.default_provider))
        } else {
            self.state.get_provider(&req.provider_type)
        };
        
        match provider {
            Some(p) => {
                match p.health_check().await {
                    Ok(healthy) => {
                        let latency_ms = start.elapsed().as_millis() as f64;
                        
                        info!(
                            healthy = healthy,
                            latency_ms = latency_ms,
                            "Health check completed"
                        );
                        
                        Ok(Response::new(HealthCheckResponse {
                            healthy,
                            provider_type: p.provider_type().to_string(),
                            model: p.model_name().to_string(),
                            latency_ms,
                            version: env!("CARGO_PKG_VERSION").to_string(),
                            capabilities: {
                                let mut caps = std::collections::HashMap::new();
                                caps.insert("chat".to_string(), "true".to_string());
                                caps.insert("stream".to_string(), "true".to_string());
                                caps.insert("embeddings".to_string(), "true".to_string());
                                caps.insert("tokens".to_string(), "true".to_string());
                                caps
                            },
                        }))
                    }
                    Err(e) => {
                        warn!(error = %e, "Health check failed");
                        
                        Ok(Response::new(HealthCheckResponse {
                            healthy: false,
                            provider_type: p.provider_type().to_string(),
                            model: p.model_name().to_string(),
                            latency_ms: start.elapsed().as_millis() as f64,
                            version: env!("CARGO_PKG_VERSION").to_string(),
                            capabilities: Default::default(),
                        }))
                    }
                }
            }
            None => {
                Err(Status::not_found(format!("Provider not found: {}", req.provider_type)))
            }
        }
    }
}
