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
// - hierarchical:  Multi-level memory organization (orphaned) [2025-05-25]
//   Status: Has #[allow(dead_code)], waiting for Phase 1C alignment with carpai-internal
// - knowledge_graph: Extended knowledge graph (orphaned) [2025-05-25]
//   Status: Has #[allow(dead_code)], extends base KnowledgeGraph with find_related()
// - knowledge:      Centralized knowledge base (orphaned)
// - knowledge_agents: Knowledge agent automation (orphaned) [2025-05-25]
//   Status: Has #[allow(dead_code)], provides automated knowledge management
// - types:          Additional memory type definitions (orphaned)
//
// DEAD CODE ANALYSIS RESULT (2025-05-25):
// ✅ Confirmed: These 4 modules have ZERO references in carpai-core or parent crates
// ⚠️ Decision: Keep as scaffolding with dead_code allowance until Phase 1C
// 📅 Next action: Integrate in Phase 1C after type alignment with carpai-internal
//
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
