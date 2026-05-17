//! Layer 2: Retrieval Layer - Multi-Engine Fusion Search

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use anyhow::Result;
use chrono::{DateTime, Utc};
use tracing::{debug, error, info, warn};

use crate::{
    PhaseResult, PhaseName, PhaseOutput, SurgicalRequest, Language,
    ContextWindow, ContextSegment, SourceBreakdown, RetrievalSource,
    SymbolMatch, PatternMatch, SimilarCodeMatch,
    RetrievalLayer,
    indexing_layer::GlobalSymbolIndexer,
};

/// Retrieval configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalConfig {
    pub max_context_window_tokens: usize,
    pub default_top_k: usize,
    pub enable_cache: bool,
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            max_context_window_tokens: 64000, // 64K tokens for modern LLMs
            default_top_k: 10,
            enable_cache: true,
        }
    }
}

/// String search provider trait
#[async_trait::async_trait]
pub trait StringSearchProvider: Send + Sync {
    async fn search(&self, pattern: &str) -> Result<Vec<RawSearchResult>>;
}

/// Raw search result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawSearchResult {
    pub file_path: PathBuf,
    pub matched_content: String,
    pub start_line: usize,
    pub end_line: usize,
    pub score: f64,
}

/// Multi-engine retriever
pub struct MultiEngineRetriever {
    config: RetrievalConfig,
    indexer: Arc<GlobalSymbolIndexer>,
    string_searcher: Arc<dyn StringSearchProvider>,
}

impl MultiEngineRetriever {
    pub fn new(
        config: RetrievalConfig,
        indexer: Arc<GlobalSymbolIndexer>,
        string_searcher: Arc<dyn StringSearchProvider>,
    ) -> Self {
        Self {
            config,
            indexer,
            string_searcher,
        }
    }

    /// Core retrieval method - multi-engine fusion search
    pub async fn retrieve(&self, query: &str, _language: Language) -> Result<Vec<FusedSearchResult>> {
        let start_time = std::time::Instant::now();

        info!(query = %query[..query.len().min(50)], "Starting multi-engine retrieval");

        // Run string search (simplified)
        let string_results = self.string_searcher.search(query).await.unwrap_or_default();

        // Run symbol search (simplified)
        let symbol_results = self.indexer.fuzzy_search_symbols(query, 10).await;

        // Fuse and rank results (simplified scoring)
        let mut fused_results = Vec::new();

        for (i, result) in string_results.iter().enumerate() {
            fused_results.push(FusedSearchResult {
                final_score: result.score * 0.6,
                content: ContextSegment {
                    id: format!("seg_{}", i),
                    file_path: result.file_path.clone(),
                    content: result.matched_content.clone(),
                    start_line: result.start_line,
                    end_line: result.end_line,
                    language: "unknown".to_string(),
                    relevance_score: result.score,
                    source: RetrievalSource::StringMatch,
                },
                rank: i + 1,
            });
        }

        for (i, symbol) in symbol_results.iter().enumerate() {
            fused_results.push(FusedSearchResult {
                final_score: 0.8,
                content: ContextSegment {
                    id: format!("sym_{}", i),
                    file_path: symbol.file_path.clone(),
                    content: format!("{}: {}", symbol.kind, symbol.name),
                    start_line: symbol.definition_line.saturating_sub(3),
                    end_line: symbol.definition_line + 3,
                    language: "unknown".to_string(),
                    relevance_score: 0.8,
                    source: RetrievalSource::SymbolReference,
                },
                rank: fused_results.len() + 1,
            });
        }

        // Sort by score
        fused_results.sort_by(|a, b| b.final_score.partial_cmp(&a.final_score).unwrap());

        // Limit to top-k
        if fused_results.len() > self.config.default_top_k {
            fused_results.truncate(self.config.default_top_k);
        }

        let duration_ms = start_time.elapsed().as_millis() as u64;

        info!(
            results = fused_results.len(),
            duration_ms = duration_ms,
            "Retrieval completed"
        );

        Ok(fused_results)
    }

    /// Build context window from results
    pub fn build_context_window(&self, results: &[FusedSearchResult], request: &SurgicalRequest) -> ContextWindow {
        let mut segments = Vec::new();
        let mut total_tokens = 0;
        let mut breakdown = SourceBreakdown {
            string_match_count: 0,
            symbol_reference_count: 0,
            semantic_similarity_count: 0,
            user_activity_count: 0,
            explicit_count: 0,
        };

        for result in results {
            // Check token limit
            let estimated_tokens = result.content.content.len() / 4; // Rough estimate
            
            if total_tokens + estimated_tokens > self.config.max_context_window_tokens {
                warn!(
                    current_tokens = total_tokens,
                    limit = self.config.max_context_window_tokens,
                    "Context window size limit reached"
                );
                break;
            }

            segments.push(result.content.clone());
            total_tokens += estimated_tokens;

            match result.content.source {
                RetrievalSource::StringMatch => breakdown.string_match_count += 1,
                RetrievalSource::SymbolReference => breakdown.symbol_reference_count += 1,
                RetrievalSource::SemanticSimilarity => breakdown.semantic_similarity_count += 1,
                RetrievalSource::UserActivity => breakdown.user_activity_count += 1,
                RetrievalSource::ExplicitInclusion => breakdown.explicit_count += 1,
            }
        }

        ContextWindow {
            id: format!("ctx_{}", request.request_id),
            segments,
            total_tokens,
            source_breakdown: breakdown,
        }
    }
}

/// Fused search result
#[derive(Debug, Clone)]
pub struct FusedSearchResult {
    pub final_score: f64,
    pub content: ContextSegment,
    pub rank: usize,
}

#[async_trait::async_trait]
impl RetrievalLayer for MultiEngineRetriever {
    async fn retrieve_relevant_context(
        &self,
        request: &SurgicalRequest,
        _indexing_output: &PhaseOutput,
    ) -> Result<PhaseResult> {
        let start_time = std::time::Instant::now();

        info!(
            request_id = %request.request_id,
            intent = %request.intent[..request.intent.len().min(80)],
            "Retrieving relevant context"
        );

        // Extract query from intent (simplified)
        let query = request.intent.clone();
        
        // Infer target language
        let language = Language::Rust; // Default to Rust

        // Execute multi-engine retrieval
        let search_results = self.retrieve(&query, language).await?;

        // Build context window
        let context_window = self.build_context_window(&search_results, request);
        
        // Check if context window is empty before using it
        let is_empty = context_window.segments.is_empty();

        // Extract relevance scores
        let relevance_scores: Vec<f64> = search_results.iter()
            .map(|r| r.final_score)
            .collect();

        let duration_ms = start_time.elapsed().as_millis() as u64;

        Ok(PhaseResult {
            phase: PhaseName::Retrieval,
            passed: !is_empty,
            duration_ms,
            output: PhaseOutput::RetrievalOutput {
                context_windows: vec![context_window],
                relevance_scores,
                retrieval_duration_ms: duration_ms,
            },
            warnings: if is_empty {
                vec!["No relevant code found".to_string()]
            } else {
                Vec::new()
            },
            errors: Vec::new(),
        })
    }

    async fn search_symbol(&self, name: &str, _language: Option<Language>) -> Result<Vec<SymbolMatch>> {
        let symbols = self.indexer.find_symbol_by_name(name).await;

        Ok(symbols.into_iter()
            .map(|s| SymbolMatch {
                symbol_name: s.name.clone(),
                kind: s.kind,
                file_path: s.file_path,
                line: s.definition_line,
                definition: None,
            })
            .collect())
    }

    async fn search_code_pattern(&self, pattern: &str, _language: Option<Language>) -> Result<Vec<PatternMatch>> {
        // Simplified implementation
        Ok(vec![PatternMatch {
            file_path: PathBuf::from(""),
            line: 0,
            matched_text: pattern.to_string(),
            context_before: String::new(),
            context_after: String::new(),
        }])
    }

    async fn find_similar_code(&self, code: &str, _language: Language, _top_k: usize) -> Result<Vec<SimilarCodeMatch>> {
        // Simplified implementation
        Ok(vec![SimilarCodeMatch {
            file_path: PathBuf::from(""),
            similarity: 0.5,
            snippet: code.chars().take(500).collect(),
        }])
    }
}
