//! CarpAI Performance Layer — caching, concurrency control, circuit breaking, backpressure, token budget

pub mod cache_tracker;
pub mod cache_optimizer;
pub mod cache_integration;
pub mod cache_break_detector;
pub mod concurrency;
pub mod compression;
pub mod circuit_breaker;
pub mod backpressure;
pub mod token_budget;
pub mod denial_tracking;
pub mod perf;

// Re-export key public types from each module
pub use perf::{
    PerformanceTier,
    SystemProfile,
    SyntheticSystemProfile,
    TuiPerfPolicy,
    profile,
    synthetic_profile,
    tui_policy,
};

pub use cache_tracker::{CacheTracker, CacheViolation};
pub use cache_optimizer::{TokenCacheOptimizer, CacheOptimizerConfig, CacheStats, TokenCacheEntry};
pub use cache_break_detector::{CacheBreakDetector, CacheMetrics, CacheBreakEvent};
pub use concurrency::{ConcurrencyOptimizer, RequestPriority, RequestTask, ConcurrencyStats};
pub use compression::{ConversationCompressor, CompressionConfig, CompressionStrategy, CompressionStats, Message as CompressionMessage};
pub use circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, CircuitBreakerMetrics, CircuitBreakerManager, CircuitState};
pub use backpressure::{BackpressureController, BackpressureConfig, BackpressureGuard, OverloadedError, BackpressureMetrics};
pub use token_budget::{TokenBudgetTracker, BudgetDecision, CompletionEvent, DEFAULT_BUDGET_TOKENS, MAX_AUTO_CONTINUATIONS};
pub use denial_tracking::{DenialTrackingState, DenialSnapshot, DecisionMetadata, DecisionSource, PermissionCheckResult};
