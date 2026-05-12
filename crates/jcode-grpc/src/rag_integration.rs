//! RAG Integration for LLM Service
//!
//! This module provides integration between the LLM gRPC service
//! and jcode-rag's editing layer for context-aware code editing.

use std::sync::Arc;
use std::time::Instant;

use tracing::{info, debug, warn, instrument};
use anyhow::{Result, Context};

use jcode_llm::{
    types::*,
    LlmProvider,
};
use jcode_rag::{
    EditingLayer,
    TextDiff,
    ApplyResult,
    PreviewResult,
};

/// RAG-enhanced chat context
#[derive(Debug, Clone)]
pub struct RagChatContext {
    /// Project path for codebase indexing
    pub project_path: String,
    
    /// Whether to enable RAG retrieval
    pub enable_rag: bool,
    
    /// Maximum number of retrieved contexts to include
    pub max_retrieved_contexts: usize,
}

impl Default for RagChatContext {
    fn default() -> Self {
        Self {
            project_path: String::new(),
            enable_rag: true,
            max_retrieved_contexts: 5,
        }
    }
}

/// RAG-integrated LLM service wrapper
pub struct RagLlmService {
    /// Base LLM provider
    provider: Arc<dyn LlmProvider>,
    
    /// Editing layer (optional)
    editing_layer: Option<Arc<dyn EditingLayer>>,
}

impl RagLlmService {
    /// Create new RAG LLM service
    pub fn new(provider: Arc<dyn LlmProvider>) -> Self {
        Self {
            provider,
            editing_layer: None,
        }
    }
    
    /// Set editing layer
    pub fn with_editing_layer(mut self, layer: Arc<dyn EditingLayer>) -> Self {
        self.editing_layer = Some(layer);
        self
    }
    
    /// RAG-enhanced chat completion (simplified version)
    #[instrument(skip(self, request), fields(model = %request.model))]
    pub async fn rag_chat_completion(
        &self,
        request: ChatCompletionRequest,
        context: &RagChatContext,
    ) -> Result<ChatCompletionResponse> {
        let start = Instant::now();
        
        info!(
            model = %request.model,
            messages = request.messages.len(),
            rag_enabled = context.enable_rag,
            "Executing RAG-enhanced chat completion"
        );
        
        // If RAG is enabled and project path is provided, augment context
        if context.enable_rag && !context.project_path.is_empty() {
            debug!(project = %context.project_path, "RAG enabled, checking for codebase context");
            
            // Analyze if query needs codebase context
            let user_message = request.messages.last()
                .map(|m| m.content.clone().unwrap_or_default())
                .unwrap_or_default();
            
            if self.analyze_query_for_codebase(&user_message) {
                debug!("Query requires codebase context, augmenting with project information");
                
                let augmented_request = self.augment_request_with_project_context(request, &context.project_path);
                
                match self.provider.chat_completion(augmented_request).await {
                    Ok(response) => {
                        let latency_ms = start.elapsed().as_millis() as f64;
                        info!(latency_ms = latency_ms, "RAG-augmented completion successful");
                        Ok(response)
                    }
                    Err(e) => {
                        warn!(error = %e, "RAG-augmented completion failed, returning error");
                        Err(e.into())
                    }
                }
            } else {
                debug!("Query does not require codebase context, using standard completion");
                let response = self.provider.chat_completion(request).await?;
                Ok(response)
            }
        } else {
            debug!("RAG disabled or no project path, using standard completion");
            let response = self.provider.chat_completion(request).await?;
            Ok(response)
        }
    }
    
    /// Apply code edits with safety checks
    #[instrument(skip(self), fields(diff_count = diffs.len()))]
    pub async fn apply_edits(&self, diffs: &[TextDiff]) -> Result<ApplyResult> {
        info!("Applying code edits through editing layer");
        
        let editing_layer = self.editing_layer.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Editing layer not configured"))?;
        
        let result = editing_layer.apply_edits(diffs).await
            .context("Failed to apply edits through editing layer")?;
        
        info!(
            success = result.success,
            applied = result.applied_count,
            failed = result.failed_items.len(),
            "Edits applied successfully"
        );
        
        Ok(result)
    }
    
    /// Preview a diff before applying
    #[instrument(skip(self))]
    pub async fn preview_diff(&self, diff: &TextDiff) -> Result<PreviewResult> {
        let editing_layer = self.editing_layer.as_ref()
            .ok_or_else(|| anyhow::anyhow!("Editing layer not configured"))?;
        
        editing_layer.preview_diff(diff).await
            .context("Failed to preview diff")
    }
    
    /// Analyze query to determine if it needs codebase context
    fn analyze_query_for_codebase(&self, query: &str) -> bool {
        let keywords = [
            "function", "class", "method", "variable", "import",
            "file", "module", "package", "dependency",
            "implement", "refactor", "modify", "change",
            "where is", "how does", "find", "locate",
            "definition", "usage", "reference",
            "bug", "error", "issue", "fix",
            "test", "spec", "assertion",
        ];
        
        let query_lower = query.to_lowercase();
        
        keywords.iter().any(|&keyword| query_lower.contains(keyword))
    }
    
    /// Augment request with basic project context
    fn augment_request_with_project_context(
        &self,
        mut request: ChatCompletionRequest,
        project_path: &str,
    ) -> ChatCompletionRequest {
        let system_context = format!(
            "## Project Context\n\n- Project Path: {}\n- Working in this codebase\n\nWhen answering questions about code, consider the project structure and existing implementations.\n",
            project_path
        );
        
        let context_message = ChatMessage {
            role: MessageRole::System,
            content: Some(system_context),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        };
        
        request.messages.insert(0, context_message);
        
        request
    }
}
