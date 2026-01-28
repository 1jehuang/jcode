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
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Instant;
use tokio::sync::mpsc;

use crate::embedding;
use crate::memory::{self, MemoryEntry, MemoryManager};
use crate::sidecar::HaikuSidecar;
use crate::tui::info_widget::{MemoryEventKind, MemoryState};

/// Context from a retrieval operation for post-retrieval maintenance
#[derive(Debug, Clone)]
struct RetrievalContext {
    /// Embedding of the query context
    embedding: Vec<f32>,
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

/// Global memory agent instance
static MEMORY_AGENT: tokio::sync::OnceCell<MemoryAgentHandle> = tokio::sync::OnceCell::const_new();

/// Handle to communicate with the memory agent
#[derive(Clone)]
pub struct MemoryAgentHandle {
    /// Send messages to the agent
    tx: mpsc::Sender<AgentMessage>,
}

impl MemoryAgentHandle {
    /// Send a context update to the memory agent (async)
    pub async fn update_context(&self, messages: Vec<crate::message::Message>) {
        self.update_context_sync(messages);
    }

    /// Send a context update to the memory agent (sync, non-blocking)
    pub fn update_context_sync(&self, messages: Vec<crate::message::Message>) {
        let msg = AgentMessage::Context {
            messages,
            timestamp: Instant::now(),
        };
        // Don't block if channel is full - memory is non-critical
        let _ = self.tx.try_send(msg);
    }

    /// Reset all memory agent state (call on new session)
    pub fn reset(&self) {
        let _ = self.tx.try_send(AgentMessage::Reset);
    }
}

/// Messages sent to the memory agent
enum AgentMessage {
    /// Context update with conversation messages
    Context {
        messages: Vec<crate::message::Message>,
        timestamp: Instant,
    },
    /// Reset all agent state (e.g., on new session)
    Reset,
}

/// The persistent memory agent state
pub struct MemoryAgent {
    /// Channel to receive messages
    rx: mpsc::Receiver<AgentMessage>,

    /// Haiku sidecar for LLM decisions
    sidecar: HaikuSidecar,

    /// Memory manager for storage
    memory_manager: MemoryManager,

    /// Last context embedding (for topic change detection)
    last_context_embedding: Option<Vec<f32>>,

    /// IDs of memories already surfaced this "session" (avoid repetition)
    surfaced_memories: HashSet<String>,

    /// Conversation turn count (for deciding when to reset)
    turn_count: usize,
}

impl MemoryAgent {
    /// Create a new memory agent
    fn new(rx: mpsc::Receiver<AgentMessage>) -> Self {
        Self {
            rx,
            sidecar: HaikuSidecar::new(),
            memory_manager: MemoryManager::new(),
            last_context_embedding: None,
            surfaced_memories: HashSet::new(),
            turn_count: 0,
        }
    }

    /// Reset all agent state
    fn reset(&mut self) {
        crate::logging::info("Memory agent reset: clearing all state");
        self.last_context_embedding = None;
        self.surfaced_memories.clear();
        self.turn_count = 0;
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
                    messages,
                    timestamp,
                } => {
                    self.turn_count += 1;

                    // Periodic reset to prevent unbounded state growth
                    if self.turn_count % TURN_RESET_INTERVAL == 0 {
                        crate::logging::info(&format!(
                            "Memory agent periodic reset at turn {} (clearing {} surfaced memories)",
                            self.turn_count,
                            self.surfaced_memories.len()
                        ));
                        self.surfaced_memories.clear();
                    }

                    if let Err(e) = self.process_context(messages, timestamp).await {
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
        messages: Vec<crate::message::Message>,
        _timestamp: Instant,
    ) -> Result<()> {
        // Format context for embedding
        let context = memory::format_context_for_relevance(&messages);
        if context.is_empty() {
            return Ok(());
        }

        // Update activity state
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

        // Check for topic change
        if let Some(ref last_emb) = self.last_context_embedding {
            let similarity = embedding::cosine_similarity(&context_embedding, last_emb);
            if similarity < TOPIC_CHANGE_THRESHOLD {
                // Topic changed significantly - reset surfaced memories
                crate::logging::info(&format!(
                    "Topic change detected (sim={:.2}), resetting memory agent state",
                    similarity
                ));
                self.surfaced_memories.clear();
            }
        }
        self.last_context_embedding = Some(context_embedding.clone());

        // Step 2: Find similar memories by embedding
        let candidates = self.memory_manager.find_similar(
            &context,
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

        // Filter out already-surfaced memories
        let new_candidates: Vec<_> = candidates
            .into_iter()
            .filter(|(entry, _)| !self.surfaced_memories.contains(&entry.id))
            .collect();

        if new_candidates.is_empty() {
            memory::set_state(MemoryState::Idle);
            return Ok(());
        }

        // Step 3: Use Haiku to decide what's relevant and worth surfacing
        memory::set_state(MemoryState::SidecarChecking {
            count: new_candidates.len(),
        });
        memory::add_event(MemoryEventKind::SidecarStarted);

        // Collect candidate IDs for tracking
        let candidate_ids: Vec<String> = new_candidates.iter().map(|(e, _)| e.id.clone()).collect();

        let relevant = self.evaluate_candidates(&context, new_candidates).await?;

        // Build retrieval context for maintenance
        let verified_ids: Vec<String> = relevant.iter().map(|e| e.id.clone()).collect();
        let rejected_ids: Vec<String> = candidate_ids
            .iter()
            .filter(|id| !verified_ids.contains(id))
            .cloned()
            .collect();

        let retrieval_ctx = RetrievalContext {
            embedding: context_embedding,
            verified_ids: verified_ids.clone(),
            rejected_ids,
            context_snippet: context[..context.len().min(200)].to_string(),
        };

        // Step 4: Format and store for main agent
        if !relevant.is_empty() {
            let mut prompt = String::from("# Relevant Memory\n\n");
            for entry in &relevant {
                prompt.push_str(&format!("- {}\n", entry.content));
                self.surfaced_memories.insert(entry.id.clone());
            }

            memory::set_pending_memory(prompt, relevant.len());
            memory::set_state(MemoryState::FoundRelevant {
                count: relevant.len(),
            });
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

    /// Post-retrieval maintenance tasks
    ///
    /// After serving memories, we can use the retrieval context to:
    /// 1. Create links between co-relevant memories
    /// 2. Boost confidence for verified memories
    /// 3. Decay confidence for rejected memories
    /// 4. Log memory gaps for future learning
    async fn post_retrieval_maintenance(&self, ctx: RetrievalContext) {
        // Run maintenance in background - don't block retrieval flow
        let memory_manager = self.memory_manager.clone();

        tokio::spawn(async move {
            // 1. Link discovery: Create RelatesTo edges between co-relevant memories
            if ctx.verified_ids.len() >= 2 {
                if let Err(e) = discover_links(&memory_manager, &ctx.verified_ids).await {
                    crate::logging::info(&format!("Link discovery failed: {}", e));
                }
            }

            // 2. Boost confidence for verified memories (they were actually useful)
            for id in &ctx.verified_ids {
                if let Err(e) = boost_memory_confidence(&memory_manager, id, 0.05) {
                    crate::logging::info(&format!("Confidence boost failed for {}: {}", id, e));
                }
            }

            // 3. Gentle decay for rejected memories (may be stale)
            for id in &ctx.rejected_ids {
                if let Err(e) = decay_memory_confidence(&memory_manager, id, 0.02) {
                    crate::logging::info(&format!("Confidence decay failed for {}: {}", id, e));
                }
            }

            // 4. Gap detection: Log when we had no relevant memories
            if ctx.verified_ids.is_empty() && !ctx.rejected_ids.is_empty() {
                crate::logging::info(&format!(
                    "Memory gap detected: {} candidates retrieved but none relevant. Context: {}...",
                    ctx.rejected_ids.len(),
                    &ctx.context_snippet[..ctx.context_snippet.len().min(100)]
                ));
            }
        });
    }
}

/// Discover links between co-relevant memories
async fn discover_links(manager: &MemoryManager, memory_ids: &[String]) -> Result<()> {
    // For each pair of co-relevant memories, create a RelatesTo link
    // Use a moderate weight since we're inferring the relationship
    const LINK_WEIGHT: f32 = 0.6;

    for i in 0..memory_ids.len() {
        for j in (i + 1)..memory_ids.len() {
            let from = &memory_ids[i];
            let to = &memory_ids[j];

            // Try to link (may fail if memories are in different stores)
            if let Err(e) = manager.link_memories(from, to, LINK_WEIGHT) {
                // This is expected for cross-store memories, just log at debug level
                crate::logging::info(&format!("Could not link {} -> {}: {}", from, to, e));
            }
        }
    }

    Ok(())
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
pub async fn update_context(messages: Vec<crate::message::Message>) {
    if let Some(handle) = get() {
        handle.update_context(messages).await;
    }
}

/// Send a context update synchronously (for use from non-async code)
/// This is non-blocking - it just sends to the channel
pub fn update_context_sync(messages: Vec<crate::message::Message>) {
    if let Some(handle) = get() {
        handle.update_context_sync(messages);
    } else {
        // Agent not initialized yet - spawn initialization and send
        tokio::spawn(async move {
            if let Ok(handle) = init().await {
                handle.update_context_sync(messages);
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

/// Check if the memory agent is currently processing (has been initialized)
pub fn is_active() -> bool {
    get().is_some()
}

// Re-export constants for use in memory.rs
pub use crate::memory::{EMBEDDING_MAX_HITS, EMBEDDING_SIMILARITY_THRESHOLD};
