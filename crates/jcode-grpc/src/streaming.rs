//! SSE (Server-Sent Events) Streaming Utilities
//!
//! This module provides streaming utilities for implementing
//! efficient server-side streaming of LLM responses.

use serde::{Serialize, Deserialize};

/// A single chunk in the stream
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamChunk {
    /// Unique identifier for this response
    pub id: String,
    
    /// Model used for generation
    pub model: String,
    
    /// Content delta (text fragment)
    pub content: String,
    
    /// Whether this is the final chunk
    pub done: bool,
    
    /// Token usage information (only present in final chunk)
    pub usage: Option<UsageInfo>,
}

/// Token usage statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageInfo {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// SSE Streamer for converting LLM streams to SSE format
pub struct SseStreamer {
    buffer: String,
}

impl SseStreamer {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }
    
    /// Format a chunk as SSE data
    pub fn format_sse_event(chunk: &StreamChunk) -> String {
        let json = serde_json::to_string(chunk).expect("Failed to serialize chunk");
        format!("data: {}\n\n", json)
    }
    
    /// Format SSE done event
    pub fn format_sse_done() -> String {
        "data: [DONE]\n\n".to_string()
    }
    
    /// Accumulate chunks and return complete content when done
    pub fn accumulate(&mut self, chunk: &StreamChunk) -> Option<String> {
        if !chunk.content.is_empty() {
            self.buffer.push_str(&chunk.content);
        }
        
        if chunk.done {
            let full_content = std::mem::take(&mut self.buffer);
            Some(full_content)
        } else {
            None
        }
    }
    
    /// Reset the accumulator
    pub fn reset(&mut self) {
        self.buffer.clear();
    }
}

impl Default for SseStreamer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_sse_formatting() {
        let chunk = StreamChunk {
            id: "test-123".to_string(),
            model: "deepseek-chat".to_string(),
            content: "Hello".to_string(),
            done: false,
            usage: None,
        };
        
        let sse = SseStreamer::format_sse_event(&chunk);
        assert!(sse.starts_with("data: "));
        assert!(sse.ends_with("\n\n"));
        assert!(sse.contains("\"Hello\""));
    }
    
    #[test]
    fn test_accumulator() {
        let mut streamer = SseStreamer::new();
        
        assert_eq!(streamer.accumulate(&StreamChunk {
            id: "test".to_string(),
            model: "test".to_string(),
            content: "Hello ".to_string(),
            done: false,
            usage: None,
        }), None);
        
        assert_eq!(streamer.accumulate(&StreamChunk {
            id: "test".to_string(),
            model: "test".to_string(),
            content: "World!".to_string(),
            done: true,
            usage: None,
        }), Some("Hello World!".to_string()));
    }
    
    #[test]
    fn test_done_event() {
        let done = SseStreamer::format_sse_done();
        assert_eq!(done, "data: [DONE]\n\n");
    }
}
