//! OpenAI-Compatible REST API Server (for Cursor, etc.)
//!
//! ## Overview
//!
//! This module implements a REST API server that is compatible with the OpenAI API format.
//! This allows tools like Cursor, VS Code Copilot, and other AI-powered IDEs to connect to jcode
//! as if it were an OpenAI-compatible endpoint.
//!
//! ## Endpoints
//!
//! - `POST /v1/chat/completions` - Chat completion (sync + streaming)
//! - `POST /v1/embeddings` - Generate embeddings
//! - `GET /v1/models` - List available models
//! - `GET /health` - Health check

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

use crate::provider::LlmProvider;
use crate::types::*;
use crate::config::LlmConfig;

/// Application state containing the LLM provider
#[derive(Clone)]
pub struct AppState {
    pub provider: Arc<dyn LlmProvider>,
    pub config: Arc<LlmConfig>,
}

/// Create the main router with all routes
pub fn create_router(provider: Arc<dyn LlmProvider>, config: Arc<LlmConfig>) -> Router {
    let state = AppState { 
        provider,
        config,
    };

    Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/embeddings", post(embeddings))
        .route("/v1/models", get(list_models))
        .route("/health", get(health_check))
        .layer(CorsLayer::new().allow_origin(Any))
        .with_state(state)
}

// ============== Request/Response Types (OpenAI Compatible) ==============

/// Chat completion request (OpenAI format)
#[derive(Debug, Deserialize)]
pub struct ChatCompletionApiRequest {
    pub model: String,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(rename = "top_p", skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,
}

/// Chat completion response (OpenAI format)
#[derive(Debug, Serialize)]
pub struct ChatCompletionApiResponse {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: Usage,
}

/// Embedding request (OpenAI format)
#[derive(Debug, Deserialize)]
pub struct EmbeddingApiRequest {
    pub model: String,
    pub input: EmbeddingInput,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoding_format: Option<String>,
}

/// Input can be a string or array of strings
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum EmbeddingInput {
    Single(String),
    Multiple(Vec<String>),
}

impl EmbeddingInput {
    fn into_vec(self) -> Vec<String> {
        match self {
            Self::Single(s) => vec![s],
            Self::Multiple(v) => v,
        }
    }
}

/// Models list response (OpenAI format)
#[derive(Debug, Serialize)]
pub struct ModelsListResponse {
    pub object: String,
    pub data: Vec<ModelInfoOpenAi>,
}

/// Model info in OpenAI format
#[derive(Debug, Serialize)]
pub struct ModelInfoOpenAi {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub owned_by: String,
}

/// Health check response
#[derive(Debug, Serialize)]
pub struct HealthCheckResponse {
    pub status: String,
    pub model: String,
    pub provider: String,
    pub version: String,
}

// ============== Route Handlers ==============

/// POST /v1/chat/completions
async fn chat_completions(
    State(state): State<AppState>,
    Json(request): Json<ChatCompletionApiRequest>,
) -> impl IntoResponse {
    let start_time = std::time::Instant::now();
    
    tracing::info!(
        model = %request.model,
        messages = request.messages.len(),
        stream = ?request.stream,
        "Received chat completion request"
    );

    // Convert API request to internal format
    let internal_request = ChatCompletionRequest {
        model: request.model.clone(),
        messages: request.messages,
        temperature: request.temperature.or(Some(0.7)),
        max_tokens: request.max_tokens.or(Some(4096)),
        top_p: request.top_p,
        tools: request.tools,
        stream: Some(false), // We handle streaming separately below
        stop: request.stop,
    };

    // Handle streaming vs non-streaming
    if request.stream.unwrap_or(false) {
        // TODO: Implement proper SSE streaming (requires axum sse feature configuration)
        // For now, return a message indicating streaming is not yet implemented
        (
            StatusCode::NOT_IMPLEMENTED,
            Json(serde_json::json!({
                "error": {
                    "message": "Streaming is not yet fully implemented. Use stream=false for now.",
                    "type": "not_implemented"
                }
            })),
        ).into_response()
    } else {
        // Return normal response
        match state.provider.chat_completion(internal_request).await {
            Ok(response) => {
                let latency_ms = start_time.elapsed().as_millis() as f64;
                
                tracing::info!(
                    latency_ms = latency_ms,
                    tokens = ?response.usage,
                    "Chat completion successful"
                );

                (
                    StatusCode::OK,
                    Json(ChatCompletionApiResponse {
                        id: response.id,
                        object: "chat.completion".to_string(),
                        created: response.created,
                        model: response.model,
                        choices: response.choices,
                        usage: response.usage,
                    }),
                ).into_response()
            }
            Err(e) => {
                tracing::error!(error = %e, "Chat completion failed");
                
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": {
                            "message": e.to_string(),
                            "type": "server_error"
                        }
                    })),
                ).into_response()
            }
        }
    }
}

/// POST /v1/embeddings
async fn embeddings(
    State(state): State<AppState>,
    Json(request): Json<EmbeddingApiRequest>,
) -> impl IntoResponse {
    tracing::info!(
        model = %request.model,
        "Received embedding request"
    );

    let input_vec = request.input.into_vec();
    
    let internal_request = EmbeddingRequest {
        model: request.model.clone(),
        input: input_vec,
        encoding_format: request.encoding_format,
    };

    match state.provider.embeddings(internal_request).await {
        Ok(response) => {
            tracing::info!(
                embeddings = response.data.len(),
                "Embeddings generated successfully"
            );

            (
                StatusCode::OK,
                Json(response),
            ).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "Embedding generation failed");
            
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": {
                        "message": e.to_string(),
                        "type": "embedding_error"
                    }
                })),
            ).into_response()
        }
    }
}

/// GET /v1/models
async fn list_models(
    State(state): State<AppState>,
) -> impl IntoResponse {
    tracing::info!("Listing models");

    match state.provider.list_models().await {
        Ok(models) => {
            let openai_models: Vec<ModelInfoOpenAi> = models.iter()
                .map(|m| ModelInfoOpenAi {
                    id: m.id.clone(),
                    object: "model".to_string(),
                    created: chrono::Utc::now().timestamp(),
                    owned_by: m.owned_by.to_string(),
                })
                .collect();

            (
                StatusCode::OK,
                Json(ModelsListResponse {
                    object: "list".to_string(),
                    data: openai_models,
                }),
            ).into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to list models");
            
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": {
                        "message": e.to_string(),
                        "type": "models_error"
                    }
                })),
            ).into_response()
        }
    }
}

/// GET /health
async fn health_check(State(state): State<AppState>) -> impl IntoResponse {
    let healthy = state.provider.health_check().await.unwrap_or(false);

    (
        StatusCode::OK,
        Json(HealthCheckResponse {
            status: if healthy { "healthy" } else { "unhealthy" }.to_string(),
            model: state.provider.model_name().to_string(),
            provider: state.provider.provider_type().to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }),
    )
}
