//! Code Analysis & AST - Business Logic Layer (Layer 1)
//!
//! This module contains code analysis and AST-related functionality:
//! - AST parsing and manipulation
//! - Code classification
//! - Context pruning for efficient prompting
//! - Incremental indexing
//! - Proactive context gathering

// --- AST & Classification ---
pub mod classifier;

// --- Context Management ---
pub mod context_pruner;
pub mod proactive_context;

// --- Indexing ---
pub mod incremental_index;

// Re-export key types
pub use classifier::{LlmClassifier as CodeClassifier, ClassificationResult, ClassificationRequest, ClassificationResponse};
pub use context_pruner::ContextPruner;
pub use proactive_context::{ProactiveContextService as ProactiveContextGatherer, ProactiveContextPredictor};
pub use incremental_index::{IncrementalIndexer as IncrementalIndex, GlobalIndexer, IncrementalIndexConfig};
