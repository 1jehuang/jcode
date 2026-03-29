use super::Agent;
use crate::logging;
use crate::message::{ContentBlock, Role};
use crate::protocol::ServerEvent;
use crate::session::StoredDisplayRole;
use anyhow::Result;
use std::sync::Arc;

/// A soft interrupt message queued for injection at the next safe point
#[derive(Debug, Clone)]
pub struct SoftInterruptMessage {
    pub content: String,
    /// If true, can skip remaining tools when injected at point C
    pub urgent: bool,
    pub source: SoftInterruptSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoftInterruptSource {
    User,
    System,
}

impl SoftInterruptSource {
    fn session_display_role(self) -> Option<StoredDisplayRole> {
        match self {
            Self::User => None,
            Self::System => Some(StoredDisplayRole::System),
        }
    }

    fn protocol_display_role(self) -> Option<String> {
        match self {
            Self::User => None,
            Self::System => Some("system".to_string()),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct InjectedSoftInterrupt {
    pub(super) content: String,
    pub(super) source: SoftInterruptSource,
}

/// Thread-safe soft interrupt queue that can be accessed without holding the agent lock
pub type SoftInterruptQueue = Arc<std::sync::Mutex<Vec<SoftInterruptMessage>>>;

/// Signal to move the currently executing tool to background.
/// Set by the server when the client sends BackgroundTool request.
/// Uses std::sync so it can be set without async from outside the agent lock.
pub type BackgroundToolSignal = Arc<std::sync::atomic::AtomicBool>;
pub type GracefulShutdownSignal = Arc<std::sync::atomic::AtomicBool>;

/// Async-aware interrupt signal that combines AtomicBool (sync read) with
/// tokio::Notify (async wake). Eliminates spin-loops during tool execution.
#[derive(Clone)]
pub struct InterruptSignal {
    flag: Arc<std::sync::atomic::AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
}

impl InterruptSignal {
    pub fn new() -> Self {
        Self {
            flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            notify: Arc::new(tokio::sync::Notify::new()),
        }
    }

    pub fn fire(&self) {
        self.flag.store(true, std::sync::atomic::Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn is_set(&self) -> bool {
        self.flag.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn reset(&self) {
        self.flag.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    pub async fn notified(&self) {
        let notified = self.notify.notified();
        if self.is_set() {
            return;
        }
        notified.await;
    }

    pub fn as_atomic(&self) -> Arc<std::sync::atomic::AtomicBool> {
        Arc::clone(&self.flag)
    }
}

impl Default for InterruptSignal {
    fn default() -> Self {
        Self::new()
    }
}

pub(super) enum NoToolCallOutcome {
    Break,
    ContinueWithoutEvent,
    ContinueWithSoftInterrupt {
        injected: Vec<InjectedSoftInterrupt>,
        point: &'static str,
    },
}

pub(super) enum PostToolInterruptOutcome {
    NoInterrupt,
    SoftInterrupt {
        injected: Vec<InjectedSoftInterrupt>,
        point: &'static str,
    },
}

impl Agent {
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
    pub fn queue_soft_interrupt(&self, content: String, urgent: bool, source: SoftInterruptSource) {
        if let Ok(mut queue) = self.soft_interrupt_queue.lock() {
            queue.push(SoftInterruptMessage {
                content,
                urgent,
                source,
            });
        }
    }

    /// Get a handle to the soft interrupt queue.
    /// The server can use this to queue interrupts without holding the agent lock.
    pub fn soft_interrupt_queue(&self) -> SoftInterruptQueue {
        Arc::clone(&self.soft_interrupt_queue)
    }

    /// Get a handle to the background tool signal.
    /// The server can use this to signal "move tool to background" without holding the agent lock.
    pub fn background_tool_signal(&self) -> InterruptSignal {
        self.background_tool_signal.clone()
    }

    pub fn graceful_shutdown_signal(&self) -> InterruptSignal {
        self.graceful_shutdown.clone()
    }

    pub fn request_graceful_shutdown(&self) {
        self.graceful_shutdown.fire();
    }

    pub(super) fn is_graceful_shutdown(&self) -> bool {
        self.graceful_shutdown.is_set()
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
            "last_connection_type": self.last_connection_type,
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
    pub(super) fn inject_soft_interrupts(&mut self) -> Vec<InjectedSoftInterrupt> {
        let messages: Vec<SoftInterruptMessage> = {
            let mut queue = match self.soft_interrupt_queue.lock() {
                Ok(queue) => queue,
                Err(_) => return Vec::new(),
            };
            if queue.is_empty() {
                return Vec::new();
            }
            queue.drain(..).collect()
        };

        let mut injected = Vec::new();
        let mut current_source: Option<SoftInterruptSource> = None;
        let mut current_parts: Vec<String> = Vec::new();

        let flush_group = |agent: &mut Self,
                           injected: &mut Vec<InjectedSoftInterrupt>,
                           source: SoftInterruptSource,
                           parts: &mut Vec<String>| {
            if parts.is_empty() {
                return;
            }
            let content = parts.join("\n\n");
            parts.clear();
            agent.add_message_with_display_role(
                Role::User,
                vec![ContentBlock::Text {
                    text: content.clone(),
                    cache_control: None,
                }],
                source.session_display_role(),
            );
            injected.push(InjectedSoftInterrupt { content, source });
        };

        for message in messages {
            match current_source {
                Some(source) if source != message.source => {
                    flush_group(self, &mut injected, source, &mut current_parts);
                    current_source = Some(message.source);
                }
                None => current_source = Some(message.source),
                _ => {}
            }
            current_parts.push(message.content);
        }

        if let Some(source) = current_source {
            flush_group(self, &mut injected, source, &mut current_parts);
        }

        let _ = self.session.save();
        injected
    }

    pub(super) fn handle_streaming_no_tool_calls(
        &mut self,
        stop_reason: Option<&str>,
        incomplete_continuations: &mut u32,
    ) -> Result<NoToolCallOutcome> {
        if self.maybe_continue_incomplete_response(stop_reason, incomplete_continuations)? {
            return Ok(NoToolCallOutcome::ContinueWithoutEvent);
        }
        logging::info("Turn complete - no tool calls");
        let injected = self.inject_soft_interrupts();
        if !injected.is_empty() {
            return Ok(NoToolCallOutcome::ContinueWithSoftInterrupt {
                injected,
                point: "B",
            });
        }
        Ok(NoToolCallOutcome::Break)
    }

    pub(super) fn take_post_tool_soft_interrupt(&mut self) -> PostToolInterruptOutcome {
        let injected = self.inject_soft_interrupts();
        if !injected.is_empty() {
            PostToolInterruptOutcome::SoftInterrupt {
                injected,
                point: "D",
            }
        } else {
            PostToolInterruptOutcome::NoInterrupt
        }
    }

    pub(super) fn build_soft_interrupt_events(
        injected: Vec<InjectedSoftInterrupt>,
        point: &'static str,
        tools_skipped: Option<usize>,
    ) -> Vec<ServerEvent> {
        injected
            .into_iter()
            .enumerate()
            .map(|(idx, interrupt)| ServerEvent::SoftInterruptInjected {
                content: interrupt.content,
                display_role: interrupt.source.protocol_display_role(),
                point: point.to_string(),
                tools_skipped: if idx == 0 { tools_skipped } else { None },
            })
            .collect()
    }
}
