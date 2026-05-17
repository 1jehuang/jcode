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
use jcode_unified_scheduler::{ScheduledTask, AgentRole, TaskPriority};
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

    // 1. 评估优先级 (Ruflo)
    let priority = state.priority_engine.evaluate(
        &jcode_unified_scheduler::AgentRole::Developer,
        &request.model,
        "chat",
    );

    // 2. 计算分布式推理最优路由 (Parallax Phase 1 + Phase 2)
    let route_info = if let Some(ref ds) = state.distributed_scheduler {
        match ds.route_request(&request.model, 80).await {
            Ok(route) => {
                tracing::info!(
                    "[Parallax] 路由决策: model={}, target_node={:?}, layers={}",
                    request.model,
                    route.target_node,
                    route.total_layers
                );
                Some(route)
            }
            Err(e) => {
                tracing::warn!("[Parallax] 路由失败，回退到本地推理: {:?}", e);
                None
            }
        }
    } else {
        None
    };

    // 3. 提交任务到 UnifiedScheduler（用于 Ruflo 优先级调度和指标收集）
    let task = jcode_unified_scheduler::ScheduledTask {
        id: uuid::Uuid::new_v4(),
        description: format!("Chat: {}", &request.model),
        role: jcode_unified_scheduler::AgentRole::Developer,
        priority: jcode_unified_scheduler::TaskPriority::from(priority),
        required_model: request.model.clone(),
        dependencies: vec![],
        goal: None, actions: vec![], plan: None,
        submitted_at: None, started_at: None, completed_at: None,
        status: jcode_unified_scheduler::TaskStatus::Pending,
        result: None, error_message: None,
        max_retries: 0, retry_count: 0,
    };
    let task_id = match state.scheduler.submit_task(task).await {
        Ok(id) => id,
        Err(e) => {
            tracing::error!("[UnifiedScheduler] 任务提交失败: {:?}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": {
                        "message": format!("调度器错误: {}", e),
                        "type": "scheduler_error"
                    }
                })),
            ).into_response();
        }
    };

    // 4. 根据路由结果选择执行路径
    let provider = if let Some(ref route) = route_info {
        // 分布式路径：检查目标节点是否为本机
        if route.target_node.is_none() || route.layer_assignments.is_empty() {
            // 无分布式节点或分配为空，使用本地provider
            state.find_provider(&request.model).await
        } else {
            // TODO: 实现跨节点分布式推理
            // 当前阶段：如果目标不是本机，记录警告并使用本地provider
            tracing::warn!(
                "[Parallax] 跨节点分布式推理尚未实现，回退到本地: target_node={:?}",
                route.target_node
            );
            state.find_provider(&request.model).await
        }
    } else {
        // 无路由信息，使用本地provider
        state.find_provider(&request.model).await
    };

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

    // 5. 调用推理
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

            // 构建响应头，包含调度信息
            use axum::http::{HeaderMap, HeaderValue};
            let mut headers = HeaderMap::new();

            // 添加任务ID
            headers.insert("X-CarpAI-Task-ID", HeaderValue::from_str(&task_id.to_string()).unwrap_or_else(|_| HeaderValue::from_static("")));

            // 添加路由信息
            if let Some(ref route) = route_info {
                if let Some(target) = route.target_node {
                    headers.insert("X-CarpAI-Target-Node", HeaderValue::from_str(&target.to_string()).unwrap_or_else(|_| HeaderValue::from_static("")));
                }
                headers.insert("X-CarpAI-Layer-Count", HeaderValue::from(route.total_layers));
                headers.insert("X-CarpAI-Scheduler", HeaderValue::from_static("Parallax"));
            } else {
                headers.insert("X-CarpAI-Scheduler", HeaderValue::from_static("Local"));
            }

            // 添加优先级信息
            headers.insert("X-CarpAI-Priority", HeaderValue::from(priority as i64));

            tracing::debug!(
                "[API] 响应完成: task_id={}, latency={}ms, scheduler={}",
                task_id,
                latency_ms,
                if route_info.is_some() { "Parallax" } else { "Local" }
            );

            (
                StatusCode::OK,
                headers,
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
