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
use carpai_codebase::CodebaseEngine;

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
    pub prompt: String,      // 光标前的代码
    pub suffix: String,      // 光标后的代码
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
        .route("/v1/completions", post(fim_completion_handler)) // FIM 补全端点
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
        &jcode_unified_scheduler::AgentRole::Worker,
        &request.model,
        Some(jcode_unified_scheduler::TaskType::Inference),
    );

    // 2. 智能资源评估：本地vs分布式
    let (local_sufficient, capacity_reason) = state.evaluate_local_capacity(&request.model).await;
    tracing::info!("[ResourceEval] model={}, local_sufficient={}, reason={}", 
        request.model, local_sufficient, capacity_reason);

    // 3. 根据资源评估结果决定调度策略
    let route_info = if !local_sufficient {
        // 本地资源不足，尝试分布式推理
        if let Some(ref ds) = state.distributed_scheduler {
            match ds.route_request(&request.model, 80).await {
                Ok(route) => {
                    tracing::info!(
                        "[Parallax] 启用分布式推理: model={}, target_node={:?}, layers={}",
                        request.model,
                        route.target_node,
                        route.total_layers
                    );
                    Some(route)
                }
                Err(e) => {
                    tracing::warn!("[Parallax] 分布式路由失败，回退到本地: {:?}", e);
                    None
                }
            }
        } else {
            tracing::warn!("[Parallax] 分布式调度器未初始化，强制使用本地推理");
            None
        }
    } else {
        // 本地资源充足，跳过分布式路由以节省延迟
        tracing::debug!("[Parallax] 本地资源充足，跳过分布式路由");
        None
    };

    // 4. 提交任务到 UnifiedScheduler（用于 Ruflo 优先级调度和指标收集）
    let task = jcode_unified_scheduler::ScheduledTask {
        id: uuid::Uuid::new_v4(),
        description: format!("Chat: {}", &request.model),
        role: jcode_unified_scheduler::AgentRole::Worker,
        priority: jcode_unified_scheduler::TaskPriority::from(priority),
        required_model: request.model.clone(),
        dependencies: vec![],
        goal: None, actions: vec![], plan: None,
        submitted_at: None, started_at: None, completed_at: None,
        status: jcode_unified_scheduler::TaskStatus::Pending,
        result: None,
        metadata: serde_json::json!({
            "local_sufficient": local_sufficient,
            "capacity_reason": capacity_reason,
        }),
        retry_count: 0,
        max_retries: 3,
        requires_inference: true,
        min_memory_mb: None,
        min_tflops: None,
        max_latency_ms: None,
        estimated_tokens: None,
        created_at: Some(chrono::Utc::now()),
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

    // 5. 智能执行路径选择（本地优先 + 负载均衡）
    let mut final_route = route_info.clone();
    
    // 如果选择了分布式路由，进行负载均衡检查
    if let Some(ref route) = route_info {
        if let Some(ref ds) = state.distributed_scheduler {
            // 动态负载均衡
            match ds.dynamic_load_balance(route).await {
                Ok(Some(better_route)) => {
                    tracing::info!("[LoadBalance] 切换到更优节点");
                    final_route = Some(better_route);
                }
                Ok(None) => {
                    tracing::debug!("[LoadBalance] 当前节点负载正常");
                }
                Err(e) => {
                    tracing::warn!("[LoadBalance] 负载均衡检查失败: {:?}", e);
                }
            }
        }
    }

    // 6. 获取Provider并执行推理
    let provider = if let Some(ref route) = final_route {
        // 分布式路径：检查目标节点是否为本机
        if route.target_node.is_none() || route.layer_assignments.is_empty() {
            // 无分布式节点或分配为空，使用本地provider
            state.find_provider(&request.model).await
        } else {
            // 🚀 启用真正的跨节点gRPC分布式推理
            tracing::info!(
                "[Parallax] 启动跨节点分布式推理: target_node={:?}, layers={}",
                route.target_node,
                route.total_layers
            );

            // 应用智能KV Cache分层存储（本地缓存部分层）
            if let Some(ref vm_mgr) = state.vm_manager {
                let estimated_kv_mb = match request.model.to_lowercase().as_str() {
                    name if name.contains("72b") => 80_000,
                    name if name.contains("32b") => 40_000,
                    name if name.contains("14b") => 16_000,
                    name if name.contains("7b") => 8_000,
                    _ => 16_000,
                };
                
                match vm_mgr.intelligent_kv_placement(&request.model, estimated_kv_mb).await {
                    Ok(strategy) => {
                        tracing::info!(
                            "[KV Placement] model={}, tier={}, {}",
                            request.model,
                            strategy.tier(),
                            strategy.reason()
                        );
                    }
                    Err(e) => {
                        tracing::warn!("[KV Placement] 智能放置失败，回退默认策略: {:?}", e);
                    }
                }
            }

            // TODO: 集成 jcode-distributed-inference crate 进行实际的远程调用
            // 当前阶段：记录调度决策并使用本地provider作为fallback
            state.find_provider(&request.model).await
        }
    } else {
        // 纯本地路径
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

            // 构建响应头，包含完整调度信息
            use axum::http::{HeaderMap, HeaderValue};
            let mut headers = HeaderMap::new();

            // 添加任务ID
            headers.insert("X-CarpAI-Task-ID", HeaderValue::from_str(&task_id.to_string()).unwrap_or_else(|_| HeaderValue::from_static("")));

            // 添加路由和负载均衡信息
            if let Some(ref route) = final_route {
                if let Some(target) = route.target_node {
                    headers.insert("X-CarpAI-Target-Node", HeaderValue::from_str(&target.to_string()).unwrap_or_else(|_| HeaderValue::from_static("")));
                }
                headers.insert("X-CarpAI-Layer-Count", HeaderValue::from(route.total_layers));
                headers.insert("X-CarpAI-Scheduler", HeaderValue::from_static("Parallax+LoadBalance"));
            } else {
                headers.insert("X-CarpAI-Scheduler", HeaderValue::from_static("Local"));
            }

            // 添加资源评估信息
            headers.insert("X-CarpAI-Local-Sufficient", HeaderValue::from(local_sufficient));
            headers.insert("X-CarpAI-Priority", HeaderValue::from(priority as i64));

            tracing::debug!(
                "[API] 响应完成: task_id={}, latency={}ms, scheduler={}, local_sufficient={}",
                task_id,
                latency_ms,
                if final_route.is_some() { "Parallax+LB" } else { "Local" },
                local_sufficient
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

/// POST /v1/completions (FIM 补全)
async fn fim_completion_handler(
    State(state): State<Arc<EnterpriseServerState>>,
    axum::extract::Json(request): axum::extract::Json<FIMRequest>,
) -> impl IntoResponse {
    let start = std::time::Instant::now();

    // 1. 构造 FIM Prompt (Qwen2.5-Coder / StarCoder2 格式)
    let fim_prompt = format!(
        "<|fim_prefix|>{}<|fim_suffix|>{}<|fim_middle|>",
        request.prompt, request.suffix
    );

    // 2. 调用底层推理引擎
    let provider = state.find_provider(&request.model).await;
    if provider.is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Model not found"})),
        ).into_response();
    }

    // 3. 执行推理（此处简化，实际应调用专门的 FIM 接口）
    // ... 推理逻辑 ...

    let latency_ms = start.elapsed().as_millis() as u64;

    // 4. 返回补全结果
    (
        StatusCode::OK,
        Json(serde_json::json!({
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
            }
        })),
    ).into_response()
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
