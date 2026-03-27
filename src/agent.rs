#![allow(unused_assignments)]

mod compaction_support;
mod interrupts;
mod message_support;
mod prompt_support;
mod response_recovery;
mod stream_support;
mod tool_support;
mod utils;

use self::stream_support::{
    send_stream_keepalive_broadcast, send_stream_keepalive_mpsc, stream_keepalive_ticker,
};
use self::tool_support::{print_tool_summary, tool_output_to_content_blocks};
use self::utils::{git_state_for_dir, trace_enabled};
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
use crate::session::{EnvSnapshot, Session, SessionStatus, StoredDisplayRole, StoredMessage};
use crate::skill::SkillRegistry;
use crate::tool::{Registry, ToolContext, ToolExecutionMode};
use anyhow::Result;
use chrono::Utc;
use futures::StreamExt;
use std::collections::HashSet;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc};

pub use interrupts::{
    BackgroundToolSignal, GracefulShutdownSignal, InterruptSignal, SoftInterruptMessage,
    SoftInterruptQueue, SoftInterruptSource,
};
use interrupts::{NoToolCallOutcome, PostToolInterruptOutcome};

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct StreamError {
    pub message: String,
    pub retry_after_secs: Option<u64>,
}

impl StreamError {
    fn new(message: String, retry_after_secs: Option<u64>) -> Self {
        Self {
            message,
            retry_after_secs,
        }
    }
}

const JCODE_NATIVE_TOOLS: &[&str] = &["selfdev", "communicate"];
static RECOVERED_TEXT_WRAPPED_TOOL_CALLS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
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
    skills: SkillRegistry,
    session: Session,
    active_skill: Option<String>,
    allowed_tools: Option<HashSet<String>>,
    /// Provider-specific session ID for conversation resume (e.g., Claude Code CLI session)
    provider_session_id: Option<String>,
    /// Last upstream provider (OpenRouter) observed for this session
    last_upstream_provider: Option<String>,
    /// Last observed transport/connection type for this session
    last_connection_type: Option<String>,
    /// Pending swarm alerts to inject into the next turn
    pending_alerts: Vec<String>,
    /// Transient reminder injected into provider requests for the current turn only.
    /// Not persisted to session history.
    current_turn_system_reminder: Option<String>,
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
        let skills = SkillRegistry::load().unwrap_or_default();
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
            pending_alerts: Vec::new(),
            current_turn_system_reminder: None,
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

    pub fn available_skill_names(&self) -> Vec<String> {
        self.skills
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
        crate::telemetry::begin_session(&agent.provider.name(), &agent.provider.model());
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
            if let Err(e) = agent.provider.set_model(&model) {
                logging::error(&format!(
                    "Failed to restore session model '{}': {}",
                    model, e
                ));
            }
        } else {
            agent.session.model = Some(agent.provider.model());
        }
        agent.seed_compaction_from_session();
        agent.log_env_snapshot("attach");
        crate::telemetry::begin_session(&agent.provider.name(), &agent.provider.model());
        agent
    }

    fn seed_compaction_from_session(&mut self) {
        logging::info(&format!(
            "seed_compaction_from_session: session has {} messages",
            self.session.messages.len()
        ));
        let compaction = self.registry.compaction();
        let mut manager = compaction.try_write().expect("compaction lock");
        manager.reset();
        let budget = self.provider.context_window();
        manager.set_budget(budget);
        if let Some(state) = self.session.compaction.as_ref() {
            manager.restore_persisted_state(state, self.session.messages.len());
        } else {
            manager.seed_restored_messages(self.session.messages.len());
        }
        logging::info(&format!(
            "seed_compaction_from_session: seeded compaction with {} messages",
            self.session.messages.len()
        ));
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
            manager.restore_persisted_state(&state, self.session.messages.len());
        }

        self.cache_tracker.reset();
        self.locked_tools = None;
        self.provider_session_id = None;
        self.session.provider_session_id = None;
        self.session.save()?;
        Ok(())
    }

    fn messages_for_provider(&mut self) -> (Vec<Message>, Option<CompactionEvent>) {
        // Convert session messages to provider messages (single allocation)
        let all_messages = self.session.messages_for_provider();
        if self.provider.uses_jcode_compaction() || self.session.compaction.is_some() {
            let compaction = self.registry.compaction();
            match compaction.try_write() {
                Ok(mut manager) => {
                    if self.provider.uses_jcode_compaction() {
                        let action =
                            manager.ensure_context_fits(&all_messages, self.provider.clone());
                        match action {
                            crate::compaction::CompactionAction::BackgroundStarted { trigger } => {
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
                    let messages = manager.messages_for_api_with(&all_messages);
                    let event = if self.provider.uses_jcode_compaction() {
                        manager.take_compaction_event()
                    } else {
                        None
                    };
                    if event.is_some() {
                        self.sync_session_compaction_state_from_manager(&manager);
                    }
                    logging::info(&format!(
                        "messages_for_provider (compaction): returning {} messages, roles: {:?}",
                        messages.len(),
                        messages
                            .iter()
                            .map(|m| format!("{:?}", m.role))
                            .collect::<Vec<_>>()
                    ));
                    return (messages, event);
                }
                Err(_) => {
                    logging::info("messages_for_provider: compaction lock failed, using session");
                }
            };
        }
        logging::info(&format!(
            "messages_for_provider (session): returning {} messages, roles: {:?}",
            all_messages.len(),
            all_messages
                .iter()
                .map(|m| format!("{:?}", m.role))
                .collect::<Vec<_>>()
        ));
        (all_messages, None)
    }

    fn repair_missing_tool_outputs(&mut self) -> usize {
        let mut known_results = HashSet::new();
        for msg in &self.session.messages {
            if let Role::User = msg.role {
                for block in &msg.content {
                    if let ContentBlock::ToolResult { tool_use_id, .. } = block {
                        known_results.insert(tool_use_id.clone());
                    }
                }
            }
        }

        let mut repaired = 0usize;
        let mut index = 0usize;
        while index < self.session.messages.len() {
            let mut missing_for_message: Vec<String> = Vec::new();
            if let Role::Assistant = self.session.messages[index].role {
                for block in &self.session.messages[index].content {
                    if let ContentBlock::ToolUse { id, .. } = block {
                        if !known_results.contains(id) {
                            known_results.insert(id.clone());
                            missing_for_message.push(id.clone());
                        }
                    }
                }
            }

            if !missing_for_message.is_empty() {
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
                        .insert_message(index + 1 + offset, stored_message);
                    repaired += 1;
                }
                index += missing_for_message.len();
            }

            index += 1;
        }

        if repaired > 0 {
            let _ = self.session.save();
            self.cache_tracker.reset();
            self.locked_tools = None;
        }

        repaired
    }

    pub fn session_id(&self) -> &str {
        &self.session.id
    }

    /// Mark this agent session as closed and persist it.
    pub fn mark_closed(&mut self) {
        crate::telemetry::end_session_with_reason(
            &self.provider.name(),
            &self.provider.model(),
            crate::telemetry::SessionEndReason::NormalExit,
        );
        self.session.mark_closed();
        if !self.session.messages.is_empty() {
            let _ = self.session.save();
        }
    }

    pub fn mark_crashed(&mut self, message: Option<String>) {
        crate::telemetry::record_crash(
            &self.provider.name(),
            &self.provider.model(),
            crate::telemetry::SessionEndReason::Unknown,
        );
        self.session.mark_crashed(message);
        if !self.session.messages.is_empty() {
            let _ = self.session.save();
        }
    }

    /// Get the last token usage from the most recent API request
    pub fn last_usage(&self) -> &TokenUsage {
        &self.last_usage
    }

    /// Set logging context for this agent's session/provider
    fn set_log_context(&self) {
        logging::set_session(&self.session.id);
        logging::set_provider_info(self.provider.name(), &self.provider.model());
    }

    /// Record a lightweight environment snapshot for post-mortem debugging
    fn log_env_snapshot(&mut self, reason: &str) {
        let snapshot = self.build_env_snapshot(reason);
        self.session.record_env_snapshot(snapshot.clone());
        if !self.session.messages.is_empty() {
            let _ = self.session.save();
        }
        if let Ok(json) = serde_json::to_string(&snapshot) {
            logging::info(&format!("ENV_SNAPSHOT {}", json));
        } else {
            logging::info("ENV_SNAPSHOT {}");
        }
    }

    fn build_env_snapshot(&self, reason: &str) -> EnvSnapshot {
        let (jcode_git_hash, jcode_git_dirty) = if let Some(repo_dir) = build::get_repo_dir() {
            (
                build::current_git_hash(&repo_dir).ok(),
                build::is_working_tree_dirty(&repo_dir).ok(),
            )
        } else {
            (None, None)
        };

        let working_dir = self.session.working_dir.clone();
        let working_git = working_dir
            .as_deref()
            .and_then(|dir| git_state_for_dir(Path::new(dir)));

        EnvSnapshot {
            captured_at: Utc::now(),
            reason: reason.to_string(),
            session_id: self.session.id.clone(),
            working_dir,
            provider: self.provider.name().to_string(),
            model: self.provider.model().to_string(),
            jcode_version: env!("JCODE_VERSION").to_string(),
            jcode_git_hash,
            jcode_git_dirty,
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            pid: std::process::id(),
            is_selfdev: self.session.is_self_dev(),
            is_debug: self.session.is_debug,
            is_canary: self.session.is_canary,
            testing_build: self.session.testing_build.clone(),
            working_git,
        }
    }

    pub fn message_count(&self) -> usize {
        self.session.messages.len()
    }

    pub fn last_message_role(&self) -> Option<Role> {
        self.session.messages.last().map(|m| m.role.clone())
    }

    /// Get the text content of the last message (first Text block)
    pub fn last_message_text(&self) -> Option<&str> {
        self.session.messages.last().and_then(|m| {
            m.content.iter().find_map(|block| {
                if let ContentBlock::Text { text, .. } = block {
                    Some(text.as_str())
                } else {
                    None
                }
            })
        })
    }

    /// Build a transcript string for memory extraction
    /// This is a standalone method so it can be called before spawning async tasks
    pub fn build_transcript_for_extraction(&self) -> String {
        let mut transcript = String::new();
        for msg in &self.session.messages {
            let role = match msg.role {
                Role::User => "User",
                Role::Assistant => "Assistant",
            };
            transcript.push_str(&format!("**{}:**\n", role));
            for block in &msg.content {
                match block {
                    ContentBlock::Text { text, .. } => {
                        transcript.push_str(text);
                        transcript.push('\n');
                    }
                    ContentBlock::ToolUse { name, .. } => {
                        transcript.push_str(&format!("[Used tool: {}]\n", name));
                    }
                    ContentBlock::ToolResult { content, .. } => {
                        let preview = if content.len() > 200 {
                            format!("{}...", crate::util::truncate_str(content, 200))
                        } else {
                            content.clone()
                        };
                        transcript.push_str(&format!("[Result: {}]\n", preview));
                    }
                    ContentBlock::Reasoning { .. } => {}
                    ContentBlock::Image { .. } => {
                        transcript.push_str("[Image]\n");
                    }
                    ContentBlock::OpenAICompaction { .. } => {
                        transcript.push_str("[OpenAI native compaction]\n");
                    }
                }
            }
            transcript.push('\n');
        }
        transcript
    }

    pub fn last_assistant_text(&self) -> Option<String> {
        self.session
            .messages
            .iter()
            .rev()
            .find(|msg| msg.role == Role::Assistant)
            .map(|msg| {
                msg.content
                    .iter()
                    .filter_map(|c| {
                        if let ContentBlock::Text { text, .. } = c {
                            Some(text.clone())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
    }

    pub fn last_upstream_provider(&self) -> Option<String> {
        self.last_upstream_provider
            .clone()
            .or_else(|| self.provider.preferred_provider())
    }

    pub fn last_connection_type(&self) -> Option<String> {
        self.last_connection_type.clone()
    }

    pub fn provider_name(&self) -> String {
        self.provider.name().to_string()
    }

    pub fn provider_model(&self) -> String {
        self.provider.model().to_string()
    }

    pub fn set_premium_mode(&self, mode: crate::provider::copilot::PremiumMode) {
        self.provider.set_premium_mode(mode);
    }

    pub fn premium_mode(&self) -> crate::provider::copilot::PremiumMode {
        self.provider.premium_mode()
    }

    pub fn provider_fork(&self) -> Arc<dyn Provider> {
        self.provider.fork()
    }

    pub fn provider_handle(&self) -> Arc<dyn Provider> {
        Arc::clone(&self.provider)
    }

    pub fn available_models(&self) -> Vec<&'static str> {
        self.provider.available_models()
    }

    pub fn available_models_for_switching(&self) -> Vec<String> {
        self.provider.available_models_for_switching()
    }

    pub fn available_models_display(&self) -> Vec<String> {
        self.provider.available_models_display()
    }

    pub fn model_routes(&self) -> Vec<crate::provider::ModelRoute> {
        self.provider.model_routes()
    }

    pub fn registry(&self) -> Registry {
        self.registry.clone()
    }

    pub async fn compaction_mode(&self) -> crate::config::CompactionMode {
        let mode = self.registry.compaction().read().await.mode();
        mode
    }

    pub async fn set_compaction_mode(&self, mode: crate::config::CompactionMode) -> Result<()> {
        let compaction = self.registry.compaction();
        let mut manager = compaction.write().await;
        manager.set_mode(mode);
        Ok(())
    }

    pub fn provider_messages(&self) -> Vec<Message> {
        self.session.messages_for_provider()
    }

    pub fn set_model(&mut self, model: &str) -> Result<()> {
        self.provider.set_model(model)?;
        self.session.model = Some(self.provider.model());
        self.log_env_snapshot("set_model");
        Ok(())
    }

    pub fn subagent_model(&self) -> Option<String> {
        self.session.subagent_model.clone()
    }

    pub fn set_subagent_model(&mut self, model: Option<String>) -> Result<()> {
        self.session.subagent_model = model;
        self.log_env_snapshot("set_subagent_model");
        self.session.save()?;
        Ok(())
    }

    pub fn autoreview_enabled(&self) -> Option<bool> {
        self.session.autoreview_enabled
    }

    pub fn set_autoreview_enabled(&mut self, enabled: bool) -> Result<()> {
        self.session.autoreview_enabled = Some(enabled);
        self.log_env_snapshot("set_autoreview_enabled");
        self.session.save()?;
        Ok(())
    }

    /// Get the short/friendly name for this session (e.g., "fox")
    pub fn session_short_name(&self) -> Option<&str> {
        self.session.short_name.as_deref()
    }

    /// Set the working directory for this session
    pub fn set_working_dir(&mut self, dir: &str) {
        if self.session.working_dir.as_deref() == Some(dir) {
            return;
        }
        self.session.working_dir = Some(dir.to_string());
        self.log_env_snapshot("working_dir");
    }

    /// Get the working directory for this session
    pub fn working_dir(&self) -> Option<&str> {
        self.session.working_dir.as_deref()
    }

    /// Get the stored messages (for transcript export)
    pub fn messages(&self) -> &[StoredMessage] {
        &self.session.messages
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
                                &content[..2000],
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

    /// Run a single turn with the given user message
    pub async fn run_once(&mut self, user_message: &str) -> Result<()> {
        self.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: user_message.to_string(),
                cache_control: None,
            }],
        );
        self.session.save()?;
        if trace_enabled() {
            eprintln!("[trace] session_id {}", self.session.id);
        }
        let _ = self.run_turn(true).await?;
        Ok(())
    }

    pub async fn run_once_capture(&mut self, user_message: &str) -> Result<String> {
        self.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: user_message.to_string(),
                cache_control: None,
            }],
        );
        self.session.save()?;
        if trace_enabled() {
            eprintln!("[trace] session_id {}", self.session.id);
        }
        self.run_turn(false).await
    }

    /// Run a single message with events streamed to a broadcast channel (for server mode)
    pub async fn run_once_streaming(
        &mut self,
        user_message: &str,
        event_tx: broadcast::Sender<ServerEvent>,
    ) -> Result<()> {
        // Inject any pending notifications before the user message
        let alerts = self.take_alerts();
        if !alerts.is_empty() {
            let alert_text = format!(
                "[NOTIFICATION]\nYou received {} notification(s) from other agents working in this codebase:\n\n{}\n\nUse the communicate tool (actions: list, read, message/broadcast, dm, channel, share) to coordinate with other agents.",
                alerts.len(),
                alerts.join("\n\n---\n\n")
            );
            self.add_message(
                Role::User,
                vec![ContentBlock::Text {
                    text: alert_text,
                    cache_control: None,
                }],
            );
        }

        self.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: user_message.to_string(),
                cache_control: None,
            }],
        );
        self.session.save()?;
        self.run_turn_streaming(event_tx).await
    }

    /// Run one conversation turn with streaming events via mpsc channel (per-client)
    pub async fn run_once_streaming_mpsc(
        &mut self,
        user_message: &str,
        images: Vec<(String, String)>,
        system_reminder: Option<String>,
        event_tx: mpsc::UnboundedSender<ServerEvent>,
    ) -> Result<()> {
        // Inject any pending notifications before the user message
        let alerts = self.take_alerts();
        if !alerts.is_empty() {
            let alert_text = format!(
                "[NOTIFICATION]\nYou received {} notification(s) from other agents working in this codebase:\n\n{}\n\nUse the communicate tool (actions: list, read, message/broadcast, dm, channel, share) to coordinate with other agents.",
                alerts.len(),
                alerts.join("\n\n---\n\n")
            );
            self.add_message(
                Role::User,
                vec![ContentBlock::Text {
                    text: alert_text,
                    cache_control: None,
                }],
            );
        }

        self.current_turn_system_reminder =
            system_reminder.filter(|value| !value.trim().is_empty());

        let mut blocks: Vec<ContentBlock> = images
            .into_iter()
            .map(|(media_type, data)| ContentBlock::Image { media_type, data })
            .collect();
        blocks.push(ContentBlock::Text {
            text: user_message.to_string(),
            cache_control: None,
        });

        if blocks.len() > 1 {
            crate::logging::info(&format!(
                "Agent received message with {} image(s)",
                blocks.len() - 1
            ));
        }

        self.add_message(Role::User, blocks);
        crate::telemetry::record_turn();
        self.session.save()?;
        let result = self.run_turn_streaming_mpsc(event_tx).await;
        self.current_turn_system_reminder = None;
        result
    }

    /// Clear conversation history
    pub fn clear(&mut self) {
        let preserve_canary = self.session.is_canary;
        let preserve_testing_build = self.session.testing_build.clone();
        let preserve_debug = self.session.is_debug;
        let preserve_working_dir = self.session.working_dir.clone();

        self.session.mark_closed();
        let _ = self.session.save();

        let mut new_session = Session::create(None, None);
        new_session.mark_active();
        new_session.model = Some(self.provider.model());
        new_session.is_canary = preserve_canary;
        new_session.testing_build = preserve_testing_build;
        new_session.is_debug = preserve_debug;
        new_session.working_dir = preserve_working_dir;

        self.session = new_session;
        self.active_skill = None;
        self.provider_session_id = None;
        self.seed_compaction_from_session();
    }

    /// Clear provider session so the next turn sends full context.
    pub fn reset_provider_session(&mut self) {
        self.provider_session_id = None;
        self.session.provider_session_id = None;
        let _ = self.session.save();
    }

    /// Unlock the tool list so the next API request picks up any new tools.
    /// Called after MCP reload or when the user explicitly wants new tools.
    pub fn unlock_tools(&mut self) {
        if self.locked_tools.is_some() {
            logging::info("Tool list unlocked — next request will pick up current tools");
            self.locked_tools = None;
            self.cache_tracker.reset();
        }
    }

    /// Unlock tools if a tool execution may have changed the registry
    /// (e.g., mcp connect/disconnect/reload)
    fn unlock_tools_if_needed(&mut self, tool_name: &str) {
        if tool_name == "mcp" {
            self.unlock_tools();
        }
    }

    pub fn is_canary(&self) -> bool {
        self.session.is_canary
    }

    pub fn is_debug(&self) -> bool {
        self.session.is_debug
    }

    pub fn set_canary(&mut self, build_hash: &str) {
        self.session.set_canary(build_hash);
        if let Err(err) = self.session.save() {
            logging::error(&format!("Failed to persist canary session state: {}", err));
        }
    }

    /// Mark this session as a debug/test session
    /// Set a custom system prompt override (used by ambient mode).
    /// When set, this replaces the normal system prompt entirely.
    pub fn set_system_prompt(&mut self, prompt: &str) {
        self.system_prompt_override = Some(prompt.to_string());
    }

    pub fn set_debug(&mut self, is_debug: bool) {
        self.session.set_debug(is_debug);
        if let Err(err) = self.session.save() {
            logging::error(&format!("Failed to persist debug session state: {}", err));
        }
    }

    /// Enable or disable memory features for this session.
    pub fn set_memory_enabled(&mut self, enabled: bool) {
        self.memory_enabled = enabled;
        if !enabled {
            crate::memory::clear_pending_memory(&self.session.id);
        }
    }

    /// Check whether memory features are enabled for this session.
    pub fn memory_enabled(&self) -> bool {
        self.memory_enabled
    }

    /// Set the stdin request channel for interactive stdin forwarding
    pub fn set_stdin_request_tx(
        &mut self,
        tx: tokio::sync::mpsc::UnboundedSender<crate::tool::StdinInputRequest>,
    ) {
        self.stdin_request_tx = Some(tx);
    }

    async fn tool_definitions(&mut self) -> Vec<ToolDefinition> {
        if self.session.is_canary {
            self.registry.register_selfdev_tools().await;
        }

        // Return locked tools if available (prevents cache invalidation from
        // MCP tools arriving asynchronously after the first API request)
        if let Some(ref locked) = self.locked_tools {
            return locked.clone();
        }

        let mut tools = self.registry.definitions(self.allowed_tools.as_ref()).await;
        if !self.session.is_canary {
            tools.retain(|tool| tool.name != "selfdev");
        }

        // Lock the tool list on first call to prevent cache invalidation
        // when MCP tools arrive asynchronously mid-session
        logging::info(&format!(
            "Locking tool list at {} tools for cache stability",
            tools.len()
        ));
        self.locked_tools = Some(tools.clone());
        tools
    }

    pub async fn tool_names(&self) -> Vec<String> {
        self.registry.tool_names().await
    }

    /// Get full tool definitions for debug introspection (bypasses lock)
    pub async fn tool_definitions_for_debug(&self) -> Vec<crate::message::ToolDefinition> {
        if self.session.is_canary {
            self.registry.register_selfdev_tools().await;
        }
        let mut tools = self.registry.definitions(self.allowed_tools.as_ref()).await;
        if !self.session.is_canary {
            tools.retain(|tool| tool.name != "selfdev");
        }
        tools
    }

    pub async fn execute_tool(
        &self,
        name: &str,
        input: serde_json::Value,
    ) -> Result<crate::tool::ToolOutput> {
        self.validate_tool_allowed(name)?;

        let call_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| format!("debug-{}", d.as_millis()))
            .unwrap_or_else(|_| "debug".to_string());
        let ctx = ToolContext {
            session_id: self.session.id.clone(),
            message_id: self.session.id.clone(),
            tool_call_id: call_id,
            working_dir: self.working_dir().map(PathBuf::from),
            stdin_request_tx: self.stdin_request_tx.clone(),
            graceful_shutdown_signal: Some(self.graceful_shutdown.clone()),
            execution_mode: ToolExecutionMode::Direct,
        };
        self.registry.execute(name, input, ctx).await
    }

    pub fn add_manual_tool_use(
        &mut self,
        tool_call_id: String,
        tool_name: String,
        input: serde_json::Value,
    ) -> Result<String> {
        let message_id = self.add_message(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: tool_call_id,
                name: tool_name,
                input,
            }],
        );
        self.session.save()?;
        Ok(message_id)
    }

    pub fn add_manual_tool_result(
        &mut self,
        tool_call_id: String,
        output: crate::tool::ToolOutput,
        duration_ms: u64,
    ) -> Result<()> {
        let blocks = tool_output_to_content_blocks(tool_call_id, output);
        self.add_message_with_duration(Role::User, blocks, Some(duration_ms));
        self.session.save()?;
        Ok(())
    }

    pub fn add_manual_tool_error(
        &mut self,
        tool_call_id: String,
        error: String,
        duration_ms: u64,
    ) -> Result<()> {
        self.add_message_with_duration(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: tool_call_id,
                content: error,
                is_error: Some(true),
            }],
            Some(duration_ms),
        );
        self.session.save()?;
        Ok(())
    }

    fn validate_tool_allowed(&self, name: &str) -> Result<()> {
        if let Some(allowed) = self.allowed_tools.as_ref() {
            if !allowed.contains(name) {
                return Err(anyhow::anyhow!("Tool '{}' is not allowed", name));
            }
        }
        Ok(())
    }

    /// Restore a session by ID (loads from disk)
    pub fn restore_session(&mut self, session_id: &str) -> Result<SessionStatus> {
        let session = Session::load(session_id)?;
        logging::info(&format!(
            "Restoring session '{}' with {} messages, provider_session_id: {:?}, status: {}",
            session_id,
            session.messages.len(),
            session.provider_session_id,
            session.status.display()
        ));
        let previous_status = session.status.clone();
        // Restore provider_session_id for Claude CLI session resume
        self.provider_session_id = session.provider_session_id.clone();
        self.session = session;
        self.active_skill = None;
        if let Some(model) = self.session.model.clone() {
            if let Err(e) = self.provider.set_model(&model) {
                logging::error(&format!(
                    "Failed to restore session model '{}': {}",
                    model, e
                ));
            }
        } else {
            self.session.model = Some(self.provider.model());
        }
        self.session.mark_active();
        logging::info(&format!(
            "restore_session: loaded session {} with {} messages, calling seed_compaction",
            session_id,
            self.session.messages.len()
        ));
        self.seed_compaction_from_session();
        self.log_env_snapshot("resume");
        if let Err(err) = self.session.save() {
            logging::error(&format!(
                "Failed to persist resumed session state for {}: {}",
                session_id, err
            ));
        }
        logging::info(&format!(
            "Session restored: {} messages in session",
            self.session.messages.len()
        ));
        Ok(previous_status)
    }

    /// Get conversation history for sync
    pub fn get_history(&self) -> Vec<HistoryMessage> {
        crate::session::render_messages(&self.session)
            .into_iter()
            .map(|msg| HistoryMessage {
                role: msg.role,
                content: msg.content,
                tool_calls: if msg.tool_calls.is_empty() {
                    None
                } else {
                    Some(msg.tool_calls)
                },
                tool_data: msg.tool_data,
            })
            .collect()
    }

    pub fn get_rendered_images(&self) -> Vec<crate::session::RenderedImage> {
        crate::session::render_images(&self.session)
    }

    pub fn get_tool_call_summaries(&self, limit: usize) -> Vec<crate::protocol::ToolCallSummary> {
        crate::session::summarize_tool_calls(&self.session, limit)
    }

    /// Start an interactive REPL
    pub async fn repl(&mut self) -> Result<()> {
        println!("J-Code - Coding Agent");
        println!("Type your message, or 'quit' to exit.");

        // Show available skills
        let skill_list = self.skills.list();
        if !skill_list.is_empty() {
            println!(
                "Available skills: {}",
                skill_list
                    .iter()
                    .map(|s| format!("/{}", s.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        println!();

        loop {
            print!("> ");
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;

            let input = input.trim();
            if input.is_empty() {
                continue;
            }

            if input == "quit" || input == "exit" {
                break;
            }

            if input == "clear" {
                self.clear();
                println!("Conversation cleared.");
                continue;
            }

            // Check for skill invocation
            if let Some(skill_name) = SkillRegistry::parse_invocation(input) {
                if let Some(skill) = self.skills.get(skill_name) {
                    println!("Activating skill: {}", skill.name);
                    println!("{}\n", skill.description);
                    self.active_skill = Some(skill_name.to_string());
                    continue;
                } else {
                    println!("Unknown skill: /{}", skill_name);
                    println!(
                        "Available: {}",
                        self.skills
                            .list()
                            .iter()
                            .map(|s| format!("/{}", s.name))
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    continue;
                }
            }

            if let Err(e) = self.run_once(input).await {
                eprintln!("\nError: {}\n", e);
            }

            println!();
        }

        // Extract memories from session before exiting
        self.extract_session_memories().await;

        Ok(())
    }

    /// Extract memories from the session transcript
    /// Returns the number of memories extracted, or 0 if none/skipped
    pub async fn extract_session_memories(&self) -> usize {
        if !self.memory_enabled {
            return 0;
        }

        // Need at least 4 messages for meaningful extraction
        if self.session.messages.len() < 4 {
            return 0;
        }

        logging::info(&format!(
            "Extracting memories from {} messages",
            self.session.messages.len()
        ));

        // Build transcript
        let mut transcript = String::new();
        for msg in &self.session.messages {
            let role = match msg.role {
                Role::User => "User",
                Role::Assistant => "Assistant",
            };
            transcript.push_str(&format!("**{}:**\n", role));
            for block in &msg.content {
                match block {
                    ContentBlock::Text { text, .. } => {
                        transcript.push_str(&text);
                        transcript.push('\n');
                    }
                    ContentBlock::ToolUse { name, .. } => {
                        transcript.push_str(&format!("[Used tool: {}]\n", name));
                    }
                    ContentBlock::ToolResult { content, .. } => {
                        let preview = if content.len() > 200 {
                            format!("{}...", crate::util::truncate_str(content, 200))
                        } else {
                            content.clone()
                        };
                        transcript.push_str(&format!("[Result: {}]\n", preview));
                    }
                    ContentBlock::Reasoning { .. } => {}
                    ContentBlock::Image { .. } => {
                        transcript.push_str("[Image]\n");
                    }
                    ContentBlock::OpenAICompaction { .. } => {
                        transcript.push_str("[OpenAI native compaction]\n");
                    }
                }
            }
            transcript.push('\n');
        }

        // Extract using sidecar
        let sidecar = crate::sidecar::Sidecar::new();
        match sidecar.extract_memories(&transcript).await {
            Ok(extracted) if !extracted.is_empty() => {
                let manager = self
                    .session
                    .working_dir
                    .as_deref()
                    .map(|dir| crate::memory::MemoryManager::new().with_project_dir(dir))
                    .unwrap_or_else(crate::memory::MemoryManager::new);
                let mut stored_count = 0;

                for memory in &extracted {
                    let category = crate::memory::MemoryCategory::from_extracted(&memory.category);

                    let trust = match memory.trust.as_str() {
                        "high" => crate::memory::TrustLevel::High,
                        "low" => crate::memory::TrustLevel::Low,
                        _ => crate::memory::TrustLevel::Medium,
                    };

                    let entry = crate::memory::MemoryEntry::new(category, &memory.content)
                        .with_source(&self.session.id)
                        .with_trust(trust);

                    if manager.remember_project(entry).is_ok() {
                        stored_count += 1;
                    }
                }

                if stored_count > 0 {
                    logging::info(&format!("Extracted {} memories from session", stored_count));
                }
                return stored_count;
            }
            Ok(_) => return 0,
            Err(e) => {
                logging::info(&format!("Memory extraction skipped: {}", e));
                return 0;
            }
        }
    }

    /// Run turns until no more tool calls
    /// Maximum number of context-limit compaction retries before giving up.
    const MAX_CONTEXT_LIMIT_RETRIES: u32 = 5;
    const MAX_INCOMPLETE_CONTINUATION_ATTEMPTS: u32 = 3;

    async fn run_turn(&mut self, print_output: bool) -> Result<String> {
        self.set_log_context();
        let mut final_text = String::new();
        let trace = trace_enabled();
        let mut context_limit_retries = 0u32;
        let mut incomplete_continuations = 0u32;

        loop {
            let repaired = self.repair_missing_tool_outputs();
            if repaired > 0 {
                logging::warn(&format!(
                    "Recovered {} missing tool output(s) before API call",
                    repaired
                ));
            }
            let (messages, compaction_event) = self.messages_for_provider();
            if let Some(event) = compaction_event {
                // Reset cache tracker and tool lock on compaction since the message history changes
                self.cache_tracker.reset();
                self.locked_tools = None;
                if print_output {
                    let tokens_str = event
                        .pre_tokens
                        .map(|t| format!(" ({} tokens)", t))
                        .unwrap_or_default();
                    println!("📦 Context compacted ({}){}", event.trigger, tokens_str);
                }
            }

            let tools = self.tool_definitions().await;
            // Non-blocking memory: uses pending result from last turn, spawns check for next turn
            let memory_pending = self.build_memory_prompt_nonblocking(&messages);
            // Use split prompt for better caching - static content cached, dynamic not
            let split_prompt = self.build_system_prompt_split(None);

            // Check for client-side cache violations before memory injection.
            // Memory is an ephemeral suffix that changes each turn; tracking it would cause
            // false-positive violations every turn (prior turn's memory ≠ current history prefix).
            if self.should_track_client_cache() {
                if let Some(violation) = self.cache_tracker.record_request(&messages) {
                    logging::warn(&format!(
                        "CLIENT_CACHE_VIOLATION: {} | turn={} messages={}",
                        violation.reason, violation.turn, violation.message_count
                    ));
                }
            }

            // Inject memory as a user message at the end (preserves cache prefix)
            let mut messages_with_memory = messages;
            if let Some(memory) = memory_pending.as_ref() {
                let memory_count = memory.count.max(1);
                let age_ms = memory.computed_at.elapsed().as_millis() as u64;
                crate::memory::record_injected_prompt(&memory.prompt, memory_count, age_ms);
                logging::info(&format!(
                    "Memory injected as message ({} chars)",
                    memory.prompt.len()
                ));
                let memory_msg =
                    format!("<system-reminder>\n{}\n</system-reminder>", memory.prompt);
                messages_with_memory.push(Message::user(&memory_msg));
            }

            logging::info(&format!(
                "API call starting: {} messages, {} tools",
                messages_with_memory.len(),
                tools.len()
            ));
            let api_start = Instant::now();

            // Publish status for TUI to show during Task execution
            Bus::global().publish(BusEvent::SubagentStatus(SubagentStatus {
                session_id: self.session.id.clone(),
                status: "calling API".to_string(),
                model: Some(self.provider.model()),
            }));

            let stamped;
            let send_messages: &[Message] = if crate::config::config().features.message_timestamps {
                stamped = Message::with_timestamps(&messages_with_memory);
                &stamped
            } else {
                &messages_with_memory
            };
            let mut stream = match self
                .provider
                .complete_split(
                    send_messages,
                    &tools,
                    &split_prompt.static_part,
                    &split_prompt.dynamic_part,
                    self.provider_session_id.as_deref(),
                )
                .await
            {
                Ok(stream) => stream,
                Err(e) => {
                    if self.try_auto_compact_after_context_limit(&e.to_string()) {
                        context_limit_retries += 1;
                        if context_limit_retries > Self::MAX_CONTEXT_LIMIT_RETRIES {
                            logging::warn(
                                "Context-limit compaction retry limit reached; giving up",
                            );
                            return Err(anyhow::anyhow!(
                                "Context limit exceeded after {} compaction retries",
                                Self::MAX_CONTEXT_LIMIT_RETRIES
                            ));
                        }
                        continue;
                    }
                    return Err(e);
                }
            };

            // Successful API call - reset retry counter
            context_limit_retries = 0;

            logging::info(&format!(
                "API stream opened in {:.2}s",
                api_start.elapsed().as_secs_f64()
            ));

            Bus::global().publish(BusEvent::SubagentStatus(SubagentStatus {
                session_id: self.session.id.clone(),
                status: "streaming".to_string(),
                model: Some(self.provider.model()),
            }));

            let mut text_content = String::new();
            #[allow(unused_variables)]
            let text_wrapped_detected = false;
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut current_tool: Option<ToolCall> = None;
            let mut current_tool_input = String::new();
            let mut usage_input: Option<u64> = None;
            let mut usage_output: Option<u64> = None;
            let mut usage_cache_read: Option<u64> = None;
            let mut usage_cache_creation: Option<u64> = None;
            let mut saw_message_end = false;
            let mut stop_reason: Option<String> = None;
            let mut _thinking_start: Option<Instant> = None;
            let store_reasoning_content = self.provider.name() == "openrouter";
            let mut reasoning_content = String::new();
            // Track tool results from provider (already executed by Claude Code CLI)
            let mut sdk_tool_results: std::collections::HashMap<String, (String, bool)> =
                std::collections::HashMap::new();
            let mut openai_native_compaction: Option<(String, usize)> = None;

            let mut retry_after_compaction = false;
            while let Some(event) = stream.next().await {
                let event = match event {
                    Ok(event) => event,
                    Err(e) => {
                        let err_str = e.to_string();
                        if self.try_auto_compact_after_context_limit(&err_str) {
                            context_limit_retries += 1;
                            if context_limit_retries > Self::MAX_CONTEXT_LIMIT_RETRIES {
                                logging::warn(
                                    "Context-limit compaction retry limit reached; giving up",
                                );
                                return Err(anyhow::anyhow!(
                                    "Context limit exceeded after {} compaction retries",
                                    Self::MAX_CONTEXT_LIMIT_RETRIES
                                ));
                            }
                            retry_after_compaction = true;
                            break;
                        }
                        return Err(e);
                    }
                };

                match event {
                    StreamEvent::ThinkingStart => {
                        // Track start but don't print - wait for ThinkingDone
                        _thinking_start = Some(Instant::now());
                    }
                    StreamEvent::ThinkingDelta(thinking_text) => {
                        // Display reasoning content only if enabled
                        if print_output && crate::config::config().display.show_thinking {
                            println!("💭 {}", thinking_text);
                        }
                        if store_reasoning_content {
                            reasoning_content.push_str(&thinking_text);
                        }
                    }
                    StreamEvent::ThinkingEnd => {
                        // Don't print here - ThinkingDone has accurate timing
                        _thinking_start = None;
                    }
                    StreamEvent::ThinkingDone { duration_secs } => {
                        // Bridge provides accurate wall-clock timing
                        if print_output {
                            println!("Thought for {:.1}s\n", duration_secs);
                        }
                    }
                    StreamEvent::TextDelta(text) => {
                        if print_output {
                            print!("{}", text);
                            io::stdout().flush()?;
                        }
                        text_content.push_str(&text);
                    }
                    StreamEvent::ToolUseStart { id, name } => {
                        if trace {
                            eprintln!("\n[trace] tool_use_start name={} id={}", name, id);
                        }
                        if print_output {
                            print!("\n[{}] ", name);
                            io::stdout().flush()?;
                        }
                        current_tool = Some(ToolCall {
                            id,
                            name,
                            input: serde_json::Value::Null,
                            intent: None,
                        });
                        current_tool_input.clear();
                    }
                    StreamEvent::ToolInputDelta(delta) => {
                        current_tool_input.push_str(&delta);
                    }
                    StreamEvent::ToolUseEnd => {
                        if let Some(mut tool) = current_tool.take() {
                            // Parse the accumulated JSON
                            let tool_input =
                                serde_json::from_str::<serde_json::Value>(&current_tool_input)
                                    .unwrap_or(serde_json::Value::Null);
                            tool.input = tool_input.clone();

                            if trace {
                                if current_tool_input.trim().is_empty() {
                                    eprintln!("[trace] tool_input {} (empty)", tool.name);
                                } else if tool_input == serde_json::Value::Null {
                                    eprintln!(
                                        "[trace] tool_input {} (raw) {}",
                                        tool.name, current_tool_input
                                    );
                                } else {
                                    let pretty = serde_json::to_string_pretty(&tool_input)
                                        .unwrap_or_else(|_| tool_input.to_string());
                                    eprintln!("[trace] tool_input {} {}", tool.name, pretty);
                                }
                            }

                            if print_output {
                                // Show brief tool info
                                print_tool_summary(&tool);
                            }

                            tool_calls.push(tool);
                            current_tool_input.clear();
                        }
                    }
                    StreamEvent::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        // SDK already executed this tool, store the result
                        if trace {
                            eprintln!(
                                "[trace] sdk_tool_result id={} is_error={} content_len={}",
                                tool_use_id,
                                is_error,
                                content.len()
                            );
                        }
                        sdk_tool_results.insert(tool_use_id, (content, is_error));
                    }
                    StreamEvent::TokenUsage {
                        input_tokens,
                        output_tokens,
                        cache_read_input_tokens,
                        cache_creation_input_tokens,
                    } => {
                        if let Some(input) = input_tokens {
                            usage_input = Some(input);
                        }
                        if let Some(output) = output_tokens {
                            usage_output = Some(output);
                        }
                        if cache_read_input_tokens.is_some() {
                            usage_cache_read = cache_read_input_tokens;
                        }
                        if cache_creation_input_tokens.is_some() {
                            usage_cache_creation = cache_creation_input_tokens;
                        }
                        if let Some(input) = usage_input {
                            self.update_compaction_usage_from_stream(
                                input,
                                usage_cache_read,
                                usage_cache_creation,
                            );
                        }
                        if trace {
                            eprintln!(
                                "[trace] token_usage input={} output={} cache_read={} cache_write={}",
                                usage_input.unwrap_or(0),
                                usage_output.unwrap_or(0),
                                usage_cache_read.unwrap_or(0),
                                usage_cache_creation.unwrap_or(0)
                            );
                        }
                    }
                    StreamEvent::ConnectionType { connection } => {
                        if trace {
                            eprintln!("[trace] connection_type={}", connection);
                        }
                        crate::telemetry::record_connection_type(&connection);
                        self.last_connection_type = Some(connection);
                    }
                    StreamEvent::ConnectionPhase { phase } => {
                        if trace {
                            eprintln!("[trace] connection_phase={}", phase);
                        }
                    }
                    StreamEvent::MessageEnd {
                        stop_reason: reason,
                    } => {
                        saw_message_end = true;
                        if reason.is_some() {
                            stop_reason = reason;
                        }
                        // Don't break yet - wait for SessionId which comes after MessageEnd
                        // (but stream close will also end the loop for providers without SessionId)
                    }
                    StreamEvent::SessionId(sid) => {
                        if trace {
                            eprintln!("[trace] session_id {}", sid);
                        }
                        self.provider_session_id = Some(sid.clone());
                        self.session.provider_session_id = Some(sid);
                        // We've received session_id, can exit the loop now
                        if saw_message_end {
                            break;
                        }
                    }
                    StreamEvent::UpstreamProvider { provider } => {
                        // Log upstream provider for local trace output
                        if trace {
                            eprintln!("[trace] upstream_provider={}", provider);
                        }
                        self.last_upstream_provider = Some(provider);
                    }
                    StreamEvent::Compaction {
                        trigger,
                        pre_tokens,
                        openai_encrypted_content,
                    } => {
                        if let Some(encrypted_content) = openai_encrypted_content {
                            openai_native_compaction.get_or_insert_with(|| {
                                (encrypted_content, self.session.messages.len())
                            });
                        }
                        if print_output {
                            let tokens_str = pre_tokens
                                .map(|t| format!(" ({} tokens)", t))
                                .unwrap_or_default();
                            println!("📦 Context compacted ({}){}", trigger, tokens_str);
                        }
                    }
                    StreamEvent::NativeToolCall {
                        request_id,
                        tool_name,
                        input,
                    } => {
                        // Execute native tool and send result back to SDK bridge
                        if trace {
                            eprintln!(
                                "[trace] native_tool_call request_id={} tool={}",
                                request_id, tool_name
                            );
                        }
                        let ctx = ToolContext {
                            session_id: self.session.id.clone(),
                            message_id: self.session.id.clone(),
                            tool_call_id: request_id.clone(),
                            working_dir: self.working_dir().map(PathBuf::from),
                            stdin_request_tx: self.stdin_request_tx.clone(),
                            graceful_shutdown_signal: Some(self.graceful_shutdown.clone()),
                            execution_mode: ToolExecutionMode::AgentTurn,
                        };
                        crate::telemetry::record_tool_call();
                        let tool_result = self.registry.execute(&tool_name, input, ctx).await;
                        if tool_result.is_err() {
                            crate::telemetry::record_tool_failure();
                        }
                        let native_result = match tool_result {
                            Ok(output) => NativeToolResult::success(request_id, output.output),
                            Err(e) => NativeToolResult::error(request_id, e.to_string()),
                        };
                        // Send result back to SDK bridge
                        if let Some(sender) = self.provider.native_result_sender() {
                            let _ = sender.send(native_result).await;
                        }
                    }
                    StreamEvent::Error {
                        message,
                        retry_after_secs,
                    } => {
                        if trace {
                            eprintln!("[trace] stream_error {}", message);
                        }
                        if self.try_auto_compact_after_context_limit(&message) {
                            context_limit_retries += 1;
                            if context_limit_retries > Self::MAX_CONTEXT_LIMIT_RETRIES {
                                logging::warn(
                                    "Context-limit compaction retry limit reached; giving up",
                                );
                                return Err(anyhow::anyhow!(
                                    "Context limit exceeded after {} compaction retries",
                                    Self::MAX_CONTEXT_LIMIT_RETRIES
                                ));
                            }
                            retry_after_compaction = true;
                            break;
                        }
                        return Err(StreamError::new(message, retry_after_secs).into());
                    }
                }
            }

            if retry_after_compaction {
                continue;
            }

            let api_elapsed = api_start.elapsed();
            logging::info(&format!(
                "API call complete in {:.2}s (input={} output={} cache_read={} cache_write={})",
                api_elapsed.as_secs_f64(),
                usage_input.unwrap_or(0),
                usage_output.unwrap_or(0),
                usage_cache_read.unwrap_or(0),
                usage_cache_creation.unwrap_or(0),
            ));

            if print_output
                && (usage_input.is_some()
                    || usage_output.is_some()
                    || usage_cache_read.is_some()
                    || usage_cache_creation.is_some())
            {
                let input = usage_input.unwrap_or(0);
                let output = usage_output.unwrap_or(0);
                let cache_read = usage_cache_read.unwrap_or(0);
                let cache_creation = usage_cache_creation.unwrap_or(0);
                let cache_str = if usage_cache_read.is_some() || usage_cache_creation.is_some() {
                    format!(
                        " cache_read: {} cache_write: {}",
                        cache_read, cache_creation
                    )
                } else {
                    String::new()
                };
                print!(
                    "\n[Tokens] upload: {} download: {}{}\n",
                    input, output, cache_str
                );
                io::stdout().flush()?;
            }

            // Store usage for debug queries
            self.last_usage = TokenUsage {
                input_tokens: usage_input.unwrap_or(0),
                output_tokens: usage_output.unwrap_or(0),
                cache_read_input_tokens: usage_cache_read,
                cache_creation_input_tokens: usage_cache_creation,
            };

            self.recover_text_wrapped_tool_call(&mut text_content, &mut tool_calls);

            // Add assistant message to history
            let mut content_blocks = Vec::new();
            if !text_content.is_empty() {
                content_blocks.push(ContentBlock::Text {
                    text: text_content.clone(),
                    cache_control: None,
                });
            }
            if store_reasoning_content && !reasoning_content.is_empty() {
                content_blocks.push(ContentBlock::Reasoning {
                    text: reasoning_content.clone(),
                });
            }
            for tc in &tool_calls {
                content_blocks.push(ContentBlock::ToolUse {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    input: tc.input.clone(),
                });
            }

            let assistant_message_id = if !content_blocks.is_empty() {
                crate::telemetry::record_assistant_response();
                let token_usage = Some(crate::session::StoredTokenUsage {
                    input_tokens: self.last_usage.input_tokens,
                    output_tokens: self.last_usage.output_tokens,
                    cache_read_input_tokens: self.last_usage.cache_read_input_tokens,
                    cache_creation_input_tokens: self.last_usage.cache_creation_input_tokens,
                });
                let message_id =
                    self.add_message_ext(Role::Assistant, content_blocks, None, token_usage);
                self.push_embedding_snapshot_if_semantic(&text_content);
                self.session.save()?;
                Some(message_id)
            } else {
                None
            };

            if let Some((encrypted_content, compacted_count)) = openai_native_compaction.take() {
                self.apply_openai_native_compaction(encrypted_content, compacted_count)?;
            }

            // If stop_reason indicates truncation (e.g. max_tokens), discard tool calls
            // with null/empty inputs since they were likely truncated mid-generation.
            // This prevents executing broken tool calls and instead requests a continuation.
            self.filter_truncated_tool_calls(
                stop_reason.as_deref(),
                &mut tool_calls,
                assistant_message_id.as_ref(),
            );

            // If no tool calls, we're done
            if tool_calls.is_empty() {
                if self.maybe_continue_incomplete_response(
                    stop_reason.as_deref(),
                    &mut incomplete_continuations,
                )? {
                    continue;
                }
                logging::info("Turn complete - no tool calls, returning");
                if print_output {
                    println!();
                }
                final_text = text_content;
                break;
            }

            logging::info(&format!(
                "Turn has {} tool calls to execute",
                tool_calls.len()
            ));

            // If provider handles tools internally (like Claude Code CLI), only run native tools locally
            if self.provider.handles_tools_internally() {
                tool_calls.retain(|tc| JCODE_NATIVE_TOOLS.contains(&tc.name.as_str()));
                if tool_calls.is_empty() {
                    logging::info("Provider handles tools internally - task complete");
                    break;
                }
                logging::info("Provider handles tools internally - executing native tools locally");
            }

            // Execute tools and add results
            let mut tool_results_dirty = false;
            for tc in tool_calls {
                self.validate_tool_allowed(&tc.name)?;

                let message_id = assistant_message_id
                    .clone()
                    .unwrap_or_else(|| self.session.id.clone());

                let is_native_tool = JCODE_NATIVE_TOOLS.contains(&tc.name.as_str());

                // Check if SDK already executed this tool
                if let Some((sdk_content, sdk_is_error)) = sdk_tool_results.remove(&tc.id) {
                    // For native tools, ignore SDK errors and execute locally
                    if is_native_tool && sdk_is_error {
                        if trace {
                            eprintln!(
                                "[trace] sdk_error_for_native_tool name={} id={}, executing locally",
                                tc.name, tc.id
                            );
                        }
                        // Fall through to local execution below
                    } else {
                        if trace {
                            eprintln!(
                                "[trace] using_sdk_result name={} id={} is_error={}",
                                tc.name, tc.id, sdk_is_error
                            );
                        }
                        if print_output {
                            print!("\n  → ");
                            let preview = if sdk_content.len() > 200 {
                                format!("{}...", crate::util::truncate_str(&sdk_content, 200))
                            } else {
                                sdk_content.clone()
                            };
                            println!("{}", preview.lines().next().unwrap_or("(done via SDK)"));
                        }

                        Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                            session_id: self.session.id.clone(),
                            message_id: message_id.clone(),
                            tool_call_id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                            status: if sdk_is_error {
                                ToolStatus::Error
                            } else {
                                ToolStatus::Completed
                            },
                            title: None,
                        }));

                        self.add_message(
                            Role::User,
                            vec![ContentBlock::ToolResult {
                                tool_use_id: tc.id,
                                content: sdk_content,
                                is_error: if sdk_is_error { Some(true) } else { None },
                            }],
                        );
                        tool_results_dirty = true;
                        continue;
                    }
                }

                // SDK didn't execute this tool, run it locally
                if print_output {
                    print!("\n  → ");
                    io::stdout().flush()?;
                }

                let ctx = ToolContext {
                    session_id: self.session.id.clone(),
                    message_id: message_id.clone(),
                    tool_call_id: tc.id.clone(),
                    working_dir: self.working_dir().map(PathBuf::from),
                    stdin_request_tx: self.stdin_request_tx.clone(),
                    graceful_shutdown_signal: Some(self.graceful_shutdown.clone()),
                    execution_mode: ToolExecutionMode::AgentTurn,
                };

                if trace {
                    eprintln!("[trace] tool_exec_start name={} id={}", tc.name, tc.id);
                }
                Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                    session_id: self.session.id.clone(),
                    message_id: message_id.clone(),
                    tool_call_id: tc.id.clone(),
                    tool_name: tc.name.clone(),
                    status: ToolStatus::Running,
                    title: None,
                }));

                logging::info(&format!("Tool starting: {}", tc.name));
                let tool_start = Instant::now();

                // Publish status for TUI to show during Task execution
                Bus::global().publish(BusEvent::SubagentStatus(SubagentStatus {
                    session_id: self.session.id.clone(),
                    status: format!("running {}", tc.name),
                    model: Some(self.provider.model()),
                }));

                let result = self.registry.execute(&tc.name, tc.input.clone(), ctx).await;
                crate::telemetry::record_tool_call();
                self.unlock_tools_if_needed(&tc.name);
                let tool_elapsed = tool_start.elapsed();
                logging::info(&format!(
                    "Tool finished: {} in {:.2}s",
                    tc.name,
                    tool_elapsed.as_secs_f64()
                ));

                match result {
                    Ok(output) => {
                        Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                            session_id: self.session.id.clone(),
                            message_id: message_id.clone(),
                            tool_call_id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                            status: ToolStatus::Completed,
                            title: output.title.clone(),
                        }));

                        if trace {
                            eprintln!(
                                "[trace] tool_exec_done name={} id={}\n{}",
                                tc.name, tc.id, output.output
                            );
                        }
                        if print_output {
                            let preview = if output.output.len() > 200 {
                                format!("{}...", crate::util::truncate_str(&output.output, 200))
                            } else {
                                output.output.clone()
                            };
                            println!("{}", preview.lines().next().unwrap_or("(done)"));
                        }

                        let blocks = tool_output_to_content_blocks(tc.id, output);
                        self.add_message_with_duration(
                            Role::User,
                            blocks,
                            Some(tool_elapsed.as_millis() as u64),
                        );
                        tool_results_dirty = true;
                    }
                    Err(e) => {
                        crate::telemetry::record_tool_failure();
                        Bus::global().publish(BusEvent::ToolUpdated(ToolEvent {
                            session_id: self.session.id.clone(),
                            message_id: message_id.clone(),
                            tool_call_id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                            status: ToolStatus::Error,
                            title: None,
                        }));

                        let error_msg = format!("Error: {}", e);
                        if trace {
                            eprintln!(
                                "[trace] tool_exec_error name={} id={} {}",
                                tc.name, tc.id, error_msg
                            );
                        }
                        if print_output {
                            println!("{}", error_msg);
                        }
                        self.add_message_with_duration(
                            Role::User,
                            vec![ContentBlock::ToolResult {
                                tool_use_id: tc.id,
                                content: error_msg,
                                is_error: Some(true),
                            }],
                            Some(tool_elapsed.as_millis() as u64),
                        );
                        tool_results_dirty = true;
                    }
                }
            }

            if tool_results_dirty {
                self.session.save()?;
            }

            if print_output {
                println!();
            }

            // Check for soft interrupts (e.g. Telegram messages) and inject them for the next turn
            let injected = self.inject_soft_interrupts();
            if !injected.is_empty() {
                let total_chars: usize = injected.iter().map(|item| item.content.len()).sum();
                logging::info(&format!(
                    "Soft interrupt injected into headless turn ({} message(s), {} chars)",
                    injected.len(),
                    total_chars
                ));
            }
        }

        Ok(final_text)
    }

    /// Run turns with events streamed to a broadcast channel (for server mode)
    async fn run_turn_streaming(&mut self, event_tx: broadcast::Sender<ServerEvent>) -> Result<()> {
        self.set_log_context();
        let trace = trace_enabled();
        let mut context_limit_retries = 0u32;
        let mut incomplete_continuations = 0u32;

        loop {
            let repaired = self.repair_missing_tool_outputs();
            if repaired > 0 {
                logging::warn(&format!(
                    "Recovered {} missing tool output(s) before API call",
                    repaired
                ));
            }
            let (messages, compaction_event) = self.messages_for_provider();
            if let Some(event) = compaction_event {
                // Reset cache tracker and tool lock on compaction since the message history changes
                self.cache_tracker.reset();
                self.locked_tools = None;
                logging::info(&format!(
                    "Context compacted ({}{})",
                    event.trigger,
                    event
                        .pre_tokens
                        .map(|t| format!(" {} tokens", t))
                        .unwrap_or_default()
                ));
                let _ = event_tx.send(ServerEvent::Compaction {
                    trigger: event.trigger.clone(),
                    pre_tokens: event.pre_tokens,
                    post_tokens: event.post_tokens,
                    tokens_saved: event.tokens_saved,
                    duration_ms: event.duration_ms,
                    messages_dropped: None,
                    messages_compacted: event.messages_compacted,
                    summary_chars: event.summary_chars,
                    active_messages: event.active_messages,
                });
            }

            let tools = self.tool_definitions().await;
            // Non-blocking memory: uses pending result from last turn, spawns check for next turn
            let memory_pending = self.build_memory_prompt_nonblocking(&messages);
            // Use split prompt for better caching - static content cached, dynamic not
            let split_prompt = self.build_system_prompt_split(None);

            // Check for client-side cache violations before memory injection.
            // Memory is an ephemeral suffix that changes each turn; tracking it would cause
            // false-positive violations every turn (prior turn's memory ≠ current history prefix).
            if self.should_track_client_cache() {
                if let Some(violation) = self.cache_tracker.record_request(&messages) {
                    logging::warn(&format!(
                        "CLIENT_CACHE_VIOLATION: {} | turn={} messages={}",
                        violation.reason, violation.turn, violation.message_count
                    ));
                }
            }

            // Inject memory as a user message at the end (preserves cache prefix)
            let mut messages_with_memory = messages;
            if let Some(memory) = memory_pending.as_ref() {
                let memory_count = memory.count.max(1);
                let computed_age_ms = memory.computed_at.elapsed().as_millis() as u64;
                crate::memory::record_injected_prompt(
                    &memory.prompt,
                    memory_count,
                    computed_age_ms,
                );
                let _ = event_tx.send(ServerEvent::MemoryInjected {
                    count: memory_count,
                    prompt: memory.prompt.clone(),
                    display_prompt: memory.display_prompt.clone(),
                    prompt_chars: memory.prompt.chars().count(),
                    computed_age_ms,
                });
                let memory_msg =
                    format!("<system-reminder>\n{}\n</system-reminder>", memory.prompt);
                messages_with_memory.push(Message::user(&memory_msg));
            }

            logging::info(&format!(
                "API call starting: {} messages, {} tools",
                messages_with_memory.len(),
                tools.len()
            ));
            let api_start = Instant::now();

            let stamped;
            let send_messages: &[Message] = if crate::config::config().features.message_timestamps {
                stamped = Message::with_timestamps(&messages_with_memory);
                &stamped
            } else {
                &messages_with_memory
            };
            let provider = Arc::clone(&self.provider);
            let resume_session_id = self.provider_session_id.clone();
            let mut keepalive = stream_keepalive_ticker();
            let mut complete_future = std::pin::pin!(provider.complete_split(
                send_messages,
                &tools,
                &split_prompt.static_part,
                &split_prompt.dynamic_part,
                resume_session_id.as_deref(),
            ));
            let mut stream = loop {
                tokio::select! {
                    _ = keepalive.tick() => {
                        send_stream_keepalive_broadcast(&event_tx);
                    }
                    result = &mut complete_future => {
                        match result {
                            Ok(stream) => break stream,
                            Err(e) => {
                                if self.try_auto_compact_after_context_limit(&e.to_string()) {
                                    context_limit_retries += 1;
                                    if context_limit_retries > Self::MAX_CONTEXT_LIMIT_RETRIES {
                                        logging::warn(
                                            "Context-limit compaction retry limit reached; giving up",
                                        );
                                        return Err(anyhow::anyhow!(
                                            "Context limit exceeded after {} compaction retries",
                                            Self::MAX_CONTEXT_LIMIT_RETRIES
                                        ));
                                    }
                                    let _ = event_tx.send(ServerEvent::Compaction {
                                        trigger: "auto_recovery".to_string(),
                                        pre_tokens: None,
                                        post_tokens: None,
                                        tokens_saved: None,
                                        duration_ms: None,
                                        messages_dropped: None,
                                        messages_compacted: None,
                                        summary_chars: None,
                                        active_messages: None,
                                    });
                                    continue;
                                }
                                return Err(e);
                            }
                        }
                    }
                }
            };
            drop(complete_future);

            // Successful API call - reset retry counter
            context_limit_retries = 0;

            logging::info(&format!(
                "API stream opened in {:.2}s",
                api_start.elapsed().as_secs_f64()
            ));

            let mut text_content = String::new();
            let mut text_wrapped_detected = false;
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut current_tool: Option<ToolCall> = None;
            let mut current_tool_input = String::new();
            let mut usage_input: Option<u64> = None;
            let mut usage_output: Option<u64> = None;
            let mut usage_cache_read: Option<u64> = None;
            let mut usage_cache_creation: Option<u64> = None;
            let mut stop_reason: Option<String> = None;
            let mut sdk_tool_results: std::collections::HashMap<String, (String, bool)> =
                std::collections::HashMap::new();
            let store_reasoning_content = self.provider.name() == "openrouter";
            let mut reasoning_content = String::new();
            let mut openai_native_compaction: Option<(String, usize)> = None;
            // Track tool_use_id -> name for tool results
            let mut tool_id_to_name: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();

            let mut retry_after_compaction = false;
            let mut keepalive = stream_keepalive_ticker();
            loop {
                let next_event = std::pin::pin!(stream.next());
                let event = tokio::select! {
                    _ = keepalive.tick() => {
                        send_stream_keepalive_broadcast(&event_tx);
                        continue;
                    }
                    event = next_event => event,
                };
                let Some(event) = event else {
                    break;
                };
                let event = match event {
                    Ok(event) => event,
                    Err(e) => {
                        let err_str = e.to_string();
                        if self.try_auto_compact_after_context_limit(&err_str) {
                            context_limit_retries += 1;
                            if context_limit_retries > Self::MAX_CONTEXT_LIMIT_RETRIES {
                                logging::warn(
                                    "Context-limit compaction retry limit reached; giving up",
                                );
                                return Err(anyhow::anyhow!(
                                    "Context limit exceeded after {} compaction retries",
                                    Self::MAX_CONTEXT_LIMIT_RETRIES
                                ));
                            }
                            retry_after_compaction = true;
                            let _ = event_tx.send(ServerEvent::Compaction {
                                trigger: "auto_recovery".to_string(),
                                pre_tokens: None,
                                post_tokens: None,
                                tokens_saved: None,
                                duration_ms: None,
                                messages_dropped: None,
                                messages_compacted: None,
                                summary_chars: None,
                                active_messages: None,
                            });
                            break;
                        }
                        return Err(e);
                    }
                };

                match event {
                    StreamEvent::ThinkingStart | StreamEvent::ThinkingEnd => {}
                    StreamEvent::ThinkingDelta(thinking_text) => {
                        // Only send thinking content if enabled in config
                        if crate::config::config().display.show_thinking {
                            let _ = event_tx.send(ServerEvent::TextDelta {
                                text: format!("💭 {}\n", thinking_text),
                            });
                        }
                        if store_reasoning_content {
                            reasoning_content.push_str(&thinking_text);
                        }
                    }
                    StreamEvent::ThinkingDone { duration_secs } => {
                        let _ = event_tx.send(ServerEvent::TextDelta {
                            text: format!("Thought for {:.1}s\n", duration_secs),
                        });
                    }
                    StreamEvent::TextDelta(text) => {
                        text_content.push_str(&text);
                        if !text_wrapped_detected {
                            if let Some(marker_idx) = text_content
                                .find("to=functions.")
                                .or_else(|| text_content.find("+#+#"))
                            {
                                text_wrapped_detected = true;
                                let clean_prefix =
                                    text_content[..marker_idx].trim_end().to_string();
                                let _ =
                                    event_tx.send(ServerEvent::TextReplace { text: clean_prefix });
                            } else {
                                let _ =
                                    event_tx.send(ServerEvent::TextDelta { text: text.clone() });
                            }
                        }
                    }
                    StreamEvent::ToolUseStart { id, name } => {
                        let _ = event_tx.send(ServerEvent::ToolStart {
                            id: id.clone(),
                            name: name.clone(),
                        });
                        // Track tool name for later tool_done event
                        tool_id_to_name.insert(id.clone(), name.clone());
                        current_tool = Some(ToolCall {
                            id,
                            name,
                            input: serde_json::Value::Null,
                            intent: None,
                        });
                        current_tool_input.clear();
                    }
                    StreamEvent::ToolInputDelta(delta) => {
                        let _ = event_tx.send(ServerEvent::ToolInput {
                            delta: delta.clone(),
                        });
                        current_tool_input.push_str(&delta);
                    }
                    StreamEvent::ToolUseEnd => {
                        if let Some(mut tool) = current_tool.take() {
                            let tool_input =
                                serde_json::from_str::<serde_json::Value>(&current_tool_input)
                                    .unwrap_or(serde_json::Value::Null);
                            tool.input = tool_input;

                            let _ = event_tx.send(ServerEvent::ToolExec {
                                id: tool.id.clone(),
                                name: tool.name.clone(),
                            });

                            tool_calls.push(tool);
                            current_tool_input.clear();
                        }
                    }
                    StreamEvent::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        // SDK executed tool - send result and store for later
                        let tool_name = tool_id_to_name
                            .get(&tool_use_id)
                            .cloned()
                            .unwrap_or_default();
                        let _ = event_tx.send(ServerEvent::ToolDone {
                            id: tool_use_id.clone(),
                            name: tool_name,
                            output: content.clone(),
                            error: if is_error {
                                Some("Tool error".to_string())
                            } else {
                                None
                            },
                        });
                        sdk_tool_results.insert(tool_use_id, (content, is_error));
                    }
                    StreamEvent::TokenUsage {
                        input_tokens,
                        output_tokens,
                        cache_read_input_tokens,
                        cache_creation_input_tokens,
                    } => {
                        if let Some(input) = input_tokens {
                            usage_input = Some(input);
                        }
                        if let Some(output) = output_tokens {
                            usage_output = Some(output);
                        }
                        if cache_read_input_tokens.is_some() {
                            usage_cache_read = cache_read_input_tokens;
                        }
                        if cache_creation_input_tokens.is_some() {
                            usage_cache_creation = cache_creation_input_tokens;
                        }
                        if let Some(input) = usage_input {
                            self.update_compaction_usage_from_stream(
                                input,
                                usage_cache_read,
                                usage_cache_creation,
                            );
                        }
                    }
                    StreamEvent::ConnectionType { connection } => {
                        crate::telemetry::record_connection_type(&connection);
                        self.last_connection_type = Some(connection.clone());
                        let _ = event_tx.send(ServerEvent::ConnectionType { connection });
                    }
                    StreamEvent::ConnectionPhase { phase } => {
                        let _ = event_tx.send(ServerEvent::ConnectionPhase {
                            phase: phase.to_string(),
                        });
                    }
                    StreamEvent::MessageEnd {
                        stop_reason: reason,
                    } => {
                        if reason.is_some() {
                            stop_reason = reason;
                        }
                    }
                    StreamEvent::SessionId(sid) => {
                        self.provider_session_id = Some(sid.clone());
                        self.session.provider_session_id = Some(sid.clone());
                        let _ = event_tx.send(ServerEvent::SessionId { session_id: sid });
                    }
                    StreamEvent::Compaction {
                        openai_encrypted_content,
                        ..
                    } => {
                        if let Some(encrypted_content) = openai_encrypted_content {
                            openai_native_compaction.get_or_insert_with(|| {
                                (encrypted_content, self.session.messages.len())
                            });
                        }
                    }
                    StreamEvent::NativeToolCall {
                        request_id,
                        tool_name,
                        input,
                    } => {
                        // Execute native tool and send result back to SDK bridge
                        let ctx = ToolContext {
                            session_id: self.session.id.clone(),
                            message_id: self.session.id.clone(),
                            tool_call_id: request_id.clone(),
                            working_dir: self.working_dir().map(PathBuf::from),
                            stdin_request_tx: self.stdin_request_tx.clone(),
                            graceful_shutdown_signal: Some(self.graceful_shutdown.clone()),
                            execution_mode: ToolExecutionMode::AgentTurn,
                        };
                        crate::telemetry::record_tool_call();
                        let tool_result = self.registry.execute(&tool_name, input, ctx).await;
                        if tool_result.is_err() {
                            crate::telemetry::record_tool_failure();
                        }
                        let native_result = match tool_result {
                            Ok(output) => NativeToolResult::success(request_id, output.output),
                            Err(e) => NativeToolResult::error(request_id, e.to_string()),
                        };
                        if let Some(sender) = self.provider.native_result_sender() {
                            let _ = sender.send(native_result).await;
                        }
                    }
                    StreamEvent::UpstreamProvider { provider } => {
                        self.last_upstream_provider = Some(provider.clone());
                        let _ = event_tx.send(ServerEvent::UpstreamProvider { provider });
                    }
                    StreamEvent::Error {
                        message,
                        retry_after_secs,
                    } => {
                        if self.try_auto_compact_after_context_limit(&message) {
                            context_limit_retries += 1;
                            if context_limit_retries > Self::MAX_CONTEXT_LIMIT_RETRIES {
                                logging::warn(
                                    "Context-limit compaction retry limit reached; giving up",
                                );
                                return Err(anyhow::anyhow!(
                                    "Context limit exceeded after {} compaction retries",
                                    Self::MAX_CONTEXT_LIMIT_RETRIES
                                ));
                            }
                            retry_after_compaction = true;
                            let _ = event_tx.send(ServerEvent::Compaction {
                                trigger: "auto_recovery".to_string(),
                                pre_tokens: None,
                                post_tokens: None,
                                tokens_saved: None,
                                duration_ms: None,
                                messages_dropped: None,
                                messages_compacted: None,
                                summary_chars: None,
                                active_messages: None,
                            });
                            break;
                        }
                        return Err(StreamError::new(message, retry_after_secs).into());
                    }
                }
            }

            if retry_after_compaction {
                continue;
            }

            let api_elapsed = api_start.elapsed();
            logging::info(&format!(
                "API call complete in {:.2}s (input={} output={} cache_read={} cache_write={})",
                api_elapsed.as_secs_f64(),
                usage_input.unwrap_or(0),
                usage_output.unwrap_or(0),
                usage_cache_read.unwrap_or(0),
                usage_cache_creation.unwrap_or(0),
            ));

            // Send token usage
            if usage_input.is_some()
                || usage_output.is_some()
                || usage_cache_read.is_some()
                || usage_cache_creation.is_some()
            {
                let _ = event_tx.send(ServerEvent::TokenUsage {
                    input: usage_input.unwrap_or(0),
                    output: usage_output.unwrap_or(0),
                    cache_read_input: usage_cache_read,
                    cache_creation_input: usage_cache_creation,
                });
            }

            // Store usage for debug queries
            self.last_usage = TokenUsage {
                input_tokens: usage_input.unwrap_or(0),
                output_tokens: usage_output.unwrap_or(0),
                cache_read_input_tokens: usage_cache_read,
                cache_creation_input_tokens: usage_cache_creation,
            };

            let had_tool_calls_before = !tool_calls.is_empty();
            self.recover_text_wrapped_tool_call(&mut text_content, &mut tool_calls);

            if !had_tool_calls_before && !tool_calls.is_empty() {
                if let Some(tc) = tool_calls.last() {
                    if tc.id.starts_with("fallback_text_call_") {
                        let _ = event_tx.send(ServerEvent::TextReplace {
                            text: text_content.clone(),
                        });
                        let _ = event_tx.send(ServerEvent::ToolStart {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                        });
                        tool_id_to_name.insert(tc.id.clone(), tc.name.clone());
                        let _ = event_tx.send(ServerEvent::ToolInput {
                            delta: tc.input.to_string(),
                        });
                        let _ = event_tx.send(ServerEvent::ToolExec {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                        });
                    }
                }
            }

            // Add assistant message to history
            let mut content_blocks = Vec::new();
            if !text_content.is_empty() {
                content_blocks.push(ContentBlock::Text {
                    text: text_content.clone(),
                    cache_control: None,
                });
            }
            if store_reasoning_content && !reasoning_content.is_empty() {
                content_blocks.push(ContentBlock::Reasoning {
                    text: reasoning_content.clone(),
                });
            }
            for tc in &tool_calls {
                content_blocks.push(ContentBlock::ToolUse {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    input: tc.input.clone(),
                });
            }

            let assistant_message_id = if !content_blocks.is_empty() {
                crate::telemetry::record_assistant_response();
                let token_usage = Some(crate::session::StoredTokenUsage {
                    input_tokens: self.last_usage.input_tokens,
                    output_tokens: self.last_usage.output_tokens,
                    cache_read_input_tokens: self.last_usage.cache_read_input_tokens,
                    cache_creation_input_tokens: self.last_usage.cache_creation_input_tokens,
                });
                let message_id =
                    self.add_message_ext(Role::Assistant, content_blocks, None, token_usage);
                self.push_embedding_snapshot_if_semantic(&text_content);
                self.session.save()?;
                Some(message_id)
            } else {
                None
            };

            if let Some((encrypted_content, compacted_count)) = openai_native_compaction.take() {
                self.apply_openai_native_compaction(encrypted_content, compacted_count)?;
            }

            // If stop_reason indicates truncation (e.g. max_tokens), discard tool calls
            // with null/empty inputs since they were likely truncated mid-generation.
            self.filter_truncated_tool_calls(
                stop_reason.as_deref(),
                &mut tool_calls,
                assistant_message_id.as_ref(),
            );

            // If no tool calls, check for soft interrupt or exit
            // NOTE: We only inject here (Point B) when there are no tools.
            // Injecting before tool_results would break the API requirement that
            // tool_use must be immediately followed by tool_result.
            if tool_calls.is_empty() {
                match self.handle_streaming_no_tool_calls(
                    stop_reason.as_deref(),
                    &mut incomplete_continuations,
                )? {
                    NoToolCallOutcome::Break => break,
                    NoToolCallOutcome::ContinueWithoutEvent => continue,
                    NoToolCallOutcome::ContinueWithSoftInterrupt { injected, point } => {
                        for event in Self::build_soft_interrupt_events(injected, point, None) {
                            let _ = event_tx.send(event);
                        }
                        continue;
                    }
                }
            }

            logging::info(&format!(
                "Turn has {} tool calls to execute",
                tool_calls.len()
            ));

            // If provider handles tools internally, only run native tools locally
            if self.provider.handles_tools_internally() {
                tool_calls.retain(|tc| JCODE_NATIVE_TOOLS.contains(&tc.name.as_str()));
                if tool_calls.is_empty() {
                    // === INJECTION POINT D: After provider-handled tools, before next API call ===
                    let injected = self.inject_soft_interrupts();
                    if !injected.is_empty() {
                        for event in Self::build_soft_interrupt_events(injected, "D", None) {
                            let _ = event_tx.send(event);
                        }
                        // Don't break - continue loop to process injected message
                        continue;
                    }
                    break;
                }
            }

            // Execute tools and add results
            let tool_count = tool_calls.len();
            let mut tool_results_dirty = false;
            for tool_index in 0..tool_count {
                // === INJECTION POINT C (before): Check for urgent abort before each tool (except first) ===
                if tool_index > 0 && self.has_urgent_interrupt() {
                    // Add tool_results for all remaining skipped tools to maintain valid history
                    for skipped_tc in &tool_calls[tool_index..] {
                        self.add_message(
                            Role::User,
                            vec![ContentBlock::ToolResult {
                                tool_use_id: skipped_tc.id.clone(),
                                content: "[Skipped: user interrupted]".to_string(),
                                is_error: Some(true),
                            }],
                        );
                    }
                    let tools_remaining = tool_count - tool_index;
                    let injected = self.inject_soft_interrupts();
                    if !injected.is_empty() {
                        for event in
                            Self::build_soft_interrupt_events(injected, "C", Some(tools_remaining))
                        {
                            let _ = event_tx.send(event);
                        }
                        // Add note about skipped tools for the AI
                        self.add_message(
                            Role::User,
                            vec![ContentBlock::Text {
                                text: format!(
                                    "[User interrupted: {} remaining tool(s) skipped]",
                                    tools_remaining
                                ),
                                cache_control: None,
                            }],
                        );
                    }
                    let _ = self.session.save();
                    break; // Skip remaining tools
                }
                let tc = &tool_calls[tool_index];

                self.validate_tool_allowed(&tc.name)?;

                let message_id = assistant_message_id
                    .clone()
                    .unwrap_or_else(|| self.session.id.clone());

                let is_native_tool = JCODE_NATIVE_TOOLS.contains(&tc.name.as_str());

                // Check if SDK already executed this tool
                if let Some((sdk_content, sdk_is_error)) = sdk_tool_results.remove(&tc.id) {
                    // For native tools, ignore SDK errors and execute locally
                    if !(is_native_tool && sdk_is_error) {
                        self.add_message(
                            Role::User,
                            vec![ContentBlock::ToolResult {
                                tool_use_id: tc.id.clone(),
                                content: sdk_content,
                                is_error: if sdk_is_error { Some(true) } else { None },
                            }],
                        );
                        tool_results_dirty = true;

                        // NOTE: No injection here - wait for Point D after all tools

                        continue;
                    }
                    // Fall through to local execution for native tools with SDK errors
                }

                // SDK didn't execute this tool (or native tool with SDK error), run it locally
                let ctx = ToolContext {
                    session_id: self.session.id.clone(),
                    message_id: message_id.clone(),
                    tool_call_id: tc.id.clone(),
                    working_dir: self.working_dir().map(PathBuf::from),
                    stdin_request_tx: self.stdin_request_tx.clone(),
                    graceful_shutdown_signal: Some(self.graceful_shutdown.clone()),
                    execution_mode: ToolExecutionMode::AgentTurn,
                };

                if trace {
                    eprintln!("[trace] tool_exec_start name={} id={}", tc.name, tc.id);
                }

                logging::info(&format!("Tool starting: {}", tc.name));
                let tool_start = Instant::now();

                let result = self.registry.execute(&tc.name, tc.input.clone(), ctx).await;
                crate::telemetry::record_tool_call();
                self.unlock_tools_if_needed(&tc.name);
                let tool_elapsed = tool_start.elapsed();
                logging::info(&format!(
                    "Tool finished: {} in {:.2}s",
                    tc.name,
                    tool_elapsed.as_secs_f64()
                ));

                match result {
                    Ok(output) => {
                        let _ = event_tx.send(ServerEvent::ToolDone {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                            output: output.output.clone(),
                            error: None,
                        });

                        let blocks = tool_output_to_content_blocks(tc.id.clone(), output);
                        self.add_message_with_duration(
                            Role::User,
                            blocks,
                            Some(tool_elapsed.as_millis() as u64),
                        );
                        tool_results_dirty = true;
                    }
                    Err(e) => {
                        let error_msg = format!("Error: {}", e);
                        let _ = event_tx.send(ServerEvent::ToolDone {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                            output: error_msg.clone(),
                            error: Some(error_msg.clone()),
                        });

                        self.add_message_with_duration(
                            Role::User,
                            vec![ContentBlock::ToolResult {
                                tool_use_id: tc.id.clone(),
                                content: error_msg,
                                is_error: Some(true),
                            }],
                            Some(tool_elapsed.as_millis() as u64),
                        );
                        tool_results_dirty = true;
                    }
                }

                // NOTE: We do NOT inject between tools (non-urgent) because that would
                // place user text between tool_results, which may violate API constraints.
                // All non-urgent injection happens at Point D after all tools are done.
            }

            if tool_results_dirty {
                self.session.save()?;
            }

            // === INJECTION POINT D: All tools done, before next API call ===
            // This is the safest point for non-urgent injection since all tool_results
            // have been added and the conversation is in a valid state.
            if let PostToolInterruptOutcome::SoftInterrupt { injected, point } =
                self.take_post_tool_soft_interrupt()
            {
                for event in Self::build_soft_interrupt_events(injected, point, None) {
                    let _ = event_tx.send(event);
                }
            }
        }

        Ok(())
    }

    /// Run turns with events streamed to an mpsc channel (for per-client server mode)
    async fn run_turn_streaming_mpsc(
        &mut self,
        event_tx: mpsc::UnboundedSender<ServerEvent>,
    ) -> Result<()> {
        self.set_log_context();
        let trace = trace_enabled();
        let mut context_limit_retries = 0u32;
        let mut incomplete_continuations = 0u32;

        loop {
            let repaired = self.repair_missing_tool_outputs();
            if repaired > 0 {
                logging::warn(&format!(
                    "Recovered {} missing tool output(s) before API call",
                    repaired
                ));
            }
            let (messages, compaction_event) = self.messages_for_provider();
            if let Some(event) = compaction_event {
                // Reset cache tracker and tool lock on compaction since the message history changes
                self.cache_tracker.reset();
                self.locked_tools = None;
                logging::info(&format!(
                    "Context compacted ({}{})",
                    event.trigger,
                    event
                        .pre_tokens
                        .map(|t| format!(" {} tokens", t))
                        .unwrap_or_default()
                ));
                let _ = event_tx.send(ServerEvent::Compaction {
                    trigger: event.trigger.clone(),
                    pre_tokens: event.pre_tokens,
                    post_tokens: event.post_tokens,
                    tokens_saved: event.tokens_saved,
                    duration_ms: event.duration_ms,
                    messages_dropped: None,
                    messages_compacted: event.messages_compacted,
                    summary_chars: event.summary_chars,
                    active_messages: event.active_messages,
                });
            }

            let tools = self.tool_definitions().await;
            // Non-blocking memory: uses pending result from last turn, spawns check for next turn
            let memory_pending = self.build_memory_prompt_nonblocking(&messages);
            // Use split prompt for better caching - static content cached, dynamic not
            let split_prompt = self.build_system_prompt_split(None);

            // Check for client-side cache violations before memory injection.
            // Memory is an ephemeral suffix that changes each turn; tracking it would cause
            // false-positive violations every turn (prior turn's memory ≠ current history prefix).
            if self.should_track_client_cache() {
                if let Some(violation) = self.cache_tracker.record_request(&messages) {
                    logging::warn(&format!(
                        "CLIENT_CACHE_VIOLATION: {} | turn={} messages={}",
                        violation.reason, violation.turn, violation.message_count
                    ));
                }
            }

            // Inject memory as a user message at the end (preserves cache prefix)
            let mut messages_with_memory = messages;
            if let Some(memory) = memory_pending.as_ref() {
                let memory_count = memory.count.max(1);
                let computed_age_ms = memory.computed_at.elapsed().as_millis() as u64;
                crate::memory::record_injected_prompt(
                    &memory.prompt,
                    memory_count,
                    computed_age_ms,
                );
                let _ = event_tx.send(ServerEvent::MemoryInjected {
                    count: memory_count,
                    prompt: memory.prompt.clone(),
                    display_prompt: memory.display_prompt.clone(),
                    prompt_chars: memory.prompt.chars().count(),
                    computed_age_ms,
                });
                let memory_msg =
                    format!("<system-reminder>\n{}\n</system-reminder>", memory.prompt);
                messages_with_memory.push(Message::user(&memory_msg));
            }

            logging::info(&format!(
                "API call starting: {} messages, {} tools",
                messages_with_memory.len(),
                tools.len()
            ));
            let api_start = Instant::now();

            let stamped;
            let send_messages: &[Message] = if crate::config::config().features.message_timestamps {
                stamped = Message::with_timestamps(&messages_with_memory);
                &stamped
            } else {
                &messages_with_memory
            };
            let provider = Arc::clone(&self.provider);
            let resume_session_id = self.provider_session_id.clone();
            let mut keepalive = stream_keepalive_ticker();
            let mut complete_future = std::pin::pin!(provider.complete_split(
                send_messages,
                &tools,
                &split_prompt.static_part,
                &split_prompt.dynamic_part,
                resume_session_id.as_deref(),
            ));
            let mut stream = loop {
                tokio::select! {
                    _ = keepalive.tick() => {
                        send_stream_keepalive_mpsc(&event_tx);
                    }
                    result = &mut complete_future => {
                        match result {
                            Ok(stream) => break stream,
                            Err(e) => {
                                if self.try_auto_compact_after_context_limit(&e.to_string()) {
                                    context_limit_retries += 1;
                                    if context_limit_retries > Self::MAX_CONTEXT_LIMIT_RETRIES {
                                        logging::warn(
                                            "Context-limit compaction retry limit reached; giving up",
                                        );
                                        return Err(anyhow::anyhow!(
                                            "Context limit exceeded after {} compaction retries",
                                            Self::MAX_CONTEXT_LIMIT_RETRIES
                                        ));
                                    }
                                    let _ = event_tx.send(ServerEvent::Compaction {
                                        trigger: "auto_recovery".to_string(),
                                        pre_tokens: None,
                                        post_tokens: None,
                                        tokens_saved: None,
                                        duration_ms: None,
                                        messages_dropped: None,
                                        messages_compacted: None,
                                        summary_chars: None,
                                        active_messages: None,
                                    });
                                    continue;
                                }
                                return Err(e);
                            }
                        }
                    }
                }
            };
            drop(complete_future);

            // Successful API call - reset retry counter
            context_limit_retries = 0;

            logging::info(&format!(
                "API stream opened in {:.2}s",
                api_start.elapsed().as_secs_f64()
            ));

            let mut text_content = String::new();
            let mut text_wrapped_detected = false;
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut current_tool: Option<ToolCall> = None;
            let mut current_tool_input = String::new();
            let mut usage_input: Option<u64> = None;
            let mut usage_output: Option<u64> = None;
            let mut usage_cache_read: Option<u64> = None;
            let mut usage_cache_creation: Option<u64> = None;
            let mut stop_reason: Option<String> = None;
            let mut sdk_tool_results: std::collections::HashMap<String, (String, bool)> =
                std::collections::HashMap::new();
            let store_reasoning_content = self.provider.name() == "openrouter";
            let mut reasoning_content = String::new();
            let mut openai_native_compaction: Option<(String, usize)> = None;
            let mut tool_id_to_name: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();

            let mut retry_after_compaction = false;
            let mut keepalive = stream_keepalive_ticker();
            loop {
                let next_event = std::pin::pin!(stream.next());
                let event = tokio::select! {
                    _ = keepalive.tick() => {
                        send_stream_keepalive_mpsc(&event_tx);
                        continue;
                    }
                    event = next_event => event,
                };
                let Some(event) = event else {
                    break;
                };
                let event = match event {
                    Ok(event) => event,
                    Err(e) => {
                        let err_str = e.to_string();
                        if self.try_auto_compact_after_context_limit(&err_str) {
                            context_limit_retries += 1;
                            if context_limit_retries > Self::MAX_CONTEXT_LIMIT_RETRIES {
                                logging::warn(
                                    "Context-limit compaction retry limit reached; giving up",
                                );
                                return Err(anyhow::anyhow!(
                                    "Context limit exceeded after {} compaction retries",
                                    Self::MAX_CONTEXT_LIMIT_RETRIES
                                ));
                            }
                            retry_after_compaction = true;
                            let _ = event_tx.send(ServerEvent::Compaction {
                                trigger: "auto_recovery".to_string(),
                                pre_tokens: None,
                                post_tokens: None,
                                tokens_saved: None,
                                duration_ms: None,
                                messages_dropped: None,
                                messages_compacted: None,
                                summary_chars: None,
                                active_messages: None,
                            });
                            break;
                        }
                        return Err(e);
                    }
                };

                match event {
                    StreamEvent::ThinkingStart | StreamEvent::ThinkingEnd => {}
                    StreamEvent::ThinkingDelta(thinking_text) => {
                        // Only send thinking content if enabled in config
                        if crate::config::config().display.show_thinking {
                            let _ = event_tx.send(ServerEvent::TextDelta {
                                text: format!("💭 {}\n", thinking_text),
                            });
                        }
                        if store_reasoning_content {
                            reasoning_content.push_str(&thinking_text);
                        }
                    }
                    StreamEvent::ThinkingDone { duration_secs } => {
                        let _ = event_tx.send(ServerEvent::TextDelta {
                            text: format!("Thought for {:.1}s\n", duration_secs),
                        });
                    }
                    StreamEvent::TextDelta(text) => {
                        text_content.push_str(&text);
                        if !text_wrapped_detected {
                            if let Some(marker_idx) = text_content
                                .find("to=functions.")
                                .or_else(|| text_content.find("+#+#"))
                            {
                                text_wrapped_detected = true;
                                let clean_prefix =
                                    text_content[..marker_idx].trim_end().to_string();
                                let _ =
                                    event_tx.send(ServerEvent::TextReplace { text: clean_prefix });
                            } else {
                                let _ =
                                    event_tx.send(ServerEvent::TextDelta { text: text.clone() });
                            }
                        }
                        if self.is_graceful_shutdown() {
                            logging::info(
                                "Graceful shutdown during streaming - checkpointing partial response",
                            );
                            let _ = event_tx.send(ServerEvent::TextDelta {
                                text: "\n\n[generation interrupted - server reloading]".to_string(),
                            });
                            text_content
                                .push_str("\n\n[generation interrupted - server reloading]");
                            break;
                        }
                    }
                    StreamEvent::ToolUseStart { id, name } => {
                        let _ = event_tx.send(ServerEvent::ToolStart {
                            id: id.clone(),
                            name: name.clone(),
                        });
                        tool_id_to_name.insert(id.clone(), name.clone());
                        current_tool = Some(ToolCall {
                            id,
                            name,
                            input: serde_json::Value::Null,
                            intent: None,
                        });
                        current_tool_input.clear();
                    }
                    StreamEvent::ToolInputDelta(delta) => {
                        let _ = event_tx.send(ServerEvent::ToolInput {
                            delta: delta.clone(),
                        });
                        current_tool_input.push_str(&delta);
                    }
                    StreamEvent::ToolUseEnd => {
                        if let Some(mut tool) = current_tool.take() {
                            let tool_input =
                                serde_json::from_str::<serde_json::Value>(&current_tool_input)
                                    .unwrap_or(serde_json::Value::Null);
                            tool.input = tool_input;

                            let _ = event_tx.send(ServerEvent::ToolExec {
                                id: tool.id.clone(),
                                name: tool.name.clone(),
                            });

                            tool_calls.push(tool);
                            current_tool_input.clear();
                        }
                    }
                    StreamEvent::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        let tool_name = tool_id_to_name
                            .get(&tool_use_id)
                            .cloned()
                            .unwrap_or_default();
                        let _ = event_tx.send(ServerEvent::ToolDone {
                            id: tool_use_id.clone(),
                            name: tool_name,
                            output: content.clone(),
                            error: if is_error {
                                Some("Tool error".to_string())
                            } else {
                                None
                            },
                        });
                        sdk_tool_results.insert(tool_use_id, (content, is_error));
                    }
                    StreamEvent::TokenUsage {
                        input_tokens,
                        output_tokens,
                        cache_read_input_tokens,
                        cache_creation_input_tokens,
                    } => {
                        if let Some(input) = input_tokens {
                            usage_input = Some(input);
                        }
                        if let Some(output) = output_tokens {
                            usage_output = Some(output);
                        }
                        if cache_read_input_tokens.is_some() {
                            usage_cache_read = cache_read_input_tokens;
                        }
                        if cache_creation_input_tokens.is_some() {
                            usage_cache_creation = cache_creation_input_tokens;
                        }
                        if let Some(input) = usage_input {
                            self.update_compaction_usage_from_stream(
                                input,
                                usage_cache_read,
                                usage_cache_creation,
                            );
                        }
                    }
                    StreamEvent::ConnectionType { connection } => {
                        crate::telemetry::record_connection_type(&connection);
                        self.last_connection_type = Some(connection.clone());
                        let _ = event_tx.send(ServerEvent::ConnectionType { connection });
                    }
                    StreamEvent::ConnectionPhase { phase } => {
                        let _ = event_tx.send(ServerEvent::ConnectionPhase {
                            phase: phase.to_string(),
                        });
                    }
                    StreamEvent::MessageEnd {
                        stop_reason: reason,
                    } => {
                        if reason.is_some() {
                            stop_reason = reason;
                        }
                    }
                    StreamEvent::SessionId(sid) => {
                        self.provider_session_id = Some(sid.clone());
                        self.session.provider_session_id = Some(sid.clone());
                        let _ = event_tx.send(ServerEvent::SessionId { session_id: sid });
                    }
                    StreamEvent::Compaction {
                        openai_encrypted_content,
                        ..
                    } => {
                        if let Some(encrypted_content) = openai_encrypted_content {
                            openai_native_compaction.get_or_insert_with(|| {
                                (encrypted_content, self.session.messages.len())
                            });
                        }
                    }
                    StreamEvent::NativeToolCall {
                        request_id,
                        tool_name,
                        input,
                    } => {
                        // Execute native tool and send result back to SDK bridge
                        let ctx = ToolContext {
                            session_id: self.session.id.clone(),
                            message_id: self.session.id.clone(),
                            tool_call_id: request_id.clone(),
                            working_dir: self.working_dir().map(PathBuf::from),
                            stdin_request_tx: self.stdin_request_tx.clone(),
                            graceful_shutdown_signal: Some(self.graceful_shutdown.clone()),
                            execution_mode: ToolExecutionMode::AgentTurn,
                        };
                        crate::telemetry::record_tool_call();
                        let tool_result = self.registry.execute(&tool_name, input, ctx).await;
                        if tool_result.is_err() {
                            crate::telemetry::record_tool_failure();
                        }
                        let native_result = match tool_result {
                            Ok(output) => NativeToolResult::success(request_id, output.output),
                            Err(e) => NativeToolResult::error(request_id, e.to_string()),
                        };
                        if let Some(sender) = self.provider.native_result_sender() {
                            let _ = sender.send(native_result).await;
                        }
                    }
                    StreamEvent::UpstreamProvider { provider } => {
                        self.last_upstream_provider = Some(provider.clone());
                        let _ = event_tx.send(ServerEvent::UpstreamProvider { provider });
                    }
                    StreamEvent::Error {
                        message,
                        retry_after_secs,
                    } => {
                        if self.try_auto_compact_after_context_limit(&message) {
                            context_limit_retries += 1;
                            if context_limit_retries > Self::MAX_CONTEXT_LIMIT_RETRIES {
                                logging::warn(
                                    "Context-limit compaction retry limit reached; giving up",
                                );
                                return Err(anyhow::anyhow!(
                                    "Context limit exceeded after {} compaction retries",
                                    Self::MAX_CONTEXT_LIMIT_RETRIES
                                ));
                            }
                            retry_after_compaction = true;
                            let _ = event_tx.send(ServerEvent::Compaction {
                                trigger: "auto_recovery".to_string(),
                                pre_tokens: None,
                                post_tokens: None,
                                tokens_saved: None,
                                duration_ms: None,
                                messages_dropped: None,
                                messages_compacted: None,
                                summary_chars: None,
                                active_messages: None,
                            });
                            break;
                        }
                        return Err(StreamError::new(message, retry_after_secs).into());
                    }
                }
            }

            if retry_after_compaction {
                continue;
            }

            let api_elapsed = api_start.elapsed();
            logging::info(&format!(
                "API call complete in {:.2}s (input={} output={} cache_read={} cache_write={})",
                api_elapsed.as_secs_f64(),
                usage_input.unwrap_or(0),
                usage_output.unwrap_or(0),
                usage_cache_read.unwrap_or(0),
                usage_cache_creation.unwrap_or(0),
            ));

            if usage_input.is_some()
                || usage_output.is_some()
                || usage_cache_read.is_some()
                || usage_cache_creation.is_some()
            {
                let _ = event_tx.send(ServerEvent::TokenUsage {
                    input: usage_input.unwrap_or(0),
                    output: usage_output.unwrap_or(0),
                    cache_read_input: usage_cache_read,
                    cache_creation_input: usage_cache_creation,
                });
            }

            // Store usage for debug queries
            self.last_usage = TokenUsage {
                input_tokens: usage_input.unwrap_or(0),
                output_tokens: usage_output.unwrap_or(0),
                cache_read_input_tokens: usage_cache_read,
                cache_creation_input_tokens: usage_cache_creation,
            };

            let had_tool_calls_before = !tool_calls.is_empty();
            self.recover_text_wrapped_tool_call(&mut text_content, &mut tool_calls);

            if !had_tool_calls_before && !tool_calls.is_empty() {
                if let Some(tc) = tool_calls.last() {
                    if tc.id.starts_with("fallback_text_call_") {
                        let _ = event_tx.send(ServerEvent::TextReplace {
                            text: text_content.clone(),
                        });
                        let _ = event_tx.send(ServerEvent::ToolStart {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                        });
                        tool_id_to_name.insert(tc.id.clone(), tc.name.clone());
                        let _ = event_tx.send(ServerEvent::ToolInput {
                            delta: tc.input.to_string(),
                        });
                        let _ = event_tx.send(ServerEvent::ToolExec {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                        });
                    }
                }
            }

            // Add assistant message to history
            let mut content_blocks = Vec::new();
            if !text_content.is_empty() {
                content_blocks.push(ContentBlock::Text {
                    text: text_content.clone(),
                    cache_control: None,
                });
            }
            if store_reasoning_content && !reasoning_content.is_empty() {
                content_blocks.push(ContentBlock::Reasoning {
                    text: reasoning_content.clone(),
                });
            }
            for tc in &tool_calls {
                content_blocks.push(ContentBlock::ToolUse {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    input: tc.input.clone(),
                });
            }

            let assistant_message_id = if !content_blocks.is_empty() {
                crate::telemetry::record_assistant_response();
                let token_usage = Some(crate::session::StoredTokenUsage {
                    input_tokens: self.last_usage.input_tokens,
                    output_tokens: self.last_usage.output_tokens,
                    cache_read_input_tokens: self.last_usage.cache_read_input_tokens,
                    cache_creation_input_tokens: self.last_usage.cache_creation_input_tokens,
                });
                let message_id =
                    self.add_message_ext(Role::Assistant, content_blocks, None, token_usage);
                self.push_embedding_snapshot_if_semantic(&text_content);
                self.session.save()?;
                Some(message_id)
            } else {
                None
            };

            if let Some((encrypted_content, compacted_count)) = openai_native_compaction.take() {
                self.apply_openai_native_compaction(encrypted_content, compacted_count)?;
            }

            // If stop_reason indicates truncation (e.g. max_tokens), discard tool calls
            // with null/empty inputs since they were likely truncated mid-generation.
            self.filter_truncated_tool_calls(
                stop_reason.as_deref(),
                &mut tool_calls,
                assistant_message_id.as_ref(),
            );

            // If no tool calls, check for soft interrupt or exit
            // NOTE: We only inject here (Point B) when there are no tools.
            // Injecting before tool_results would break the API requirement that
            // tool_use must be immediately followed by tool_result.
            if tool_calls.is_empty() {
                match self.handle_streaming_no_tool_calls(
                    stop_reason.as_deref(),
                    &mut incomplete_continuations,
                )? {
                    NoToolCallOutcome::Break => break,
                    NoToolCallOutcome::ContinueWithoutEvent => continue,
                    NoToolCallOutcome::ContinueWithSoftInterrupt { injected, point } => {
                        for event in Self::build_soft_interrupt_events(injected, point, None) {
                            let _ = event_tx.send(event);
                        }
                        continue;
                    }
                }
            }

            // If graceful shutdown was signaled during streaming and we have tool calls,
            // we need to provide tool results for them (API requires tool_use -> tool_result)
            // then exit cleanly
            if self.is_graceful_shutdown() {
                logging::info(&format!(
                    "Graceful shutdown - skipping {} tool call(s)",
                    tool_calls.len()
                ));
                for tc in &tool_calls {
                    self.add_message(
                        Role::User,
                        vec![ContentBlock::ToolResult {
                            tool_use_id: tc.id.clone(),
                            content: "[Skipped - server reloading]".to_string(),
                            is_error: Some(true),
                        }],
                    );
                }
                self.session.save()?;
                break;
            }

            logging::info(&format!(
                "Turn has {} tool calls to execute",
                tool_calls.len()
            ));

            if self.provider.handles_tools_internally() {
                tool_calls.retain(|tc| JCODE_NATIVE_TOOLS.contains(&tc.name.as_str()));
                if tool_calls.is_empty() {
                    // === INJECTION POINT D: After provider-handled tools, before next API call ===
                    let injected = self.inject_soft_interrupts();
                    if !injected.is_empty() {
                        for event in Self::build_soft_interrupt_events(injected, "D", None) {
                            let _ = event_tx.send(event);
                        }
                        // Don't break - continue loop to process injected message
                        continue;
                    }
                    break;
                }
            }

            // Execute tools and add results
            let tool_count = tool_calls.len();
            let mut tool_results_dirty = false;
            for tool_index in 0..tool_count {
                // === INJECTION POINT C (before): Check for urgent abort before each tool (except first) ===
                if tool_index > 0 && self.has_urgent_interrupt() {
                    // Add tool_results for all remaining skipped tools to maintain valid history
                    for skipped_tc in &tool_calls[tool_index..] {
                        self.add_message(
                            Role::User,
                            vec![ContentBlock::ToolResult {
                                tool_use_id: skipped_tc.id.clone(),
                                content: "[Skipped: user interrupted]".to_string(),
                                is_error: Some(true),
                            }],
                        );
                    }
                    let tools_remaining = tool_count - tool_index;
                    let injected = self.inject_soft_interrupts();
                    if !injected.is_empty() {
                        for event in
                            Self::build_soft_interrupt_events(injected, "C", Some(tools_remaining))
                        {
                            let _ = event_tx.send(event);
                        }
                        // Add note about skipped tools for the AI
                        self.add_message(
                            Role::User,
                            vec![ContentBlock::Text {
                                text: format!(
                                    "[User interrupted: {} remaining tool(s) skipped]",
                                    tools_remaining
                                ),
                                cache_control: None,
                            }],
                        );
                    }
                    let _ = self.session.save();
                    break; // Skip remaining tools
                }
                let tc = &tool_calls[tool_index];

                self.validate_tool_allowed(&tc.name)?;

                let message_id = assistant_message_id
                    .clone()
                    .unwrap_or_else(|| self.session.id.clone());

                let is_native_tool = JCODE_NATIVE_TOOLS.contains(&tc.name.as_str());

                if let Some((sdk_content, sdk_is_error)) = sdk_tool_results.remove(&tc.id) {
                    // For native tools, ignore SDK errors and execute locally
                    if !(is_native_tool && sdk_is_error) {
                        self.add_message(
                            Role::User,
                            vec![ContentBlock::ToolResult {
                                tool_use_id: tc.id.clone(),
                                content: sdk_content,
                                is_error: if sdk_is_error { Some(true) } else { None },
                            }],
                        );
                        tool_results_dirty = true;

                        // NOTE: No injection here - wait for Point D after all tools

                        continue;
                    }
                    // Fall through to local execution for native tools with SDK errors
                }

                let ctx = ToolContext {
                    session_id: self.session.id.clone(),
                    message_id: message_id.clone(),
                    tool_call_id: tc.id.clone(),
                    working_dir: self.working_dir().map(PathBuf::from),
                    stdin_request_tx: self.stdin_request_tx.clone(),
                    graceful_shutdown_signal: Some(self.graceful_shutdown.clone()),
                    execution_mode: ToolExecutionMode::AgentTurn,
                };

                if trace {
                    eprintln!("[trace] tool_exec_start name={} id={}", tc.name, tc.id);
                }

                logging::info(&format!("Tool starting: {}", tc.name));
                let tool_start = Instant::now();

                // Spawn tool in its own task so we can detach it to background on Alt+B
                let registry_clone = self.registry.clone();
                let tool_name_for_spawn = tc.name.clone();
                let tool_input_for_spawn = tc.input.clone();
                let tool_handle = tokio::spawn(async move {
                    registry_clone
                        .execute(&tool_name_for_spawn, tool_input_for_spawn, ctx)
                        .await
                });

                // Reset background signal before waiting
                self.background_tool_signal.reset();

                // Wait for tool completion OR background signal from user (Alt+B)
                // OR graceful shutdown signal from server reload
                let bg_signal = self.background_tool_signal.clone();
                let shutdown_signal = self.graceful_shutdown.clone();
                let allow_reload_handoff = tc.name == "bash";
                let tool_result;
                let mut tool_handle = tool_handle;
                tokio::select! {
                    biased;
                    res = &mut tool_handle => {
                        tool_result = Some(match res {
                            Ok(r) => r,
                            Err(e) => Err(anyhow::anyhow!("Tool task panicked: {}", e)),
                        });
                    }
                    _ = async {
                        tokio::select! {
                            _ = bg_signal.notified() => {}
                            _ = shutdown_signal.notified() => {}
                        }
                    } => {
                        if self.is_graceful_shutdown() && allow_reload_handoff {
                            tool_result = match tokio::time::timeout(
                                Duration::from_millis(750),
                                &mut tool_handle,
                            )
                            .await
                            {
                                Ok(res) => Some(match res {
                                    Ok(r) => r,
                                    Err(e) => Err(anyhow::anyhow!("Tool task panicked: {}", e)),
                                }),
                                Err(_) => None,
                            };
                        } else {
                            tool_result = None;
                        }
                    }
                };

                self.unlock_tools_if_needed(&tc.name);
                let tool_elapsed = tool_start.elapsed();

                if let Some(result) = tool_result {
                    // Normal tool completion
                    logging::info(&format!(
                        "Tool finished: {} in {:.2}s",
                        tc.name,
                        tool_elapsed.as_secs_f64()
                    ));

                    match result {
                        Ok(output) => {
                            let _ = event_tx.send(ServerEvent::ToolDone {
                                id: tc.id.clone(),
                                name: tc.name.clone(),
                                output: output.output.clone(),
                                error: None,
                            });

                            let blocks = tool_output_to_content_blocks(tc.id.clone(), output);
                            self.add_message_with_duration(
                                Role::User,
                                blocks,
                                Some(tool_elapsed.as_millis() as u64),
                            );
                            tool_results_dirty = true;
                        }
                        Err(e) => {
                            let error_msg = format!("Error: {}", e);
                            let _ = event_tx.send(ServerEvent::ToolDone {
                                id: tc.id.clone(),
                                name: tc.name.clone(),
                                output: error_msg.clone(),
                                error: Some(error_msg.clone()),
                            });

                            self.add_message_with_duration(
                                Role::User,
                                vec![ContentBlock::ToolResult {
                                    tool_use_id: tc.id.clone(),
                                    content: error_msg,
                                    is_error: Some(true),
                                }],
                                Some(tool_elapsed.as_millis() as u64),
                            );
                            tool_results_dirty = true;
                        }
                    }
                } else if self.is_graceful_shutdown() {
                    // Server reload - abort tool and save interrupted result
                    logging::info(&format!(
                        "Tool '{}' interrupted by server reload after {:.1}s",
                        tc.name,
                        tool_elapsed.as_secs_f64()
                    ));
                    tool_handle.abort();

                    // For selfdev reload, the interruption is intentional -
                    // the tool triggered the reload and blocked waiting for shutdown.
                    // Use a non-error message so the conversation history is clean.
                    let is_selfdev_reload = tc.name == "selfdev";
                    let interrupted_msg = if is_selfdev_reload {
                        "Reload initiated. Process restarting...".to_string()
                    } else {
                        format!(
                            "[Tool '{}' interrupted by server reload after {:.1}s]",
                            tc.name,
                            tool_elapsed.as_secs_f64()
                        )
                    };

                    let _ = event_tx.send(ServerEvent::ToolDone {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        output: interrupted_msg.clone(),
                        error: if is_selfdev_reload {
                            None
                        } else {
                            Some("interrupted by reload".to_string())
                        },
                    });

                    self.add_message_with_duration(
                        Role::User,
                        vec![ContentBlock::ToolResult {
                            tool_use_id: tc.id.clone(),
                            content: interrupted_msg,
                            is_error: Some(!is_selfdev_reload),
                        }],
                        Some(tool_elapsed.as_millis() as u64),
                    );
                    self.session.save()?;

                    // Add results for any remaining tools too
                    for remaining_tc in &tool_calls[(tool_index + 1)..] {
                        self.add_message(
                            Role::User,
                            vec![ContentBlock::ToolResult {
                                tool_use_id: remaining_tc.id.clone(),
                                content: "[Skipped - server reloading]".to_string(),
                                is_error: Some(true),
                            }],
                        );
                    }
                    self.session.save()?;
                    return Ok(());
                } else {
                    // User pressed Alt+B — move tool to background
                    logging::info(&format!(
                        "Tool '{}' moved to background after {:.1}s",
                        tc.name,
                        tool_elapsed.as_secs_f64()
                    ));

                    let bg_info = crate::background::global()
                        .adopt(&tc.name, &self.session.id, tool_handle)
                        .await;

                    let bg_msg = format!(
                        "Tool '{}' was moved to background by the user (task_id: {}). \
                         Use the `bg` tool with action 'status' or 'output' to check on it.",
                        tc.name, bg_info.task_id
                    );

                    let _ = event_tx.send(ServerEvent::ToolDone {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        output: bg_msg.clone(),
                        error: None,
                    });

                    self.add_message_with_duration(
                        Role::User,
                        vec![ContentBlock::ToolResult {
                            tool_use_id: tc.id.clone(),
                            content: bg_msg,
                            is_error: None,
                        }],
                        Some(tool_elapsed.as_millis() as u64),
                    );
                    self.session.save()?;

                    self.background_tool_signal.reset();
                }

                // NOTE: We do NOT inject between tools (non-urgent) because that would
                // place user text between tool_results, which may violate API constraints.
                // All non-urgent injection happens at Point D after all tools are done.
            }

            if tool_results_dirty {
                self.session.save()?;
            }

            // === INJECTION POINT D: All tools done, before next API call ===
            // This is the safest point for non-urgent injection since all tool_results
            // have been added and the conversation is in a valid state.
            if let PostToolInterruptOutcome::SoftInterrupt { injected, point } =
                self.take_post_tool_soft_interrupt()
            {
                for event in Self::build_soft_interrupt_events(injected, point, None) {
                    let _ = event_tx.send(event);
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{Message, StreamEvent, ToolDefinition};
    use crate::provider::{EventStream, Provider};
    use crate::tool::Registry;
    use crate::tool::ToolOutput;
    use async_trait::async_trait;
    use tokio::sync::mpsc as tokio_mpsc;
    use tokio_stream::wrappers::ReceiverStream;

    struct DelayedProvider {
        open_delay: Duration,
        first_event_delay: Duration,
    }

    struct NativeAutoCompactionProvider;

    #[async_trait]
    impl Provider for DelayedProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _system: &str,
            _resume_session_id: Option<&str>,
        ) -> Result<EventStream> {
            tokio::time::sleep(self.open_delay).await;

            let first_event_delay = self.first_event_delay;
            let (tx, rx) = tokio_mpsc::channel::<Result<StreamEvent>>(8);
            tokio::spawn(async move {
                tokio::time::sleep(first_event_delay).await;
                let _ = tx
                    .send(Ok(StreamEvent::TextDelta("hello".to_string())))
                    .await;
                let _ = tx
                    .send(Ok(StreamEvent::MessageEnd {
                        stop_reason: Some("end_turn".to_string()),
                    }))
                    .await;
            });

            Ok(Box::pin(ReceiverStream::new(rx)))
        }

        fn name(&self) -> &str {
            "delayed"
        }

        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(Self {
                open_delay: self.open_delay,
                first_event_delay: self.first_event_delay,
            })
        }
    }

    #[async_trait]
    impl Provider for NativeAutoCompactionProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _system: &str,
            _resume_session_id: Option<&str>,
        ) -> Result<EventStream> {
            let (_tx, rx) = tokio_mpsc::channel::<Result<StreamEvent>>(1);
            Ok(Box::pin(ReceiverStream::new(rx)))
        }

        fn name(&self) -> &str {
            "openai"
        }

        fn supports_compaction(&self) -> bool {
            true
        }

        fn uses_jcode_compaction(&self) -> bool {
            false
        }

        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(Self)
        }
    }

    #[test]
    fn tool_output_to_content_blocks_preserves_labeled_images() {
        let output = ToolOutput::new("Image ready").with_labeled_image(
            "image/png",
            "ZmFrZQ==",
            "screenshots/example.png",
        );

        let blocks = tool_output_to_content_blocks("call_1".to_string(), output);
        assert_eq!(blocks.len(), 3);

        match &blocks[0] {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "call_1");
                assert_eq!(content, "Image ready");
                assert_eq!(*is_error, None);
            }
            other => panic!("expected tool result, got {other:?}"),
        }

        match &blocks[1] {
            ContentBlock::Image { media_type, data } => {
                assert_eq!(media_type, "image/png");
                assert_eq!(data, "ZmFrZQ==");
            }
            other => panic!("expected image block, got {other:?}"),
        }

        match &blocks[2] {
            ContentBlock::Text { text, .. } => {
                assert!(text.contains("screenshots/example.png"));
                assert!(text.contains("preceding tool result"));
            }
            other => panic!("expected trailing label text, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_turn_streaming_mpsc_emits_keepalive_while_provider_is_quiet() {
        let _guard = crate::storage::lock_test_env();
        let provider: Arc<dyn Provider> = Arc::new(DelayedProvider {
            open_delay: Duration::from_secs(2),
            first_event_delay: Duration::from_secs(2),
        });
        let registry = Registry::new(provider.clone()).await;
        let mut agent = Agent::new(provider, registry);
        agent.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "test".to_string(),
                cache_control: None,
            }],
        );

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(async move { agent.run_turn_streaming_mpsc(tx).await });

        let mut saw_keepalive = false;
        let keepalive_deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < keepalive_deadline {
            match tokio::time::timeout(Duration::from_secs(1), rx.recv()).await {
                Ok(Some(ServerEvent::Pong { id })) => {
                    assert_eq!(id, STREAM_KEEPALIVE_PONG_ID);
                    saw_keepalive = true;
                    break;
                }
                Ok(Some(ServerEvent::TextDelta { text })) => {
                    panic!("expected keepalive before text delta, got: {text}");
                }
                Ok(Some(_)) => {}
                Ok(None) => panic!("channel closed before keepalive"),
                Err(_) => {
                    assert!(
                        !task.is_finished(),
                        "streaming task finished before keepalive arrived"
                    );
                }
            }
        }
        assert!(saw_keepalive, "expected keepalive before provider response");

        let mut saw_text = false;
        let text_deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < text_deadline {
            match tokio::time::timeout(Duration::from_secs(1), rx.recv()).await {
                Ok(Some(ServerEvent::TextDelta { text })) => {
                    assert_eq!(text, "hello");
                    saw_text = true;
                    break;
                }
                Ok(Some(ServerEvent::Pong { id })) => {
                    assert_eq!(id, STREAM_KEEPALIVE_PONG_ID);
                }
                Ok(Some(_)) => {}
                Ok(None) => panic!("channel closed before text delta"),
                Err(_) => {
                    assert!(
                        !task.is_finished(),
                        "streaming task finished before text delta arrived"
                    );
                }
            }
        }

        assert!(saw_text, "expected delayed provider text after keepalive");
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn messages_for_provider_replays_persisted_native_compaction_in_auto_mode() {
        let provider: Arc<dyn Provider> = Arc::new(NativeAutoCompactionProvider);
        let registry = Registry::new(provider.clone()).await;
        let mut agent = Agent::new(provider, registry);

        agent.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "first".to_string(),
                cache_control: None,
            }],
        );
        agent.add_message(
            Role::Assistant,
            vec![ContentBlock::Text {
                text: "second".to_string(),
                cache_control: None,
            }],
        );

        agent
            .apply_openai_native_compaction("enc_auto".to_string(), 1)
            .expect("persist native compaction");

        let (messages, event) = agent.messages_for_provider();
        assert!(event.is_none());
        assert_eq!(messages.len(), 2);
        match &messages[0].content[0] {
            ContentBlock::OpenAICompaction { encrypted_content } => {
                assert_eq!(encrypted_content, "enc_auto");
            }
            other => panic!("expected OpenAI compaction block, got {other:?}"),
        }
        assert_eq!(messages[1].role, Role::Assistant);
    }

    // ── InterruptSignal tests ────────────────────────────────────────────────

    #[tokio::test]
    async fn interrupt_signal_fire_before_notified_does_not_hang() {
        // Regression test: fire() called BEFORE notified().await must not hang.
        // The old code called notify_waiters() which drops the notification if
        // nobody is waiting yet. The flag is still set so the fast path catches it,
        // but only if the future is created before the flag check.
        let sig = InterruptSignal::new();
        sig.fire(); // fire before anyone is waiting
        tokio::time::timeout(std::time::Duration::from_millis(100), sig.notified())
            .await
            .expect("notified() hung when signal was already set before call");
    }

    #[tokio::test]
    async fn interrupt_signal_fire_concurrent_with_notified() {
        // Regression test for the race window: fire() is called concurrently while
        // notified() is being set up. The fix (create future before flag check) ensures
        // the notify_waiters() in fire() wakes the registered future.
        let sig = Arc::new(InterruptSignal::new());
        let sig2 = Arc::clone(&sig);

        // Spawn a task that fires after a tiny delay, giving the main task time to
        // enter notified() but before it reaches notified().await.
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            sig2.fire();
        });

        tokio::time::timeout(std::time::Duration::from_millis(500), sig.notified())
            .await
            .expect("notified() hung during concurrent fire()");
    }

    #[tokio::test]
    async fn interrupt_signal_is_set_false_initially() {
        let sig = InterruptSignal::new();
        assert!(!sig.is_set());
    }

    #[tokio::test]
    async fn interrupt_signal_is_set_true_after_fire() {
        let sig = InterruptSignal::new();
        sig.fire();
        assert!(sig.is_set());
    }

    #[tokio::test]
    async fn interrupt_signal_reset_clears_flag() {
        let sig = InterruptSignal::new();
        sig.fire();
        assert!(sig.is_set());
        sig.reset();
        assert!(!sig.is_set());
    }

    #[tokio::test]
    async fn interrupt_signal_notified_completes_after_fire() {
        let sig = Arc::new(InterruptSignal::new());
        let sig2 = Arc::clone(&sig);

        let handle = tokio::spawn(async move {
            sig2.notified().await;
        });

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        sig.fire();

        tokio::time::timeout(std::time::Duration::from_millis(200), handle)
            .await
            .expect("notified() task timed out after fire()")
            .expect("task panicked");
    }

    #[tokio::test]
    async fn new_agent_registers_active_pid_and_clear_swaps_it() {
        let _guard = crate::storage::lock_test_env();
        let provider: Arc<dyn Provider> = Arc::new(NativeAutoCompactionProvider);
        let registry = Registry::new(provider.clone()).await;
        let mut agent = Agent::new(provider, registry);

        let first_session_id = agent.session_id().to_string();
        assert!(
            crate::session::active_session_ids().contains(&first_session_id),
            "fresh agent session should be tracked as active"
        );

        agent.clear();

        let second_session_id = agent.session_id().to_string();
        let active = crate::session::active_session_ids();
        assert_ne!(first_session_id, second_session_id);
        assert!(
            active.contains(&second_session_id),
            "replacement session should be tracked as active"
        );
        assert!(
            !active.contains(&first_session_id),
            "cleared session should no longer be tracked as active"
        );
    }
}
