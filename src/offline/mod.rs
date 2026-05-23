//! Offline Mode Support Module
//!
//! Provides local caching and offline fallback capabilities for CarpAI.

pub mod cache;
pub mod hnsw_index;
pub mod vector_store;

pub use cache::{LocalCache, CacheEntry, CacheStats, OfflineModeManager};
pub use hnsw_index::HNSWIndex;
pub use vector_store::LocalVectorStore;
