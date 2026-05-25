//! REST API router and handlers (OpenAI compatible)
//!
//! This module implements REST endpoints that delegate to carpai-core's
//! execute_agent_turn() function and SessionStore.

use crate::config::ServerConfig;
use crate::service::context::ServerContext;
use axum::{
    Router,
    routing::{get, post},
    Json, extract::State,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub fn router_with_ctx(ctx: Arc<ServerContext>) -> Router<Arc<ServerContext>> {
    Router::new()
        // OpenAI compatible endpoints
        .route("/chat/completions", post(chat_completions))
        .route("/completions", post(completions))
        // Session management
        .route("/sessions", get(list_sessions).post(create_session))
        .route("/sessions/:id", get(get_session).delete(delete_session))
        // Tool execution
        .route("/tools", get(list_tools))
        .route("/tools/:name", post(execute_tool))
        // Health check
        .route("/health", get(health_check))
}

// Legacy function for backward compatibility
pub fn router(_config: &ServerConfig) -> Router {
    Router::new()
        .route("/chat/completions", post(chat_completions))
        .route("/completions", post(completions))
        .route("/sessions", get(list_sessions))
        .route("/sessions/:id", get(get_session).delete(delete_session))
        .route("/tools", get(list_tools))
        .route("/tools/:name", post(execute_tool))
        .route("/health", get(health_check))
}

// === OpenAI Compatible Types ===

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<ChatChoice>,
    pub usage: UsageInfo,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatChoice {
    pub index: u32,
    pub message: ChatMessage,
    pub finish_reason: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UsageInfo {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

// === Handlers ===

/// POST /v1/chat/completions - OpenAI compatible chat endpoint
async fn chat_completions(
    State(_ctx): State<Arc<ServerContext>>,
    Json(req): Json<ChatCompletionRequest>,
) -> Json<ChatCompletionResponse> {
    // TODO: Wire up with ServerContext in Week 7-8 when Axum state is properly configured
    // For now, return a placeholder response

    // Extract user message
    let _user_message = req.messages.iter()
        .rev()
        .find(|msg| msg.role == "user")
        .map(|msg| msg.content.as_str());

    // When ready, call execute_agent_turn:
    // let output = carpai_core::execute_agent_turn(&ctx.agent_ctx, user_message)
    //     .await
    //     .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Json(ChatCompletionResponse {
        id: "chatcmpl-placeholder".to_string(),
        object: "chat.completion".to_string(),
        created: chrono::Utc::now().timestamp(),
        model: req.model,
        choices: vec![ChatChoice {
            index: 0,
            message: ChatMessage {
                role: "assistant".to_string(),
                content: "REST integration pending - use gRPC for now".to_string(),
                name: None,
            },
            finish_reason: "stop".to_string(),
        }],
        usage: UsageInfo {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        },
    })
}

async fn completions() -> &'static str {
    "TODO"
}

async fn list_sessions() -> &'static str {
    "TODO"
}

async fn create_session() -> &'static str {
    "TODO"
}

async fn get_session() -> &'static str {
    "TODO"
}

async fn delete_session() -> &'static str {
    "TODO"
}

async fn list_tools() -> &'static str {
    "TODO"
}

async fn execute_tool() -> &'static str {
    "TODO"
}

async fn health_check() -> &'static str {
    "OK"
}
