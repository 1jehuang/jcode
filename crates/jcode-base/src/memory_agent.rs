//! Persistent Memory Agent
//!
//! A dedicated Haiku-powered agent for memory management that runs alongside
//! the main agent. It has access to memory-specific tools only (no code execution).
//!
//! Architecture:
//! - Receives context updates from main agent via channel
//! - Uses embeddings for fast similarity search
//! - Uses Haiku LLM to decide what's relevant and dig deeper
//! - Surfaces relevant memories to main agent via PENDING_MEMORY

use anyhow::Result;
use chrono::Utc;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tokio::sync::mpsc;

use crate::embedding;
use crate::memory::{self, MemoryEntry, MemoryManager};
use crate::memory_graph::{ClusterEntry, EdgeKind, MemoryGraph};
use crate::memory_types::{MemoryEventKind, MemoryState, StepResult, StepStatus};
use crate::sidecar::Sidecar;

/// Context from a retrieval operation for post-retrieval maintenance
#[derive(Debug, Clone)]
struct RetrievalContext {
    /// Memory IDs that were verified as relevant by Haiku
    verified_ids: Vec<String>,
    /// Memory IDs that were retrieved but rejected by Haiku
    rejected_ids: Vec<String>,
    /// Brief snippet of the context for gap logging
    context_snippet: String,
}

/// Channel capacity for context updates
const CONTEXT_CHANNEL_CAPACITY: usize = 16;

/// Similarity threshold for topic change detection (lower = more different)
const TOPIC_CHANGE_THRESHOLD: f32 = 0.3;

/// Maximum memories to surface per turn
const MAX_MEMORIES_PER_TURN: usize = 5;

/// Dynamic no-sidecar gate tunables (variable-k surfacing without an LLM).
///
/// When the memory sidecar is disabled (no LLM to judge relevance), we used to
/// blindly pad the hybrid top-5 every turn, which injected ~5 memories even on
/// turns that needed none. Instead we keep a score-relative window: always keep
/// the top candidate, then keep each following candidate only while its hybrid
/// score stays within `GATE_REL_FLOOR` of the top AND within `GATE_DROP_RATIO`
/// of the previous kept score. The first big gap cuts the tail. This injects a
/// VARIABLE count (1..=MAX_MEMORIES_PER_TURN) instead of a fixed 5.
///
/// Bench (self-dev corpus, 150 query windows): precision@5 0.23 -> 0.36 (+56%),
/// avg injected 5.0 -> ~2.25/turn, at zero added cost. Note this cannot drop to
/// 0 on no-memory turns (cosdiag proved no zero-cost score separates them); the
/// only lever for true 0-injection is the LLM precision rerank (sidecar mode).
const GATE_REL_FLOOR: f32 = 0.90;
const GATE_DROP_RATIO: f32 = 0.95;

/// Score-relative dynamic gate over hybrid-ranked `(entry, score)` candidates.
///
/// Always keeps the top candidate, then keeps each following candidate only
/// while its score stays within `GATE_REL_FLOOR` of the top score AND within
/// `GATE_DROP_RATIO` of the previously kept score; the first gap that breaks
/// either bound truncates the tail. Caps output at `max_k`. Returns a variable
/// count (1..=max_k for a non-empty input), not a fixed top-k.
fn dynamic_gate_select(
    candidates: Vec<(MemoryEntry, f32)>,
    max_k: usize,
) -> Vec<(MemoryEntry, f32)> {
    if candidates.is_empty() {
        return Vec::new();
    }
    let top = candidates[0].1.max(f32::MIN_POSITIVE);
    let mut prev = top;
    let mut out: Vec<(MemoryEntry, f32)> = Vec::new();
    for (entry, sim) in candidates.into_iter().take(max_k) {
        if !out.is_empty() && (sim < top * GATE_REL_FLOOR || sim < prev * GATE_DROP_RATIO) {
            break;
        }
        prev = sim;
        out.push((entry, sim));
    }
    out
}

/// Reset surfaced memories every N turns to allow re-surfacing
const TURN_RESET_INTERVAL: usize = 50;

/// How often to run periodic cluster refinement in post-retrieval maintenance.
const CLUSTER_REFINEMENT_INTERVAL: u64 = 50;

/// Global memory agent instance
static MEMORY_AGENT: tokio::sync::OnceCell<MemoryAgentHandle> = tokio::sync::OnceCell::const_new();
static MAINTENANCE_TICK: AtomicU64 = AtomicU64::new(0);

/// Lightweight runtime stats for UI/debugging.
#[derive(Debug, Clone, Default)]
pub struct MemoryAgentStats {
    /// Number of context turns processed by memory agent.
    pub turns_processed: usize,
    /// Number of maintenance cycles completed.
    pub maintenance_runs: usize,
    /// Last maintenance duration in ms.
    pub last_maintenance_ms: Option<u64>,
}

static MEMORY_AGENT_STATS: Mutex<MemoryAgentStats> = Mutex::new(MemoryAgentStats {
    turns_processed: 0,
    maintenance_runs: 0,
    last_maintenance_ms: None,
});

/// Build a transcript string suitable for memory extraction.
pub fn build_transcript_for_extraction(messages: &[crate::message::Message]) -> String {
    let mut transcript = String::new();
    for msg in messages {
        let role = match msg.role {
            crate::message::Role::User => "User",
            crate::message::Role::Assistant => "Assistant",
        };
        transcript.push_str(&format!("**{}:**\n", role));
        for block in &msg.content {
            match block {
                crate::message::ContentBlock::Text { text, .. } => {
                    if text.trim_start().starts_with("<system-reminder>") {
                        continue;
                    }
                    transcript.push_str(text);
                    transcript.push('\n');
                }
                crate::message::ContentBlock::ToolUse { name, .. } => {
                    transcript.push_str(&format!("[Used tool: {}]\n", name));
                }
                crate::message::ContentBlock::ToolResult { content, .. } => {
                    let preview = if content.len() > 200 {
                        format!("{}...", crate::util::truncate_str(content, 200))
                    } else {
                        content.clone()
                    };
                    transcript.push_str(&format!("[Result: {}]\n", preview));
                }
                crate::message::ContentBlock::Reasoning { .. }
                | crate::message::ContentBlock::ReasoningTrace { .. }
                | crate::message::ContentBlock::AnthropicThinking { .. }
                | crate::message::ContentBlock::OpenAIReasoning { .. } => {}
                crate::message::ContentBlock::Image { .. } => {
                    transcript.push_str("[Image]\n");
                }
                crate::message::ContentBlock::OpenAICompaction { .. } => {
                    transcript.push_str("[OpenAI native compaction]\n");
                }
            }
        }
        transcript.push('\n');
    }
    transcript
}

fn manager_for_working_dir(working_dir: Option<&str>) -> MemoryManager {
    match working_dir {
        Some(dir) if !dir.trim().is_empty() => MemoryManager::new().with_project_dir(dir),
        _ => MemoryManager::new(),
    }
}

async fn run_final_extraction(transcript: String, session_id: String, working_dir: Option<String>) {
    crate::logging::info(&format!(
        "Final extraction starting for session {} ({} chars)",
        session_id,
        transcript.len()
    ));

    let sidecar = crate::sidecar::Sidecar::new();
    let manager = manager_for_working_dir(working_dir.as_deref());

    let existing: Vec<String> = manager
        .list_all()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.active)
        .map(|e| e.content)
        .collect();

    let result = sidecar
        .extract_memories_with_existing(&transcript, &existing)
        .await;

    match result {
        Ok(extracted) if !extracted.is_empty() => {
            let mut stored_count = 0;

            for mem in &extracted {
                let category = crate::memory::MemoryCategory::from_extracted(&mem.category);

                let trust = match mem.trust.as_str() {
                    "high" => crate::memory::TrustLevel::High,
                    "low" => crate::memory::TrustLevel::Low,
                    _ => crate::memory::TrustLevel::Medium,
                };

                let entry = crate::memory::MemoryEntry::new(category, &mem.content)
                    .with_source(&session_id)
                    .with_trust(trust);

                if manager.remember_project(entry).is_ok() {
                    stored_count += 1;
                }
            }

            if stored_count > 0 {
                crate::logging::info(&format!(
                    "Final extraction for session {}: stored {} memories",
                    session_id, stored_count
                ));
            }
        }
        Ok(_) => {
            crate::logging::info(&format!(
                "Final extraction for session {}: no memories extracted",
                session_id
            ));
        }
        Err(e) => {
            crate::logging::info(&format!(
                "Final extraction for session {} failed: {}",
                session_id, e
            ));
        }
    }
}

/// Handle to communicate with the memory agent
#[derive(Clone)]
pub struct MemoryAgentHandle {
    /// Send messages to the agent
    tx: mpsc::Sender<AgentMessage>,
}

impl MemoryAgentHandle {
    /// Send a context update to the memory agent (async)
    pub async fn update_context(
        &self,
        session_id: &str,
        messages: Arc<[crate::message::Message]>,
        working_dir: Option<String>,
    ) {
        self.update_context_sync_with_dir(session_id, messages, working_dir);
    }

    pub fn update_context_sync(&self, session_id: &str, messages: Arc<[crate::message::Message]>) {
        self.update_context_sync_with_dir(session_id, messages, None);
    }

    pub fn update_context_sync_with_dir(
        &self,
        session_id: &str,
        messages: Arc<[crate::message::Message]>,
        working_dir: Option<String>,
    ) {
        let msg = AgentMessage::Context {
            session_id: session_id.to_string(),
            messages,
            working_dir,
            timestamp: Instant::now(),
        };
        let _ = self.tx.try_send(msg);
    }

    /// Reset all memory agent state (call on new session)
    pub fn reset(&self) {
        let _ = self.tx.try_send(AgentMessage::Reset);
    }
}

/// Messages sent to the memory agent
enum AgentMessage {
    Context {
        session_id: String,
        messages: Arc<[crate::message::Message]>,
        working_dir: Option<String>,
        timestamp: Instant,
    },
    Reset,
}

/// Minimum turns before we consider extracting on topic change
const MIN_TURNS_FOR_EXTRACTION: usize = 4;

/// Trigger a periodic incremental extraction every N turns, even without a topic change.
/// This ensures memories are captured during long single-topic sessions.
const PERIODIC_EXTRACTION_INTERVAL: usize = 12;

/// Skip repeated relevance checks when the formatted context is unchanged.
const RELEVANCE_CONTEXT_REPEAT_SUPPRESSION_SECS: u64 = 30;

fn relevance_context_signature(context: &str) -> String {
    context
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Decide whether to run the expensive Mode-2 listwise rerank this turn.
///
/// - First rerank of a session (`last_rerank_turn == None`) always fires.
/// - A topic change always fires (don't delay a genuine topic jump).
/// - Otherwise the cadence floor applies: fire only if at least `cadence` turns
///   have passed since the last rerank. `cadence <= 1` means every turn.
fn should_run_rerank(
    turn_count: usize,
    last_rerank_turn: Option<usize>,
    cadence: usize,
    topic_changed: bool,
) -> bool {
    if topic_changed {
        return true;
    }
    match last_rerank_turn {
        None => true,
        Some(last) => cadence <= 1 || turn_count.saturating_sub(last) >= cadence,
    }
}

fn retrieval_query<'a>(context: &'a str, focused_query: &'a str) -> &'a str {
    if focused_query.trim().is_empty() {
        context
    } else {
        focused_query
    }
}

fn bump_turn_stat() {
    if let Ok(mut stats) = MEMORY_AGENT_STATS.lock() {
        stats.turns_processed = stats.turns_processed.saturating_add(1);
    }
}

fn record_maintenance_stat(duration_ms: u64) {
    if let Ok(mut stats) = MEMORY_AGENT_STATS.lock() {
        stats.maintenance_runs = stats.maintenance_runs.saturating_add(1);
        stats.last_maintenance_ms = Some(duration_ms);
    }
}

/// Per-session state tracked by the memory agent
#[derive(Default)]
struct SessionState {
    /// Working directory associated with this session.
    working_dir: Option<String>,
    /// Last context embedding (for topic change detection)
    last_context_embedding: Option<Vec<f32>>,
    /// Last context string (for extraction when topic changes)
    last_context_string: Option<String>,
    /// Signature of the last relevance-check context.
    last_relevance_context_signature: Option<String>,
    /// When the last relevance check was started for this session.
    last_relevance_check_at: Option<Instant>,
    /// IDs of memories already surfaced to this session (avoid repetition)
    surfaced_memories: HashSet<String>,
    /// Conversation turn count for this session
    turn_count: usize,
    /// Turn count since last extraction for this session
    turns_since_extraction: usize,
    /// `turn_count` at which the Mode-2 listwise rerank last ran, for the
    /// cadence floor (rerank at most once per `memory_rerank_cadence` turns).
    last_rerank_turn: Option<usize>,
    /// Memory IDs that the last consensus rerank verified as relevant. On
    /// cadence-gated turns we re-surface only these (intersected with the
    /// current candidate set) instead of falling back to the noisy no-LLM
    /// hybrid order, which would otherwise inject low-similarity bloat and
    /// destroy the high-precision guarantee.
    last_verified_ids: Vec<String>,
}

/// The persistent memory agent state
pub struct MemoryAgent {
    /// Channel to receive messages
    rx: mpsc::Receiver<AgentMessage>,

    /// Per-session state keyed by session ID
    sessions: HashMap<String, SessionState>,
}

impl MemoryAgent {
    /// Create a new memory agent
    fn new(rx: mpsc::Receiver<AgentMessage>) -> Self {
        Self {
            rx,
            sessions: HashMap::new(),
        }
    }

    /// Construct a fresh sidecar for an LLM-backed memory operation, but ONLY
    /// when the LLM precision-judge path is actually usable right now (sidecar
    /// mode is enabled AND a real LLM backend is reachable).
    ///
    /// Built fresh on each call rather than cached at construction so that
    /// login changes (gaining or losing access to a provider/credentials) are
    /// reflected immediately without restarting the agent.
    fn live_sidecar(&self) -> Option<Sidecar> {
        memory::memory_llm_judge_available().then(Sidecar::new)
    }

    /// Reset all agent state
    fn reset(&mut self) {
        crate::logging::info(&format!(
            "Memory agent reset: clearing all state ({} sessions)",
            self.sessions.len()
        ));
        self.sessions.clear();
        memory::clear_all_injected_memories();
        if let Ok(mut stats) = MEMORY_AGENT_STATS.lock() {
            stats.turns_processed = 0;
            stats.maintenance_runs = 0;
            stats.last_maintenance_ms = None;
        }
    }

    /// Get or create per-session state
    fn session_state(&mut self, session_id: &str) -> &mut SessionState {
        self.sessions.entry(session_id.to_string()).or_default()
    }

    fn manager_for_session(&self, session_id: &str) -> MemoryManager {
        let working_dir = self
            .sessions
            .get(session_id)
            .and_then(|state| state.working_dir.as_deref());
        manager_for_working_dir(working_dir)
    }

    /// Run the memory agent loop
    async fn run(mut self) {
        crate::logging::info("Memory agent started");

        while let Some(msg) = self.rx.recv().await {
            match msg {
                AgentMessage::Reset => {
                    self.reset();
                }
                AgentMessage::Context {
                    session_id,
                    messages,
                    working_dir,
                    timestamp,
                } => {
                    {
                        let ss = self.session_state(&session_id);
                        if working_dir.is_some() {
                            ss.working_dir = working_dir;
                        }
                        ss.turn_count += 1;
                    }
                    bump_turn_stat();

                    {
                        let ss = self.session_state(&session_id);
                        if ss.turn_count.is_multiple_of(TURN_RESET_INTERVAL) {
                            crate::logging::info(&format!(
                                "[{}] Memory agent periodic reset at turn {} (clearing {} surfaced memories)",
                                session_id,
                                ss.turn_count,
                                ss.surfaced_memories.len()
                            ));
                            ss.surfaced_memories.clear();
                        }
                    }

                    if let Err(e) = self.process_context(&session_id, messages, timestamp).await {
                        crate::logging::error(&format!("Memory agent error: {}", e));
                    }
                }
            }
        }

        crate::logging::info("Memory agent stopped");
    }

    /// Process a context update
    async fn process_context(
        &mut self,
        session_id: &str,
        messages: Arc<[crate::message::Message]>,
        _timestamp: Instant,
    ) -> Result<()> {
        let memory_manager = self.manager_for_session(session_id);
        let context = memory::format_context_for_relevance(&messages);
        if context.is_empty() {
            return Ok(());
        }
        // Memory is only productive with the LLM precision judge. If sidecar mode
        // is requested but no LLM backend is reachable (e.g. logged out / lost
        // provider access), go dormant for this turn instead of silently
        // degrading to the low-precision no-LLM hybrid path. Re-checked live, so
        // memory resumes automatically once a login returns.
        if !memory::memory_runtime_active() {
            crate::logging::event_rate_limited(
                crate::logging::LogLevel::Info,
                "memory_runtime_dormant",
                std::time::Duration::from_secs(300),
                "MEMORY_RUNTIME_DORMANT",
                vec![
                    ("session_id", session_id.to_string()),
                    (
                        "reason",
                        "sidecar_mode_without_reachable_llm_backend".to_string(),
                    ),
                ],
            );
            memory::set_state(MemoryState::Idle);
            crate::memory_judge_metrics::record(
                crate::memory_judge_metrics::JudgeDecision::NoBackend,
                session_id,
                0,
            );
            return Ok(());
        }
        // Focused query (latest user intent, boilerplate/tool-noise stripped) used
        // for listwise LLM reranking. Benchmarking showed the cross-encoder/LLM
        // reranker only works with this focused query, not the full noisy window.
        let focused_query = memory::format_focused_query_for_relevance(&messages);
        let retrieval_text = retrieval_query(&context, &focused_query).to_string();

        let context_signature = relevance_context_signature(&context);
        {
            let ss = self.session_state(session_id);
            if ss.last_relevance_context_signature.as_deref() == Some(context_signature.as_str())
                && ss.last_relevance_check_at.is_some_and(|at| {
                    at.elapsed().as_secs() < RELEVANCE_CONTEXT_REPEAT_SUPPRESSION_SECS
                })
            {
                crate::logging::info(&format!(
                    "[{}] Skipping memory relevance check for unchanged context",
                    session_id
                ));
                return Ok(());
            }

            ss.last_relevance_context_signature = Some(context_signature);
            ss.last_relevance_check_at = Some(Instant::now());
        }

        self.session_state(session_id).turns_since_extraction += 1;

        memory::set_state(MemoryState::Embedding);
        memory::add_event(MemoryEventKind::EmbeddingStarted);

        // Step 1: Embed current context (via the active embedding backend:
        // local MiniLM by default, or the remote OpenAI backend when configured).
        let start = Instant::now();
        let context_for_embedding = context.clone();
        let context_embedding = match tokio::task::spawn_blocking(move || {
            crate::embedding_backend::embed_query_active(&context_for_embedding)
        })
        .await
        {
            Ok(Ok((emb, _model))) => emb,
            Ok(Err(e)) => {
                crate::logging::event_rate_limited(
                    crate::logging::LogLevel::Info,
                    "memory_agent_embedding_failed",
                    std::time::Duration::from_secs(60),
                    "MEMORY_EMBEDDING_FAILED",
                    vec![
                        ("session_id", session_id.to_string()),
                        ("error", e.to_string()),
                        ("fallback", "skip_memory_relevance".to_string()),
                    ],
                );
                memory::set_state(MemoryState::Idle);
                return Ok(());
            }
            Err(e) => {
                crate::logging::info(&format!("Embedding task failed: {}", e));
                memory::set_state(MemoryState::Idle);
                return Ok(());
            }
        };

        // Check for topic change (comparing against this session's last embedding)
        let mut topic_changed = false;
        {
            let ss = self.session_state(session_id);
            if let Some(ref last_emb) = ss.last_context_embedding {
                let similarity = embedding::cosine_similarity(&context_embedding, last_emb);
                if similarity < TOPIC_CHANGE_THRESHOLD {
                    topic_changed = true;
                    crate::logging::info(&format!(
                        "[{}] Topic change detected (sim={:.2}), resetting session memory state",
                        session_id, similarity
                    ));
                    crate::memory_log::log_topic_change(
                        session_id,
                        &format!("sim={:.2}", similarity),
                        "new topic detected",
                    );

                    // Extract memories from the PREVIOUS topic before moving on
                    if ss.turns_since_extraction >= MIN_TURNS_FOR_EXTRACTION {
                        if let Some(prev_context) = ss.last_context_string.clone() {
                            crate::logging::info(&format!(
                                "[{}] Triggering incremental extraction ({} turns since last)",
                                session_id, ss.turns_since_extraction
                            ));
                            ss.turns_since_extraction = 0;
                            let _ = ss;
                            self.extract_from_context(session_id, &prev_context, "topic change")
                                .await;
                            let ss = self.session_state(session_id);
                            ss.surfaced_memories.clear();
                        } else {
                            ss.surfaced_memories.clear();
                        }
                    } else {
                        ss.surfaced_memories.clear();
                    }
                    // NOTE: injected-memory tracking is intentionally NOT
                    // cleared here. Topic changes fire frequently on real
                    // sessions (consecutive coding turns often drop below the
                    // similarity threshold), and the previously injected
                    // memories are still in the transcript, so the model
                    // already knows them. `surfaced_memories` (pending
                    // payloads that may never have been consumed) is cleared
                    // so a new topic can re-surface them; actually-injected
                    // IDs age out via the TTL in `memory::pending` instead.
                }
            }
        }

        // Store current context for potential future extraction
        {
            let ss = self.session_state(session_id);
            ss.last_context_embedding = Some(context_embedding.clone());
            ss.last_context_string = Some(context.clone());
        }

        // Hybrid retrieval must use the same representation for both halves of
        // the fusion. Keep the broad context embedding above for topic-change
        // tracking, but embed the focused lexical query for dense retrieval.
        let retrieval_embedding = if retrieval_text == context {
            context_embedding.clone()
        } else {
            let text = retrieval_text.clone();
            match tokio::task::spawn_blocking(move || {
                crate::embedding_backend::embed_query_active(&text)
            })
            .await
            {
                Ok(Ok((embedding, _model))) => embedding,
                Ok(Err(e)) => {
                    crate::logging::event_rate_limited(
                        crate::logging::LogLevel::Info,
                        "memory_agent_retrieval_embedding_failed",
                        std::time::Duration::from_secs(60),
                        "MEMORY_RETRIEVAL_EMBEDDING_FAILED",
                        vec![
                            ("session_id", session_id.to_string()),
                            ("error", e.to_string()),
                            ("fallback", "skip_memory_relevance".to_string()),
                        ],
                    );
                    memory::set_state(MemoryState::Idle);
                    return Ok(());
                }
                Err(e) => {
                    crate::logging::info(&format!("Retrieval embedding task failed: {}", e));
                    memory::set_state(MemoryState::Idle);
                    return Ok(());
                }
            }
        };

        // Periodic extraction: even without topic change, extract every N turns
        {
            let ss = self.session_state(session_id);
            if ss.turns_since_extraction >= PERIODIC_EXTRACTION_INTERVAL {
                let extraction_ctx = memory::format_context_for_extraction(&messages);
                if extraction_ctx.len() >= 200 {
                    crate::logging::info(&format!(
                        "[{}] Triggering periodic extraction ({} turns since last, {} chars context)",
                        session_id,
                        ss.turns_since_extraction,
                        extraction_ctx.len()
                    ));
                    ss.turns_since_extraction = 0;
                    let _ = ss;
                    self.extract_from_context(session_id, &extraction_ctx, "periodic")
                        .await;
                }
            }
        }

        // Step 2: Find candidate memories via hybrid retrieval (dense + BM25
        // fused with RRF). Benchmarking showed the old dense-only path with a
        // 0.5 cosine floor surfaced essentially nothing on real session windows;
        // hybrid recovers recall and lets the sidecar/rerank do the filtering.
        let candidates = memory_manager.find_similar_hybrid(
            &retrieval_text,
            &retrieval_embedding,
            memory::EMBEDDING_MAX_HITS,
        )?;

        let embedding_latency = start.elapsed().as_millis() as u64;
        memory::add_event(MemoryEventKind::EmbeddingComplete {
            latency_ms: embedding_latency,
            hits: candidates.len(),
        });

        if candidates.is_empty() {
            memory::set_state(MemoryState::Idle);
            return Ok(());
        }

        // Filter out already-surfaced memories (per-session + global injection tracking)
        let total_before_filter = candidates.len();
        let new_candidates: Vec<_> = {
            let ss = self.session_state(session_id);
            candidates
                .into_iter()
                .filter(|(entry, _)| {
                    !ss.surfaced_memories.contains(&entry.id)
                        && !memory::is_memory_injected(session_id, &entry.id)
                })
                .collect()
        };

        crate::memory_log::log_candidate_filter(
            session_id,
            total_before_filter,
            new_candidates.len(),
            &context,
        );

        if new_candidates.is_empty() {
            memory::set_state(MemoryState::Idle);
            return Ok(());
        }

        // Step 3: Decide which candidates to surface.
        // Mode-2 (sidecar enabled): a single listwise LLM rerank reorders the
        // hybrid candidates by relevance to the focused query and omits
        // irrelevant ones; we surface the top MAX_MEMORIES_PER_TURN. This matches
        // the validated benchmark pipeline (recall@5 0.53 -> 0.75) and uses ONE
        // LLM call instead of the old per-candidate binary checks.
        // Mode-1 (no sidecar): take the top hybrid-ranked candidates by score.
        memory::set_state(MemoryState::SidecarChecking {
            count: new_candidates.len(),
        });
        memory::add_event(MemoryEventKind::SidecarStarted);

        let candidate_ids: Vec<String> = new_candidates.iter().map(|(e, _)| e.id.clone()).collect();

        // Cadence gate for the EXPENSIVE Mode-2 rerank: run the listwise LLM
        // rerank at most once per `memory_rerank_cadence` turns. Skipped turns
        // re-surface only the last judge-verified set (never unvetted hybrid),
        // so precision is preserved between reranks. A topic change or the first
        // rerank of a session always fires, so genuine topic jumps are never
        // delayed.
        let should_rerank = {
            let cadence = crate::config::config().agents.memory_rerank_cadence;
            let ss = self.session_state(session_id);
            should_run_rerank(ss.turn_count, ss.last_rerank_turn, cadence, topic_changed)
        };

        let relevant = if let Some(sidecar) = self.live_sidecar() {
            if should_rerank {
                let agents = &crate::config::config().agents;
                let votes = agents.memory_rerank_votes.max(1);
                let min_agree = agents.memory_rerank_min_agree.clamp(1, votes);
                // Record the attempt before awaiting the judge. Failed attempts
                // still obey the configured cadence instead of retrying on every
                // subsequent context update.
                let attempt_turn = self.session_state(session_id).turn_count;
                self.session_state(session_id).last_rerank_turn = Some(attempt_turn);
                let (reranked, outcome) =
                    crate::memory_rerank::rerank_candidates_consensus_attributed(
                        &sidecar,
                        &focused_query,
                        new_candidates.clone(),
                        votes,
                        min_agree,
                    )
                    .await;
                // Attribute exactly why this turn surfaced what it did: a judged
                // verdict is the productive path; any rerank failure (transport
                // error / unparseable / all judges failed) is a no-LLM
                // degradation we want to drive to zero.
                crate::memory_judge_metrics::record(
                    crate::memory_judge_metrics::JudgeDecision::from_rerank_outcome(outcome),
                    session_id,
                    candidate_ids.len(),
                );
                if outcome == crate::memory_rerank::RerankOutcome::Judged {
                    // Real judge verdict: surface it and remember it as the new
                    // verified set for future cadence/failure carries.
                    let result: Vec<_> = reranked.into_iter().take(MAX_MEMORIES_PER_TURN).collect();
                    {
                        let ss = self.session_state(session_id);
                        ss.last_verified_ids = result.iter().map(|e| e.id.clone()).collect();
                    }
                    result
                } else {
                    // Judge FAILED this turn (rerank returned empty). Do NOT inject
                    // unvetted hybrid order; carry the last judge-verified set so
                    // everything surfaced stays judge-backed. The failed attempt still
                    // advances the cadence, while the global circuit breaker suppresses
                    // cross-session retry storms.
                    let carried = self.carry_verified(session_id, new_candidates);
                    crate::logging::event_rate_limited(
                        crate::logging::LogLevel::Info,
                        "memory_judge_failed_carry",
                        std::time::Duration::from_secs(60),
                        "Memory judge unavailable; carrying previously verified memories",
                        vec![
                            ("session_id", session_id.to_string()),
                            ("outcome", format!("{outcome:?}")),
                            ("carried", carried.len().to_string()),
                        ],
                    );
                    carried
                }
            } else {
                // Cadence-gated turn: re-surface ONLY the memories the last
                // consensus rerank verified (intersected with the current
                // candidate set), preserving high precision. Falling back to the
                // noisy no-LLM hybrid order here would inject low-similarity
                // bloat (the exact behavior we are trying to avoid).
                crate::memory_judge_metrics::record(
                    crate::memory_judge_metrics::JudgeDecision::CadenceCarry,
                    session_id,
                    candidate_ids.len(),
                );
                let carried = self.carry_verified(session_id, new_candidates);
                crate::logging::info(&format!(
                    "[{}] Memory rerank gated by cadence; re-surfacing {} consensus-verified memories",
                    session_id,
                    carried.len()
                ));
                carried
            }
        } else {
            // No LLM judge. This is reached only when the user explicitly opted
            // OUT of the sidecar (`memory_sidecar_enabled = false`); when sidecar
            // mode is on but no LLM backend is reachable, `process_context`
            // returns early before this point (memory goes dormant rather than
            // degrading to the low-precision no-LLM path).
            crate::memory_judge_metrics::record(
                crate::memory_judge_metrics::JudgeDecision::OptedOut,
                session_id,
                candidate_ids.len(),
            );
            self.select_top_candidates_no_sidecar(session_id, new_candidates)
        };

        let verified_ids: Vec<String> = relevant.iter().map(|e| e.id.clone()).collect();
        let rejected_ids: Vec<String> = candidate_ids
            .iter()
            .filter(|id| !verified_ids.contains(id))
            .cloned()
            .collect();

        let retrieval_ctx = RetrievalContext {
            verified_ids: verified_ids.clone(),
            rejected_ids,
            context_snippet: jcode_core::util::truncate_str(&context, 200).to_string(),
        };

        // Step 4: Format and store for main agent
        if !relevant.is_empty() {
            let ids: Vec<String> = relevant.iter().map(|e| e.id.clone()).collect();
            {
                let ss = self.session_state(session_id);
                for entry in &relevant {
                    ss.surfaced_memories.insert(entry.id.clone());
                }
            }

            if let Some(prompt) = memory::format_relevant_prompt(&relevant, MAX_MEMORIES_PER_TURN) {
                let display_prompt =
                    memory::format_relevant_display_prompt(&relevant, MAX_MEMORIES_PER_TURN);
                let count = prompt
                    .lines()
                    .map(str::trim_start)
                    .filter(|line| {
                        line.split_once(". ")
                            .map(|(prefix, _)| {
                                !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_digit())
                            })
                            .unwrap_or(false)
                    })
                    .count()
                    .max(1);

                memory::set_pending_memory_with_ids_and_display(
                    session_id,
                    prompt,
                    count,
                    ids,
                    display_prompt,
                );
                memory::set_state(MemoryState::FoundRelevant { count });
            } else {
                memory::set_state(MemoryState::Idle);
            }
        } else {
            memory::set_state(MemoryState::Idle);
        }

        // Step 5: Post-retrieval maintenance (runs in background)
        self.post_retrieval_maintenance(memory_manager, retrieval_ctx)
            .await;

        Ok(())
    }

    /// Mode-1 (no sidecar) candidate selection: a score-relative dynamic gate
    /// over the hybrid-ranked candidates. Returns a VARIABLE number of memories
    /// (1..=`MAX_MEMORIES_PER_TURN`) instead of always padding to a fixed top-k,
    /// cutting the tail at the first large score gap. See `GATE_REL_FLOOR` /
    /// `GATE_DROP_RATIO` for the rationale and benchmark numbers.
    ///
    /// In Mode-2 the listwise LLM reranker (`memory_rerank::rerank_candidates`)
    /// handles relevance selection instead (and can drop to 0), so this is only
    /// reached when the memory sidecar is disabled (no LLM available to judge
    /// relevance) or on a cadence-gated turn.
    fn select_top_candidates_no_sidecar(
        &self,
        session_id: &str,
        candidates: Vec<(MemoryEntry, f32)>,
    ) -> Vec<MemoryEntry> {
        let selected = dynamic_gate_select(candidates, MAX_MEMORIES_PER_TURN);
        for (entry, sim) in &selected {
            crate::logging::info(&format!(
                "[{}] Memory relevant (semantic sim={:.2}): {}",
                session_id,
                sim,
                jcode_core::util::truncate_str(&entry.content, 40)
            ));
        }
        selected.into_iter().map(|(entry, _)| entry).collect()
    }

    /// Re-surface ONLY the memories the last consensus rerank verified,
    /// intersected with the current candidate set. Used both for cadence-gated
    /// turns and as the fallback when a judge fails this turn: in either case we
    /// ride the last judge verdict rather than dropping to unvetted hybrid order.
    /// No prior verdict (or no overlap) -> surface nothing. This keeps the LLM
    /// judge the ONLY thing that can put a memory in front of the agent.
    fn carry_verified(
        &mut self,
        session_id: &str,
        candidates: Vec<(MemoryEntry, f32)>,
    ) -> Vec<MemoryEntry> {
        let verified: HashSet<String> = self
            .session_state(session_id)
            .last_verified_ids
            .iter()
            .cloned()
            .collect();
        candidates
            .into_iter()
            .filter(|(e, _)| verified.contains(&e.id))
            .map(|(e, _)| e)
            .collect()
    }

    /// Extract memories from a context string
    ///
    /// This is an incremental extraction - we extract from a portion of the
    /// conversation (on topic change or periodically) rather than waiting for session end.
    async fn extract_from_context(&self, session_id: &str, context: &str, reason: &str) {
        // Memory extraction requires the LLM. Skip when sidecar mode is off OR
        // (sidecar mode on but) no LLM backend is reachable. Re-checked live so a
        // login change is reflected without a restart.
        let Some(sidecar) = self.live_sidecar() else {
            crate::logging::info(&format!(
                "Incremental extraction skipped for session {}: LLM judge unavailable",
                session_id
            ));
            return;
        };

        // Don't extract from very short contexts
        if context.len() < 200 {
            return;
        }

        // Update UI state
        memory::set_state(MemoryState::Extracting {
            reason: reason.to_string(),
        });
        memory::add_event(MemoryEventKind::ExtractionStarted {
            reason: reason.to_string(),
        });

        let memory_manager = self.manager_for_session(session_id);
        let context_owned = context.to_string();
        let session_id_owned = session_id.to_string();

        let existing: Vec<String> = {
            let context_summary = if context_owned.len() > 2000 {
                &context_owned[context_owned.len() - 2000..]
            } else {
                &context_owned
            };
            match memory_manager.find_similar(context_summary, 0.25, 80) {
                Ok(similar) if !similar.is_empty() => similar
                    .into_iter()
                    .map(|(entry, _score)| entry.content)
                    .collect(),
                _ => memory_manager
                    .list_all()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|e| e.active)
                    .take(40)
                    .map(|e| e.content)
                    .collect(),
            }
        };

        // Similarity threshold for duplicate detection
        const DUPLICATE_THRESHOLD: f32 = 0.90;

        // Run extraction in background - don't block the main flow
        tokio::spawn(async move {
            match sidecar
                .extract_memories_with_existing(&context_owned, &existing)
                .await
            {
                Ok(extracted) if !extracted.is_empty() => {
                    let mut stored_count = 0;
                    let mut stored_ids: Vec<String> = Vec::new();
                    let mut known_ids: Vec<String> = Vec::new();
                    let mut reinforced_count = 0;
                    let mut superseded_count = 0;

                    for mem in extracted {
                        let category = match mem.category.as_str() {
                            "fact" => memory::MemoryCategory::Fact,
                            "preference" => memory::MemoryCategory::Preference,
                            "correction" => memory::MemoryCategory::Correction,
                            _ => memory::MemoryCategory::Fact,
                        };

                        let trust = match mem.trust.as_str() {
                            "high" => memory::TrustLevel::High,
                            "low" => memory::TrustLevel::Low,
                            _ => memory::TrustLevel::Medium,
                        };

                        // Check for duplicate: find semantically similar existing memories
                        let similar =
                            memory_manager.find_similar(&mem.content, DUPLICATE_THRESHOLD, 1);

                        if let Ok(matches) = similar
                            && let Some((existing, _sim)) = matches.first()
                        {
                            let existing_id = existing.id.clone();
                            let mut did_reinforce = false;

                            if let Ok(mut graph) = memory_manager.load_project_graph()
                                && graph.get_memory(&existing_id).is_some()
                            {
                                let strength = if let Some(entry) =
                                    graph.get_memory_mut(&existing_id)
                                {
                                    entry.reinforce("incremental", 0);
                                    entry.strength
                                } else {
                                    crate::logging::warn(&format!(
                                        "Expected project memory {} during reinforcement, but it disappeared before update",
                                        existing_id
                                    ));
                                    continue;
                                };
                                if memory_manager.save_project_graph(&graph).is_ok() {
                                    did_reinforce = true;
                                    crate::logging::info(&format!(
                                        "Reinforced existing memory {} (strength={})",
                                        existing_id, strength
                                    ));
                                }
                            }

                            if !did_reinforce
                                && let Ok(mut graph) = memory_manager.load_global_graph()
                                && graph.get_memory(&existing_id).is_some()
                            {
                                if let Some(entry) = graph.get_memory_mut(&existing_id) {
                                    entry.reinforce("incremental", 0);
                                    let _ = memory_manager.save_global_graph(&graph);
                                    did_reinforce = true;
                                } else {
                                    crate::logging::warn(&format!(
                                        "Expected global memory {} during reinforcement, but it disappeared before update",
                                        existing_id
                                    ));
                                }
                            }

                            if did_reinforce {
                                reinforced_count += 1;
                                known_ids.push(existing_id.clone());
                            }
                            continue;
                        }

                        // No duplicate - check for contradiction in same category
                        let contradiction_found =
                            match memory_manager.find_similar(&mem.content, 0.5, 5) {
                                Ok(candidates) => {
                                    let mut found = None;
                                    for (candidate, _) in &candidates {
                                        if candidate.category == category {
                                            match sidecar
                                                .check_contradiction(
                                                    &mem.content,
                                                    &candidate.content,
                                                )
                                                .await
                                            {
                                                Ok(true) => {
                                                    found = Some(candidate.id.clone());
                                                    break;
                                                }
                                                Ok(false) => {}
                                                Err(e) => {
                                                    crate::logging::info(&format!(
                                                        "Contradiction check failed: {}",
                                                        e
                                                    ));
                                                }
                                            }
                                        }
                                    }
                                    found
                                }
                                Err(_) => None,
                            };

                        // Create the new memory
                        let entry = memory::MemoryEntry::new(category, &mem.content)
                            .with_source("incremental")
                            .with_trust(trust);

                        match memory_manager.remember_project(entry) {
                            Ok(new_id) => {
                                stored_count += 1;
                                stored_ids.push(new_id.clone());

                                // If contradiction found, supersede the old memory and add Contradicts edge
                                if let Some(old_id) = contradiction_found
                                    && let Ok(mut graph) = memory_manager.load_project_graph()
                                {
                                    graph.mark_contradiction(&new_id, &old_id);
                                    if let Some(old_entry) = graph.get_memory_mut(&old_id) {
                                        old_entry.supersede(&new_id);
                                    }
                                    if memory_manager.save_project_graph(&graph).is_ok() {
                                        superseded_count += 1;
                                        crate::logging::info(&format!(
                                            "Superseded memory {} with {} (Contradicts edge added)",
                                            old_id, new_id
                                        ));
                                    }
                                }
                            }
                            Err(e) => {
                                crate::logging::info(&format!("Failed to store memory: {}", e));
                            }
                        }
                    }

                    // Create DerivedFrom edges between co-extracted memories
                    if stored_ids.len() >= 2
                        && let Ok(mut graph) = memory_manager.load_project_graph()
                    {
                        let mut linked = false;
                        for i in 0..stored_ids.len() {
                            for j in (i + 1)..stored_ids.len() {
                                graph.add_edge(
                                    &stored_ids[i],
                                    &stored_ids[j],
                                    crate::memory_graph::EdgeKind::DerivedFrom,
                                );
                                linked = true;
                            }
                        }
                        if linked {
                            let _ = memory_manager.save_project_graph(&graph);
                        }
                    }

                    let total = stored_count + reinforced_count;
                    if total > 0 {
                        crate::logging::info(&format!(
                            "Incremental extraction: {} stored, {} reinforced, {} superseded",
                            stored_count, reinforced_count, superseded_count
                        ));
                        memory::add_event(MemoryEventKind::ExtractionComplete { count: total });
                    }

                    // The session this transcript came from already contains
                    // this information verbatim; re-injecting freshly
                    // extracted (or just-reinforced) memories back into it
                    // would be a pure echo. Mark them as known so retrieval
                    // skips them for this session (other sessions still see
                    // them normally).
                    known_ids.extend(stored_ids.iter().cloned());
                    memory::mark_memories_known(
                        &session_id_owned,
                        &known_ids,
                        "extracted from this session's transcript",
                    );
                    memory::set_state(MemoryState::Idle);
                }
                Ok(_) => {
                    // No memories extracted - that's fine
                    memory::set_state(MemoryState::Idle);
                }
                Err(e) => {
                    crate::logging::info(&format!("Incremental extraction failed: {}", e));
                    memory::add_event(MemoryEventKind::Error {
                        message: e.to_string(),
                    });
                    memory::set_state(MemoryState::Idle);
                }
            }
        });
    }

    /// Post-retrieval maintenance tasks
    ///
    /// After serving memories, we can use the retrieval context to:
    /// 1. Create links between co-relevant memories
    /// 2. Boost confidence for verified memories
    /// 3. Decay confidence for rejected memories
    /// 4. Log memory gaps for future learning
    async fn post_retrieval_maintenance(
        &self,
        memory_manager: MemoryManager,
        ctx: RetrievalContext,
    ) {
        memory::set_state(MemoryState::Maintaining {
            phase: "graph upkeep".to_string(),
        });
        memory::add_event(MemoryEventKind::MaintenanceStarted {
            verified: ctx.verified_ids.len(),
            rejected: ctx.rejected_ids.len(),
        });
        memory::pipeline_update(|p| {
            p.maintain = StepStatus::Running;
        });

        // Run maintenance in background - don't block retrieval flow
        tokio::spawn(async move {
            let started = Instant::now();

            // 1. Link discovery: Create RelatesTo edges between co-relevant memories
            let mut links = 0usize;
            if ctx.verified_ids.len() >= 2 {
                match discover_links(&memory_manager, &ctx.verified_ids).await {
                    Ok(count) => {
                        links = count;
                        if count > 0 {
                            memory::add_event(MemoryEventKind::MaintenanceLinked { links: count });
                        }
                    }
                    Err(e) => {
                        crate::logging::info(&format!("Link discovery failed: {}", e));
                    }
                }
            }

            // 2 + 3. Batch confidence updates: boost verified, decay rejected.
            // Each graph is loaded and saved ONCE for the whole turn instead of
            // once per id (graphs are multi-MB JSON; per-id round trips rewrote
            // megabytes 5-10x per turn).
            let (boosted, decayed) =
                apply_confidence_updates(&memory_manager, &ctx.verified_ids, &ctx.rejected_ids);
            if boosted > 0 || decayed > 0 {
                memory::add_event(MemoryEventKind::MaintenanceConfidence { boosted, decayed });
            }

            // 4. Gap detection: Log when we had no relevant memories
            if ctx.verified_ids.is_empty() && !ctx.rejected_ids.is_empty() {
                memory::add_event(MemoryEventKind::MaintenanceGap {
                    candidates: ctx.rejected_ids.len(),
                });
                crate::logging::info(&format!(
                    "Memory gap detected: {} candidates retrieved but none relevant. Context: {}...",
                    ctx.rejected_ids.len(),
                    jcode_core::util::truncate_str(&ctx.context_snippet, 100)
                ));
            }

            // 5. Periodic cluster refinement
            let tick = MAINTENANCE_TICK.fetch_add(1, Ordering::Relaxed) + 1;
            if tick.is_multiple_of(CLUSTER_REFINEMENT_INTERVAL) && ctx.verified_ids.len() >= 2 {
                match refine_clusters(&memory_manager, &ctx.verified_ids).await {
                    Ok(stats) => {
                        if stats.clusters_touched > 0 {
                            memory::add_event(MemoryEventKind::MaintenanceCluster {
                                clusters: stats.clusters_touched,
                                members: stats.member_links,
                            });
                        }
                    }
                    Err(e) => {
                        crate::logging::info(&format!("Cluster refinement failed: {}", e));
                    }
                }
            }

            // 6. Tag inference from shared context
            if ctx.verified_ids.len() >= 2 {
                match infer_context_tag(&memory_manager, &ctx.verified_ids, &ctx.context_snippet) {
                    Ok(Some((tag, applied))) => {
                        memory::add_event(MemoryEventKind::MaintenanceTagInferred { tag, applied });
                    }
                    Ok(None) => {}
                    Err(e) => {
                        crate::logging::info(&format!("Tag inference failed: {}", e));
                    }
                }
            }

            // 7. Periodic garbage collection: prune low-confidence memories
            let mut pruned = 0usize;
            if tick.is_multiple_of(CLUSTER_REFINEMENT_INTERVAL * 5) {
                match prune_low_confidence(&memory_manager) {
                    Ok(count) => pruned = count,
                    Err(e) => {
                        crate::logging::info(&format!("Memory pruning failed: {}", e));
                    }
                }
            }

            let latency_ms = started.elapsed().as_millis() as u64;
            record_maintenance_stat(latency_ms);
            memory::add_event(MemoryEventKind::MaintenanceComplete { latency_ms });
            memory::pipeline_update(|p| {
                p.maintain = StepStatus::Done;
                p.maintain_result = Some(StepResult {
                    summary: format!("{}L {}↑ {}↓ {}P", links, boosted, decayed, pruned),
                    latency_ms,
                });
            });
            memory::set_state(MemoryState::Idle);
            crate::logging::info(&format!(
                "Memory maintenance complete: links={}, boosted={}, decayed={}, {}ms",
                links, boosted, decayed, latency_ms
            ));
        });
    }
}

#[derive(Debug, Default)]
struct ClusterRefinementStats {
    clusters_touched: usize,
    member_links: usize,
    cluster_id: Option<String>,
}

async fn refine_clusters(
    manager: &MemoryManager,
    verified_ids: &[String],
) -> Result<ClusterRefinementStats> {
    if verified_ids.len() < 2 {
        return Ok(ClusterRefinementStats::default());
    }

    let mut project_graph = manager.load_project_graph()?;
    let mut global_graph = manager.load_global_graph()?;
    let now = Utc::now();

    let project_ids: Vec<String> = verified_ids
        .iter()
        .filter(|id| project_graph.memories.contains_key(*id))
        .cloned()
        .collect();
    let global_ids: Vec<String> = verified_ids
        .iter()
        .filter(|id| global_graph.memories.contains_key(*id))
        .cloned()
        .collect();

    let mut out = ClusterRefinementStats::default();
    let mut project_changed = false;
    let mut global_changed = false;

    if project_ids.len() >= 2 {
        let stats = apply_cluster_assignment(&mut project_graph, "project", &project_ids, now);
        if stats.clusters_touched > 0 {
            out.clusters_touched += stats.clusters_touched;
            out.member_links += stats.member_links;
            project_changed = true;

            if let Some(cluster_id) = stats.cluster_id.as_ref()
                && project_graph
                    .clusters
                    .get(cluster_id)
                    .and_then(|c| c.name.as_deref())
                    .map(|n| n.ends_with("co-relevance"))
                    .unwrap_or(false)
            {
                let member_contents: Vec<String> = project_ids
                    .iter()
                    .filter_map(|id| project_graph.get_memory(id))
                    .map(|m| jcode_core::util::truncate_str(&m.content, 80).to_string())
                    .collect();
                if let Ok(name) = name_cluster_with_sidecar(&member_contents).await
                    && let Some(cluster) = project_graph.clusters.get_mut(cluster_id)
                {
                    cluster.name = Some(name);
                }
            }
        }
    }
    if global_ids.len() >= 2 {
        let stats = apply_cluster_assignment(&mut global_graph, "global", &global_ids, now);
        if stats.clusters_touched > 0 {
            out.clusters_touched += stats.clusters_touched;
            out.member_links += stats.member_links;
            global_changed = true;
        }
    }

    if project_changed {
        manager.save_project_graph(&project_graph)?;
    }
    if global_changed {
        manager.save_global_graph(&global_graph)?;
    }

    Ok(out)
}

async fn name_cluster_with_sidecar(member_contents: &[String]) -> Result<String> {
    if !memory::memory_sidecar_enabled() {
        let fallback = infer_candidate_tag(&member_contents.join(" "))
            .unwrap_or_else(|| "shared context".to_string());
        return Ok(fallback);
    }

    let sidecar = Sidecar::new();
    let mut prompt = String::from(
        "These memories were retrieved together. Give this cluster a short descriptive name (2-4 words, no quotes):\n",
    );
    for (i, content) in member_contents.iter().enumerate() {
        prompt.push_str(&format!("{}. {}\n", i + 1, content));
    }
    let name = sidecar
        .complete(
            "You name memory clusters. Reply with ONLY the cluster name, 2-4 words, no quotes or punctuation.",
            &prompt,
        )
        .await?;
    let name = name.trim().to_string();
    if name.is_empty() || name.len() > 60 {
        anyhow::bail!("Invalid cluster name");
    }
    Ok(name)
}

fn apply_cluster_assignment(
    graph: &mut MemoryGraph,
    scope: &str,
    member_ids: &[String],
    now: chrono::DateTime<Utc>,
) -> ClusterRefinementStats {
    let mut members: Vec<String> = member_ids.to_vec();
    members.sort();
    members.dedup();
    if members.len() < 2 {
        return ClusterRefinementStats::default();
    }

    let cluster_key = format!("auto-{}-{:016x}", scope, stable_hash(&members));
    let cluster_id = format!("cluster:{}", cluster_key);
    let centroid = average_embedding(graph, &members);

    {
        let cluster = graph
            .clusters
            .entry(cluster_id.clone())
            .or_insert_with(|| ClusterEntry::new(cluster_key.clone()));
        if cluster.name.is_none() {
            cluster.name = Some(format!("{} co-relevance", scope));
        }
        cluster.member_count = members.len() as u32;
        cluster.updated_at = now;
        cluster.centroid = centroid;
    }

    graph.metadata.last_cluster_update = Some(now);

    let mut linked = 0usize;
    for id in members {
        if !graph.memories.contains_key(&id) {
            continue;
        }
        let before = graph.get_edges(&id).len();
        graph.add_edge(&id, &cluster_id, EdgeKind::InCluster);
        let after = graph.get_edges(&id).len();
        if after > before {
            linked += 1;
        }
    }

    ClusterRefinementStats {
        clusters_touched: 1,
        member_links: linked,
        cluster_id: Some(cluster_id),
    }
}

fn prune_low_confidence(manager: &MemoryManager) -> Result<usize> {
    let min_confidence = 0.15;
    let min_age_hours = 24;
    let now = Utc::now();
    let mut pruned = 0usize;

    for scope in &["project", "global"] {
        let mut graph = if *scope == "project" {
            manager.load_project_graph()?
        } else {
            manager.load_global_graph()?
        };

        let ids_to_prune: Vec<String> = graph
            .memories
            .iter()
            .filter(|(_, entry)| {
                let age_hours = (now - entry.created_at).num_hours();
                age_hours >= min_age_hours && entry.confidence < min_confidence
            })
            .map(|(id, _)| id.clone())
            .collect();

        if ids_to_prune.is_empty() {
            continue;
        }

        for id in &ids_to_prune {
            graph.remove_memory(id);
            pruned += 1;
        }

        if *scope == "project" {
            manager.save_project_graph(&graph)?;
        } else {
            manager.save_global_graph(&graph)?;
        }

        if !ids_to_prune.is_empty() {
            crate::logging::info(&format!(
                "Pruned {} low-confidence {} memories (conf < {}, age >= {}h)",
                ids_to_prune.len(),
                scope,
                min_confidence,
                min_age_hours
            ));
        }
    }

    Ok(pruned)
}

fn stable_hash(values: &[String]) -> u64 {
    // Deterministic FNV-1a hash to keep auto-cluster IDs stable across runs.
    let mut hash: u64 = 0xcbf29ce484222325;
    for value in values {
        for byte in value.as_bytes() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    hash
}

fn average_embedding(graph: &MemoryGraph, member_ids: &[String]) -> Vec<f32> {
    let mut sum: Vec<f32> = Vec::new();
    let mut count = 0usize;

    for id in member_ids {
        let Some(emb) = graph.memories.get(id).and_then(|m| m.embedding.as_ref()) else {
            continue;
        };
        if sum.is_empty() {
            sum = vec![0.0; emb.len()];
        }
        if emb.len() != sum.len() {
            continue;
        }
        for (slot, value) in sum.iter_mut().zip(emb.iter()) {
            *slot += *value;
        }
        count += 1;
    }

    if count == 0 {
        return Vec::new();
    }

    let denom = count as f32;
    for value in &mut sum {
        *value /= denom;
    }
    sum
}

fn infer_context_tag(
    manager: &MemoryManager,
    verified_ids: &[String],
    context_snippet: &str,
) -> Result<Option<(String, usize)>> {
    if verified_ids.len() < 2 {
        return Ok(None);
    }

    let project_graph = manager.load_project_graph()?;
    let global_graph = manager.load_global_graph()?;

    let mut tag_sets: Vec<HashSet<String>> = Vec::new();
    for id in verified_ids {
        let Some(memory) = project_graph
            .memories
            .get(id)
            .or_else(|| global_graph.memories.get(id))
        else {
            continue;
        };
        tag_sets.push(memory.tags.iter().map(|t| t.to_ascii_lowercase()).collect());
    }

    if tag_sets.len() < 2 {
        return Ok(None);
    }

    let mut common = tag_sets[0].clone();
    for tags in tag_sets.iter().skip(1) {
        common.retain(|tag| tags.contains(tag));
    }
    if !common.is_empty() {
        return Ok(None);
    }

    let Some(tag) = infer_candidate_tag(context_snippet) else {
        return Ok(None);
    };

    let mut applied = 0usize;
    for id in verified_ids {
        let already_tagged = project_graph
            .memories
            .get(id)
            .or_else(|| global_graph.memories.get(id))
            .map(|m| m.tags.iter().any(|t| t.eq_ignore_ascii_case(&tag)))
            .unwrap_or(false);
        if already_tagged {
            continue;
        }
        if manager.tag_memory(id, &tag).is_ok() {
            applied += 1;
        }
    }

    if applied > 0 {
        Ok(Some((tag, applied)))
    } else {
        Ok(None)
    }
}

fn infer_candidate_tag(context: &str) -> Option<String> {
    const STOPWORDS: &[&str] = &[
        "about", "after", "again", "agent", "also", "because", "before", "being", "build", "check",
        "code", "context", "could", "debug", "extract", "from", "have", "into", "just", "memory",
        "might", "project", "really", "should", "that", "their", "there", "these", "they", "this",
        "those", "very", "what", "when", "with", "would", "your",
    ];

    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut token = String::new();
    let mut flush = |raw: &mut String| {
        if raw.is_empty() {
            return;
        }
        let candidate = raw.to_ascii_lowercase();
        raw.clear();
        if candidate.len() < 4 || candidate.len() > 32 {
            return;
        }
        if candidate.chars().all(|ch| ch.is_ascii_digit()) {
            return;
        }
        if STOPWORDS.contains(&candidate.as_str()) {
            return;
        }
        *counts.entry(candidate).or_insert(0) += 1;
    };

    for ch in context.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            token.push(ch);
        } else {
            flush(&mut token);
        }
    }
    flush(&mut token);

    counts
        .into_iter()
        .filter(|(_, count)| *count >= 2)
        .max_by_key(|(_, count)| *count)
        .map(|(tag, _)| tag)
}

/// Discover links between co-relevant memories
async fn discover_links(manager: &MemoryManager, memory_ids: &[String]) -> Result<usize> {
    // For each pair of co-relevant memories, create a RelatesTo link
    // Use a moderate weight since we're inferring the relationship
    const LINK_WEIGHT: f32 = 0.6;
    let mut linked = 0usize;

    for i in 0..memory_ids.len() {
        for j in (i + 1)..memory_ids.len() {
            let from = &memory_ids[i];
            let to = &memory_ids[j];

            // Try to link (may fail if memories are in different stores)
            match manager.link_memories(from, to, LINK_WEIGHT) {
                Ok(()) => linked += 1,
                Err(e) => {
                    // This is expected for cross-store memories, just log at debug level
                    crate::logging::info(&format!("Could not link {} -> {}: {}", from, to, e));
                }
            }
        }
    }

    Ok(linked)
}

/// Apply confidence boosts (verified) and decays (rejected) in a single pass
/// over each graph. Loads and saves the project and global graphs at most ONCE
/// each, instead of once per id, to avoid rewriting multi-MB JSON repeatedly.
///
/// Returns (boosted_count, decayed_count).
fn apply_confidence_updates(
    manager: &MemoryManager,
    verified_ids: &[String],
    rejected_ids: &[String],
) -> (usize, usize) {
    const BOOST: f32 = 0.05;
    const DECAY: f32 = 0.02;

    if verified_ids.is_empty() && rejected_ids.is_empty() {
        return (0, 0);
    }

    let mut boosted = 0usize;
    let mut decayed = 0usize;

    // Process project then global; an id lives in exactly one graph, so once an
    // update lands we don't need to touch it again.
    for scope in ["project", "global"] {
        let mut graph = match if scope == "project" {
            manager.load_project_graph()
        } else {
            manager.load_global_graph()
        } {
            Ok(g) => g,
            Err(e) => {
                crate::logging::info(&format!(
                    "Confidence update: failed to load {} graph: {}",
                    scope, e
                ));
                continue;
            }
        };

        let mut changed = false;
        for id in verified_ids {
            if let Some(entry) = graph.get_memory_mut(id) {
                entry.boost_confidence(BOOST);
                boosted += 1;
                changed = true;
            }
        }
        for id in rejected_ids {
            if let Some(entry) = graph.get_memory_mut(id) {
                entry.decay_confidence(DECAY);
                decayed += 1;
                changed = true;
            }
        }

        if changed {
            let saved = if scope == "project" {
                manager.save_project_graph(&graph)
            } else {
                manager.save_global_graph(&graph)
            };
            if let Err(e) = saved {
                crate::logging::info(&format!(
                    "Confidence update: failed to save {} graph: {}",
                    scope, e
                ));
            }
        }
    }

    (boosted, decayed)
}

/// Initialize and start the global memory agent
pub async fn init() -> Result<MemoryAgentHandle> {
    let handle = MEMORY_AGENT
        .get_or_init(|| async {
            let (tx, rx) = mpsc::channel(CONTEXT_CHANNEL_CAPACITY);

            // Spawn the memory agent task
            let agent = MemoryAgent::new(rx);
            tokio::spawn(agent.run());

            MemoryAgentHandle { tx }
        })
        .await;

    Ok(handle.clone())
}

/// Get the global memory agent handle (if initialized)
pub fn get() -> Option<MemoryAgentHandle> {
    MEMORY_AGENT.get().cloned()
}

/// Send a context update to the memory agent (convenience function)
pub async fn update_context(
    session_id: &str,
    messages: Arc<[crate::message::Message]>,
    working_dir: Option<String>,
) {
    if let Some(handle) = get() {
        handle
            .update_context(session_id, messages, working_dir)
            .await;
    }
}

/// Send a context update synchronously (for use from non-async code)
/// This is non-blocking - it just sends to the channel
pub fn update_context_sync(session_id: &str, messages: Arc<[crate::message::Message]>) {
    update_context_sync_with_dir(session_id, messages, None);
}

pub fn update_context_sync_with_dir(
    session_id: &str,
    messages: Arc<[crate::message::Message]>,
    working_dir: Option<String>,
) {
    if let Some(handle) = get() {
        handle.update_context_sync_with_dir(session_id, messages, working_dir);
    } else {
        let sid = session_id.to_string();
        tokio::spawn(async move {
            if let Ok(handle) = init().await {
                handle.update_context_sync_with_dir(&sid, messages, working_dir);
            }
        });
    }
}

/// Reset the memory agent state (call on new session)
/// This clears surfaced memories, context embedding, and turn count
pub fn reset() {
    if let Some(handle) = get() {
        handle.reset();
    }
}

/// Trigger a final memory extraction when a session ends.
///
/// This is fire-and-forget: spawns a tokio task that runs extraction
/// and logs the result. Does not block the caller.
pub fn trigger_final_extraction(transcript: String, session_id: String) {
    trigger_final_extraction_with_dir(transcript, session_id, None);
}

pub fn trigger_final_extraction_with_dir(
    transcript: String,
    session_id: String,
    working_dir: Option<String>,
) {
    if transcript.len() < 200 {
        return;
    }

    crate::memory_log::log_final_extraction(&session_id, transcript.len());

    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(run_final_extraction(transcript, session_id, working_dir));
    } else {
        std::thread::spawn(move || {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => {
                    runtime.block_on(run_final_extraction(transcript, session_id, working_dir))
                }
                Err(err) => crate::logging::info(&format!(
                    "Final extraction runtime startup failed: {}",
                    err
                )),
            }
        });
    }
}

/// Check if the memory agent is currently processing (has been initialized)
pub fn is_active() -> bool {
    get().is_some()
}

/// Snapshot memory-agent runtime stats for UI/debug.
pub fn stats() -> MemoryAgentStats {
    MEMORY_AGENT_STATS
        .lock()
        .map(|s| s.clone())
        .unwrap_or_default()
}

// Re-export constants for use in memory.rs

#[cfg(test)]
#[path = "memory_agent_tests.rs"]
mod tests;
