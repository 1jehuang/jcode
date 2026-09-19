pub mod graph;
pub use graph::{
    ClusterEntry, Edge, EdgeKind, GRAPH_VERSION, GraphMetadata, MemoryGraph, TagEntry,
};

use std::time::Instant;

/// Represents current memory system activity.
#[derive(Debug, Clone)]
pub struct MemoryActivity {
    /// Current state of the memory system.
    pub state: MemoryState,
    /// When the current state was entered, used for elapsed time display and staleness detection.
    pub state_since: Instant,
    /// Pipeline progress for the per-turn search, verify, inject, maintain flow.
    pub pipeline: Option<PipelineState>,
    /// Recent events, most recent first.
    pub recent_events: Vec<MemoryEvent>,
}

impl MemoryActivity {
    pub fn is_processing(&self) -> bool {
        !matches!(self.state, MemoryState::Idle)
            || self
                .pipeline
                .as_ref()
                .map(PipelineState::has_running_step)
                .unwrap_or(false)
    }
}

/// Status of a single pipeline step.
#[derive(Debug, Clone, PartialEq)]
pub enum StepStatus {
    Pending,
    Running,
    Done,
    Error,
    Skipped,
}

/// Result data for a completed pipeline step.
#[derive(Debug, Clone)]
pub struct StepResult {
    pub summary: String,
    pub latency_ms: u64,
}

/// Tracks the 4-step per-turn memory pipeline: search, verify, inject, maintain.
#[derive(Debug, Clone)]
pub struct PipelineState {
    pub search: StepStatus,
    pub search_result: Option<StepResult>,
    pub verify: StepStatus,
    pub verify_result: Option<StepResult>,
    pub verify_progress: Option<(usize, usize)>,
    pub inject: StepStatus,
    pub inject_result: Option<StepResult>,
    pub maintain: StepStatus,
    pub maintain_result: Option<StepResult>,
    pub started_at: Instant,
}

impl PipelineState {
    pub fn new() -> Self {
        Self {
            search: StepStatus::Pending,
            search_result: None,
            verify: StepStatus::Pending,
            verify_result: None,
            verify_progress: None,
            inject: StepStatus::Pending,
            inject_result: None,
            maintain: StepStatus::Pending,
            maintain_result: None,
            started_at: Instant::now(),
        }
    }

    pub fn is_complete(&self) -> bool {
        matches!(
            (&self.search, &self.verify, &self.inject, &self.maintain),
            (
                StepStatus::Done | StepStatus::Error | StepStatus::Skipped,
                StepStatus::Done | StepStatus::Error | StepStatus::Skipped,
                StepStatus::Done | StepStatus::Error | StepStatus::Skipped,
                StepStatus::Done | StepStatus::Error | StepStatus::Skipped,
            )
        )
    }

    pub fn has_running_step(&self) -> bool {
        matches!(self.search, StepStatus::Running)
            || matches!(self.verify, StepStatus::Running)
            || matches!(self.inject, StepStatus::Running)
            || matches!(self.maintain, StepStatus::Running)
    }
}

impl Default for PipelineState {
    fn default() -> Self {
        Self::new()
    }
}

/// State of the memory sidecar.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum MemoryState {
    /// Idle, no activity.
    #[default]
    Idle,
    /// Running embedding search.
    Embedding,
    /// Sidecar checking relevance.
    SidecarChecking { count: usize },
    /// Found relevant memories.
    FoundRelevant { count: usize },
    /// Extracting memories from conversation.
    Extracting { reason: String },
    /// Background maintenance or gardening of the memory graph.
    Maintaining { phase: String },
    /// Agent is actively using a memory tool.
    ToolAction { action: String, detail: String },
}

/// A memory system event.
#[derive(Debug, Clone)]
pub struct MemoryEvent {
    /// Type of event.
    pub kind: MemoryEventKind,
    /// When it happened.
    pub timestamp: Instant,
    /// Optional details.
    pub detail: Option<String>,
}

#[derive(Debug, Clone)]
pub struct InjectedMemoryItem {
    pub section: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub enum MemoryEventKind {
    /// Embedding search started.
    EmbeddingStarted,
    /// Embedding search completed.
    EmbeddingComplete { latency_ms: u64, hits: usize },
    /// Sidecar started checking.
    SidecarStarted,
    /// Sidecar found memory relevant.
    SidecarRelevant { memory_preview: String },
    /// Sidecar found memory not relevant.
    SidecarNotRelevant,
    /// Sidecar call completed with latency.
    SidecarComplete { latency_ms: u64 },
    /// Memory was surfaced to main agent.
    MemorySurfaced { memory_preview: String },
    /// Memory payload was injected into model context.
    MemoryInjected {
        count: usize,
        prompt_chars: usize,
        age_ms: u64,
        preview: String,
        items: Vec<InjectedMemoryItem>,
    },
    /// Background maintenance started.
    MaintenanceStarted { verified: usize, rejected: usize },
    /// Background maintenance discovered or strengthened links.
    MaintenanceLinked { links: usize },
    /// Background maintenance adjusted confidence.
    MaintenanceConfidence { boosted: usize, decayed: usize },
    /// Background maintenance refined clusters.
    MaintenanceCluster { clusters: usize, members: usize },
    /// Background maintenance inferred or applied a shared tag.
    MaintenanceTagInferred { tag: String, applied: usize },
    /// Background maintenance detected a gap.
    MaintenanceGap { candidates: usize },
    /// Background maintenance completed.
    MaintenanceComplete { latency_ms: u64 },
    /// Extraction started.
    ExtractionStarted { reason: String },
    /// Extraction completed.
    ExtractionComplete { count: usize },
    /// Error occurred.
    Error { message: String },
    /// Agent stored a memory via tool.
    ToolRemembered {
        content: String,
        scope: String,
        category: String,
    },
    /// Agent recalled or searched memories via tool.
    ToolRecalled { query: String, count: usize },
    /// Agent forgot a memory via tool.
    ToolForgot { id: String },
    /// Agent tagged a memory via tool.
    ToolTagged { id: String, tags: String },
    /// Agent linked memories via tool.
    ToolLinked { from: String, to: String },
    /// Agent listed memories via tool.
    ToolListed { count: usize },
}

// Persistent memory model and pure search helpers.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
    /// Identifier of the embedding model that produced `embedding`, e.g.
    /// "minilm-l6-v2" (local) or "openai:text-embedding-3-small". `None` means
    /// the legacy local MiniLM model (memories written before model tagging).
    /// Used to keep dense similarity comparisons within a single vector space:
    /// only embeddings from the active model are compared; mismatched memories
    /// remain reachable via lexical (BM25) search and RRF fusion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_model: Option<String>,
    /// Confidence score (0.0-1.0) - decays over time, boosted by use
    #[serde(default = "default_confidence")]
    pub confidence: f32,
    /// Provenance citation: the exact source span this memory was drawn
    /// from. `None` for memories banked before citations or from
    /// non-file sources. Serde-defaulted: old entries load unchanged and
    /// recall ignores uncited memories for verification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citation: Option<SourceCitation>,
}

/// Model id used for memories embedded before model tagging existed. These were
/// all produced by the local all-MiniLM-L6-v2 model.
pub const LEGACY_EMBEDDING_MODEL: &str = "minilm-l6-v2";

impl MemoryEntry {
    /// The embedding model id for this entry, treating an untagged (`None`)
    /// embedding as the legacy local MiniLM model.
    pub fn effective_embedding_model(&self) -> &str {
        self.embedding_model
            .as_deref()
            .unwrap_or(LEGACY_EMBEDDING_MODEL)
    }

    /// Whether this entry's embedding was produced by `model` (legacy-aware).
    pub fn embedding_matches_model(&self, model: &str) -> bool {
        self.embedding.is_some() && self.effective_embedding_model() == model
    }
}

fn default_confidence() -> f32 {
    1.0
}

fn default_active() -> bool {
    true
}

fn new_memory_id() -> String {
    let ts = Utc::now().timestamp_millis();
    let rand: u64 = rand::random();
    format!("mem_{ts}_{rand}")
}

impl MemoryEntry {
    pub fn new(category: MemoryCategory, content: impl Into<String>) -> Self {
        let now = Utc::now();
        let content = content.into();
        Self {
            id: new_memory_id(),
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
            embedding_model: None,
            confidence: 1.0,
            citation: None,
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

    /// Override the generated id (e.g. deterministic ids like `skill:<name>`).
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// Override created/updated timestamps (e.g. to backdate synthetic entries).
    pub fn with_timestamps(mut self, created_at: DateTime<Utc>, updated_at: DateTime<Utc>) -> Self {
        self.created_at = created_at;
        self.updated_at = updated_at;
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

    /// Set embedding vector together with the model id that produced it.
    pub fn with_embedding_for_model(
        mut self,
        embedding: Vec<f32>,
        model: impl Into<String>,
    ) -> Self {
        self.embedding = Some(embedding);
        self.embedding_model = Some(model.into());
        self
    }

    /// Set or clear the embedding and its model id together, keeping the two
    /// fields consistent.
    pub fn set_embedding(&mut self, embedding: Option<Vec<f32>>, model: Option<String>) {
        self.embedding = embedding;
        self.embedding_model = model;
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
            _ => MemoryCategory::Fact,
        }
    }
}

use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryScope {
    Project,
    Global,
    All,
}

impl MemoryScope {
    pub fn includes_project(self) -> bool {
        matches!(self, Self::Project | Self::All)
    }

    pub fn includes_global(self) -> bool {
        matches!(self, Self::Global | Self::All)
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
            .filter(|entry| &entry.category == category)
            .collect()
    }

    pub fn search(&self, query: &str) -> Vec<&MemoryEntry> {
        let query_lower = normalize_search_text(query);
        if query_lower.is_empty() {
            return Vec::new();
        }

        self.entries
            .iter()
            .filter(|entry| memory_matches_search(entry, &query_lower))
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<&MemoryEntry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    pub fn remove(&mut self, id: &str) -> Option<MemoryEntry> {
        if let Some(pos) = self.entries.iter().position(|entry| entry.id == id) {
            Some(self.entries.remove(pos))
        } else {
            None
        }
    }

    pub fn get_relevant(&self, limit: usize) -> Vec<&MemoryEntry> {
        ranking::top_k_by_score(
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

pub fn memory_score(entry: &MemoryEntry) -> f64 {
    if !entry.active {
        return 0.0;
    }

    let mut score = 0.0;
    let age_hours = (Utc::now() - entry.updated_at).num_hours() as f64;
    score += 100.0 / (1.0 + age_hours / 24.0);
    score += (entry.access_count as f64).sqrt() * 10.0;
    score += match entry.category {
        MemoryCategory::Correction => 50.0,
        MemoryCategory::Preference => 30.0,
        MemoryCategory::Fact => 20.0,
        MemoryCategory::Entity => 10.0,
        MemoryCategory::Custom(_) => 5.0,
    };
    score *= match entry.trust {
        TrustLevel::High => 1.5,
        TrustLevel::Medium => 1.0,
        TrustLevel::Low => 0.7,
    };
    score += (entry.strength as f64).ln() * 5.0;
    score
}

fn selected_entries_for_prompt(entries: &[MemoryEntry], limit: usize) -> Vec<&MemoryEntry> {
    let mut selected = Vec::new();
    let mut seen_content = HashSet::new();

    for entry in entries.iter().filter(|entry| entry.active) {
        if selected.len() >= limit {
            break;
        }

        let dedupe_key = entry
            .content
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        if dedupe_key.is_empty() || !seen_content.insert(dedupe_key) {
            continue;
        }

        selected.push(entry);
    }

    selected
}

/// Provenance citation for a memory: the exact source span the fact was
/// drawn from. A tripwire, not a judge: it answers whether the cited lines
/// changed, never whether the meaning changed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SourceCitation {
    /// Repo-relative path (e.g. `src/agent/turn_loops.rs`). Absolute paths
    /// and any `..` component are rejected at verify time.
    pub path: String,
    /// 0-based start line of the span (inclusive).
    pub start_line: usize,
    /// 0-based end line of the span (exclusive).
    pub end_line: usize,
    /// sha256 hex of the EXACT span bytes (lines joined with a single
    /// `\n`, no trailing newline). No normalization: a byte change inside
    /// a string literal or indentation in a whitespace-sensitive language
    /// is a real change and must read as one.
    pub span_hash: String,
    /// The span text at bank time (lines joined with `\n`). Used to find
    /// a moved block by content search when offsets shift.
    pub span_text: String,
    /// Up to 5 lines immediately before the span at bank time (joined with
    /// `\n`). Relocation context only.
    #[serde(default)]
    pub context_before: String,
    /// Up to 5 lines immediately after the span at bank time (joined with
    /// `\n`). Relocation context only.
    #[serde(default)]
    pub context_after: String,
}

/// Lines of file context stored each side of a cited span for relocation.
pub const CITATION_CONTEXT_LINES: usize = 5;

/// Outcome of checking a citation against the current tree.
#[derive(Debug, Clone, PartialEq)]
pub enum CitationStatus {
    /// Span bytes hash equal at the stored offsets.
    Fresh,
    /// Span moved; content found at the returned offsets. Offsets are NOT
    /// persisted (the entry is never rewritten by verification); the fresh
    /// location is used for this check only.
    Relocated { start_line: usize, end_line: usize },
    /// Span content no longer matches anywhere in the file.
    Stale,
    /// File missing, unreadable, or the path escapes the repo root.
    /// Treated as no mark (never blocks recall).
    Unverifiable,
}

impl SourceCitation {
    /// sha256 hex of lines joined with a single `\n`, no trailing newline.
    pub fn hash_span_lines(lines: &[&str]) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(lines.join("\n"));
        format!("{:x}", hasher.finalize())
    }

    /// Confine a repo-relative path under `repo_root`. Rejects absolute
    /// paths and any `..` component, then canonicalizes and requires the
    /// result to stay under the canonicalized root (symlink escape safe).
    fn confined_path(&self, repo_root: &std::path::Path) -> Option<std::path::PathBuf> {
        let rel = std::path::Path::new(&self.path);
        if rel.is_absolute()
            || rel.components().any(|c| {
                matches!(
                    c,
                    std::path::Component::ParentDir | std::path::Component::Prefix(_)
                )
            })
        {
            return None;
        }
        let root = repo_root.canonicalize().ok()?;
        let full = root.join(rel).canonicalize().ok()?;
        if full.starts_with(&root) {
            Some(full)
        } else {
            None
        }
    }

    /// Check the citation against the current tree. Pure read: never writes
    /// the entry, never deletes, never blocks.
    pub fn verify(&self, repo_root: &std::path::Path) -> CitationStatus {
        if self.end_line <= self.start_line {
            return CitationStatus::Unverifiable;
        }
        let path = match self.confined_path(repo_root) {
            Some(path) => path,
            None => return CitationStatus::Unverifiable,
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(_) => return CitationStatus::Unverifiable,
        };
        let lines: Vec<&str> = text.lines().collect();
        // Exact window first.
        if self.end_line <= lines.len() {
            let window = &lines[self.start_line..self.end_line];
            if Self::hash_span_lines(window) == self.span_hash {
                return CitationStatus::Fresh;
            }
        }
        // Relocation: exact content search for the banked span lines.
        let span_lines: Vec<&str> = self.span_text.lines().collect();
        if !span_lines.is_empty() && span_lines.len() == self.end_line - self.start_line {
            let n = span_lines.len();
            if n <= lines.len() {
                for start in 0..=lines.len() - n {
                    if lines[start..start + n] == span_lines[..]
                        && Self::hash_span_lines(&lines[start..start + n]) == self.span_hash
                    {
                        return CitationStatus::Relocated {
                            start_line: start,
                            end_line: start + n,
                        };
                    }
                }
            }
        }
        CitationStatus::Stale
    }
}

pub fn format_entries_for_prompt(entries: &[MemoryEntry], limit: usize) -> Option<String> {
    format_entries_for_prompt_with_header(entries, limit, false, false, None)
}

pub fn format_relevant_prompt(entries: &[MemoryEntry], limit: usize) -> Option<String> {
    format_entries_for_prompt(entries, limit).map(|formatted| format!("# Memory\n\n{formatted}"))
}

/// Recall-facing formatter: verifies cited entries against `repo_root` and
/// carries an advisory stale mark on drifted ones. Never withholds a memory.
pub fn format_relevant_prompt_verified(
    entries: &[MemoryEntry],
    limit: usize,
    repo_root: &std::path::Path,
) -> Option<String> {
    format_entries_for_prompt_with_header(entries, limit, false, false, Some(repo_root))
        .map(|formatted| format!("# Memory\n\n{formatted}"))
}

pub fn format_relevant_display_prompt(entries: &[MemoryEntry], limit: usize) -> Option<String> {
    format_entries_for_prompt_with_header(entries, limit, true, true, None)
}

/// Advisory stale mark for a cited memory, or `None` when no mark applies.
/// Fresh, relocated, unverifiable, and uncited memories render unmarked:
/// staleness never blocks recall, it only informs the model that the source
/// changed since banking so it can re-verify before relying on the fact.
pub fn citation_stale_mark(
    entry: &MemoryEntry,
    repo_root: Option<&std::path::Path>,
) -> Option<String> {
    let citation = entry.citation.as_ref()?;
    let root = repo_root?;
    if citation.verify(root) == CitationStatus::Stale {
        Some(format!(
            "  (source changed since this was recorded: {} — verify before relying on it)",
            citation.path
        ))
    } else {
        None
    }
}

fn format_entries_for_prompt_with_header(
    entries: &[MemoryEntry],
    limit: usize,
    include_header: bool,
    include_updated_at_comments: bool,
    repo_root: Option<&std::path::Path>,
) -> Option<String> {
    let mut sections: HashMap<MemoryCategory, Vec<&MemoryEntry>> = HashMap::new();

    for entry in selected_entries_for_prompt(entries, limit) {
        sections
            .entry(entry.category.clone())
            .or_default()
            .push(entry);
    }

    if sections.is_empty() {
        return None;
    }

    let mut output = String::new();
    let order = [
        MemoryCategory::Correction,
        MemoryCategory::Fact,
        MemoryCategory::Preference,
        MemoryCategory::Entity,
    ];

    let mut write_section = |title: &str, items: Vec<&MemoryEntry>| {
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(&format!("## {title}\n"));
        for (idx, item) in items.into_iter().enumerate() {
            output.push_str(&format!("{}. {}\n", idx + 1, item.content.trim()));
            if let Some(mark) = citation_stale_mark(item, repo_root) {
                output.push_str(&mark);
                output.push('\n');
            }
            if include_updated_at_comments {
                output.push_str(&format!(
                    "<!-- updated_at: {} -->\n",
                    item.updated_at.to_rfc3339()
                ));
            }
        }
    };

    for cat in &order {
        if let Some(items) = sections.remove(cat) {
            let title = match cat {
                MemoryCategory::Correction => "Corrections",
                MemoryCategory::Fact => "Facts",
                MemoryCategory::Preference => "Preferences",
                MemoryCategory::Entity => "Entities",
                MemoryCategory::Custom(_) => "Custom",
            };
            write_section(title, items);
        }
    }

    let mut custom_sections: BTreeMap<String, Vec<&MemoryEntry>> = BTreeMap::new();
    for (cat, items) in sections {
        match cat {
            MemoryCategory::Custom(name) => {
                custom_sections.insert(name, items);
            }
            other => {
                custom_sections.insert(other.to_string(), items);
            }
        }
    }
    for (name, items) in custom_sections {
        write_section(&name, items);
    }

    if output.is_empty() {
        None
    } else if include_header {
        Some(format!("# Memory\n\n{}", output.trim()))
    } else {
        Some(output.trim().to_string())
    }
}

pub fn normalize_search_text(text: &str) -> String {
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

pub fn is_skill_memory(entry: &MemoryEntry) -> bool {
    entry.id.starts_with("skill:")
        || entry.source.as_deref() == Some("skill_registry")
        || matches!(
            &entry.category,
            MemoryCategory::Custom(name) if name.eq_ignore_ascii_case("Skills")
        )
}

pub fn collect_skill_query_terms(query_text: &str) -> HashSet<String> {
    const STOPWORDS: &[&str] = &[
        "about", "after", "before", "could", "from", "have", "just", "make", "ready", "should",
        "start", "that", "their", "there", "they", "this", "what", "when", "where", "which",
        "while", "will", "with", "work", "would", "your",
    ];

    let normalized = normalize_search_text(query_text);
    normalized
        .split_whitespace()
        .filter(|term| term.len() >= 4)
        .filter(|term| !STOPWORDS.contains(term))
        .map(str::to_string)
        .collect()
}

pub fn skill_retrieval_bonus(entry: &MemoryEntry, query_terms: &HashSet<String>) -> f32 {
    if !is_skill_memory(entry) || query_terms.is_empty() {
        return 0.0;
    }

    let searchable = entry.searchable_text();
    let overlap = query_terms
        .iter()
        .filter(|term| searchable.contains(term.as_str()))
        .count();

    match overlap {
        0 | 1 => 0.0,
        2 => 0.08,
        3 => 0.14,
        _ => 0.20,
    }
}

pub fn normalize_memory_search_text(content: &str, tags: &[String]) -> String {
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

pub fn memory_matches_search(memory: &MemoryEntry, normalized_query: &str) -> bool {
    memory.searchable_text().contains(normalized_query)
}

pub mod ranking {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;

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

    pub fn top_k_by_score<T, I>(items: I, limit: usize) -> Vec<(T, f32)>
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

    pub fn top_k_by_ord<T, K, I>(items: I, limit: usize) -> Vec<(T, K)>
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

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::{
            CitationStatus, MemoryCategory, MemoryEntry, SourceCitation, format_relevant_prompt,
            format_relevant_prompt_verified,
        };

        #[test]
        fn top_k_by_score_keeps_highest_scores_in_order() {
            let ranked = top_k_by_score([("a", 1.0), ("b", 3.0), ("c", 2.0)], 2);
            assert_eq!(ranked, vec![("b", 3.0), ("c", 2.0)]);
        }

        #[test]
        fn top_k_by_ord_keeps_highest_keys_in_order() {
            let ranked = top_k_by_ord([("a", 1), ("b", 3), ("c", 2)], 2);
            assert_eq!(ranked, vec![("b", 3), ("c", 2)]);
        }

        #[test]
        fn top_k_zero_limit_is_empty() {
            assert!(top_k_by_score([("a", 1.0)], 0).is_empty());
            assert!(top_k_by_ord([("a", 1)], 0).is_empty());
        }

        fn write_tree(files: &[(&str, &str)]) -> tempfile::TempDir {
            let dir = tempfile::TempDir::new().expect("temp dir");
            for (name, content) in files {
                std::fs::write(dir.path().join(name), content).expect("write file");
            }
            dir
        }

        fn cite(path: &str, start: usize, end: usize, text: &str) -> SourceCitation {
            let lines: Vec<&str> = text.lines().collect();
            let span: Vec<&str> = lines[start..end].to_vec();
            SourceCitation {
                path: path.to_string(),
                start_line: start,
                end_line: end,
                span_hash: SourceCitation::hash_span_lines(&span),
                span_text: span.join("\n"),
                context_before: String::new(),
                context_after: String::new(),
            }
        }

        fn cited_entry(path: &str, start: usize, end: usize, text: &str) -> MemoryEntry {
            let mut entry = MemoryEntry::new(MemoryCategory::Fact, "banked fact");
            entry.citation = Some(cite(path, start, end, text));
            entry
        }

        const SAMPLE: &str = "line0\nfn alpha() {\n    let x = 1;\n}\nline4\n";

        #[test]
        fn citation_fresh_on_unchanged_span() {
            let dir = write_tree(&[("a.rs", SAMPLE)]);
            let entry = cited_entry("a.rs", 1, 4, SAMPLE);
            assert_eq!(
                entry.citation.as_ref().unwrap().verify(dir.path()),
                CitationStatus::Fresh
            );
        }

        #[test]
        fn citation_stale_on_content_edit() {
            let dir = write_tree(&[("a.rs", SAMPLE)]);
            let entry = cited_entry("a.rs", 1, 4, SAMPLE);
            std::fs::write(
                dir.path().join("a.rs"),
                SAMPLE.replace("let x = 1;", "let x = 2;"),
            )
            .unwrap();
            assert_eq!(
                entry.citation.as_ref().unwrap().verify(dir.path()),
                CitationStatus::Stale
            );
        }

        #[test]
        fn citation_stale_on_whitespace_only_edit() {
            // Exact bytes: an indentation change is a real change.
            let dir = write_tree(&[("a.rs", SAMPLE)]);
            let entry = cited_entry("a.rs", 1, 4, SAMPLE);
            std::fs::write(
                dir.path().join("a.rs"),
                SAMPLE.replace("    let x", "  let x"),
            )
            .unwrap();
            assert_eq!(
                entry.citation.as_ref().unwrap().verify(dir.path()),
                CitationStatus::Stale
            );
        }

        #[test]
        fn citation_stale_on_string_literal_edit() {
            let dir = write_tree(&[("s.py", "a = 1\nname = \"old\"\nb = 2\n")]);
            let text = "a = 1\nname = \"old\"\nb = 2\n";
            let entry = cited_entry("s.py", 1, 2, text);
            std::fs::write(dir.path().join("s.py"), text.replace("old", "new")).unwrap();
            assert_eq!(
                entry.citation.as_ref().unwrap().verify(dir.path()),
                CitationStatus::Stale
            );
        }

        #[test]
        fn citation_relocated_when_block_moves() {
            let dir = write_tree(&[("a.rs", SAMPLE)]);
            let entry = cited_entry("a.rs", 1, 4, SAMPLE);
            let moved = "// header\n// more\n".to_string() + SAMPLE;
            std::fs::write(dir.path().join("a.rs"), &moved).unwrap();
            assert_eq!(
                entry.citation.as_ref().unwrap().verify(dir.path()),
                CitationStatus::Relocated {
                    start_line: 3,
                    end_line: 6
                }
            );
        }

        #[test]
        fn citation_unverifiable_paths() {
            let dir = write_tree(&[("a.rs", SAMPLE)]);
            let entry = cited_entry("a.rs", 1, 4, SAMPLE);
            // Absolute path rejected.
            let mut abs = entry.clone();
            abs.citation.as_mut().unwrap().path =
                dir.path().join("a.rs").to_string_lossy().to_string();
            assert_eq!(
                abs.citation.as_ref().unwrap().verify(dir.path()),
                CitationStatus::Unverifiable
            );
            // Parent traversal rejected.
            let mut trav = entry.clone();
            trav.citation.as_mut().unwrap().path = "../a.rs".to_string();
            assert_eq!(
                trav.citation.as_ref().unwrap().verify(dir.path()),
                CitationStatus::Unverifiable
            );
            // Missing file.
            let mut missing = entry.clone();
            missing.citation.as_mut().unwrap().path = "gone.rs".to_string();
            assert_eq!(
                missing.citation.as_ref().unwrap().verify(dir.path()),
                CitationStatus::Unverifiable
            );
            // Degenerate range.
            let mut degen = entry;
            degen.citation.as_mut().unwrap().end_line = 1;
            assert_eq!(
                degen.citation.as_ref().unwrap().verify(dir.path()),
                CitationStatus::Unverifiable
            );
        }

        #[test]
        fn citation_symlink_escape_rejected() {
            #[cfg(unix)]
            {
                let outside = write_tree(&[("secret.rs", SAMPLE)]);
                let dir = write_tree(&[("a.rs", SAMPLE)]);
                std::os::unix::fs::symlink(
                    outside.path().join("secret.rs"),
                    dir.path().join("link.rs"),
                )
                .unwrap();
                let entry = cited_entry("link.rs", 1, 4, SAMPLE);
                assert_eq!(
                    entry.citation.as_ref().unwrap().verify(dir.path()),
                    CitationStatus::Unverifiable
                );
            }
        }

        #[test]
        fn verified_prompt_marks_only_stale() {
            let dir = write_tree(&[("a.rs", SAMPLE)]);
            let fresh = cited_entry("a.rs", 1, 4, SAMPLE);
            let mut stale = cited_entry("a.rs", 1, 4, SAMPLE);
            stale.content = "drifted fact".to_string();
            std::fs::write(
                dir.path().join("a.rs"),
                SAMPLE.replace("let x = 1;", "let x = 9;"),
            )
            .unwrap();
            let plain = MemoryEntry::new(MemoryCategory::Fact, "plain fact");
            // After the edit both cited entries verify Stale; relocate the
            // fresh one's target back so it reads Fresh.
            std::fs::write(dir.path().join("a.rs"), SAMPLE).unwrap();
            let out = format_relevant_prompt_verified(
                &[fresh.clone(), stale.clone(), plain.clone()],
                10,
                dir.path(),
            )
            .expect("prompt");
            assert!(!out.contains("source changed"), "fresh + plain unmarked");
            // Now drift again: only the mark path matters.
            std::fs::write(
                dir.path().join("a.rs"),
                SAMPLE.replace("let x = 1;", "let x = 9;"),
            )
            .unwrap();
            let out = format_relevant_prompt_verified(&[stale], 10, dir.path()).expect("prompt");
            assert!(out.contains("drifted fact"));
            assert!(out.contains("source changed since this was recorded: a.rs"));
        }

        #[test]
        fn relocated_entry_renders_unmarked() {
            let dir = write_tree(&[("a.rs", SAMPLE)]);
            let entry = cited_entry("a.rs", 1, 4, SAMPLE);
            let moved = "// header\n".to_string() + SAMPLE;
            std::fs::write(dir.path().join("a.rs"), &moved).unwrap();
            let out = format_relevant_prompt_verified(std::slice::from_ref(&entry), 10, dir.path())
                .expect("prompt");
            assert!(!out.contains("source changed"));
        }

        #[test]
        fn uncited_entries_load_and_recall_unchanged() {
            // Old JSON without `citation` loads with None (no migration).
            let json = r#"{"id":"m1","category":"fact","content":"c","tags":[],"search_text":"","created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z","access_count":0}"#;
            let entry: MemoryEntry = serde_json::from_str(json).expect("old json loads");
            assert!(entry.citation.is_none());
            // Unverified formatter ignores citations entirely.
            let out = format_relevant_prompt(std::slice::from_ref(&entry), 10).expect("prompt");
            assert!(out.contains('c'));
            assert!(!out.contains("source changed"));
        }
    }
}
