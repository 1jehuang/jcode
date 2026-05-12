//! Streaming support for CarpAI SDK

use crate::error::Result;
use crate::types::{StreamChunk, TokenUsage};
use futures::Stream;
use futures::StreamExt;

/// Stream event emitted during streaming
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// Text delta received
    TextDelta(String),

    /// Stream completed with final data
    Done {
        text: String,
        usage: Option<TokenUsage>,
        finish_reason: Option<String>,
    },

    /// Error occurred
    Error(String),
}

/// Handler for processing stream events
pub struct StreamHandler {
    /// Complete text accumulated so far
    full_text: String,

    /// Final usage information
    usage: Option<TokenUsage>,

    /// Finish reason
    finish_reason: Option<String>,

    /// Whether the stream is complete
    is_complete: bool,
}

impl StreamHandler {
    pub fn new() -> Self {
        Self {
            full_text: String::new(),
            usage: None,
            finish_reason: None,
            is_complete: false,
        }
    }

    /// Process a stream chunk and return the event
    pub fn process_chunk(&mut self, chunk: StreamChunk) -> StreamEvent {
        if chunk.is_final {
            self.is_complete = true;
            self.usage = chunk.usage;
            self.finish_reason = chunk.finish_reason;

            StreamEvent::Done {
                text: self.full_text.clone(),
                usage: self.usage.clone(),
                finish_reason: self.finish_reason.clone(),
            }
        } else if let Some(content) = chunk.content {
            self.full_text.push_str(&content);
            StreamEvent::TextDelta(content)
        } else {
            // Empty chunk (keepalive or metadata)
            StreamEvent::TextDelta(String::new())
        }
    }

    /// Get the complete text so far
    pub fn get_text(&self) -> &str {
        &self.full_text
    }

    /// Check if the stream is complete
    pub fn is_complete(&self) -> bool {
        self.is_complete
    }

    /// Reset the handler for reuse
    pub fn reset(&mut self) {
        self.full_text.clear();
        self.usage = None;
        self.finish_reason = None;
        self.is_complete = false;
    }
}

impl Default for StreamHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// Collect all events from a stream into a single result
pub async fn collect_stream<S>(stream: S) -> Result<(String, Option<TokenUsage>, Option<String>)>
where
    S: Stream<Item = Result<StreamChunk>> + Unpin,
{
    let mut handler = StreamHandler::new();

    let mut pinned = Box::pin(stream);
    while let Some(result) = pinned.next().await {
        match result {
            Ok(chunk) => {
                handler.process_chunk(chunk);
            }
            Err(e) => return Err(e),
        }
    }

    Ok((
        handler.get_text().to_string(),
        handler.usage.clone(),
        handler.finish_reason.clone(),
    ))
}
