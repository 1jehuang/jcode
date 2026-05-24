//! Refactoring Engine - Business Logic Layer (Layer 1)
//!
//! This module contains all refactoring-related business logic:
//! - Precise edit engine for block-level code editing
//! - Atomic edit coordinator for multi-file transactions
//! - Diff engine for change visualization
//! - Compilation and diagnostics integration
//! - Transaction management and rollback support
//! - Delivery pipeline for safe code deployment

// --- Core Engine ---
pub mod engine;

// --- Edit Operations ---
pub mod precise_edit;
pub mod atomic_edit;

// --- Diff & Preview ---
pub mod diff_engine;
pub mod diff_integration;
pub mod streaming_preview;

// --- Compilation & Validation ---
pub mod compilation;
pub mod verify_pipeline;

// --- Delivery ---
pub mod delivery_pipeline;

// Re-export key types
pub use engine::RefactorEngine;
pub use precise_edit::{EditOperation, EditResult, MatchStrategy, IndentStyle};
pub use atomic_edit::{AtomicEditCoordinator, TransactionStatus};
pub use compilation::{CompilationEngine, FixEngine};
pub use diff_engine::{DiffOp, DiffHunk, StructuredPatch, WordDiff};
