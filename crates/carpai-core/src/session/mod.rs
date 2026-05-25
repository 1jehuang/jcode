//! Session System - Business Logic Layer (Layer 1)
//!
//! This module contains all session-related business logic implementations:
//! - Session CRUD operations
//! - Session export/import
//! - Cost tracking
//! - Garbage collection
//! - Runtime management

// --- Core Session Types ---
pub mod core_types;

// --- Session Components ---
pub mod export;
pub mod gc;
pub mod runtime_manager;

// NOTE: Enhanced cost tracker exists as scaffolding but is NOT yet integrated:
// - cost_tracker: Detailed cost tracking with per-API-call records [2025-05-25]
//   Status: Has detailed CostRecord struct, extends base SessionCostTracker
//   References: Only uses session::core_types::SessionCostTracker
//
// DEAD CODE ANALYSIS RESULT (2025-05-25):
// ✅ Confirmed: cost_tracker has ZERO references in carpai-core or parent crates
// ⚠️ Decision: Keep as scaffolding until cost tracking requirements are finalized
// 📅 Next action: Integrate when enhanced cost reporting is needed

// Re-export key types
pub use core_types::{
    SessionExport, SessionImport, SessionCostTracker, ImportResult,
    GcConfig, GcResult, CostSummary, RuntimeState,
};

// Re-export components
pub use export::SessionExporter;
pub use gc::SessionGc;
pub use runtime_manager::{SessionRuntimeManager, ActiveSession, SessionStats};
