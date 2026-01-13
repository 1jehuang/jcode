pub mod claude;
pub mod openai;

use crate::message::{Message, StreamEvent, ToolDefinition};
use anyhow::Result;
use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;

/// Stream of events from a provider
pub type EventStream = Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>;

/// Provider trait for LLM backends
#[async_trait]
pub trait Provider: Send + Sync {
    /// Send messages and get a streaming response
    /// resume_session_id: Optional session ID to resume a previous conversation (provider-specific)
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream>;

    /// Get the provider name
    fn name(&self) -> &str;

    /// Get the model identifier being used
    fn model(&self) -> &str {
        "unknown"
    }
}
