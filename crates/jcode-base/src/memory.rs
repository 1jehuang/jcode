//! Memory system for cross-session learning
//!
//! Provides persistent memory that survives across sessions, organized by:
//! - Project (per working directory)
//! - Global (user-level preferences)
//!
//! Jev provides typed relevance decisions. Optional text-generating extraction
//! is independent of recall and is never required to read existing memories.

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
    CitationStatus, MemoryCategory, MemoryEntry, MemoryScope, MemoryStore, Reinforcement,
    SourceCitation, TrustLevel, citation_stale_mark, format_relevant_display_prompt,
    format_relevant_prompt, format_relevant_prompt_verified,
};
use crate::memory_types::{
    bm25_token_stream, collect_skill_query_terms, format_entries_for_prompt,
    memory_matches_search, normalize_memory_search_text, normalize_search_text,
    skill_retrieval_bonus,
};
pub use activity::{
    activity_snapshot, add_event, apply_remote_activity_snapshot, check_staleness, clear_activity,
    get_activity, pipeline_start, pipeline_update, record_injected_prompt, set_state,
};
use cache::{cache_graph, cached_graph};
pub(crate) use pending::set_pending_memory_for_project_with_selection;
pub use pending::{
    PendingMemory, clear_all_injected_memories, clear_all_pending_memory, clear_injected_memories,
    clear_pending_memory, has_any_pending_memory, has_pending_memory, is_memory_injected,
    is_memory_injected_any, mark_memories_injected, mark_memories_known, set_pending_memory,
    set_pending_memory_for_project, set_pending_memory_with_ids,
    set_pending_memory_with_ids_and_display, sync_injected_memories, take_pending_memory,
    take_pending_memory_for_project,
};
#[cfg(test)]
use pending::{backdate_injected_memory_for_test, insert_pending_memory_for_test};
use pending::{begin_memory_check, finish_memory_check};
pub(crate) use prompt_support::format_context_for_extraction;
pub use prompt_support::{
    focus_query_text, format_context_for_relevance, format_focused_query_for_relevance,
};

const LEGACY_NOTE_CATEGORY: &str = "note";
const MEMORY_RELEVANCE_MAX_CANDIDATES: usize = 30;
const MEMORY_RELEVANCE_MAX_RESULTS: usize = 10;

/// Producer of synthetic [`MemoryEntry`] values contributed by a higher layer.
///
/// Used to invert the legacy `memory -> skill` dependency: the `skill` layer
/// (which already depends on `MemoryEntry`) registers a provider that turns the
/// shared skill registry into synthetic memory entries, instead of `memory`
/// reaching up into `skill::SkillRegistry`.
type SyntheticEntryProvider = fn() -> Vec<MemoryEntry>;

static SYNTHETIC_ENTRY_PROVIDERS: std::sync::LazyLock<
    std::sync::RwLock<Vec<SyntheticEntryProvider>>,
> = std::sync::LazyLock::new(|| std::sync::RwLock::new(Vec::new()));

/// Register a provider of synthetic memory entries (e.g. skills).
///
/// Inverts `memory -> skill`: higher layers register their synthetic-entry
/// source here at startup so `memory` stays free of upward references.
pub fn register_synthetic_entry_provider(provider: SyntheticEntryProvider) {
    SYNTHETIC_ENTRY_PROVIDERS
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(provider);
}

#[cfg(test)]
fn collect_synthetic_entries() -> Vec<MemoryEntry> {
    let providers = SYNTHETIC_ENTRY_PROVIDERS
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut entries = Vec::new();
    for provider in providers.iter() {
        entries.extend(provider());
    }
    entries
}

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

/// Optional text-generating extraction is independent of Jev recall.
pub fn memory_sidecar_enabled() -> bool {
    crate::config::config().agents.memory_sidecar_enabled
}

/// Availability of the optional extraction sidecar, never used to gate recall.
pub fn memory_llm_judge_available() -> bool {
    memory_sidecar_enabled() && crate::sidecar::Sidecar::llm_backend_available()
}

/// Recall requires a Jev credential route. Subscription entitlement is checked
/// by the gateway, not inferred from a cached client tier.
pub fn memory_runtime_active() -> bool {
    crate::jev::JevClient::available()
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

        match crate::embedding_backend::embed_passage_active(&self.content) {
            Ok((embedding, model_id)) => {
                // Tag with the ACTIVE backend's model id so dense search only
                // compares vectors from the same model/vector space. Untagged
                // legacy memories are treated as local MiniLM via
                // effective_embedding_model().
                self.set_embedding(Some(embedding), Some(model_id));
                true
            }
            Err(err) => {
                crate::logging::info(&format!("Failed to generate embedding: {err}"));
                false
            }
        }
    }
}

/// Per-project memory file for `project_dir`. Keyed by the absolute path, so a
/// migrated session that keeps the same repo path keeps the same memories.
pub fn project_memory_file(project_dir: &std::path::Path) -> Result<PathBuf> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    project_dir.hash(&mut hasher);
    let project_hash = format!("{:016x}", hasher.finish());
    let memory_dir = storage::jcode_dir()?.join("memory").join("projects");
    Ok(memory_dir.join(format!("{}.json", project_hash)))
}

#[derive(Debug, Clone)]
pub struct MemoryManager {
    project_dir: Option<PathBuf>,
    /// When true, use isolated test storage instead of real memory
    test_mode: bool,
    include_skills: bool,
}

/// Recall output retains the exact entries supplied to the relevance judge so
/// publication can reject even non-rendered metadata changes during inference.
#[derive(Default)]
pub struct MemoryRelevanceResult {
    pub prompt: Option<String>,
    pub display_prompt: Option<String>,
    pub selected_entries: Vec<MemoryEntry>,
}

impl MemoryManager {
    pub fn new() -> Self {
        Self {
            project_dir: None,
            test_mode: false,
            include_skills: true,
        }
    }

    pub fn with_project_dir(mut self, project_dir: impl Into<PathBuf>) -> Self {
        self.project_dir = Some(project_dir.into());
        self
    }

    pub fn with_skills(mut self, include_skills: bool) -> Self {
        self.include_skills = include_skills;
        self
    }

    /// Create a memory manager in test mode (isolated storage)
    pub fn new_test() -> Self {
        Self {
            project_dir: None,
            test_mode: true,
            include_skills: true,
        }
    }

    /// Check if running in test mode
    pub fn is_test_mode(&self) -> bool {
        self.test_mode
    }

    /// Set test mode (for debug sessions)
    pub fn set_test_mode(&mut self, test_mode: bool) {
        self.test_mode = test_mode;
    }

    /// Clear all test memories (only works in test mode)
    pub fn clear_test_storage(&self) -> Result<()> {
        if !self.test_mode {
            anyhow::bail!("clear_test_storage only allowed in test mode");
        }

        let test_dir = storage::jcode_dir()?.join("memory").join("test");
        if test_dir.exists() {
            std::fs::remove_dir_all(&test_dir)?;
            crate::logging::info("Cleared test memory storage");
        }
        Ok(())
    }

    fn get_project_dir(&self) -> Option<PathBuf> {
        self.project_dir.clone()
    }

    fn project_memory_path(&self) -> Result<Option<PathBuf>> {
        // In test mode, use test directory
        if self.test_mode {
            let test_dir = storage::jcode_dir()?.join("memory").join("test");
            std::fs::create_dir_all(&test_dir)?;
            return Ok(Some(test_dir.join("test_project.json")));
        }

        let project_dir = match self.get_project_dir() {
            Some(d) => d,
            None => return Ok(None),
        };

        project_memory_file(&project_dir).map(Some)
    }

    fn legacy_notes_path(&self) -> Result<Option<PathBuf>> {
        if self.test_mode {
            let test_dir = storage::jcode_dir()?.join("notes").join("test");
            std::fs::create_dir_all(&test_dir)?;
            return Ok(Some(test_dir.join("test_notes.json")));
        }

        let project_dir = match self.get_project_dir() {
            Some(d) => d,
            None => return Ok(None),
        };

        let project_hash = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            project_dir.hash(&mut hasher);
            format!("{:016x}", hasher.finish())
        };

        Ok(Some(
            storage::jcode_dir()?
                .join("notes")
                .join(format!("{}.json", project_hash)),
        ))
    }

    fn normalize_graph_search_text(graph: &mut MemoryGraph) -> bool {
        let mut changed = false;
        for memory in graph.memories.values_mut() {
            let expected = normalize_memory_search_text(&memory.content, &memory.tags);
            if memory.search_text != expected {
                memory.search_text = expected;
                changed = true;
            }
        }
        changed
    }

    fn import_legacy_notes_into_graph(&self, graph: &mut MemoryGraph) -> Result<bool> {
        let Some(path) = self.legacy_notes_path()? else {
            return Ok(false);
        };
        if !path.exists() {
            return Ok(false);
        }

        let legacy: LegacyNotesFile = storage::read_json(&path)?;
        if legacy.entries.is_empty() {
            return Ok(false);
        }

        let mut changed = false;
        for note in legacy.entries {
            if graph.memories.contains_key(&note.id) {
                continue;
            }

            let mut entry = MemoryEntry::new(
                MemoryCategory::Custom(LEGACY_NOTE_CATEGORY.to_string()),
                note.content,
            );
            entry.id = note.id;
            entry.created_at = note.created_at;
            entry.updated_at = note.created_at;
            entry.source = Some("legacy_remember_migration".to_string());
            if let Some(tag) = note.tag {
                entry.tags.push(tag);
            }
            graph.add_memory(entry);
            changed = true;
        }

        Ok(changed)
    }

    fn global_memory_path(&self) -> Result<PathBuf> {
        if self.test_mode {
            let test_dir = storage::jcode_dir()?.join("memory").join("test");
            std::fs::create_dir_all(&test_dir)?;
            Ok(test_dir.join("test_global.json"))
        } else {
            Ok(storage::jcode_dir()?.join("memory").join("global.json"))
        }
    }

    pub fn load_project(&self) -> Result<MemoryStore> {
        match self.project_memory_path()? {
            Some(path) if path.exists() => storage::read_json(&path),
            _ => Ok(MemoryStore::new()),
        }
    }

    pub fn load_global(&self) -> Result<MemoryStore> {
        let path = self.global_memory_path()?;
        if path.exists() {
            storage::read_json(&path)
        } else {
            Ok(MemoryStore::new())
        }
    }

    pub fn save_project(&self, store: &MemoryStore) -> Result<()> {
        if let Some(path) = self.project_memory_path()? {
            storage::write_json(&path, store)?;
        }
        Ok(())
    }

    pub fn save_global(&self, store: &MemoryStore) -> Result<()> {
        let path = self.global_memory_path()?;
        storage::write_json(&path, store)
    }

    /// Store with embedding inference. Exact duplicates reinforce an existing
    /// entry only within the requested scope, never mutate a different project.
    /// The incoming entry is embedded (persisted) before the R2 scan, so the
    /// scan reads the stored vector and `find_similar_hybrid` can retrieve the
    /// row later. Fail-open: without a usable backend the entry is added
    /// exactly as before (no vector, keyword-only).
    pub fn remember_project(&self, entry: MemoryEntry) -> Result<String> {
        anyhow::ensure!(
            self.project_memory_path()?.is_some(),
            "Project memory requires a working directory; use global scope explicitly"
        );
        let mut graph = self.load_project_graph()?;
        let id = Self::remember_in_graph(&mut graph, entry);
        self.save_project_graph(&graph)?;
        Ok(id)
    }

    pub fn remember_global(&self, entry: MemoryEntry) -> Result<String> {
        let mut graph = self.load_global_graph()?;
        let id = Self::remember_in_graph(&mut graph, entry);
        self.save_global_graph(&graph)?;
        Ok(id)
    }

    fn remember_in_graph(graph: &mut MemoryGraph, mut entry: MemoryEntry) -> String {
        let normalized = entry.content.trim();
        let duplicate = graph
            .active_memories()
            .into_iter()
            .find(|existing| {
                existing.category == entry.category && existing.content.trim() == normalized
            })
            .map(|existing| existing.id.clone());
        if let Some(id) = duplicate {
            if let Some(existing) = graph.get_memory_mut(&id) {
                existing.reinforce(entry.source.as_deref().unwrap_or("dedup"), 0);
            }
            return id;
        }
        // Write-path embedding (restored): persist the incoming entry's vector
        // so hybrid retrieval can see this row later. Fail-open: without a
        // usable backend this is a no-op and the entry stays keyword-only.
        // The R2 scan below is unchanged: candidate vectors stay transient and
        // are never persisted.
        entry.ensure_embedding();
        // R2 (UPDATE): candidate scan between the exact-dup check and add.
        // Same store (this graph) + same category; supersede only on high
        // vector similarity AND an explicit update signal / deterministic
        // contradiction. Never on similarity alone (H-006 near-miss guard).
        // Fail-open: without embeddings the scan is skipped and the entry is
        // added exactly as before. Scan-generated candidate vectors are
        // transient and never persisted (only the incoming entry's own
        // write-path vector, set above, is stored).
        if let Some(stale_id) = Self::find_update_candidate(
            graph,
            &entry,
            Self::UPDATE_SIMILARITY_THRESHOLD,
            &|text| {
                crate::embedding_backend::embed_passage_active(text)
                    .ok()
                    .map(|(vec, _)| vec)
            },
        ) {
            let id = graph.add_memory(entry);
            graph.supersede(&id, &stale_id);
            return id;
        }
        graph.add_memory(entry)
    }

    /// Minimum cosine similarity for the R2 UPDATE candidate scan.
    /// Validated end to end by the 08-stale-writer arm (2026-09-27): real
    /// ONNX passage cosine clears this on all 6 UPDATE fixtures while the
    /// H-006-shaped near-miss stays distinct via `detect_update`.
    pub(crate) const UPDATE_SIMILARITY_THRESHOLD: f32 = 0.80;

    /// Substrings (lowercased) marking an explicit supersede signal: the new
    /// text announces a replacement rather than an additional fact.
    const UPDATE_MARKERS: &'static [&'static str] = &[
        "moved to",
        "changed to",
        "update",
        "updated",
        "reschedul",
        "switch",
        "is now",
        "are now",
        "became",
        "become",
        "replac",
        "extended to",
        "extend to",
    ];

    /// Minimum shared-anchor coverage for the R2 UPDATE gate: the fraction
    /// of the OLD (candidate) fact's content tokens that must also appear in
    /// the incoming text. Calibrated 2026-09-27 on the blind harness pairs
    /// (11-r2-precision): 16 true UPDATE pairs cover 0.25..1.0, the 4 cosine
    /// false-supersede pairs cover 0.062..0.143, so 0.20 separates them with
    /// margin on both sides. Threshold-only separation is impossible
    /// (coffee true-min 0.8235 < false-max 0.8340), hence this lexical gate.
    pub(crate) const UPDATE_MIN_ANCHOR_COVERAGE: f32 = 0.20;

    /// Stopwords excluded from the shared-anchor extraction in `detect_update`.
    const UPDATE_STOPWORDS: &'static [&'static str] = &[
        "the", "and", "for", "with", "from", "that", "this", "are", "was", "were", "has",
        "have", "had", "will", "would", "can", "not", "but", "our", "your",
    ];

    /// Content tokens of `text`: lowercased alphanumeric runs longer than 2
    /// chars (numeric runs kept at any length: values like `09:00` split into
    /// `09`/`00`, and the replaced-value check needs them), minus stopwords.
    /// Deterministic; no model involved.
    fn update_content_tokens(text: &str) -> Vec<String> {
        text.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|tok| {
                if Self::UPDATE_STOPWORDS.contains(tok) {
                    return false;
                }
                // Numeric runs kept at len >= 2: values like `09:00` split
                // into `09`/`00`, and the replaced-value check needs them.
                tok.len() > 2 || (tok.len() == 2 && tok.chars().all(|c| c.is_ascii_digit()))
            })
            .map(|tok| tok.to_string())
            .collect()
    }

    /// Deterministic same-predicate contradiction test (R2 gate, no LLM).
    ///
    /// True iff ALL hold: (a) the new text carries an explicit update marker;
    /// (b) the incoming text covers at least `UPDATE_MIN_ANCHOR_COVERAGE of
    /// the old fact's content tokens (same predicate scope — the update must
    /// restate the old fact's subject, not merely share generic vocabulary);
    /// (c) each side has content the other lacks (a value was REPLACED, not
    /// merely restated or extended). The H-006 guard pair (Ana/Theo
    /// mentorship vs Ana/Ravi buddy) fails (a): no update markers, so the two
    /// stay distinct no matter how similar their vectors are. The 4 harness
    /// false-supersede pairs (K-003-new→K-002, K-006-new→K-001, K-001-new→R-003,
    /// R-003-new→R-001, all cosine ≥ 0.80 with markers present) fail (b):
    /// they share only generic tokens (`update`, `moved`, `minutes`, `00`),
    /// covering ≤ 0.143 of the victim fact, while every true UPDATE pair
    /// covers ≥ 0.25 with subject nouns.
    pub(crate) fn detect_update(old_content: &str, new_content: &str) -> bool {
        let new_lower = new_content.to_lowercase();
        if !Self::UPDATE_MARKERS
            .iter()
            .any(|marker| new_lower.contains(marker))
        {
            return false;
        }
        let old_tokens = Self::update_content_tokens(old_content);
        let new_tokens = Self::update_content_tokens(new_content);
        if old_tokens.is_empty() || new_tokens.is_empty() {
            return false;
        }
        let old_unique: std::collections::HashSet<&str> =
            old_tokens.iter().map(|tok| tok.as_str()).collect();
        let new_unique: std::collections::HashSet<&str> =
            new_tokens.iter().map(|tok| tok.as_str()).collect();
        let shared = old_unique.intersection(&new_unique).count();
        if (shared as f32) / (old_unique.len() as f32) < Self::UPDATE_MIN_ANCHOR_COVERAGE
        {
            return false;
        }
        let old_minus_new = old_unique.difference(&new_unique).next().is_some();
        let new_minus_old = new_unique.difference(&old_unique).next().is_some();
        old_minus_new && new_minus_old
    }

    /// R2 candidate scan: find the active same-category memory `entry` updates.
    ///
    /// `embed` produces passage vectors (production: `embed_passage_active`;
    /// tests inject stubs). Returns `None` (fail-open, plain add) when the
    /// incoming text cannot be embedded. Candidate vectors prefer the stored
    /// embedding when its model matches the active backend, else embed the
    /// candidate content transiently. A candidate wins only on cosine >=
    /// `threshold` AND `detect_update` — never on similarity alone.
    pub(crate) fn find_update_candidate(
        graph: &MemoryGraph,
        entry: &MemoryEntry,
        threshold: f32,
        embed: &dyn Fn(&str) -> Option<Vec<f32>>,
    ) -> Option<String> {
        let active_model = crate::embedding_backend::active_model_id();
        let incoming_vec: Vec<f32> = match entry.embedding.as_deref() {
            Some(stored) if entry.effective_embedding_model() == active_model => {
                stored.to_vec()
            }
            _ => embed(&entry.content)?,
        };
        let mut best: Option<(String, f32)> = None;
        for candidate in graph.active_memories() {
            if candidate.id == entry.id || candidate.category != entry.category {
                continue;
            }
            let candidate_vec: Vec<f32> = match candidate.embedding.as_deref() {
                Some(stored) if candidate.effective_embedding_model() == active_model => {
                    stored.to_vec()
                }
                _ => match embed(&candidate.content) {
                    Some(vec) => vec,
                    None => continue,
                },
            };
            let similarity =
                crate::embedding::cosine_similarity(&incoming_vec, &candidate_vec);
            if similarity < threshold {
                continue;
            }
            if !Self::detect_update(&candidate.content, &entry.content) {
                continue;
            }
            let replace = match &best {
                Some((_, best_sim)) => similarity > *best_sim,
                None => true,
            };
            if replace {
                best = Some((candidate.id.clone(), similarity));
            }
        }
        best.map(|(id, _)| id)
    }

    /// Insert or update a memory with a stable ID in the project graph.
    /// Preserves existing inbound/outbound graph relationships while refreshing
    /// content and tags.
    pub fn upsert_project_memory(&self, entry: MemoryEntry) -> Result<String> {
        let mut graph = self.load_project_graph()?;
        let id = self.upsert_memory_in_graph(&mut graph, entry);
        self.save_project_graph(&graph)?;
        Ok(id)
    }

    /// Insert or update a memory with a stable ID in the global graph.
    /// Preserves existing inbound/outbound graph relationships while refreshing
    /// content and tags.
    pub fn upsert_global_memory(&self, entry: MemoryEntry) -> Result<String> {
        let mut graph = self.load_global_graph()?;
        let id = self.upsert_memory_in_graph(&mut graph, entry);
        self.save_global_graph(&graph)?;
        Ok(id)
    }

    fn upsert_memory_in_graph(
        &self,
        graph: &mut crate::memory_graph::MemoryGraph,
        entry: MemoryEntry,
    ) -> String {
        let id = entry.id.clone();

        let Some(existing_snapshot) = graph.get_memory(&id).cloned() else {
            return graph.add_memory(entry);
        };

        let old_tags: std::collections::HashSet<String> =
            existing_snapshot.tags.iter().cloned().collect();
        let new_tags: std::collections::HashSet<String> = entry.tags.iter().cloned().collect();

        for tag in old_tags.difference(&new_tags) {
            graph.untag_memory(&id, tag);
        }
        for tag in new_tags.difference(&old_tags) {
            graph.tag_memory(&id, tag);
        }

        if let Some(existing) = graph.get_memory_mut(&id) {
            let content_changed = existing.content != entry.content;
            existing.category = entry.category;
            existing.content = entry.content;
            existing.tags = entry.tags;
            existing.updated_at = entry.updated_at;
            existing.source = entry.source;
            existing.trust = entry.trust;
            existing.active = entry.active;
            existing.superseded_by = entry.superseded_by;
            existing.confidence = entry.confidence;
            if content_changed {
                existing.set_embedding(None, None);
            }
        }

        id
    }

    pub fn find_similar(
        &self,
        text: &str,
        threshold: f32,
        limit: usize,
    ) -> Result<Vec<(MemoryEntry, f32)>> {
        // Generate embedding for query text
        let query_embedding = match crate::embedding_backend::embed_query_active(text) {
            Ok((emb, _model)) => emb,
            Err(e) => {
                crate::logging::info(&format!(
                    "Embedding failed, falling back to keyword search: {}",
                    e
                ));
                return Ok(Vec::new());
            }
        };

        self.find_similar_with_embedding(&query_embedding, threshold, limit)
    }

    pub fn find_similar_scoped(
        &self,
        text: &str,
        threshold: f32,
        limit: usize,
        scope: MemoryScope,
    ) -> Result<Vec<(MemoryEntry, f32)>> {
        let query_embedding = match crate::embedding_backend::embed_query_active(text) {
            Ok((emb, _model)) => emb,
            Err(e) => {
                crate::logging::info(&format!(
                    "Embedding failed, falling back to keyword search: {}",
                    e
                ));
                return Ok(Vec::new());
            }
        };

        self.find_similar_with_embedding_scoped(&query_embedding, threshold, limit, scope)
    }

    /// Find memories similar to the given embedding
    pub fn find_similar_with_embedding(
        &self,
        query_embedding: &[f32],
        threshold: f32,
        limit: usize,
    ) -> Result<Vec<(MemoryEntry, f32)>> {
        let entries_with_emb = self.collect_all_memories_with_embeddings()?;
        Self::score_and_filter(entries_with_emb, query_embedding, "", threshold, limit)
    }

    pub fn find_similar_with_embedding_scoped(
        &self,
        query_embedding: &[f32],
        threshold: f32,
        limit: usize,
        scope: MemoryScope,
    ) -> Result<Vec<(MemoryEntry, f32)>> {
        let entries_with_emb = self.collect_memories_with_embeddings_scoped(scope)?;
        Self::score_and_filter(entries_with_emb, query_embedding, "", threshold, limit)
    }

    /// Hybrid retrieval: fuse dense (embedding cosine) and sparse (BM25 over
    /// memory search text) rankings with Reciprocal Rank Fusion.
    ///
    /// This is the recall-oriented live retrieval path. Unlike
    /// `find_similar_with_embedding`, it does NOT apply a hard cosine floor
    /// (which benchmarking showed zeroes out recall): instead it pulls a
    /// generous candidate pool from each retriever and lets RRF + the
    /// downstream sidecar/rerank decide. Lexical signal is essential for the
    /// identifier/path/term-heavy memories agents store.
    pub fn find_similar_hybrid(
        &self,
        query_text: &str,
        query_embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<(MemoryEntry, f32)>> {
        self.find_similar_hybrid_scoped(query_text, query_embedding, limit, MemoryScope::All)
    }

    pub fn find_similar_hybrid_scoped(
        &self,
        query_text: &str,
        query_embedding: &[f32],
        limit: usize,
        scope: MemoryScope,
    ) -> Result<Vec<(MemoryEntry, f32)>> {
        let entries = self.collect_memories_with_embeddings_scoped(scope)?;
        Ok(Self::hybrid_fuse(
            entries,
            query_text,
            query_embedding,
            limit,
        ))
    }

    /// Bench/diagnostic helper for the slot-B gate: the EXACT collection +
    /// active-filter the live `get_relevant_parallel` path applies
    /// (`collect_scoped(self, MemoryScope::All)` + `entry.active`; the
    /// already-injected filter is session-pipeline state with no bench
    /// equivalent, and the seeded bench graphs have no injected ids anyway),
    /// WITHOUT the embedding-presence filter (the stage keeps
    /// embedding-less entries reachable via BM25).
    pub fn prefilter_bench_entries(&self) -> Result<Vec<MemoryEntry>> {
        Ok(crate::memory_jev::collect_scoped(self, MemoryScope::All)?
            .into_iter()
            .filter(|entry| entry.active)
            .collect())
    }

    /// RRF k from config, clamped to [1.0, 1000.0]. Config knob
    /// `memory_rrf_k` (default 60.0); env `JCODE_MEMORY_RRF_K` wins.
    /// NaN/inf can arrive via TOML (`nan`) since clamp preserves NaN:
    /// fall back to the default instead of poisoning every fused score.
    /// Public so the recall bench fuses with the same k as the runtime.
    pub fn rrf_k() -> f32 {
        let v = crate::config::config().agents.memory_rrf_k;
        if !v.is_finite() {
            return 60.0;
        }
        v.clamp(1.0, 1000.0)
    }

    /// Dense-list weight for hybrid RRF fusion (`memory_rrf_dense_weight`,
    /// default 1.0 = equal weights); env `JCODE_MEMORY_RRF_DENSE_W` wins.
    /// Non-finite or non-positive values fall back to 1.0 so a bad config
    /// can never zero out or invert the dense half.
    /// Public so the recall bench fuses with the same weight as the runtime.
    pub fn rrf_dense_weight() -> f32 {
        let v = crate::config::config().agents.memory_rrf_dense_weight;
        if !v.is_finite() || v <= 0.0 {
            return 1.0;
        }
        v
    }

    /// Convex-combination weight for hybrid fusion (C3 port, default OFF:
    /// `memory_convex_alpha`, default 0.0); env `JCODE_MEMORY_CONVEX_ALPHA`
    /// wins. 0.0 selects the shipped RRF path; (0, 1] selects convex
    /// fusion `alpha * dense_norm + (1 - alpha) * sparse_norm` with each
    /// leg min-max normalized over its own retrieved pool. Non-finite or
    /// negative values fall back to 0.0 (RRF path) so a bad config can
    /// never silently switch the fusion rule; values above 1.0 clamp to
    /// 1.0 (dense-only). Public so the recall bench fuses exactly as the
    /// runtime when driving alpha grids via env.
    pub fn convex_alpha() -> f32 {
        let v = crate::config::config().agents.memory_convex_alpha;
        if !v.is_finite() || v <= 0.0 {
            return 0.0;
        }
        v.clamp(0.0, 1.0)
    }

    /// Recency-prior weight for hybrid RRF fusion (G-R, shipped 2026-09-30:
    /// `memory_recency_weight`, default 0.05); env
    /// `JCODE_MEMORY_RECENCY_W` wins. Non-finite or negative falls back
    /// to 0.0 so a bad config can never penalize or invert ranking.
    /// Public so the recall bench fuses with the same weight as runtime.
    pub fn recency_weight() -> f32 {
        let v = crate::config::config().agents.memory_recency_weight;
        if !v.is_finite() || v < 0.0 {
            return 0.0;
        }
        v
    }

    /// Recency half-life override in days (G-R, shipped 2026-09-30:
    /// `memory_recency_tau_days`, default 90.0); env
    /// `JCODE_MEMORY_RECENCY_TAU_DAYS` wins. Non-finite or negative
    /// falls back to 0.0 (per-category table).
    pub fn recency_tau_override_days() -> f32 {
        let v = crate::config::config().agents.memory_recency_tau_days;
        if !v.is_finite() || v < 0.0 {
            return 0.0;
        }
        v
    }

    /// Recency half-life in days for an entry: global override when
    /// positive, else the existing decay table (Fact 30 / Entity 60 /
    /// Preference 90 / Correction 365 / Custom 45).
    pub fn recency_half_life_days(category: &MemoryCategory) -> f32 {
        let ov = Self::recency_tau_override_days();
        if ov > 0.0 {
            return ov;
        }
        match category {
            MemoryCategory::Correction => 365.0,
            MemoryCategory::Preference => 90.0,
            MemoryCategory::Fact => 30.0,
            MemoryCategory::Entity => 60.0,
            MemoryCategory::Custom(_) => 45.0,
        }
    }

    /// Bounded additive recency bonus for one entry (G-R, shipped):
    /// `w_r * 0.5^(age_days / half_life)` with touch-aware age
    /// (`now - max(created_at, updated_at)`). Returns 0.0 when the knob
    /// is off (explicit opt-out restores pre-G-R ranking).
    pub fn recency_bonus(entry: &MemoryEntry) -> f32 {
        let w = Self::recency_weight();
        if w <= 0.0 {
            return 0.0;
        }
        let latest = entry.created_at.max(entry.updated_at);
        let age_days = (Utc::now() - latest).num_seconds().max(0) as f32 / 86_400.0;
        let tau = Self::recency_half_life_days(&entry.category);
        Self::recency_bonus_for(w, tau, age_days)
    }

    /// Pure recency math (unit-testable, no clock, no config):
    /// `w * 0.5^(age_days / tau)`. Returns 0.0 for non-positive weight,
    /// non-finite/non-positive tau, or negative age.
    pub fn recency_bonus_for(weight: f32, tau_days: f32, age_days: f32) -> f32 {
        if weight <= 0.0 || !weight.is_finite() {
            return 0.0;
        }
        if !tau_days.is_finite() || tau_days <= 0.0 {
            return 0.0;
        }
        if !age_days.is_finite() || age_days < 0.0 {
            return 0.0;
        }
        weight * 0.5f32.powf(age_days / tau_days)
    }

    /// Slot-B prefilter mode for Jev automatic recall (`memory_prefilter_mode`,
    /// default `"off"`; `"hybrid-topk"` enables the stage).
    /// Env `JCODE_MEMORY_PREFILTER_MODE` wins. Only the exact string
    /// `"hybrid-topk"` engages; anything else (including garbage) is off.
    pub fn prefilter_mode() -> String {
        crate::config::config()
            .agents
            .memory_prefilter_mode
            .trim()
            .to_ascii_lowercase()
    }

    /// Whether the slot-B stage is enabled this process.
    pub fn prefilter_enabled() -> bool {
        Self::prefilter_mode() == "hybrid-topk"
    }

    /// Slot-B prefilter top-K (`memory_prefilter_top_k`, default 96),
    /// clamped to [24, 480] at use. Env `JCODE_MEMORY_PREFILTER_TOP_K` wins.
    pub fn prefilter_top_k() -> usize {
        crate::config::config()
            .agents
            .memory_prefilter_top_k
            .clamp(24, 480)
    }

    /// Slot-B min corpus (`memory_prefilter_min_corpus`, default 96):
    /// the stage disengages at or below this many memories.
    /// Env `JCODE_MEMORY_PREFILTER_MIN_CORPUS` wins. Zero/near-zero file
    /// values fall back to the default so the stage cannot spin on tiny sets.
    pub fn prefilter_min_corpus() -> usize {
        let v = crate::config::config().agents.memory_prefilter_min_corpus;
        if v == 0 {
            return 96;
        }
        v
    }

    /// Slot-B wall-clock budget in ms (`memory_prefilter_budget_ms`,
    /// default 500). Env `JCODE_MEMORY_PREFILTER_BUDGET_MS` wins.
    /// Zero file values fall back to the default.
    pub fn prefilter_budget_ms() -> u64 {
        let v = crate::config::config().agents.memory_prefilter_budget_ms;
        if v == 0 {
            return 500;
        }
        v
    }

    /// Slot-B shadow-audit sampling rate (`memory_prefilter_shadow_rate`,
    /// default 0.01). Env `JCODE_MEMORY_PREFILTER_SHADOW_RATE` wins.
    /// Clamped to [0.0, 1.0]; non-finite falls back to 0.01.
    pub fn prefilter_shadow_rate() -> f32 {
        let v = crate::config::config().agents.memory_prefilter_shadow_rate;
        if !v.is_finite() {
            return 0.01;
        }
        v.clamp(0.0, 1.0)
    }
}

/// Slot-B prefilter diagnostics for the 7-day shadow audit (21 §4 gate c).
/// Kept at module level for bench/test use; the live path uses the
/// take-based `MemoryManager::prefilter_for_jev_take`.
#[derive(Debug)]
#[allow(dead_code)]
pub enum PrefilterOutcome {
    Narrowed {
        kept: Vec<MemoryEntry>,
        dropped: Vec<MemoryEntry>,
    },
    Exhaustive,
}

/// Staged result of the guarded ranking heart: kept top-K plus the
/// dropped tail (shadow-audit input). Module-private.
#[derive(Debug)]
struct PrefilterStaged {
    #[allow(dead_code)]
    kept: Vec<MemoryEntry>,
    #[allow(dead_code)]
    dropped: Vec<MemoryEntry>,
}

/// Read-only snapshot of the slot-B shadow-audit counters, backing the
/// `jcode memory-prefilter-stats` CLI surface (human + `--json`).
///
/// Field order mirrors the historical `prefilter_shadow_stats` tuple
/// `(queries, engaged, failopen, shadow_sampled)` so the CLI, the tuple
/// helper, and the 21 §4 gate notes all read the same way. All four
/// fields are cumulative process-wide totals since process start; there
/// is no per-query or drop-tail-entry persistence (the tail itself is
/// consumed live at `get_relevant_parallel` and only the sampled flag
/// is counted).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PrefilterShadowSnapshot {
    /// Queries that reached the prefilter stage (post enable/corpus/top-K gates).
    pub queries: u64,
    /// Queries where ranking completed within budget (kept top-K served).
    pub engaged: u64,
    /// Queries that fell back to exhaustive Jev judgment (any fail-open arm).
    pub failopen: u64,
    /// Shadow-sampled queries (deterministic per-query hash under the rate).
    pub shadow_sampled: u64,
}

impl PrefilterShadowSnapshot {
    /// Dropped-tail summary for the human report: engaged queries produced
    /// a narrowed set (tail exists, kept top-K served to Jev); fail-open
    /// queries fell back to exhaustive judgment with no narrowing.
    pub fn dropped_tail_summary(&self) -> String {
        format!(
            "narrowed {} queries to top-K (tail dropped from Jev judgment); {} queries fell back to exhaustive judgment",
            self.engaged, self.failopen
        )
    }
}

/// Tail-hit window aggregation (SHADOW-GATE Metric 2 gate input):
/// query-level `tail_hit = tail_hits / rejudged` with the Wilson 95%
/// upper bound, plus the ENGAGED+SAMPLED joint counter as the gate
/// denominator cross-check (`rejudged <= engaged_sampled` always: every
/// re-judgment comes from a sampled engaged query).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PrefilterTailHitSnapshot {
    /// Sampled engaged queries (Metric 2 denominator; tail exists by design).
    pub engaged_sampled: u64,
    /// Sampled engaged queries whose tail `select` completed (hook reads
    /// the live counters; failures/hook-absent queries never land here).
    pub rejudged: u64,
    /// Re-judged queries with >= 1 judge-relevant tail memory.
    pub tail_hits: u64,
    /// Query-level tail-hit rate (`tail_hits / rejudged`, 0.0 unmeasured).
    pub tail_hit: f64,
    /// Wilson score 95% upper bound on the rate (1.0 unmeasured: vacuous).
    pub tail_hit_upper_95: f64,
}

/// Slot-B fail-open arm: which of the five conflated `PREFILTER_FAILOPEN`
/// sites fired (SHADOW-GATE Metric 4). Discriminant = index into the
/// `PREFILTER_FAILOPEN_ARMS` atomic array. Order is fixed; append only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefilterFailopenArm {
    /// `prefilter_take_inner`: query embed call failed.
    EmbedError = 0,
    /// `prefilter_rank_guarded`: embed already exceeded the stage budget.
    EmbedOverBudget = 1,
    /// `prefilter_rank_guarded`: hot rank pass exceeded the stage budget.
    RankOverBudget = 2,
    /// `prefilter_rank_guarded`: ranking dropped a nonempty input entirely.
    EmptyFromNonempty = 3,
    /// `prefilter_for_jev_take`: post-completion wall-clock over budget.
    PostStageOverBudget = 4,
}

impl PrefilterFailopenArm {
    /// All five arms in index order (read-site + test iteration order).
    pub const ALL: [PrefilterFailopenArm; 5] = [
        PrefilterFailopenArm::EmbedError,
        PrefilterFailopenArm::EmbedOverBudget,
        PrefilterFailopenArm::RankOverBudget,
        PrefilterFailopenArm::EmptyFromNonempty,
        PrefilterFailopenArm::PostStageOverBudget,
    ];

    /// Index into the per-arm atomic array.
    pub fn index(self) -> usize {
        self as usize
    }

    /// Stable short name for logs and tests.
    pub fn name(self) -> &'static str {
        match self {
            PrefilterFailopenArm::EmbedError => "embed_error",
            PrefilterFailopenArm::EmbedOverBudget => "embed_over_budget",
            PrefilterFailopenArm::RankOverBudget => "rank_over_budget",
            PrefilterFailopenArm::EmptyFromNonempty => "empty_from_nonempty",
            PrefilterFailopenArm::PostStageOverBudget => "post_stage_over_budget",
        }
    }
}

/// Embed-vs-rank stage-elapsed summary (SHADOW-GATE Metric 3, live half).
/// Cumulative process-wide sums beside the existing counters: `mean =
/// sum / count`, variance via `sum_sq` (`E[x^2] - E[x]^2`). Microseconds
/// (`u64`, saturating) so sub-millisecond stages keep precision. Embed is
/// recorded whenever the query embed completes; rank whenever the hot rank
/// pass completes (over-budget and empty arms included — the cost was paid).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct PrefilterStageStats {
    pub embed_count: u64,
    pub embed_sum_us: u64,
    pub embed_sum_sq_us: u64,
    pub embed_max_us: u64,
    pub rank_count: u64,
    pub rank_sum_us: u64,
    pub rank_sum_sq_us: u64,
    pub rank_max_us: u64,
}

impl PrefilterStageStats {
    /// Mean embed-stage microseconds (0.0 with no samples).
    pub fn embed_mean_us(&self) -> f64 {
        if self.embed_count == 0 {
            0.0
        } else {
            self.embed_sum_us as f64 / self.embed_count as f64
        }
    }

    /// Mean hot-rank-pass microseconds (0.0 with no samples).
    pub fn rank_mean_us(&self) -> f64 {
        if self.rank_count == 0 {
            0.0
        } else {
            self.rank_sum_us as f64 / self.rank_count as f64
        }
    }
}

/// Cumulative count/sum/sum-sq/max accumulator behind one
/// [`PrefilterStageStats`] half. One atomic per moment; all `Relaxed`
/// (monotonic counters, exact cross-moment consistency not required).
pub(crate) struct PrefilterStageAccum {
    count: std::sync::atomic::AtomicU64,
    sum_us: std::sync::atomic::AtomicU64,
    sum_sq_us: std::sync::atomic::AtomicU64,
    max_us: std::sync::atomic::AtomicU64,
}

impl PrefilterStageAccum {
    pub(crate) const fn new() -> Self {
        Self {
            count: std::sync::atomic::AtomicU64::new(0),
            sum_us: std::sync::atomic::AtomicU64::new(0),
            sum_sq_us: std::sync::atomic::AtomicU64::new(0),
            max_us: std::sync::atomic::AtomicU64::new(0),
        }
    }

    pub(crate) fn note(&self, elapsed_us: u64) {
        use std::sync::atomic::Ordering;
        self.count.fetch_add(1, Ordering::Relaxed);
        self.sum_us.fetch_add(elapsed_us, Ordering::Relaxed);
        self.sum_sq_us.fetch_add(
            elapsed_us.saturating_mul(elapsed_us),
            Ordering::Relaxed,
        );
        self.max_us.fetch_max(elapsed_us, Ordering::Relaxed);
    }

    pub(crate) fn snapshot(&self) -> (u64, u64, u64, u64) {
        use std::sync::atomic::Ordering;
        (
            self.count.load(Ordering::Relaxed),
            self.sum_us.load(Ordering::Relaxed),
            self.sum_sq_us.load(Ordering::Relaxed),
            self.max_us.load(Ordering::Relaxed),
        )
    }
}

/// Per-sampled-engaged-query drop-tail record (SHADOW-GATE Metric 2
/// numerator plumbing). The raw query text is NEVER stored — only its
/// deterministic hash (same `DefaultHasher` posture as the sampler). Tail
/// IDs are bounded at [`PREFILTER_TAIL_LOG_MAX_IDS`] with totals kept so
/// truncation is visible to the window aggregation.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PrefilterTailDetail {
    /// Hex (`{:016x}`) hash of the query text. Joins the tail log to the
    /// re-judge output without disclosing query content.
    pub query_hash: String,
    /// Served kept-set IDs (length <= top_k by construction).
    pub kept_ids: Vec<String>,
    pub kept_total: usize,
    /// Dropped-tail IDs, truncated to the bound below.
    pub tail_ids: Vec<String>,
    /// Full tail length before truncation.
    pub tail_total: usize,
    /// True when `tail_ids` was cut at the bound.
    pub tail_truncated: bool,
    /// Stage top-K the kept set was narrowed to.
    pub top_k: usize,
    /// Sampler rate in force when the query engaged.
    pub rate: f32,
    /// Train/test split tag. Live traffic counts as test per SHADOW-GATE
    /// §4.2 unless c3 later tunes on live-derived queries (no tuning-set
    /// registry exists yet, so the constant below is the whole policy).
    pub split_tag: String,
}

/// Max tail IDs per [`PrefilterTailDetail`] log line (tail can be
/// corpus − top_k; IDs only, but the JSONL line stays bounded).
pub const PREFILTER_TAIL_LOG_MAX_IDS: usize = 1024;

/// Split tag for live shadow traffic (see [`PrefilterTailDetail::split_tag`]).
pub const PREFILTER_SPLIT_TAG: &str = "live";

impl MemoryManager {
    /// Slot-B hybrid prefilter stage for Jev automatic recall (§3).
    ///
    /// Takes the ALREADY-COLLECTED set out of `entries_opt` (post-`entry.active`
    /// filter, post-already-injected filter — exactly the set Jev would
    /// otherwise judge exhaustively), narrows it to `prefilter_top_k` via the
    /// same dense+BM25 RRF ranking the shipped hybrid path uses, and puts the
    /// narrowed set back. Returns the dropped tail (shadow-audit input;
    /// empty unless the query was shadow-sampled). Do NOT re-collect here:
    /// output ⊆ input by construction.
    ///
    /// Fail-open is scoped to this stage ONLY (the input is left in place):
    /// disabled / at-or-below `prefilter_min_corpus` / already fits top-K /
    /// embed error / empty-ranked-from-nonempty / stage wall-clock over
    /// `prefilter_budget_ms` → input untouched, returns None (the full set
    /// flows to Jev), logged once. Downstream fail-closed (JevClient::new
    /// Err, select Err, 60 s timeout) is the caller's and is untouched.
    /// Always leaves a set in `entries_opt` (never None on return).
    pub fn prefilter_for_jev_take(
        entries_opt: &mut Option<Vec<MemoryEntry>>,
        query_text: &str,
    ) -> Option<Vec<MemoryEntry>> {
        use std::sync::atomic::Ordering;
        let entries = entries_opt.take().expect("prefilter stage takes a set");
        // Disengage WITHOUT consuming: put the input straight back.
        if !Self::prefilter_enabled() {
            *entries_opt = Some(entries);
            return None;
        }
        let min_corpus = Self::prefilter_min_corpus();
        if entries.len() <= min_corpus {
            *entries_opt = Some(entries);
            return None;
        }
        let top_k = Self::prefilter_top_k();
        if entries.len() <= top_k {
            *entries_opt = Some(entries);
            return None;
        }
        let budget = std::time::Duration::from_millis(Self::prefilter_budget_ms());
        let stage_start = Instant::now();

        PREFILTER_QUERIES.fetch_add(1, Ordering::Relaxed);
        // Shadow sampling: deterministic per-query hash so the rate is stable
        // across restarts and testable without RNG plumbing.
        if Self::prefilter_shadow_sampled(query_text) {
            PREFILTER_SHADOW_SAMPLED.fetch_add(1, Ordering::Relaxed);
        }

        // The take above consumed the input: put it back so the inner
        // stage (borrow-split around the embed call) can take it again.
        // Disengaged arms returned early with the input already restored;
        // only this engaged path reaches the inner with a None slot.
        *entries_opt = Some(entries);
        match Self::prefilter_take_inner(entries_opt, query_text, stage_start, budget, top_k) {
            Some(PrefilterStaged { kept, dropped }) => {
                PREFILTER_ENGAGED.fetch_add(1, Ordering::Relaxed);
                // The staged kept set flows out of the rank guard (no
                // store-take round trip there): it lands back in the slot
                // here, so the "always leaves a set" contract holds on
                // every arm below (fail-open extends it back to full).
                *entries_opt = Some(kept);
                if stage_start.elapsed() > budget {
                    // Over budget even though ranking finished: still fail open
                    // (the guarantee is wall-clock, not just completion).
                    Self::note_prefilter_failopen(PrefilterFailopenArm::PostStageOverBudget);
                    crate::logging::info(&format!(
                        "memory prefilter over budget ({}ms > {}ms): flowing exhaustive to Jev",
                        stage_start.elapsed().as_millis(),
                        budget.as_millis()
                    ));
                    // Kept is back in entries_opt (stored just above);
                    // extend it with the tail in place.
                    if let Some(slot) = entries_opt.as_mut() {
                        slot.extend(dropped);
                    }
                    return None;
                }
                // Engaged AND sampled: the tail below is real (fail-open
                // queries have none by design), so this is the Metric 2
                // denominator and the tail-log trigger. Recomputing the
                // sampler here (pure function of the query text) agrees
                // with the counter-site decision above by construction.
                if Self::prefilter_shadow_sampled(query_text) {
                    PREFILTER_ENGAGED_SAMPLED.fetch_add(1, Ordering::Relaxed);
                    let kept_ref = entries_opt.as_ref().expect("kept just stored");
                    let detail = Self::prefilter_tail_detail(
                        query_text,
                        kept_ref,
                        &dropped,
                        top_k,
                        Self::prefilter_shadow_rate(),
                    );
                    crate::logging::info(&format!(
                        "memory prefilter shadow tail query_hash={} kept={} tail={} top_k={} rate={} split={}",
                        detail.query_hash,
                        detail.kept_total,
                        detail.tail_total,
                        detail.top_k,
                        detail.rate,
                        detail.split_tag,
                    ));
                    crate::memory_log::log_prefilter_tail(&detail);
                    // Tail-only re-judge (SHADOW-GATE Metric 2, item 5): the
                    // live kept judgments transfer as observed, so only the
                    // dropped tail needs a second `select` (same 24-entry
                    // batches, same 60 s deadline, same shipped threshold).
                    // Off-thread: the live `select` over kept proceeds
                    // untouched. Read-only: touches no recall counts or
                    // priors (none exist; keep it that way).
                    Self::spawn_shadow_tail_rejudge(
                        query_text.to_string(),
                        detail.query_hash.clone(),
                        detail.split_tag.clone(),
                        dropped,
                    );
                    drop(detail);
                }
                // The tail moved into the spawned re-judge (or was empty
                // when unsampled); the kept set stays in the slot for the
                // live `select`. Return an empty tail: the shadow audit no
                // longer flows through this return value.
                Some(Vec::new())
            }
            None => {
                // Inner already restored the FULL input on every fail-open
                // arm and bumped that arm's counter; the coarse total moves
                // here so the call-site diff stays one line per arm.
                PREFILTER_FAILOPEN.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    /// Bump one fail-open arm's counter (SHADOW-GATE Metric 4). The coarse
    /// `PREFILTER_FAILOPEN` total moves at the `prefilter_for_jev_take`
    /// call site, so every arm increment preserves `failopen ==
    /// sum(arms)` and the existing CLI/tuple readers never change.
    fn note_prefilter_failopen(arm: PrefilterFailopenArm) {
        use std::sync::atomic::Ordering;
        PREFILTER_FAILOPEN_ARMS[arm.index()].fetch_add(1, Ordering::Relaxed);
    }

    /// Per-arm fail-open counts in [`PrefilterFailopenArm::ALL`] order.
    /// Sums to `failopen` (every arm bump pairs with the coarse bump at
    /// the take call site).
    pub fn prefilter_failopen_arms() -> [u64; 5] {
        use std::sync::atomic::Ordering;
        [
            PREFILTER_FAILOPEN_ARMS[0].load(Ordering::Relaxed),
            PREFILTER_FAILOPEN_ARMS[1].load(Ordering::Relaxed),
            PREFILTER_FAILOPEN_ARMS[2].load(Ordering::Relaxed),
            PREFILTER_FAILOPEN_ARMS[3].load(Ordering::Relaxed),
            PREFILTER_FAILOPEN_ARMS[4].load(Ordering::Relaxed),
        ]
    }

    /// Engaged AND shadow-sampled queries: the Metric 2 denominator
    /// (sampled-but-failopen queries have no tail by design, so the
    /// marginal `shadow_sampled` counter cannot serve).
    pub fn prefilter_engaged_sampled() -> u64 {
        use std::sync::atomic::Ordering;
        PREFILTER_ENGAGED_SAMPLED.load(Ordering::Relaxed)
    }

    /// Embed-vs-rank stage-elapsed summary (Metric 3 live half).
    pub fn prefilter_stage_stats() -> PrefilterStageStats {
        let (embed_count, embed_sum_us, embed_sum_sq_us, embed_max_us) =
            PREFILTER_STAGE_EMBED.snapshot();
        let (rank_count, rank_sum_us, rank_sum_sq_us, rank_max_us) =
            PREFILTER_STAGE_RANK.snapshot();
        PrefilterStageStats {
            embed_count,
            embed_sum_us,
            embed_sum_sq_us,
            embed_max_us,
            rank_count,
            rank_sum_us,
            rank_sum_sq_us,
            rank_max_us,
        }
    }

    /// Build the per-query drop-tail record: hash (never raw text),
    /// kept/tail IDs (tail bounded), top_k, rate, split tag.
    /// Pure constructor so tests cover the shape without I/O.
    pub fn prefilter_tail_detail(
        query_text: &str,
        kept: &[MemoryEntry],
        dropped: &[MemoryEntry],
        top_k: usize,
        rate: f32,
    ) -> PrefilterTailDetail {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        query_text.hash(&mut hasher);
        let tail_total = dropped.len();
        PrefilterTailDetail {
            query_hash: format!("{:016x}", hasher.finish()),
            kept_ids: kept.iter().map(|e| e.id.clone()).collect(),
            kept_total: kept.len(),
            tail_ids: dropped
                .iter()
                .take(PREFILTER_TAIL_LOG_MAX_IDS)
                .map(|e| e.id.clone())
                .collect(),
            tail_total,
            tail_truncated: tail_total > PREFILTER_TAIL_LOG_MAX_IDS,
            top_k,
            rate,
            split_tag: PREFILTER_SPLIT_TAG.to_string(),
        }
    }

    /// Take-based ranking inner: embeds (fail-open restores input), then
    /// delegates to the guarded ranker. Split from `prefilter_for_jev_take`
    /// so the embed borrow of `entries_opt` never overlaps the ranker's
    /// mutable borrow.
    fn prefilter_take_inner(
        entries_opt: &mut Option<Vec<MemoryEntry>>,
        query_text: &str,
        stage_start: Instant,
        budget: std::time::Duration,
        top_k: usize,
    ) -> Option<PrefilterStaged> {
        // Same embed call the stale-rank guard uses (LRU-cached). Embed
        // BEFORE taking the set: on error the input never moved.
        let embed_start = Instant::now();
        let query_embedding = match crate::embedding::embed(query_text) {
            Ok(v) => v,
            Err(e) => {
                Self::note_prefilter_failopen(PrefilterFailopenArm::EmbedError);
                crate::logging::info(&format!(
                    "memory prefilter embed failed ({e}): flowing exhaustive to Jev"
                ));
                return None;
            }
        };
        let embed_us =
            u64::try_from(embed_start.elapsed().as_micros()).unwrap_or(u64::MAX);
        PREFILTER_STAGE_EMBED.note(embed_us);
        Self::prefilter_rank_guarded(
            entries_opt,
            query_text,
            stage_start,
            budget,
            top_k,
            &query_embedding,
        )
    }

    /// Guarded ranking heart of the slot-B stage. Puts kept top-K back into
    /// `entries_opt` on success and returns the staged tail; on ANY fail-open
    /// trigger (embed error, empty-from-nonempty, over-budget) puts the FULL
    /// input back untouched and returns None. The full-depth tail pass
    /// (shadow-sampled queries) runs on a clone.
    fn prefilter_rank_guarded(
        entries_opt: &mut Option<Vec<MemoryEntry>>,
        query_text: &str,
        stage_start: Instant,
        budget: std::time::Duration,
        top_k: usize,
        query_embedding: &[f32],
    ) -> Option<PrefilterStaged> {
        let entries = entries_opt.take().expect("rank guard takes a set");
        if stage_start.elapsed() > budget {
            Self::note_prefilter_failopen(PrefilterFailopenArm::EmbedOverBudget);
            crate::logging::info("memory prefilter embed over budget: flowing exhaustive to Jev");
            *entries_opt = Some(entries);
            return None;
        }
        let pool = top_k.saturating_mul(5).max(HYBRID_POOL_MIN);
        // Fail-open restore needs the FULL input back (the ranking core
        // consumes its input). Clone once up front: the hot path reuses it
        // only on the rare fail-open arms; the shadow-sampled path reuses
        // it for the full-depth tail pass. One clone per engaged query is
        // the price of a lossless fail-open guarantee.
        let restore_clone: Vec<MemoryEntry> = entries.clone();
        // Shadow path needs the FULL ranking (kept + tail); the hot path
        // needs only top-K. Both go through the same public core.
        let sampled = Self::prefilter_shadow_sampled(query_text);
        let rank_start = Instant::now();
        let ranked = Self::hybrid_prefilter_rank(entries, query_text, query_embedding, top_k, pool);
        let rank_us = u64::try_from(rank_start.elapsed().as_micros()).unwrap_or(u64::MAX);
        PREFILTER_STAGE_RANK.note(rank_us);
        if stage_start.elapsed() > budget {
            Self::note_prefilter_failopen(PrefilterFailopenArm::RankOverBudget);
            crate::logging::info("memory prefilter rank over budget: flowing exhaustive to Jev");
            *entries_opt = Some(restore_clone);
            return None;
        }
        if ranked.is_empty() {
            // Empty-from-nonempty: ranking dropped everything (e.g. no
            // BM25 overlap AND no dense-eligible entries). Fail open.
            Self::note_prefilter_failopen(PrefilterFailopenArm::EmptyFromNonempty);
            crate::logging::info(
                "memory prefilter ranked empty from nonempty input: flowing exhaustive to Jev",
            );
            *entries_opt = Some(restore_clone);
            return None;
        }
        let kept_ids: std::collections::HashSet<String> =
            ranked.iter().map(|(e, _)| e.id.clone()).collect();
        let mut kept = Vec::with_capacity(ranked.len());
        for (e, _) in ranked {
            kept.push(e);
        }
        let dropped: Vec<MemoryEntry> = if sampled {
            let full = Self::hybrid_prefilter_rank(
                restore_clone,
                query_text,
                query_embedding,
                usize::MAX,
                usize::MAX,
            );
            full.into_iter()
                .filter(|(e, _)| !kept_ids.contains(&e.id))
                .map(|(e, _)| e)
                .collect()
        } else {
            Vec::new()
        };
        // Kept flows out in the staged struct (no store-take round trip);
        // the caller puts it back into the slot. Fail-open arms above
        // restored the FULL input instead and returned None.
        Some(PrefilterStaged { kept, dropped })
    }

    /// Deterministic shadow-sampling decision for a query string.
    /// Public so the bench and tests exercise the exact shipped sampler.
    pub fn prefilter_shadow_sampled(query_text: &str) -> bool {
        let rate = Self::prefilter_shadow_rate();
        if rate <= 0.0 {
            return false;
        }
        if rate >= 1.0 {
            return true;
        }
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        query_text.hash(&mut hasher);
        let draw = (hasher.finish() % 10_000) as f32 / 10_000.0;
        draw < rate
    }

    /// Slot-B shadow-audit counters (21 §4 gate (c) plumbing). Read via
    /// `jcode memory-prefilter-stats` (human + `--json`) or the
    /// `prefilter_shadow_stats` tuple helper. The tail re-judge hook
    /// (`spawn_shadow_tail_rejudge`) is the consumer of the drop tail;
    /// these counters prove the sampling path engaged.
    /// (queries, engaged, failopen, shadow_sampled)
    pub fn prefilter_shadow_stats() -> (u64, u64, u64, u64) {
        let snapshot = Self::prefilter_shadow_snapshot();
        (
            snapshot.queries,
            snapshot.engaged,
            snapshot.failopen,
            snapshot.shadow_sampled,
        )
    }

    /// Read-only snapshot of the slot-B shadow-audit counters for the
    /// `jcode memory-prefilter-stats` CLI surface. Loads the four
    /// process-wide atomics; performs no scoring, no mutation, and no I/O.
    /// No new counters: the tuple accessor above stays the canonical
    /// bench/test reader.
    pub fn prefilter_shadow_snapshot() -> PrefilterShadowSnapshot {
        use std::sync::atomic::Ordering;
        PrefilterShadowSnapshot {
            queries: PREFILTER_QUERIES.load(Ordering::Relaxed),
            engaged: PREFILTER_ENGAGED.load(Ordering::Relaxed),
            failopen: PREFILTER_FAILOPEN.load(Ordering::Relaxed),
            shadow_sampled: PREFILTER_SHADOW_SAMPLED.load(Ordering::Relaxed),
        }
    }

    /// Fire-and-forget tail-only re-judge (SHADOW-GATE Metric 2, item 5).
    ///
    /// Runs on shadow-sampled ENGAGED queries only (the tail is real there;
    /// sampled-but-failopen queries have none by design, and unsampled
    /// queries never reach this call site). Spawns onto the runtime so the
    /// live `select` over the kept set proceeds untouched: 2x judge cost on
    /// ~1% of engaged traffic during the window only.
    ///
    /// Read-only by construction: the task judges cloned tail entries and
    /// bumps two counters plus one log line. It never touches recall
    /// counts or priors (none exist yet; keep it that way), never mutates
    /// the live kept set, and never feeds the accept set back into recall.
    fn spawn_shadow_tail_rejudge(
        query: String,
        query_hash: String,
        split_tag: String,
        tail: Vec<MemoryEntry>,
    ) {
        use std::sync::atomic::Ordering;
        if tail.is_empty() {
            return;
        }
        // `get_relevant_parallel` always runs under a runtime (the fire
        // path spawns it); outside one the hook must not block the live
        // path, so skip the spawn but keep the tail log as the record.
        if tokio::runtime::Handle::try_current().is_err() {
            crate::logging::info(&format!(
                "memory prefilter shadow re-judge skipped (no runtime) query_hash={query_hash} split={split_tag}"
            ));
            return;
        }
        let handle = tokio::spawn(async move {
            let accepted: usize = match crate::jev::JevClient::new() {
                Ok(client) => {
                    match crate::memory_jev::select(
                        &client,
                        &query,
                        tail,
                        crate::memory_jev::MAX_BATCH_ENTRIES,
                    )
                    .await
                    {
                        Ok(results) => results.len(),
                        Err(e) => {
                            crate::logging::info(&format!(
                                "memory prefilter shadow re-judge failed query_hash={query_hash} split={split_tag}: {e}"
                            ));
                            return;
                        }
                    }
                }
                Err(e) => {
                    crate::logging::info(&format!(
                        "memory prefilter shadow re-judge no client query_hash={query_hash} split={split_tag}: {e}"
                    ));
                    return;
                }
            };
            PREFILTER_TAIL_REJUDGED.fetch_add(1, Ordering::Relaxed);
            if accepted > 0 {
                PREFILTER_TAIL_HITS.fetch_add(1, Ordering::Relaxed);
            }
            crate::memory_log::log_prefilter_rejudge(
                &query_hash,
                &split_tag,
                accepted,
                Self::prefilter_shadow_rate(),
            );
        });
        drop(handle);
    }

    /// Record one tail re-judgment outcome (test/maintenance seam for the
    /// spawned hook above: same counters, same log line, no Jev call).
    /// `tail_accepts` is the judge-relevant tail count for one sampled
    /// engaged query; `tail_accepts > 0` marks a query-level tail hit.
    pub fn note_shadow_tail_rejudged(query_hash: &str, split_tag: &str, tail_accepts: usize) {
        use std::sync::atomic::Ordering;
        PREFILTER_TAIL_REJUDGED.fetch_add(1, Ordering::Relaxed);
        if tail_accepts > 0 {
            PREFILTER_TAIL_HITS.fetch_add(1, Ordering::Relaxed);
        }
        crate::memory_log::log_prefilter_rejudge(
            query_hash,
            split_tag,
            tail_accepts,
            Self::prefilter_shadow_rate(),
        );
    }

    /// Tail-hit window aggregation (SHADOW-GATE Metric 2 gate input).
    ///
    /// Query-level `tail_hit = tail_hits / engaged_sampled`: the fraction
    /// of re-judged sampled engaged queries with >= 1 judge-relevant tail
    /// memory. The denominator is the ENGAGED+SAMPLED joint counter
    /// (sampled-but-failopen queries have no tail by design); the
    /// numerator counts re-judged queries with `tail_accepts > 0`. Gate on
    /// test-split queries only (live traffic counts as test per §4.2
    /// unless c3 tunes on live-derived queries); the per-query split tag
    /// lives on both log lines for offline filtering.
    pub fn prefilter_tail_hit_snapshot() -> PrefilterTailHitSnapshot {
        use std::sync::atomic::Ordering;
        let rejudged = PREFILTER_TAIL_REJUDGED.load(Ordering::Relaxed);
        let hits = PREFILTER_TAIL_HITS.load(Ordering::Relaxed);
        let engaged_sampled = PREFILTER_ENGAGED_SAMPLED.load(Ordering::Relaxed);
        let tail_hit = if rejudged == 0 {
            0.0
        } else {
            hits as f64 / rejudged as f64
        };
        PrefilterTailHitSnapshot {
            engaged_sampled,
            rejudged,
            tail_hits: hits,
            tail_hit,
            tail_hit_upper_95: Self::wilson_upper_95(hits, rejudged),
        }
    }

    /// Wilson score upper bound (95%, z = 1.96) for a binomial rate.
    /// Pure math, no counters: `upper = (p + z^2/2n + z*sqrt(p(1-p)/n +
    /// z^2/4n^2)) / (1 + z^2/n)`. Returns 1.0 with no samples (no
    /// evidence: the bound is vacuous) and clamps to [0, 1].
    pub fn wilson_upper_95(hits: u64, total: u64) -> f64 {
        if total == 0 {
            return 1.0;
        }
        const Z: f64 = 1.96;
        const Z2: f64 = Z * Z;
        let n = total as f64;
        let p = hits as f64 / n;
        let denom = 1.0 + Z2 / n;
        let center = p + Z2 / (2.0 * n);
        let spread = Z * (p * (1.0 - p) / n + Z2 / (4.0 * n * n)).sqrt();
        ((center + spread) / denom).clamp(0.0, 1.0)
    }

    /// Pull pool, rank by dense and BM25 separately, fuse with RRF.
    ///
    /// Delegates to [`Self::hybrid_prefilter_rank`] after the legacy
    /// embedding-presence filter, so the live `find_similar_hybrid` API and
    /// the slot-B prefilter share ONE fusion implementation.
    fn hybrid_fuse(
        entries: Vec<MemoryEntry>,
        query_text: &str,
        query_embedding: &[f32],
        limit: usize,
    ) -> Vec<(MemoryEntry, f32)> {
        let entries: Vec<MemoryEntry> = entries
            .into_iter()
            .filter(|e| e.embedding.is_some())
            .collect();
        if entries.is_empty() {
            return Vec::new();
        }

        // Generous per-retriever pool so fusion has signal to work with.
        let pool = limit.saturating_mul(5).max(HYBRID_POOL_MIN);
        Self::hybrid_prefilter_rank(entries, query_text, query_embedding, limit, pool)
    }

    /// Slot-B hybrid ranking core: dense (cosine, active-model space only) +
    /// BM25 lexical, fused with RRF, over a CALLER-SUPPLIED entry set.
    ///
    /// Unlike [`Self::hybrid_fuse`] this does NOT re-collect and does NOT
    /// drop embedding-less entries: they stay reachable via the BM25 half
    /// (backend-switch contract, §3.2). Output ⊆ input by construction.
    /// `pool` is the per-retriever candidate depth (hybrid_fuse passes
    /// `(limit*5).max(50)`; the slot-B stage passes the same shape).
    ///
    /// Public so the recall bench measures the exact shipped ranking.
    pub fn hybrid_prefilter_rank(
        entries: Vec<MemoryEntry>,
        query_text: &str,
        query_embedding: &[f32],
        limit: usize,
        pool: usize,
    ) -> Vec<(MemoryEntry, f32)> {
        if entries.is_empty() || limit == 0 {
            return Vec::new();
        }

        // Dense ranking (no hard threshold; just take the top by cosine).
        // Vector-space gate: only entries embedded by the ACTIVE backend share a
        // comparable space, so dense scores are computed over those only. Other
        // entries (different model, e.g. not-yet-re-embedded local memories when
        // OpenAI is active) still participate via the BM25 lexical half below, so
        // they remain reachable rather than disappearing on a backend switch.
        let active_model = crate::embedding_backend::active_model_id();
        let dense_eligible: Vec<usize> = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.effective_embedding_model() == active_model)
            .map(|(i, _)| i)
            .collect();
        let emb_refs: Vec<&[f32]> = dense_eligible
            .iter()
            .filter_map(|&i| entries[i].embedding.as_deref())
            .collect();
        let dense_scores = crate::embedding::batch_cosine_similarity(query_embedding, &emb_refs);
        let mut dense: Vec<(usize, f32)> =
            dense_eligible.iter().copied().zip(dense_scores).collect();
        dense.sort_by(|a, b| {
            b.1.total_cmp(&a.1)
                .then_with(|| entries[a.0].id.cmp(&entries[b.0].id))
        });
        dense.truncate(pool);

        // Sparse (BM25) ranking over memory search text.
        let sparse = bm25_rank(&entries, query_text, pool);

        // RRF fusion. k comes from config (default 60.0); higher k
        // compresses rank gaps, lower k rewards top ranks more steeply.
        // The dense list is weighted by `memory_rrf_dense_weight`
        // (default 1.0 = equal weights); the sparse list keeps weight 1.0.
        //
        // C3 convex-combination port (default OFF): when
        // `memory_convex_alpha` is in (0, 1], fuse instead as
        // `alpha * dense_norm + (1 - alpha) * sparse_norm` where each leg
        // is min-max normalized over its own retrieved pool (missing =
        // 0.0, degenerate pool = 1.0 for retrieved docs — the c3 scaffold
        // semantics from exec/c3-fusion). The trio BM25 leg, the prefilter
        // pool, and the G-R recency position below are unchanged; only
        // the combination rule switches. Alpha 0.0 (the default) keeps
        // the exact RRF path below, bit-identical to pre-port.
        let rrf_k = Self::rrf_k();
        let w_dense = Self::rrf_dense_weight();
        let convex_alpha = Self::convex_alpha();
        let mut fused: std::collections::HashMap<usize, f32> = std::collections::HashMap::new();
        if convex_alpha > 0.0 {
            let dense_norm = minmax_normalize(&dense);
            let sparse_norm = minmax_normalize(&sparse);
            for (idx, v) in dense_norm {
                *fused.entry(idx).or_insert(0.0) += convex_alpha * v;
            }
            for (idx, v) in sparse_norm {
                *fused.entry(idx).or_insert(0.0) += (1.0 - convex_alpha) * v;
            }
        } else {
            for (rank, (idx, _)) in dense.iter().enumerate() {
                *fused.entry(*idx).or_insert(0.0) += w_dense / (rrf_k + rank as f32 + 1.0);
            }
            for (rank, (idx, _)) in sparse.iter().enumerate() {
                *fused.entry(*idx).or_insert(0.0) += 1.0 / (rrf_k + rank as f32 + 1.0);
            }
        }

        // G-R recency prior: bounded additive bonus per fused candidate.
        // Touch-aware age, global tau=90 default (0.0 = per-category table).
        // Applies only among live rows: tombstoned / superseded entries
        // never reach this pool (active-only collection upstream).
        if Self::recency_weight() > 0.0 {
            for (idx, score) in fused.iter_mut() {
                if let Some(e) = entries.get(*idx) {
                    *score += Self::recency_bonus(e);
                }
            }
        }

        let mut entries: Vec<Option<MemoryEntry>> = entries.into_iter().map(Some).collect();
        top_k_by_score(
            fused
                .into_iter()
                .filter_map(|(idx, score)| entries[idx].take().map(|e| (e, score))),
            limit,
            |entry| entry.id.as_str(),
        )
    }

    fn collect_all_memories_with_embeddings(&self) -> Result<Vec<MemoryEntry>> {
        self.collect_memories_with_embeddings_scoped(MemoryScope::All)
    }

    fn collect_memories_with_embeddings_scoped(
        &self,
        scope: MemoryScope,
    ) -> Result<Vec<MemoryEntry>> {
        let mut entries: Vec<MemoryEntry> = Vec::new();
        if scope.includes_project()
            && let Ok(project) = self.load_project_graph()
        {
            entries.extend(
                project
                    .active_memories()
                    .filter(|m| m.embedding.is_some())
                    .cloned(),
            );
        }
        if scope.includes_global()
            && let Ok(global) = self.load_global_graph()
        {
            entries.extend(
                global
                    .active_memories()
                    .filter(|m| m.embedding.is_some())
                    .cloned(),
            );
        }
        Ok(entries)
    }

    fn collect_memories_scoped(&self, scope: MemoryScope) -> Result<Vec<MemoryEntry>> {
        let mut entries = Vec::new();
        if scope.includes_project()
            && let Ok(project) = self.load_project_graph()
        {
            entries.extend(project.all_memories().cloned());
        }
        if scope.includes_global()
            && let Ok(global) = self.load_global_graph()
        {
            entries.extend(global.all_memories().cloned());
        }
        Ok(entries)
    }

    #[cfg(test)]
    fn synthetic_skill_entries(&self) -> Vec<MemoryEntry> {
        if !self.include_skills {
            return Vec::new();
        }

        collect_synthetic_entries()
    }

    #[cfg(test)]
    fn collect_retrieval_candidates_scoped(&self, scope: MemoryScope) -> Result<Vec<MemoryEntry>> {
        let mut entries = self.collect_memories_scoped(scope)?;
        if scope.includes_global() {
            entries.extend(self.synthetic_skill_entries());
        }
        Ok(entries)
    }

    fn score_and_filter(
        entries: Vec<MemoryEntry>,
        query_embedding: &[f32],
        query_text: &str,
        threshold: f32,
        limit: usize,
    ) -> Result<Vec<(MemoryEntry, f32)>> {
        if entries.is_empty() {
            return Ok(Vec::new());
        }

        let mut filtered_entries = Vec::with_capacity(entries.len());
        let mut skipped_missing_embeddings = 0usize;
        // Vector-space gate: only compare embeddings produced by the ACTIVE
        // backend (same model id). When the active backend differs from an
        // entry's stored model (e.g. user switched to OpenAI but this memory was
        // embedded with local MiniLM, not yet re-embedded), the cosine would be
        // meaningless, so we exclude it from dense scoring. Such memories remain
        // reachable via the lexical/BM25 path in hybrid retrieval.
        let active_model = crate::embedding_backend::active_model_id();
        let mut skipped_model_mismatch = 0usize;
        for entry in entries {
            if entry.embedding.is_none() {
                skipped_missing_embeddings += 1;
            } else if entry.effective_embedding_model() != active_model {
                skipped_model_mismatch += 1;
            } else {
                filtered_entries.push(entry);
            }
        }
        if skipped_missing_embeddings > 0 {
            crate::logging::warn(&format!(
                "Skipped {} retrieval candidate(s) without embeddings during similarity scoring",
                skipped_missing_embeddings
            ));
        }
        if skipped_model_mismatch > 0 {
            crate::logging::info(&format!(
                "Skipped {} retrieval candidate(s) embedded with a different model than the active backend ({})",
                skipped_model_mismatch, active_model
            ));
        }
        if filtered_entries.is_empty() {
            return Ok(Vec::new());
        }
        let emb_refs: Vec<&[f32]> = filtered_entries
            .iter()
            .filter_map(|entry| entry.embedding.as_deref())
            .collect();
        let scores = crate::embedding::batch_cosine_similarity(query_embedding, &emb_refs);
        let skill_query_terms = collect_skill_query_terms(query_text);

        let scored = top_k_by_score(
            filtered_entries
                .into_iter()
                .zip(scores)
                .map(|(entry, sim)| {
                    let adjusted = sim + skill_retrieval_bonus(&entry, &skill_query_terms);
                    (entry, adjusted)
                })
                .filter(|(_, sim)| *sim >= threshold),
            limit,
            |entry| entry.id.as_str(),
        );

        let scored = Self::apply_gap_filter(scored);

        Ok(scored)
    }

    /// Drop trailing low-relevance results by detecting natural gaps in the
    /// score distribution. If the top hit is 0.85 and the next cluster is
    /// 0.40-0.42, the 0.15+ gap tells us those lower results are noise.
    ///
    /// Algorithm: walk the sorted scores and cut when the drop from one score
    /// to the next exceeds `GAP_FACTOR` of the range (top - floor_threshold).
    fn apply_gap_filter(scored: Vec<(MemoryEntry, f32)>) -> Vec<(MemoryEntry, f32)> {
        if scored.len() <= 1 {
            return scored;
        }

        const GAP_FACTOR: f32 = 0.25;
        const MIN_KEEP: usize = 1;

        let top_score = scored[0].1;
        let range = (top_score - EMBEDDING_SIMILARITY_THRESHOLD).max(0.01);
        let max_gap = range * GAP_FACTOR;

        let mut keep = scored.len();
        for i in 1..scored.len() {
            let drop = scored[i - 1].1 - scored[i].1;
            if drop > max_gap && i >= MIN_KEEP {
                keep = i;
                break;
            }
        }

        scored.into_iter().take(keep).collect()
    }

    /// Ensure all memories have embeddings (backfill for existing memories)
    pub fn backfill_embeddings(&self) -> Result<(usize, usize)> {
        let mut generated = 0;
        let mut failed = 0;

        // Process project memories
        if let Ok(mut graph) = self.load_project_graph() {
            let mut changed = false;
            for entry in graph.memories.values_mut() {
                if entry.embedding.is_none() {
                    if entry.ensure_embedding() {
                        generated += 1;
                        changed = true;
                    } else {
                        failed += 1;
                    }
                }
            }
            if changed {
                self.save_project_graph(&graph)?;
            }
        }

        // Process global memories
        if let Ok(mut graph) = self.load_global_graph() {
            let mut changed = false;
            for entry in graph.memories.values_mut() {
                if entry.embedding.is_none() {
                    if entry.ensure_embedding() {
                        generated += 1;
                        changed = true;
                    } else {
                        failed += 1;
                    }
                }
            }
            if changed {
                self.save_global_graph(&graph)?;
            }
        }

        Ok((generated, failed))
    }

    pub fn get_prompt_memories(&self, limit: usize) -> Option<String> {
        self.get_prompt_memories_scoped(limit, MemoryScope::All)
    }

    pub fn get_prompt_memories_scoped(&self, limit: usize, scope: MemoryScope) -> Option<String> {
        let all_entries: Vec<_> = top_k_by_ord(
            self.collect_memories_scoped(scope)
                .ok()?
                .into_iter()
                .map(|entry| {
                    let updated_at = entry.updated_at.timestamp_millis();
                    (entry, updated_at)
                }),
            limit,
        )
        .into_iter()
        .map(|(entry, _)| entry)
        .collect();

        if all_entries.is_empty() {
            return None;
        }

        format_entries_for_prompt(&all_entries, limit)
    }

    pub async fn relevant_prompt_for_messages(
        &self,
        messages: &[crate::message::Message],
    ) -> Result<Option<String>> {
        let context = format_context_for_relevance(messages);
        if context.is_empty() {
            return Ok(None);
        }
        self.relevant_prompt_for_context(
            &context,
            MEMORY_RELEVANCE_MAX_CANDIDATES,
            MEMORY_RELEVANCE_MAX_RESULTS,
        )
        .await
    }

    pub async fn relevant_prompt_for_context(
        &self,
        context: &str,
        max_candidates: usize,
        limit: usize,
    ) -> Result<Option<String>> {
        let relevant = self
            .get_relevant_for_context(context, max_candidates)
            .await?;
        if relevant.is_empty() {
            return Ok(None);
        }
        Ok(format_relevant_prompt(&relevant, limit))
    }

    pub fn search(&self, query: &str) -> Result<Vec<MemoryEntry>> {
        self.search_scoped(query, MemoryScope::All)
    }

    pub fn search_scoped(&self, query: &str, scope: MemoryScope) -> Result<Vec<MemoryEntry>> {
        let query_lower = normalize_search_text(query);
        if query_lower.is_empty() {
            return Ok(Vec::new());
        }

        let mut results = Vec::new();

        for memory in self.collect_memories_scoped(scope)? {
            // Tombstoned (inactive) memories are invisible to retrieval (R3).
            if !memory.active {
                continue;
            }
            if memory_matches_search(&memory, &query_lower) {
                results.push(memory);
            }
        }

        Ok(results)
    }

    pub fn list_all(&self) -> Result<Vec<MemoryEntry>> {
        self.list_all_scoped(MemoryScope::All)
    }

    pub fn list_all_scoped(&self, scope: MemoryScope) -> Result<Vec<MemoryEntry>> {
        let mut all = self.collect_memories_scoped(scope)?;
        all.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(all)
    }

    /// Forget a memory: tombstone by default (R3 DELETE policy).
    ///
    /// Tombstoning preserves the row with `active=false, superseded_by=None`
    /// for provenance; every retrieval path filters on `active`, so the fact
    /// becomes invisible while its history survives. Pass `privacy=true` only
    /// for erasure the user asked to be unrecoverable — the row is hard-removed
    /// exactly as `forget` behaved before.
    pub fn forget(&self, id: &str) -> Result<bool> {
        self.forget_with_policy(id, false)
    }

    pub fn forget_with_policy(&self, id: &str, privacy: bool) -> Result<bool> {
        if privacy {
            return self.hard_forget(id);
        }
        // Tombstone first in project, then global scope (same order as the
        // old hard removal). No Invalidates edge here: a forget carries no
        // invalidator memory, and inventing one would be a phantom node.
        let mut project_graph = self.load_project_graph()?;
        if let Some(tombstone) = project_graph.get_memory_mut(id) {
            tombstone.active = false;
            tombstone.superseded_by = None;
            self.save_project_graph(&project_graph)?;
            return Ok(true);
        }

        let mut global_graph = self.load_global_graph()?;
        if let Some(tombstone) = global_graph.get_memory_mut(id) {
            tombstone.active = false;
            tombstone.superseded_by = None;
            self.save_global_graph(&global_graph)?;
            return Ok(true);
        }

        Ok(false)
    }

    /// Privacy-flagged erasure: hard row removal, no tombstone, no edge.
    fn hard_forget(&self, id: &str) -> Result<bool> {
        // Try graph-based removal first (new format)
        let mut project_graph = self.load_project_graph()?;
        if project_graph.remove_memory(id).is_some() {
            self.save_project_graph(&project_graph)?;
            return Ok(true);
        }

        let mut global_graph = self.load_global_graph()?;
        if global_graph.remove_memory(id).is_some() {
            self.save_global_graph(&global_graph)?;
            return Ok(true);
        }

        Ok(false)
    }

    // === Sidecar Integration ===

    /// Extract memories from a session transcript using the Haiku sidecar
    pub async fn extract_from_transcript(
        &self,
        transcript: &str,
        session_id: &str,
    ) -> Result<Vec<String>> {
        if !memory_llm_judge_available() {
            crate::logging::info("Memory transcript extraction skipped: LLM judge unavailable");
            return Ok(Vec::new());
        }

        let sidecar = Sidecar::new();
        let extracted = sidecar.extract_memories(transcript).await?;

        let mut ids = Vec::new();
        for memory in extracted {
            let category: MemoryCategory = memory.category.parse().unwrap_or(MemoryCategory::Fact);
            let trust = match memory.trust.as_str() {
                "high" => TrustLevel::High,
                "medium" => TrustLevel::Medium,
                _ => TrustLevel::Low,
            };

            let entry = MemoryEntry::new(category, memory.content)
                .with_source(session_id)
                .with_trust(trust);

            // Store in project scope by default
            let id = self.remember_project(entry)?;
            ids.push(id);
        }

        Ok(ids)
    }

    /// Recall directly through Jev. The legacy `max_candidates` argument now
    /// limits output, not the input pool: old memories must remain discoverable.
    pub async fn get_relevant_for_context(
        &self,
        context: &str,
        max_candidates: usize,
    ) -> Result<Vec<MemoryEntry>> {
        Ok(
            crate::memory_jev::recall(self, context, max_candidates, MemoryScope::All)
                .await?
                .into_iter()
                .map(|(entry, _)| entry)
                .collect(),
        )
    }

    /// Local keyword lookup, available without a remote decision provider.
    pub fn get_relevant_keywords(
        &self,
        keywords: &[&str],
        limit: usize,
    ) -> Result<Vec<MemoryEntry>> {
        let normalized_keywords: Vec<String> = keywords
            .iter()
            .map(|keyword| normalize_search_text(keyword))
            .filter(|keyword| !keyword.is_empty())
            .collect();
        if normalized_keywords.is_empty() {
            return Ok(Vec::new());
        }

        let matches: Vec<_> = top_k_by_ord(
            self.collect_memories_scoped(MemoryScope::All)?
                .into_iter()
                // Tombstoned (inactive) memories are invisible to retrieval (R3).
                .filter(|entry| entry.active)
                .filter(|entry| {
                    let content_lower = normalize_search_text(&entry.content);
                    normalized_keywords
                        .iter()
                        .any(|kw| content_lower.contains(kw))
                })
                .map(|entry| {
                    let updated_at = entry.updated_at.timestamp_millis();
                    (entry, updated_at)
                }),
            limit,
        )
        .into_iter()
        .map(|(entry, _)| entry)
        .collect();

        Ok(matches)
    }

    // === Async Memory Checking ===

    /// Spawn a background task to check memory relevance for a specific session.
    /// Results are stored in PENDING_MEMORY keyed by session_id and can be retrieved
    /// with take_pending_memory(session_id).
    /// This method returns immediately and never blocks the caller.
    /// Only ONE memory check runs at a time per session - additional calls are ignored.
    pub fn spawn_relevance_check(
        &self,
        session_id: &str,
        messages: std::sync::Arc<[crate::message::Message]>,
        event_tx: Option<MemoryEventSink>,
    ) {
        let sid = session_id.to_string();

        if !begin_memory_check(&sid) {
            return;
        }

        let manager = self.clone();

        tokio::spawn(async move {
            match manager
                .get_relevant_parallel(&sid, &messages, event_tx.clone())
                .await
            {
                Ok(MemoryRelevanceResult {
                    prompt: Some(prompt),
                    display_prompt,
                    selected_entries,
                }) => {
                    let count = selected_entries.len();
                    set_pending_memory_for_project_with_selection(
                        &sid,
                        prompt,
                        count,
                        &selected_entries,
                        display_prompt,
                        manager
                            .project_dir
                            .as_deref()
                            .and_then(|path| path.to_str()),
                    );
                    emit_memory_activity(event_tx.as_ref());
                }
                Ok(MemoryRelevanceResult { prompt: None, .. }) => {
                    clear_pending_memory(&sid);
                    set_state(MemoryState::Idle);
                    emit_memory_activity(event_tx.as_ref());
                }
                Err(e) => {
                    clear_pending_memory(&sid);
                    crate::logging::error(&format!("Background memory check failed: {}", e));
                    add_event(MemoryEventKind::Error {
                        message: e.to_string(),
                    });
                    set_state(MemoryState::Idle);
                    emit_memory_activity(event_tx.as_ref());
                }
            }

            finish_memory_check(&sid);
        });
    }

    /// Jev-only automatic recall. Storage and per-session dedup remain local;
    /// there is no embedding, conventional LLM, or unjudged fallback path.
    pub async fn get_relevant_parallel(
        &self,
        session_id: &str,
        messages: &[crate::message::Message],
        event_tx: Option<MemoryEventSink>,
    ) -> Result<MemoryRelevanceResult> {
        let query = format_focused_query_for_relevance(messages);
        let query = crate::util::truncate_str(&query, crate::memory_jev::MAX_QUERY_BYTES);
        if query.trim().is_empty() {
            return Ok(MemoryRelevanceResult::default());
        }
        pipeline_start();
        let entries = match crate::memory_jev::collect_scoped(self, MemoryScope::All) {
            Ok(entries) => entries,
            Err(error) => {
                clear_pending_memory(session_id);
                pipeline_update(|p| {
                    p.search = StepStatus::Error;
                    p.verify = StepStatus::Skipped;
                    p.inject = StepStatus::Skipped;
                });
                set_state(MemoryState::Idle);
                emit_memory_activity(event_tx.as_ref());
                return Err(error);
            }
        };
        let entries: Vec<_> = entries
            .into_iter()
            .filter(|entry| entry.active && !is_memory_injected(session_id, &entry.id))
            .collect();
        // Slot-B R3 composition (rebase onto upstream 72-cap): our
        // Option-take stage runs when engaged; upstream's BM25 top-72
        // `prefilter_for_jev` is the disengaged/fail-open floor (kept
        // verbatim for next-rebase merging). The two stages never act on
        // the same query (no double-narrow); the floor only changes
        // behavior where raw exhaustive is dangerous (large stores).
        let mut entries_opt = Some(entries);
        Self::prefilter_for_jev_take(&mut entries_opt, &query);
        let entries = entries_opt.expect("prefilter stage always leaves a set");
        // Floor queries (mode-off / fail-open) arrive here as the raw set;
        // cap them at 72 via the upstream floor fn so the disengaged path
        // never reproduces the 107-call outage.
        let entries = prefilter_for_jev(entries, &query);
        pipeline_update(|p| {
            p.search = StepStatus::Done;
            p.search_result = Some(StepResult {
                summary: format!("{} local memories", entries.len()),
                latency_ms: 0,
            });
            p.verify = StepStatus::Running;
            p.maintain = StepStatus::Skipped;
        });
        set_state(MemoryState::SidecarChecking {
            count: entries.len(),
        });
        emit_memory_activity(event_tx.as_ref());
        let started = Instant::now();
        // Slot-B hybrid prefilter (§3): narrows the ALREADY-COLLECTED set
        // (post-`entry.active`, post-already-injected) to prefilter_top_k
        // before Jev judges it. `entries` is Option-wrapped so the stage can
        // fail open pre-rank WITHOUT consuming the input: None from the
        // stage means disengaged/failed-open (take the input back and judge
        // exhaustively); Some carries the narrowed-or-recombined set.
        // Downstream fail-closed (JevClient::new Err, select Err) is
        // bit-for-bit unchanged below.
        let mut entries_opt = Some(entries);
        let prefilter_dropped =
            match Self::prefilter_for_jev_take(&mut entries_opt, &query) {
                Some(dropped) => dropped,
                None => Vec::new(),
            };
        let entries = entries_opt.take().expect("prefilter stage always leaves a set");
        let _ = prefilter_dropped;
        let result = async {
            if entries.is_empty() {
                return Ok(Vec::new());
            }
            let client = crate::jev::JevClient::new()?;
            crate::memory_jev::select(&client, query, entries, 5).await
        }
        .await;
        let relevant: Vec<MemoryEntry> = match result {
            Ok(results) => results.into_iter().map(|(entry, _)| entry).collect(),
            Err(error) => {
                clear_pending_memory(session_id);
                pipeline_update(|p| {
                    p.verify = StepStatus::Error;
                    p.inject = StepStatus::Skipped;
                });
                set_state(MemoryState::Idle);
                emit_memory_activity(event_tx.as_ref());
                return Err(error);
            }
        };
        let count = relevant.len();
        pipeline_update(|p| {
            p.verify = StepStatus::Done;
            p.verify_result = Some(StepResult {
                summary: format!("Jev: {count} relevant"),
                latency_ms: started.elapsed().as_millis() as u64,
            });
            p.inject = if count == 0 {
                StepStatus::Skipped
            } else {
                StepStatus::Pending
            };
        });
        // Citation verification rides here (not in the agent): the Jev
        // refactor builds the prompt inside the manager, so the repo root
        // comes from the manager's own project dir. Unverifiable entries
        // (no root, legacy) fall back to the unverified prompt — recall
        // never withholds a memory.
        let prompt = match self.get_project_dir() {
            Some(root) => format_relevant_prompt_verified(&relevant, 5, &root)
                .or_else(|| format_relevant_prompt(&relevant, 5)),
            None => format_relevant_prompt(&relevant, 5),
        };
        let display = format_relevant_display_prompt(&relevant, 5);
        set_state(if count == 0 {
            MemoryState::Idle
        } else {
            MemoryState::FoundRelevant { count }
        });
        emit_memory_activity(event_tx.as_ref());
        Ok(MemoryRelevanceResult {
            prompt,
            display_prompt: display,
            selected_entries: relevant,
        })
    }

    /// Load the existing project graph without generating embeddings.
    pub fn load_project_graph(&self) -> Result<MemoryGraph> {
        let Some(path) = self.project_memory_path()? else {
            return Ok(MemoryGraph::new());
        };

        if !self.test_mode
            && let Some(mut graph) = cached_graph(&path)
        {
            if Self::normalize_graph_search_text(&mut graph) {
                cache_graph(path.clone(), &graph);
            }
            return Ok(graph);
        }

        if path.exists() {
            // Try loading as MemoryGraph first
            if let Ok(graph) = storage::read_json::<MemoryGraph>(&path)
                && graph.graph_version == GRAPH_VERSION
            {
                let mut graph = graph;
                let normalized = Self::normalize_graph_search_text(&mut graph);
                if self.import_legacy_notes_into_graph(&mut graph)? {
                    self.save_project_graph(&graph)?;
                } else if normalized {
                    storage::write_json(&path, &graph)?;
                }
                if !self.test_mode {
                    cache_graph(path, &graph);
                }
                return Ok(graph);
            }

            // Fall back to legacy MemoryStore and migrate
            let store: MemoryStore = storage::read_json(&path)?;
            let mut graph = MemoryGraph::from_legacy_store(store);
            let _ = self.import_legacy_notes_into_graph(&mut graph)?;

            // Save migrated format (create backup first)
            let backup_path = path.with_extension("json.bak");
            if !backup_path.exists() {
                let _ = std::fs::copy(&path, &backup_path);
            }
            storage::write_json(&path, &graph)?;

            crate::logging::info(&format!(
                "Migrated memory store to graph format: {}",
                path.display()
            ));
            if !self.test_mode {
                cache_graph(path, &graph);
            }
            Ok(graph)
        } else {
            let mut graph = MemoryGraph::new();
            if self.import_legacy_notes_into_graph(&mut graph)? {
                self.save_project_graph(&graph)?;
            }
            if !self.test_mode {
                cache_graph(path, &graph);
            }
            Ok(graph)
        }
    }

    /// Load global memories as a MemoryGraph with automatic migration
    pub fn load_global_graph(&self) -> Result<MemoryGraph> {
        let path = self.global_memory_path()?;
        if !self.test_mode
            && let Some(mut graph) = cached_graph(&path)
        {
            if Self::normalize_graph_search_text(&mut graph) {
                cache_graph(path.clone(), &graph);
            }
            return Ok(graph);
        }

        if path.exists() {
            // Try loading as MemoryGraph first
            if let Ok(graph) = storage::read_json::<MemoryGraph>(&path)
                && graph.graph_version == GRAPH_VERSION
            {
                let mut graph = graph;
                if Self::normalize_graph_search_text(&mut graph) {
                    storage::write_json(&path, &graph)?;
                }
                if !self.test_mode {
                    cache_graph(path, &graph);
                }
                return Ok(graph);
            }

            // Fall back to legacy MemoryStore and migrate
            let store: MemoryStore = storage::read_json(&path)?;
            let graph = MemoryGraph::from_legacy_store(store);

            // Save migrated format (create backup first)
            let backup_path = path.with_extension("json.bak");
            if !backup_path.exists() {
                let _ = std::fs::copy(&path, &backup_path);
            }
            storage::write_json(&path, &graph)?;

            crate::logging::info(&format!(
                "Migrated global memory store to graph format: {}",
                path.display()
            ));
            if !self.test_mode {
                cache_graph(path, &graph);
            }
            Ok(graph)
        } else {
            let graph = MemoryGraph::new();
            if !self.test_mode {
                cache_graph(path, &graph);
            }
            Ok(graph)
        }
    }

    /// Save project memories as a MemoryGraph
    pub fn save_project_graph(&self, graph: &MemoryGraph) -> Result<()> {
        if let Some(path) = self.project_memory_path()? {
            storage::write_json(&path, graph)?;
            if !self.test_mode {
                cache_graph(path, graph);
            }
        }
        Ok(())
    }

    /// Save global memories as a MemoryGraph
    pub fn save_global_graph(&self, graph: &MemoryGraph) -> Result<()> {
        let path = self.global_memory_path()?;
        storage::write_json(&path, graph)?;
        if !self.test_mode {
            cache_graph(path, graph);
        }
        Ok(())
    }

    /// Add a tag to a memory
    pub fn tag_memory(&self, memory_id: &str, tag: &str) -> Result<()> {
        // Try project first
        let mut graph = self.load_project_graph()?;
        if graph.memories.contains_key(memory_id) {
            graph.tag_memory(memory_id, tag);
            return self.save_project_graph(&graph);
        }

        // Try global
        let mut graph = self.load_global_graph()?;
        if graph.memories.contains_key(memory_id) {
            graph.tag_memory(memory_id, tag);
            return self.save_global_graph(&graph);
        }

        Err(anyhow::anyhow!("Memory not found: {}", memory_id))
    }

    /// Link two memories with a RelatesTo edge
    pub fn link_memories(&self, from_id: &str, to_id: &str, weight: f32) -> Result<()> {
        // Try project first
        let mut graph = self.load_project_graph()?;
        if graph.memories.contains_key(from_id) && graph.memories.contains_key(to_id) {
            graph.link_memories(from_id, to_id, weight);
            return self.save_project_graph(&graph);
        }

        // Try global
        let mut graph = self.load_global_graph()?;
        if graph.memories.contains_key(from_id) && graph.memories.contains_key(to_id) {
            graph.link_memories(from_id, to_id, weight);
            return self.save_global_graph(&graph);
        }

        // Cross-store links not supported for now
        Err(anyhow::anyhow!(
            "Both memories must be in the same store (project or global)"
        ))
    }

    /// Get memories related to a given memory via graph traversal
    pub fn get_related(&self, memory_id: &str, depth: usize) -> Result<Vec<MemoryEntry>> {
        // Find which store contains the memory
        let (mut graph, _is_project) = {
            let project_graph = self.load_project_graph()?;
            if project_graph.memories.contains_key(memory_id) {
                (project_graph, true)
            } else {
                let global_graph = self.load_global_graph()?;
                if global_graph.memories.contains_key(memory_id) {
                    (global_graph, false)
                } else {
                    return Err(anyhow::anyhow!("Memory not found: {}", memory_id));
                }
            }
        };

        // Use cascade retrieval to find related memories
        let results = graph.cascade_retrieve(&[memory_id.to_string()], &[1.0], depth, 20);

        // Collect memory entries (excluding the seed)
        let entries: Vec<MemoryEntry> = results
            .into_iter()
            .filter(|(id, _)| id != memory_id)
            .filter_map(|(id, _)| graph.get_memory(&id).cloned())
            .collect();

        Ok(entries)
    }

    /// Find similar memories with cascade retrieval through the graph
    ///
    /// This extends the basic embedding search by also traversing through
    /// tags to find related memories that might not have direct embedding similarity.
    pub fn find_similar_with_cascade(
        &self,
        text: &str,
        threshold: f32,
        limit: usize,
    ) -> Result<Vec<(MemoryEntry, f32)>> {
        self.find_similar_with_cascade_scoped(text, threshold, limit, MemoryScope::All)
    }

    pub fn find_similar_with_cascade_scoped(
        &self,
        text: &str,
        threshold: f32,
        limit: usize,
        scope: MemoryScope,
    ) -> Result<Vec<(MemoryEntry, f32)>> {
        // First, do basic embedding search
        let embedding_hits = self.find_similar_scoped(text, threshold, limit, scope)?;

        if embedding_hits.is_empty() {
            return Ok(Vec::new());
        }

        // Get seed IDs and scores
        let seed_ids: Vec<String> = embedding_hits.iter().map(|(e, _)| e.id.clone()).collect();
        let seed_scores: Vec<f32> = embedding_hits.iter().map(|(_, s)| *s).collect();

        // Load graphs and perform cascade retrieval
        let mut project_graph = if scope.includes_project() {
            Some(self.load_project_graph()?)
        } else {
            None
        };
        let mut global_graph = if scope.includes_global() {
            Some(self.load_global_graph()?)
        } else {
            None
        };

        // Cascade through project graph
        let project_cascade = project_graph
            .as_mut()
            .map(|graph| graph.cascade_retrieve(&seed_ids, &seed_scores, 2, limit * 2))
            .unwrap_or_default();

        // Cascade through global graph
        let global_cascade = global_graph
            .as_mut()
            .map(|graph| graph.cascade_retrieve(&seed_ids, &seed_scores, 2, limit * 2))
            .unwrap_or_default();

        // Merge results, keeping highest score for each memory
        let mut merged: std::collections::HashMap<String, f32> = std::collections::HashMap::new();

        for (id, score) in embedding_hits.iter() {
            merged.insert(id.id.clone(), *score);
        }
        for (id, score) in project_cascade {
            let existing = merged.get(&id).copied().unwrap_or(0.0);
            if score > existing {
                merged.insert(id, score);
            }
        }
        for (id, score) in global_cascade {
            let existing = merged.get(&id).copied().unwrap_or(0.0);
            if score > existing {
                merged.insert(id, score);
            }
        }

        // Look up entries and keep only the top-scoring results
        let results: Vec<(MemoryEntry, f32)> = top_k_by_score(
            merged.into_iter().filter_map(|(id, score)| {
                project_graph
                    .as_ref()
                    .and_then(|graph| graph.get_memory(&id))
                    .or_else(|| {
                        global_graph
                            .as_ref()
                            .and_then(|graph| graph.get_memory(&id))
                    })
                    .cloned()
                    .map(|entry| (entry, score))
            }),
            limit,
            |entry| entry.id.as_str(),
        );

        Ok(results)
    }

    /// Get graph statistics for display
    pub fn graph_stats(&self) -> Result<(usize, usize, usize, usize)> {
        let project = self.load_project_graph()?;
        let global = self.load_global_graph()?;

        let memories = project.memories.len() + global.memories.len();
        let tags = project.tags.len() + global.tags.len();
        let edges = project.edge_count() + global.edge_count();
        let clusters = project.clusters.len() + global.clusters.len();

        Ok((memories, tags, edges, clusters))
    }
}


/// Slot-B shadow-audit counters: (queries, engaged, failopen, sampled).
/// Process-wide atomics; the 7-day live window reads them via
/// `MemoryManager::prefilter_shadow_stats`.
static PREFILTER_QUERIES: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
static PREFILTER_ENGAGED: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
static PREFILTER_FAILOPEN: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
static PREFILTER_SHADOW_SAMPLED: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Slot-B per-arm fail-open counters (SHADOW-GATE Metric 4): the five arms
/// conflated in `PREFILTER_FAILOPEN`, indexed by `PrefilterFailopenArm`.
/// Every increment pairs with the coarse bump at the
/// `prefilter_for_jev_take` call site, so `failopen == sum(arms)`.
static PREFILTER_FAILOPEN_ARMS: [std::sync::atomic::AtomicU64; 5] = [
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
];

/// Slot-B engaged-AND-sampled joint counter (SHADOW-GATE Metric 2
/// denominator). Sampled-but-failopen queries have no tail by design, so
/// only queries whose tail is actually emitted bump this.
static PREFILTER_ENGAGED_SAMPLED: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Slot-B tail re-judge counters (SHADOW-GATE Metric 2 numerator): how
/// many sampled engaged tails finished a second `select` (`REJUDGED`),
/// and how many of those had >= 1 judge-relevant tail memory (`HITS`).
/// Process-wide, `Relaxed` (monotonic; exact cross-counter consistency
/// not required). The hook bumps these read-only: no recall counts or
/// priors are touched (none exist yet; keep it that way).
static PREFILTER_TAIL_REJUDGED: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
static PREFILTER_TAIL_HITS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Slot-B stage-elapsed accumulators (SHADOW-GATE Metric 3 live half):
/// embed half (query-embed wall clock) and rank half (hot rank pass).
static PREFILTER_STAGE_EMBED: PrefilterStageAccum = PrefilterStageAccum::new();
static PREFILTER_STAGE_RANK: PrefilterStageAccum = PrefilterStageAccum::new();

/// Embedding similarity threshold (0.0 - 1.0)
/// Lower = more candidates, higher = fewer but more relevant
pub const EMBEDDING_SIMILARITY_THRESHOLD: f32 = 0.5;

/// Maximum embedding hits to verify with sidecar
pub const EMBEDDING_MAX_HITS: usize = 10;

/// Minimum per-retriever candidate pool size for hybrid fusion.
const HYBRID_POOL_MIN: usize = 50;

/// Rank memories by BM25F-lite over content + tag fields.
///
/// Two-field BM25F following Robertson-Zaragoza-Taylor 2004: per-field
/// length norms fold into an additive field-weighted tf BEFORE saturation
/// (normalize-then-saturate, no second global dl factor). Shared b=0.75.
/// Tag terms reuse the same pool-local unigram IDF (lite simplification;
/// no per-field IDF).

/// Most memories Jcode sends to Jev per automatic recall. Every 24 entries costs
/// one Jev decision, so judging a whole store (thousands of memories) each turn
/// exhausted the daily plan allowance within hours. Lexical relevance narrows
/// the field first; Jev still judges every candidate that reaches the prompt.
/// (Upstream `00c1d655b`, kept verbatim for next-rebase merging; wired as the
/// disengaged/fail-open floor inside `prefilter_for_jev_take` per R3 XOR —
/// the engaged path runs our stage alone and never chains.)
pub(crate) const MAX_JEV_RECALL_CANDIDATES: usize = 72;

/// Keep the most lexically relevant memories for Jev judgement. Small stores
/// pass through unchanged. Memories with no term overlap are dropped only when
/// the store is larger than the candidate budget.
fn prefilter_for_jev(entries: Vec<MemoryEntry>, query: &str) -> Vec<MemoryEntry> {
    if entries.len() <= MAX_JEV_RECALL_CANDIDATES {
        return entries;
    }
    let ranked = bm25_rank(&entries, query, MAX_JEV_RECALL_CANDIDATES);
    let mut entries: Vec<Option<MemoryEntry>> = entries.into_iter().map(Some).collect();
    ranked
        .into_iter()
        .filter_map(|(idx, _)| entries[idx].take())
        .collect()
}

/// Per-query min-max normalization of one retriever leg over its retrieved
/// pool (C3 convex-combination port; c3 scaffold semantics from
/// exec/c3-fusion PLAN.md §4): `(s - min) / (max - min)` into [0, 1].
/// A doc absent from the leg contributes nothing (caller fuses 0.0 by
/// union-ing only retrieved idxs). Degenerate pools (empty, single-item,
/// or max == min) map every retrieved doc to 1.0 so a constant leg stays
/// neutral rather than zeroing its side of the convex sum. Non-finite
/// scores are treated as the pool minimum (defensive: BM25/cosine legs
/// are finite in practice, but a NaN must not poison normalization).
fn minmax_normalize(ranked: &[(usize, f32)]) -> Vec<(usize, f32)> {
    if ranked.is_empty() {
        return Vec::new();
    }
    // Range over finite scores only: a non-finite leg score maps to 0.0
    // (pool minimum) instead of stretching or poisoning the span.
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for (_, s) in ranked {
        if !s.is_finite() {
            continue;
        }
        if *s < lo {
            lo = *s;
        }
        if *s > hi {
            hi = *s;
        }
    }
    if !lo.is_finite() || !hi.is_finite() || (hi - lo) <= 0.0 {
        return ranked.iter().map(|(idx, _)| (*idx, 1.0)).collect();
    }
    let span = hi - lo;
    ranked
        .iter()
        .map(|(idx, s)| {
            let s = if s.is_finite() { *s } else { lo };
            (*idx, (s - lo) / span)
        })
        .collect()
}

fn bm25_rank(entries: &[MemoryEntry], query_text: &str, limit: usize) -> Vec<(usize, f32)> {
    const K1: f32 = 1.2;
    const B: f32 = 0.75;
    /// Tag-field term-frequency boost (c2a gate grid: {2, 4, 8}).
    const TAG_BOOST: f32 = 2.0;
    // c2c: additive bigram bonus weight (C3 G3-additive primary; union-model
    // stays diagnostic-only per c5 W3). 0.05 selected by the c2c sweep:
    // all of {0.05, 0.10, 0.15, 0.25} score identically on dev C1/C4 and
    // hold both blind sets, so the smallest weight ships (least veto risk
    // under two-list RRF per c5 X3).
    const BIGRAM_W: f32 = 0.05;

    // c2b: light plural folding via bm25_token_stream (query AND docs
    // symmetric). BM25-leg only — normalize_search_text callers elsewhere
    // are untouched.
    // Narrowing (attempt 1, reachability-preserving): doc-side folding
    // applies ONLY to docs that already engage the query in unfolded OR
    // folded space (support = doc unfolded tokens hit the unfolded or
    // folded query set). A doc with no support falls back to pristine
    // (unfolded tokens vs unfolded query) and can never gain a
    // fold-conjured match. Consequence: the BM25-leg scoring set is a
    // SUBSET of the pristine scoring set — folding reweights reached docs
    // but reaches no new doc. This blocks the observed 04 harm (a
    // `codes->code` fold conjuring a BM25 score for a doc the unfolded
    // query never reached) while keeping every dev-W1 rescue (each W1
    // gold already shares an unfolded term with its query).
    let q_unfolded: Vec<String> = normalize_search_text(query_text)
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();
    let q_terms: Vec<String> = bm25_token_stream(query_text);
    if q_terms.is_empty() {
        return Vec::new();
    }
    let q_set: std::collections::HashSet<&String> = q_terms.iter().collect();
    // Support vocabulary: unfolded + folded query forms. A doc token in
    // either space counts as engagement (so pristine-exact pairs always
    // confer support, and every pristine pair survives folding).
    let q_support: std::collections::HashSet<&str> = q_unfolded
        .iter()
        .map(|s| s.as_str())
        .chain(q_terms.iter().map(|s| s.as_str()))
        .collect();
    // c2c: query bigrams from pre-dedup order (adjacency available on the
    // Vec before the HashSet above). Repeated query bigrams count once
    // (set semantics, matches query-dedup).
    let q_bigrams: std::collections::HashSet<(&str, &str)> = q_terms
        .windows(2)
        .map(|w| (w[0].as_str(), w[1].as_str()))
        .collect();

    // c2a+c2b composed: per-field streams with c2b support-gating.
    // FieldDoc split (c2a): content/tags tokenized separately from raw
    // fields (same tokenizer the `search_text` cache was built with, so
    // the content+tag union matches the cached flat join exactly).
    // Support gate (c2b narrowing): a field folds ONLY when the doc
    // already engages the query in unfolded OR folded space; otherwise
    // that field falls back to pristine tokens and can never gain a
    // fold-conjured match. BM25-leg scoring set stays a SUBSET of the
    // pristine set.
    struct FieldDoc {
        content: Vec<String>,
        tags: Vec<String>,
    }
    let docs: Vec<FieldDoc> = entries
        .iter()
        .map(|e| {
            let content_unfolded: Vec<String> = normalize_search_text(&e.content)
                .split_whitespace()
                .map(|s| s.to_string())
                .collect();
            let tags_unfolded: Vec<String> = normalize_search_text(&e.tags.join(" "))
                .split_whitespace()
                .map(|s| s.to_string())
                .collect();
            let supported = content_unfolded
                .iter()
                .chain(tags_unfolded.iter())
                .any(|t| q_support.contains(t.as_str()));
            if supported {
                FieldDoc {
                    content: bm25_token_stream(&e.content),
                    tags: bm25_token_stream(&e.tags.join(" ")),
                }
            } else {
                FieldDoc {
                    content: content_unfolded,
                    tags: tags_unfolded,
                }
            }
        })
        .collect();

    let n = docs.len().max(1) as f32;
    let avglen_c = docs.iter().map(|d| d.content.len()).sum::<usize>() as f32 / n;
    let avglen_t = docs.iter().map(|d| d.tags.len()).sum::<usize>() as f32 / n;
    // Pool-local unigram df over the field union (a doc counts when the
    // term appears in EITHER field) — same basis as the flat-join df.
    let mut df: std::collections::HashMap<&str, f32> = std::collections::HashMap::new();
    for doc in &docs {
        let unique: std::collections::HashSet<&str> = doc
            .content
            .iter()
            .chain(doc.tags.iter())
            .map(|s| s.as_str())
            .collect();
        for t in unique {
            *df.entry(t).or_insert(0.0) += 1.0;
        }
    }

    let mut scored: Vec<(usize, f32)> = Vec::new();
    for (idx, doc) in docs.iter().enumerate() {
        if doc.content.is_empty() && doc.tags.is_empty() {
            continue;
        }
        let mut tf_c: std::collections::HashMap<&str, f32> = std::collections::HashMap::new();
        for t in &doc.content {
            *tf_c.entry(t.as_str()).or_insert(0.0) += 1.0;
        }
        let mut tf_t: std::collections::HashMap<&str, f32> = std::collections::HashMap::new();
        for t in &doc.tags {
            *tf_t.entry(t.as_str()).or_insert(0.0) += 1.0;
        }
        // Empty-field guards: no 0/0 when a pool is contentless/tagless.
        // (b_c is defined below with the c2a-guard norm floor.)
        let tag_field_live = avglen_t > 0.0;
        let b_t = if tag_field_live {
            1.0 - B + B * doc.tags.len() as f32 / avglen_t
        } else {
            1.0
        };
        let mut score = 0.0f32;
        // c2a-guard (corrected 2026-10-01): FieldDoc content-norm floor
        // for stopword-only overlap. Forensics (raw BM25-leg scores on
        // 04-rescore M-006) proved the keep-alive is content-side, not
        // tag-side: mh-M-004-S03-T01 matches `the` in CONTENT
        // (content=[replace,the,hallway,printer,toner], tags
        // query-disjoint), so a tag-only veto never fires (guarded
        // sparse-leg byte-identical to unguarded, all queries). The real
        // inversion is the content-norm split: the 5-token distractor
        // gets b=0.53 (tf 1.87) on `the` while the 15-token gold gets
        // b=1.10 (tf 0.91), and c2b folding discounts IDF(buddy)
        // 2.38->2.04 / IDF(code) 1.79->1.59, compressing the gold.
        // Two drop-rule variants were tried and REJECTED (both measured):
        // per-term stopword veto = inert (zero query-term tag overlap
        // corpus-wide); per-doc stopword-only veto = backfires (drops
        // the boundary gold itself, whose M-006 overlap is also
        // stopword-only — recall falls OUT of top-10). The floor instead
        // caps the norm AMPLIFICATION a stopword-only doc can draw: when
        // the doc has no unfolded non-stopword overlap with the query,
        // its content length norm b_c is floored at 1.0 (no sub-unity
        // boost; tf can only saturate DOWN toward the gold, never above
        // it on length grounds). Docs WITH unfolded topical overlap
        // (every gold and every c2b rescue on content-bearing queries)
        // score EXACTLY as before. Pristine reachability preserved: the
        // floor only lowers score, never adds reach (scoring-set subset
        // holds; the c2b support gate is untouched).
        let doc_has_unfolded_topical_overlap = q_unfolded.iter().any(|t| {
            !jcode_session_types::is_session_search_stop_word(t.as_str())
                && (tf_c.contains_key(t.as_str()) || tf_t.contains_key(t.as_str()))
        });
        let b_c = if avglen_c > 0.0 {
            let b = 1.0 - B + B * doc.content.len() as f32 / avglen_c;
            if !doc_has_unfolded_topical_overlap {
                b.max(1.0)
            } else {
                b
            }
        } else {
            1.0
        };
        for term in &q_set {
            let f_c = tf_c.get(term.as_str()).copied().unwrap_or(0.0);
            let f_t = if tag_field_live {
                tf_t.get(term.as_str()).copied().unwrap_or(0.0)
            } else {
                0.0
            };
            if f_c == 0.0 && f_t == 0.0 {
                continue;
            }
            let n_q = *df.get(term.as_str()).unwrap_or(&0.0);
            if n_q == 0.0 {
                continue;
            }
            let idf = (((n - n_q + 0.5) / (n_q + 0.5)) + 1.0).ln();
            let tf_tilde = f_c / b_c + TAG_BOOST * f_t / b_t;
            score += idf * (tf_tilde * (K1 + 1.0)) / (tf_tilde + K1);
        }
        // c2c: additive bigram bonus `w*(idf_a + idf_b)` per shared bigram,
        // scored on UNIGRAM IDFs (reuse df map — no bigram-IDF term, so no
        // high-IDF noise on accidental bigrams per c5 W3). Bigrams run over
        // the content+tag UNION stream (c2a composed: adjacency across the
        // field boundary is ignored — content-internal and tag-internal
        // pairs only; cross-boundary pairs would be spurious). A shared
        // bigram implies both unigrams shared, so no drop-rule interaction:
        // docs with no shared bigram get +0. Stopword-carrying bigrams
        // self-discount via the unigram-IDF sum (no stoplist).
        // Streams are the support-gated folded streams (c2b composed).
        if !q_bigrams.is_empty() {
            let idf_of = |t: &str| {
                let n_q = *df.get(t).unwrap_or(&0.0);
                if n_q == 0.0 {
                    0.0
                } else {
                    (((n - n_q + 0.5) / (n_q + 0.5)) + 1.0).ln()
                }
            };
            let mut d_bigrams: std::collections::HashSet<(&str, &str)> =
                std::collections::HashSet::new();
            for stream in [&doc.content, &doc.tags] {
                if stream.len() >= 2 {
                    d_bigrams.extend(
                        stream
                            .windows(2)
                            .map(|w| (w[0].as_str(), w[1].as_str())),
                    );
                }
            }
            for (a, b) in q_bigrams.intersection(&d_bigrams) {
                score += BIGRAM_W * (idf_of(a) + idf_of(b));
            }
        }
        if score > 0.0 {
            scored.push((idx, score));
        }
    }
    scored.sort_by(|a, b| {
        b.1.total_cmp(&a.1)
            .then_with(|| entries[a.0].id.cmp(&entries[b.0].id))
    });
    scored.truncate(limit);
    scored
}

impl Default for MemoryManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "memory_tests.rs"]
mod tests;
