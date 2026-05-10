//! Memory system for cross-session learning
//!
//! Provides persistent memory that survives across sessions, organized by:
//! - Project (per working directory)
//! - Global (user-level preferences)
//!
//! Integrates with the Haiku sidecar for relevance verification and extraction.

use crate::memory_graph::{GRAPH_VERSION, MemoryGraph};
use crate::memory_types::{
    InjectedMemoryItem, MemoryActivity, MemoryEvent, MemoryEventKind, MemoryState, StepResult,
    StepStatus,
    ranking::{top_k_by_ord, top_k_by_score},
};
use crate::sidecar::Sidecar;
use crate::storage;
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

#[path = "memory/activity.rs"]
mod activity;
mod cache;
#[path = "memory/pending.rs"]
mod pending;
#[path = "memory_prompt.rs"]
mod prompt_support;

pub use crate::memory_types::{
    MemoryCategory, MemoryEntry, MemoryScope, MemoryStore, Reinforcement, TrustLevel,
    format_relevant_display_prompt, format_relevant_prompt,
};
use crate::memory_types::{
    collect_skill_query_terms, format_entries_for_prompt, memory_matches_search, memory_score,
    normalize_memory_search_text, normalize_search_text, skill_retrieval_bonus,
};
pub use activity::{
    activity_snapshot, add_event, apply_remote_activity_snapshot, check_staleness, clear_activity,
    get_activity, pipeline_start, pipeline_update, record_injected_prompt, set_state,
};
use cache::{cache_graph, cached_graph};
#[cfg(test)]
use pending::insert_pending_memory_for_test;
pub use pending::{
    PendingMemory, clear_all_injected_memories, clear_all_pending_memory, clear_injected_memories,
    clear_pending_memory, has_any_pending_memory, has_pending_memory, is_memory_injected,
    is_memory_injected_any, mark_memories_injected, set_pending_memory,
    set_pending_memory_with_ids, set_pending_memory_with_ids_and_display, sync_injected_memories,
    take_pending_memory,
};
use pending::{begin_memory_check, finish_memory_check};
pub(crate) use prompt_support::{format_context_for_extraction, format_context_for_relevance};

const LEGACY_NOTE_CATEGORY: &str = "note";
const MEMORY_RELEVANCE_MAX_CANDIDATES: usize = 30;
const MEMORY_RELEVANCE_MAX_RESULTS: usize = 10;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct LegacyNotesFile {
    #[serde(default)]
    entries: Vec<LegacyNoteEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacyNoteEntry {
    id: String,
    content: String,
    created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tag: Option<String>,
}

pub type MemoryEventSink = Arc<dyn Fn(crate::protocol::ServerEvent) + Send + Sync>;

pub fn memory_sidecar_enabled() -> bool {
    crate::config::config().agents.memory_sidecar_enabled
}

fn emit_memory_activity(event_tx: Option<&MemoryEventSink>) {
    let (Some(event_tx), Some(activity)) = (event_tx, activity_snapshot()) else {
        return;
    };
    (event_tx)(crate::protocol::ServerEvent::MemoryActivity { activity });
}

trait MemoryEntryEmbeddingExt {
    fn ensure_embedding(&mut self) -> bool;
}

impl MemoryEntryEmbeddingExt for MemoryEntry {
    /// Generate and set embedding if not already present.
    /// Returns true if embedding was generated, false if already exists or failed.
    fn ensure_embedding(&mut self) -> bool {
        if self.embedding.is_some() {
            return false;
        }

        match crate::embedding::embed(&self.content) {
            Ok(embedding) => {
                self.embedding = Some(embedding);
                true
            }
            Err(err) => {
                crate::logging::info(&format!("Failed to generate embedding: {err}"));
                false
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct MemoryManager {
    project_dir: Option<PathBuf>,
    /// When true, use isolated test storage instead of real memory
    test_mode: bool,
    include_skills: bool,
}

mod manager_impl;

pub use manager_impl::{EMBEDDING_MAX_HITS, EMBEDDING_SIMILARITY_THRESHOLD};


#[cfg(test)]
#[path = "memory_tests.rs"]
mod tests;
