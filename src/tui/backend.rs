//! Backend abstraction for TUI - allows running locally or via server
//!
//! This module provides a unified interface for message processing,
//! whether running standalone (LocalBackend) or as a client (RemoteBackend).
//!
//! Also provides debug socket events for exposing full TUI state.

use crate::message::ToolCall;
use crate::protocol::{Request, ServerEvent};
use crate::server;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::UnixStream;
use tokio::sync::Mutex;

/// Debug events broadcast by standalone TUI via debug socket.
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
    TokenUsage { input_tokens: u64, output_tokens: u64 },

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
    ToolStart { id: String, name: String },

    /// Tool input JSON delta
    ToolInput { delta: String },

    /// Tool is about to execute (after input complete)
    ToolExec { id: String, name: String },

    /// Tool execution completed
    ToolDone { id: String, name: String, output: String, is_error: bool },

    /// Token usage update
    TokenUsage { input_tokens: u64, output_tokens: u64 },

    /// Thinking started (extended thinking mode)
    ThinkingStart,

    /// Thinking ended
    ThinkingEnd,

    /// Thinking completed with duration
    ThinkingDone { duration_secs: f32 },

    /// Context compaction occurred
    Compaction { trigger: String, pre_tokens: u64 },

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

/// Information about the backend's provider
#[derive(Debug, Clone)]
pub struct BackendInfo {
    pub provider_name: String,
    pub provider_model: String,
    pub mcp_servers: Vec<String>,
    pub skills: Vec<String>,
}

/// Data for pending file diff generation (client-side)
struct PendingFileDiff {
    file_path: String,
    original_content: String,
}

/// Check if client-side diff generation is enabled
fn show_diffs_enabled() -> bool {
    std::env::var("JCODE_SHOW_DIFFS")
        .map(|v| v != "0" && v.to_lowercase() != "false")
        .unwrap_or(true)
}

/// Remote connection to jcode server
pub struct RemoteConnection {
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: Arc<Mutex<OwnedWriteHalf>>,
    session_id: Option<String>,
    next_request_id: u64,
    provider_name: String,
    provider_model: String,
    pending_diffs: HashMap<String, PendingFileDiff>,
    current_tool_id: Option<String>,
    current_tool_name: Option<String>,
    current_tool_input: String,
    line_buffer: String,
    has_loaded_history: bool,
}

impl RemoteConnection {
    /// Connect to the server
    pub async fn connect() -> Result<Self> {
        let stream = UnixStream::connect(server::socket_path()).await?;
        let (reader, writer) = stream.into_split();

        let mut conn = Self {
            reader: BufReader::new(reader),
            writer: Arc::new(Mutex::new(writer)),
            session_id: None,
            next_request_id: 1,
            provider_name: "remote".to_string(),
            provider_model: "unknown".to_string(),
            pending_diffs: HashMap::new(),
            current_tool_id: None,
            current_tool_name: None,
            current_tool_input: String::new(),
            line_buffer: String::new(),
            has_loaded_history: false,
        };

        // Subscribe to events
        conn.send_request(Request::Subscribe { id: conn.next_request_id }).await?;
        conn.next_request_id += 1;

        // Request history
        conn.send_request(Request::GetHistory { id: conn.next_request_id }).await?;
        conn.next_request_id += 1;

        Ok(conn)
    }

    async fn send_request(&self, request: Request) -> Result<()> {
        let json = serde_json::to_string(&request)? + "\n";
        let mut w = self.writer.lock().await;
        w.write_all(json.as_bytes()).await?;
        Ok(())
    }

    /// Send a message to the server
    /// Send a message to the server and return the request ID
    pub async fn send_message(&mut self, content: String) -> Result<u64> {
        let id = self.next_request_id;
        let request = Request::Message {
            id,
            content,
        };
        self.next_request_id += 1;
        self.send_request(request).await?;
        Ok(id)
    }

    /// Request server reload
    pub async fn reload(&mut self) -> Result<()> {
        let request = Request::Reload { id: self.next_request_id };
        self.next_request_id += 1;
        self.send_request(request).await
    }

    /// Resume a specific session by ID
    pub async fn resume_session(&mut self, session_id: &str) -> Result<()> {
        let request = Request::ResumeSession {
            id: self.next_request_id,
            session_id: session_id.to_string(),
        };
        self.next_request_id += 1;
        self.send_request(request).await
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

    /// Read the next event from the server (returns None on disconnect)
    pub async fn next_event(&mut self) -> Option<ServerEvent> {
        self.line_buffer.clear();
        match self.reader.read_line(&mut self.line_buffer).await {
            Ok(0) => None,
            Ok(_) => serde_json::from_str(&self.line_buffer).ok(),
            Err(_) => None,
        }
    }

    /// Get writer for sending requests
    pub fn writer(&self) -> Arc<Mutex<OwnedWriteHalf>> {
        Arc::clone(&self.writer)
    }

    /// Get session ID
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
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
        self.current_tool_id = Some(id.to_string());
        self.current_tool_name = Some(name.to_string());
        self.current_tool_input.clear();
    }

    /// Handle tool input delta
    pub fn handle_tool_input(&mut self, delta: &str) {
        self.current_tool_input.push_str(delta);
    }

    /// Get parsed current tool input (before it's cleared in handle_tool_exec)
    pub fn get_current_tool_input(&self) -> serde_json::Value {
        serde_json::from_str(&self.current_tool_input)
            .unwrap_or(serde_json::Value::Null)
    }

    /// Handle tool exec - cache file content if edit/write
    pub fn handle_tool_exec(&mut self, id: &str, name: &str) {
        if show_diffs_enabled() && (name == "edit" || name == "write") {
            if let Ok(input) = serde_json::from_str::<serde_json::Value>(&self.current_tool_input) {
                if let Some(file_path) = input.get("file_path").and_then(|v| v.as_str()) {
                    let original = std::fs::read_to_string(file_path).unwrap_or_default();
                    self.pending_diffs.insert(id.to_string(), PendingFileDiff {
                        file_path: file_path.to_string(),
                        original_content: original,
                    });
                }
            }
        }
        self.current_tool_id = None;
        self.current_tool_name = None;
        self.current_tool_input.clear();
    }

    /// Handle tool done - generate diff if we have pending data
    pub fn handle_tool_done(&mut self, id: &str, name: &str, output: &str) -> String {
        if let Some(pending) = self.pending_diffs.remove(id) {
            let new_content = std::fs::read_to_string(&pending.file_path).unwrap_or_default();
            let diff = generate_unified_diff(&pending.original_content, &new_content, &pending.file_path);
            if !diff.is_empty() {
                return format!("[{}] {}\n{}", name, pending.file_path, diff);
            }
        }
        format!("[{}] {}", name, output)
    }

    /// Clear pending diff state
    pub fn clear_pending(&mut self) {
        self.pending_diffs.clear();
    }
}

/// Generate a unified diff between two strings
fn generate_unified_diff(old: &str, new: &str, file_path: &str) -> String {
    use similar::TextDiff;
    let diff = TextDiff::from_lines(old, new);
    let mut output = String::new();

    output.push_str(&format!("--- a/{}\n", file_path));
    output.push_str(&format!("+++ b/{}\n", file_path));

    for hunk in diff.unified_diff().context_radius(3).iter_hunks() {
        output.push_str(&format!("{}", hunk));
    }

    output
}
