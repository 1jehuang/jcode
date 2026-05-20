//! TUI Completion Prefetch Helper
//!
//! This module provides integration between the TUI editor and the jcode-completion engine,
//! enabling background prefetching when cursor moves or user types.

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::debug;

/// Completion prefetch state for the TUI
pub struct CompletionPrefetchState {
    /// Last prefetched position (file, line, column)
    last_prefetch: Arc<RwLock<Option<(String, usize, usize)>>>,
    /// Debounce interval in milliseconds (avoid too frequent prefetches)
    debounce_ms: u64,
    /// Last prefetch timestamp
    last_prefetch_time: Arc<RwLock<Option<std::time::Instant>>>,
}

impl CompletionPrefetchState {
    pub fn new(debounce_ms: u64) -> Self {
        Self {
            last_prefetch: Arc::new(RwLock::new(None)),
            debounce_ms,
            last_prefetch_time: Arc::new(RwLock::new(None)),
        }
    }

    /// Check if we should trigger a prefetch (debounce logic)
    pub async fn should_prefetch(&self, file: &str, line: usize, column: usize) -> bool {
        let last = self.last_prefetch.read().await;
        let last_time = self.last_prefetch_time.read().await;

        // Always prefetch if never done before
        if last.is_none() {
            return true;
        }

        let (last_file, last_line, last_col) = last.as_ref().unwrap();

        // Prefetch if position changed significantly (>3 lines or >10 columns)
        let line_diff = if line > *last_line { line - last_line } else { last_line - line };
        let col_diff = if column > *last_col { column - last_col } else { last_col - column };

        if line_diff > 3 || col_diff > 10 || file != last_file {
            // Check debounce
            if let Some(last_instant) = last_time.as_ref() {
                let elapsed = last_instant.elapsed().as_millis();
                if elapsed < self.debounce_ms as u128 {
                    return false; // Too soon, skip this prefetch
                }
            }
            return true;
        }

        false // Position hasn't changed enough
    }

    /// Record that a prefetch was triggered
    pub async fn record_prefetch(&self, file: String, line: usize, column: usize) {
        let mut last = self.last_prefetch.write().await;
        *last = Some((file, line, column));

        let mut last_time = self.last_prefetch_time.write().await;
        *last_time = Some(std::time::Instant::now());
    }

    /// Trigger background prefetch (non-blocking)
    pub async fn trigger_prefetch(
        &self,
        engine: Arc<jcode_completion::CompletionEngine>,
        file: String,
        content: String,
        line: usize,
        column: usize,
    ) {
        if !self.should_prefetch(&file, line, column).await {
            return;
        }

        self.record_prefetch(file.clone(), line, column).await;

        // Spawn background task (don't block UI)
        tokio::spawn(async move {
            debug!("Triggering completion prefetch at {}:{}:{}", file, line, column);
            let start = std::time::Instant::now();

            // Call completion engine (this will populate prefetch cache)
            let _completions = engine.complete(&file, &content, line, column).await;

            let elapsed = start.elapsed();
            debug!("Prefetch completed in {:?}", elapsed);

            // Log performance metrics
            let stats = engine.get_prefetch_stats();
            debug!(
                "Prefetch stats: hit_rate={:.1}%, cache_size={}",
                stats.hit_rate * 100.0,
                stats.cache_size
            );
        });
    }
}

impl Default for CompletionPrefetchState {
    fn default() -> Self {
        Self::new(200) // 200ms debounce by default
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_debounce_logic() {
        let state = CompletionPrefetchState::new(100);

        // First prefetch should always trigger
        assert!(state.should_prefetch("file.rs", 10, 5).await);

        // Immediate second prefetch at same position should be debounced
        state.record_prefetch("file.rs".to_string(), 10, 5).await;
        assert!(!state.should_prefetch("file.rs", 10, 5).await);

        // Different file should trigger
        assert!(state.should_prefetch("other.rs", 10, 5).await);

        // Large position change should trigger
        assert!(state.should_prefetch("file.rs", 20, 5).await);
    }
}
