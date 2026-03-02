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
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;
use tokio::sync::mpsc;

use crate::embedding;
use crate::memory::{self, MemoryEntry, MemoryManager};
use crate::memory_graph::{ClusterEntry, EdgeKind, MemoryGraph};
use crate::sidecar::Sidecar;
use crate::tui::info_widget::{MemoryEventKind, MemoryState};

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

/// Handle to communicate with the memory agent
#[derive(Clone)]
pub struct MemoryAgentHandle {
    /// Send messages to the agent
    tx: mpsc::Sender<AgentMessage>,
}

impl MemoryAgentHandle {
    /// Send a context update to the memory agent (async)
    pub async fn update_context(&self, session_id: &str, messages: Vec<crate::message::Message>) {
        self.update_context_sync(session_id, messages);
    }

    pub fn update_context_sync(&self, session_id: &str, messages: Vec<crate::message::Message>) {
        let msg = AgentMessage::Context {
            session_id: session_id.to_string(),
            messages,
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
        messages: Vec<crate::message::Message>,
        timestamp: Instant,
    },
    Reset,
}

/// Minimum turns before we consider extracting on topic change
const MIN_TURNS_FOR_EXTRACTION: usize = 4;

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
    /// Last context embedding (for topic change detection)
    last_context_embedding: Option<Vec<f32>>,
    /// Last context string (for extraction when topic changes)
    last_context_string: Option<String>,
    /// IDs of memories already surfaced to this session (avoid repetition)
    surfaced_memories: HashSet<String>,
    /// Conversation turn count for this session
    turn_count: usize,
    /// Turn count since last extraction for this session
    turns_since_extraction: usize,
}

/// The persistent memory agent state
pub struct MemoryAgent {
    /// Channel to receive messages
    rx: mpsc::Receiver<AgentMessage>,

    /// Haiku sidecar for LLM decisions
    sidecar: Sidecar,

    /// Memory manager for storage
    memory_manager: MemoryManager,

    /// Per-session state keyed by session ID
    sessions: HashMap<String, SessionState>,
}

impl MemoryAgent {
    /// Create a new memory agent
    fn new(rx: mpsc::Receiver<AgentMessage>) -> Self {
        Self {
            rx,
            sidecar: Sidecar::new(),
            memory_manager: MemoryManager::new(),
            sessions: HashMap::new(),
        }
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
        self.sessions
            .entry(session_id.to_string())
            .or_insert_with(SessionState::default)
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
                    timestamp,
                } => {
                    {
                        let ss = self.session_state(&session_id);
                        ss.turn_count += 1;
                    }
                    bump_turn_stat();

                    {
                        let ss = self.session_state(&session_id);
                        if ss.turn_count % TURN_RESET_INTERVAL == 0 {
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
        messages: Vec<crate::message::Message>,
        _timestamp: Instant,
    ) -> Result<()> {
        let context = memory::format_context_for_relevance(&messages);
        if context.is_empty() {
            return Ok(());
        }

        self.session_state(session_id).turns_since_extraction += 1;

        memory::set_state(MemoryState::Embedding);
        memory::add_event(MemoryEventKind::EmbeddingStarted);

        // Step 1: Embed current context
        let start = Instant::now();
        let context_embedding = match embedding::embed(&context) {
            Ok(emb) => emb,
            Err(e) => {
                crate::logging::info(&format!("Embedding failed: {}", e));
                memory::set_state(MemoryState::Idle);
                return Ok(());
            }
        };

        // Check for topic change (comparing against this session's last embedding)
        {
            let ss = self.session_state(session_id);
            if let Some(ref last_emb) = ss.last_context_embedding {
                let similarity = embedding::cosine_similarity(&context_embedding, last_emb);
                if similarity < TOPIC_CHANGE_THRESHOLD {
                    crate::logging::info(&format!(
                        "[{}] Topic change detected (sim={:.2}), resetting session memory state",
                        session_id, similarity
                    ));

                    // Extract memories from the PREVIOUS topic before moving on
                    if ss.turns_since_extraction >= MIN_TURNS_FOR_EXTRACTION {
                        if let Some(prev_context) = ss.last_context_string.clone() {
                            crate::logging::info(&format!(
                                "[{}] Triggering incremental extraction ({} turns since last)",
                                session_id, ss.turns_since_extraction
                            ));
                            ss.turns_since_extraction = 0;
                            drop(ss);
                            self.extract_from_context(&prev_context).await;
                            let ss = self.session_state(session_id);
                            ss.surfaced_memories.clear();
                            memory::clear_injected_memories(session_id);
                        } else {
                            ss.surfaced_memories.clear();
                            memory::clear_injected_memories(session_id);
                        }
                    } else {
                        ss.surfaced_memories.clear();
                        memory::clear_injected_memories(session_id);
                    }
                }
            }
        }

        // Store current context for potential future extraction
        {
            let ss = self.session_state(session_id);
            ss.last_context_embedding = Some(context_embedding.clone());
            ss.last_context_string = Some(context.clone());
        }

        // Step 2: Find similar memories by embedding
        let candidates = self.memory_manager.find_similar_with_embedding(
            &context_embedding,
            memory::EMBEDDING_SIMILARITY_THRESHOLD,
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

        if new_candidates.is_empty() {
            memory::set_state(MemoryState::Idle);
            return Ok(());
        }

        // Step 3: Use Haiku to decide what's relevant and worth surfacing
        memory::set_state(MemoryState::SidecarChecking {
            count: new_candidates.len(),
        });
        memory::add_event(MemoryEventKind::SidecarStarted);

        let candidate_ids: Vec<String> = new_candidates.iter().map(|(e, _)| e.id.clone()).collect();

        let relevant = self.evaluate_candidates(&context, new_candidates).await?;

        let verified_ids: Vec<String> = relevant.iter().map(|e| e.id.clone()).collect();
        let rejected_ids: Vec<String> = candidate_ids
            .iter()
            .filter(|id| !verified_ids.contains(id))
            .cloned()
            .collect();

        let retrieval_ctx = RetrievalContext {
            verified_ids: verified_ids.clone(),
            rejected_ids,
            context_snippet: context[..context.len().min(200)].to_string(),
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

                memory::set_pending_memory_with_ids(session_id, prompt, count, ids);
                memory::set_state(MemoryState::FoundRelevant { count });
            } else {
                memory::set_state(MemoryState::Idle);
            }
        } else {
            memory::set_state(MemoryState::Idle);
        }

        // Step 5: Post-retrieval maintenance (runs in background)
        self.post_retrieval_maintenance(retrieval_ctx).await;

        Ok(())
    }

    /// Use Haiku to evaluate which candidates are actually relevant
    async fn evaluate_candidates(
        &self,
        context: &str,
        candidates: Vec<(MemoryEntry, f32)>,
    ) -> Result<Vec<MemoryEntry>> {
        let mut relevant = Vec::new();

        // Process in parallel
        let futures: Vec<_> = candidates
            .iter()
            .take(MAX_MEMORIES_PER_TURN)
            .map(|(entry, sim)| {
                let sidecar = self.sidecar.clone();
                let content = entry.content.clone();
                let ctx = context.to_string();
                let similarity = *sim;
                async move {
                    let start = Instant::now();
                    let result = sidecar.check_relevance(&content, &ctx).await;
                    (result, start.elapsed(), similarity)
                }
            })
            .collect();

        let results = futures::future::join_all(futures).await;

        for ((entry, _), (result, elapsed, sim)) in candidates.iter().zip(results) {
            match result {
                Ok((is_relevant, reason)) => {
                    memory::add_event(MemoryEventKind::SidecarComplete {
                        latency_ms: elapsed.as_millis() as u64,
                    });

                    if is_relevant {
                        crate::logging::info(&format!(
                            "Memory relevant (sim={:.2}): {} - {}",
                            sim,
                            &entry.content[..entry.content.len().min(40)],
                            reason
                        ));
                        memory::add_event(MemoryEventKind::SidecarRelevant {
                            memory_preview: entry.content[..entry.content.len().min(30)]
                                .to_string(),
                        });
                        relevant.push(entry.clone());
                    } else {
                        memory::add_event(MemoryEventKind::SidecarNotRelevant);
                    }
                }
                Err(e) => {
                    memory::add_event(MemoryEventKind::Error {
                        message: e.to_string(),
                    });
                }
            }

            if relevant.len() >= MAX_MEMORIES_PER_TURN {
                break;
            }
        }

        Ok(relevant)
    }

    /// Search past sessions for more context (tool for memory agent)
    #[allow(dead_code)]
    async fn search_sessions(&self, query: &str) -> Result<Vec<SessionSearchResult>> {
        // This will use the session_search tool
        // For now, return empty - will implement with tool integration
        crate::logging::info(&format!("Memory agent searching sessions: {}", query));
        Ok(Vec::new())
    }

    /// Read the source that caused an embedding hit (tool for memory agent)
    #[allow(dead_code)]
    async fn read_source(&self, memory_id: &str) -> Result<Option<SourceContext>> {
        // Get the memory entry
        let all = self.memory_manager.list_all()?;
        let entry = all.iter().find(|e| e.id == memory_id);

        if let Some(entry) = entry {
            // Return the source session/context if available
            Ok(Some(SourceContext {
                memory_id: memory_id.to_string(),
                content: entry.content.clone(),
                source_session: entry.source.clone(),
                category: entry.category.to_string(),
            }))
        } else {
            Ok(None)
        }
    }

    /// Extract memories from a context string (called on topic change)
    ///
    /// This is an incremental extraction - we extract from a portion of the
    /// conversation when the topic changes, rather than waiting for session end.
    async fn extract_from_context(&self, context: &str) {
        // Don't extract from very short contexts
        if context.len() < 200 {
            return;
        }

        // Update UI state
        memory::set_state(MemoryState::Extracting {
            reason: "topic change".to_string(),
        });
        memory::add_event(MemoryEventKind::ExtractionStarted {
            reason: "topic change".to_string(),
        });

        let sidecar = self.sidecar.clone();
        let memory_manager = self.memory_manager.clone();
        let context_owned = context.to_string();

        // Similarity threshold for duplicate detection
        const DUPLICATE_THRESHOLD: f32 = 0.90;

        // Run extraction in background - don't block the main flow
        tokio::spawn(async move {
            match sidecar.extract_memories(&context_owned).await {
                Ok(extracted) if !extracted.is_empty() => {
                    let mut stored_count = 0;
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

                        if let Ok(matches) = similar {
                            if let Some((existing, _sim)) = matches.first() {
                                // Duplicate found - reinforce existing memory instead
                                let existing_id = existing.id.clone();
                                let mut did_reinforce = false;

                                if let Ok(mut graph) = memory_manager.load_project_graph() {
                                    if graph.get_memory(&existing_id).is_some() {
                                        let strength = {
                                            let entry = graph.get_memory_mut(&existing_id).unwrap();
                                            entry.reinforce("incremental", 0);
                                            entry.strength
                                        };
                                        if memory_manager.save_project_graph(&graph).is_ok() {
                                            did_reinforce = true;
                                            crate::logging::info(&format!(
                                                "Reinforced existing memory {} (strength={})",
                                                existing_id, strength
                                            ));
                                        }
                                    }
                                }

                                if !did_reinforce {
                                    if let Ok(mut graph) = memory_manager.load_global_graph() {
                                        if graph.get_memory(&existing_id).is_some() {
                                            graph
                                                .get_memory_mut(&existing_id)
                                                .unwrap()
                                                .reinforce("incremental", 0);
                                            let _ = memory_manager.save_global_graph(&graph);
                                            did_reinforce = true;
                                        }
                                    }
                                }

                                if did_reinforce {
                                    reinforced_count += 1;
                                }
                                continue;
                            }
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

                                // If contradiction found, supersede the old memory
                                if let Some(old_id) = contradiction_found {
                                    if let Ok(mut graph) = memory_manager.load_project_graph() {
                                        if let Some(old_entry) = graph.get_memory_mut(&old_id) {
                                            old_entry.supersede(&new_id);
                                            if memory_manager.save_project_graph(&graph).is_ok() {
                                                superseded_count += 1;
                                                crate::logging::info(&format!(
                                                    "Superseded memory {} with {}",
                                                    old_id, new_id
                                                ));
                                            }
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                crate::logging::info(&format!("Failed to store memory: {}", e));
                            }
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
    async fn post_retrieval_maintenance(&self, ctx: RetrievalContext) {
        memory::set_state(MemoryState::Maintaining {
            phase: "graph upkeep".to_string(),
        });
        memory::add_event(MemoryEventKind::MaintenanceStarted {
            verified: ctx.verified_ids.len(),
            rejected: ctx.rejected_ids.len(),
        });
        memory::pipeline_update(|p| {
            use crate::tui::info_widget::StepStatus;
            p.maintain = StepStatus::Running;
        });

        // Run maintenance in background - don't block retrieval flow
        let memory_manager = self.memory_manager.clone();

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

            // 2. Boost confidence for verified memories (they were actually useful)
            let mut boosted = 0usize;
            for id in &ctx.verified_ids {
                match boost_memory_confidence(&memory_manager, id, 0.05) {
                    Ok(()) => boosted += 1,
                    Err(e) => {
                        crate::logging::info(&format!("Confidence boost failed for {}: {}", id, e))
                    }
                }
            }

            // 3. Gentle decay for rejected memories (may be stale)
            let mut decayed = 0usize;
            for id in &ctx.rejected_ids {
                match decay_memory_confidence(&memory_manager, id, 0.02) {
                    Ok(()) => decayed += 1,
                    Err(e) => {
                        crate::logging::info(&format!("Confidence decay failed for {}: {}", id, e))
                    }
                }
            }
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
                    &ctx.context_snippet[..ctx.context_snippet.len().min(100)]
                ));
            }

            // 5. Periodic cluster refinement
            let tick = MAINTENANCE_TICK.fetch_add(1, Ordering::Relaxed) + 1;
            if tick % CLUSTER_REFINEMENT_INTERVAL == 0 && ctx.verified_ids.len() >= 2 {
                match refine_clusters(&memory_manager, &ctx.verified_ids) {
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

            let latency_ms = started.elapsed().as_millis() as u64;
            record_maintenance_stat(latency_ms);
            memory::add_event(MemoryEventKind::MaintenanceComplete { latency_ms });
            memory::pipeline_update(|p| {
                use crate::tui::info_widget::{StepResult, StepStatus};
                p.maintain = StepStatus::Done;
                p.maintain_result = Some(StepResult {
                    summary: format!("{}L {}↑ {}↓", links, boosted, decayed),
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
}

fn refine_clusters(
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
    }
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

/// Boost a memory's confidence score
fn boost_memory_confidence(manager: &MemoryManager, memory_id: &str, amount: f32) -> Result<()> {
    // Load project graph first
    let mut graph = manager.load_project_graph()?;
    if graph.get_memory(memory_id).is_some() {
        if let Some(entry) = graph.get_memory_mut(memory_id) {
            entry.boost_confidence(amount);
            let conf = entry.confidence;
            manager.save_project_graph(&graph)?;
            crate::logging::info(&format!(
                "Boosted confidence for {} to {:.2}",
                memory_id, conf
            ));
        }
        return Ok(());
    }

    // Try global
    let mut graph = manager.load_global_graph()?;
    if graph.get_memory(memory_id).is_some() {
        if let Some(entry) = graph.get_memory_mut(memory_id) {
            entry.boost_confidence(amount);
            let conf = entry.confidence;
            manager.save_global_graph(&graph)?;
            crate::logging::info(&format!(
                "Boosted confidence for {} to {:.2}",
                memory_id, conf
            ));
        }
        return Ok(());
    }

    Err(anyhow::anyhow!("Memory not found: {}", memory_id))
}

/// Decay a memory's confidence score
fn decay_memory_confidence(manager: &MemoryManager, memory_id: &str, amount: f32) -> Result<()> {
    // Load project graph first
    let mut graph = manager.load_project_graph()?;
    if graph.get_memory(memory_id).is_some() {
        if let Some(entry) = graph.get_memory_mut(memory_id) {
            entry.decay_confidence(amount);
            let conf = entry.confidence;
            manager.save_project_graph(&graph)?;
            crate::logging::info(&format!(
                "Decayed confidence for {} to {:.2}",
                memory_id, conf
            ));
        }
        return Ok(());
    }

    // Try global
    let mut graph = manager.load_global_graph()?;
    if graph.get_memory(memory_id).is_some() {
        if let Some(entry) = graph.get_memory_mut(memory_id) {
            entry.decay_confidence(amount);
            let conf = entry.confidence;
            manager.save_global_graph(&graph)?;
            crate::logging::info(&format!(
                "Decayed confidence for {} to {:.2}",
                memory_id, conf
            ));
        }
        return Ok(());
    }

    Err(anyhow::anyhow!("Memory not found: {}", memory_id))
}

/// Result from session search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSearchResult {
    pub session_id: String,
    pub snippet: String,
    pub relevance: f32,
}

/// Context about a memory's source
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceContext {
    pub memory_id: String,
    pub content: String,
    pub source_session: Option<String>,
    pub category: String,
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
pub async fn update_context(session_id: &str, messages: Vec<crate::message::Message>) {
    if let Some(handle) = get() {
        handle.update_context(session_id, messages).await;
    }
}

/// Send a context update synchronously (for use from non-async code)
/// This is non-blocking - it just sends to the channel
pub fn update_context_sync(session_id: &str, messages: Vec<crate::message::Message>) {
    if let Some(handle) = get() {
        handle.update_context_sync(session_id, messages);
    } else {
        let sid = session_id.to_string();
        tokio::spawn(async move {
            if let Ok(handle) = init().await {
                handle.update_context_sync(&sid, messages);
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
/// with a 5-second timeout, then logs the result. Does not block the caller.
pub fn trigger_final_extraction(transcript: String, session_id: String) {
    if transcript.len() < 200 {
        return;
    }

    tokio::spawn(async move {
        crate::logging::info(&format!(
            "Final extraction starting for session {} ({} chars)",
            session_id,
            transcript.len()
        ));

        let sidecar = crate::sidecar::Sidecar::new();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            sidecar.extract_memories(&transcript),
        )
        .await;

        match result {
            Ok(Ok(extracted)) if !extracted.is_empty() => {
                let manager = crate::memory::MemoryManager::new();
                let mut stored_count = 0;

                for mem in &extracted {
                    let category = match mem.category.as_str() {
                        "fact" => crate::memory::MemoryCategory::Fact,
                        "preference" => crate::memory::MemoryCategory::Preference,
                        "correction" => crate::memory::MemoryCategory::Correction,
                        _ => crate::memory::MemoryCategory::Fact,
                    };

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
            Ok(Ok(_)) => {
                crate::logging::info(&format!(
                    "Final extraction for session {}: no memories extracted",
                    session_id
                ));
            }
            Ok(Err(e)) => {
                crate::logging::info(&format!(
                    "Final extraction for session {} failed: {}",
                    session_id, e
                ));
            }
            Err(_) => {
                crate::logging::info(&format!(
                    "Final extraction for session {} timed out (5s)",
                    session_id
                ));
            }
        }
    });
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
pub use crate::memory::{EMBEDDING_MAX_HITS, EMBEDDING_SIMILARITY_THRESHOLD};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemoryCategory;

    #[test]
    fn infer_candidate_tag_uses_repeated_non_stopword() {
        let tag = infer_candidate_tag(
            "scheduler retries failed jobs and scheduler metrics update dashboard",
        );
        assert_eq!(tag.as_deref(), Some("scheduler"));
    }

    #[test]
    fn apply_cluster_assignment_links_members() {
        let mut graph = MemoryGraph::new();
        let mut a = MemoryEntry::new(MemoryCategory::Fact, "A");
        a.embedding = Some(vec![1.0, 0.0]);
        let id_a = graph.add_memory(a);

        let mut b = MemoryEntry::new(MemoryCategory::Fact, "B");
        b.embedding = Some(vec![0.0, 1.0]);
        let id_b = graph.add_memory(b);

        let stats = apply_cluster_assignment(
            &mut graph,
            "project",
            &[id_a.clone(), id_b.clone()],
            Utc::now(),
        );

        assert_eq!(stats.clusters_touched, 1);
        assert_eq!(stats.member_links, 2);
        assert_eq!(graph.clusters.len(), 1);

        let cluster_id = graph
            .clusters
            .keys()
            .next()
            .expect("cluster id")
            .to_string();
        assert!(graph
            .get_edges(&id_a)
            .iter()
            .any(|e| e.target == cluster_id && matches!(e.kind, EdgeKind::InCluster)));
        assert!(graph
            .get_edges(&id_b)
            .iter()
            .any(|e| e.target == cluster_id && matches!(e.kind, EdgeKind::InCluster)));
    }
}
