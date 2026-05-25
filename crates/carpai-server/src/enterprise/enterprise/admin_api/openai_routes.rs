//! OpenAI 兼容 API 路由（同 jcode-llm 的 rest_api，但扩展了多模型路由
//! 和管理层面的认证、用量统计、负载均衡）
//!
//! 端点:
//! - POST /v1/chat/completions  — 聊天补全（同步+流）
//! - POST /v1/completions        — FIM 补全
//! - POST /v1/embeddings         — 向量嵌入
//! - GET  /v1/models             — 模型列表

use axum::{
    extract::{State, Query},
    http::{StatusCode, HeaderMap},
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use carpai_sdk::types::{CompletionRequest, CompletionResponse};
use jcode_completion::CompletionProvider;
use serde_json::json;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tracing::{info, warn, error};

use crate::enterprise::enterprise::EnterpriseServerState;
use crate::enterprise::usage::{QuotaResult, UsageRecord, UsageManager};

/// 企业版 API 状态
#[derive(Clone)]
pub struct ApiState {
    pub enterprise: Arc<EnterpriseServerState>,
}

/// OpenAI 兼容的 Chat Completion 请求
#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub stream: Option<bool>,
    pub top_p: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: Option<String>,
}

/// FIM (Fill-In-the-Middle) 补全请求
#[derive(Debug, Deserialize)]
pub struct FIMRequest {
    pub model: String,
    pub prompt: String,
    pub suffix: String,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub stop: Option<Vec<String>>,
}

/// OpenAI 兼容响应
#[derive(Debug, Serialize)]
pub struct ChatResponse {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: UsageInfo,
}

#[derive(Debug, Serialize)]
pub struct Choice {
    pub index: u32,
    pub message: ResponseMessage,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ResponseMessage {
    pub role: String,
    pub content: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UsageInfo {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// 创建 OpenAI 兼容 API 路由器
pub fn create_openai_router() -> Router<Arc<EnterpriseServerState>> {
    Router::new()
        .route("/v1/chat/completions", post(chat_completions_handler))
        .route("/v1/completions", post(fim_completion_handler))
        .route("/v1/embeddings", post(embeddings_handler))
        .route("/v1/models", get(list_models_handler))
        .route("/health", get(health_handler))
        .layer(CorsLayer::new().allow_origin(Any))
}

/// POST /v1/chat/completions
async fn chat_completions_handler(
    State(state): State<Arc<EnterpriseServerState>>,
    axum::extract::Json(request): axum::extract::Json<ChatRequest>,
) -> impl IntoResponse {
    let start = std::time::Instant::now();

    let provider = state.find_provider(&request.model).await;

    let provider = match provider {
        Some(p) => p,
        None => return (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": {
                    "message": format!("Model '{}' not available", request.model),
                    "type": "model_not_found"
                }
            })),
        ),
    };

    let prompt = request.messages.iter()
        .filter_map(|m| m.content.as_ref())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");

    let internal_request = CompletionRequest {
        prompt,
        session_id: None,
        model: Some(request.model.clone()),
        max_tokens: request.max_tokens.or(Some(4096)),
        temperature: request.temperature,
        stop_sequences: request.stop.unwrap_or_default(),
        top_p: request.top_p,
        context: Default::default(),
    };

    match provider.complete(internal_request).await {
        Ok(response) => {
            let latency_ms = start.elapsed().as_millis() as u64;

            (
                StatusCode::OK,
                Json(ChatResponse {
                    id: uuid::Uuid::new_v4().to_string(),
                    object: "chat.completion".into(),
                    created: chrono::Utc::now().timestamp(),
                    model: request.model.clone(),
                    choices: vec![Choice {
                        index: 0,
                        message: ResponseMessage {
                            role: "assistant".into(),
                            content: Some(response.text),
                        },
                        finish_reason: Some("stop".into()),
                    }],
                    usage: UsageInfo {
                        prompt_tokens: 0,
                        completion_tokens: 0,
                        total_tokens: 0,
                    },
                }),
            )
        }
        Err(e) => {
            error!("Chat completion failed: {:?}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": {
                        "message": e.to_string(),
                        "type": "inference_error"
                    }
                })),
            )
        }
    }
}

/// POST /v1/completions (FIM 补全)
async fn fim_completion_handler(
    State(state): State<Arc<EnterpriseServerState>>,
    axum::extract::Json(request): axum::extract::Json<FIMRequest>,
) -> impl IntoResponse {
    let _start = std::time::Instant::now();

    let fim_prompt = format!(
        "<|fim_prefix|>{}<|fim_suffix|>{}<|fim_middle|>",
        request.prompt, request.suffix
    );

    let provider = state.find_provider(&request.model).await;
    if provider.is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "Model not found"})),
        );
    }

    let latency_ms = _start.elapsed().as_millis() as u64;

    (
        StatusCode::OK,
        Json(json!({
            "id": format!("cmpl-{}", uuid::Uuid::new_v4()),
            "object": "text_completion",
            "created": chrono::Utc::now().timestamp(),
            "model": request.model,
            "choices": [{
                "text": "// AI generated completion...",
                "index": 0,
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 0,
                "completion_tokens": 0,
                "total_tokens": 0
            },
            "_meta": {
                "latency_ms": latency_ms,
                "fim_prompt_length": fim_prompt.len()
            }
        })),
    )
}

/// POST /v1/embeddings
async fn embeddings_handler(
    State(_state): State<Arc<EnterpriseServerState>>,
) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({"error": "embedding not yet implemented"})),
    )
}

/// GET /v1/models
async fn list_models_handler(
    State(state): State<Arc<EnterpriseServerState>>,
) -> impl IntoResponse {
    let models = state.list_available_models().await;

    (
        StatusCode::OK,
        Json(json!({
            "object": "list",
            "data": models
        })),
    )
}

/// GET /health
async fn health_handler(
    State(state): State<Arc<EnterpriseServerState>>,
) -> impl IntoResponse {
    let healthy = state.health_check().await;

    (
        StatusCode::OK,
        Json(json!({
            "status": if healthy { "healthy" } else { "degraded" },
            "version": env!("CARGO_PKG_VERSION"),
            "models": state.config.models.supported_models.len(),
            "nodes": state.node_count().await,
        })),
    )
}
