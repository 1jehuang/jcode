//! Memory system for cross-session learning
//!
//! Provides persistent memory that survives across sessions, organized by:
//! - Project (per working directory)
//! - Global (user-level preferences)
//!
//! Integrates with the Haiku sidecar for relevance verification and extraction.

use crate::memory_graph::{EdgeKind, MemoryGraph, GRAPH_VERSION};
use crate::sidecar::Sidecar;
use crate::storage;
use crate::tui::info_widget::{
    InjectedMemoryItem, MemoryActivity, MemoryEvent, MemoryEventKind, MemoryState,
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime};

// === Global Activity Tracking ===

/// Global memory activity state - updated by sidecar, read by info widget
static MEMORY_ACTIVITY: Mutex<Option<MemoryActivity>> = Mutex::new(None);

/// Maximum number of recent events to keep
const MAX_RECENT_EVENTS: usize = 10;

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

// === Async Memory Buffer ===

/// Pending memory prompt from background check - ready to inject on next turn
static PENDING_MEMORY: Mutex<Option<PendingMemory>> = Mutex::new(None);

/// Signature of the last injected prompt to suppress near-immediate duplicates.
static LAST_INJECTED_PROMPT_SIGNATURE: Mutex<Option<(String, Instant)>> = Mutex::new(None);

/// Guard to ensure only one memory check runs at a time
static MEMORY_CHECK_IN_PROGRESS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Suppress repeated identical memory payloads within this many seconds.
const MEMORY_REPEAT_SUPPRESSION_SECS: u64 = 90;

/// A pending memory result from async checking
#[derive(Debug, Clone)]
pub struct PendingMemory {
    /// The formatted memory prompt ready for injection
    pub prompt: String,
    /// When this was computed
    pub computed_at: Instant,
    /// Number of relevant memories found
    pub count: usize,
}

impl PendingMemory {
    /// Check if this pending memory is still fresh (not too old)
    pub fn is_fresh(&self) -> bool {
        // Consider stale after 2 minutes
        self.computed_at.elapsed().as_secs() < 120
    }
}

fn prompt_signature(prompt: &str) -> String {
    prompt
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase()
}

/// Take pending memory if available and fresh
pub fn take_pending_memory() -> Option<PendingMemory> {
    if let Ok(mut guard) = PENDING_MEMORY.lock() {
        if let Some(pending) = guard.take() {
            if !pending.is_fresh() {
                return None;
            }

            let sig = prompt_signature(&pending.prompt);
            if let Ok(mut last_guard) = LAST_INJECTED_PROMPT_SIGNATURE.lock() {
                if let Some((last_sig, last_at)) = last_guard.as_ref() {
                    if *last_sig == sig
                        && last_at.elapsed().as_secs() < MEMORY_REPEAT_SUPPRESSION_SECS
                    {
                        return None;
                    }
                }
                *last_guard = Some((sig, Instant::now()));
            }

            return Some(pending);
        }
    }
    None
}

/// Store a pending memory result
pub fn set_pending_memory(prompt: String, count: usize) {
    if let Ok(mut guard) = PENDING_MEMORY.lock() {
        *guard = Some(PendingMemory {
            prompt,
            computed_at: Instant::now(),
            count,
        });
    }
}

/// Clear any pending memory result that has not been consumed yet.
pub fn clear_pending_memory() {
    if let Ok(mut guard) = PENDING_MEMORY.lock() {
        *guard = None;
    }
    if let Ok(mut guard) = LAST_INJECTED_PROMPT_SIGNATURE.lock() {
        *guard = None;
    }
}

/// Check if there's a pending memory check in progress or result waiting
pub fn has_pending_memory() -> bool {
    PENDING_MEMORY
        .lock()
        .ok()
        .map(|g| g.is_some())
        .unwrap_or(false)
}

/// Get current memory activity state
pub fn get_activity() -> Option<MemoryActivity> {
    MEMORY_ACTIVITY.lock().ok().and_then(|guard| guard.clone())
}

/// Update the memory activity state
pub fn set_state(state: MemoryState) {
    if let Ok(mut guard) = MEMORY_ACTIVITY.lock() {
        if let Some(activity) = guard.as_mut() {
            activity.state = state;
        } else {
            *guard = Some(MemoryActivity {
                state,
                recent_events: Vec::new(),
            });
        }
    }
}

/// Add an event to the activity log
pub fn add_event(kind: MemoryEventKind) {
    if let Ok(mut guard) = MEMORY_ACTIVITY.lock() {
        let event = MemoryEvent {
            kind,
            timestamp: Instant::now(),
            detail: None,
        };

        if let Some(activity) = guard.as_mut() {
            activity.recent_events.insert(0, event);
            activity.recent_events.truncate(MAX_RECENT_EVENTS);
        } else {
            *guard = Some(MemoryActivity {
                state: MemoryState::Idle,
                recent_events: vec![event],
            });
        }
    }
}

/// Clear activity (reset to idle with no events)
pub fn clear_activity() {
    if let Ok(mut guard) = MEMORY_ACTIVITY.lock() {
        *guard = None;
    }
}

/// Record that a memory payload was injected into model context.
/// This feeds the memory info widget with injected content + metadata.
pub fn record_injected_prompt(prompt: &str, count: usize, age_ms: u64) {
    let items = parse_injected_items(prompt, 8);
    let preview = prompt_preview(prompt, 72);
    add_event(MemoryEventKind::MemoryInjected {
        count,
        prompt_chars: prompt.chars().count(),
        age_ms,
        preview: preview.clone(),
        items,
    });
    add_event(MemoryEventKind::MemorySurfaced {
        memory_preview: preview,
    });
}

fn parse_injected_items(prompt: &str, max_items: usize) -> Vec<InjectedMemoryItem> {
    let mut items: Vec<InjectedMemoryItem> = Vec::new();
    let mut section = String::from("Memory");

    for raw_line in prompt.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line == "# Memory" {
            continue;
        }
        if let Some(header) = line.strip_prefix("## ") {
            let header = header.trim();
            if !header.is_empty() {
                section = header.to_string();
            }
            continue;
        }

        let content = if let Some(rest) = line.strip_prefix("- ") {
            Some(rest.trim())
        } else if let Some((prefix, rest)) = line.split_once(". ") {
            if !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_digit()) {
                Some(rest.trim())
            } else {
                None
            }
        } else {
            None
        };

        if let Some(content) = content {
            if content.is_empty() {
                continue;
            }
            items.push(InjectedMemoryItem {
                section: section.clone(),
                content: content.to_string(),
            });
            if items.len() >= max_items {
                return items;
            }
        }
    }

    if items.is_empty() {
        let fallback = prompt
            .lines()
            .map(str::trim)
            .filter(|line| {
                !line.is_empty()
                    && !line.starts_with('#')
                    && !line.starts_with("## ")
                    && !line.starts_with("- ")
            })
            .collect::<Vec<_>>()
            .join(" ");
        if !fallback.is_empty() {
            items.push(InjectedMemoryItem {
                section,
                content: fallback,
            });
        }
    }

    items
}

fn prompt_preview(prompt: &str, max_chars: usize) -> String {
    let bullet = prompt
        .lines()
        .map(str::trim)
        .find_map(|line| {
            if line.starts_with("- ") {
                Some(line.trim_start_matches("- ").trim())
            } else if let Some((prefix, rest)) = line.split_once(". ") {
                if !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_digit()) {
                    Some(rest.trim())
                } else {
                    None
                }
            } else {
                None
            }
        })
        .unwrap_or_else(|| prompt.trim());

    if bullet.chars().count() <= max_chars {
        bullet.to_string()
    } else {
        let mut out = String::new();
        for (i, ch) in bullet.chars().enumerate() {
            if i >= max_chars.saturating_sub(3) {
                break;
            }
            out.push(ch);
        }
        out.push_str("...");
        out
    }
}

/// Trust levels for memories
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TrustLevel {
    /// User explicitly stated this
    High,
    /// Observed from user behavior
    Medium,
    /// Inferred by the agent
    Low,
}

impl Default for TrustLevel {
    fn default() -> Self {
        TrustLevel::Medium
    }
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
        Self {
            id: crate::id::new_id("mem"),
            category,
            content: content.into(),
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
        let query_lower = query.to_lowercase();
        self.entries
            .iter()
            .filter(|e| {
                e.content.to_lowercase().contains(&query_lower)
                    || e.tags
                        .iter()
                        .any(|t| t.to_lowercase().contains(&query_lower))
            })
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
        let mut entries: Vec<&MemoryEntry> = self.entries.iter().filter(|e| e.active).collect();
        entries.sort_by(|a, b| {
            let score_a = memory_score(a);
            let score_b = memory_score(b);
            score_b
                .partial_cmp(&score_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        entries.into_iter().take(limit).collect()
    }

    pub fn format_for_prompt(&self, limit: usize) -> Option<String> {
        let relevant: Vec<MemoryEntry> = self.get_relevant(limit).into_iter().cloned().collect();
        format_entries_for_prompt(&relevant, limit)
    }
}

const MEMORY_CONTEXT_MAX_CHARS: usize = 8_000;
const MEMORY_CONTEXT_MAX_MESSAGES: usize = 12;
const MEMORY_CONTEXT_MAX_BLOCK_CHARS: usize = 1_200;
const MEMORY_RELEVANCE_MAX_CANDIDATES: usize = 30;
const MEMORY_RELEVANCE_MAX_RESULTS: usize = 10;

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    value.chars().take(max_chars).collect()
}

fn format_content_block(block: &crate::message::ContentBlock) -> Option<String> {
    match block {
        crate::message::ContentBlock::Text { text, .. } => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(truncate_chars(trimmed, MEMORY_CONTEXT_MAX_BLOCK_CHARS))
            }
        }
        crate::message::ContentBlock::ToolUse { name, input, .. } => {
            let input_str =
                serde_json::to_string(input).unwrap_or_else(|_| "<invalid json>".into());
            let input_str = truncate_chars(&input_str, MEMORY_CONTEXT_MAX_BLOCK_CHARS / 2);
            Some(format!("[Tool: {} input: {}]", name, input_str))
        }
        crate::message::ContentBlock::ToolResult {
            content, is_error, ..
        } => {
            let label = if is_error.unwrap_or(false) {
                "Tool error"
            } else {
                "Tool result"
            };
            let content = truncate_chars(content, MEMORY_CONTEXT_MAX_BLOCK_CHARS / 2);
            Some(format!("[{}: {}]", label, content))
        }
        crate::message::ContentBlock::Reasoning { .. } => None,
        crate::message::ContentBlock::Image { .. } => Some("[Image]".to_string()),
    }
}

fn format_message_context(message: &crate::message::Message) -> String {
    let role = match message.role {
        crate::message::Role::User => "User",
        crate::message::Role::Assistant => "Assistant",
    };

    let mut chunk = String::new();
    chunk.push_str(role);
    chunk.push_str(":\n");

    let mut has_content = false;
    for block in &message.content {
        if let Some(text) = format_content_block(block) {
            if !text.is_empty() {
                has_content = true;
                chunk.push_str(&text);
                chunk.push('\n');
            }
        }
    }

    if has_content {
        chunk
    } else {
        String::new()
    }
}

/// Format messages into a context string for relevance checking
pub fn format_context_for_relevance(messages: &[crate::message::Message]) -> String {
    let mut chunks: Vec<String> = Vec::new();
    let mut total_chars = 0usize;

    for message in messages.iter().rev().take(MEMORY_CONTEXT_MAX_MESSAGES) {
        let chunk = format_message_context(message);
        if chunk.is_empty() {
            continue;
        }
        let chunk_len = chunk.chars().count();
        if total_chars + chunk_len > MEMORY_CONTEXT_MAX_CHARS {
            if total_chars == 0 {
                chunks.push(truncate_chars(&chunk, MEMORY_CONTEXT_MAX_CHARS));
            }
            break;
        }
        total_chars += chunk_len;
        chunks.push(chunk);
    }

    chunks.reverse();
    chunks.join("\n").trim().to_string()
}

fn format_entries_for_prompt(entries: &[MemoryEntry], limit: usize) -> Option<String> {
    let mut sections: HashMap<MemoryCategory, Vec<&MemoryEntry>> = HashMap::new();
    let mut seen_content = HashSet::new();
    let mut added = 0usize;

    for entry in entries.iter().filter(|e| e.active) {
        if added >= limit {
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

        sections
            .entry(entry.category.clone())
            .or_default()
            .push(entry);
        added += 1;
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
        output.push_str(&format!("## {}\n", title));
        for (idx, item) in items.into_iter().enumerate() {
            output.push_str(&format!("{}. {}\n", idx + 1, item.content.trim()));
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
    } else {
        Some(output.trim().to_string())
    }
}

pub(crate) fn format_relevant_prompt(entries: &[MemoryEntry], limit: usize) -> Option<String> {
    format_entries_for_prompt(entries, limit).map(|formatted| format!("# Memory\n\n{}", formatted))
}

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

#[derive(Clone)]
pub struct MemoryManager {
    project_dir: Option<PathBuf>,
    /// When true, use isolated test storage instead of real memory
    test_mode: bool,
}

impl MemoryManager {
    pub fn new() -> Self {
        Self {
            project_dir: None,
            test_mode: false,
        }
    }

    /// Create a memory manager in test mode (isolated storage)
    pub fn new_test() -> Self {
        Self {
            project_dir: None,
            test_mode: true,
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

    pub fn remember_project(&self, entry: MemoryEntry) -> Result<String> {
        let mut entry = entry;
        // Generate embedding for new memory (non-blocking - if it fails, we store without embedding)
        entry.ensure_embedding();

        let mut graph = self.load_project_graph()?;
        let id = graph.add_memory(entry);
        self.save_project_graph(&graph)?;
        Ok(id)
    }

    pub fn remember_global(&self, entry: MemoryEntry) -> Result<String> {
        let mut entry = entry;
        // Generate embedding for new memory
        entry.ensure_embedding();

        let mut graph = self.load_global_graph()?;
        let id = graph.add_memory(entry);
        self.save_global_graph(&graph)?;
        Ok(id)
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

    /// Find memories similar to the given embedding
    pub fn find_similar_with_embedding(
        &self,
        query_embedding: &[f32],
        threshold: f32,
        limit: usize,
    ) -> Result<Vec<(MemoryEntry, f32)>> {
        // Collect all memories with embeddings
        let mut entries_with_emb: Vec<MemoryEntry> = Vec::new();
        if let Ok(project) = self.load_project_graph() {
            entries_with_emb.extend(
                project
                    .active_memories()
                    .filter(|m| m.embedding.is_some())
                    .cloned(),
            );
        }
        if let Ok(global) = self.load_global_graph() {
            entries_with_emb.extend(
                global
                    .active_memories()
                    .filter(|m| m.embedding.is_some())
                    .cloned(),
            );
        }

        if entries_with_emb.is_empty() {
            return Ok(Vec::new());
        }

        // Batch cosine similarity: single vectorized pass over all embeddings
        let emb_refs: Vec<&[f32]> = entries_with_emb
            .iter()
            .map(|e| e.embedding.as_ref().unwrap().as_slice())
            .collect();
        let scores = crate::embedding::batch_cosine_similarity(query_embedding, &emb_refs);

        // Filter by threshold and sort
        let mut scored: Vec<(MemoryEntry, f32)> = entries_with_emb
            .into_iter()
            .zip(scores)
            .filter(|(_, sim)| *sim >= threshold)
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);

        Ok(scored)
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
        let mut all_entries: Vec<MemoryEntry> = Vec::new();
        if let Ok(project) = self.load_project_graph() {
            all_entries.extend(project.all_memories().cloned());
        }
        if let Ok(global) = self.load_global_graph() {
            all_entries.extend(global.all_memories().cloned());
        }

        if all_entries.is_empty() {
            return None;
        }

        // Sort by updated_at descending and limit
        all_entries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        all_entries.truncate(limit);

        // Format as prompt
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
        let mut results = Vec::new();
        let query_lower = query.to_lowercase();

        // Search in project graph
        if let Ok(project) = self.load_project_graph() {
            for memory in project.all_memories() {
                if memory.content.to_lowercase().contains(&query_lower)
                    || memory
                        .tags
                        .iter()
                        .any(|t| t.to_lowercase().contains(&query_lower))
                {
                    results.push(memory.clone());
                }
            }
        }

        // Search in global graph
        if let Ok(global) = self.load_global_graph() {
            for memory in global.all_memories() {
                if memory.content.to_lowercase().contains(&query_lower)
                    || memory
                        .tags
                        .iter()
                        .any(|t| t.to_lowercase().contains(&query_lower))
                {
                    results.push(memory.clone());
                }
            }
        }

        Ok(results)
    }

    pub fn list_all(&self) -> Result<Vec<MemoryEntry>> {
        let mut all = Vec::new();

        if let Ok(project) = self.load_project_graph() {
            all.extend(project.all_memories().cloned());
        }
        if let Ok(global) = self.load_global_graph() {
            all.extend(global.all_memories().cloned());
        }

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
        let mut candidates: Vec<_> = self
            .list_all()?
            .into_iter()
            .filter(|entry| entry.active)
            .collect();
        candidates.sort_by(|a, b| {
            let score_a = memory_score(a);
            let score_b = memory_score(b);
            score_b
                .partial_cmp(&score_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        candidates.truncate(max_candidates);

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
        let all = self.list_all()?;

        let matches: Vec<_> = all
            .into_iter()
            .filter(|e| {
                let content_lower = e.content.to_lowercase();
                keywords
                    .iter()
                    .any(|kw| content_lower.contains(&kw.to_lowercase()))
            })
            .take(limit)
            .collect();

        Ok(matches)
    }

    // === Async Memory Checking ===

    /// Spawn a background task to check memory relevance
    /// Results are stored in PENDING_MEMORY and can be retrieved with take_pending_memory()
    /// This method returns immediately and never blocks the caller
    /// Only ONE memory check runs at a time - additional calls are ignored
    pub fn spawn_relevance_check(&self, messages: Vec<crate::message::Message>) {
        use std::sync::atomic::Ordering;

        // Only spawn if no check is currently in progress
        if MEMORY_CHECK_IN_PROGRESS.swap(true, Ordering::SeqCst) {
            // Another check is already running - skip this one
            return;
        }

        let project_dir = self.project_dir.clone();

        tokio::spawn(async move {
            let manager = MemoryManager {
                project_dir: project_dir.or_else(|| std::env::current_dir().ok()),
                test_mode: false, // Prefetch uses real storage
            };

            match manager.get_relevant_parallel(&messages).await {
                Ok(Some(prompt)) => {
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
                    set_pending_memory(prompt, count);
                    add_event(MemoryEventKind::SidecarComplete { latency_ms: 0 });
                }
                Ok(None) => {
                    // No relevant memories - that's fine
                    set_state(MemoryState::Idle);
                }
                Err(e) => {
                    // Log but don't crash - memory is non-critical
                    crate::logging::error(&format!("Background memory check failed: {}", e));
                    add_event(MemoryEventKind::Error {
                        message: e.to_string(),
                    });
                    set_state(MemoryState::Idle);
                }
            }

            // Release the guard when done
            MEMORY_CHECK_IN_PROGRESS.store(false, Ordering::SeqCst);
        });
    }

    /// Get relevant memories using embedding search + sidecar verification
    /// 1. Embed the context (fast, local, ~30ms)
    /// 2. Find similar memories by embedding (instant)
    /// 3. Only call sidecar for embedding hits (1-5 calls instead of 30)
    pub async fn get_relevant_parallel(
        &self,
        messages: &[crate::message::Message],
    ) -> Result<Option<String>> {
        let context = format_context_for_relevance(messages);
        if context.is_empty() {
            return Ok(None);
        }

        // Step 1: Embedding search (fast, local)
        set_state(MemoryState::Embedding);
        add_event(MemoryEventKind::EmbeddingStarted);

        let embedding_start = Instant::now();
        let candidates =
            match self.find_similar(&context, EMBEDDING_SIMILARITY_THRESHOLD, EMBEDDING_MAX_HITS) {
                Ok(hits) => {
                    let latency_ms = embedding_start.elapsed().as_millis() as u64;
                    if hits.is_empty() {
                        add_event(MemoryEventKind::EmbeddingComplete {
                            latency_ms,
                            hits: 0,
                        });
                        set_state(MemoryState::Idle);
                        return Ok(None);
                    }
                    add_event(MemoryEventKind::EmbeddingComplete {
                        latency_ms,
                        hits: hits.len(),
                    });
                    hits
                }
                Err(e) => {
                    // Embedding failed - fall back to score-based selection
                    crate::logging::info(&format!("Embedding search failed, falling back: {}", e));
                    add_event(MemoryEventKind::Error {
                        message: e.to_string(),
                    });

                    // Fallback: use score-based selection (old behavior)
                    let mut all: Vec<_> =
                        self.list_all()?.into_iter().filter(|e| e.active).collect();
                    all.sort_by(|a, b| {
                        memory_score(b)
                            .partial_cmp(&memory_score(a))
                            .unwrap_or(std::cmp::Ordering::Equal)
                    });
                    all.truncate(MEMORY_RELEVANCE_MAX_CANDIDATES);
                    all.into_iter().map(|e| (e, 0.0)).collect()
                }
            };

        if candidates.is_empty() {
            set_state(MemoryState::Idle);
            return Ok(None);
        }

        // Step 2: Sidecar verification (only for embedding hits - much fewer calls!)
        set_state(MemoryState::SidecarChecking {
            count: candidates.len(),
        });
        add_event(MemoryEventKind::SidecarStarted);

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
                                "Memory relevant (sim={:.2}): {}",
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
            }
        }

        let _ = self.touch_entries(&relevant_ids);

        if relevant.is_empty() {
            set_state(MemoryState::Idle);
            return Ok(None);
        }

        set_state(MemoryState::FoundRelevant {
            count: relevant.len(),
        });

        Ok(format_relevant_prompt(
            &relevant,
            MEMORY_RELEVANCE_MAX_RESULTS,
        ))
    }

    // ==================== Graph-Based Operations ====================

    /// Load project memories as a MemoryGraph with automatic migration
    pub fn load_project_graph(&self) -> Result<MemoryGraph> {
        let Some(path) = self.project_memory_path()? else {
            return Ok(MemoryGraph::new());
        };

        if !self.test_mode {
            if let Some(graph) = cached_graph(&path) {
                return Ok(graph);
            }
        }

        if path.exists() {
            // Try loading as MemoryGraph first
            if let Ok(graph) = storage::read_json::<MemoryGraph>(&path) {
                if graph.graph_version == GRAPH_VERSION {
                    if !self.test_mode {
                        cache_graph(path, &graph);
                    }
                    return Ok(graph);
                }
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
                "Migrated memory store to graph format: {}",
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

    /// Load global memories as a MemoryGraph with automatic migration
    pub fn load_global_graph(&self) -> Result<MemoryGraph> {
        let path = self.global_memory_path()?;
        if !self.test_mode {
            if let Some(graph) = cached_graph(&path) {
                return Ok(graph);
            }
        }

        if path.exists() {
            // Try loading as MemoryGraph first
            if let Ok(graph) = storage::read_json::<MemoryGraph>(&path) {
                if graph.graph_version == GRAPH_VERSION {
                    if !self.test_mode {
                        cache_graph(path, &graph);
                    }
                    return Ok(graph);
                }
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
        let (mut graph, is_project) = {
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
        // First, do basic embedding search
        let embedding_hits = self.find_similar(text, threshold, limit)?;

        if embedding_hits.is_empty() {
            return Ok(Vec::new());
        }

        // Get seed IDs and scores
        let seed_ids: Vec<String> = embedding_hits.iter().map(|(e, _)| e.id.clone()).collect();
        let seed_scores: Vec<f32> = embedding_hits.iter().map(|(_, s)| *s).collect();

        // Load graphs and perform cascade retrieval
        let mut project_graph = self.load_project_graph()?;
        let mut global_graph = self.load_global_graph()?;

        // Cascade through project graph
        let project_cascade = project_graph.cascade_retrieve(&seed_ids, &seed_scores, 2, limit * 2);

        // Cascade through global graph
        let global_cascade = global_graph.cascade_retrieve(&seed_ids, &seed_scores, 2, limit * 2);

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

        // Look up entries and sort by score
        let mut results: Vec<(MemoryEntry, f32)> = merged
            .into_iter()
            .filter_map(|(id, score)| {
                project_graph
                    .get_memory(&id)
                    .or_else(|| global_graph.get_memory(&id))
                    .cloned()
                    .map(|entry| (entry, score))
            })
            .collect();

        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(limit);

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
pub const EMBEDDING_SIMILARITY_THRESHOLD: f32 = 0.4;

/// Maximum embedding hits to verify with sidecar
pub const EMBEDDING_MAX_HITS: usize = 10;

impl Default for MemoryManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{ContentBlock, Message, Role};
    use serde_json::json;
    use std::fs;
    use std::path::Path;
    use std::sync::Mutex;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    static ENV_LOCK: Mutex<()> = Mutex::new(());
    static PENDING_MEMORY_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn with_temp_home<F, T>(f: F) -> T
    where
        F: FnOnce(&Path) -> T,
    {
        let _guard = ENV_LOCK.lock().expect("env lock poisoned");
        let old = std::env::var("JCODE_HOME").ok();
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time went backwards")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("jcode-test-{}", unique));
        fs::create_dir_all(&dir).expect("create temp dir");
        std::env::set_var("JCODE_HOME", &dir);

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&dir)));

        match old {
            Some(value) => std::env::set_var("JCODE_HOME", value),
            None => std::env::remove_var("JCODE_HOME"),
        }
        let _ = fs::remove_dir_all(&dir);

        match result {
            Ok(value) => value,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    #[test]
    fn pending_memory_freshness_and_clear() {
        let _guard = PENDING_MEMORY_TEST_LOCK
            .lock()
            .expect("pending memory test lock poisoned");
        clear_pending_memory();

        set_pending_memory("hello".to_string(), 2);
        assert!(has_pending_memory());
        let pending = take_pending_memory().expect("pending memory");
        assert_eq!(pending.prompt, "hello");
        assert_eq!(pending.count, 2);
        assert!(!has_pending_memory());

        {
            let mut guard = PENDING_MEMORY.lock().expect("pending memory lock");
            *guard = Some(PendingMemory {
                prompt: "stale".to_string(),
                computed_at: Instant::now() - Duration::from_secs(121),
                count: 1,
            });
        }
        assert!(take_pending_memory().is_none());
    }

    #[test]
    fn pending_memory_suppresses_immediate_duplicate_payloads() {
        let _guard = PENDING_MEMORY_TEST_LOCK
            .lock()
            .expect("pending memory test lock poisoned");
        clear_pending_memory();

        set_pending_memory("same payload".to_string(), 1);
        assert!(take_pending_memory().is_some());

        set_pending_memory("same payload".to_string(), 1);
        assert!(
            take_pending_memory().is_none(),
            "identical payload should be suppressed when repeated immediately"
        );
    }

    #[test]
    fn format_context_includes_roles_and_tools() {
        let messages = vec![
            Message::user("Hello world"),
            Message {
                role: Role::Assistant,
                content: vec![ContentBlock::ToolUse {
                    id: "tool-1".to_string(),
                    name: "memory".to_string(),
                    input: json!({"action": "list"}),
                }],
                timestamp: None,
            },
            Message::tool_result("tool-1", "ok", false),
            Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "tool-2".to_string(),
                    content: "boom".to_string(),
                    is_error: Some(true),
                }],
                timestamp: None,
            },
        ];

        let context = format_context_for_relevance(&messages);
        assert!(context.contains("User:\nHello world"));
        assert!(context.contains("[Tool: memory input:"));
        assert!(context.contains("[Tool result: ok]"));
        assert!(context.contains("[Tool error: boom]"));
    }

    #[test]
    fn memory_store_format_groups_by_category() {
        let mut store = MemoryStore::new();
        let now = Utc::now();
        let mut correction = MemoryEntry::new(MemoryCategory::Correction, "Fix lint rules");
        correction.updated_at = now;
        let mut fact = MemoryEntry::new(MemoryCategory::Fact, "Uses tokio");
        fact.updated_at = now;
        let mut preference =
            MemoryEntry::new(MemoryCategory::Preference, "Prefers ASCII-only edits");
        preference.updated_at = now;
        let mut entity = MemoryEntry::new(MemoryCategory::Entity, "Jeremy");
        entity.updated_at = now;
        let mut custom = MemoryEntry::new(MemoryCategory::Custom("team".to_string()), "Platform");
        custom.updated_at = now;

        store.add(correction);
        store.add(fact);
        store.add(preference);
        store.add(entity);
        store.add(custom);

        let output = store.format_for_prompt(10).expect("formatted output");
        let correction_idx = output.find("## Corrections").expect("correction heading");
        let fact_idx = output.find("## Facts").expect("fact heading");
        let preference_idx = output.find("## Preferences").expect("preference heading");
        let entity_idx = output.find("## Entities").expect("entity heading");
        let custom_idx = output.find("## team").expect("custom heading");

        assert!(correction_idx < fact_idx);
        assert!(fact_idx < preference_idx);
        assert!(preference_idx < entity_idx);
        assert!(entity_idx < custom_idx);
    }

    #[test]
    fn memory_store_search_matches_content_and_tags() {
        let mut store = MemoryStore::new();
        let entry = MemoryEntry::new(MemoryCategory::Fact, "Uses Tokio runtime")
            .with_tags(vec!["async".to_string()]);
        store.add(entry);

        let content_hits = store.search("tokio");
        assert_eq!(content_hits.len(), 1);

        let tag_hits = store.search("ASYNC");
        assert_eq!(tag_hits.len(), 1);
    }

    #[test]
    fn manager_persists_and_forgets_memories() {
        with_temp_home(|_dir| {
            let manager = MemoryManager::new();
            let entry_project =
                MemoryEntry::new(MemoryCategory::Fact, "Project memory").with_embedding(vec![0.0]);
            let entry_global = MemoryEntry::new(MemoryCategory::Preference, "Global memory")
                .with_embedding(vec![0.0]);

            let project_id = manager
                .remember_project(entry_project)
                .expect("remember project");
            let global_id = manager
                .remember_global(entry_global)
                .expect("remember global");

            let all = manager.list_all().expect("list all");
            assert_eq!(all.len(), 2);

            let search = manager.search("global").expect("search");
            assert_eq!(search.len(), 1);

            assert!(manager.forget(&project_id).expect("forget project"));
            let remaining = manager.list_all().expect("list all");
            assert_eq!(remaining.len(), 1);

            assert!(!manager.forget(&project_id).expect("forget missing"));
            assert!(manager.forget(&global_id).expect("forget global"));
        });
    }

    #[test]
    fn graph_based_memory_operations() {
        with_temp_home(|_home| {
            let manager = MemoryManager::new();

            // Create two memories
            let entry1 = MemoryEntry::new(MemoryCategory::Fact, "Rust is a systems language");
            let entry2 = MemoryEntry::new(MemoryCategory::Fact, "Cargo is Rust's package manager");

            let id1 = manager.remember_project(entry1).expect("remember 1");
            let id2 = manager.remember_project(entry2).expect("remember 2");

            // Test tagging
            manager.tag_memory(&id1, "rust").expect("tag memory");
            manager.tag_memory(&id1, "language").expect("tag memory 2");
            manager.tag_memory(&id2, "rust").expect("tag memory 3");

            // Check graph stats (memories, tags, edges, clusters)
            let (mems, tags, edges, _clusters) = manager.graph_stats().expect("stats");
            assert_eq!(mems, 2, "expected 2 memories");
            assert_eq!(tags, 2, "expected 2 tags: rust and language");
            assert!(edges >= 3, "expected at least 3 edges, got {}", edges);

            // Test linking
            manager.link_memories(&id1, &id2, 0.8).expect("link");

            // Test get_related
            let related = manager.get_related(&id1, 2).expect("get related");
            assert!(!related.is_empty());
            // Should find id2 through the RelatesTo edge
            assert!(related.iter().any(|e| e.id == id2));

            // Clean up
            manager.forget(&id1).expect("forget 1");
            manager.forget(&id2).expect("forget 2");
        });
    }
}
