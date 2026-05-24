//! REST API Layer - Exposes Internal API traits via HTTP/REST
//!
//! Endpoints:
//! - POST /api/v1/completions/inline - Code completion
//! - POST /api/v1/chat - Chat completion
//! - GET  /api/v1/memory - Search memories
//! - POST /api/v1/tools/execute - Execute tool

use axum::{
    extract::State,
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tracing::info;

// Import Internal API traits
use carpai_internal::{
    CodeCompletion, CompletionRequest, CompletionCandidate,
    AuthProvider,
    InferenceEngine, InferenceRequest,
};

/// Application state shared across all handlers
#[derive(Clone)]
pub struct ApiState {
    pub completion_engine: Option<Arc<dyn CodeCompletion>>,
    pub auth_provider: Arc<dyn AuthProvider>,
    pub inference_engine: Option<Arc<dyn InferenceEngine>>,
}

/// Create the REST API router
pub fn create_router(state: ApiState) -> Router {
    Router::new()
        // Completion endpoints
        .route("/api/v1/completions/inline", post(inline_completion))
        .route("/api/v1/completions/prefetch", post(prefetch_completion))

        // Chat endpoints
        .route("/api/v1/chat/completions", post(chat_completion))

        // Memory endpoints
        .route("/api/v1/memory/search", get(search_memory))

        // Tool endpoints
        .route("/api/v1/tools/list", get(list_tools))
        .route("/api/v1/tools/execute", post(execute_tool))

        // Health check
        .route("/health", get(health_check))

        .with_state(state)
        .layer(CorsLayer::permissive())
}

// ============================================================================
// Completion Handlers
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct InlineCompletionRequest {
    pub file_path: String,
    pub content: String,
    pub cursor_line: usize,
    pub cursor_column: usize,
    pub language: Option<String>,
    pub max_candidates: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct InlineCompletionResponse {
    pub completions: Vec<CompletionCandidateDto>,
}

#[derive(Debug, Serialize)]
pub struct CompletionCandidateDto {
    pub id: String,
    pub text: String,
    pub score: f32,
    pub kind: String,
    pub label: Option<String>,
}

impl From<CompletionCandidate> for CompletionCandidateDto {
    fn from(candidate: CompletionCandidate) -> Self {
        Self {
            id: candidate.id,
            text: candidate.text,
            score: candidate.score,
            kind: format!("{:?}", candidate.kind),
            label: candidate.label,
        }
    }
}

/// POST /api/v1/completions/inline
async fn inline_completion(
    State(state): State<ApiState>,
    Json(req): Json<InlineCompletionRequest>,
) -> Result<Json<InlineCompletionResponse>, ApiError> {
    let engine = state.completion_engine
        .ok_or_else(|| ApiError::ServiceUnavailable("Completion engine not initialized".into()))?;

    let internal_req = CompletionRequest {
        file_path: req.file_path,
        content: req.content,
        cursor_line: req.cursor_line,
        cursor_column: req.cursor_column,
        language: req.language,
        trigger_char: None,
        max_candidates: req.max_candidates.or(Some(3)),
        timeout_ms: Some(2000),
    };

    let candidates = engine.complete(internal_req).await
        .map_err(|e| ApiError::Internal(format!("Completion failed: {}", e)))?;

    let response = InlineCompletionResponse {
        completions: candidates.into_iter().map(Into::into).collect(),
    };

    Ok(Json(response))
}

/// POST /api/v1/completions/prefetch
async fn prefetch_completion(
    State(state): State<ApiState>,
    Json(req): Json<InlineCompletionRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let engine = state.completion_engine
        .ok_or_else(|| ApiError::ServiceUnavailable("Completion engine not initialized".into()))?;

    let internal_req = CompletionRequest {
        file_path: req.file_path,
        content: req.content,
        cursor_line: req.cursor_line,
        cursor_column: req.cursor_column,
        language: req.language,
        trigger_char: None,
        max_candidates: req.max_candidates,
        timeout_ms: Some(5000),
    };

    tokio::spawn(async move {
        if let Err(e) = engine.prefetch(internal_req).await {
            tracing::warn!("Prefetch failed: {:?}", e);
        }
    });

    Ok(Json(serde_json::json!({ "status": "prefetching" })))
}

// ============================================================================
// Chat Handlers
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatChoice>,
    pub usage: UsageInfo,
}

#[derive(Debug, Serialize)]
pub struct ChatChoice {
    pub index: usize,
    pub message: ChatMessage,
    pub finish_reason: String,
}

#[derive(Debug, Serialize)]
pub struct UsageInfo {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub total_tokens: usize,
}

/// POST /api/v1/chat/completions
async fn chat_completion(
    State(state): State<ApiState>,
    Json(req): Json<ChatCompletionRequest>,
) -> Result<Json<ChatCompletionResponse>, ApiError> {
    let engine = state.inference_engine
        .ok_or_else(|| ApiError::ServiceUnavailable("Inference engine not initialized".into()))?;

    let messages: Vec<carpai_internal::inference::Message> = req.messages
        .into_iter()
        .map(|msg| carpai_internal::inference::Message {
            role: msg.role,
            content: msg.content,
        })
        .collect();

    let internal_req = InferenceRequest {
        model: req.model.clone(),
        prompt: String::new(),
        system_message: None,
        messages: Some(messages),
        max_tokens: req.max_tokens,
        temperature: req.temperature,
        top_p: None,
        stop: None,
        metadata: None,
    };

    let result = engine.infer(internal_req).await
        .map_err(|e| ApiError::Internal(format!("Inference failed: {}", e)))?;

    let response = ChatCompletionResponse {
        id: format!("chatcmpl-{}", uuid::Uuid::new_v4()),
        object: "chat.completion".to_string(),
        created: chrono::Utc::now().timestamp() as u64,
        model: req.model,
        choices: vec![ChatChoice {
            index: 0,
            message: ChatMessage {
                role: "assistant".to_string(),
                content: result.text,
            },
            finish_reason: format!("{:?}", result.finish_reason),
        }],
        usage: UsageInfo {
            prompt_tokens: result.usage.prompt_tokens,
            completion_tokens: result.usage.completion_tokens,
            total_tokens: result.usage.total_tokens,
        },
    };

    Ok(Json(response))
}

// ============================================================================
// Placeholder Handlers (TODO implementations)
// ============================================================================

#[derive(Debug, Serialize)]
pub struct MemorySearchResponse {
    pub results: Vec<serde_json::Value>,
    pub total: usize,
}

async fn search_memory(
    State(_state): State<ApiState>,
) -> Result<Json<MemorySearchResponse>, ApiError> {
    Ok(Json(MemorySearchResponse {
        results: vec![],
        total: 0,
    }))
}

#[derive(Debug, Serialize)]
pub struct ToolListResponse {
    pub tools: Vec<serde_json::Value>,
}

async fn list_tools(
    State(_state): State<ApiState>,
) -> Result<Json<ToolListResponse>, ApiError> {
    Ok(Json(ToolListResponse {
        tools: vec![],
    }))
}

#[derive(Debug, Deserialize)]
pub struct ToolExecuteRequest {
    pub tool_name: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct ToolExecuteResponse {
    pub success: bool,
    pub output: String,
    pub execution_time_ms: u64,
}

async fn execute_tool(
    State(_state): State<ApiState>,
    Json(_req): Json<ToolExecuteRequest>,
) -> Result<Json<ToolExecuteResponse>, ApiError> {
    Ok(Json(ToolExecuteResponse {
        success: false,
        output: "Not implemented".to_string(),
        execution_time_ms: 0,
    }))
}

async fn health_check() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "healthy",
        "version": env!("CARGO_PKG_VERSION"),
        "timestamp": chrono::Utc::now().to_rfc3339(),
    }))
}

// ============================================================================
// Error Types
// ============================================================================

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    #[error("Forbidden: {0}")]
    Forbidden(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Service unavailable: {0}")]
    ServiceUnavailable(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl axum::response::IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match &self {
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            ApiError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg),
            ApiError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg),
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            ApiError::ServiceUnavailable(msg) => (StatusCode::SERVICE_UNAVAILABLE, msg),
            ApiError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };

        let body = Json(serde_json::json!({
            "error": {
                "code": status.as_u16(),
                "message": message,
            }
        }));

        (status, body).into_response()
    }
}

/// Start the REST API server
pub async fn start_server(state: ApiState, port: u16) -> anyhow::Result<()> {
    let app = create_router(state);
    let addr = format!("0.0.0.0:{}", port);

    info!("Starting REST API server on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_health_check() {
        let response = health_check().await;
        let json = serde_json::to_value(response.0).unwrap();
        assert_eq!(json["status"], "healthy");
    }
}
