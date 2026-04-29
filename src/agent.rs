#![cfg_attr(test, allow(clippy::await_holding_lock))]

mod compaction;
mod environment;
mod interrupts;
mod messages;
mod prompting;
mod provider;
mod response_recovery;
mod status;
mod streaming;
mod tools;
mod turn_execution;
mod turn_loops;
mod turn_streaming_broadcast;
mod turn_streaming_mpsc;
mod utils;

use self::streaming::{
    send_stream_keepalive_broadcast, send_stream_keepalive_mpsc, stream_keepalive_ticker,
};
use self::tools::{print_tool_summary, tool_output_to_content_blocks};
use self::utils::trace_enabled;
use crate::build;
use crate::bus::{Bus, BusEvent, SubagentStatus, ToolEvent, ToolStatus};
use crate::cache_tracker::CacheTracker;
use crate::compaction::CompactionEvent;
use crate::id;
use crate::logging;
use crate::message::{
    ContentBlock, Message, Role, StreamEvent, TOOL_OUTPUT_MISSING_TEXT, ToolCall, ToolDefinition,
};
use crate::protocol::{HistoryMessage, ServerEvent};
use crate::provider::{NativeToolResult, Provider};
use crate::session::{GitState, Session, SessionStatus, StoredDisplayRole, StoredMessage};
use crate::skill::SkillRegistry;
use crate::tool::{Registry, ToolContext, ToolExecutionMode};
use anyhow::Result;
use futures::StreamExt;
use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc};

use interrupts::{NoToolCallOutcome, PostToolInterruptOutcome};
pub use jcode_agent_runtime::{
    BackgroundToolSignal, GracefulShutdownSignal, InterruptSignal, SoftInterruptMessage,
    SoftInterruptQueue, SoftInterruptSource, StreamError,
};

const JCODE_NATIVE_TOOLS: &[&str] = &["selfdev", "communicate"];
static RECOVERED_TEXT_WRAPPED_TOOL_CALLS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
static JCODE_REPO_SOURCE_STATE: LazyLock<(Option<String>, Option<bool>)> = LazyLock::new(|| {
    crate::build::get_repo_dir()
        .map(|repo_dir| {
            (
                build::current_git_hash(&repo_dir).ok(),
                build::is_working_tree_dirty(&repo_dir).ok(),
            )
        })
        .unwrap_or((None, None))
});
static WORKING_GIT_STATE_CACHE: LazyLock<StdMutex<HashMap<PathBuf, Option<GitState>>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));
const STREAM_KEEPALIVE_PONG_ID: u64 = 0;

/// Token usage from the last API request
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
}

pub struct Agent {
    provider: Arc<dyn Provider>,
    registry: Registry,
    skills: Arc<SkillRegistry>,
    session: Session,
    active_skill: Option<String>,
    allowed_tools: Option<HashSet<String>>,
    /// Provider-specific session ID for conversation resume (e.g., Claude Code CLI session)
    provider_session_id: Option<String>,
    /// Last upstream provider (OpenRouter) observed for this session
    last_upstream_provider: Option<String>,
    /// Last observed transport/connection type for this session
    last_connection_type: Option<String>,
    /// Last provider-supplied human-readable transport detail for this session
    last_status_detail: Option<String>,
    /// Pending swarm alerts to inject into the next turn
    pending_alerts: Vec<String>,
    /// Transient reminder injected into provider requests for the current turn only.
    /// Not persisted to session history.
    current_turn_system_reminder: Option<String>,
    /// Tool call ids observed in the current session transcript.
    tool_call_ids: HashSet<String>,
    /// Tool result ids observed in the current session transcript.
    tool_result_ids: HashSet<String>,
    /// Number of stored session messages already indexed for missing tool-output repair.
    tool_output_scan_index: usize,
    /// Soft interrupt queue: messages to inject at next safe point without cancelling
    /// Uses std::sync::Mutex so it can be accessed without async, even while agent is processing
    soft_interrupt_queue: SoftInterruptQueue,
    /// Signal from client to move the currently executing tool to background
    background_tool_signal: InterruptSignal,
    /// Signal to gracefully stop generation (checkpoint partial response and exit)
    graceful_shutdown: InterruptSignal,
    /// Client-side cache tracking for detecting append-only violations
    cache_tracker: CacheTracker,
    /// Last token usage from API request (for debug socket queries)
    last_usage: TokenUsage,
    /// Locked tool list: once the first API request is sent, freeze the tool list
    /// to avoid cache invalidation when MCP tools arrive asynchronously.
    /// Cleared on compaction/reset.
    locked_tools: Option<Vec<ToolDefinition>>,
    /// Override system prompt (used by ambient mode to inject a custom prompt)
    system_prompt_override: Option<String>,
    /// Whether memory features are enabled for this session
    memory_enabled: bool,
    /// Channel for tools to request stdin input from the user
    stdin_request_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::tool::StdinInputRequest>>,
}

/// Try to apply a session's persisted model selection to the provider, lazily
/// hot-initialising any sub-providers from on-disk credentials if the first
/// attempt fails because the provider believes credentials are unavailable.
///
/// This guards the session-restore path against a race where the running
/// `MultiProvider` instance was constructed before the user finished an OAuth
/// flow (e.g. across a `restart-snapshot` reload). Without this, restore would
/// silently log an error and prompt the user to log in again even though
/// `~/.jcode/auth.json` already holds valid credentials.
pub(crate) fn set_session_model_with_lazy_auth_init(
    provider: &dyn Provider,
    model: &str,
) -> anyhow::Result<()> {
    match provider.set_model(model) {
        Ok(()) => Ok(()),
        Err(initial_err) => {
            // Best-effort: ask the provider to re-scan on-disk credentials and
            // retry once. `on_auth_changed` is idempotent and cheap (a few file
            // reads), and is the same hook the login flow uses.
            provider.on_auth_changed();
            match provider.set_model(model) {
                Ok(()) => {
                    logging::info(&format!(
                        "Recovered session model '{}' after lazy auth re-init",
                        model
                    ));
                    Ok(())
                }
                Err(_) => Err(initial_err),
            }
        }
    }
}

impl Agent {
    fn should_track_client_cache(&self) -> bool {
        match std::env::var("JCODE_TRACK_CLIENT_CACHE") {
            Ok(value) => {
                let value = value.trim();
                !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
            }
            Err(_) => false,
        }
    }

    fn build_base(
        provider: Arc<dyn Provider>,
        registry: Registry,
        session: Session,
        allowed_tools: Option<HashSet<String>>,
    ) -> Self {
        let skills = SkillRegistry::shared_snapshot();
        Self {
            provider,
            registry,
            skills,
            session,
            active_skill: None,
            allowed_tools,
            provider_session_id: None,
            last_upstream_provider: None,
            last_connection_type: None,
            last_status_detail: None,
            pending_alerts: Vec::new(),
            current_turn_system_reminder: None,
            tool_call_ids: HashSet::new(),
            tool_result_ids: HashSet::new(),
            tool_output_scan_index: 0,
            soft_interrupt_queue: Arc::new(std::sync::Mutex::new(Vec::new())),
            background_tool_signal: InterruptSignal::new(),
            graceful_shutdown: InterruptSignal::new(),
            cache_tracker: CacheTracker::new(),
            last_usage: TokenUsage::default(),
            locked_tools: None,
            system_prompt_override: None,
            memory_enabled: crate::config::config().features.memory,
            stdin_request_tx: None,
        }
    }

    fn current_skills_snapshot(&self) -> Arc<SkillRegistry> {
        self.registry
            .skills()
            .try_read()
            .map(|skills| Arc::new(skills.clone()))
            .unwrap_or_else(|_| self.skills.clone())
    }

    pub fn available_skill_names(&self) -> Vec<String> {
        self.current_skills_snapshot()
            .list()
            .iter()
            .map(|skill| skill.name.clone())
            .collect()
    }

    pub fn new(provider: Arc<dyn Provider>, registry: Registry) -> Self {
        let mut agent = Self::build_base(provider, registry, Session::create(None, None), None);
        agent.session.mark_active();
        agent.session.model = Some(agent.provider.model());
        agent.session.provider_key =
            crate::session::derive_session_provider_key(agent.provider.name());
        agent.seed_compaction_from_session();
        agent.log_env_snapshot("create");
        crate::telemetry::begin_session(agent.provider.name(), &agent.provider.model());
        agent
    }

    pub fn new_with_session(
        provider: Arc<dyn Provider>,
        registry: Registry,
        session: Session,
        allowed_tools: Option<HashSet<String>>,
    ) -> Self {
        let mut agent = Self::build_base(provider, registry, session, allowed_tools);
        agent.session.mark_active();
        if agent.session.provider_key.is_none() {
            agent.session.provider_key =
                crate::session::derive_session_provider_key(agent.provider.name());
        }
        if let Some(model) = agent.session.model.clone() {
            if let Err(e) = set_session_model_with_lazy_auth_init(&*agent.provider, &model) {
                logging::error(&format!(
                    "Failed to restore session model '{}': {}",
                    model, e
                ));
            }
        } else {
            agent.session.model = Some(agent.provider.model());
        }
        agent.sync_memory_dedup_state_from_session();
        agent.seed_compaction_from_session();
        agent.log_env_snapshot("attach");
        crate::telemetry::begin_session(agent.provider.name(), &agent.provider.model());
        agent
    }

    fn seed_compaction_from_session(&mut self) {
        logging::info(&format!(
            "seed_compaction_from_session: session has {} messages",
            self.session.messages.len()
        ));
        let compaction = self.registry.compaction();
        let mut manager = match compaction.try_write() {
            Ok(manager) => manager,
            Err(_) => {
                logging::warn(
                    "seed_compaction_from_session: compaction lock unavailable, skipping restore",
                );
                return;
            }
        };
        manager.reset();
        let budget = self.provider.context_window();
        manager.set_budget(budget);
        if let Some(state) = self.session.compaction.as_ref() {
            manager.restore_persisted_stored_state_with(state, &self.session.messages);
        } else {
            manager.seed_restored_stored_messages_with(&self.session.messages);
        }
        logging::info(&format!(
            "seed_compaction_from_session: seeded compaction with {} messages",
            self.session.messages.len()
        ));
    }

    fn sync_memory_dedup_state_from_session(&self) {
        crate::memory::sync_injected_memories(
            &self.session.id,
            &self.session.injected_memory_ids(),
        );
    }

    fn record_memory_injection_in_session(&mut self, memory: &crate::memory::PendingMemory) {
        let count = memory.count.max(1);
        let age_ms = memory.computed_at.elapsed().as_millis() as u64;
        let summary = if count == 1 {
            "🧠 auto-recalled 1 memory".to_string()
        } else {
            format!("🧠 auto-recalled {} memories", count)
        };
        let display_prompt = memory.display_prompt.clone().unwrap_or_else(|| {
            if memory.prompt.trim().is_empty() {
                "# Memory\n\n## Notes\n1. (empty injection payload)".to_string()
            } else {
                memory.prompt.clone()
            }
        });

        self.session.record_memory_injection(
            summary,
            display_prompt,
            count as u32,
            age_ms,
            memory.memory_ids.clone(),
        );
        if let Err(err) = self.session.save() {
            logging::warn(&format!(
                "Failed to persist memory injection for session {}: {}",
                self.session.id, err
            ));
        }
    }

    fn persist_session_best_effort(&mut self, context: &str) {
        if let Err(err) = self.session.save() {
            logging::warn(&format!(
                "Failed to persist {} for session {}: {}",
                context, self.session.id, err
            ));
        }
    }

    fn reset_runtime_state_for_session_change(&mut self) {
        self.active_skill = None;
        self.last_upstream_provider = None;
        self.last_connection_type = None;
        self.last_status_detail = None;
        self.pending_alerts.clear();
        self.current_turn_system_reminder = None;
        self.reset_tool_output_tracking();
        if let Ok(mut queue) = self.soft_interrupt_queue.lock() {
            queue.clear();
        }
        self.background_tool_signal.reset();
        self.graceful_shutdown.reset();
        self.cache_tracker.reset();
        self.last_usage = TokenUsage::default();
        self.locked_tools = None;
    }

    fn sync_session_compaction_state_from_manager(
        &mut self,
        manager: &crate::compaction::CompactionManager,
    ) {
        let new_state = manager.persisted_state();
        if self.session.compaction != new_state {
            self.session.compaction = new_state;
            if let Err(err) = self.session.save() {
                logging::error(&format!(
                    "Failed to persist compaction state for session {}: {}",
                    self.session.id, err
                ));
            }
        }
    }

    fn apply_openai_native_compaction(
        &mut self,
        encrypted_content: String,
        compacted_count: usize,
    ) -> Result<()> {
        let state = crate::session::StoredCompactionState {
            summary_text: String::new(),
            openai_encrypted_content: Some(encrypted_content),
            covers_up_to_turn: compacted_count,
            original_turn_count: compacted_count,
            compacted_count,
        };

        self.session.compaction = Some(state.clone());
        let compaction = self.registry.compaction();
        if let Ok(mut manager) = compaction.try_write() {
            manager.set_budget(self.provider.context_window());
            manager.restore_persisted_stored_state_with(&state, &self.session.messages);
        }

        self.cache_tracker.reset();
        self.locked_tools = None;
        self.provider_session_id = None;
        self.session.provider_session_id = None;
        self.session.save()?;
        crate::runtime_memory_log::emit_event(
            crate::runtime_memory_log::RuntimeMemoryLogEvent::new(
                "native_compaction_applied",
                "provider_native_compaction_persisted",
            )
            .with_session_id(self.session.id.clone())
            .force_attribution(),
        );
        Ok(())
    }

    fn messages_for_provider(&mut self) -> (Vec<Message>, Option<CompactionEvent>) {
        if self.provider.uses_jcode_compaction() || self.session.compaction.is_some() {
            let compaction = self.registry.compaction();
            match compaction.try_write() {
                Ok(mut manager) => {
                    let messages = {
                        let all_messages = self.session.provider_messages();
                        if self.provider.uses_jcode_compaction() {
                            let action =
                                manager.ensure_context_fits(all_messages, self.provider.clone());
                            match action {
                                crate::compaction::CompactionAction::BackgroundStarted {
                                    trigger,
                                } => {
                                    logging::info(&format!(
                                        "Background compaction started ({})",
                                        trigger
                                    ));
                                }
                                crate::compaction::CompactionAction::HardCompacted(dropped) => {
                                    logging::warn(&format!(
                                        "Emergency hard compact: dropped {} messages (context was critical)",
                                        dropped
                                    ));
                                }
                                crate::compaction::CompactionAction::None => {}
                            }
                        }
                        manager.messages_for_api_with(all_messages)
                    };
                    let event = if self.provider.uses_jcode_compaction() {
                        manager.take_compaction_event()
                    } else {
                        None
                    };
                    if event.is_some() {
                        self.sync_session_compaction_state_from_manager(&manager);
                    }
                    let user_count = messages
                        .iter()
                        .filter(|message| matches!(message.role, Role::User))
                        .count();
                    let assistant_count = messages.len().saturating_sub(user_count);
                    logging::info(&format!(
                        "messages_for_provider (compaction): returning {} messages (user={}, assistant={})",
                        messages.len(),
                        user_count,
                        assistant_count,
                    ));
                    return (messages, event);
                }
                Err(_) => {
                    logging::info("messages_for_provider: compaction lock failed, using session");
                }
            };
        }

        let all_messages = self.session.provider_messages();
        let messages = all_messages.to_vec();
        let user_count = messages
            .iter()
            .filter(|message| matches!(message.role, Role::User))
            .count();
        let assistant_count = messages.len().saturating_sub(user_count);
        logging::info(&format!(
            "messages_for_provider (session): returning {} messages (user={}, assistant={})",
            messages.len(),
            user_count,
            assistant_count,
        ));
        (messages, None)
    }

    fn record_client_cache_request(&mut self, messages: &[Message]) {
        if !self.should_track_client_cache() {
            return;
        }

        let fast_snapshot =
            if !self.provider.uses_jcode_compaction() && self.session.compaction.is_none() {
                let previous_count = self.cache_tracker.previous_message_count();
                let prefix_hashes = self.session.provider_message_prefix_hashes();
                let current_count = prefix_hashes.len();
                let current_full_hash = prefix_hashes.last().copied();
                let prefix_hash_at_previous_count =
                    if previous_count == 0 || previous_count > current_count {
                        None
                    } else {
                        Some(prefix_hashes[previous_count - 1])
                    };
                Some((
                    current_count,
                    prefix_hash_at_previous_count,
                    current_full_hash,
                ))
            } else {
                None
            };

        let violation =
            if let Some((current_count, prefix_hash_at_previous_count, current_full_hash)) =
                fast_snapshot
            {
                self.cache_tracker.record_prefix_hash_snapshot(
                    current_count,
                    prefix_hash_at_previous_count,
                    current_full_hash,
                )
            } else {
                self.cache_tracker.record_request(messages)
            };

        if let Some(violation) = violation {
            logging::warn(&format!(
                "CLIENT_CACHE_VIOLATION: {} | turn={} messages={}",
                violation.reason, violation.turn, violation.message_count
            ));
        }
    }

    fn repair_missing_tool_outputs(&mut self) -> usize {
        if self.tool_output_scan_index > self.session.messages.len() {
            self.reset_tool_output_tracking();
        }

        let scan_start = self.tool_output_scan_index;
        let mut new_result_ids = Vec::new();
        let mut assistant_tool_uses: Vec<(usize, Vec<String>)> = Vec::new();

        for (index, msg) in self.session.messages.iter().enumerate().skip(scan_start) {
            match msg.role {
                Role::User => {
                    for block in &msg.content {
                        if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                            new_result_ids.push(tool_use_id.clone());
                        }
                    }
                }
                Role::Assistant => {
                    let tool_uses = msg
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            ContentBlock::ToolUse { id, .. } => Some(id.clone()),
                            _ => None,
                        })
                        .collect::<Vec<_>>();
                    if !tool_uses.is_empty() {
                        assistant_tool_uses.push((index, tool_uses));
                    }
                }
            }
        }

        self.tool_result_ids.extend(new_result_ids);

        let mut missing_repairs: Vec<(usize, Vec<String>)> = Vec::new();
        for (index, tool_uses) in assistant_tool_uses {
            let mut missing_for_message = Vec::new();
            for id in tool_uses {
                self.tool_call_ids.insert(id.clone());
                if !self.tool_result_ids.contains(&id) {
                    missing_for_message.push(id);
                }
            }
            if !missing_for_message.is_empty() {
                missing_repairs.push((index, missing_for_message));
            }
        }

        self.tool_output_scan_index = self.session.messages.len();

        let mut repaired = 0usize;
        let mut inserted = 0usize;
        for (index, missing_for_message) in missing_repairs {
            for (offset, id) in missing_for_message.iter().enumerate() {
                let tool_block = ContentBlock::ToolResult {
                    tool_use_id: id.clone(),
                    content: TOOL_OUTPUT_MISSING_TEXT.to_string(),
                    is_error: Some(true),
                };
                let stored_message = StoredMessage {
                    id: id::new_id("message"),
                    role: Role::User,
                    content: vec![tool_block],
                    display_role: None,
                    timestamp: Some(chrono::Utc::now()),
                    tool_duration_ms: None,
                    token_usage: None,
                };
                self.session
                    .insert_message(index + 1 + inserted + offset, stored_message);
                self.tool_result_ids.insert(id.clone());
                repaired += 1;
            }
            inserted += missing_for_message.len();
        }

        self.tool_output_scan_index = self.session.messages.len();

        if repaired > 0 {
            self.persist_session_best_effort("missing tool-output repair");
            self.cache_tracker.reset();
            self.locked_tools = None;
        }

        repaired
    }

    fn reset_tool_output_tracking(&mut self) {
        self.tool_call_ids.clear();
        self.tool_result_ids.clear();
        self.tool_output_scan_index = 0;
    }

    pub fn session_id(&self) -> &str {
        &self.session.id
    }

    /// Mark this agent session as closed and persist it.
    pub fn mark_closed(&mut self) {
        crate::telemetry::end_session_with_reason(
            self.provider.name(),
            &self.provider.model(),
            crate::telemetry::SessionEndReason::NormalExit,
        );
        self.persist_soft_interrupt_snapshot();
        self.session.mark_closed();
        if !self.session.messages.is_empty() {
            self.persist_session_best_effort("session close state");
        }
    }

    pub fn mark_crashed(&mut self, message: Option<String>) {
        crate::telemetry::record_crash(
            self.provider.name(),
            &self.provider.model(),
            crate::telemetry::SessionEndReason::Unknown,
        );
        self.persist_soft_interrupt_snapshot();
        self.session.mark_crashed(message);
        if !self.session.messages.is_empty() {
            self.persist_session_best_effort("session crash state");
        }
    }

    /// Get the last token usage from the most recent API request
    pub fn last_usage(&self) -> &TokenUsage {
        &self.last_usage
    }

    /// Export the full conversation as a markdown transcript.
    pub fn export_conversation_markdown(&self) -> String {
        let mut md = String::new();
        for msg in &self.session.messages {
            let role_label = match msg.role {
                Role::User => "User",
                Role::Assistant => "Assistant",
            };
            md.push_str(&format!("### {}\n\n", role_label));
            for block in &msg.content {
                match block {
                    ContentBlock::Text { text, .. } => {
                        md.push_str(text);
                        md.push_str("\n\n");
                    }
                    ContentBlock::Reasoning { text } => {
                        md.push_str(&format!("*Thinking:* {}\n\n", text));
                    }
                    ContentBlock::ToolUse { name, input, .. } => {
                        let input_str = serde_json::to_string_pretty(input)
                            .unwrap_or_else(|_| input.to_string());
                        md.push_str(&format!(
                            "**Tool: `{}`**\n```json\n{}\n```\n\n",
                            name, input_str
                        ));
                    }
                    ContentBlock::ToolResult {
                        content, is_error, ..
                    } => {
                        let label = if is_error == &Some(true) {
                            "Error"
                        } else {
                            "Result"
                        };
                        // Truncate very long results
                        let display = if content.len() > 2000 {
                            format!(
                                "{}... (truncated, {} chars total)",
                                crate::util::truncate_str(content, 2000),
                                content.len()
                            )
                        } else {
                            content.clone()
                        };
                        md.push_str(&format!("**{}:**\n```\n{}\n```\n\n", label, display));
                    }
                    ContentBlock::Image { .. } => {
                        md.push_str("[Image]\n\n");
                    }
                    ContentBlock::OpenAICompaction { .. } => {
                        md.push_str("[OpenAI native compaction]\n\n");
                    }
                }
            }
        }
        md
    }
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
