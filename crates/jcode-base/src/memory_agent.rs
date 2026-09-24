//! Nonblocking Jev recall coordinator.
//!
//! Recall sends scoped local memories directly to Jev's typed Decisions API.
//! It never loads an embedder or calls a text-generating sidecar. Optional
//! periodic/final extraction is independent and can be disabled separately.
use crate::memory::{self, MemoryManager};
use crate::memory_types::{MemoryEventKind, MemoryState};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const CONTEXT_CHANNEL_CAPACITY: usize = 16;
const PERIODIC_EXTRACTION_INTERVAL: usize = 12;
const REPEAT_SUPPRESSION: Duration = Duration::from_secs(30);
const FAILURE_BACKOFF: Duration = Duration::from_secs(60);
static MEMORY_AGENT: tokio::sync::OnceCell<MemoryAgentHandle> = tokio::sync::OnceCell::const_new();

/// Retain the public diagnostics shape for older clients. Jev does not need
/// embedding-cluster maintenance; those counters remain zero.
#[derive(Debug, Clone, Default)]
pub struct MemoryAgentStats {
    pub turns_processed: usize,
    pub maintenance_runs: usize,
    pub last_maintenance_ms: Option<u64>,
}
static MEMORY_AGENT_STATS: Mutex<MemoryAgentStats> = Mutex::new(MemoryAgentStats {
    turns_processed: 0,
    maintenance_runs: 0,
    last_maintenance_ms: None,
});

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
    // Extraction is optional. Its model/credentials never gate recall.
    if !memory::memory_llm_judge_available() {
        return;
    }
    let manager = manager_for_working_dir(working_dir.as_deref());
    match manager
        .extract_from_transcript(&transcript, &session_id)
        .await
    {
        Ok(ids) => {
            memory::mark_memories_known(&session_id, &ids, "extracted from this session");
            memory::add_event(MemoryEventKind::ExtractionComplete { count: ids.len() });
        }
        Err(_) => crate::logging::info("Optional memory extraction failed"),
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

#[derive(Default)]
struct SessionState {
    working_dir: Option<String>,
    last_query: Option<String>,
    last_check: Option<Instant>,
    failed_at: Option<Instant>,
    turns: usize,
}

pub struct MemoryAgent {
    rx: mpsc::Receiver<AgentMessage>,
    sessions: HashMap<String, SessionState>,
}

impl MemoryAgent {
    fn new(rx: mpsc::Receiver<AgentMessage>) -> Self {
        Self {
            rx,
            sessions: HashMap::new(),
        }
    }

    fn reset(&mut self) {
        self.sessions.clear();
        memory::clear_all_pending_memory();
        memory::clear_all_injected_memories();
        if let Ok(mut stats) = MEMORY_AGENT_STATS.lock() {
            *stats = MemoryAgentStats::default();
        }
    }

    async fn run(mut self) {
        while let Some(message) = self.rx.recv().await {
            match message {
                AgentMessage::Reset => self.reset(),
                AgentMessage::Context {
                    session_id,
                    messages,
                    working_dir,
                    timestamp,
                } => {
                    // A bounded queue must not recall for a context that aged out
                    // while another session's remote provider was unavailable.
                    if timestamp.elapsed() > Duration::from_secs(120) {
                        continue;
                    }
                    let ss = self.sessions.entry(session_id.clone()).or_default();
                    // None is meaningful: never retain another project's scope.
                    if ss.working_dir != working_dir {
                        *ss = SessionState {
                            working_dir,
                            ..Default::default()
                        };
                        memory::clear_pending_memory(&session_id);
                    }
                    ss.turns += 1;
                    if let Ok(mut stats) = MEMORY_AGENT_STATS.lock() {
                        stats.turns_processed += 1;
                    }
                    if let Err(error) = self.process_context(&session_id, &messages).await {
                        memory::clear_pending_memory(&session_id);
                        self.sessions.entry(session_id).or_default().failed_at =
                            Some(Instant::now());
                        memory::add_event(MemoryEventKind::Error {
                            message: error.to_string(),
                        });
                        memory::set_state(MemoryState::Idle);
                    }
                }
            }
        }
    }

    async fn process_context(
        &mut self,
        session_id: &str,
        messages: &[crate::message::Message],
    ) -> Result<()> {
        let query = memory::format_focused_query_for_relevance(messages);
        if query.trim().is_empty() {
            return Ok(());
        }
        let ss = self.sessions.entry(session_id.to_string()).or_default();
        // Optional writing is fire-and-forget and never delays a recall request.
        if ss.turns > 0
            && ss.turns.is_multiple_of(PERIODIC_EXTRACTION_INTERVAL)
            && memory::memory_llm_judge_available()
        {
            trigger_final_extraction_with_dir(
                memory::format_context_for_extraction(messages),
                session_id.to_string(),
                ss.working_dir.clone(),
            );
        }
        if ss
            .failed_at
            .is_some_and(|at| at.elapsed() < FAILURE_BACKOFF)
        {
            return Ok(());
        }
        if ss.last_query.as_deref() == Some(query.as_str())
            && ss
                .last_check
                .is_some_and(|at| at.elapsed() < REPEAT_SUPPRESSION)
        {
            return Ok(());
        }
        ss.last_query = Some(query);
        ss.last_check = Some(Instant::now());
        let manager = manager_for_working_dir(ss.working_dir.as_deref());
        memory::clear_pending_memory(session_id);
        if !memory::memory_runtime_active() {
            return Ok(());
        }
        let memory::MemoryRelevanceResult {
            prompt,
            display_prompt,
            selected_entries,
        } = manager
            .get_relevant_parallel(session_id, messages, None)
            .await?;
        if let Some(prompt) = prompt {
            let count = selected_entries.len();
            memory::set_pending_memory_for_project_with_selection(
                session_id,
                prompt,
                count,
                &selected_entries,
                display_prompt,
                self.sessions
                    .get(session_id)
                    .and_then(|state| state.working_dir.as_deref()),
            );
        }
        self.sessions
            .entry(session_id.to_string())
            .or_default()
            .failed_at = None;
        Ok(())
    }
}

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
/// This clears per-session recall state and pending injections.
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

#[cfg(test)]
#[path = "memory_agent_tests.rs"]
mod tests;
