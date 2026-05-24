//! CarpAI Code Completion System — FIM-enhanced multi-candidate completion with auto fallback
//!
//! This module provides the complete code completion pipeline for CarpAI:
//!
//! - **engine** — `CompletionEngine` with multi-provider abstraction (local Ollama, cloud APIs)
//! - **quality** — FIM format optimization, context building, multi-candidate ranking, acceptance tracking
//! - **fallback** — Auto fallback router (Local → Cloud inference switching with health checks)
//!
//! ## Architecture
//!
//! ```text
//! CompletionRequest
//!     │
//!     ▼
//! ┌──────────────┐    ┌──────────────────────┐
//! │  Engine      │───▶│  Quality Pipeline    │
//! │  (provider   │    │  (FIM + Context +    │
//! │   selection) │    │   Ranking + Tracker) │
//! └──────────────┘    └──────────────────────┘
//!       │                      │
//!       ▼                      ▼
//! ┌──────────────┐    ┌──────────────────────┐
//! │  Fallback    │    │  SmartCompleter      │
//! │  Router      │    │  (adaptive params)   │
//! └──────────────┘    └──────────────────────┘
//! ```

pub mod engine;
pub mod quality;
pub mod fallback;

// ========================================================================
// Re-exports from carpai-internal (CodeCompletion trait & types)
// ========================================================================

pub use carpai_internal::completion::{
    CodeCompletion,
    CompletionRequest,
    CompletionCandidate,
    CompletionKind,
    CompletionError,
};

// ========================================================================
// Re-exports from sub-modules
// ========================================================================

// --- Engine ---
pub use engine::{
    CompletionEngine,
    CompletionProvider,
    CompletionOutput,
    LocalCompletionProvider,
};

// --- Quality ---
pub use quality::{
    FimCompletionRequest,
    FimCompletionResponse,
    FimCandidate,
    FimCompleter,
    CompletionContext,
    ContextBuilder,
    CompletionFeedback,
    ModelStats,
    AcceptanceTracker,
    SmartCompleter,
    completion_loop_stats,
};

// --- Fallback ---
pub use fallback::{
    InferenceTarget,
    FallbackStatus,
    AutoFallbackRouter,
};
