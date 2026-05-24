//! Memory System - Business Logic Layer (Layer 1)
//!
//! This module contains all memory-related business logic implementations:
//! - Core memory types and storage
//! - Enhanced memory with vector search
//! - Knowledge graph integration
//! - Semantic memory with embeddings
//! - Protocol adapters for external memory systems

// --- Core Memory Types ---
pub mod core_types;

// --- Memory Components ---
pub mod agent;
pub mod graph;
pub mod log;
pub mod prompt;
pub mod advanced;
pub mod semantic;
pub mod compaction;
pub mod protocol;

// NOTE: The following modules exist as scaffolding but are NOT yet integrated:
// - hierarchical:  Multi-level memory organization (orphaned)
// - knowledge_graph: Extended knowledge graph (orphaned)
// - knowledge:      Centralized knowledge base (orphaned)
// - knowledge_agents: Knowledge agent automation (orphaned)
// - types:          Additional memory type definitions (orphaned)
// These will be declared here and unified in Phase 1C.

// Re-export key types
pub use core_types::{
    MemoryEntry, MemoryQuery, MemoryType, MemoryScope, TrustLevel,
    EnhancedMemoryEntry, EnhancedMemoryQuery, VectorSearchResult,
};

// Re-export components
pub use agent::MemoryAgent;
pub use graph::KnowledgeGraph;
pub use log::MemoryLog;
pub use prompt::MemoryPromptBuilder;
pub use advanced::AdvancedMemoryOps;
pub use semantic::SemanticMemory;
pub use compaction::MemoryCompactor;
pub use protocol::{ProtocolAdapter, ProtocolAdapterConfig, AdapterType};
