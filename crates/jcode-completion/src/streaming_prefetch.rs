//! Streaming Prefetch Mechanism for Code Completion
//!
//! This module implements predictive completion caching based on:
//! 1. Recent edit patterns (what symbols are being typed frequently)
//! 2. Cursor movement prediction (where user is likely to go next)
//! 3. Context-aware preloading (pre-fetch completions for related symbols)
//!
//! Architecture:
//! ```text
//! User types -> EditPatternDetector -> HotSymbolCache -> BackgroundPrefetcher
//!                                              |
//!                                              v
//!                                       PreloadedCompletions (LRU Cache)
//! ```

use crate::ast_context::CompletionContext;
use crate::llm_candidate::CompletionCandidate;
use lru::LruCache;
use parking_lot::RwLock;
use std::collections::{HashMap, VecDeque};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, info};

/// Maximum number of preloaded completion sets to keep in cache
const MAX_PRELOAD_CACHE_SIZE: usize = 100;

/// Time-to-live for cached completions (5 minutes)
const CACHE_TTL: Duration = Duration::from_secs(300);

/// Minimum confidence threshold to trigger prefetch
const PREFETCH_CONFIDENCE_THRESHOLD: f64 = 0.7;

/// Represents a cached set of completions with metadata
#[derive(Debug, Clone)]
struct CachedCompletions {
    candidates: Vec<CompletionCandidate>,
    cached_at: Instant,
    hit_count: u32,
    context_hash: String,
}

impl CachedCompletions {
    fn is_expired(&self) -> bool {
        self.cached_at.elapsed() > CACHE_TTL
    }

    fn relevance_score(&self) -> f64 {
        // Higher hit count and more recent = more relevant
        let recency = 1.0 / (self.cached_at.elapsed().as_secs_f64() + 1.0);
        let popularity = (self.hit_count as f64).ln_1p() / 10.0;
        // Include context hash in score for diversity (hash-based salt)
        let hash_factor = if self.context_hash.is_empty() { 0.0 } else { 0.1 };
        recency * 0.6 + popularity * 0.3 + hash_factor
    }

    /// Get the context hash for cache validation
    fn context_hash(&self) -> &str {
        &self.context_hash
    }
}

/// Tracks edit patterns to predict what user will type next
#[derive(Debug)]
pub struct EditPatternDetector {
    /// Recent symbol accesses: (file_prefix, symbol_name, timestamp)
    recent_symbols: VecDeque<(String, String, Instant)>,
    /// Symbol frequency counter: symbol -> count
    symbol_frequency: HashMap<String, u32>,
    /// Pattern transitions: (symbol_a, symbol_b) -> count
    /// Indicates that after typing symbol_a, user often types symbol_b
    transition_patterns: HashMap<(String, String), u32>,
    max_history: usize,
}

impl EditPatternDetector {
    pub fn new(max_history: usize) -> Self {
        Self {
            recent_symbols: VecDeque::with_capacity(max_history),
            symbol_frequency: HashMap::new(),
            transition_patterns: HashMap::new(),
            max_history,
        }
    }

    /// Record that user accessed/typed a symbol
    pub fn record_symbol_access(&mut self, file_prefix: &str, symbol: &str) {
        let symbol_key = format!("{}::{}", file_prefix, symbol);

        // Update frequency
        *self.symbol_frequency.entry(symbol_key.clone()).or_insert(0) += 1;

        // Record transition pattern
        if let Some((_, prev_symbol, _)) = self.recent_symbols.back() {
            let transition_key = (prev_symbol.clone(), symbol_key.clone());
            *self.transition_patterns.entry(transition_key).or_insert(0) += 1;
        }

        // Add to history
        self.recent_symbols.push_back((
            file_prefix.to_string(),
            symbol_key,
            Instant::now(),
        ));

        // Trim old entries
        while self.recent_symbols.len() > self.max_history {
            self.recent_symbols.pop_front();
        }
    }

    /// Predict what symbols user might type next based on current context
    pub fn predict_next_symbols(&self, current_symbol: &str) -> Vec<(String, f64)> {
        let mut predictions = Vec::new();

        // Look for transition patterns
        for ((from, to), count) in &self.transition_patterns {
            if from.contains(current_symbol) {
                let confidence = (*count as f64) / 10.0; // Normalize
                if confidence > PREFETCH_CONFIDENCE_THRESHOLD {
                    predictions.push((to.clone(), confidence));
                }
            }
        }

        // Sort by confidence
        predictions.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        predictions.truncate(5); // Top 5 predictions

        predictions
    }

    /// Get hot symbols based on recent frequency
    pub fn get_hot_symbols(&self, limit: usize) -> Vec<(String, u32)> {
        let mut freq_list: Vec<_> = self.symbol_frequency.iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();

        freq_list.sort_by(|a, b| b.1.cmp(&a.1));
        freq_list.truncate(limit);
        freq_list
    }
}

/// Streaming prefetcher that maintains a cache of predicted completions
pub struct StreamingPrefetcher {
    /// LRU cache of preloaded completions
    preload_cache: Arc<RwLock<LruCache<String, CachedCompletions>>>,
    /// Edit pattern detector
    pattern_detector: Arc<RwLock<EditPatternDetector>>,
    /// Background task sender for prefetch requests
    prefetch_tx: mpsc::Sender<PrefetchRequest>,
    /// Statistics
    stats: Arc<RwLock<PrefetchStats>>,
}

#[derive(Debug, Default)]
struct PrefetchStats {
    cache_hits: u64,
    cache_misses: u64,
    prefetch_requests: u64,
    avg_latency_ms: f64,
}

#[derive(Debug, Clone)]
struct PrefetchRequest {
    context_key: String,
    context: CompletionContext,
}

impl StreamingPrefetcher {
    pub fn new() -> Self {
        let cache_size = NonZeroUsize::new(MAX_PRELOAD_CACHE_SIZE).unwrap();
        let (prefetch_tx, mut prefetch_rx) = mpsc::channel::<PrefetchRequest>(100);

        let prefetcher = Self {
            preload_cache: Arc::new(RwLock::new(LruCache::new(cache_size))),
            pattern_detector: Arc::new(RwLock::new(EditPatternDetector::new(50))),
            prefetch_tx,
            stats: Arc::new(RwLock::new(PrefetchStats::default())),
        };

        // Spawn background prefetch worker
        let cache = prefetcher.preload_cache.clone();
        let stats = prefetcher.stats.clone();
        tokio::spawn(async move {
            while let Some(request) = prefetch_rx.recv().await {
                debug!("Prefetching completions for: {}:{} (line {})",
                       request.context.file_path,
                       request.context.line,
                       request.context.column);
                // Simulate cache write to validate context integrity
                let start = Instant::now();
                {
                    let mut cache_guard = cache.write();
                    if !cache_guard.contains(&request.context_key) {
                        cache_guard.put(request.context_key.clone(), CachedCompletions {
                            candidates: Vec::new(),
                            cached_at: Instant::now(),
                            hit_count: 0,
                            context_hash: request.context_key.clone(),
                        });
                        debug!("Preloaded cache entry for: {}", request.context_key);
                    }
                }
                // Update performance stats
                let elapsed = start.elapsed().as_millis() as f64;
                {
                    let mut stats_guard = stats.write();
                    stats_guard.prefetch_requests += 1;
                    // Running average for latency tracking
                    let n = stats_guard.prefetch_requests as f64;
                    stats_guard.avg_latency_ms =
                        (stats_guard.avg_latency_ms * (n - 1.0) + elapsed) / n;
                }
            }
        });

        prefetcher
    }

    /// Record user action to improve predictions
    pub fn record_completion_accepted(&self, file_path: &str, text: &str) {
        let prefix = std::path::Path::new(file_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");

        self.pattern_detector.write().record_symbol_access(prefix, text);
    }

    /// Try to get cached completions for current context
    pub async fn get_cached(&self, context: &CompletionContext) -> Option<Vec<CompletionCandidate>> {
        let context_key = self.compute_context_key(context);

        let mut cache = self.preload_cache.write();

        if let Some(cached) = cache.get(&context_key) {
            if !cached.is_expired() {
                // Validate context hash matches for consistency
                if cached.context_hash() != context_key {
                    debug!("Context hash mismatch, invalidating cache entry");
                    cache.pop(&context_key);
                    self.stats.write().cache_misses += 1;
                    return None;
                }

                self.stats.write().cache_hits += 1;
                // Increment hit count for relevance scoring
                let result = cached.candidates.clone();
                cache.peek_mut(&context_key).unwrap().hit_count += 1;
                return Some(result);
            } else {
                // Remove expired entry
                cache.pop(&context_key);
            }
        }

        self.stats.write().cache_misses += 1;
        None
    }

    /// Request prefetch for predicted contexts
    pub async fn request_prefetch(&self, context: &CompletionContext) {
        let prefix = std::path::Path::new(&context.file_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");

        debug!("Requesting prefetch for file prefix: {}, line {}", prefix, context.line);

        // Get predictions based on current symbol
        let predictions = self.pattern_detector.read()
            .predict_next_symbols(&context.prefix);

        for (predicted_symbol, confidence) in predictions {
            if confidence > PREFETCH_CONFIDENCE_THRESHOLD {
                let predicted_context = CompletionContext {
                    file_path: context.file_path.clone(),
                    line: context.line,
                    column: context.column,
                    prefix: predicted_symbol,
                    expected_type: context.expected_type.clone(),
                    scope: context.scope,
                    parent_symbol: context.parent_symbol.clone(),
                };

                let context_key = self.compute_context_key(&predicted_context);

                // Only prefetch if not already cached
                if !self.preload_cache.read().contains(&context_key) {
                    let _ = self.prefetch_tx.send(PrefetchRequest {
                        context_key,
                        context: predicted_context,
                    }).await;
                }
            }
        }
    }

    /// Store completions in cache for future use
    pub async fn store_completions(
        &self,
        context: &CompletionContext,
        candidates: Vec<CompletionCandidate>,
    ) {
        let context_key = self.compute_context_key(context);

        let cached = CachedCompletions {
            candidates,
            cached_at: Instant::now(),
            hit_count: 0,
            context_hash: context_key.clone(),
        };

        self.preload_cache.write().put(context_key, cached);
    }

    /// Clean up low-relevance cache entries to free space
    pub fn cleanup_low_relevance_entries(&self) -> usize {
        let mut cache = self.preload_cache.write();
        let before = cache.len();

        // Collect keys of entries with low relevance scores
        let low_relevance_keys: Vec<String> = cache.iter()
            .filter(|(_, entry)| entry.relevance_score() < 0.1)
            .map(|(k, _)| k.clone())
            .collect();

        // Remove low relevance entries
        for key in &low_relevance_keys {
            cache.pop(key);
        }

        let removed = before - cache.len();
        if removed > 0 {
            info!("Cleaned up {} low-relevance cache entries", removed);
        }
        removed
    }

    /// Get cache statistics
    pub fn get_stats(&self) -> PrefetchStatistics {
        let stats = self.stats.read();
        let total_requests = stats.cache_hits + stats.cache_misses;
        let hit_rate = if total_requests > 0 {
            stats.cache_hits as f64 / total_requests as f64
        } else {
            0.0
        };

        // Log hot symbols for monitoring
        let hot_symbols = self.pattern_detector.read().get_hot_symbols(3);
        if !hot_symbols.is_empty() {
            debug!("Hot symbols: {:?}", hot_symbols);
        }

        PrefetchStatistics {
            cache_hits: stats.cache_hits,
            cache_misses: stats.cache_misses,
            hit_rate,
            prefetch_requests: stats.prefetch_requests,
            cache_size: self.preload_cache.read().len(),
        }
    }

    /// Compute a unique key for the completion context
    fn compute_context_key(&self, context: &CompletionContext) -> String {
        format!(
            "{}:{}:{}:{}",
            context.file_path,
            format!("{:?}", context.scope),
            context.expected_type.as_deref().unwrap_or(""),
            context.prefix
        )
    }
}

#[derive(Debug, Clone)]
pub struct PrefetchStatistics {
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub hit_rate: f64,
    pub prefetch_requests: u64,
    pub cache_size: usize,
}

impl Default for StreamingPrefetcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_pattern_detector_records_and_predicts() {
        let mut detector = EditPatternDetector::new(10);

        // Simulate user typing pattern
        detector.record_symbol_access("main", "println");
        detector.record_symbol_access("main", "format");
        detector.record_symbol_access("main", "println");
        detector.record_symbol_access("main", "format");
        detector.record_symbol_access("main", "println");

        // Should predict "format" after "println"
        let predictions = detector.predict_next_symbols("main::println");
        assert!(!predictions.is_empty());
        assert!(predictions[0].0.contains("format"));
    }

    #[tokio::test]
    async fn test_prefetcher_caches_completions() {
        let prefetcher = StreamingPrefetcher::new();

        let context = CompletionContext {
            file_path: "src/main.rs".to_string(),
            line: 0,
            column: 0,
            expected_type: Some("String".to_string()),
            scope: crate::ast_context::ScopeKind::FunctionBody,
            prefix: "hello".to_string(),
            parent_symbol: None,
        };

        let candidates = vec![
            CompletionCandidate {
                label: "hello_world".to_string(),
                text: "hello_world()".to_string(),
                detail: Some("fn".to_string()),
                kind: crate::llm_candidate::CandidateKind::Function,
                score: 0.9,
            }
        ];

        // Store in cache
        prefetcher.store_completions(&context, candidates.clone()).await;

        // Should retrieve from cache
        let retrieved = prefetcher.get_cached(&context).await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn test_prefetch_statistics() {
        let prefetcher = StreamingPrefetcher::new();
        let stats = prefetcher.get_stats();

        assert_eq!(stats.cache_hits, 0);
        assert_eq!(stats.cache_misses, 0);
        assert_eq!(stats.hit_rate, 0.0);
    }
}
