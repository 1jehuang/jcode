//! Code Completion Trait - Unified interface for all completion providers
//!
//! This trait abstracts away the differences between:
//! - Inline completions (TUI/IDE)
//! - Chat completions (Agent conversations)
//! - Multi-file edits (Cross-file repair)

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Main completion trait - implemented by all completion engines
#[async_trait]
pub trait CodeCompletion: Send + Sync {
    /// Generate code completions at cursor position
    ///
    /// # Arguments
    /// * `request` - Completion request with context
    ///
    /// # Returns
    /// Ranked list of completion candidates
    async fn complete(&self, request: CompletionRequest) -> Result<Vec<CompletionCandidate>, CompletionError>;

    /// Prefetch completions asynchronously (non-blocking)
    /// Results will be cached for fast retrieval
    async fn prefetch(&self, request: CompletionRequest) -> Result<(), CompletionError>;

    /// Get cached completions (if available)
    fn get_cached(&self, cache_key: &str) -> Option<Vec<CompletionCandidate>>;

    /// Record user acceptance/rejection for learning
    fn record_feedback(&self, candidate_id: &str, accepted: bool);

    /// Check if completion engine is ready
    fn is_ready(&self) -> bool;
}

/// Completion request context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionRequest {
    /// File path being edited
    pub file_path: String,

    /// Full file content
    pub content: String,

    /// Cursor line (0-indexed)
    pub cursor_line: usize,

    /// Cursor column (0-indexed)
    pub cursor_column: usize,

    /// Optional: language identifier
    pub language: Option<String>,

    /// Optional: trigger character (e.g., '.', '(')
    pub trigger_char: Option<char>,

    /// Optional: max number of candidates to return
    pub max_candidates: Option<usize>,

    /// Optional: timeout in milliseconds
    pub timeout_ms: Option<u64>,
}

/// Single completion candidate
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionCandidate {
    /// Unique identifier for this candidate
    pub id: String,

    /// Suggested text to insert
    pub text: String,

    /// Confidence score (0.0 - 1.0)
    pub score: f32,

    /// Completion type
    pub kind: CompletionKind,

    /// Optional: display label for UI
    pub label: Option<String>,

    /// Optional: detailed documentation
    pub documentation: Option<String>,

    /// Optional: range to replace (start_line, start_col, end_line, end_col)
    pub replace_range: Option<(usize, usize, usize, usize)>,
}

/// Type of completion
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompletionKind {
    /// Text completion (continue current line)
    Text,

    /// Function/method call
    Function,

    /// Variable/identifier
    Variable,

    /// Import statement
    Import,

    /// Code snippet/template
    Snippet,

    /// Documentation comment
    Documentation,
}

/// Completion error types
#[derive(Debug, thiserror::Error)]
pub enum CompletionError {
    #[error("Timeout exceeded: {0}")]
    Timeout(String),

    #[error("Provider error: {0}")]
    ProviderError(String),

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Cache miss: {0}")]
    CacheMiss(String),

    #[error("Engine not ready: {0}")]
    NotReady(String),

    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

/// Adapter to convert concrete implementation to trait object
pub struct CompletionAdapter<E: CodeCompletion> {
    engine: Arc<E>,
}

impl<E: CodeCompletion + 'static> CompletionAdapter<E> {
    pub fn new(engine: E) -> Self {
        Self {
            engine: Arc::new(engine),
        }
    }

    pub fn as_trait_object(&self) -> Arc<dyn CodeCompletion> {
        self.engine.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mock implementation for testing
    struct MockCompletionEngine;

    #[async_trait]
    impl CodeCompletion for MockCompletionEngine {
        async fn complete(&self, _req: CompletionRequest) -> Result<Vec<CompletionCandidate>, CompletionError> {
            Ok(vec![CompletionCandidate {
                id: "mock-1".to_string(),
                text: "println!(\"Hello, world!\");".to_string(),
                score: 0.95,
                kind: CompletionKind::Function,
                label: Some("println!".to_string()),
                documentation: None,
                replace_range: None,
            }])
        }

        async fn prefetch(&self, _req: CompletionRequest) -> Result<(), CompletionError> {
            Ok(())
        }

        fn get_cached(&self, _key: &str) -> Option<Vec<CompletionCandidate>> {
            None
        }

        fn record_feedback(&self, _id: &str, _accepted: bool) {}

        fn is_ready(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn test_mock_completion() {
        let engine = MockCompletionEngine;
        let request = CompletionRequest {
            file_path: "test.rs".to_string(),
            content: "fn main() {\n    ".to_string(),
            cursor_line: 1,
            cursor_column: 4,
            language: Some("rust".to_string()),
            trigger_char: None,
            max_candidates: Some(3),
            timeout_ms: Some(1000),
        };

        let result = engine.complete(request).await;
        assert!(result.is_ok());
        let candidates = result.unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].score, 0.95);
    }
}
