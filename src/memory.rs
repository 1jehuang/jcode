//! Memory system for cross-session learning
//!
//! Provides persistent memory that survives across sessions, organized by:
//! - Project (per working directory)
//! - Global (user-level preferences)
//!
//! Integrates with the Haiku sidecar for relevance verification and extraction.

use crate::memory_graph::{GRAPH_VERSION, MemoryGraph};
use crate::sidecar::Sidecar;
use crate::storage;
use crate::tui::info_widget::{
    InjectedMemoryItem, MemoryActivity, MemoryEvent, MemoryEventKind, MemoryState, StepResult,
    StepStatus,
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Instant, SystemTime};

#[path = "memory/activity.rs"]
mod activity;
#[path = "memory/pending.rs"]
mod pending;
#[path = "memory_prompt.rs"]
mod prompt_support;

pub use activity::{
    activity_snapshot, add_event, apply_remote_activity_snapshot, check_staleness, clear_activity,
    get_activity, pipeline_start, pipeline_update, record_injected_prompt, set_state,
};
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
use prompt_support::format_entries_for_prompt;
pub(crate) use prompt_support::{
    format_context_for_extraction, format_context_for_relevance, format_relevant_display_prompt,
    format_relevant_prompt,
};

const LEGACY_NOTE_CATEGORY: &str = "note";

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryScope {
    Project,
    Global,
    All,
}

impl MemoryScope {
    fn includes_project(self) -> bool {
        matches!(self, Self::Project | Self::All)
    }

    fn includes_global(self) -> bool {
        matches!(self, Self::Global | Self::All)
    }
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

// === Graph Cache ===

struct GraphCacheEntry {
    graph: MemoryGraph,
    modified: Option<SystemTime>,
}

struct GraphCache {
    entries: HashMap<PathBuf, GraphCacheEntry>,
}

impl GraphCache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }
}

static GRAPH_CACHE: OnceLock<Mutex<GraphCache>> = OnceLock::new();

fn graph_cache() -> &'static Mutex<GraphCache> {
    GRAPH_CACHE.get_or_init(|| Mutex::new(GraphCache::new()))
}

fn graph_mtime(path: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

fn cached_graph(path: &PathBuf) -> Option<MemoryGraph> {
    let modified = graph_mtime(path);
    let cache = graph_cache().lock().ok()?;
    let entry = cache.entries.get(path)?;
    if entry.modified == modified {
        Some(entry.graph.clone())
    } else {
        None
    }
}

fn cache_graph(path: PathBuf, graph: &MemoryGraph) {
    let modified = graph_mtime(&path);
    if let Ok(mut cache) = graph_cache().lock() {
        cache.entries.insert(
            path,
            GraphCacheEntry {
                graph: graph.clone(),
                modified,
            },
        );
    }
}

/// Trust levels for memories
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum TrustLevel {
    /// User explicitly stated this
    High,
    /// Observed from user behavior
    #[default]
    Medium,
    /// Inferred by the agent
    Low,
}

/// A reinforcement breadcrumb tracking when/where a memory was reinforced
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reinforcement {
    pub session_id: String,
    pub message_index: usize,
    pub timestamp: DateTime<Utc>,
}

/// A single memory entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub category: MemoryCategory,
    pub content: String,
    pub tags: Vec<String>,
    /// Pre-normalized lowercase search text for content + tags.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub search_text: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub access_count: u32,
    pub source: Option<String>,
    /// Trust level for this memory
    #[serde(default)]
    pub trust: TrustLevel,
    /// Consolidation strength (how many times this was reinforced)
    #[serde(default)]
    pub strength: u32,
    /// Whether this memory is active or superseded
    #[serde(default = "default_active")]
    pub active: bool,
    /// ID of memory that superseded this one
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
    /// Reinforcement provenance (breadcrumbs of when/where this was reinforced)
    #[serde(default)]
    pub reinforcements: Vec<Reinforcement>,
    /// Embedding vector for similarity search (384 dimensions for MiniLM)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
    /// Confidence score (0.0-1.0) - decays over time, boosted by use
    #[serde(default = "default_confidence")]
    pub confidence: f32,
}

fn default_confidence() -> f32 {
    1.0
}

fn default_active() -> bool {
    true
}

impl MemoryEntry {
    pub fn new(category: MemoryCategory, content: impl Into<String>) -> Self {
        let now = Utc::now();
        let content = content.into();
        Self {
            id: crate::id::new_id("mem"),
            category,
            search_text: normalize_memory_search_text(&content, &[]),
            content,
            tags: Vec::new(),
            created_at: now,
            updated_at: now,
            access_count: 0,
            source: None,
            trust: TrustLevel::default(),
            strength: 1,
            active: true,
            superseded_by: None,
            reinforcements: Vec::new(),
            embedding: None,
            confidence: 1.0,
        }
    }

    pub fn refresh_search_text(&mut self) {
        self.search_text = normalize_memory_search_text(&self.content, &self.tags);
    }

    pub fn searchable_text(&self) -> std::borrow::Cow<'_, str> {
        if self.search_text.is_empty() {
            std::borrow::Cow::Owned(normalize_memory_search_text(&self.content, &self.tags))
        } else {
            std::borrow::Cow::Borrowed(&self.search_text)
        }
    }

    /// Get effective confidence after time-based decay
    /// Half-life varies by category:
    /// - Correction: 365 days (user corrections are high value)
    /// - Preference: 90 days (preferences may evolve)
    /// - Fact: 30 days (codebase facts can become stale)
    /// - Entity: 60 days (entities change moderately)
    pub fn effective_confidence(&self) -> f32 {
        let age_days = (Utc::now() - self.created_at).num_days() as f32;
        let half_life = match self.category {
            MemoryCategory::Correction => 365.0,
            MemoryCategory::Preference => 90.0,
            MemoryCategory::Fact => 30.0,
            MemoryCategory::Entity => 60.0,
            MemoryCategory::Custom(_) => 45.0, // Default for custom categories
        };

        // Exponential decay: confidence * e^(-age/half_life * ln(2))
        // Also boost slightly for access count
        let decay = (-age_days / half_life * 0.693).exp();
        let access_boost = 1.0 + 0.1 * (self.access_count as f32 + 1.0).ln();

        (self.confidence * decay * access_boost).min(1.0)
    }

    /// Boost confidence (called when memory was useful)
    pub fn boost_confidence(&mut self, amount: f32) {
        self.confidence = (self.confidence + amount).min(1.0);
        self.access_count += 1;
        self.updated_at = Utc::now();
    }

    /// Decay confidence (called when memory was retrieved but not relevant)
    pub fn decay_confidence(&mut self, amount: f32) {
        self.confidence = (self.confidence - amount).max(0.0);
    }

    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self.refresh_search_text();
        self
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    pub fn with_trust(mut self, trust: TrustLevel) -> Self {
        self.trust = trust;
        self
    }

    pub fn touch(&mut self) {
        self.updated_at = Utc::now();
        self.access_count += 1;
    }

    /// Reinforce this memory (called when same info is encountered again)
    pub fn reinforce(&mut self, session_id: &str, message_index: usize) {
        self.strength += 1;
        self.updated_at = Utc::now();
        self.reinforcements.push(Reinforcement {
            session_id: session_id.to_string(),
            message_index,
            timestamp: Utc::now(),
        });
    }

    /// Mark this memory as superseded by another
    pub fn supersede(&mut self, new_id: &str) {
        self.active = false;
        self.superseded_by = Some(new_id.to_string());
    }

    /// Set embedding vector
    pub fn with_embedding(mut self, embedding: Vec<f32>) -> Self {
        self.embedding = Some(embedding);
        self
    }

    /// Generate and set embedding if not already present
    /// Returns true if embedding was generated, false if already exists or failed
    pub fn ensure_embedding(&mut self) -> bool {
        if self.embedding.is_some() {
            return false;
        }

        match crate::embedding::embed(&self.content) {
            Ok(emb) => {
                self.embedding = Some(emb);
                true
            }
            Err(e) => {
                crate::logging::info(&format!("Failed to generate embedding: {}", e));
                false
            }
        }
    }

    /// Check if this memory has an embedding
    pub fn has_embedding(&self) -> bool {
        self.embedding.is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum MemoryCategory {
    Fact,
    Preference,
    Entity,
    Correction,
    Custom(String),
}

impl std::fmt::Display for MemoryCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryCategory::Fact => write!(f, "fact"),
            MemoryCategory::Preference => write!(f, "preference"),
            MemoryCategory::Entity => write!(f, "entity"),
            MemoryCategory::Correction => write!(f, "correction"),
            MemoryCategory::Custom(s) => write!(f, "{}", s),
        }
    }
}

impl std::str::FromStr for MemoryCategory {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_lowercase().as_str() {
            "fact" => MemoryCategory::Fact,
            "preference" => MemoryCategory::Preference,
            "entity" => MemoryCategory::Entity,
            "correction" => MemoryCategory::Correction,
            other => MemoryCategory::Custom(other.to_string()),
        })
    }
}

impl MemoryCategory {
    /// Parse a category string from LLM extraction output.
    /// Maps legacy/incorrect category names to the correct variant and avoids
    /// blindly defaulting to Fact.
    pub fn from_extracted(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "fact" | "facts" => MemoryCategory::Fact,
            "preference" | "preferences" | "pref" => MemoryCategory::Preference,
            "correction" | "corrections" | "fix" | "bug" => MemoryCategory::Correction,
            "entity" | "entities" => MemoryCategory::Entity,
            "observation" | "lesson" | "learning" => MemoryCategory::Fact,
            other => {
                crate::logging::info(&format!(
                    "Unknown memory category from extraction: '{}', defaulting to fact",
                    other
                ));
                MemoryCategory::Fact
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryStore {
    pub entries: Vec<MemoryEntry>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, entry: MemoryEntry) -> String {
        let id = entry.id.clone();
        self.entries.push(entry);
        id
    }

    pub fn by_category(&self, category: &MemoryCategory) -> Vec<&MemoryEntry> {
        self.entries
            .iter()
            .filter(|e| &e.category == category)
            .collect()
    }

    pub fn search(&self, query: &str) -> Vec<&MemoryEntry> {
        let query_lower = normalize_search_text(query);
        if query_lower.is_empty() {
            return Vec::new();
        }

        self.entries
            .iter()
            .filter(|e| memory_matches_search(e, &query_lower))
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<&MemoryEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn remove(&mut self, id: &str) -> Option<MemoryEntry> {
        if let Some(pos) = self.entries.iter().position(|e| e.id == id) {
            Some(self.entries.remove(pos))
        } else {
            None
        }
    }

    pub fn get_relevant(&self, limit: usize) -> Vec<&MemoryEntry> {
        top_k_by_score(
            self.entries
                .iter()
                .filter(|entry| entry.active)
                .map(|entry| (entry, memory_score(entry) as f32)),
            limit,
        )
        .into_iter()
        .map(|(entry, _)| entry)
        .collect()
    }

    pub fn format_for_prompt(&self, limit: usize) -> Option<String> {
        let relevant: Vec<MemoryEntry> = self.get_relevant(limit).into_iter().cloned().collect();
        format_entries_for_prompt(&relevant, limit)
    }
}

const MEMORY_RELEVANCE_MAX_CANDIDATES: usize = 30;
const MEMORY_RELEVANCE_MAX_RESULTS: usize = 10;

fn memory_score(entry: &MemoryEntry) -> f64 {
    // Skip inactive memories
    if !entry.active {
        return 0.0;
    }

    let mut score = 0.0;

    // Recency factor (decays over time)
    let age_hours = (Utc::now() - entry.updated_at).num_hours() as f64;
    score += 100.0 / (1.0 + age_hours / 24.0);

    // Access frequency bonus
    score += (entry.access_count as f64).sqrt() * 10.0;

    // Category importance
    score += match entry.category {
        MemoryCategory::Correction => 50.0,
        MemoryCategory::Preference => 30.0,
        MemoryCategory::Fact => 20.0,
        MemoryCategory::Entity => 10.0,
        MemoryCategory::Custom(_) => 5.0,
    };

    // Trust level multiplier
    score *= match entry.trust {
        TrustLevel::High => 1.5,
        TrustLevel::Medium => 1.0,
        TrustLevel::Low => 0.7,
    };

    // Consolidation strength bonus
    score += (entry.strength as f64).ln() * 5.0;

    score
}

#[derive(Debug)]
struct TopKItem<T> {
    score: f32,
    ordinal: usize,
    value: T,
}

impl<T> PartialEq for TopKItem<T> {
    fn eq(&self, other: &Self) -> bool {
        self.score.to_bits() == other.score.to_bits() && self.ordinal == other.ordinal
    }
}

impl<T> Eq for TopKItem<T> {}

impl<T> PartialOrd for TopKItem<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for TopKItem<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.score
            .total_cmp(&other.score)
            .then_with(|| self.ordinal.cmp(&other.ordinal))
    }
}

fn top_k_by_score<T, I>(items: I, limit: usize) -> Vec<(T, f32)>
where
    I: IntoIterator<Item = (T, f32)>,
{
    if limit == 0 {
        return Vec::new();
    }

    let mut heap: BinaryHeap<Reverse<TopKItem<T>>> = BinaryHeap::new();

    for (ordinal, (value, score)) in items.into_iter().enumerate() {
        let candidate = Reverse(TopKItem {
            score,
            ordinal,
            value,
        });

        if heap.len() < limit {
            heap.push(candidate);
            continue;
        }

        let replace = heap
            .peek()
            .map(|smallest| score > smallest.0.score)
            .unwrap_or(false);
        if replace {
            heap.pop();
            heap.push(candidate);
        }
    }

    let mut results: Vec<_> = heap
        .into_iter()
        .map(|Reverse(item)| (item.value, item.score, item.ordinal))
        .collect();
    results.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.2.cmp(&b.2)));
    results
        .into_iter()
        .map(|(value, score, _)| (value, score))
        .collect()
}

#[derive(Debug)]
struct TopKOrdItem<T, K> {
    key: K,
    ordinal: usize,
    value: T,
}

impl<T, K: Ord> PartialEq for TopKOrdItem<T, K> {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key && self.ordinal == other.ordinal
    }
}

impl<T, K: Ord> Eq for TopKOrdItem<T, K> {}

impl<T, K: Ord> PartialOrd for TopKOrdItem<T, K> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T, K: Ord> Ord for TopKOrdItem<T, K> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.key
            .cmp(&other.key)
            .then_with(|| self.ordinal.cmp(&other.ordinal))
    }
}

fn top_k_by_ord<T, K, I>(items: I, limit: usize) -> Vec<(T, K)>
where
    I: IntoIterator<Item = (T, K)>,
    K: Ord,
{
    if limit == 0 {
        return Vec::new();
    }

    let mut heap: BinaryHeap<Reverse<TopKOrdItem<T, K>>> = BinaryHeap::new();

    for (ordinal, (value, key)) in items.into_iter().enumerate() {
        let candidate = Reverse(TopKOrdItem {
            key,
            ordinal,
            value,
        });

        if heap.len() < limit {
            heap.push(candidate);
            continue;
        }

        let replace = heap
            .peek()
            .map(|smallest| candidate.0.key > smallest.0.key)
            .unwrap_or(false);
        if replace {
            heap.pop();
            heap.push(candidate);
        }
    }

    let mut results: Vec<_> = heap
        .into_iter()
        .map(|Reverse(item)| (item.value, item.key, item.ordinal))
        .collect();
    results.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.2.cmp(&b.2)));
    results
        .into_iter()
        .map(|(value, key, _)| (value, key))
        .collect()
}

fn normalize_search_text(text: &str) -> String {
    let lowered = text.trim().to_lowercase();
    let mut normalized = String::with_capacity(lowered.len());
    let mut last_was_space = true;

    for ch in lowered.chars() {
        let mapped = if ch.is_whitespace() || matches!(ch, '-' | '_' | '/' | '\\' | '.' | ':') {
            ' '
        } else {
            ch
        };

        if mapped == ' ' {
            if !last_was_space {
                normalized.push(' ');
                last_was_space = true;
            }
        } else {
            normalized.push(mapped);
            last_was_space = false;
        }
    }

    normalized.trim_end().to_string()
}

fn normalize_memory_search_text(content: &str, tags: &[String]) -> String {
    let normalized_content = normalize_search_text(content);
    let normalized_tags: Vec<String> = tags
        .iter()
        .map(|tag| normalize_search_text(tag))
        .filter(|tag| !tag.is_empty())
        .collect();

    if normalized_tags.is_empty() {
        return normalized_content;
    }

    if normalized_content.is_empty() {
        return normalized_tags.join(" ");
    }

    format!("{} {}", normalized_content, normalized_tags.join(" "))
}

fn memory_matches_search(memory: &MemoryEntry, normalized_query: &str) -> bool {
    memory.searchable_text().contains(normalized_query)
}

#[derive(Clone)]
pub struct MemoryManager {
    project_dir: Option<PathBuf>,
    /// When true, use isolated test storage instead of real memory
    test_mode: bool,
    include_skills: bool,
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
        self.project_dir
            .clone()
            .or_else(|| std::env::current_dir().ok())
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

        let project_hash = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            project_dir.hash(&mut hasher);
            format!("{:016x}", hasher.finish())
        };

        let memory_dir = storage::jcode_dir()?.join("memory").join("projects");
        Ok(Some(memory_dir.join(format!("{}.json", project_hash))))
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
            entry.ensure_embedding();
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

    /// Similarity threshold for storage-layer dedup.
    /// Memories above this threshold are considered duplicates and reinforced instead.
    const STORAGE_DEDUP_THRESHOLD: f32 = 0.85;

    pub fn remember_project(&self, entry: MemoryEntry) -> Result<String> {
        let mut entry = entry;
        entry.ensure_embedding();

        let mut graph = self.load_project_graph()?;

        if let Some(ref emb) = entry.embedding {
            if let Some(existing_id) =
                Self::find_duplicate_in_graph(&graph, emb, Self::STORAGE_DEDUP_THRESHOLD)
                && let Some(existing) = graph.get_memory_mut(&existing_id)
            {
                existing.reinforce(entry.source.as_deref().unwrap_or("dedup"), 0);
                self.save_project_graph(&graph)?;
                return Ok(existing_id);
            }

            // Cross-store dedup: also check global graph
            if let Ok(mut global_graph) = self.load_global_graph()
                && let Some(existing_id) =
                    Self::find_duplicate_in_graph(&global_graph, emb, Self::STORAGE_DEDUP_THRESHOLD)
                && let Some(existing) = global_graph.get_memory_mut(&existing_id)
            {
                existing.reinforce(entry.source.as_deref().unwrap_or("cross-dedup"), 0);
                self.save_global_graph(&global_graph)?;
                return Ok(existing_id);
            }
        }

        let id = graph.add_memory(entry);
        self.save_project_graph(&graph)?;
        Ok(id)
    }

    pub fn remember_global(&self, entry: MemoryEntry) -> Result<String> {
        let mut entry = entry;
        entry.ensure_embedding();

        let mut graph = self.load_global_graph()?;

        if let Some(ref emb) = entry.embedding {
            if let Some(existing_id) =
                Self::find_duplicate_in_graph(&graph, emb, Self::STORAGE_DEDUP_THRESHOLD)
                && let Some(existing) = graph.get_memory_mut(&existing_id)
            {
                existing.reinforce(entry.source.as_deref().unwrap_or("dedup"), 0);
                self.save_global_graph(&graph)?;
                return Ok(existing_id);
            }

            // Cross-store dedup: also check project graph
            if let Ok(mut project_graph) = self.load_project_graph()
                && let Some(existing_id) = Self::find_duplicate_in_graph(
                    &project_graph,
                    emb,
                    Self::STORAGE_DEDUP_THRESHOLD,
                )
                && let Some(existing) = project_graph.get_memory_mut(&existing_id)
            {
                existing.reinforce(entry.source.as_deref().unwrap_or("cross-dedup"), 0);
                self.save_project_graph(&project_graph)?;
                return Ok(existing_id);
            }
        }

        let id = graph.add_memory(entry);
        self.save_global_graph(&graph)?;
        Ok(id)
    }

    /// Insert or update a memory with a stable ID in the project graph.
    /// Preserves existing inbound/outbound graph relationships while refreshing
    /// content and tags.
    pub fn upsert_project_memory(&self, entry: MemoryEntry) -> Result<String> {
        let mut graph = self.load_project_graph()?;
        let id = Self::upsert_memory_in_graph(&mut graph, entry);
        self.save_project_graph(&graph)?;
        Ok(id)
    }

    /// Insert or update a memory with a stable ID in the global graph.
    /// Preserves existing inbound/outbound graph relationships while refreshing
    /// content and tags.
    pub fn upsert_global_memory(&self, entry: MemoryEntry) -> Result<String> {
        let mut graph = self.load_global_graph()?;
        let id = Self::upsert_memory_in_graph(&mut graph, entry);
        self.save_global_graph(&graph)?;
        Ok(id)
    }

    fn upsert_memory_in_graph(
        graph: &mut crate::memory_graph::MemoryGraph,
        mut entry: MemoryEntry,
    ) -> String {
        let id = entry.id.clone();
        entry.ensure_embedding();

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
                existing.embedding = None;
                existing.ensure_embedding();
            }
        }

        id
    }

    fn find_duplicate_in_graph(
        graph: &crate::memory_graph::MemoryGraph,
        query_emb: &[f32],
        threshold: f32,
    ) -> Option<String> {
        let mut best: Option<(String, f32)> = None;
        for entry in graph.active_memories() {
            if let Some(ref emb) = entry.embedding {
                let sim = crate::embedding::cosine_similarity(query_emb, emb);
                if sim >= threshold && best.as_ref().map(|(_, s)| sim > *s).unwrap_or(true) {
                    best = Some((entry.id.clone(), sim));
                }
            }
        }
        best.map(|(id, _)| id)
    }

    /// Find memories similar to the given text using embedding search
    /// Returns memories with similarity above threshold, sorted by similarity
    pub fn find_similar(
        &self,
        text: &str,
        threshold: f32,
        limit: usize,
    ) -> Result<Vec<(MemoryEntry, f32)>> {
        // Generate embedding for query text
        let query_embedding = match crate::embedding::embed(text) {
            Ok(emb) => emb,
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
        let query_embedding = match crate::embedding::embed(text) {
            Ok(emb) => emb,
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
        Self::score_and_filter(entries_with_emb, query_embedding, threshold, limit)
    }

    pub fn find_similar_with_embedding_scoped(
        &self,
        query_embedding: &[f32],
        threshold: f32,
        limit: usize,
        scope: MemoryScope,
    ) -> Result<Vec<(MemoryEntry, f32)>> {
        let entries_with_emb = self.collect_memories_with_embeddings_scoped(scope)?;
        Self::score_and_filter(entries_with_emb, query_embedding, threshold, limit)
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

    fn synthetic_skill_entries(&self) -> Vec<MemoryEntry> {
        if !self.include_skills {
            return Vec::new();
        }

        crate::skill::SkillRegistry::shared_snapshot()
            .list()
            .into_iter()
            .map(|skill| skill.as_memory_entry())
            .collect()
    }

    fn collect_retrieval_candidates_scoped(&self, scope: MemoryScope) -> Result<Vec<MemoryEntry>> {
        let mut entries = self.collect_memories_scoped(scope)?;
        if scope.includes_global() {
            entries.extend(self.synthetic_skill_entries());
        }
        Ok(entries)
    }

    fn collect_retrieval_candidates_with_embeddings_scoped(
        &self,
        scope: MemoryScope,
    ) -> Result<Vec<MemoryEntry>> {
        let mut entries = self.collect_memories_with_embeddings_scoped(scope)?;
        if scope.includes_global() {
            entries.extend(
                self.synthetic_skill_entries()
                    .into_iter()
                    .filter_map(|mut entry| entry.ensure_embedding().then_some(entry)),
            );
        }
        Ok(entries)
    }

    fn find_retrieval_candidates_similar_scoped(
        &self,
        text: &str,
        threshold: f32,
        limit: usize,
        scope: MemoryScope,
    ) -> Result<Vec<(MemoryEntry, f32)>> {
        let query_embedding = match crate::embedding::embed(text) {
            Ok(emb) => emb,
            Err(e) => {
                crate::logging::info(&format!(
                    "Embedding failed for retrieval candidates, falling back to keyword search: {}",
                    e
                ));
                return Ok(Vec::new());
            }
        };

        let entries = self.collect_retrieval_candidates_with_embeddings_scoped(scope)?;
        Self::score_and_filter(entries, &query_embedding, threshold, limit)
    }

    fn score_and_filter(
        entries: Vec<MemoryEntry>,
        query_embedding: &[f32],
        threshold: f32,
        limit: usize,
    ) -> Result<Vec<(MemoryEntry, f32)>> {
        if entries.is_empty() {
            return Ok(Vec::new());
        }

        let mut filtered_entries = Vec::with_capacity(entries.len());
        let mut skipped_missing_embeddings = 0usize;
        for entry in entries {
            if entry.embedding.is_some() {
                filtered_entries.push(entry);
            } else {
                skipped_missing_embeddings += 1;
            }
        }
        if skipped_missing_embeddings > 0 {
            crate::logging::warn(&format!(
                "Skipped {} retrieval candidate(s) without embeddings during similarity scoring",
                skipped_missing_embeddings
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

        let scored = top_k_by_score(
            filtered_entries
                .into_iter()
                .zip(scores)
                .filter(|(_, sim)| *sim >= threshold),
            limit,
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

    fn touch_entries(&self, ids: &[String]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }

        let id_set: std::collections::HashSet<&str> = ids.iter().map(|id| id.as_str()).collect();

        let mut project = self.load_project_graph()?;
        let mut project_changed = false;
        for entry in project.memories.values_mut() {
            if id_set.contains(entry.id.as_str()) {
                entry.touch();
                project_changed = true;
            }
        }
        if project_changed {
            self.save_project_graph(&project)?;
        }

        let mut global = self.load_global_graph()?;
        let mut global_changed = false;
        for entry in global.memories.values_mut() {
            if id_set.contains(entry.id.as_str()) {
                entry.touch();
                global_changed = true;
            }
        }
        if global_changed {
            self.save_global_graph(&global)?;
        }

        Ok(())
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

    pub fn forget(&self, id: &str) -> Result<bool> {
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
        if !memory_sidecar_enabled() {
            crate::logging::info("Memory transcript extraction skipped: memory sidecar disabled");
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

    /// Check if stored memories are relevant to the current context
    /// Returns memories that the sidecar deems relevant
    pub async fn get_relevant_for_context(
        &self,
        context: &str,
        max_candidates: usize,
    ) -> Result<Vec<MemoryEntry>> {
        // Get top candidate memories by score
        let candidates: Vec<_> = top_k_by_score(
            self.collect_retrieval_candidates_scoped(MemoryScope::All)?
                .into_iter()
                .filter(|entry| entry.active)
                .map(|entry| {
                    let score = memory_score(&entry) as f32;
                    (entry, score)
                }),
            max_candidates,
        )
        .into_iter()
        .map(|(entry, _)| entry)
        .collect();

        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        // Update activity state - checking memories
        set_state(MemoryState::SidecarChecking {
            count: candidates.len(),
        });
        add_event(MemoryEventKind::SidecarStarted);

        let sidecar = Sidecar::new();
        let mut relevant = Vec::new();
        let mut relevant_ids = Vec::new();

        for memory in candidates {
            let start = Instant::now();
            match sidecar.check_relevance(&memory.content, context).await {
                Ok((is_relevant, _reason)) => {
                    let latency_ms = start.elapsed().as_millis() as u64;
                    add_event(MemoryEventKind::SidecarComplete { latency_ms });

                    if is_relevant {
                        let preview = if memory.content.len() > 30 {
                            format!("{}...", crate::util::truncate_str(&memory.content, 30))
                        } else {
                            memory.content.clone()
                        };
                        add_event(MemoryEventKind::SidecarRelevant {
                            memory_preview: preview,
                        });
                        relevant_ids.push(memory.id.clone());
                        relevant.push(memory);
                    } else {
                        add_event(MemoryEventKind::SidecarNotRelevant);
                    }
                }
                Err(e) => {
                    add_event(MemoryEventKind::Error {
                        message: e.to_string(),
                    });
                    crate::logging::error(&format!("Sidecar relevance check failed: {}", e));
                }
            }
        }

        let _ = self.touch_entries(&relevant_ids);

        // Update final state
        if relevant.is_empty() {
            set_state(MemoryState::Idle);
        } else {
            set_state(MemoryState::FoundRelevant {
                count: relevant.len(),
            });
        }

        Ok(relevant)
    }

    /// Simple relevance check without sidecar (keyword-based)
    /// Use this for quick checks when sidecar is not needed
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
            let manager = if manager.project_dir.is_none() {
                MemoryManager {
                    project_dir: std::env::current_dir().ok(),
                    ..manager
                }
            } else {
                manager
            };

            match manager
                .get_relevant_parallel(&sid, &messages, event_tx.clone())
                .await
            {
                Ok((Some(prompt), memory_ids, display_prompt)) => {
                    let count = prompt
                        .lines()
                        .map(str::trim_start)
                        .filter(|line| {
                            line.starts_with("- ")
                                || line
                                    .split_once(". ")
                                    .map(|(prefix, _)| {
                                        !prefix.is_empty()
                                            && prefix.chars().all(|c| c.is_ascii_digit())
                                    })
                                    .unwrap_or(false)
                        })
                        .count()
                        .max(1);
                    set_pending_memory_with_ids_and_display(
                        &sid,
                        prompt,
                        count,
                        memory_ids,
                        display_prompt,
                    );
                    if memory_sidecar_enabled() {
                        add_event(MemoryEventKind::SidecarComplete { latency_ms: 0 });
                    }
                    emit_memory_activity(event_tx.as_ref());
                }
                Ok((None, _, _)) => {
                    set_state(MemoryState::Idle);
                    emit_memory_activity(event_tx.as_ref());
                }
                Err(e) => {
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

    /// Get relevant memories using embedding search + sidecar verification.
    ///
    /// 1. Embed the context (fast, local, ~30ms)
    /// 2. Find similar memories by embedding (instant)
    /// 3. Only call sidecar for embedding hits (1-5 calls instead of 30)
    ///
    /// Returns `(formatted_prompt, memory_ids, display_prompt)` on success.
    pub async fn get_relevant_parallel(
        &self,
        session_id: &str,
        messages: &[crate::message::Message],
        event_tx: Option<MemoryEventSink>,
    ) -> Result<(Option<String>, Vec<String>, Option<String>)> {
        let context = format_context_for_relevance(messages);
        if context.is_empty() {
            return Ok((None, Vec::new(), None));
        }

        // Start pipeline tracking
        pipeline_start();

        // Step 1: Embedding search (fast, local)
        set_state(MemoryState::Embedding);
        add_event(MemoryEventKind::EmbeddingStarted);
        pipeline_update(|p| p.search = StepStatus::Running);
        emit_memory_activity(event_tx.as_ref());

        let embedding_start = Instant::now();
        let candidates = match self.find_retrieval_candidates_similar_scoped(
            &context,
            EMBEDDING_SIMILARITY_THRESHOLD,
            EMBEDDING_MAX_HITS,
            MemoryScope::All,
        ) {
            Ok(hits) => {
                let latency_ms = embedding_start.elapsed().as_millis() as u64;
                if hits.is_empty() {
                    add_event(MemoryEventKind::EmbeddingComplete {
                        latency_ms,
                        hits: 0,
                    });
                    pipeline_update(|p| {
                        p.search = StepStatus::Done;
                        p.search_result = Some(StepResult {
                            summary: "0 hits".to_string(),
                            latency_ms,
                        });
                        p.verify = StepStatus::Skipped;
                        p.inject = StepStatus::Skipped;
                        p.maintain = StepStatus::Skipped;
                    });
                    set_state(MemoryState::Idle);
                    emit_memory_activity(event_tx.as_ref());
                    return Ok((None, Vec::new(), None));
                }
                pipeline_update(|p| {
                    p.search = StepStatus::Done;
                    p.search_result = Some(StepResult {
                        summary: format!("{} hits", hits.len()),
                        latency_ms,
                    });
                });
                add_event(MemoryEventKind::EmbeddingComplete {
                    latency_ms,
                    hits: hits.len(),
                });
                hits
            }
            Err(e) => {
                crate::logging::info(&format!("Embedding search failed, falling back: {}", e));
                add_event(MemoryEventKind::Error {
                    message: e.to_string(),
                });
                pipeline_update(|p| {
                    p.search = StepStatus::Error;
                    p.search_result = Some(StepResult {
                        summary: "fallback".to_string(),
                        latency_ms: embedding_start.elapsed().as_millis() as u64,
                    });
                });
                emit_memory_activity(event_tx.as_ref());

                top_k_by_score(
                    self.collect_retrieval_candidates_scoped(MemoryScope::All)?
                        .into_iter()
                        .filter(|entry| entry.active)
                        .map(|entry| {
                            let score = memory_score(&entry) as f32;
                            (entry, score)
                        }),
                    MEMORY_RELEVANCE_MAX_CANDIDATES,
                )
                .into_iter()
                .map(|(entry, _)| (entry, 0.0))
                .collect()
            }
        };

        // Filter out memories that have already been injected in this session
        let pre_filter_count = candidates.len();
        let candidates: Vec<_> = candidates
            .into_iter()
            .filter(|(entry, _)| !is_memory_injected_any(&entry.id))
            .collect();
        if candidates.len() < pre_filter_count {
            crate::logging::info(&format!(
                "Filtered out {} already-injected memories ({} -> {} candidates)",
                pre_filter_count - candidates.len(),
                pre_filter_count,
                candidates.len()
            ));
        }

        if candidates.is_empty() {
            pipeline_update(|p| {
                p.verify = StepStatus::Skipped;
                p.inject = StepStatus::Skipped;
                p.maintain = StepStatus::Skipped;
            });
            set_state(MemoryState::Idle);
            emit_memory_activity(event_tx.as_ref());
            return Ok((None, Vec::new(), None));
        }

        if !memory_sidecar_enabled() {
            let relevant: Vec<_> = candidates
                .into_iter()
                .take(MEMORY_RELEVANCE_MAX_RESULTS)
                .map(|(entry, _)| entry)
                .collect();
            let relevant_ids: Vec<String> = relevant.iter().map(|entry| entry.id.clone()).collect();
            let _ = self.touch_entries(&relevant_ids);

            if relevant.is_empty() {
                pipeline_update(|p| {
                    p.verify = StepStatus::Skipped;
                    p.verify_result = Some(StepResult {
                        summary: "semantic only".to_string(),
                        latency_ms: 0,
                    });
                    p.inject = StepStatus::Skipped;
                    p.maintain = StepStatus::Skipped;
                });
                set_state(MemoryState::Idle);
                emit_memory_activity(event_tx.as_ref());
                return Ok((None, Vec::new(), None));
            }

            pipeline_update(|p| {
                p.verify = StepStatus::Skipped;
                p.verify_result = Some(StepResult {
                    summary: format!("semantic {}", relevant.len()),
                    latency_ms: 0,
                });
                p.inject = StepStatus::Running;
            });

            set_state(MemoryState::FoundRelevant {
                count: relevant.len(),
            });
            emit_memory_activity(event_tx.as_ref());

            let prompt = format_relevant_prompt(&relevant, MEMORY_RELEVANCE_MAX_RESULTS);
            let display_prompt =
                format_relevant_display_prompt(&relevant, MEMORY_RELEVANCE_MAX_RESULTS);

            pipeline_update(|p| {
                p.inject = StepStatus::Done;
                p.inject_result = Some(StepResult {
                    summary: format!("{} memories", relevant.len()),
                    latency_ms: 0,
                });
            });
            emit_memory_activity(event_tx.as_ref());

            return Ok((prompt, relevant_ids, display_prompt));
        }

        // Step 2: Sidecar verification (only for embedding hits - much fewer calls!)
        let total_candidates = candidates.len();
        set_state(MemoryState::SidecarChecking {
            count: total_candidates,
        });
        add_event(MemoryEventKind::SidecarStarted);
        pipeline_update(|p| {
            p.verify = StepStatus::Running;
            p.verify_progress = Some((0, total_candidates));
        });
        emit_memory_activity(event_tx.as_ref());

        let sidecar = Sidecar::new();
        let mut relevant = Vec::new();
        let mut relevant_ids = Vec::new();

        // Process in parallel batches
        const BATCH_SIZE: usize = 5;
        for batch in candidates.chunks(BATCH_SIZE) {
            let futures: Vec<_> = batch
                .iter()
                .map(|(memory, _sim)| {
                    let sidecar = sidecar.clone();
                    let content = memory.content.clone();
                    let ctx = context.clone();
                    async move {
                        let start = Instant::now();
                        let result = sidecar.check_relevance(&content, &ctx).await;
                        (result, start.elapsed())
                    }
                })
                .collect();

            let results = futures::future::join_all(futures).await;

            for ((memory, sim), (result, elapsed)) in batch.iter().zip(results) {
                match result {
                    Ok((is_relevant, _reason)) => {
                        add_event(MemoryEventKind::SidecarComplete {
                            latency_ms: elapsed.as_millis() as u64,
                        });

                        if is_relevant {
                            let preview = if memory.content.len() > 30 {
                                format!("{}...", crate::util::truncate_str(&memory.content, 30))
                            } else {
                                memory.content.clone()
                            };
                            add_event(MemoryEventKind::SidecarRelevant {
                                memory_preview: preview,
                            });
                            relevant_ids.push(memory.id.clone());
                            relevant.push(memory.clone());
                            crate::logging::info(&format!(
                                "[{}] Memory relevant (sim={:.2}): {}",
                                session_id,
                                sim,
                                crate::util::truncate_str(&memory.content, 50)
                            ));
                        } else {
                            add_event(MemoryEventKind::SidecarNotRelevant);
                        }
                    }
                    Err(e) => {
                        add_event(MemoryEventKind::Error {
                            message: e.to_string(),
                        });
                        crate::logging::info(&format!("Sidecar check failed: {}", e));
                    }
                }
                // Update verify progress
                let checked = relevant.len()
                    + batch.len().saturating_sub(
                        batch.len(), // approximate
                    );
                let _ = checked; // Progress updated below per-batch
            }
            // Update pipeline verify progress after each batch
            pipeline_update(|p| {
                p.verify_progress = Some((
                    relevant_ids.len()
                        + (total_candidates - candidates.len().min(total_candidates)),
                    total_candidates,
                ));
            });
            emit_memory_activity(event_tx.as_ref());
        }

        let verify_latency_ms = embedding_start.elapsed().as_millis() as u64;
        let _ = self.touch_entries(&relevant_ids);

        if relevant.is_empty() {
            pipeline_update(|p| {
                p.verify = StepStatus::Done;
                p.verify_result = Some(StepResult {
                    summary: "0 relevant".to_string(),
                    latency_ms: verify_latency_ms,
                });
                p.inject = StepStatus::Skipped;
                p.maintain = StepStatus::Skipped;
            });
            set_state(MemoryState::Idle);
            emit_memory_activity(event_tx.as_ref());
            return Ok((None, Vec::new(), None));
        }

        pipeline_update(|p| {
            p.verify = StepStatus::Done;
            p.verify_result = Some(StepResult {
                summary: format!("{} relevant", relevant.len()),
                latency_ms: verify_latency_ms,
            });
            p.inject = StepStatus::Running;
        });

        set_state(MemoryState::FoundRelevant {
            count: relevant.len(),
        });
        emit_memory_activity(event_tx.as_ref());

        let prompt = format_relevant_prompt(&relevant, MEMORY_RELEVANCE_MAX_RESULTS);
        let display_prompt =
            format_relevant_display_prompt(&relevant, MEMORY_RELEVANCE_MAX_RESULTS);

        // Mark inject as done - the prompt is ready for injection
        pipeline_update(|p| {
            p.inject = StepStatus::Done;
            p.inject_result = Some(StepResult {
                summary: format!("{} memories", relevant.len()),
                latency_ms: 0,
            });
        });
        emit_memory_activity(event_tx.as_ref());

        Ok((prompt, relevant_ids, display_prompt))
    }

    // ==================== Graph-Based Operations ====================

    /// Load project memories as a MemoryGraph with automatic migration
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

/// Embedding similarity threshold (0.0 - 1.0)
/// Lower = more candidates, higher = fewer but more relevant
pub const EMBEDDING_SIMILARITY_THRESHOLD: f32 = 0.5;

/// Maximum embedding hits to verify with sidecar
pub const EMBEDDING_MAX_HITS: usize = 10;

impl Default for MemoryManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "memory_tests.rs"]
mod tests;
