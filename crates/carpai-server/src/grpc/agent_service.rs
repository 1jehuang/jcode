//! Agent service - ChatCompletion RPC handler
//!
//! This module implements the gRPC AgentService by delegating to carpai-core's
//! execute_agent_turn() function.

use crate::grpc::carpai::agent::{
    agent_service_server::{AgentService, AgentServiceServer},
    ChatCompletionRequest, ChatCompletionResponse, ChatCompletionChunk,
    AppendMessageRequest, AppendMessageResponse,
    GetSessionMessagesRequest, GetSessionMessagesResponse,
};
use crate::service::context::ServerContext;
use tonic::{Request, Response, Status, Streaming};
use tracing::info;
use std::sync::Arc;

/// Agent service implementation that delegates to carpai-core
#[derive(Debug)]
pub struct AgentServiceImpl {
    /// Server context containing the AgentContext
    ctx: Arc<ServerContext>,
}

impl AgentServiceImpl {
    /// Create a new AgentServiceImpl with the given server context
    pub fn new(ctx: Arc<ServerContext>) -> Self {
        Self { ctx }
    }
}

#[tonic::async_trait]
impl AgentService for AgentServiceImpl {
    async fn chat_completion(
        &self,
        request: Request<ChatCompletionRequest>,
    ) -> Result<Response<ChatCompletionResponse>, Status> {
        let req = request.into_inner();
        info!(
            model = %req.model,
            messages_count = req.messages.len(),
            "Received chat completion request"
        );

        // Extract user message from the last user message in the conversation
        let user_message = req.messages.iter()
            .rev()
            .find(|msg| msg.role == "user")
            .map(|msg| msg.content.as_str())
            .ok_or_else(|| Status::invalid_argument("No user message found"))?;

        // Execute agent turn using carpai-core
        let output = carpai_core::execute_agent_turn(
            &self.ctx.agent_ctx,
            user_message,
        ).await.map_err(|e| {
            Status::internal(format!("Agent execution failed: {}", e))
        })?;

        // Convert AgentTurnOutput to ChatCompletionResponse
        let response = ChatCompletionResponse {
            id: output.session_id.to_string(),
            object: "chat.completion".to_string(),
            created: chrono::Utc::now().timestamp() as i64,
            model: req.model,
            choices: vec![carpai::agent::ChatChoice {
                index: 0,
                message: Some(carpai::agent::ChatMessage {
                    role: "assistant".to_string(),
                    content: output.text,
                    name: None,
                }),
                finish_reason: "stop".to_string(),
            }],
            usage: Some(carpai::agent::UsageInfo {
                prompt_tokens: output.usage.prompt_tokens as i32,
                completion_tokens: output.usage.completion_tokens as i32,
                total_tokens: output.usage.total_tokens as i32,
            }),
        };

        Ok(Response::new(response))
    }

    type ChatCompletionStreamStream =
        std::pin::Pin<Box<dyn tokio_stream::Stream<Item = Result<ChatCompletionChunk, Status>> + Send + 'static>>;

    async fn chat_completion_stream(
        &self,
        request: Request<ChatCompletionRequest>,
    ) -> Result<Response<Self::ChatCompletionStreamStream>, Status> {
        let req = request.into_inner();
        info!(
            model = %req.model,
            "Received streaming chat completion request"
        );

        // TODO: Implement proper streaming in Week 7-8
        // For now, return a single chunk with the response
        let user_message = req.messages.iter()
            .rev()
            .find(|msg| msg.role == "user")
            .map(|msg| msg.content.as_str())
            .ok_or_else(|| Status::invalid_argument("No user message found"))?;

        let output = carpai_core::execute_agent_turn(
            &self.ctx.agent_ctx,
            user_message,
        ).await.map_err(|e| {
            Status::internal(format!("Agent execution failed: {}", e))
        })?;

        let chunk = ChatCompletionChunk {
            id: output.session_id.to_string(),
            object: "chat.completion.chunk".to_string(),
            created: chrono::Utc::now().timestamp() as i64,
            model: req.model,
            choices: vec![carpai::agent::ChatChoiceChunk {
                index: 0,
                delta: Some(carpai::agent::ChatMessage {
                    role: "assistant".to_string(),
                    content: output.text,
                    name: None,
                }),
                finish_reason: Some("stop".to_string()),
            }],
        };

        let stream = tokio_stream::once(Ok(chunk));
        Ok(Response::new(Box::pin(stream)))
    }

    async fn append_message(
        &self,
        request: Request<AppendMessageRequest>,
    ) -> Result<Response<AppendMessageResponse>, Status> {
        let req = request.into_inner();
        info!(
            session_id = %req.session_id,
            "Received append message request"
        );

        // Use SessionStore from AgentContext to append message
        use carpai_internal::{SessionStore, SessionId, StoredMessage, MessageRole, ContentBlock};

        let session_id = SessionId(req.session_id);
        let message = StoredMessage {
            id: uuid::Uuid::new_v4().to_string(),
            role: match req.role.as_str() {
                "user" => MessageRole::User,
                "assistant" => MessageRole::Assistant,
                "system" => MessageRole::System,
                _ => MessageRole::User,
            },
            content: vec![ContentBlock::Text { text: req.content }],
            timestamp: chrono::Utc::now(),
            token_usage: None,
            model: None,
        };

        self.ctx.agent_ctx.sessions.append_messages(session_id, vec![message])
            .await
            .map_err(|e| Status::internal(format!("Failed to append message: {}", e)))?;

        Ok(Response::new(AppendMessageResponse {
            success: true,
        }))
    }

    async fn get_session_messages(
        &self,
        request: Request<GetSessionMessagesRequest>,
    ) -> Result<Response<GetSessionMessagesResponse>, Status> {
        let req = request.into_inner();
        info!(session_id = %req.session_id, "Received get session messages request");

        // Use SessionStore from AgentContext to load session
        use carpai_internal::{SessionStore, SessionId};

        let session_id = SessionId(req.session_id);
        let session = self.ctx.agent_ctx.sessions.load_session(&session_id)
            .await
            .map_err(|e| Status::internal(format!("Failed to load session: {}", e)))?
            .ok_or_else(|| Status::not_found("Session not found"))?;

        let messages = session.messages.iter().map(|msg| {
            let content = if msg.content.is_empty() {
                String::new()
            } else {
                match &msg.content[0] {
                    carpai_internal::ContentBlock::Text { text } => text.clone(),
                    carpai_internal::ContentBlock::ToolUse { name, input, .. } => {
                        format!("[Tool Call] {}({})", name, input)
                    }
                    carpai_internal::ContentBlock::ToolResult { content, .. } => content.clone(),
                    carpai_internal::ContentBlock::Thinking { text, .. } => text.clone(),
                }
            };

            carpai::agent::ChatMessage {
                role: match msg.role {
                    carpai_internal::MessageRole::User => "user".to_string(),
                    carpai_internal::MessageRole::Assistant => "assistant".to_string(),
                    carpai_internal::MessageRole::System => "system".to_string(),
                    carpai_internal::MessageRole::Tool => "tool".to_string(),
                },
                content,
                name: None,
            }
        }).collect();

        Ok(Response::new(GetSessionMessagesResponse {
            messages,
            session_id: req.session_id,
        }))
    }
}

pub fn create_agent_service(ctx: Arc<ServerContext>) -> AgentServiceServer<AgentServiceImpl> {
    AgentServiceServer::new(AgentServiceImpl::new(ctx))
}
