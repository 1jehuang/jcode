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

// Re-export key types
pub use core_types::{
    SessionExport, SessionImport, SessionCostTracker, ImportResult,
    GcConfig, GcResult, CostSummary, RuntimeState,
};

// Re-export components
pub use export::SessionExporter;
pub use gc::SessionGc;
pub use runtime_manager::{SessionRuntimeManager, ActiveSession, SessionStats};
