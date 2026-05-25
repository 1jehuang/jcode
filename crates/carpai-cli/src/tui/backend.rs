//! Backend abstraction for TUI runtime transports.
//!
//! This module provides a unified interface for message processing across
//! local harnesses and server-backed remote clients.
//!
//! Also provides debug socket events for exposing full TUI state.

use crate::message::ToolCall;
use crate::protocol::{FeatureToggle, Request, ServerEvent};
use crate::server;
use crate::transport::{Stream, WriteHalf};
use crate::tui::remote_diff::RemoteDiffTracker;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

/// Debug events broadcast by local harnesses via debug socket.
/// These expose the full internal state for debugging/comparison.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DebugEvent {
    /// Full state snapshot (sent on connect)
    StateSnapshot {
        display_messages: Vec<DebugMessage>,
        streaming_text: String,
        streaming_tool_calls: Vec<ToolCall>,
        input: String,
        cursor_pos: usize,
        is_processing: bool,
        scroll_offset: usize,
        status: String,
        provider_name: String,
        provider_model: String,
        mcp_servers: Vec<String>,
        skills: Vec<String>,
        session_id: Option<String>,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_input_tokens: Option<u64>,
        cache_creation_input_tokens: Option<u64>,
        queued_messages: Vec<String>,
    },

    /// Text delta appended to streaming_text
    TextDelta { text: String },

    /// Tool started
    ToolStart { id: String, name: String },

    /// Tool input delta
    ToolInput { delta: String },

    /// Tool about to execute
    ToolExec { id: String, name: String },

    /// Tool completed
    ToolDone {
        id: String,
        name: String,
        output: String,
        is_error: bool,
    },

    /// Message added to display_messages
    MessageAdded { message: DebugMessage },

    /// Streaming text cleared (turn complete)
    StreamingCleared,

    /// Processing state changed
    ProcessingChanged { is_processing: bool },

    /// Status changed
    StatusChanged { status: String },

    /// Token usage update
    TokenUsage {
        input_tokens: u64,
        output_tokens: u64,
        cache_read_input_tokens: Option<u64>,
        cache_creation_input_tokens: Option<u64>,
    },

    /// Input changed (user typing)
    InputChanged { input: String, cursor_pos: usize },

    /// Scroll offset changed
    ScrollChanged { offset: usize },

    /// Message queued
    MessageQueued { content: String },

    /// Queued message sent
    QueuedMessageSent { index: usize },

    /// Session ID set
    SessionId { id: String },

    /// Thinking started
    ThinkingStart,

    /// Thinking ended
    ThinkingEnd,

    /// Compaction occurred
    Compaction { trigger: String, pre_tokens: u64 },

    /// Error occurred
    Error { message: String },
}

/// Simplified message for debug serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugMessage {
    pub role: String,
    pub content: String,
    pub tool_calls: Vec<String>,
    pub duration_secs: Option<f32>,
    pub title: Option<String>,
    pub tool_data: Option<ToolCall>,
}

/// Events emitted by backends during message processing
#[derive(Debug, Clone)]
pub enum BackendEvent {
    /// Text content delta from assistant
    TextDelta(String),

    /// Tool execution started
    ToolStart {
        id: String,
        name: String,
    },

    /// Tool input JSON delta
    ToolInput {
        delta: String,
    },

    /// Tool is about to execute (after input complete)
    ToolExec {
        id: String,
        name: String,
    },

    /// Tool execution completed
    ToolDone {
        id: String,
        name: String,
        output: String,
        is_error: bool,
    },

    /// Token usage update
    TokenUsage {
        input_tokens: u64,
        output_tokens: u64,
        cache_read_input_tokens: Option<u64>,
        cache_creation_input_tokens: Option<u64>,
    },

    /// Thinking started (extended thinking mode)
    ThinkingStart,

    /// Thinking ended
    ThinkingEnd,

    /// Thinking completed with duration
    ThinkingDone {
        duration_secs: f32,
    },

    /// Context compaction occurred
    Compaction {
        trigger: String,
        pre_tokens: u64,
    },

    /// Session ID assigned/updated
    SessionId(String),

    /// Message processing complete
    Done,

    /// Error occurred
    Error(String),

    /// Server is reloading (remote only)
    Reloading,

    /// Connection state changed
    Connected,
    Disconnected,
}

#[derive(Debug, Clone)]
pub enum RemoteDisconnectReason {
    PeerClosed,
    Io(String),
    Protocol(String),
}

#[derive(Debug, Clone)]
#[expect(
    clippy::large_enum_variant,
    reason = "remote reads carry full server events directly to keep transport handling simple"
)]
pub enum RemoteRead {
    Event(ServerEvent),
    Disconnected(RemoteDisconnectReason),
}

/// Information about the backend's provider
#[derive(Debug, Clone)]
pub struct BackendInfo {
    pub provider_name: String,
    pub provider_model: String,
    pub mcp_servers: Vec<String>,
    pub skills: Vec<String>,
}

/// Remote connection to jcode server
pub struct RemoteConnection {
    reader: BufReader<crate::transport::ReadHalf>,
    writer: Arc<Mutex<WriteHalf>>,
    _dummy_peer: Option<Stream>,
    session_id: Option<String>,
    client_instance_id: Option<String>,
    next_request_id: u64,
    tool_diff: RemoteDiffTracker,
    line_buffer: String,
    has_loaded_history: bool,
    call_output_tokens_seen: u64,
}

const DETACHED_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) trait RemoteEventState {
    fn handle_tool_start(&mut self, id: &str, name: &str);
    fn handle_tool_input(&mut self, delta: &str);
    fn get_current_tool_input(&self) -> serde_json::Value;
    fn handle_tool_exec(&mut self, id: &str, name: &str);
    fn handle_tool_done(&mut self, id: &str, name: &str, output: &str) -> String;
    fn clear_pending(&mut self);
    fn call_output_tokens_seen(&mut self) -> &mut u64;
    fn reset_call_output_tokens_seen(&mut self);
    fn set_session_id(&mut self, id: String);
    fn has_loaded_history(&self) -> bool;
    fn mark_history_loaded(&mut self);
}

#[derive(Default)]
pub(crate) struct ReplayRemoteState {
    tool_diff: RemoteDiffTracker,
    call_output_tokens_seen: u64,
}

impl RemoteConnection {
    /// Connect to the server
    pub async fn connect() -> Result<Self> {
        Self::connect_with_session(None, None, false, false).await
    }

    /// Connect to the server and optionally resume a specific session.
    ///
    /// When `client_has_local_history` is true, the client already restored the
    /// transcript locally and only needs lightweight session metadata from the server.
    pub async fn connect_with_session(
        resume_session: Option<&str>,
        client_instance_id: Option<&str>,
        client_has_local_history: bool,
        allow_session_takeover: bool,
    ) -> Result<Self> {
        let connect_start = Instant::now();
        let socket_connect_start = Instant::now();
        let stream = Stream::connect(server::socket_path()).await?;
        let socket_connect_ms = socket_connect_start.elapsed().as_millis();
        let (reader, writer) = stream.into_split();

        let mut conn = Self {
            reader: BufReader::new(reader),
            writer: Arc::new(Mutex::new(writer)),
            _dummy_peer: None,
            session_id: None,
            client_instance_id: client_instance_id.map(str::to_string),
            next_request_id: 1,
            tool_diff: RemoteDiffTracker::default(),
            line_buffer: String::new(),
            has_loaded_history: false,
            call_output_tokens_seen: 0,
        };

        // Subscribe to events
        let subscribe_start = Instant::now();
        let (working_dir, selfdev) = super::subscribe_metadata();
        let resume_target = resume_session
            .filter(|session_id| crate::session::session_exists(session_id))
            .map(|session_id| session_id.to_string());
        conn.send_request(Request::Subscribe {
            id: conn.next_request_id,
            working_dir,
            selfdev,
            target_session_id: resume_target.clone(),
            client_instance_id: conn.client_instance_id.clone(),
            client_has_local_history,
            allow_session_takeover,
        })
        .await?;
        let subscribe_ms = subscribe_start.elapsed().as_millis();
        conn.next_request_id += 1;

        // If resuming a session, the target-aware Subscribe attaches directly to
        // that session and returns History, so avoid a second bootstrap request.
        let bootstrap_request_start = Instant::now();
        let mut bootstrap_request = "get_history";
        if resume_target.is_none() {
            conn.send_request(Request::GetHistory {
                id: conn.next_request_id,
            })
            .await?;
            conn.next_request_id += 1;
        } else {
            bootstrap_request = "subscribe_resume";
        }
        let bootstrap_request_ms = bootstrap_request_start.elapsed().as_millis();

        crate::logging::info(&format!(
            "[TIMING] remote connect: socket={}ms, subscribe={}ms, bootstrap_request={}ms, total={}ms, resumed={}, request={}",
            socket_connect_ms,
            subscribe_ms,
            bootstrap_request_ms,
            connect_start.elapsed().as_millis(),
            resume_session.is_some(),
            bootstrap_request,
        ));

        Ok(conn)
    }

    async fn send_request(&self, request: Request) -> Result<()> {
        let json = serde_json::to_string(&request)? + "\n";
        let mut w = self.writer.lock().await;
        w.write_all(json.as_bytes()).await?;
        Ok(())
    }

    fn send_request_detached(&self, request: Request, label: &'static str) {
        let writer = Arc::clone(&self.writer);
        tokio::spawn(async move {
            let json = match serde_json::to_string(&request) {
                Ok(json) => json + "\n",
                Err(error) => {
                    crate::logging::warn(&format!(
                        "Failed to serialize detached remote request {}: {}",
                        label, error
                    ));
                    return;
                }
            };

            let write_future = async {
                let mut w = writer.lock().await;
                w.write_all(json.as_bytes()).await
            };

            match tokio::time::timeout(DETACHED_REQUEST_TIMEOUT, write_future).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    crate::logging::warn(&format!(
                        "Detached remote request {} failed: {}",
                        label, error
                    ));
                }
                Err(_) => {
                    crate::logging::warn(&format!(
                        "Detached remote request {} timed out after {:?}",
                        label, DETACHED_REQUEST_TIMEOUT
                    ));
                }
            }
        });
    }

    /// Send a message to the server
    /// Send a message to the server and return the request ID
    pub async fn send_message(&mut self, content: String) -> Result<u64> {
        self.send_message_with_images_and_reminder(content, vec![], None)
            .await
    }

    /// Clear the server-side conversation and replace it with a fresh session.
    pub async fn clear(&mut self) -> Result<u64> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        self.send_request(Request::Clear { id }).await?;
        Ok(id)
    }

    /// Send a message with images to the server and return the request ID
    pub async fn send_message_with_images(
        &mut self,
        content: String,
        images: Vec<(String, String)>,
    ) -> Result<u64> {
        self.send_message_with_images_and_reminder(content, images, None)
            .await
    }

    pub async fn send_message_with_images_and_reminder(
        &mut self,
        content: String,
        images: Vec<(String, String)>,
        system_reminder: Option<String>,
    ) -> Result<u64> {
        // Output token usage snapshots are cumulative within a single API call.
        // Reset per-call watermark before sending the next user request.
        self.reset_call_output_tokens_seen();

        let id = self.next_request_id;
        let request = Request::Message {
            id,
            content,
            images,
            system_reminder,
        };
        self.next_request_id += 1;
        self.send_request(request).await?;
        Ok(id)
    }

    /// Request server reload
    pub async fn reload(&mut self) -> Result<()> {
        let request = Request::Reload {
            id: self.next_request_id,
        };
        self.next_request_id += 1;
        self.send_request(request).await
    }

    /// Resume a specific session by ID
    pub async fn resume_session(&mut self, session_id: &str) -> Result<()> {
        let request = Request::ResumeSession {
            id: self.next_request_id,
            session_id: session_id.to_string(),
            client_instance_id: self.client_instance_id.clone(),
            client_has_local_history: false,
            allow_session_takeover: false,
        };
        self.next_request_id += 1;
        self.send_request(request).await
    }

    /// Request a wider compacted-history window for the active session.
    pub async fn get_compacted_history(&mut self, visible_messages: usize) -> Result<u64> {
        let id = self.next_request_id;
        let request = Request::GetCompactedHistory {
            id,
            visible_messages,
        };
        self.next_request_id += 1;
        self.send_request(request).await?;
        Ok(id)
    }

    /// Ask the server to truncate the active session to a 1-based message index.
    pub async fn rewind(&mut self, message_index: usize) -> Result<u64> {
        let id = self.next_request_id;
        let request = Request::Rewind { id, message_index };
        self.next_request_id += 1;

        // The server responds by sending a fresh History payload for the same
        // session. Allow that payload to replace the current display state even
        // though this connection has already completed its initial bootstrap.
        self.has_loaded_history = false;
        self.send_request(request).await?;
        Ok(id)
    }

    /// Ask the server to undo the most recent rewind for the active session.
    pub async fn rewind_undo(&mut self) -> Result<u64> {
        let id = self.next_request_id;
        self.next_request_id += 1;

        // The server responds by sending a fresh History payload for the same
        // session. Allow that payload to replace the current display state.
        self.has_loaded_history = false;
        self.send_request(Request::RewindUndo { id }).await?;
        Ok(id)
    }

    /// Cycle the active model on the server
    pub async fn cycle_model(&mut self, direction: i8) -> Result<()> {
        let request = Request::CycleModel {
            id: self.next_request_id,
            direction,
        };
        self.next_request_id += 1;
        self.send_request(request).await
    }

    /// Trigger a background refresh of available models on the server.
    pub async fn refresh_models(&mut self) -> Result<()> {
        let request = Request::RefreshModels {
            id: self.next_request_id,
        };
        self.next_request_id += 1;
        self.send_request(request).await
    }

    /// Set the active model on the server
    pub async fn set_model(&mut self, model: &str) -> Result<()> {
        let request = Request::SetModel {
            id: self.next_request_id,
            model: model.to_string(),
        };
        self.next_request_id += 1;
        self.send_request(request).await
    }

    /// Set or clear the session-scoped subagent model on the server.
    pub async fn set_subagent_model(&mut self, model: Option<String>) -> Result<()> {
        let request = Request::SetSubagentModel {
            id: self.next_request_id,
            model,
        };
        self.next_request_id += 1;
        self.send_request(request).await
    }

    /// Launch a subagent immediately on the active remote session.
    pub async fn run_subagent(
        &mut self,
        prompt: String,
        subagent_type: String,
        model: Option<String>,
        session_id: Option<String>,
    ) -> Result<()> {
        let request = Request::RunSubagent {
            id: self.next_request_id,
            prompt,
            subagent_type,
            model,
            session_id,
        };
        self.next_request_id += 1;
        self.send_request(request).await
    }

    /// Set Copilot premium request conservation mode on the server
    pub async fn set_premium_mode(&mut self, mode: u8) -> Result<()> {
        let request = Request::SetPremiumMode {
            id: self.next_request_id,
            mode,
        };
        self.next_request_id += 1;
        self.send_request(request).await
    }

    /// Set reasoning effort on the server (for OpenAI models)
    pub async fn set_reasoning_effort(&mut self, effort: &str) -> Result<()> {
        let request = Request::SetReasoningEffort {
            id: self.next_request_id,
            effort: effort.to_string(),
        };
        self.next_request_id += 1;
        self.send_request(request).await
    }

    /// Set service tier on the server (for OpenAI models)
    pub async fn set_service_tier(&mut self, service_tier: &str) -> Result<()> {
        let request = Request::SetServiceTier {
            id: self.next_request_id,
            service_tier: service_tier.to_string(),
        };
        self.next_request_id += 1;
        self.send_request(request).await
    }

    /// Set connection transport on the server (for OpenAI models)
    pub async fn set_transport(&mut self, transport: &str) -> Result<()> {
        let request = Request::SetTransport {
            id: self.next_request_id,
            transport: transport.to_string(),
        };
        self.next_request_id += 1;
        self.send_request(request).await
    }

    /// Toggle a runtime feature on the server for this session
    pub async fn set_feature(&mut self, feature: FeatureToggle, enabled: bool) -> Result<()> {
        let request = Request::SetFeature {
            id: self.next_request_id,
            feature,
            enabled,
        };
        self.next_request_id += 1;
        self.send_request(request).await
    }

    /// Set compaction mode on the server for this session.
    pub async fn set_compaction_mode(&mut self, mode: crate::config::CompactionMode) -> Result<()> {
        let request = Request::SetCompactionMode {
            id: self.next_request_id,
            mode,
        };
        self.next_request_id += 1;
        self.send_request(request).await
    }

    /// Set or clear the custom session display title on the server.
    pub async fn rename_session(&mut self, title: Option<String>) -> Result<()> {
        let request = Request::RenameSession {
            id: self.next_request_id,
            title,
        };
        self.next_request_id += 1;
        self.send_request(request).await
    }

    /// Inject externally transcribed text into the active remote TUI session.
    pub async fn send_transcript(
        &mut self,
        text: String,
        mode: crate::protocol::TranscriptMode,
    ) -> Result<()> {
        let request = Request::Transcript {
            id: self.next_request_id,
            text,
            mode,
            session_id: self.session_id.clone(),
        };
        self.next_request_id += 1;
        self.send_request(request).await
    }

    /// Execute a `!cmd` shell command in the active remote session.
    pub async fn send_input_shell(&mut self, command: String) -> Result<u64> {
        let id = self.next_request_id;
        let request = Request::InputShell { id, command };
        self.next_request_id += 1;
        self.send_request(request).await?;
        Ok(id)
    }

    /// Send stdin input back to a running command
    pub async fn send_stdin_response(&mut self, request_id: &str, input: &str) -> Result<()> {
        let request = Request::StdinResponse {
            id: self.next_request_id,
            request_id: request_id.to_string(),
            input: input.to_string(),
        };
        self.next_request_id += 1;
        self.send_request(request).await
    }

    /// Cancel the current generation on the server
    pub async fn cancel(&mut self) -> Result<()> {
        let request = Request::Cancel {
            id: self.next_request_id,
        };
        self.next_request_id += 1;
        self.send_request(request).await
    }

    /// Move the currently executing tool to background
    pub async fn background_tool(&mut self) -> Result<()> {
        let request = Request::BackgroundTool {
            id: self.next_request_id,
        };
        self.next_request_id += 1;
        self.send_request(request).await
    }

    /// Queue a soft interrupt message to be injected at the next safe point
    /// This doesn't cancel anything - the message is naturally incorporated
    pub async fn soft_interrupt(&mut self, content: String, urgent: bool) -> Result<u64> {
        let id = self.next_request_id;
        let request = Request::SoftInterrupt {
            id,
            content,
            urgent,
        };
        self.next_request_id += 1;
        self.send_request(request).await?;
        Ok(id)
    }

    pub async fn cancel_soft_interrupts(&mut self) -> Result<()> {
        let request = Request::CancelSoftInterrupts {
            id: self.next_request_id,
        };
        self.next_request_id += 1;
        self.send_request(request).await
    }

    /// Split the current session — ask server to clone conversation into a new session
    pub async fn split(&mut self) -> Result<u64> {
        let id = self.next_request_id;
        let request = Request::Split { id };
        self.next_request_id += 1;
        self.send_request(request).await?;
        Ok(id)
    }

    /// Transfer the current session into a compacted handoff session
    pub async fn transfer(&mut self) -> Result<u64> {
        let id = self.next_request_id;
        let request = Request::Transfer { id };
        self.next_request_id += 1;
        self.send_request(request).await?;
        Ok(id)
    }

    /// Trigger manual context compaction on the server
    pub async fn compact(&mut self) -> Result<u64> {
        let id = self.next_request_id;
        let request = Request::Compact { id };
        self.next_request_id += 1;
        self.send_request(request).await?;
        Ok(id)
    }

    /// Trigger immediate memory extraction on the server for the active session.
    pub async fn trigger_memory_extraction(&mut self) -> Result<()> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        self.send_request(Request::TriggerMemoryExtraction { id })
            .await
    }

    /// Notify the server that auth credentials changed (e.g., after login)
    pub async fn notify_auth_changed(&mut self) -> Result<()> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        self.send_request(Request::NotifyAuthChanged { id }).await
    }

    /// Notify the server about auth changes without blocking the caller.
    pub fn notify_auth_changed_detached(&mut self) {
        let id = self.next_request_id;
        self.next_request_id += 1;
        self.send_request_detached(Request::NotifyAuthChanged { id }, "notify_auth_changed");
    }

    /// Ask server to switch active Anthropic account for this process/session.
    pub async fn switch_anthropic_account(&mut self, label: &str) -> Result<()> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        self.send_request(Request::SwitchAnthropicAccount {
            id,
            label: label.to_string(),
        })
        .await
    }

    /// Ask server to switch active OpenAI account for this process/session.
    pub async fn switch_openai_account(&mut self, label: &str) -> Result<()> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        self.send_request(Request::SwitchOpenAiAccount {
            id,
            label: label.to_string(),
        })
        .await
    }

    /// Send a response for a client debug request
    pub async fn send_client_debug_response(&mut self, id: u64, output: String) -> Result<()> {
        self.send_request(Request::ClientDebugResponse { id, output })
            .await
    }

    /// Read the next event from the server.
    pub async fn next_event(&mut self) -> RemoteRead {
        loop {
            self.line_buffer.clear();
            match self.reader.read_line(&mut self.line_buffer).await {
                Ok(0) => {
                    crate::logging::warn(&format!(
                        "RemoteConnection::next_event: peer closed (session_id={:?}, client_instance_id={:?})",
                        self.session_id, self.client_instance_id
                    ));
                    return RemoteRead::Disconnected(RemoteDisconnectReason::PeerClosed);
                }
                Ok(_) => {
                    if self.line_buffer.trim().is_empty() {
                        crate::logging::warn(&format!(
                            "RemoteConnection::next_event: skipping blank line (session_id={:?}, client_instance_id={:?})",
                            self.session_id, self.client_instance_id
                        ));
                        continue;
                    }
                    match serde_json::from_str(&self.line_buffer) {
                        Ok(event) => return RemoteRead::Event(event),
                        Err(error) => {
                            crate::logging::warn(&format!(
                                "RemoteConnection::next_event: protocol error={} line={:?} (session_id={:?}, client_instance_id={:?})",
                                error, self.line_buffer, self.session_id, self.client_instance_id
                            ));
                            return RemoteRead::Disconnected(RemoteDisconnectReason::Protocol(
                                error.to_string(),
                            ));
                        }
                    }
                }
                Err(error) => {
                    crate::logging::warn(&format!(
                        "RemoteConnection::next_event: io error={} (session_id={:?}, client_instance_id={:?})",
                        error, self.session_id, self.client_instance_id
                    ));
                    return RemoteRead::Disconnected(RemoteDisconnectReason::Io(error.to_string()));
                }
            }
        }
    }

    /// Get writer for sending requests
    pub fn writer(&self) -> Arc<Mutex<WriteHalf>> {
        Arc::clone(&self.writer)
    }

    /// Get session ID
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Create a dummy RemoteConnection for replay mode (no real server)
    #[cfg(test)]
    pub fn dummy() -> Self {
        let (a, b) = crate::transport::Stream::pair()
            .unwrap_or_else(|err| panic!("failed to create dummy socketpair for tests: {}", err));
        let (reader, writer) = a.into_split();
        Self {
            reader: BufReader::new(reader),
            writer: Arc::new(Mutex::new(writer)),
            _dummy_peer: Some(b),
            session_id: None,
            client_instance_id: None,
            next_request_id: 1,
            tool_diff: RemoteDiffTracker::default(),
            line_buffer: String::new(),
            has_loaded_history: false,
            call_output_tokens_seen: 0,
        }
    }

    /// Set session ID
    pub fn set_session_id(&mut self, id: String) {
        self.session_id = Some(id);
    }

    /// Check if history has been loaded
    pub fn has_loaded_history(&self) -> bool {
        self.has_loaded_history
    }

    /// Mark history as loaded
    pub fn mark_history_loaded(&mut self) {
        self.has_loaded_history = true;
    }

    /// Handle tool start - begin tracking for diff generation
    pub fn handle_tool_start(&mut self, id: &str, name: &str) {
        self.tool_diff.handle_tool_start(id, name);
    }

    /// Handle tool input delta
    pub fn handle_tool_input(&mut self, delta: &str) {
        self.tool_diff.handle_tool_input(delta);
    }

    /// Get parsed current tool input (before it's cleared in handle_tool_exec)
    pub fn get_current_tool_input(&self) -> serde_json::Value {
        self.tool_diff.current_tool_input_json()
    }

    /// Handle tool exec - cache file content if edit/write
    pub fn handle_tool_exec(&mut self, id: &str, name: &str) {
        self.tool_diff.handle_tool_exec(id, name);
    }

    /// Handle tool done - generate diff if we have pending data
    pub fn handle_tool_done(&mut self, id: &str, name: &str, output: &str) -> String {
        self.tool_diff.finish_tool(id, name, output)
    }

    /// Clear pending diff state
    pub fn clear_pending(&mut self) {
        self.tool_diff.clear();
    }

    /// Per-API-call output token watermark (for TPS delta accumulation).
    pub fn call_output_tokens_seen(&mut self) -> &mut u64 {
        &mut self.call_output_tokens_seen
    }

    /// Reset per-call output token watermark.
    pub fn reset_call_output_tokens_seen(&mut self) {
        self.call_output_tokens_seen = 0;
    }
}

impl RemoteEventState for RemoteConnection {
    fn handle_tool_start(&mut self, id: &str, name: &str) {
        Self::handle_tool_start(self, id, name);
    }

    fn handle_tool_input(&mut self, delta: &str) {
        Self::handle_tool_input(self, delta);
    }

    fn get_current_tool_input(&self) -> serde_json::Value {
        Self::get_current_tool_input(self)
    }

    fn handle_tool_exec(&mut self, id: &str, name: &str) {
        Self::handle_tool_exec(self, id, name);
    }

    fn handle_tool_done(&mut self, id: &str, name: &str, output: &str) -> String {
        Self::handle_tool_done(self, id, name, output)
    }

    fn clear_pending(&mut self) {
        Self::clear_pending(self);
    }

    fn call_output_tokens_seen(&mut self) -> &mut u64 {
        Self::call_output_tokens_seen(self)
    }

    fn reset_call_output_tokens_seen(&mut self) {
        Self::reset_call_output_tokens_seen(self);
    }

    fn set_session_id(&mut self, id: String) {
        Self::set_session_id(self, id);
    }

    fn has_loaded_history(&self) -> bool {
        Self::has_loaded_history(self)
    }

    fn mark_history_loaded(&mut self) {
        Self::mark_history_loaded(self);
    }
}

impl RemoteEventState for ReplayRemoteState {
    fn handle_tool_start(&mut self, id: &str, name: &str) {
        self.tool_diff.handle_tool_start(id, name);
    }

    fn handle_tool_input(&mut self, delta: &str) {
        self.tool_diff.handle_tool_input(delta);
    }

    fn get_current_tool_input(&self) -> serde_json::Value {
        self.tool_diff.current_tool_input_json()
    }

    fn handle_tool_exec(&mut self, id: &str, name: &str) {
        self.tool_diff.handle_tool_exec(id, name);
    }

    fn handle_tool_done(&mut self, id: &str, name: &str, output: &str) -> String {
        self.tool_diff.finish_tool(id, name, output)
    }

    fn clear_pending(&mut self) {
        self.tool_diff.clear();
    }

    fn call_output_tokens_seen(&mut self) -> &mut u64 {
        &mut self.call_output_tokens_seen
    }

    fn reset_call_output_tokens_seen(&mut self) {
        self.call_output_tokens_seen = 0;
    }

    fn set_session_id(&mut self, _id: String) {}

    fn has_loaded_history(&self) -> bool {
        true
    }

    fn mark_history_loaded(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn detached_auth_changed_notification_does_not_wait_for_writer_lock() {
        let mut remote = RemoteConnection::dummy();
        let writer = remote.writer();
        let _guard = writer.lock().await;

        let start = Instant::now();
        remote.notify_auth_changed_detached();
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_millis(50),
            "detached notify_auth_changed should return immediately, took {:?}",
            elapsed
        );
        assert_eq!(remote.next_request_id, 2);
    }

    #[tokio::test]
    async fn clear_sends_clear_request_to_remote_server() {
        let mut remote = RemoteConnection::dummy();
        let peer = remote
            ._dummy_peer
            .take()
            .expect("dummy remote should retain peer stream");
        let (reader, _writer) = peer.into_split();
        let mut reader = BufReader::new(reader);

        let request_id = remote.clear().await.expect("clear request should send");

        let mut line = String::new();
        reader
            .read_line(&mut line)
            .await
            .expect("clear request should be readable by peer");
        assert_eq!(request_id, 1);
        assert_eq!(remote.next_request_id, 2);
        assert!(matches!(
            serde_json::from_str::<Request>(&line).expect("clear request should deserialize"),
            Request::Clear { id: 1 }
        ));
    }
}
