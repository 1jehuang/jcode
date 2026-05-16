//! OpenAI 兼容 API 路由（同 jcode-llm 的 rest_api，但扩展了多模型路由
//! 和管理层面的认证、用量统计、负载均衡）
//!
//! 端点:
//! - POST /v1/chat/completions  — 聊天补全（同步+流）
//! - POST /v1/embeddings        — 向量嵌入
//! - GET  /v1/models             — 模型列表

use axum::{
    extract::{State, Query},
    http::StatusCode,
    response::{IntoResponse, Json, sse::Sse},
    routing::{get, post},
    Router,
};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

use crate::enterprise::EnterpriseServerState;
use crate::auth::JwtClaims;
use crate::usage::{QuotaResult, UsageRecord, UsageManager};

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

    // 1. 从请求中找出使用的是哪个模型，获取对应的 Provider
    let provider = state.find_provider(&request.model).await;

    let provider = match provider {
        Some(p) => p,
        None => return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": {
                    "message": format!("模型 '{}' 不可用", request.model),
                    "type": "model_not_found"
                }
            })),
        ).into_response()
    };

    // 2. 构建内部请求
    let internal_request = jcode_llm::ChatCompletionRequest {
        model: request.model.clone(),
        messages: request.messages.iter().map(|m| jcode_llm::ChatMessage {
            role: match m.role.as_str() {
                "system" => jcode_llm::MessageRole::System,
                "assistant" => jcode_llm::MessageRole::Assistant,
                _ => jcode_llm::MessageRole::User,
            },
            content: m.content.clone(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }).collect(),
        temperature: request.temperature,
        max_tokens: request.max_tokens.or(Some(4096)),
        top_p: request.top_p,
        tools: None,
        stream: Some(false),
        stop: None,
    };

    // 3. 调用推理
    match provider.chat_completion(internal_request).await {
        Ok(response) => {
            let latency_ms = start.elapsed().as_millis() as u64;

            // 记录用量（异步，不阻塞响应）
            let state_clone = state.clone();
            let model = request.model.clone();
            tokio::spawn(async move {
                if let Some(prompt_tokens) = response.usage.as_ref().map(|u| u.prompt_tokens) {
                    state_clone.usage_manager.write().await.record_usage(
                        UsageRecord {
                            id: uuid::Uuid::new_v4().to_string(),
                            org_id: "system".into(),
                            user_id: None,
                            model_name: model,
                            prompt_tokens: response.usage.as_ref().map(|u| u.prompt_tokens).unwrap_or(0),
                            completion_tokens: response.usage.as_ref().map(|u| u.completion_tokens).unwrap_or(0),
                            total_tokens: response.usage.as_ref().map(|u| u.total_tokens).unwrap_or(0),
                            latency_ms,
                            request_type: "chat".into(),
                            created_at: chrono::Utc::now(),
                        }
                    ).await;
                }
            });

            (
                StatusCode::OK,
                Json(ChatResponse {
                    id: response.id,
                    object: "chat.completion".into(),
                    created: response.created,
                    model: response.model,
                    choices: response.choices.iter().map(|c| Choice {
                        index: c.index,
                        message: ResponseMessage {
                            role: "assistant".into(),
                            content: c.message.content.clone(),
                        },
                        finish_reason: c.finish_reason.clone(),
                    }).collect(),
                    usage: response.usage.as_ref().map(|u| UsageInfo {
                        prompt_tokens: u.prompt_tokens,
                        completion_tokens: u.completion_tokens,
                        total_tokens: u.total_tokens,
                    }).unwrap_or(UsageInfo { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 }),
                }),
            ).into_response()
        }
        Err(e) => {
            tracing::error!("Chat completion failed: {:?}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": {
                        "message": e.to_string(),
                        "type": "inference_error"
                    }
                })),
            ).into_response()
        }
    }
}

/// POST /v1/embeddings
async fn embeddings_handler(
    State(state): State<Arc<EnterpriseServerState>>,
) -> impl IntoResponse {
    // 嵌入向量生成（暂不实现，占位）
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({"error": "embedding not yet implemented"})),
    )
}

/// GET /v1/models
async fn list_models_handler(
    State(state): State<Arc<EnterpriseServerState>>,
) -> impl IntoResponse {
    let models = state.list_available_models().await;

    (
        StatusCode::OK,
        Json(serde_json::json!({
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
        Json(serde_json::json!({
            "status": if healthy { "healthy" } else { "degraded" },
            "version": env!("CARGO_PKG_VERSION"),
            "models": state.config.models.supported_models.len(),
            "nodes": state.node_count().await,
        })),
    )
}
