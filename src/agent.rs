#![allow(unused_assignments)]
#![allow(unused_assignments)]

use crate::build;
use crate::bus::{Bus, BusEvent, SubagentStatus, ToolEvent, ToolStatus};
use crate::cache_tracker::CacheTracker;
use crate::compaction::CompactionEvent;
use crate::id;
use crate::logging;
use crate::message::{
    ContentBlock, Message, Role, StreamEvent, ToolCall, ToolDefinition, TOOL_OUTPUT_MISSING_TEXT,
};
use crate::protocol::{HistoryMessage, ServerEvent};
use crate::provider::{NativeToolResult, Provider};
use crate::session::{EnvSnapshot, GitState, Session, StoredMessage};
use crate::skill::SkillRegistry;
use crate::tool::{Registry, ToolContext};
use anyhow::Result;
use chrono::Utc;
use futures::StreamExt;
use std::collections::HashSet;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{broadcast, mpsc};

const JCODE_NATIVE_TOOLS: &[&str] = &["selfdev", "communicate"];

/// A soft interrupt message queued for injection at the next safe point
#[derive(Debug, Clone)]
pub struct SoftInterruptMessage {
    pub content: String,
    /// If true, can skip remaining tools when injected at point C
    pub urgent: bool,
}

/// Thread-safe soft interrupt queue that can be accessed without holding the agent lock
pub type SoftInterruptQueue = Arc<std::sync::Mutex<Vec<SoftInterruptMessage>>>;

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
    /// Pending swarm alerts to inject into the next turn
    pending_alerts: Vec<String>,
    /// Soft interrupt queue: messages to inject at next safe point without cancelling
    /// Uses std::sync::Mutex so it can be accessed without async, even while agent is processing
    soft_interrupt_queue: SoftInterruptQueue,
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
}

impl Agent {
    pub fn new(provider: Arc<dyn Provider>, registry: Registry) -> Self {
        let skills = SkillRegistry::load().unwrap_or_default();
        let mut agent = Self {
            provider,
            registry,
            skills,
            session: Session::create(None, None),
            active_skill: None,
            allowed_tools: None,
            provider_session_id: None,
            last_upstream_provider: None,
            pending_alerts: Vec::new(),
            soft_interrupt_queue: Arc::new(std::sync::Mutex::new(Vec::new())),
            cache_tracker: CacheTracker::new(),
            last_usage: TokenUsage::default(),
            locked_tools: None,
            system_prompt_override: None,
            memory_enabled: crate::config::config().features.memory,
        };
        agent.session.model = Some(agent.provider.model());
        agent.seed_compaction_from_session();
        agent.log_env_snapshot("create");
        agent
    }

    pub fn new_with_session(
        provider: Arc<dyn Provider>,
        registry: Registry,
        session: Session,
        allowed_tools: Option<HashSet<String>>,
    ) -> Self {
        let skills = SkillRegistry::load().unwrap_or_default();
        let mut agent = Self {
            provider,
            registry,
            skills,
            session,
            active_skill: None,
            allowed_tools,
            provider_session_id: None,
            last_upstream_provider: None,
            pending_alerts: Vec::new(),
            soft_interrupt_queue: Arc::new(std::sync::Mutex::new(Vec::new())),
            cache_tracker: CacheTracker::new(),
            last_usage: TokenUsage::default(),
            locked_tools: None,
            system_prompt_override: None,
            memory_enabled: crate::config::config().features.memory,
        };
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
        for msg in &self.session.messages {
            manager.add_message(msg.to_message());
        }
        logging::info(&format!(
            "seed_compaction_from_session: seeded compaction with {} messages",
            self.session.messages.len()
        ));
    }

    fn add_message(&mut self, role: Role, content: Vec<ContentBlock>) -> String {
        let id = self.session.add_message(role.clone(), content.clone());
        let message = Message { role, content, timestamp: Some(chrono::Utc::now()) };
        let compaction = self.registry.compaction();
        if let Ok(mut manager) = compaction.try_write() {
            manager.add_message(message);
        }
        id
    }

    fn messages_for_provider(&mut self) -> (Vec<Message>, Option<CompactionEvent>) {
        if self.provider.supports_compaction() {
            let compaction = self.registry.compaction();
            match compaction.try_write() {
                Ok(mut manager) => {
                    manager.maybe_start_compaction(self.provider.clone());
                    let messages = manager.messages_for_api();
                    let event = manager.take_compaction_event();
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
        let messages = self.session.messages_for_provider();
        logging::info(&format!(
            "messages_for_provider (session): returning {} messages, roles: {:?}",
            messages.len(),
            messages
                .iter()
                .map(|m| format!("{:?}", m.role))
                .collect::<Vec<_>>()
        ));
        (messages, None)
    }

    fn effective_context_tokens_from_usage(
        &self,
        input_tokens: u64,
        cache_read_input_tokens: Option<u64>,
        cache_creation_input_tokens: Option<u64>,
    ) -> u64 {
        if input_tokens == 0 {
            return 0;
        }
        let cache_read = cache_read_input_tokens.unwrap_or(0);
        let cache_creation = cache_creation_input_tokens.unwrap_or(0);
        let provider_name = self.provider.name().to_lowercase();

        let split_cache_accounting = provider_name.contains("anthropic")
            || provider_name.contains("claude")
            || cache_creation > 0
            || cache_read > input_tokens;

        if split_cache_accounting {
            input_tokens
                .saturating_add(cache_read)
                .saturating_add(cache_creation)
        } else {
            input_tokens
        }
    }

    fn update_compaction_usage_from_stream(
        &mut self,
        input_tokens: u64,
        cache_read_input_tokens: Option<u64>,
        cache_creation_input_tokens: Option<u64>,
    ) {
        if !self.provider.supports_compaction() || input_tokens == 0 {
            return;
        }
        let observed = self.effective_context_tokens_from_usage(
            input_tokens,
            cache_read_input_tokens,
            cache_creation_input_tokens,
        );
        let compaction = self.registry.compaction();
        if let Ok(mut manager) = compaction.try_write() {
            manager.update_observed_input_tokens(observed);
        };
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
                    };
                    self.session
                        .messages
                        .insert(index + 1 + offset, stored_message);
                    repaired += 1;
                }
                index += missing_for_message.len();
            }

            index += 1;
        }

        if repaired > 0 {
            let _ = self.session.save();
            self.seed_compaction_from_session();
            self.cache_tracker.reset();
            self.locked_tools = None;
        }

        repaired
    }

    /// Add a swarm alert to be injected into the next turn
    pub fn push_alert(&mut self, alert: String) {
        self.pending_alerts.push(alert);
    }

    /// Take all pending alerts (clears the queue)
    pub fn take_alerts(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_alerts)
    }

    /// Queue a soft interrupt message to be injected at the next safe point.
    /// This method can be called even while the agent is processing (uses separate lock).
    pub fn queue_soft_interrupt(&self, content: String, urgent: bool) {
        if let Ok(mut queue) = self.soft_interrupt_queue.lock() {
            queue.push(SoftInterruptMessage { content, urgent });
        }
    }

    /// Get a handle to the soft interrupt queue.
    /// The server can use this to queue interrupts without holding the agent lock.
    pub fn soft_interrupt_queue(&self) -> SoftInterruptQueue {
        Arc::clone(&self.soft_interrupt_queue)
    }

    /// Check if there are pending soft interrupts
    pub fn has_soft_interrupts(&self) -> bool {
        self.soft_interrupt_queue
            .lock()
            .map(|q| !q.is_empty())
            .unwrap_or(false)
    }

    /// Check if there's an urgent soft interrupt that should skip remaining tools
    pub fn has_urgent_interrupt(&self) -> bool {
        self.soft_interrupt_queue
            .lock()
            .map(|q| q.iter().any(|m| m.urgent))
            .unwrap_or(false)
    }

    /// Get count of queued soft interrupts
    pub fn soft_interrupt_count(&self) -> usize {
        self.soft_interrupt_queue
            .lock()
            .map(|q| q.len())
            .unwrap_or(0)
    }

    /// Get count of pending alerts
    pub fn pending_alert_count(&self) -> usize {
        self.pending_alerts.len()
    }

    /// Get pending alerts (for debug visibility)
    pub fn pending_alerts_preview(&self) -> Vec<String> {
        self.pending_alerts
            .iter()
            .take(10)
            .map(|s| {
                if s.len() > 100 {
                    format!("{}...", &s[..100])
                } else {
                    s.clone()
                }
            })
            .collect()
    }

    /// Get comprehensive debug info about agent internal state
    pub fn debug_info(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": self.provider.name(),
            "model": self.provider.model(),
            "provider_session_id": self.provider_session_id,
            "last_upstream_provider": self.last_upstream_provider,
            "active_skill": self.active_skill,
            "allowed_tools": self.allowed_tools,
            "session": {
                "id": self.session.id,
                "is_canary": self.session.is_canary,
                "model": self.session.model,
                "working_dir": self.session.working_dir,
                "message_count": self.session.messages.len(),
            },
            "interrupts": {
                "soft_interrupt_count": self.soft_interrupt_count(),
                "has_urgent": self.has_urgent_interrupt(),
                "pending_alert_count": self.pending_alert_count(),
                "soft_interrupts": self.soft_interrupts_preview(),
                "pending_alerts": self.pending_alerts_preview(),
            },
            "cache_tracker": {
                "turn_count": self.cache_tracker.turn_count(),
                "had_violation": self.cache_tracker.had_violation(),
            },
            "features": {
                "memory_enabled": self.memory_enabled,
            },
            "token_usage": {
                "input": self.last_usage.input_tokens,
                "output": self.last_usage.output_tokens,
                "cache_read": self.last_usage.cache_read_input_tokens,
                "cache_write": self.last_usage.cache_creation_input_tokens,
            },
        })
    }

    /// Get soft interrupt previews (for debug visibility)
    pub fn soft_interrupts_preview(&self) -> Vec<(String, bool)> {
        self.soft_interrupt_queue
            .lock()
            .map(|q| {
                q.iter()
                    .take(10)
                    .map(|m| {
                        let preview = if m.content.len() > 100 {
                            format!("{}...", &m.content[..100])
                        } else {
                            m.content.clone()
                        };
                        (preview, m.urgent)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Inject all pending soft interrupt messages into the conversation.
    /// Returns the combined message content and clears the queue.
    fn inject_soft_interrupts(&mut self) -> Option<String> {
        let messages: Vec<SoftInterruptMessage> = {
            let mut queue = self.soft_interrupt_queue.lock().ok()?;
            if queue.is_empty() {
                return None;
            }
            queue.drain(..).collect()
        };

        let combined: String = messages
            .into_iter()
            .map(|m| m.content)
            .collect::<Vec<_>>()
            .join("\n\n");

        // Add as user message to conversation
        self.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: combined.clone(),
                cache_control: None,
            }],
        );
        let _ = self.session.save();

        Some(combined)
    }

    pub fn session_id(&self) -> &str {
        &self.session.id
    }

    /// Mark this agent session as closed and persist it.
    pub fn mark_closed(&mut self) {
        self.session.mark_closed();
        let _ = self.session.save();
    }

    pub fn mark_crashed(&mut self, message: Option<String>) {
        self.session.mark_crashed(message);
        let _ = self.session.save();
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
        let _ = self.session.save();
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
        self.last_upstream_provider.clone()
    }

    pub fn provider_name(&self) -> String {
        self.provider.name().to_string()
    }

    pub fn provider_model(&self) -> String {
        self.provider.model().to_string()
    }

    pub fn provider_fork(&self) -> Arc<dyn Provider> {
        self.provider.fork()
    }

    pub fn available_models(&self) -> Vec<&'static str> {
        self.provider.available_models()
    }

    pub fn available_models_display(&self) -> Vec<String> {
        self.provider.available_models_display()
    }

    pub fn registry(&self) -> Registry {
        self.registry.clone()
    }

    pub fn set_model(&mut self, model: &str) -> Result<()> {
        self.provider.set_model(model)?;
        self.session.model = Some(self.provider.model());
        self.log_env_snapshot("set_model");
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
        self.session.save()?;
        self.run_turn_streaming_mpsc(event_tx).await
    }

    /// Clear conversation history
    pub fn clear(&mut self) {
        let preserve_canary = self.session.is_canary;
        let preserve_testing_build = self.session.testing_build.clone();
        let preserve_debug = self.session.is_debug;
        let preserve_working_dir = self.session.working_dir.clone();

        let mut new_session = Session::create(None, None);
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

    /// Build the system prompt, including skill, memory, self-dev context, and CLAUDE.md files
    fn build_system_prompt(&self, memory_prompt: Option<&str>) -> String {
        let split = self.build_system_prompt_split(memory_prompt);
        if split.dynamic_part.is_empty() {
            split.static_part
        } else if split.static_part.is_empty() {
            split.dynamic_part
        } else {
            format!("{}\n\n{}", split.static_part, split.dynamic_part)
        }
    }

    /// Build split system prompt for better caching
    /// Returns static (cacheable) and dynamic (not cached) parts separately
    fn build_system_prompt_split(
        &self,
        memory_prompt: Option<&str>,
    ) -> crate::prompt::SplitSystemPrompt {
        // If there's a system prompt override (e.g., ambient mode), use it directly
        if let Some(ref override_prompt) = self.system_prompt_override {
            return crate::prompt::SplitSystemPrompt {
                static_part: override_prompt.clone(),
                dynamic_part: String::new(),
            };
        }

        // Get skill prompt if active
        let skill_prompt = self
            .active_skill
            .as_ref()
            .and_then(|name| self.skills.get(name).map(|s| s.get_prompt().to_string()));

        // Build list of available skills for prompt
        let available_skills: Vec<crate::prompt::SkillInfo> = self
            .skills
            .list()
            .iter()
            .map(|s| crate::prompt::SkillInfo {
                name: s.name.clone(),
                description: s.description.clone(),
            })
            .collect();

        // Get working directory from session for context loading
        let working_dir = self
            .session
            .working_dir
            .as_ref()
            .map(|s| std::path::PathBuf::from(s));

        // Use split prompt builder for better cache efficiency
        let (split, _context_info) = crate::prompt::build_system_prompt_split(
            skill_prompt.as_deref(),
            &available_skills,
            self.session.is_canary,
            memory_prompt,
            working_dir.as_deref(),
        );

        split
    }

    /// Non-blocking memory prompt - takes pending result and spawns check for next turn
    fn build_memory_prompt_nonblocking(
        &self,
        messages: &[Message],
    ) -> Option<crate::memory::PendingMemory> {
        if !self.memory_enabled {
            return None;
        }

        // Take pending memory if available (computed in background during last turn)
        let pending = crate::memory::take_pending_memory();

        // Spawn a background check for the NEXT turn (doesn't block current send)
        let manager = crate::memory::MemoryManager::new();
        manager.spawn_relevance_check(messages.to_vec());

        // Return pending memory from previous turn
        pending
    }

    /// Legacy blocking memory prompt - kept for fallback
    #[allow(dead_code)]
    async fn build_memory_prompt(&self, messages: &[Message]) -> Option<String> {
        let manager = crate::memory::MemoryManager::new();
        match manager.relevant_prompt_for_messages(messages).await {
            Ok(prompt) => prompt,
            Err(e) => {
                logging::info(&format!("Memory relevance skipped: {}", e));
                None
            }
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
            crate::memory::clear_pending_memory();
        }
    }

    /// Check whether memory features are enabled for this session.
    pub fn memory_enabled(&self) -> bool {
        self.memory_enabled
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
        if name == "selfdev" && !self.session.is_canary {
            return Err(anyhow::anyhow!(
                "Tool 'selfdev' is only available in self-dev mode"
            ));
        }
        if let Some(allowed) = self.allowed_tools.as_ref() {
            if !allowed.contains(name) {
                return Err(anyhow::anyhow!("Tool '{}' is not allowed", name));
            }
        }

        let call_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| format!("debug-{}", d.as_millis()))
            .unwrap_or_else(|_| "debug".to_string());
        let ctx = ToolContext {
            session_id: self.session.id.clone(),
            message_id: self.session.id.clone(),
            tool_call_id: call_id,
            working_dir: self.working_dir().map(PathBuf::from),
        };
        self.registry.execute(name, input, ctx).await
    }

    /// Restore a session by ID (loads from disk)
    pub fn restore_session(&mut self, session_id: &str) -> Result<()> {
        let session = Session::load(session_id)?;
        logging::info(&format!(
            "Restoring session '{}' with {} messages, provider_session_id: {:?}",
            session_id,
            session.messages.len(),
            session.provider_session_id
        ));
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
        logging::info(&format!(
            "Session restored: {} messages in session",
            self.session.messages.len()
        ));
        Ok(())
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
                }
            }
            transcript.push('\n');
        }

        // Extract using sidecar
        let sidecar = crate::sidecar::HaikuSidecar::new();
        match sidecar.extract_memories(&transcript).await {
            Ok(extracted) if !extracted.is_empty() => {
                let manager = crate::memory::MemoryManager::new();
                let mut stored_count = 0;

                for memory in &extracted {
                    let category = match memory.category.as_str() {
                        "fact" => crate::memory::MemoryCategory::Fact,
                        "preference" => crate::memory::MemoryCategory::Preference,
                        "correction" => crate::memory::MemoryCategory::Correction,
                        _ => crate::memory::MemoryCategory::Fact,
                    };

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
    async fn run_turn(&mut self, print_output: bool) -> Result<String> {
        self.set_log_context();
        let mut final_text = String::new();
        let trace = trace_enabled();

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

            // Check for client-side cache violations (fallback when provider doesn't report cache tokens)
            if let Some(violation) = self.cache_tracker.record_request(&messages_with_memory) {
                logging::warn(&format!(
                    "CLIENT_CACHE_VIOLATION: {} | turn={} messages={}",
                    violation.reason, violation.turn, violation.message_count
                ));
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
            }));

            let stamped;
            let send_messages: &[Message] = if crate::config::config().features.message_timestamps {
                stamped = Message::with_timestamps(&messages_with_memory);
                &stamped
            } else {
                &messages_with_memory
            };
            let mut stream = self
                .provider
                .complete_split(
                    send_messages,
                    &tools,
                    &split_prompt.static_part,
                    &split_prompt.dynamic_part,
                    self.provider_session_id.as_deref(),
                )
                .await?;

            logging::info(&format!(
                "API stream opened in {:.2}s",
                api_start.elapsed().as_secs_f64()
            ));

            Bus::global().publish(BusEvent::SubagentStatus(SubagentStatus {
                session_id: self.session.id.clone(),
                status: "streaming".to_string(),
            }));

            let mut text_content = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut current_tool: Option<ToolCall> = None;
            let mut current_tool_input = String::new();
            let mut usage_input: Option<u64> = None;
            let mut usage_output: Option<u64> = None;
            let mut usage_cache_read: Option<u64> = None;
            let mut usage_cache_creation: Option<u64> = None;
            let mut saw_message_end = false;
            let mut _thinking_start: Option<Instant> = None;
            let store_reasoning_content = self.provider.name() == "openrouter";
            let mut reasoning_content = String::new();
            // Track tool results from provider (already executed by Claude Code CLI)
            let mut sdk_tool_results: std::collections::HashMap<String, (String, bool)> =
                std::collections::HashMap::new();

            while let Some(event) = stream.next().await {
                match event? {
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
                    StreamEvent::MessageEnd { .. } => {
                        saw_message_end = true;
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
                    } => {
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
                        };
                        let tool_result = self.registry.execute(&tool_name, input, ctx).await;
                        let native_result = match tool_result {
                            Ok(output) => NativeToolResult::success(request_id, output.output),
                            Err(e) => NativeToolResult::error(request_id, e.to_string()),
                        };
                        // Send result back to SDK bridge
                        if let Some(sender) = self.provider.native_result_sender() {
                            let _ = sender.send(native_result).await;
                        }
                    }
                    StreamEvent::Error { message, .. } => {
                        if trace {
                            eprintln!("[trace] stream_error {}", message);
                        }
                        return Err(anyhow::anyhow!("Stream error: {}", message));
                    }
                }
            }

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
                let message_id = self.add_message(Role::Assistant, content_blocks);
                self.session.save()?;
                Some(message_id)
            } else {
                None
            };

            // If no tool calls, we're done
            if tool_calls.is_empty() {
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
            for tc in tool_calls {
                if tc.name == "selfdev" && !self.session.is_canary {
                    return Err(anyhow::anyhow!(
                        "Tool 'selfdev' is only available in self-dev mode"
                    ));
                }
                if let Some(allowed) = self.allowed_tools.as_ref() {
                    if !allowed.contains(&tc.name) {
                        return Err(anyhow::anyhow!("Tool '{}' is not allowed", tc.name));
                    }
                }

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
                        self.session.save()?;
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
                }));

                let result = self.registry.execute(&tc.name, tc.input.clone(), ctx).await;
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

                        self.add_message(
                            Role::User,
                            vec![ContentBlock::ToolResult {
                                tool_use_id: tc.id,
                                content: output.output,
                                is_error: None,
                            }],
                        );
                        self.session.save()?;
                    }
                    Err(e) => {
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
                        self.add_message(
                            Role::User,
                            vec![ContentBlock::ToolResult {
                                tool_use_id: tc.id,
                                content: error_msg,
                                is_error: Some(true),
                            }],
                        );
                        self.session.save()?;
                    }
                }
            }

            if print_output {
                println!();
            }

            // Check for soft interrupts (e.g. Telegram messages) and inject as user messages
            if let Some(content) = self.inject_soft_interrupts() {
                logging::info(&format!(
                    "Soft interrupt injected into headless turn ({} chars)",
                    content.len()
                ));
            }
        }

        Ok(final_text)
    }

    /// Run turns with events streamed to a broadcast channel (for server mode)
    async fn run_turn_streaming(&mut self, event_tx: broadcast::Sender<ServerEvent>) -> Result<()> {
        self.set_log_context();
        let trace = trace_enabled();

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
            }

            let tools = self.tool_definitions().await;
            // Non-blocking memory: uses pending result from last turn, spawns check for next turn
            let memory_pending = self.build_memory_prompt_nonblocking(&messages);
            // Use split prompt for better caching - static content cached, dynamic not
            let split_prompt = self.build_system_prompt_split(None);

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
                    prompt_chars: memory.prompt.chars().count(),
                    computed_age_ms,
                });
                let memory_msg =
                    format!("<system-reminder>\n{}\n</system-reminder>", memory.prompt);
                messages_with_memory.push(Message::user(&memory_msg));
            }

            // Check for client-side cache violations (fallback when provider doesn't report cache tokens)
            if let Some(violation) = self.cache_tracker.record_request(&messages_with_memory) {
                logging::warn(&format!(
                    "CLIENT_CACHE_VIOLATION: {} | turn={} messages={}",
                    violation.reason, violation.turn, violation.message_count
                ));
            }

            let stamped;
            let send_messages: &[Message] = if crate::config::config().features.message_timestamps {
                stamped = Message::with_timestamps(&messages_with_memory);
                &stamped
            } else {
                &messages_with_memory
            };
            let mut stream = self
                .provider
                .complete_split(
                    send_messages,
                    &tools,
                    &split_prompt.static_part,
                    &split_prompt.dynamic_part,
                    self.provider_session_id.as_deref(),
                )
                .await?;

            let mut text_content = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut current_tool: Option<ToolCall> = None;
            let mut current_tool_input = String::new();
            let mut usage_input: Option<u64> = None;
            let mut usage_output: Option<u64> = None;
            let mut usage_cache_read: Option<u64> = None;
            let mut usage_cache_creation: Option<u64> = None;
            let mut sdk_tool_results: std::collections::HashMap<String, (String, bool)> =
                std::collections::HashMap::new();
            let store_reasoning_content = self.provider.name() == "openrouter";
            let mut reasoning_content = String::new();
            // Track tool_use_id -> name for tool results
            let mut tool_id_to_name: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();

            while let Some(event) = stream.next().await {
                match event? {
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
                        let _ = event_tx.send(ServerEvent::TextDelta { text: text.clone() });
                        text_content.push_str(&text);
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
                    StreamEvent::MessageEnd { .. } => {}
                    StreamEvent::SessionId(sid) => {
                        self.provider_session_id = Some(sid.clone());
                        self.session.provider_session_id = Some(sid.clone());
                        let _ = event_tx.send(ServerEvent::SessionId { session_id: sid });
                    }
                    StreamEvent::Compaction { .. } => {}
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
                        };
                        let tool_result = self.registry.execute(&tool_name, input, ctx).await;
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
                    StreamEvent::Error { message, .. } => {
                        return Err(anyhow::anyhow!("Stream error: {}", message));
                    }
                }
            }

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
                let message_id = self.add_message(Role::Assistant, content_blocks);
                self.session.save()?;
                Some(message_id)
            } else {
                None
            };

            // If no tool calls, check for soft interrupt or exit
            // NOTE: We only inject here (Point B) when there are no tools.
            // Injecting before tool_results would break the API requirement that
            // tool_use must be immediately followed by tool_result.
            if tool_calls.is_empty() {
                // === INJECTION POINT B: No tools, turn complete ===
                if let Some(content) = self.inject_soft_interrupts() {
                    let _ = event_tx.send(ServerEvent::SoftInterruptInjected {
                        content,
                        point: "B".to_string(),
                        tools_skipped: None,
                    });
                    // Continue loop to process the injected message
                    continue;
                }
                break;
            }

            // If provider handles tools internally, only run native tools locally
            if self.provider.handles_tools_internally() {
                tool_calls.retain(|tc| JCODE_NATIVE_TOOLS.contains(&tc.name.as_str()));
                if tool_calls.is_empty() {
                    // === INJECTION POINT D: After provider-handled tools, before next API call ===
                    if let Some(content) = self.inject_soft_interrupts() {
                        let _ = event_tx.send(ServerEvent::SoftInterruptInjected {
                            content,
                            point: "D".to_string(),
                            tools_skipped: None,
                        });
                        // Don't break - continue loop to process injected message
                        continue;
                    }
                    break;
                }
            }

            // Execute tools and add results
            let tool_count = tool_calls.len();
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
                    if let Some(content) = self.inject_soft_interrupts() {
                        let _ = event_tx.send(ServerEvent::SoftInterruptInjected {
                            content,
                            point: "C".to_string(),
                            tools_skipped: Some(tools_remaining),
                        });
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

                if tc.name == "selfdev" && !self.session.is_canary {
                    return Err(anyhow::anyhow!(
                        "Tool 'selfdev' is only available in self-dev mode"
                    ));
                }
                if let Some(allowed) = self.allowed_tools.as_ref() {
                    if !allowed.contains(&tc.name) {
                        return Err(anyhow::anyhow!("Tool '{}' is not allowed", tc.name));
                    }
                }

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
                        self.session.save()?;

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
                };

                if trace {
                    eprintln!("[trace] tool_exec_start name={} id={}", tc.name, tc.id);
                }

                let result = self.registry.execute(&tc.name, tc.input.clone(), ctx).await;
                self.unlock_tools_if_needed(&tc.name);

                match result {
                    Ok(output) => {
                        let _ = event_tx.send(ServerEvent::ToolDone {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                            output: output.output.clone(),
                            error: None,
                        });

                        self.add_message(
                            Role::User,
                            vec![ContentBlock::ToolResult {
                                tool_use_id: tc.id.clone(),
                                content: output.output,
                                is_error: None,
                            }],
                        );
                        self.session.save()?;
                    }
                    Err(e) => {
                        let error_msg = format!("Error: {}", e);
                        let _ = event_tx.send(ServerEvent::ToolDone {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                            output: error_msg.clone(),
                            error: Some(error_msg.clone()),
                        });

                        self.add_message(
                            Role::User,
                            vec![ContentBlock::ToolResult {
                                tool_use_id: tc.id.clone(),
                                content: error_msg,
                                is_error: Some(true),
                            }],
                        );
                        self.session.save()?;
                    }
                }

                // NOTE: We do NOT inject between tools (non-urgent) because that would
                // place user text between tool_results, which may violate API constraints.
                // All non-urgent injection happens at Point D after all tools are done.
            }

            // === INJECTION POINT D: All tools done, before next API call ===
            // This is the safest point for non-urgent injection since all tool_results
            // have been added and the conversation is in a valid state.
            if let Some(content) = self.inject_soft_interrupts() {
                let _ = event_tx.send(ServerEvent::SoftInterruptInjected {
                    content,
                    point: "D".to_string(),
                    tools_skipped: None,
                });
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
            }

            let tools = self.tool_definitions().await;
            // Non-blocking memory: uses pending result from last turn, spawns check for next turn
            let memory_pending = self.build_memory_prompt_nonblocking(&messages);
            // Use split prompt for better caching - static content cached, dynamic not
            let split_prompt = self.build_system_prompt_split(None);

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
                    prompt_chars: memory.prompt.chars().count(),
                    computed_age_ms,
                });
                let memory_msg =
                    format!("<system-reminder>\n{}\n</system-reminder>", memory.prompt);
                messages_with_memory.push(Message::user(&memory_msg));
            }

            // Check for client-side cache violations (fallback when provider doesn't report cache tokens)
            if let Some(violation) = self.cache_tracker.record_request(&messages_with_memory) {
                logging::warn(&format!(
                    "CLIENT_CACHE_VIOLATION: {} | turn={} messages={}",
                    violation.reason, violation.turn, violation.message_count
                ));
            }

            let stamped;
            let send_messages: &[Message] = if crate::config::config().features.message_timestamps {
                stamped = Message::with_timestamps(&messages_with_memory);
                &stamped
            } else {
                &messages_with_memory
            };
            let mut stream = self
                .provider
                .complete_split(
                    send_messages,
                    &tools,
                    &split_prompt.static_part,
                    &split_prompt.dynamic_part,
                    self.provider_session_id.as_deref(),
                )
                .await?;

            let mut text_content = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut current_tool: Option<ToolCall> = None;
            let mut current_tool_input = String::new();
            let mut usage_input: Option<u64> = None;
            let mut usage_output: Option<u64> = None;
            let mut usage_cache_read: Option<u64> = None;
            let mut usage_cache_creation: Option<u64> = None;
            let mut sdk_tool_results: std::collections::HashMap<String, (String, bool)> =
                std::collections::HashMap::new();
            let store_reasoning_content = self.provider.name() == "openrouter";
            let mut reasoning_content = String::new();
            let mut tool_id_to_name: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();

            while let Some(event) = stream.next().await {
                match event? {
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
                        let _ = event_tx.send(ServerEvent::TextDelta { text: text.clone() });
                        text_content.push_str(&text);
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
                    StreamEvent::MessageEnd { .. } => {}
                    StreamEvent::SessionId(sid) => {
                        self.provider_session_id = Some(sid.clone());
                        self.session.provider_session_id = Some(sid.clone());
                        let _ = event_tx.send(ServerEvent::SessionId { session_id: sid });
                    }
                    StreamEvent::Compaction { .. } => {}
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
                        };
                        let tool_result = self.registry.execute(&tool_name, input, ctx).await;
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
                    StreamEvent::Error { message, .. } => {
                        return Err(anyhow::anyhow!("Stream error: {}", message));
                    }
                }
            }

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
                let message_id = self.add_message(Role::Assistant, content_blocks);
                self.session.save()?;
                Some(message_id)
            } else {
                None
            };

            // If no tool calls, check for soft interrupt or exit
            // NOTE: We only inject here (Point B) when there are no tools.
            // Injecting before tool_results would break the API requirement that
            // tool_use must be immediately followed by tool_result.
            if tool_calls.is_empty() {
                // === INJECTION POINT B: No tools, turn complete ===
                if let Some(content) = self.inject_soft_interrupts() {
                    let _ = event_tx.send(ServerEvent::SoftInterruptInjected {
                        content,
                        point: "B".to_string(),
                        tools_skipped: None,
                    });
                    // Continue loop to process the injected message
                    continue;
                }
                break;
            }

            if self.provider.handles_tools_internally() {
                tool_calls.retain(|tc| JCODE_NATIVE_TOOLS.contains(&tc.name.as_str()));
                if tool_calls.is_empty() {
                    // === INJECTION POINT D: After provider-handled tools, before next API call ===
                    if let Some(content) = self.inject_soft_interrupts() {
                        let _ = event_tx.send(ServerEvent::SoftInterruptInjected {
                            content,
                            point: "D".to_string(),
                            tools_skipped: None,
                        });
                        // Don't break - continue loop to process injected message
                        continue;
                    }
                    break;
                }
            }

            // Execute tools and add results
            let tool_count = tool_calls.len();
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
                    if let Some(content) = self.inject_soft_interrupts() {
                        let _ = event_tx.send(ServerEvent::SoftInterruptInjected {
                            content,
                            point: "C".to_string(),
                            tools_skipped: Some(tools_remaining),
                        });
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

                if tc.name == "selfdev" && !self.session.is_canary {
                    return Err(anyhow::anyhow!(
                        "Tool 'selfdev' is only available in self-dev mode"
                    ));
                }
                if let Some(allowed) = self.allowed_tools.as_ref() {
                    if !allowed.contains(&tc.name) {
                        return Err(anyhow::anyhow!("Tool '{}' is not allowed", tc.name));
                    }
                }

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
                        self.session.save()?;

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
                };

                if trace {
                    eprintln!("[trace] tool_exec_start name={} id={}", tc.name, tc.id);
                }

                let result = self.registry.execute(&tc.name, tc.input.clone(), ctx).await;
                self.unlock_tools_if_needed(&tc.name);

                match result {
                    Ok(output) => {
                        let _ = event_tx.send(ServerEvent::ToolDone {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                            output: output.output.clone(),
                            error: None,
                        });

                        self.add_message(
                            Role::User,
                            vec![ContentBlock::ToolResult {
                                tool_use_id: tc.id.clone(),
                                content: output.output,
                                is_error: None,
                            }],
                        );
                        self.session.save()?;
                    }
                    Err(e) => {
                        let error_msg = format!("Error: {}", e);
                        let _ = event_tx.send(ServerEvent::ToolDone {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                            output: error_msg.clone(),
                            error: Some(error_msg.clone()),
                        });

                        self.add_message(
                            Role::User,
                            vec![ContentBlock::ToolResult {
                                tool_use_id: tc.id.clone(),
                                content: error_msg,
                                is_error: Some(true),
                            }],
                        );
                        self.session.save()?;
                    }
                }

                // NOTE: We do NOT inject between tools (non-urgent) because that would
                // place user text between tool_results, which may violate API constraints.
                // All non-urgent injection happens at Point D after all tools are done.
            }

            // === INJECTION POINT D: All tools done, before next API call ===
            // This is the safest point for non-urgent injection since all tool_results
            // have been added and the conversation is in a valid state.
            if let Some(content) = self.inject_soft_interrupts() {
                let _ = event_tx.send(ServerEvent::SoftInterruptInjected {
                    content,
                    point: "D".to_string(),
                    tools_skipped: None,
                });
            }
        }

        Ok(())
    }
}

fn print_tool_summary(tool: &ToolCall) {
    match tool.name.as_str() {
        "bash" => {
            if let Some(cmd) = tool.input.get("command").and_then(|v| v.as_str()) {
                let short = if cmd.len() > 60 {
                    format!("{}...", crate::util::truncate_str(cmd, 60))
                } else {
                    cmd.to_string()
                };
                println!("$ {}", short);
            }
        }
        "read" | "write" | "edit" => {
            if let Some(path) = tool.input.get("file_path").and_then(|v| v.as_str()) {
                println!("{}", path);
            }
        }
        "glob" | "grep" => {
            if let Some(pattern) = tool.input.get("pattern").and_then(|v| v.as_str()) {
                println!("'{}'", pattern);
            }
        }
        "ls" => {
            let path = tool
                .input
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".");
            println!("{}", path);
        }
        _ => {}
    }
}

fn trace_enabled() -> bool {
    match std::env::var("JCODE_TRACE") {
        Ok(value) => {
            let value = value.trim();
            !value.is_empty() && value != "0" && value.to_lowercase() != "false"
        }
        Err(_) => false,
    }
}

fn git_state_for_dir(dir: &Path) -> Option<GitState> {
    let root = git_output(dir, &["rev-parse", "--show-toplevel"])?;
    let head = git_output(dir, &["rev-parse", "HEAD"]);
    let branch = git_output(dir, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let dirty = git_output(dir, &["status", "--porcelain"]).map(|out| !out.is_empty());

    Some(GitState {
        root,
        head,
        branch,
        dirty,
    })
}

fn git_output(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
