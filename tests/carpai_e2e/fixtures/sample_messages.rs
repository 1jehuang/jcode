//! Sample message templates for E2E tests
//!
//! Pre-defined messages used across different test scenarios to ensure
//! consistency and reduce duplication.

use serde_json::{json, Value};

/// Simple greeting message for basic connectivity tests
pub const SIMPLE_GREETING: &str = "Hello, CarpAI!";

/// Complex multi-turn conversation starter
pub const COMPLEX_QUERY: &str = "Can you help me understand how Rust's ownership system works?";

/// Code-related query for testing code generation capabilities
pub const CODE_REQUEST: &str = "Write a Rust function that sorts a vector of integers.";

/// Message expected to trigger tool use
pub const TOOL_TRIGGER_MESSAGE: &str = "What files are in the current directory?";

/// Long message to test context window handling
pub const LONG_CONTEXT_MESSAGE: &str = "
Please analyze the following code and suggest improvements:

```rust
fn process_data(items: Vec<i32>) -> Vec<i32> {
    let mut result = Vec::new();
    for item in items {
        if item > 0 {
            result.push(item * 2);
        } else {
            result.push(item.abs());
        }
    }
    result.sort();
    result.dedup();
    result
}
```

Consider:
1. Performance optimization opportunities
2. Error handling improvements
3. API design suggestions
4. Documentation needs
";

/// Expected response patterns (for validation, not exact match)
pub struct ResponsePatterns;

impl ResponsePatterns {
    /// Pattern indicating successful AI response (non-empty content)
    pub fn has_content(response: &str) -> bool {
        !response.trim().is_empty() && response.len() > 10
    }

    /// Check if response looks like it contains code
    pub fn contains_code_block(response: &str) -> bool {
        response.contains("```") || response.contains("fn ") || response.contains("function ")
    }

    /// Check if response contains markdown formatting
    pub fn is_markdown_formatted(response: &str) -> bool {
        response.contains("#") || response.contains("*") || response.contains("`")
    }
}

/// Build OpenAI-compatible chat completion request
pub fn build_chat_completion_request(message: &str, model: Option<&str>) -> Value {
    json!({
        "model": model.unwrap_or("test-model"),
        "messages": [
            {
                "role": "user",
                "content": message
            }
        ],
        "temperature": 0.7,
        "max_tokens": 1000,
        "stream": false
    })
}

/// Build a session creation request
pub fn build_session_create_request(title: &str) -> Value {
    json!({
        "title": title,
        "metadata": {
            "source": "e2e-test",
            "created_at": chrono::Utc::now().to_rfc3339()
        }
    })
}

/// Sample responses from mock provider (for SDK testing)
pub struct MockResponses;

impl MockResponses {
    /// Simple text response
    pub fn simple_text() -> Value {
        json!({
            "id": "chatcmpl-test-001",
            "object": "chat.completion",
            "created": 1700000000,
            "model": "test-model",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello! I'm CarpAI, your AI programming assistant. How can I help you today?"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 20,
                "total_tokens": 30
            }
        })
    }

    /// Code generation response
    pub fn code_response() -> Value {
        json!({
            "id": "chatcmpl-test-002",
            "object": "chat.completion",
            "created": 1700000001,
            "model": "test-model",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Here's a Rust function that sorts a vector of integers:\n\n```rust\nfn sort_vec(mut numbers: Vec<i32>) -> Vec<i32> {\n    numbers.sort();\n    numbers\n}\n```\n\nThis function takes ownership of the input vector, sorts it in place using the built-in sort method, and returns the sorted vector."
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 15,
                "completion_tokens": 50,
                "total_tokens": 65
            }
        })
    }

    /// Streaming chunk response
    pub fn streaming_chunk(text: &str, finish_reason: Option<&str>) -> Value {
        json!({
            "id": "chunk-test-001",
            "object": "chat.completion.chunk",
            "created": 1700000002,
            "model": "test-model",
            "choices": [{
                "index": 0,
                "delta": {
                    "content": text
                },
                "finish_reason": finish_reason
            }]
        })
    }

    /// Health check response
    pub fn health_check() -> Value {
        json!({
            "status": "healthy",
            "version": "1.0.0-test",
            "uptime_seconds": 60,
            "components": {
                "grpc": "ready",
                "rest": "ready",
                "storage": "ready"
            }
        })
    }

    /// Session list response
    pub fn session_list(sessions: &[Value]) -> Value {
        json!({
            "sessions": sessions,
            "total": sessions.len(),
            "page": 1,
            "page_size": 20
        })
    }

    /// Single session detail response
    pub fn session_detail(id: &str, title: &str, message_count: usize) -> Value {
        json!({
            "id": id,
            "title": title,
            "message_count": message_count,
            "created_at": chrono::Utc::now().to_rfc3339(),
            "updated_at": chrono::Utc::now().to_rfc3339(),
            "metadata": {
                "source": "e2e-test"
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_greeting_is_not_empty() {
        assert!(!SIMPLE_GREETING.is_empty());
    }

    #[test]
    fn test_build_chat_completion_request_structure() {
        let request = build_chat_completion_request(Some("Hello"), Some("gpt-4"));
        assert_eq!(request["model"], "gpt-4");
        assert_eq!(request["messages"][0]["role"], "user");
        assert_eq!(request["messages"][0]["content"], "Hello");
    }

    #[test]
    fn test_mock_responses_have_valid_structure() {
        let response = MockResponses::simple_text();
        assert!(response.get("id").is_some());
        assert!(response.get("choices").is_some());
        assert_eq!(response["choices"][0]["message"]["role"], "assistant");
    }

    #[test]
    fn test_response_patterns_detects_content() {
        assert!(ResponsePatterns::has_content("This is valid content"));
        assert!(!ResponsePatterns::has_content(""));
        assert!(!ResponsePatterns::has_content("   "));
    }

    #[test]
    fn test_response_patterns_detects_code_blocks() {
        assert!(ResponsePatterns::contains_code_block("```rust\ncode\n```"));
        assert!(ResponsePatterns::contains_code_block("fn main() {}"));
        assert!(!ResponsePatterns::contains_code_block("just text"));
    }
}
