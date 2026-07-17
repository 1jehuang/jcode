//! Backend abstraction for TUI runtime transports.
//!
//! This module provides a unified interface for message processing across
//! local harnesses and server-backed remote clients.
//!
//! Also provides debug socket events for exposing full TUI state.

use crate::message::ToolCall;
use crate::protocol::{AuthChanged, FeatureToggle, Request, ServerEvent};
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

/// Classification of a single decoded protocol line read from the server
/// stream. Used by [`RemoteConnection::next_event`] to keep the cancellation-
/// safe read loop readable. The event is boxed because [`ServerEvent`] is large.
enum LineOutcome {
    Event(Box<ServerEvent>),
    Skip,
    Disconnect(RemoteDisconnectReason),
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
    /// Bytes pulled from the socket that have not yet been split into complete
    /// newline-delimited protocol lines. This buffer is persistent across
    /// `next_event` calls so a future cancelled by a `tokio::select!` peer
    /// branch never loses partially-read bytes.
    read_buffer: Vec<u8>,
    /// First byte in `read_buffer` that has not yet been checked for a newline.
    ///
    /// Large History events can be tens of megabytes and arrive over thousands
    /// of socket reads. Searching from byte zero after every read makes framing
    /// quadratic and eventually backpressures the server writer. Keeping this
    /// cursor makes each received byte participate in at most one newline scan.
    read_buffer_scan_start: usize,
    #[cfg(test)]
    protocol_bytes_scanned: usize,
    has_loaded_history: bool,
    call_output_tokens_seen: u64,
}

const DETACHED_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_STRAY_REMOTE_PROTOCOL_LINES: usize = 32;
/// Capacity above which the persistent `read_buffer` is considered oversized
/// once its backlog drains. A single multi-megabyte protocol line (e.g. a
/// `History` event with embedded images) would otherwise pin that capacity
/// for the lifetime of the connection.
const READ_BUFFER_SHRINK_THRESHOLD: usize = 256 * 1024;
/// Capacity retained after shrinking an oversized `read_buffer`. Comfortably
/// above typical streaming line sizes so steady-state traffic never causes
/// grow/shrink thrash.
const READ_BUFFER_RETAIN_CAPACITY: usize = 64 * 1024;

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
        Self::connect_with_session(None, None, false, false, None).await
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
        remote_working_dir: Option<&str>,
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
            read_buffer: Vec::new(),
            read_buffer_scan_start: 0,
            #[cfg(test)]
            protocol_bytes_scanned: 0,
            has_loaded_history: false,
            call_output_tokens_seen: 0,
        };

        // Subscribe to events
        let subscribe_start = Instant::now();
        let (working_dir, selfdev) = super::subscribe_metadata(remote_working_dir);
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
            terminal_env: crate::terminal_launch::snapshot_client_terminal_env(),
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
        // Avoid a reconnect/reload thundering herd: every headed client used to
        // request the full expanded model catalog immediately after attach. On
        // large OpenRouter catalogs this is ~800KB per client and can make many
        // TUI processes parse/render at once, which showed up as multi-second
        // draw stalls during scrolling. The TUI hydrates the persisted remote
        // catalog cache for normal `/model` use; explicit refresh paths still
        // request fresh catalog data when needed.
        if std::env::var_os("JCODE_REMOTE_BOOTSTRAP_MODEL_CATALOG").is_some() {
            conn.send_request(Request::GetModelCatalog {
                id: conn.next_request_id,
            })
            .await?;
            conn.next_request_id += 1;
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

    fn interrupt_request_log_fields(
        &self,
        request: &Request,
        trigger: Option<&str>,
    ) -> Option<String> {
        let trigger = trigger.unwrap_or("unspecified");
        let base = |kind: &str, id: u64| {
            format!(
                "kind={} id={} trigger={} session={:?} client_instance={:?}",
                kind, id, trigger, self.session_id, self.client_instance_id
            )
        };

        match request {
            Request::Cancel { id } => Some(base("cancel", *id)),
            Request::SoftInterrupt {
                id,
                content,
                urgent,
            } => Some(format!(
                "{} urgent={} content_bytes={} content_chars={}",
                base("soft_interrupt", *id),
                urgent,
                content.len(),
                content.chars().count()
            )),
            Request::CancelSoftInterrupts { id } => Some(base("cancel_soft_interrupts", *id)),
            Request::BackgroundTool { id } => Some(base("background_tool", *id)),
            _ => None,
        }
    }

    async fn send_request_with_interrupt_trigger(
        &self,
        request: Request,
        interrupt_trigger: Option<&str>,
    ) -> Result<()> {
        let json = serde_json::to_string(&request)? + "\n";
        let interrupt_log = self.interrupt_request_log_fields(&request, interrupt_trigger);
        if let Some(fields) = &interrupt_log {
            crate::logging::info(&format!(
                "REMOTE_INTERRUPT_SEND_START {} json_bytes={}",
                fields,
                json.len()
            ));
        }

        let total_start = Instant::now();
        let writer_wait_start = Instant::now();
        let mut w = self.writer.lock().await;
        let writer_wait_ms = writer_wait_start.elapsed().as_millis();
        let write_start = Instant::now();
        let result = w.write_all(json.as_bytes()).await;
        if let Some(fields) = &interrupt_log {
            match &result {
                Ok(()) => crate::logging::info(&format!(
                    "REMOTE_INTERRUPT_SEND_OK {} writer_wait_ms={} write_ms={} total_ms={}",
                    fields,
                    writer_wait_ms,
                    write_start.elapsed().as_millis(),
                    total_start.elapsed().as_millis()
                )),
                Err(error) => crate::logging::warn(&format!(
                    "REMOTE_INTERRUPT_SEND_ERR {} writer_wait_ms={} write_ms={} total_ms={} error={}",
                    fields,
                    writer_wait_ms,
                    write_start.elapsed().as_millis(),
                    total_start.elapsed().as_millis(),
                    error
                )),
            }
        }
        result?;
        Ok(())
    }

    async fn send_request(&self, request: Request) -> Result<()> {
        self.send_request_with_interrupt_trigger(request, None)
            .await
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
            force: true,
        };
        self.next_request_id += 1;
        self.send_request(request).await
    }

    /// Ask the server to continue every live session that was interrupted and
    /// would auto-resume on a reload. Returns the request id so the client can
    /// correlate the `ResumeAllResult` event.
    pub async fn resume_all_sessions(&mut self) -> Result<u64> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        self.send_request(Request::ResumeAllSessions { id }).await?;
        Ok(id)
    }

    /// Re-request the session history payload from the server.
    ///
    /// Used by the client-side history-recovery watchdog: if the bootstrap
    /// `History` event never arrives after a (re)connect (e.g. it was dropped
    /// during a reload handoff, or the server was momentarily busy), the client
    /// would otherwise be stuck forever on "loading session…" with every prompt
    /// gated behind `has_loaded_history()`. Sending a fresh `GetHistory` lets the
    /// server resend the payload so the session can recover without a `/restart`.
    pub async fn request_history(&mut self) -> Result<u64> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        self.send_request(Request::GetHistory { id }).await?;
        Ok(id)
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
    pub async fn set_model(&mut self, model: &str) -> Result<u64> {
        let id = self.next_request_id;
        let request = Request::SetModel {
            id,
            model: model.to_string(),
        };
        self.next_request_id += 1;
        self.send_request(request).await?;
        Ok(id)
    }

    pub async fn set_route_selection(
        &mut self,
        selection: crate::provider::RouteSelection,
    ) -> Result<u64> {
        let id = self.next_request_id;
        let request = Request::SetRoute { id, selection };
        self.next_request_id += 1;
        self.send_request(request).await?;
        Ok(id)
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
            target_session_id: None,
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
        self.cancel_with_reason("remote.cancel").await
    }

    /// Cancel the current generation on the server, tagging logs with the UI trigger.
    pub async fn cancel_with_reason(&mut self, reason: &'static str) -> Result<()> {
        let request = Request::Cancel {
            id: self.next_request_id,
        };
        self.next_request_id += 1;
        self.send_request_with_interrupt_trigger(request, Some(reason))
            .await
    }

    /// Move the currently executing tool to background
    pub async fn background_tool(&mut self) -> Result<()> {
        let request = Request::BackgroundTool {
            id: self.next_request_id,
        };
        self.next_request_id += 1;
        self.send_request_with_interrupt_trigger(request, Some("background_tool"))
            .await
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
        self.send_request_with_interrupt_trigger(request, Some("soft_interrupt"))
            .await?;
        Ok(id)
    }

    pub async fn cancel_soft_interrupts(&mut self) -> Result<()> {
        let request = Request::CancelSoftInterrupts {
            id: self.next_request_id,
        };
        self.next_request_id += 1;
        self.send_request_with_interrupt_trigger(request, Some("cancel_soft_interrupts"))
            .await
    }

    /// Split the current session - ask server to clone conversation into a new session
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
        self.send_request(Request::NotifyAuthChanged {
            id,
            provider: None,
            auth: None,
            prefer_strongest: false,
        })
        .await
    }

    /// Notify the server about a typed auth lifecycle change and report
    /// transport failure to the caller.
    pub async fn notify_auth_changed_event(
        &mut self,
        provider: Option<&str>,
        auth: Option<AuthChanged>,
        prefer_strongest: bool,
    ) -> Result<()> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        self.send_request(Request::NotifyAuthChanged {
            id,
            provider: provider.map(str::to_string),
            auth,
            prefer_strongest,
        })
        .await
    }

    /// Notify the server about auth changes without blocking the caller.
    pub fn notify_auth_changed_detached(&mut self) {
        self.notify_auth_changed_for_provider_detached(None);
    }

    /// Notify the server about a provider-specific auth change without blocking the caller.
    pub fn notify_auth_changed_for_provider_detached(&mut self, provider: Option<&str>) {
        self.notify_auth_changed_detached_event(provider, None, false);
    }

    /// Notify the server about a typed auth lifecycle change without blocking the caller.
    pub fn notify_auth_changed_detached_event(
        &mut self,
        provider: Option<&str>,
        auth: Option<AuthChanged>,
        prefer_strongest: bool,
    ) {
        let id = self.next_request_id;
        self.next_request_id += 1;
        self.send_request_detached(
            Request::NotifyAuthChanged {
                id,
                provider: provider.map(str::to_string),
                auth,
                prefer_strongest,
            },
            "notify_auth_changed",
        );
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
    ///
    /// This is **cancellation safe** and may be used directly as a branch in a
    /// `tokio::select!`. Every byte pulled from the socket is moved into the
    /// persistent [`Self::read_buffer`] immediately, and the only `.await`
    /// point is [`AsyncBufReadExt::fill_buf`], which tokio guarantees reads
    /// nothing when its future is dropped. A previous implementation used
    /// `BufReader::read_line`, which tokio documents as **not** cancellation
    /// safe: when a large payload (e.g. a multi-megabyte `History` event for a
    /// session with images) was mid-read and a sibling `select!` branch (a
    /// redraw tick, terminal event, or bus event) completed first, the
    /// `read_line` future was dropped and the bytes it had already consumed
    /// from the socket were lost. That desynced the newline-framed protocol
    /// stream: the next read began mid-payload, surfaced as "stray non-JSON
    /// protocol line" warnings, the real `History` was discarded, and the
    /// session stayed stuck on "loading session…" until a manual `/restart`.
    pub async fn next_event(&mut self) -> RemoteRead {
        let mut stray_lines = 0usize;
        loop {
            // Serve any complete line already buffered before touching the
            // socket. This drains backlog left by a previous (possibly
            // cancelled) call before issuing a new read.
            if let Some(line) = self.take_buffered_line() {
                match self.classify_protocol_line(line, &mut stray_lines) {
                    LineOutcome::Event(event) => return RemoteRead::Event(*event),
                    LineOutcome::Skip => continue,
                    LineOutcome::Disconnect(reason) => return RemoteRead::Disconnected(reason),
                }
            }

            // No complete line yet: pull more bytes. `fill_buf` is cancel safe,
            // so if a `select!` peer wins the race here nothing is read or lost.
            let chunk = match self.reader.fill_buf().await {
                Ok(buf) => buf,
                Err(error) => {
                    crate::logging::warn(&format!(
                        "RemoteConnection::next_event: io error={} (session_id={:?}, client_instance_id={:?})",
                        error, self.session_id, self.client_instance_id
                    ));
                    return RemoteRead::Disconnected(RemoteDisconnectReason::Io(error.to_string()));
                }
            };
            if chunk.is_empty() {
                // EOF. Surface any trailing partial line for diagnostics, then
                // report a clean peer-closed disconnect.
                if self.read_buffer.is_empty() {
                    crate::logging::warn(&format!(
                        "RemoteConnection::next_event: peer closed (session_id={:?}, client_instance_id={:?})",
                        self.session_id, self.client_instance_id
                    ));
                } else {
                    let preview: String = String::from_utf8_lossy(&self.read_buffer)
                        .chars()
                        .take(240)
                        .collect();
                    crate::logging::warn(&format!(
                        "RemoteConnection::next_event: peer closed mid-line, discarding {} buffered bytes preview={:?} (session_id={:?}, client_instance_id={:?})",
                        self.read_buffer.len(),
                        preview,
                        self.session_id,
                        self.client_instance_id
                    ));
                    self.read_buffer.clear();
                    self.read_buffer_scan_start = 0;
                }
                return RemoteRead::Disconnected(RemoteDisconnectReason::PeerClosed);
            }
            let len = chunk.len();
            self.read_buffer.extend_from_slice(chunk);
            self.reader.consume(len);
        }
    }

    /// Split off the next complete newline-delimited line (without the trailing
    /// `\n`) from the persistent read buffer, leaving any partial remainder in
    /// place for the next read.
    fn take_buffered_line(&mut self) -> Option<Vec<u8>> {
        // Only inspect bytes appended since the previous unsuccessful scan.
        // Without this cursor, a 27 MB History line delivered in 8 KB chunks
        // causes roughly 45 GB of redundant memory scanning.
        let scan_start = self.read_buffer_scan_start.min(self.read_buffer.len());
        let unscanned = &self.read_buffer[scan_start..];
        let relative_newline = unscanned.iter().position(|&b| b == b'\n');
        #[cfg(test)]
        {
            self.protocol_bytes_scanned += relative_newline.map_or(unscanned.len(), |i| i + 1);
        }
        let Some(relative_newline) = relative_newline else {
            self.read_buffer_scan_start = self.read_buffer.len();
            return None;
        };
        let newline = scan_start + relative_newline;
        let mut line: Vec<u8> = self.read_buffer.drain(..=newline).collect();
        line.pop(); // drop trailing '\n'
        // `position` stopped at the first newline, so none of the remaining
        // bytes have been inspected yet.
        self.read_buffer_scan_start = 0;
        // A single oversized line (e.g. a multi-megabyte `History` event)
        // permanently grows this persistent buffer. Once the line has been
        // split off, release the excess so each connection returns to a small
        // steady-state footprint. The mostly-unused check (len < cap/4) plus
        // the retain floor keep normal streaming from ever reallocating.
        if self.read_buffer.capacity() > READ_BUFFER_SHRINK_THRESHOLD
            && self.read_buffer.len() < self.read_buffer.capacity() / 4
        {
            self.read_buffer
                .shrink_to(self.read_buffer.len().max(READ_BUFFER_RETAIN_CAPACITY));
        }
        Some(line)
    }

    /// Decide what a single decoded protocol line means: a real event, a line
    /// to skip (blank/stray), or a fatal protocol/transport error.
    fn classify_protocol_line(&self, line: Vec<u8>, stray_lines: &mut usize) -> LineOutcome {
        let text = match String::from_utf8(line) {
            Ok(text) => text,
            Err(error) => {
                *stray_lines += 1;
                let preview: String = String::from_utf8_lossy(error.as_bytes())
                    .chars()
                    .take(240)
                    .collect();
                crate::logging::warn(&format!(
                    "RemoteConnection::next_event: skipping stray non-UTF-8 protocol line {}/{} preview={:?} (session_id={:?}, client_instance_id={:?})",
                    *stray_lines,
                    MAX_STRAY_REMOTE_PROTOCOL_LINES,
                    preview,
                    self.session_id,
                    self.client_instance_id
                ));
                if *stray_lines >= MAX_STRAY_REMOTE_PROTOCOL_LINES {
                    return LineOutcome::Disconnect(RemoteDisconnectReason::Protocol(
                        "too many stray non-JSON protocol lines".to_string(),
                    ));
                }
                return LineOutcome::Skip;
            }
        };
        let trimmed = text.trim_start();
        if trimmed.trim().is_empty() {
            crate::logging::warn(&format!(
                "RemoteConnection::next_event: skipping blank line (session_id={:?}, client_instance_id={:?})",
                self.session_id, self.client_instance_id
            ));
            return LineOutcome::Skip;
        }
        if !trimmed.starts_with('{') {
            *stray_lines += 1;
            let preview: String = text.chars().take(240).collect();
            crate::logging::warn(&format!(
                "RemoteConnection::next_event: skipping stray non-JSON protocol line {}/{} preview={:?} (session_id={:?}, client_instance_id={:?})",
                *stray_lines,
                MAX_STRAY_REMOTE_PROTOCOL_LINES,
                preview,
                self.session_id,
                self.client_instance_id
            ));
            if *stray_lines >= MAX_STRAY_REMOTE_PROTOCOL_LINES {
                return LineOutcome::Disconnect(RemoteDisconnectReason::Protocol(
                    "too many stray non-JSON protocol lines".to_string(),
                ));
            }
            return LineOutcome::Skip;
        }
        match serde_json::from_str(&text) {
            Ok(event) => LineOutcome::Event(Box::new(event)),
            Err(error) => {
                // A single unparseable JSON line (e.g. the tail half of a frame
                // split by a lost write, or an event variant this client build
                // doesn't know) must not kill the whole session. Count it
                // against the stray-line budget and resync on the next line;
                // only give up if the stream keeps failing to parse (which
                // indicates a real protocol/version mismatch). See issue #422:
                // huge sessions used to die permanently on one corrupt frame.
                *stray_lines += 1;
                let preview: String = text.chars().take(240).collect();
                crate::logging::warn(&format!(
                    "RemoteConnection::next_event: skipping unparseable protocol line {}/{} error={} preview={:?} (session_id={:?}, client_instance_id={:?})",
                    *stray_lines,
                    MAX_STRAY_REMOTE_PROTOCOL_LINES,
                    error,
                    preview,
                    self.session_id,
                    self.client_instance_id
                ));
                if *stray_lines >= MAX_STRAY_REMOTE_PROTOCOL_LINES {
                    return LineOutcome::Disconnect(RemoteDisconnectReason::Protocol(format!(
                        "too many unparseable protocol lines; last error: {}",
                        error
                    )));
                }
                LineOutcome::Skip
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
            read_buffer: Vec::new(),
            read_buffer_scan_start: 0,
            #[cfg(test)]
            protocol_bytes_scanned: 0,
            has_loaded_history: false,
            call_output_tokens_seen: 0,
        }
    }

    #[cfg(test)]
    pub(crate) fn take_dummy_peer(&mut self) -> Option<Stream> {
        self._dummy_peer.take()
    }

    /// Set session ID
    pub fn set_session_id(&mut self, id: String) {
        self.session_id = Some(id);
    }

    /// Check if history has been loaded
    pub fn has_loaded_history(&self) -> bool {
        self.has_loaded_history
    }

    /// Whether the socket reader already holds part of an inbound protocol
    /// frame. A history recovery request must not be sent in this state: the
    /// original response is in flight, and another request only queues another
    /// full copy behind it.
    pub fn has_buffered_inbound_frame(&self) -> bool {
        !self.read_buffer.is_empty()
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
    async fn detached_auth_changed_notification_sends_provider_hint() {
        let mut remote = RemoteConnection::dummy();
        let peer = remote
            ._dummy_peer
            .take()
            .expect("dummy remote should retain peer stream");
        let (reader, _writer) = peer.into_split();
        let mut reader = BufReader::new(reader);

        remote.notify_auth_changed_for_provider_detached(Some("azure-openai"));

        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(1), reader.read_line(&mut line))
            .await
            .expect("auth changed request should be sent before timeout")
            .expect("auth changed request should be readable by peer");

        assert_eq!(remote.next_request_id, 2);
        assert!(matches!(
            serde_json::from_str::<Request>(&line).expect("auth changed request should deserialize"),
            Request::NotifyAuthChanged {
                id: 1,
                provider: Some(provider),
                auth: None,
                prefer_strongest: false,
            } if provider == "azure-openai"
        ));
    }

    #[tokio::test]
    async fn next_event_skips_stray_non_json_lines_before_valid_event() {
        let mut remote = RemoteConnection::dummy();
        let peer = remote
            ._dummy_peer
            .take()
            .expect("dummy remote should retain peer stream");
        let (_reader, mut writer) = peer.into_split();

        writer
            .write_all(b"raw tool output leaked onto protocol\n")
            .await
            .expect("stray line should write");
        writer
            .write_all(crate::protocol::encode_event(&ServerEvent::Done { id: 7 }).as_bytes())
            .await
            .expect("valid event should write");

        match remote.next_event().await {
            RemoteRead::Event(ServerEvent::Done { id }) => assert_eq!(id, 7),
            other => panic!("expected Done event after stray line, got {other:?}"),
        }
    }

    /// Regression for issue #422: a single corrupt frame (e.g. the tail half of
    /// a split multi-megabyte event, or an event variant this client build does
    /// not know) must not permanently kill the session. The client should skip
    /// the bad line, resync on the next newline, and deliver the next valid
    /// event.
    #[tokio::test]
    async fn next_event_skips_corrupt_json_frame_and_recovers() {
        let mut remote = RemoteConnection::dummy();
        let peer = remote
            ._dummy_peer
            .take()
            .expect("dummy remote should retain peer stream");
        let (_reader, mut writer) = peer.into_split();

        // Corrupt JSON that passes the '{' prefix check but fails to parse.
        writer
            .write_all(b"{\"type\":\"done\",\"id\":\n")
            .await
            .expect("corrupt frame should write");
        // Valid JSON that is not a ServerEvent (unknown variant / wrong shape).
        writer
            .write_all(b"{\"type\":\"event_from_a_newer_server_version\"}\n")
            .await
            .expect("unknown-variant frame should write");
        writer
            .write_all(crate::protocol::encode_event(&ServerEvent::Done { id: 9 }).as_bytes())
            .await
            .expect("valid event should write");

        match remote.next_event().await {
            RemoteRead::Event(ServerEvent::Done { id }) => assert_eq!(id, 9),
            other => panic!("expected Done event after corrupt frames, got {other:?}"),
        }
    }

    /// A stream that keeps failing to parse (real protocol/version mismatch)
    /// must still disconnect once the stray-line budget is exhausted, instead
    /// of spinning forever.
    #[tokio::test]
    async fn next_event_disconnects_after_too_many_corrupt_json_frames() {
        let mut remote = RemoteConnection::dummy();
        let peer = remote
            ._dummy_peer
            .take()
            .expect("dummy remote should retain peer stream");
        let (_reader, mut writer) = peer.into_split();

        for _ in 0..MAX_STRAY_REMOTE_PROTOCOL_LINES {
            writer
                .write_all(b"{\"not\":\"a server event\"}\n")
                .await
                .expect("corrupt frame should write");
        }

        match remote.next_event().await {
            RemoteRead::Disconnected(RemoteDisconnectReason::Protocol(message)) => {
                assert!(
                    message.contains("too many unparseable protocol lines"),
                    "unexpected protocol disconnect message: {message}"
                );
            }
            other => panic!("expected protocol disconnect after budget exhaustion, got {other:?}"),
        }
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

    /// Regression test for the "stuck on loading session…" bug.
    ///
    /// `next_event` runs as a branch in the client `tokio::select!`. If it were
    /// not cancellation safe, a large `History` payload that is mid-read when a
    /// peer branch (redraw tick, terminal event) wins the race would lose the
    /// bytes already consumed from the socket and desync the protocol stream.
    /// Here we cancel `next_event` repeatedly while a large event is still
    /// streaming in, then confirm the event is delivered intact.
    #[tokio::test]
    async fn next_event_is_cancellation_safe_for_large_payloads() {
        let mut remote = RemoteConnection::dummy();
        let peer = remote
            ._dummy_peer
            .take()
            .expect("dummy remote should retain peer stream");
        let (_reader, mut writer) = peer.into_split();

        // A History-sized payload: a single string field large enough to span
        // many socket reads, mimicking the multi-megabyte base64 image data
        // carried by a real `History` event for an image-bearing session.
        let big_text = "x".repeat(2 * 1024 * 1024);
        let event = ServerEvent::StatusDetail {
            detail: big_text.clone(),
        };
        let encoded = crate::protocol::encode_event(&event);
        let encoded_len = encoded.len();

        // Feed the encoded event in small chunks from a background task, so the
        // reader sees a partially-available line for most of the test.
        let writer_task = tokio::spawn(async move {
            for chunk in encoded.as_bytes().chunks(4096) {
                writer
                    .write_all(chunk)
                    .await
                    .expect("chunk should write to peer");
                // Yield so the reader gets a chance to observe a partial line.
                tokio::task::yield_now().await;
            }
        });

        // Repeatedly start and immediately cancel `next_event` (the `select!`
        // peer "wins" via a zero-delay timeout) until the full line arrives.
        let event = loop {
            tokio::select! {
                biased;
                read = remote.next_event() => break read,
                _ = tokio::time::sleep(Duration::from_micros(50)) => {
                    // Cancellation point: the in-flight `next_event` future is
                    // dropped here. A cancellation-unsafe reader would lose
                    // buffered bytes and never reassemble the event.
                }
            }
        };

        writer_task.await.expect("writer task should finish");

        match event {
            RemoteRead::Event(ServerEvent::StatusDetail { detail }) => {
                assert_eq!(
                    detail.len(),
                    big_text.len(),
                    "large payload must survive repeated cancellations intact"
                );
                assert!(detail.bytes().all(|b| b == b'x'));
            }
            other => panic!("expected intact event after cancellations, got {other:?}"),
        }
        assert_eq!(
            remote.protocol_bytes_scanned, encoded_len,
            "fragmented frame assembly must inspect each protocol byte exactly once"
        );
    }

    /// A single logical event split across multiple socket writes (no trailing
    /// newline until the end) must be reassembled into one event.
    #[tokio::test]
    async fn next_event_reassembles_event_split_across_reads() {
        let mut remote = RemoteConnection::dummy();
        let peer = remote
            ._dummy_peer
            .take()
            .expect("dummy remote should retain peer stream");
        let (_reader, mut writer) = peer.into_split();

        let encoded = crate::protocol::encode_event(&ServerEvent::Done { id: 9 });
        let bytes = encoded.as_bytes();
        let mid = bytes.len() / 2;
        writer
            .write_all(&bytes[..mid])
            .await
            .expect("first half should write");
        // Give the reader a chance to observe the partial line.
        tokio::task::yield_now().await;
        writer
            .write_all(&bytes[mid..])
            .await
            .expect("second half should write");

        match remote.next_event().await {
            RemoteRead::Event(ServerEvent::Done { id }) => assert_eq!(id, 9),
            other => panic!("expected reassembled Done event, got {other:?}"),
        }
    }

    /// Two events delivered back-to-back in a single socket write must both be
    /// returned, with the second served from the buffer without another read.
    #[tokio::test]
    async fn next_event_serves_multiple_buffered_events() {
        let mut remote = RemoteConnection::dummy();
        let peer = remote
            ._dummy_peer
            .take()
            .expect("dummy remote should retain peer stream");
        let (_reader, mut writer) = peer.into_split();

        let mut payload = crate::protocol::encode_event(&ServerEvent::Done { id: 1 });
        payload.push_str(&crate::protocol::encode_event(&ServerEvent::Done { id: 2 }));
        writer
            .write_all(payload.as_bytes())
            .await
            .expect("both events should write in one chunk");
        drop(writer);

        match remote.next_event().await {
            RemoteRead::Event(ServerEvent::Done { id }) => assert_eq!(id, 1),
            other => panic!("expected first Done event, got {other:?}"),
        }
        match remote.next_event().await {
            RemoteRead::Event(ServerEvent::Done { id }) => assert_eq!(id, 2),
            other => panic!("expected second Done event, got {other:?}"),
        }
    }

    /// A single multi-megabyte protocol line (e.g. a `History` event with
    /// embedded images) must not pin its full capacity inside the persistent
    /// `read_buffer` for the rest of the connection. Once the line drains, the
    /// buffer shrinks back to a bounded size, preserving any partial remainder.
    #[tokio::test]
    async fn take_buffered_line_shrinks_oversized_read_buffer() {
        let mut remote = RemoteConnection::dummy();
        let large_len = 4 * 1024 * 1024;
        remote.read_buffer.resize(large_len, b'x');
        remote.read_buffer.push(b'\n');
        // Trailing partial fragment of the next line must survive the shrink.
        remote.read_buffer.extend_from_slice(b"{\"partial");
        assert!(remote.read_buffer.capacity() > READ_BUFFER_SHRINK_THRESHOLD);

        let line = remote
            .take_buffered_line()
            .expect("large buffered line should be returned");
        assert_eq!(line.len(), large_len);
        assert_eq!(remote.read_buffer, b"{\"partial");
        assert!(
            remote.read_buffer.capacity() <= READ_BUFFER_RETAIN_CAPACITY,
            "read_buffer should shrink after a large line drains, capacity={}",
            remote.read_buffer.capacity()
        );
    }

    /// Steady-state streaming buffers (small capacity) must never shrink, so
    /// normal traffic does not thrash between grow and shrink reallocations.
    #[tokio::test]
    async fn take_buffered_line_keeps_capacity_for_small_buffers() {
        let mut remote = RemoteConnection::dummy();
        remote.read_buffer.reserve(32 * 1024);
        let capacity = remote.read_buffer.capacity();
        remote.read_buffer.extend_from_slice(b"hello\n");

        let line = remote
            .take_buffered_line()
            .expect("buffered line should be returned");
        assert_eq!(line, b"hello");
        assert_eq!(
            remote.read_buffer.capacity(),
            capacity,
            "small read_buffer must retain its capacity"
        );
    }

    /// While a large backlog is still buffered (buffer mostly full), capacity
    /// is retained so draining the remaining lines does not reallocate. Only
    /// once the backlog empties out does the buffer shrink.
    #[tokio::test]
    async fn take_buffered_line_keeps_capacity_while_backlog_remains() {
        let mut remote = RemoteConnection::dummy();
        let line_len = 1024 * 1024;
        for _ in 0..3 {
            let start = remote.read_buffer.len();
            remote.read_buffer.resize(start + line_len, b'y');
            remote.read_buffer.push(b'\n');
        }
        let capacity = remote.read_buffer.capacity();

        let first = remote
            .take_buffered_line()
            .expect("first buffered line should be returned");
        assert_eq!(first.len(), line_len);
        assert_eq!(
            remote.read_buffer.capacity(),
            capacity,
            "capacity must be retained while a large backlog remains buffered"
        );

        remote
            .take_buffered_line()
            .expect("second buffered line should be returned");
        remote
            .take_buffered_line()
            .expect("third buffered line should be returned");
        assert!(
            remote.read_buffer.capacity() <= READ_BUFFER_RETAIN_CAPACITY,
            "read_buffer should shrink once the backlog drains, capacity={}",
            remote.read_buffer.capacity()
        );
    }
}
