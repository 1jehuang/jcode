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

/// Provenance tier: WHO produced this memory (R10).
///
/// Orthogonal to [`TrustLevel`] (which answers HOW reliable the content is):
/// a user-stated fact and a bulk tool-imported fact can share `TrustLevel`
/// but differ in provenance. Drives the tool-ingested quarantine switch
/// ([`tool_ingested_quarantine_on`] / [`recall_visible`]).
///
/// Serde-defaulted to [`Provenance::AgentDistilled`] so every legacy row
/// (sidecar extraction, memory-tool remember, goals, bench corpora) loads
/// quarantine-exempt; only explicitly-marked bulk imports ever fall under
/// quarantine.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum Provenance {
    /// Directly stated by the user (highest provenance tier).
    User,
    /// Distilled by the agent from conversation (sidecar extraction,
    /// memory-tool remember, goal sync). The default: all legacy rows.
    #[default]
    AgentDistilled,
    /// Bulk-ingested from tool output (imports, scrapes). Subject to
    /// quarantine when `JCODE_MEMORY_QUARANTINE_TOOL_INGESTED=1`.
    ToolIngested,
}

/// Whether tool-ingested quarantine is on (R10).
///
/// Exact-match env `JCODE_MEMORY_QUARANTINE_TOOL_INGESTED=1` only, default
/// off. Lives in memory-types so the manager, the Jev prefilter, and the
/// bench read one flag.
pub fn tool_ingested_quarantine_on() -> bool {
    std::env::var("JCODE_MEMORY_QUARANTINE_TOOL_INGESTED")
        .map(|v| v == "1")
        .unwrap_or(false)
}

/// Recall visibility (R10): an entry is recall-visible iff it is live
/// (`active`) and NOT quarantined (quarantine on AND [`Provenance::ToolIngested`]).
///
/// Apply at recall points ONLY — never collection: `list_all` must still
/// show quarantined rows (auditability), and quarantine is a recall filter,
/// not erasure.
pub fn recall_visible(entry: &MemoryEntry) -> bool {
    entry.active && !(tool_ingested_quarantine_on() && entry.provenance == Provenance::ToolIngested)
}

/// Secret-bearing detector (R11 privacy pass).
///
/// Scans entry content + tags (lowercased; no `regex` dep in memory-types)
/// for three signal classes:
///
/// 1. Generic markers (`password`, `passwd`, `pwd`, `secret`, `token`,
///    `auth`, `credential`, spaced `private key` / `api key`) fire ONLY on
///    assignment adjacency: the marker, then optional whitespace, then `:`
///    or `=`, then 3+ value chars. Bare words and `is`-forms ("password is
///    river-stone-77") NEVER fire — the dev bench corpus holds such golds
///    and must stay silent (see `detector_silent_on_normal_content`).
/// 2. Compound markers (`api_key`, `apikey`, `secret_key`, `client_secret`,
///    `auth_token`, `access_token`, `refresh_token`, `private_key`,
///    `privatekey`, `session_token`) fire standalone.
/// 3. Known prefixes (`sk-`, `ghp_`, `gho_`, `akia`, `xox`+letter/`-`,
///    `-----begin` paired with `private`) and high-entropy runs
///    (contiguous `[A-Za-z0-9+/=_-]{20,}` with Shannon entropy >= 4.0
///    bits/char) fire standalone.
///
/// Down-ranks via [`safety_penalty`]; never deletes, never hides. A
/// penalized row is a CANDIDATE for hard removal (see
/// `docs/MEMORY_FORGET_GUIDANCE.md`), not an automatic one.
pub fn contains_secret(entry: &MemoryEntry) -> bool {
    let mut text = entry.content.to_lowercase();
    text.push('\n');
    for tag in &entry.tags {
        text.push_str(&tag.to_lowercase());
        text.push('\n');
    }
    let bytes = text.as_bytes();

    // Class 3a: known prefixes (standalone).
    for prefix in ["sk-", "ghp_", "gho_", "akia"] {
        if text.contains(prefix) {
            return true;
        }
    }
    if text.contains("-----begin") && text.contains("private") {
        return true;
    }
    // Slack-style tokens: xox + letter(s) + dash (xoxb-, xoxp-, ...).
    let mut rest = text.as_str();
    while let Some(pos) = rest.find("xox") {
        let after: Vec<char> = rest[pos + 3..].chars().take(8).collect();
        let mut letters = 0usize;
        for c in &after {
            if c.is_ascii_alphabetic() {
                letters += 1;
            } else {
                break;
            }
        }
        if letters >= 1 && after.get(letters) == Some(&'-') {
            return true;
        }
        rest = &rest[pos + 3..];
    }

    // Class 2: compound markers (standalone, substring match).
    for marker in [
        "api_key",
        "apikey",
        "secret_key",
        "client_secret",
        "auth_token",
        "access_token",
        "refresh_token",
        "private_key",
        "privatekey",
        "session_token",
    ] {
        if text.contains(marker) {
            return true;
        }
    }

    // Class 1: generic markers, assignment-adjacency only.
    for marker in [
        "password",
        "passwd",
        "pwd",
        "secret",
        "token",
        "auth",
        "credential",
        "private key",
        "api key",
    ] {
        if marker_adjacent_assignment(bytes, marker.as_bytes()) {
            return true;
        }
    }

    // Class 3b: high-entropy long-token heuristic.
    if has_high_entropy_run(bytes) {
        return true;
    }

    false
}

/// Generic-marker adjacency: `marker` (already lowercase) preceded by
/// start/non-alphanumeric, followed by optional spaces/tabs, then `:` or
/// `=`, then 3+ non-space value chars. `is`-forms never match (only `:`
/// and `=` count).
fn marker_adjacent_assignment(haystack: &[u8], marker: &[u8]) -> bool {
    if marker.is_empty() || haystack.len() < marker.len() {
        return false;
    }
    let mut start = 0;
    while start + marker.len() <= haystack.len() {
        let found = haystack[start..]
            .windows(marker.len())
            .position(|w| w == marker);
        let pos = match found {
            Some(p) => start + p,
            None => return false,
        };
        // Left boundary: start or non-alphanumeric (allows `client_secret`
        // style compounds to reach the adjacency check too).
        let left_ok = pos == 0 || !haystack[pos - 1].is_ascii_alphanumeric();
        if left_ok {
            let mut i = pos + marker.len();
            while i < haystack.len() && (haystack[i] == b' ' || haystack[i] == b'\t') {
                i += 1;
            }
            if i < haystack.len() && (haystack[i] == b':' || haystack[i] == b'=') {
                i += 1;
                while i < haystack.len() && (haystack[i] == b' ' || haystack[i] == b'\t') {
                    i += 1;
                }
                let value_start = i;
                while i < haystack.len()
                    && haystack[i] != b' '
                    && haystack[i] != b'\t'
                    && haystack[i] != b'\n'
                {
                    i += 1;
                }
                if i - value_start >= 3 {
                    return true;
                }
            }
        }
        start = pos + 1;
    }
    false
}

/// High-entropy run: a contiguous `[A-Za-z0-9+/=_-]{20,}` run whose
/// Shannon entropy is >= 4.0 bits/char. The length gate alone silences the
/// dev corpus (longest run 16 chars); entropy is belt-and-braces.
fn has_high_entropy_run(bytes: &[u8]) -> bool {
    fn in_run_class(b: u8) -> bool {
        b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=' || b == b'_' || b == b'-'
    }
    let mut i = 0;
    while i < bytes.len() {
        if !in_run_class(bytes[i]) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < bytes.len() && in_run_class(bytes[j]) {
            j += 1;
        }
        if j - i >= 20 {
            let run = &bytes[i..j];
            let mut counts = [0u32; 256];
            for &b in run {
                counts[b as usize] += 1;
            }
            let len = run.len() as f64;
            let entropy: f64 = counts
                .iter()
                .filter(|&&c| c > 0)
                .map(|&c| {
                    let p = c as f64 / len;
                    -p * p.log2()
                })
                .sum();
            if entropy >= 4.0 {
                return true;
            }
        }
        i = j;
    }
    false
}

/// Multiplicative safety penalty (R11): 0.5 when the entry bears a secret
/// ([`contains_secret`]), else 1.0. Lowers rank, never deletes. Applied in
/// [`memory_score`] and at the `score_and_filter` / `hybrid_prefilter_rank`
/// scoring sites in jcode-base.
pub fn safety_penalty(entry: &MemoryEntry) -> f32 {
    if contains_secret(entry) { 0.5 } else { 1.0 }
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
    /// Provenance tier: who produced this memory (R10). Serde-defaulted to
    /// `AgentDistilled` so legacy rows load quarantine-exempt.
    #[serde(default)]
    pub provenance: Provenance,
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
    /// Recall reinforcement count: strengthened ONLY on judge-verified
    /// surfacing (kept-set from Jev select in `get_relevant_parallel`).
    /// Keyword fallback and bench paths never touch this. Serde-defaulted:
    /// old rows load as zero (never recalled under this scheme).
    /// Record-only: no ranking consumer reads this yet (R4).
    #[serde(default)]
    pub recall_count: u32,
    /// Last judge-verified recall timestamp. Serde-defaulted: old rows
    /// load as never. Record-only (R4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_recalled_at: Option<DateTime<Utc>>,
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
            provenance: Provenance::default(),
            strength: 1,
            active: true,
            superseded_by: None,
            reinforcements: Vec::new(),
            embedding: None,
            embedding_model: None,
            confidence: 1.0,
            citation: None,
            recall_count: 0,
            last_recalled_at: None,
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

    pub fn with_citation(mut self, citation: SourceCitation) -> Self {
        self.citation = Some(citation);
        self
    }

    pub fn with_trust(mut self, trust: TrustLevel) -> Self {
        self.trust = trust;
        self
    }

    /// Override the provenance tier (R10). Banking assignment: memory-tool
    /// `remember` and sidecar extraction bank `AgentDistilled` (also the
    /// default); user-set goals bank `User`; bulk tool imports bank
    /// `ToolIngested`.
    pub fn with_provenance(mut self, provenance: Provenance) -> Self {
        self.provenance = provenance;
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

    /// Record one judge-verified surfacing (R4 recall-count reinforcement).
    /// Call ONLY for the kept-set from Jev select — never for keyword
    /// fallback or bench-path surfacing. Record-only: no ranking consumer
    /// reads these fields yet. `updated_at` is deliberately untouched:
    /// recall is not a content modification.
    pub fn mark_recalled(&mut self) {
        self.recall_count = self.recall_count.saturating_add(1);
        self.last_recalled_at = Some(Utc::now());
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
                // R10: quarantined tool-ingested rows are invisible to recall
                // (but still listed by collection paths).
                .filter(|entry| recall_visible(entry))
                .map(|entry| (entry, memory_score(entry) as f32)),
            limit,
            |entry| entry.id.as_str(),
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
    // R11 safety penalty: multiplicative 0.5 when secret-bearing, else 1.0.
    // Lowers rank, never deletes; silent (1.0) on all normal content.
    score *= f64::from(safety_penalty(entry));
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

/// Walk up from `start` to the enclosing repository root (a directory
/// containing `.git`). Falls back to `start` itself for non-git trees so
/// callers keep working without a repository. Used to anchor citation paths
/// so nested working directories bank and verify against the same root.
pub fn find_repo_root(start: &std::path::Path) -> std::path::PathBuf {
    let mut cursor = if start.is_file() {
        start.parent().map(|p| p.to_path_buf())
    } else {
        Some(start.to_path_buf())
    };
    while let Some(dir) = cursor {
        if dir.join(".git").exists() {
            return dir;
        }
        match dir.parent() {
            Some(parent) if parent != dir => cursor = Some(parent.to_path_buf()),
            _ => break,
        }
    }
    start.to_path_buf()
}

/// Upstream remote URL for `root` (`remote.origin.url`), best-effort, with
/// credentials stripped. HTTPS remotes can embed `user:token@` userinfo;
/// persisting that would leak secrets into memory JSON and model prompts.
/// Two clones share history (same root commit) but not provenance: the
/// clone's origin points at its source, the original's at upstream. Moves
/// preserve it; re-clones from the same upstream reproduce it. Absent
/// outside git.
pub fn repo_origin(root: &std::path::Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["config", "--get", "remote.origin.url"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if url.is_empty() {
        None
    } else {
        Some(strip_url_credentials(&url))
    }
}

/// Remove `user[:password]@` userinfo from a remote URL. Malformed input
/// passes through unchanged (fail-soft: identity, never auth).
fn strip_url_credentials(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let Some((userinfo, host)) = rest.split_once('@') else {
        return url.to_string();
    };
    // Only strip when the part before @ holds no '/': userinfo never
    // contains one, so a slash means the @ belongs to the path.
    if userinfo.contains('/') {
        return url.to_string();
    }
    format!("{scheme}://{host}")
}

/// Stable repository identity for `root`: `git:<root-commit-hash>` when
/// determinable, else `path:<canonical-root>`. The root commit survives
/// renames and moves of the checkout (same .git, same history); the path
/// fallback preserves behavior for non-git trees and missing git.
pub fn repo_identity(root: &std::path::Path) -> String {
    let git_id = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-list", "--max-parents=0", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|h| !h.is_empty())
        .map(|h| format!("git:{h}"));
    git_id.unwrap_or_else(|| {
        let path = root
            .canonicalize()
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.to_string_lossy().into_owned());
        format!("path:{path}")
    })
}

/// Best-effort short git HEAD for `root` (`rev-parse --short HEAD`).
/// `None` outside a repo, without git, or on any failure: revision is
/// advisory metadata, never a banking requirement.
pub fn git_head_for(root: &std::path::Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let head = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if head.is_empty() { None } else { Some(head) }
}

/// Provenance citation for a memory: the exact source span the fact was
/// drawn from. A tripwire, not a judge: it answers whether the cited lines
/// changed, never whether the meaning changed.
///
/// Design follows the evidence-object pattern (source id + revision +
/// passage + locator; cf. VeriCite 2025 deterministic quote-matching):
/// the citation binds path, bytes, originating repository, and revision,
/// and verification is deterministic — no model call, no fuzzy matching.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SourceCitation {
    /// Repo-relative path (e.g. `src/agent/turn_loops.rs`). Absolute paths
    /// and any `..` component are rejected at verify time.
    pub path: String,
    /// 0-based start line of the span (inclusive).
    pub start_line: usize,
    /// 0-based end line of the span (exclusive).
    pub end_line: usize,
    /// sha256 hex of the EXACT span bytes (raw file bytes from the first
    /// byte of start_line through the last byte of end_line-1, line
    /// endings included). No normalization: CRLF/LF swaps, trailing
    /// newlines, string-literal bytes, and whitespace-sensitive indentation
    /// all read as real changes.
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
    /// Stable repo identity at bank time (`git:<root-commit>` normally,
    /// `path:<canonical-root>` fallback). Verified on read: a global memory
    /// resolved under a different repo reads as Unverifiable, while a mere
    /// checkout move keeps verifying. Empty means legacy/unknown: skipped.
    #[serde(default)]
    pub repo_id: String,
    /// Upstream remote at bank time. Clones share the root commit but not
    /// the origin (a clone's origin points at its source): when the history
    /// matches but the origin doesn't, the checkout is foreign and the mark
    /// says so instead of silently presenting the fact as locally backed.
    /// None means legacy/unknown or no remote: no foreign check.
    #[serde(default)]
    pub origin: Option<String>,
    /// Short git HEAD at bank time (`rev-parse --short HEAD`), best-effort.
    /// Advisory only: when the tree moved on, the stale mark names both
    /// revisions so the model knows how far behind the memory is.
    #[serde(default)]
    pub git_head: Option<String>,
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
    /// sha256 hex of raw bytes. The citation hash covers the exact span
    /// bytes (line endings included): LF-to-CRLF conversion or a
    /// trailing-newline add/remove changes the hash and reads as Stale.
    pub fn hash_span_bytes(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        format!("{:x}", hasher.finalize())
    }

    /// sha256 hex of lines joined with a single `\n`. Used only as a fast
    /// prefilter for relocation search (normalization-tolerant); the verdict
    /// always confirms against the raw-byte hash.
    pub fn hash_span_lines(lines: &[&str]) -> String {
        Self::hash_span_bytes(lines.join("\n").as_bytes())
    }

    /// Byte offset where each line starts, plus EOF as the final sentinel,
    /// so a line span maps to an exact byte window (line endings intact).
    fn line_byte_offsets(text: &str) -> Vec<usize> {
        let mut offsets = vec![0];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                offsets.push(i + 1);
            }
        }
        if *offsets.last().unwrap() != text.len() {
            offsets.push(text.len());
        }
        offsets
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
        // A global memory banked in another repo must not verify against an
        // unrelated file that happens to share the relative path. Empty
        // repo_id means legacy/unknown: skip the check.
        if !self.repo_id.is_empty() && repo_identity(repo_root) != self.repo_id {
            return CitationStatus::Unverifiable;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(_) => return CitationStatus::Unverifiable,
        };
        let lines: Vec<&str> = text.lines().collect();
        // Persisted citations are untrusted input (hand-edited JSON, older
        // versions): validate stored bounds against the live file BEFORE any
        // arithmetic or indexing. end_line = usize::MAX overflowed
        // `end_line + 1` and indexed the offset table out of bounds.
        if self.end_line > lines.len() {
            return CitationStatus::Unverifiable;
        }
        let offsets = Self::line_byte_offsets(&text);
        // Exact window first, on raw bytes (line-ending faithful). Safe now:
        // end_line <= lines.len() < usize::MAX, so end_line + 1 cannot
        // overflow and both offsets indices are in range.
        if self.end_line + 1 <= offsets.len() {
            let window = &text.as_bytes()[offsets[self.start_line]..offsets[self.end_line]];
            if Self::hash_span_bytes(window) == self.span_hash {
                return CitationStatus::Fresh;
            }
        }
        // Relocation: exact content search for the banked span lines. A
        // span match alone is not enough: an identical twin elsewhere must
        // not mask the original's edit. The stored 5-line context each side
        // disambiguates: full context match relocates, span match with
        // context mismatch reads as Stale.
        let span_lines: Vec<&str> = self.span_text.lines().collect();
        if !span_lines.is_empty() && span_lines.len() == self.end_line - self.start_line {
            let n = span_lines.len();
            if n <= lines.len() {
                let before: Vec<&str> = self.context_before.lines().collect();
                let after: Vec<&str> = self.context_after.lines().collect();
                for start in 0..=lines.len() - n {
                    if lines[start..start + n] != span_lines[..] {
                        continue;
                    }
                    let raw = &text.as_bytes()[offsets[start]..offsets[start + n]];
                    if Self::hash_span_bytes(raw) != self.span_hash {
                        continue;
                    }
                    let end = start + n;
                    // Strict: the candidate must offer exactly the stored
                    // context on both sides. A file-start span (empty
                    // stored context) only relocates to file start; a
                    // mid-file twin with different surroundings reads as
                    // Stale, never Relocated. No vacuous empty matches.
                    let before_ok = if before.is_empty() {
                        start == 0
                    } else {
                        start >= before.len() && lines[start - before.len()..start] == before[..]
                    };
                    let after_ok = if after.is_empty() {
                        end == lines.len()
                    } else {
                        lines.len() - end >= after.len()
                            && lines[end..end + after.len()] == after[..]
                    };
                    if before_ok && after_ok {
                        return CitationStatus::Relocated {
                            start_line: start,
                            end_line: end,
                        };
                    }
                }
            }
        }
        CitationStatus::Stale
    }

    /// Bank a citation: confine `path` under `repo_root`, read the file,
    /// hash the exact span bytes, and capture the span text plus 5 lines of
    /// context each side for relocation. Returns `None` (fail-soft: the
    /// memory stores uncited) on any failure — bad range, missing file,
    /// path escape, oversized span.
    pub fn bank(
        repo_root: &std::path::Path,
        path: &str,
        start_line: usize,
        end_line: usize,
    ) -> Option<SourceCitation> {
        if end_line <= start_line || end_line - start_line > 200 {
            return None;
        }
        let probe = SourceCitation {
            path: path.to_string(),
            start_line,
            end_line,
            span_hash: String::new(),
            span_text: String::new(),
            context_before: String::new(),
            context_after: String::new(),
            repo_id: String::new(),
            git_head: None,
            origin: None,
        };
        let full = probe.confined_path(repo_root)?;
        let text = std::fs::read_to_string(&full).ok()?;
        let repo_id = repo_identity(repo_root);
        let git_head = git_head_for(repo_root);
        let origin = repo_origin(repo_root);
        let lines: Vec<&str> = text.lines().collect();
        if end_line > lines.len() {
            return None;
        }
        // Raw span bytes (line endings intact): LF/CRLF and trailing
        // newlines participate in the hash.
        let offsets = Self::line_byte_offsets(&text);
        let span_bytes = &text.as_bytes()[offsets[start_line]..offsets[end_line]];
        let span = &lines[start_line..end_line];
        let from = start_line.saturating_sub(CITATION_CONTEXT_LINES);
        let to = (end_line + CITATION_CONTEXT_LINES).min(lines.len());
        Some(SourceCitation {
            path: path.to_string(),
            start_line,
            end_line,
            span_hash: Self::hash_span_bytes(span_bytes),
            span_text: span.join("\n"),
            context_before: lines[from..start_line].join("\n"),
            context_after: lines[end_line..to].join("\n"),
            repo_id,
            git_head,
            origin,
        })
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

/// R5 stable-vs-situational split: the always-on profile leg. Renders ONLY
/// stable standing instructions (Corrections, then Preferences) as plain
/// numbered lines under a `# Memory Profile` header.
///
/// Cache-stability contract: given an unchanged store the output is
/// byte-identical — no timestamps, no counts, no stale-age marks, total
/// id-order within each category. Deliberately ignores `recall_count` /
/// `last_recalled_at` (R4 record-only fields): surfacing frequency is never
/// standing-instruction content. Facts, entities, and custom categories are
/// situational and stay on the retrieved (episodic) leg.
pub fn format_profile_prompt(entries: &[MemoryEntry], limit: usize) -> Option<String> {
    let mut corrections: Vec<&MemoryEntry> = Vec::new();
    let mut preferences: Vec<&MemoryEntry> = Vec::new();
    for entry in entries {
        match entry.category {
            MemoryCategory::Correction => corrections.push(entry),
            MemoryCategory::Preference => preferences.push(entry),
            _ => {}
        }
    }
    corrections.sort_by(|a, b| a.id.cmp(&b.id));
    preferences.sort_by(|a, b| a.id.cmp(&b.id));
    corrections.truncate(limit);
    preferences.truncate(limit.saturating_sub(corrections.len()));

    if corrections.is_empty() && preferences.is_empty() {
        return None;
    }

    let mut output = String::from("# Memory Profile\n");
    let mut write_section = |title: &str, items: &[&MemoryEntry]| {
        if items.is_empty() {
            return;
        }
        output.push_str(&format!("\n## {title}\n"));
        for (idx, item) in items.iter().enumerate() {
            output.push_str(&format!("{}. {}\n", idx + 1, item.content.trim()));
        }
    };
    write_section("Corrections", &corrections);
    write_section("Preferences", &preferences);
    Some(output.trim_end().to_string())
}

/// True when the citation verifies under matching history but the checkout
/// is foreign: same root commit, different upstream remote (a clone, a fork
/// checkout, a re-pointed remote). The bytes back the fact, but the project
/// context may not — the mark says so instead of silent Fresh.
pub fn is_foreign_checkout(citation: &SourceCitation, repo_root: &std::path::Path) -> bool {
    match (&citation.origin, repo_origin(repo_root)) {
        (Some(banked), Some(current)) => banked != &current,
        _ => false,
    }
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
    if is_foreign_checkout(citation, root)
        && matches!(
            citation.verify(root),
            CitationStatus::Fresh | CitationStatus::Relocated { .. }
        )
    {
        return Some(format!(
            "  (source verified in a different checkout of this history (origin {}) — re-read the file if the project context matters)",
            // Defense in depth: origins are redacted at capture, but a
            // citation banked before the redaction still carries secrets.
            strip_url_credentials(citation.origin.as_deref().unwrap_or("unknown")),
        ));
    }
    if citation.verify(root) == CitationStatus::Stale {
        let commit_note = match (&citation.git_head, git_head_for(root)) {
            (Some(old), Some(new)) if old != &new => {
                format!(" (recorded at {old}, tree now at {new})")
            }
            _ => String::new(),
        };
        Some(format!(
            "  (source changed since this was recorded: {} — re-read the file before relying on it{})",
            citation.path, commit_note
        ))
    } else {
        None
    }
}

/// Advisory age hedge for a recalled memory, or `None` when fresh.
///
/// Memories older than [`AGE_HEDGE_DAYS`] render with their coarse recorded
/// age so the model treats them as possibly outdated and re-verifies against
/// current code before asserting them as fact. Cited memories are exempt:
/// their freshness is already covered by [`citation_stale_mark`], which says
/// something stronger (the source changed, not merely that time passed).
/// Coarse week buckets (not daily counts) so the memory block stays
/// byte-identical across adjacent turns: bytes change ~4x/month instead of
/// daily, conditional on the same retrieved set.
pub const AGE_HEDGE_DAYS: i64 = 14;

pub fn age_hedge_mark(entry: &MemoryEntry) -> Option<String> {
    if entry.citation.is_some() {
        return None;
    }
    let age_days = (Utc::now() - entry.updated_at).num_days();
    if age_days >= AGE_HEDGE_DAYS {
        let weeks = age_days / 7;
        let week_word = if weeks == 1 { "week" } else { "weeks" };
        Some(format!(
            "  (recorded ~{weeks} {week_word} ago — verify against current code before asserting as fact)",
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
            } else if let Some(hedge) = age_hedge_mark(item) {
                output.push_str(&hedge);
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

/// Light plural/possessive folding for the BM25 leg (c2b gate, shipped
/// only if the gate passes). Symmetric application (query AND docs) turns
/// plural/singular mismatches into exact matches. Deliberately NOT Porter:
/// the dense leg owns derivational recall; aggressive stemming on the BM25
/// leg creates false positives that poison RRF fusion.
///
/// Rules (C3 §G1, fixture-validated): strip possessive `'s`; tokens shorter
/// than 5 chars pass through untouched (`glass`, `ties`, `does` classes);
/// `ies->y` (stem len >= 4: `batteries->battery`); strip-2 ONLY for
/// unambiguous `-ches/-shes/-xes/-zes` (`watches->watch`, `boxes->box`);
/// everything else singularizes via trailing `s->` strip
/// (`snapshots->snapshot`, `commands->command`, `licenses->license`),
/// except `-ss` (`access`), `-us` (`analytics`), `-is`/`-ics`.
/// Empty-after-fold tokens are dropped by the caller (no zero-length IDF).
///
/// Used ONLY by `bm25_rank`. Never by `normalize_search_text` callers
/// (`search_scoped`, `get_relevant_keywords`, skill paths, stored
/// `search_text`) - their behavior is unchanged.
pub fn bm25_token_stream(text: &str) -> Vec<String> {
    normalize_search_text(text)
        .split_whitespace()
        .filter_map(|tok| fold_plural_token(tok))
        .collect()
}

/// Fold one normalized token. Returns `None` for empty-after-fold tokens.
fn fold_plural_token(tok: &str) -> Option<String> {
    let mut s = tok.to_string();
    // Possessive strip (inert on ASCII fixtures, real on live input).
    if let Some(stripped) = s.strip_suffix("'s") {
        s = stripped.to_string();
    }
    if s.len() < 5 {
        return if s.is_empty() { None } else { Some(s) };
    }
    // ies -> y (stem guard: batteries->battery, ties untouched by len guard).
    if s.ends_with("ies") && s.len() - 3 >= 4 {
        s.truncate(s.len() - 3);
        s.push('y');
    } else if s.ends_with("ches") || s.ends_with("shes") || s.ends_with("xes") || s.ends_with("zes")
    {
        // Unambiguous -es plurals: watches->watch, boxes->box.
        // NOTE: -ses/-ces/-ges take the strip-1 path below (licenses->license,
        // cases->case); strip-2 there would give licens/cas (buses->buse is
        // the accepted miss - unattested in fixtures).
        s.truncate(s.len() - 2);
    } else if s.ends_with('s')
        && !s.ends_with("ss")
        && !s.ends_with("us")
        && !s.ends_with("is")
        && !s.ends_with("ics")
    {
        s.pop();
    }
    if s.is_empty() { None } else { Some(s) }
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

    /// Heap item for top-k selection. Total order (ascending = worst first):
    /// score ascending, then id DESCENDING (larger id sorts worse), then
    /// ordinal descending (later arrival sorts worse). The id arm fires only
    /// on bitwise score equality, so scores and thresholds are untouched.
    struct TopKItem<T> {
        score: f32,
        id: String,
        ordinal: usize,
        value: T,
    }

    impl<T> PartialEq for TopKItem<T> {
        fn eq(&self, other: &Self) -> bool {
            self.score.to_bits() == other.score.to_bits()
                && self.id == other.id
                && self.ordinal == other.ordinal
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
            // Worst-first: lower score is worse; on ties the LARGER id is
            // worse (so the final descending sort yields id ascending); the
            // ordinal arm keeps Vec-ordered inputs stable (earlier wins).
            self.score
                .total_cmp(&other.score)
                .then_with(|| other.id.cmp(&self.id))
                .then_with(|| other.ordinal.cmp(&self.ordinal))
        }
    }

    /// Deterministic top-k by score: descending score, ascending id on
    /// bitwise score ties, ascending arrival ordinal as the final key.
    /// `id_of` extracts the tiebreak id from each value. Scores are never
    /// modified: the id/ordinal arms fire only when `total_cmp` is Equal.
    pub fn top_k_by_score<T, I, F>(items: I, limit: usize, id_of: F) -> Vec<(T, f32)>
    where
        I: IntoIterator<Item = (T, f32)>,
        F: Fn(&T) -> &str,
    {
        if limit == 0 {
            return Vec::new();
        }

        let mut heap: BinaryHeap<Reverse<TopKItem<T>>> = BinaryHeap::new();

        for (ordinal, (value, score)) in items.into_iter().enumerate() {
            let candidate = Reverse(TopKItem {
                score,
                id: id_of(&value).to_string(),
                ordinal,
                value,
            });

            if heap.len() < limit {
                heap.push(candidate);
                continue;
            }

            // Full-order retention: replace iff the candidate outranks the
            // current worst (score, then smaller-id, then earlier-ordinal).
            // The old `score > smallest.score` gate never replaced on ties,
            // so which tied rows survived truncation depended on HashMap
            // arrival order.
            let replace = heap
                .peek()
                .map(|smallest| candidate.0 > smallest.0)
                .unwrap_or(false);
            if replace {
                heap.pop();
                heap.push(candidate);
            }
        }

        let mut results: Vec<_> = heap
            .into_iter()
            .map(|Reverse(item)| (item.value, item.score, item.id, item.ordinal))
            .collect();
        results.sort_by(|a, b| {
            b.1.total_cmp(&a.1)
                .then_with(|| a.2.cmp(&b.2))
                .then_with(|| a.3.cmp(&b.3))
        });
        results
            .into_iter()
            .map(|(value, score, _, _)| (value, score))
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
            AGE_HEDGE_DAYS, CitationStatus, MemoryCategory, MemoryEntry, Provenance,
            SourceCitation, age_hedge_mark, bm25_token_stream, citation_stale_mark,
            contains_secret, find_repo_root, format_relevant_prompt,
            format_relevant_prompt_verified, git_head_for, is_foreign_checkout, memory_score,
            recall_visible, safety_penalty, strip_url_credentials, tool_ingested_quarantine_on,
        };
        use chrono::{Duration, Utc};

        #[test]
        fn bm25_token_stream_folds_fixture_plurals() {
            // c2b W1 pairs (fixture-attested): folding must converge them.
            for (plural, singular) in [
                ("snapshots", "snapshot"),
                ("commands", "command"),
                ("licenses", "license"),
                ("tiers", "tier"),
                ("starts", "start"),
                ("batteries", "battery"),
            ] {
                let folded = bm25_token_stream(plural);
                assert_eq!(folded, vec![singular.to_string()], "fold {plural}");
                // Symmetric: singular input is idempotent.
                assert_eq!(
                    bm25_token_stream(singular),
                    vec![singular.to_string()],
                    "idempotent {singular}"
                );
            }
        }

        #[test]
        fn bm25_token_stream_keeps_guarded_classes() {
            // min-length + -ss/-us/-is guards (fixture-attested no-mangle set).
            for tok in [
                "glass",
                "access",
                "business",
                "analytics",
                "does",
                "ties",
                "news",
                "tier",
                "command",
            ] {
                assert_eq!(
                    bm25_token_stream(tok),
                    vec![tok.to_string()],
                    "guarded {tok}"
                );
            }
        }

        #[test]
        fn top_k_by_score_keeps_highest_scores_in_order() {
            let ranked = top_k_by_score([("a", 1.0), ("b", 3.0), ("c", 2.0)], 2, |id: &&str| *id);
            assert_eq!(ranked, vec![("b", 3.0), ("c", 2.0)]);
        }

        #[test]
        fn top_k_by_score_breaks_bitwise_ties_by_id_ascending() {
            // Equal scores sort by id ascending; the arm fires only on
            // total_cmp == Equal and never touches the scores.
            let ranked = top_k_by_score([("b", 1.0), ("a", 1.0), ("c", 1.0)], 3, |id: &&str| *id);
            assert_eq!(ranked, vec![("a", 1.0), ("b", 1.0), ("c", 1.0)]);
            // Truncation keeps the smallest ids among ties regardless of
            // arrival order.
            let ranked = top_k_by_score([("c", 1.0), ("b", 1.0), ("a", 1.0)], 2, |id: &&str| *id);
            assert_eq!(ranked, vec![("a", 1.0), ("b", 1.0)]);
        }

        #[test]
        fn top_k_by_ord_keeps_highest_keys_in_order() {
            let ranked = top_k_by_ord([("a", 1), ("b", 3), ("c", 2)], 2);
            assert_eq!(ranked, vec![("b", 3), ("c", 2)]);
        }

        #[test]
        fn top_k_zero_limit_is_empty() {
            assert!(top_k_by_score([("a", 1.0)], 0, |id: &&str| *id).is_empty());
            assert!(top_k_by_ord([("a", 1)], 0).is_empty());
        }

        fn write_tree(files: &[(&str, &str)]) -> tempfile::TempDir {
            let dir = tempfile::TempDir::new().expect("temp dir");
            for (name, content) in files {
                let path = dir.path().join(name);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).expect("mkdir");
                }
                std::fs::write(&path, content).expect("write file");
            }
            dir
        }

        fn cite(path: &str, start: usize, end: usize, text: &str) -> SourceCitation {
            // Mirrors SourceCitation::bank: raw span bytes plus 5 lines each side.
            let lines: Vec<&str> = text.lines().collect();
            let span: Vec<&str> = lines[start..end].to_vec();
            let from = start.saturating_sub(5);
            let to = (end + 5).min(lines.len());
            let offsets = SourceCitation::line_byte_offsets(text);
            SourceCitation {
                path: path.to_string(),
                start_line: start,
                end_line: end,
                span_hash: SourceCitation::hash_span_bytes(
                    &text.as_bytes()[offsets[start]..offsets[end]],
                ),
                span_text: span.join("\n"),
                context_before: lines[from..start].join("\n"),
                context_after: lines[end..to].join("\n"),
                // Tests opt out of repo binding (legacy path); binding is
                // covered by dedicated bank tests below.
                repo_id: String::new(),
                git_head: None,
                origin: None,
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
        fn age_hedge_silent_on_fresh_uncited_memory() {
            let entry = MemoryEntry::new(MemoryCategory::Fact, "fresh fact");
            assert_eq!(age_hedge_mark(&entry), None);
            let prompt = format_relevant_prompt(std::slice::from_ref(&entry), 1).unwrap();
            assert!(!prompt.contains("recorded"), "prompt was: {prompt}");
        }

        #[test]
        fn age_hedge_marks_old_uncited_memory() {
            let old = Utc::now() - Duration::days(20);
            let entry =
                MemoryEntry::new(MemoryCategory::Fact, "old fact").with_timestamps(old, old);
            let mark = age_hedge_mark(&entry).expect("hedge for 20-day-old memory");
            assert!(mark.contains("recorded ~2 weeks ago"), "mark was: {mark}");
            let prompt = format_relevant_prompt(std::slice::from_ref(&entry), 1).unwrap();
            assert!(
                prompt.contains("recorded ~2 weeks ago"),
                "prompt was: {prompt}"
            );
        }

        #[test]
        fn age_hedge_buckets_coarsely_in_weeks() {
            // V3: adjacent ages share a bucket so injection bytes stay
            // identical across nearby turns; bucket edges tick weekly.
            let hedge_for_days_ago = |days: i64| {
                let at = Utc::now() - Duration::days(days);
                let entry =
                    MemoryEntry::new(MemoryCategory::Fact, "old fact").with_timestamps(at, at);
                age_hedge_mark(&entry)
            };
            let two_a = hedge_for_days_ago(14).expect("hedge at threshold");
            let two_b = hedge_for_days_ago(20).expect("hedge mid-bucket");
            assert_eq!(two_a, two_b, "14d and 20d share the ~2-week bucket");
            assert!(two_a.contains("~2 weeks ago"), "mark was: {two_a}");
            let three = hedge_for_days_ago(21).expect("hedge next bucket");
            assert!(three.contains("~3 weeks ago"), "mark was: {three}");
            assert_ne!(two_a, three, "bucket edge must tick at 21d");
            assert_eq!(hedge_for_days_ago(13), None, "under threshold stays silent");
        }

        #[test]
        fn age_hedge_boundary_day() {
            let boundary = Utc::now() - Duration::days(AGE_HEDGE_DAYS);
            let at = MemoryEntry::new(MemoryCategory::Fact, "boundary fact")
                .with_timestamps(boundary, boundary);
            assert!(age_hedge_mark(&at).is_some());
            let just_under = Utc::now() - Duration::days(AGE_HEDGE_DAYS) + Duration::hours(1);
            let under = MemoryEntry::new(MemoryCategory::Fact, "young fact")
                .with_timestamps(just_under, just_under);
            assert_eq!(age_hedge_mark(&under), None);
        }

        #[test]
        fn age_hedge_skips_cited_memory() {
            // Cited memories answer to citation_stale_mark instead: a stale
            // citation says something stronger than an age hedge.
            let old = Utc::now() - Duration::days(60);
            let mut entry = cited_entry("a.rs", 1, 4, SAMPLE);
            entry.updated_at = old;
            assert_eq!(age_hedge_mark(&entry), None);
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
        fn bank_hashes_exact_span_with_context() {
            let dir = write_tree(&[("a.rs", SAMPLE)]);
            let cited = SourceCitation::bank(dir.path(), "a.rs", 1, 4).expect("bank");
            assert_eq!((cited.start_line, cited.end_line), (1, 4));
            assert_eq!(cited.span_text, "fn alpha() {\n    let x = 1;\n}");
            assert_eq!(cited.context_before, "line0");
            assert_eq!(cited.context_after, "line4");
            assert_eq!(cited.verify(dir.path()), CitationStatus::Fresh);
        }

        #[test]
        fn bank_rejects_bad_ranges_and_escapes() {
            let dir = write_tree(&[("a.rs", SAMPLE)]);
            assert!(SourceCitation::bank(dir.path(), "a.rs", 4, 1).is_none());
            assert!(SourceCitation::bank(dir.path(), "a.rs", 0, 99).is_none());
            assert!(SourceCitation::bank(dir.path(), "../a.rs", 1, 2).is_none());
            assert!(SourceCitation::bank(dir.path(), "gone.rs", 1, 2).is_none());
        }

        #[test]
        fn relocation_uses_context_to_break_twin_ties() {
            // Two identical blocks; the cited (first) one is edited. The twin
            // still matches the span text but its context differs, so the
            // verdict must be Stale, not Relocated.
            let text =
                "fn twin() {\n    let v = 1;\n}\n// between\nfn twin() {\n    let v = 1;\n}\n";
            let dir = write_tree(&[("t.rs", text)]);
            let cited = SourceCitation::bank(dir.path(), "t.rs", 4, 7).expect("bank");
            // Edit the CITED (second) block only: line 5 v=1 -> v=9.
            let mut edited_lines: Vec<&str> = text.lines().collect();
            edited_lines[5] = "    let v = 9;";
            let edited = edited_lines.join("\n") + "\n";
            std::fs::write(dir.path().join("t.rs"), &edited).unwrap();
            assert_eq!(cited.verify(dir.path()), CitationStatus::Stale);
        }

        #[test]
        fn relocation_finds_moved_block_with_intact_context() {
            let text =
                "fn twin() {\n    let v = 1;\n}\n// between\nfn other() {\n    let w = 0;\n}\n";
            let dir = write_tree(&[("t.rs", text)]);
            let cited = SourceCitation::bank(dir.path(), "t.rs", 4, 7).expect("bank");
            // Prepend lines: the block moves but its context moves with it.
            let moved = "// h0\n// h1\n".to_string() + text;
            std::fs::write(dir.path().join("t.rs"), &moved).unwrap();
            assert_eq!(
                cited.verify(dir.path()),
                CitationStatus::Relocated {
                    start_line: 6,
                    end_line: 9
                }
            );
        }

        #[test]
        fn find_repo_root_walks_up_and_falls_back() {
            let dir = write_tree(&[("sub/nested/a.rs", "fn a() {}\n")]);
            // No .git: falls back to the start dir itself.
            let nested = dir.path().join("sub").join("nested");
            assert_eq!(find_repo_root(&nested), nested);
            // With .git at top: nested sessions anchor at the root.
            std::fs::create_dir_all(dir.path().join(".git")).unwrap();
            assert_eq!(find_repo_root(&nested), dir.path().to_path_buf());
        }

        #[test]
        fn crlf_to_lf_conversion_reads_stale() {
            let crlf = "a = 1\r\nb = 2\r\n";
            let dir = write_tree(&[("w.py", crlf)]);
            let cited = SourceCitation::bank(dir.path(), "w.py", 0, 2).expect("bank");
            assert_eq!(cited.verify(dir.path()), CitationStatus::Fresh);
            std::fs::write(dir.path().join("w.py"), "a = 1\nb = 2\n").unwrap();
            assert_eq!(cited.verify(dir.path()), CitationStatus::Stale);
        }

        #[test]
        fn trailing_newline_add_remove_reads_stale() {
            let dir = write_tree(&[("t.rs", "fn a() {}\n")]);
            let cited = SourceCitation::bank(dir.path(), "t.rs", 0, 1).expect("bank");
            assert_eq!(cited.verify(dir.path()), CitationStatus::Fresh);
            std::fs::write(dir.path().join("t.rs"), "fn a() {}").unwrap();
            assert_eq!(cited.verify(dir.path()), CitationStatus::Stale);
            // Appending a line AFTER the cited span leaves the span bytes
            // intact: Fresh is correct, the citation covers line 0 only.
            std::fs::write(dir.path().join("t.rs"), "fn a() {}\n\n").unwrap();
            assert_eq!(
                SourceCitation::bank(dir.path(), "t.rs", 0, 1)
                    .expect("re-bank")
                    .verify(dir.path()),
                CitationStatus::Fresh
            );
        }

        #[test]
        fn repo_binding_rejects_foreign_root() {
            let dir_a = write_tree(&[("s.rs", "fn same() {}\n")]);
            let dir_b = write_tree(&[("s.rs", "fn same() {}\n")]);
            let cited = SourceCitation::bank(dir_a.path(), "s.rs", 0, 1).expect("bank");
            assert!(!cited.repo_id.is_empty());
            assert_eq!(cited.verify(dir_a.path()), CitationStatus::Fresh);
            // Identical bytes, different repo: must not verify.
            assert_eq!(cited.verify(dir_b.path()), CitationStatus::Unverifiable);
        }

        #[test]
        fn legacy_citation_without_repo_id_verifies_normally() {
            let dir = write_tree(&[("s.rs", "fn same() {}\n")]);
            let mut cited = SourceCitation::bank(dir.path(), "s.rs", 0, 1).expect("bank");
            cited.repo_id.clear();
            assert_eq!(cited.verify(dir.path()), CitationStatus::Fresh);
        }

        #[test]
        fn old_json_without_identity_fields_loads() {
            let raw = r#"{"path":"s.rs","start_line":0,"end_line":1,"span_hash":"ab","span_text":"x","context_before":"","context_after":""}"#;
            let cited: SourceCitation = serde_json::from_str(raw).expect("legacy loads");
            assert!(cited.repo_id.is_empty());
            assert!(cited.git_head.is_none());
        }

        #[test]
        fn git_head_recorded_and_drift_named_in_mark() {
            if std::process::Command::new("git")
                .arg("--version")
                .output()
                .is_err()
            {
                return;
            }
            let dir = write_tree(&[("s.rs", "fn v1() {}\n")]);
            let run = |args: &[&str]| {
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(dir.path())
                    .env("GIT_CONFIG_NOSYSTEM", "1")
                    .env("GIT_AUTHOR_NAME", "t")
                    .env("GIT_AUTHOR_EMAIL", "t@t")
                    .env("GIT_COMMITTER_NAME", "t")
                    .env("GIT_COMMITTER_EMAIL", "t@t")
                    .output()
                    .expect("git")
            };
            assert!(run(&["init", "-q"]).status.success());
            assert!(run(&["add", "."]).status.success());
            assert!(run(&["commit", "-qm", "one"]).status.success());
            let cited = SourceCitation::bank(dir.path(), "s.rs", 0, 1).expect("bank");
            let head_one = cited.git_head.clone().expect("head recorded");
            // Edit the span and commit again: stale with both revisions named.
            std::fs::write(dir.path().join("s.rs"), "fn v2() {}\n").unwrap();
            assert!(run(&["add", "."]).status.success());
            assert!(run(&["commit", "-qm", "two"]).status.success());
            let head_two = git_head_for(dir.path()).expect("head now");
            assert_ne!(head_one, head_two);
            let entry = MemoryEntry {
                citation: Some(cited),
                ..MemoryEntry::new(MemoryCategory::Fact, "v thing")
            };
            let mark =
                citation_stale_mark(&entry, Some(dir.path())).expect("stale mark with drift");
            assert!(mark.contains(&head_one), "{mark}");
            assert!(mark.contains(&head_two), "{mark}");
            assert!(mark.contains("re-read"), "{mark}");
        }

        #[test]
        fn malformed_bounds_verify_unverifiable_never_panic() {
            let dir = write_tree(&[("s.rs", "fn a() {}\n")]);
            for (start, end) in [
                (0usize, usize::MAX),
                (usize::MAX, usize::MAX),
                (5, 9),
                (2, 2),
            ] {
                let bad = SourceCitation {
                    path: "s.rs".to_string(),
                    start_line: start,
                    end_line: end,
                    span_hash: "00".to_string(),
                    span_text: String::new(),
                    context_before: String::new(),
                    context_after: String::new(),
                    repo_id: String::new(),
                    git_head: None,
                    origin: None,
                };
                assert_eq!(bad.verify(dir.path()), CitationStatus::Unverifiable);
                // Recall-level: malformed citations render unmarked, no panic.
                let entry = MemoryEntry {
                    citation: Some(bad),
                    ..MemoryEntry::new(MemoryCategory::Fact, "fragile fact")
                };
                let out = format_relevant_prompt_verified(&[entry], 10, dir.path())
                    .expect("recall renders");
                assert!(out.contains("fragile fact"));
                assert!(!out.contains("source changed"));
            }
        }

        #[test]
        fn checkout_move_keeps_verifying() {
            if std::process::Command::new("git")
                .arg("--version")
                .output()
                .is_err()
            {
                return;
            }
            let outer = tempfile::TempDir::new().expect("temp");
            let repo = outer.path().join("checkout");
            std::fs::create_dir_all(&repo).unwrap();
            std::fs::write(repo.join("s.rs"), "fn a() {}\n").unwrap();
            let run = |args: &[&str], dir: &std::path::Path| {
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(dir)
                    .env("GIT_CONFIG_NOSYSTEM", "1")
                    .env("GIT_AUTHOR_NAME", "t")
                    .env("GIT_AUTHOR_EMAIL", "t@t")
                    .env("GIT_COMMITTER_NAME", "t")
                    .env("GIT_COMMITTER_EMAIL", "t@t")
                    .output()
                    .expect("git")
            };
            assert!(run(&["init", "-q"], &repo).status.success());
            assert!(run(&["add", "."], &repo).status.success());
            assert!(run(&["commit", "-qm", "one"], &repo).status.success());
            let cited = SourceCitation::bank(&repo, "s.rs", 0, 1).expect("bank");
            assert!(cited.repo_id.starts_with("git:"));
            assert_eq!(cited.verify(&repo), CitationStatus::Fresh);
            // Rename the checkout: identity follows the history, not the path.
            let moved = outer.path().join("renamed");
            std::fs::rename(&repo, &moved).unwrap();
            assert_eq!(cited.verify(&moved), CitationStatus::Fresh);
        }

        #[test]
        fn foreign_clone_verifies_fresh_but_marked() {
            if std::process::Command::new("git")
                .arg("--version")
                .output()
                .is_err()
            {
                return;
            }
            let outer = tempfile::TempDir::new().expect("temp");
            let repo_a = outer.path().join("a");
            std::fs::create_dir_all(&repo_a).unwrap();
            std::fs::write(repo_a.join("s.rs"), "fn a() {}\n").unwrap();
            let run = |args: &[&str], dir: &std::path::Path| {
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(dir)
                    .env("GIT_CONFIG_NOSYSTEM", "1")
                    .env("GIT_AUTHOR_NAME", "t")
                    .env("GIT_AUTHOR_EMAIL", "t@t")
                    .env("GIT_COMMITTER_NAME", "t")
                    .env("GIT_COMMITTER_EMAIL", "t@t")
                    .output()
                    .expect("git")
            };
            assert!(run(&["init", "-q"], &repo_a).status.success());
            assert!(run(&["add", "."], &repo_a).status.success());
            assert!(run(&["commit", "-qm", "one"], &repo_a).status.success());
            // A pretends to be pushed upstream: origin is the upstream URL.
            assert!(
                run(
                    &["remote", "add", "origin", "https://example.com/up.git"],
                    &repo_a
                )
                .status
                .success()
            );
            let cited = SourceCitation::bank(&repo_a, "s.rs", 0, 1).expect("bank");
            assert_eq!(cited.origin.as_deref(), Some("https://example.com/up.git"));
            // Clone B: same history, but its origin points at A, not upstream.
            let repo_b = outer.path().join("b");
            assert!(
                run(
                    &[
                        "clone",
                        "-q",
                        repo_a.to_str().unwrap(),
                        repo_b.to_str().unwrap()
                    ],
                    outer.path()
                )
                .status
                .success()
            );
            assert_eq!(cited.verify(&repo_b), CitationStatus::Fresh);
            assert!(is_foreign_checkout(&cited, &repo_b));
            assert!(!is_foreign_checkout(&cited, &repo_a));
            let entry = MemoryEntry {
                citation: Some(cited),
                ..MemoryEntry::new(MemoryCategory::Fact, "cloned fact")
            };
            let out = format_relevant_prompt_verified(&[entry], 10, &repo_b).expect("renders");
            assert!(out.contains("different checkout"), "{out}");
        }

        #[test]
        fn credentialed_origins_never_persist_or_render() {
            assert_eq!(
                strip_url_credentials("https://user:s3cret@host.com/a.git"),
                "https://host.com/a.git"
            );
            assert_eq!(
                strip_url_credentials("https://user@host.com/a.git"),
                "https://host.com/a.git"
            );
            assert_eq!(
                strip_url_credentials("https://host.com/a.git"),
                "https://host.com/a.git"
            );
            // No scheme or no userinfo: untouched.
            assert_eq!(
                strip_url_credentials("git@host.com:a.git"),
                "git@host.com:a.git"
            );
            assert_eq!(
                strip_url_credentials("/local/path@weird"),
                "/local/path@weird"
            );
            if std::process::Command::new("git")
                .arg("--version")
                .output()
                .is_err()
            {
                return;
            }
            // Bank under a credentialed origin, then re-point it clean: the
            // stored origin must already be redacted, and the foreign mark
            // must never render the secret.
            let dir = write_tree(&[("s.rs", "fn a() {}\n")]);
            let run = |args: &[&str]| {
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(dir.path())
                    .env("GIT_CONFIG_NOSYSTEM", "1")
                    .env("GIT_AUTHOR_NAME", "t")
                    .env("GIT_AUTHOR_EMAIL", "t@t")
                    .env("GIT_COMMITTER_NAME", "t")
                    .env("GIT_COMMITTER_EMAIL", "t@t")
                    .output()
                    .expect("git")
            };
            assert!(run(&["init", "-q"]).status.success());
            assert!(run(&["add", "."]).status.success());
            assert!(run(&["commit", "-qm", "one"]).status.success());
            assert!(
                run(&[
                    "remote",
                    "add",
                    "origin",
                    "https://user:s3cret@host.com/a.git"
                ])
                .status
                .success()
            );
            let cited = SourceCitation::bank(dir.path(), "s.rs", 0, 1).expect("bank");
            assert_eq!(
                cited.origin.as_deref(),
                Some("https://host.com/a.git"),
                "secret stripped at capture"
            );
            // Legacy-shaped citation that still holds the secret (banked
            // before redaction): the mark must scrub it at render.
            let mut legacy = cited.clone();
            legacy.origin = Some("https://user:s3cret@host.com/a.git".to_string());
            let entry = MemoryEntry {
                citation: Some(legacy),
                ..MemoryEntry::new(MemoryCategory::Fact, "old fact")
            };
            // Point the checkout elsewhere so the mark fires as foreign.
            assert!(
                run(&["remote", "set-url", "origin", "https://other.com/b.git"])
                    .status
                    .success()
            );
            let out = format_relevant_prompt_verified(&[entry], 10, dir.path()).expect("renders");
            assert!(out.contains("different checkout"), "{out}");
            assert!(!out.contains("s3cret"), "{out}");
            assert!(!out.contains("user@"), "{out}");
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

        // --- R10 provenance tiers + R11 privacy pass ---

        fn secret_entry(content: &str) -> MemoryEntry {
            MemoryEntry::new(MemoryCategory::Fact, content)
        }

        #[test]
        fn detector_fires_on_secret_shapes() {
            // Each signal class from the R11 spec fires.
            for content in [
                // Generic markers, assignment adjacency.
                "deploy config: password=river-stone-99",
                "api auth: secret : hunter2-hunter2",
                "token=abc123xyz for the staging bot",
                "credential:AKIAIOSFODNN7EXAMPLE",
                "note passwd = s3cr3t-value here",
                "the private key: stored in vault seven",
                // Compound markers, standalone.
                "rotate the client_secret quarterly",
                "auth_token present in the webhook payload",
                "session_token expired at midnight",
                // Known prefixes, standalone.
                "key sk-live-abc123 rest of note",
                "deploy with ghp_abcdefgh12345678 token",
                "aws AKIAIOSFODNN7EXAMPLE key in use",
                "slack xoxb-1234567890-abcdefghij token",
                "-----BEGIN RSA PRIVATE KEY----- block pasted",
                // High-entropy long token (mixed-case alnum, ~5 bits/char).
                "vault blob xK9mQ2vX7pL4nR8wT5yU6bD3fH1jS0aZcE end",
            ] {
                let entry = secret_entry(content);
                assert!(contains_secret(&entry), "must fire: {content}");
                assert_eq!(safety_penalty(&entry), 0.5, "penalty: {content}");
            }
        }

        #[test]
        fn detector_silent_on_normal_content() {
            // Bare words, is-forms, and the §4 bare-word traps never fire.
            // False positives rank normally by design (fail-closed toward
            // recall); do not "fix" one by forgetting the row.
            for content in [
                // C1 corpus golds with bare `password` (R-002 / R-007 shape).
                "the staging database password is river-stone-77",
                "the office wifi password is harbor-light-209",
                // Bare generic words, no adjacency.
                "rotate personal access tokens quarterly",
                "credentials need manager signoff",
                "my library card PIN is 3390",
                "the public API rate limit is 5000 requests per hour per key",
                "my passport number ends in ZX-4021",
                "the staging license server is license-02 internal",
                "bring the security audit prep notes",
                // is-forms are forbidden as a trigger.
                "password is hunter2",
                "secret is out in the open",
                // Short values after adjacency do not fire (< 3 chars).
                "password: ab",
                // Ordinary prose with dashes stays under the entropy gate.
                "quarterly business review moved to Thursday 14:00",
            ] {
                let entry = secret_entry(content);
                assert!(!contains_secret(&entry), "must stay silent: {content}");
                assert_eq!(safety_penalty(&entry), 1.0, "no penalty: {content}");
            }
        }

        #[test]
        fn secret_bearing_entry_ranks_below_identical_clean_entry() {
            // Same recency/access/category/trust/strength; the ONLY delta is
            // the secret marker, so the 0.5x penalty decides the order.
            let clean =
                MemoryEntry::new(MemoryCategory::Fact, "the deploy freeze lifts Monday 06:00");
            let mut secret = MemoryEntry::new(
                MemoryCategory::Fact,
                "the deploy freeze lifts Monday 06:00 password=hunter2-hunter2",
            );
            // Pin the non-secret score inputs equal (ids differ by
            // construction; score has no id term).
            secret.created_at = clean.created_at;
            secret.updated_at = clean.updated_at;
            let clean_score = memory_score(&clean);
            let secret_score = memory_score(&secret);
            assert!(clean_score > 0.0);
            assert!(
                (secret_score - clean_score * 0.5).abs() < 1e-6,
                "secret score {secret_score} must be exactly half of clean {clean_score}"
            );
        }

        #[test]
        fn legacy_json_loads_provenance_default_agent_distilled() {
            // Rows banked before R10 (no `provenance` key — e.g. the dev
            // bench import JSON) load as AgentDistilled: quarantine-exempt.
            let json = r#"{"id":"m1","category":"fact","content":"c","tags":[],"search_text":"","created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z","access_count":0}"#;
            let entry: MemoryEntry = serde_json::from_str(json).expect("old json loads");
            assert_eq!(entry.provenance, Provenance::AgentDistilled);
            // Round-trips as snake_case.
            let round: MemoryEntry = serde_json::from_str(
                &serde_json::to_string(
                    &MemoryEntry::new(MemoryCategory::Fact, "c")
                        .with_provenance(Provenance::ToolIngested),
                )
                .expect("serialize"),
            )
            .expect("round trip");
            assert_eq!(round.provenance, Provenance::ToolIngested);
        }

        #[test]
        fn quarantine_switch_is_exact_match_default_off() {
            // Default off (absent or any non-"1" value): everything visible.
            assert!(!tool_ingested_quarantine_on());
            let tool = MemoryEntry::new(MemoryCategory::Fact, "bulk import row")
                .with_provenance(Provenance::ToolIngested);
            assert!(recall_visible(&tool));
        }
    }
}
