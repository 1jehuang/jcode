//! Visual Debug Infrastructure
//!
//! Captures TUI frame state for autonomous debugging by AI agents.
//! When enabled, writes detailed render information to a debug file
//! that can be read to understand visual bugs without seeing the terminal.

use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use ratatui::layout::Rect;

/// Global flag to enable visual debugging (set via /debug-visual command)
static VISUAL_DEBUG_ENABLED: AtomicBool = AtomicBool::new(false);

/// Maximum number of frames to keep in the ring buffer
const MAX_FRAMES: usize = 100;

/// Global frame buffer
static FRAME_BUFFER: OnceLock<Mutex<FrameBuffer>> = OnceLock::new();

fn get_frame_buffer() -> &'static Mutex<FrameBuffer> {
    FRAME_BUFFER.get_or_init(|| Mutex::new(FrameBuffer::new()))
}

/// A captured frame with all render context
#[derive(Debug, Clone)]
pub struct FrameCapture {
    /// Frame number (monotonically increasing)
    pub frame_id: u64,
    /// Timestamp when frame was rendered
    pub timestamp: std::time::SystemTime,
    /// Terminal dimensions
    pub terminal_size: (u16, u16),
    /// Layout areas computed for this frame
    pub layout: LayoutCapture,
    /// State snapshot at render time
    pub state: StateSnapshot,
    /// Any anomalies detected during rendering
    pub anomalies: Vec<String>,
    /// The actual text content rendered to each area (stripped of ANSI)
    pub rendered_text: RenderedText,
}

/// Captured layout computation
#[derive(Debug, Clone, Default)]
pub struct LayoutCapture {
    /// Whether packed layout was used (vs scrolling)
    pub use_packed: bool,
    /// Estimated content height
    pub estimated_content_height: usize,
    /// Messages area
    pub messages_area: Option<RectCapture>,
    /// Status line area
    pub status_area: Option<RectCapture>,
    /// Queued messages area
    pub queued_area: Option<RectCapture>,
    /// Input area
    pub input_area: Option<RectCapture>,
    /// Input line count (before wrapping)
    pub input_lines_raw: usize,
    /// Input line count (after wrapping)
    pub input_lines_wrapped: usize,
}

/// Rect capture (serializable)
#[derive(Debug, Clone, Copy)]
pub struct RectCapture {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl From<Rect> for RectCapture {
    fn from(r: Rect) -> Self {
        Self {
            x: r.x,
            y: r.y,
            width: r.width,
            height: r.height,
        }
    }
}

/// State snapshot at render time
#[derive(Debug, Clone, Default)]
pub struct StateSnapshot {
    pub is_processing: bool,
    pub input_len: usize,
    pub input_preview: String,
    pub cursor_pos: usize,
    pub scroll_offset: usize,
    pub queued_count: usize,
    pub message_count: usize,
    pub streaming_text_len: usize,
    pub has_suggestions: bool,
    pub status: String,
}

/// Actual rendered text content
#[derive(Debug, Clone, Default)]
pub struct RenderedText {
    /// Status line text (spinner, tokens, elapsed, etc.)
    pub status_line: String,
    /// Input area text (what the user is typing)
    pub input_area: String,
    /// Hint text shown above input (if any)
    pub input_hint: Option<String>,
    /// Queued messages (messages waiting to be sent)
    pub queued_messages: Vec<String>,
    /// Recent messages displayed (last few for context)
    pub recent_messages: Vec<MessageCapture>,
    /// Streaming text (if currently streaming)
    pub streaming_text_preview: String,
}

/// Captured message for debugging
#[derive(Debug, Clone, Default)]
pub struct MessageCapture {
    pub role: String,
    pub content_preview: String,
    pub content_len: usize,
}

/// Ring buffer of recent frames
struct FrameBuffer {
    frames: VecDeque<FrameCapture>,
    next_frame_id: u64,
}

impl FrameBuffer {
    fn new() -> Self {
        Self {
            frames: VecDeque::with_capacity(MAX_FRAMES),
            next_frame_id: 0,
        }
    }

    fn push(&mut self, mut frame: FrameCapture) {
        frame.frame_id = self.next_frame_id;
        self.next_frame_id += 1;

        if self.frames.len() >= MAX_FRAMES {
            self.frames.pop_front();
        }
        self.frames.push_back(frame);
    }

    fn recent(&self, count: usize) -> Vec<&FrameCapture> {
        self.frames.iter().rev().take(count).collect()
    }

    fn frames_with_anomalies(&self) -> Vec<&FrameCapture> {
        self.frames
            .iter()
            .filter(|f| !f.anomalies.is_empty())
            .collect()
    }
}


/// Enable visual debugging
pub fn enable() {
    VISUAL_DEBUG_ENABLED.store(true, Ordering::SeqCst);
    crate::logging::info("Visual debugging enabled");
}

/// Disable visual debugging
pub fn disable() {
    VISUAL_DEBUG_ENABLED.store(false, Ordering::SeqCst);
}

/// Check if visual debugging is enabled
pub fn is_enabled() -> bool {
    VISUAL_DEBUG_ENABLED.load(Ordering::SeqCst)
}

/// Record a frame capture
pub fn record_frame(frame: FrameCapture) {
    if !is_enabled() {
        return;
    }

    let mut buffer = get_frame_buffer().lock().unwrap();
    buffer.push(frame);
}

/// Get the debug output path
fn debug_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("jcode")
        .join("visual-debug.txt")
}

/// Dump recent frames to the debug file
pub fn dump_to_file() -> std::io::Result<PathBuf> {
    let path = debug_path();

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let buffer = get_frame_buffer().lock().unwrap();
    let mut file = File::create(&path)?;

    writeln!(file, "=== JCODE VISUAL DEBUG DUMP ===")?;
    writeln!(
        file,
        "Generated: {:?}",
        std::time::SystemTime::now()
    )?;
    writeln!(file, "Total frames captured: {}", buffer.next_frame_id)?;
    writeln!(file, "Frames in buffer: {}", buffer.frames.len())?;
    writeln!(file)?;

    // First, show frames with anomalies
    let anomaly_frames = buffer.frames_with_anomalies();
    if !anomaly_frames.is_empty() {
        writeln!(file, "=== FRAMES WITH ANOMALIES ({}) ===", anomaly_frames.len())?;
        for frame in anomaly_frames {
            write_frame(&mut file, frame)?;
        }
        writeln!(file)?;
    }

    // Then show recent frames
    writeln!(file, "=== RECENT FRAMES (last 20) ===")?;
    for frame in buffer.recent(20) {
        write_frame(&mut file, frame)?;
    }

    Ok(path)
}

fn write_frame(file: &mut File, frame: &FrameCapture) -> std::io::Result<()> {
    writeln!(file, "--- Frame {} ---", frame.frame_id)?;
    writeln!(file, "Time: {:?}", frame.timestamp)?;
    writeln!(
        file,
        "Terminal: {}x{}",
        frame.terminal_size.0, frame.terminal_size.1
    )?;

    // State
    writeln!(file, "State:")?;
    writeln!(file, "  is_processing: {}", frame.state.is_processing)?;
    writeln!(file, "  input_len: {}", frame.state.input_len)?;
    writeln!(file, "  input_preview: {:?}", frame.state.input_preview)?;
    writeln!(file, "  cursor_pos: {}", frame.state.cursor_pos)?;
    writeln!(file, "  scroll_offset: {}", frame.state.scroll_offset)?;
    writeln!(file, "  queued_count: {}", frame.state.queued_count)?;
    writeln!(file, "  message_count: {}", frame.state.message_count)?;
    writeln!(file, "  streaming_text_len: {}", frame.state.streaming_text_len)?;
    writeln!(file, "  has_suggestions: {}", frame.state.has_suggestions)?;
    writeln!(file, "  status: {}", frame.state.status)?;

    // Layout
    writeln!(file, "Layout:")?;
    writeln!(file, "  use_packed: {}", frame.layout.use_packed)?;
    writeln!(
        file,
        "  estimated_content_height: {}",
        frame.layout.estimated_content_height
    )?;
    if let Some(r) = frame.layout.messages_area {
        writeln!(
            file,
            "  messages_area: ({}, {}) {}x{}",
            r.x, r.y, r.width, r.height
        )?;
    }
    if let Some(r) = frame.layout.status_area {
        writeln!(
            file,
            "  status_area: ({}, {}) {}x{}",
            r.x, r.y, r.width, r.height
        )?;
    }
    if let Some(r) = frame.layout.queued_area {
        writeln!(
            file,
            "  queued_area: ({}, {}) {}x{}",
            r.x, r.y, r.width, r.height
        )?;
    }
    if let Some(r) = frame.layout.input_area {
        writeln!(
            file,
            "  input_area: ({}, {}) {}x{}",
            r.x, r.y, r.width, r.height
        )?;
    }
    writeln!(
        file,
        "  input_lines: {} raw, {} wrapped",
        frame.layout.input_lines_raw, frame.layout.input_lines_wrapped
    )?;

    // Rendered text
    writeln!(file, "Rendered:")?;
    writeln!(file, "  status_line: {:?}", frame.rendered_text.status_line)?;
    if let Some(hint) = &frame.rendered_text.input_hint {
        writeln!(file, "  input_hint: {:?}", hint)?;
    }
    writeln!(file, "  input_area: {:?}", frame.rendered_text.input_area)?;
    if !frame.rendered_text.queued_messages.is_empty() {
        writeln!(file, "  queued_messages:")?;
        for (i, msg) in frame.rendered_text.queued_messages.iter().enumerate() {
            writeln!(file, "    [{}]: {:?}", i, msg)?;
        }
    }
    if !frame.rendered_text.recent_messages.is_empty() {
        writeln!(file, "  recent_messages:")?;
        for msg in &frame.rendered_text.recent_messages {
            writeln!(
                file,
                "    [{}] ({} chars): {:?}",
                msg.role, msg.content_len, msg.content_preview
            )?;
        }
    }
    if !frame.rendered_text.streaming_text_preview.is_empty() {
        writeln!(
            file,
            "  streaming_text: {:?}",
            frame.rendered_text.streaming_text_preview
        )?;
    }

    // Anomalies
    if !frame.anomalies.is_empty() {
        writeln!(file, "ANOMALIES:")?;
        for anomaly in &frame.anomalies {
            writeln!(file, "  ⚠ {}", anomaly)?;
        }
    }

    writeln!(file)?;
    Ok(())
}

/// Builder for constructing frame captures during rendering
#[derive(Default)]
pub struct FrameCaptureBuilder {
    pub layout: LayoutCapture,
    pub state: StateSnapshot,
    pub rendered_text: RenderedText,
    pub anomalies: Vec<String>,
    terminal_size: (u16, u16),
}

impl FrameCaptureBuilder {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            terminal_size: (width, height),
            ..Default::default()
        }
    }

    /// Record an anomaly detected during rendering
    pub fn anomaly(&mut self, msg: impl Into<String>) {
        self.anomalies.push(msg.into());
    }

    /// Check a condition and record anomaly if false
    pub fn check(&mut self, condition: bool, msg: impl Into<String>) {
        if !condition {
            self.anomalies.push(msg.into());
        }
    }

    /// Build the final frame capture
    pub fn build(self) -> FrameCapture {
        FrameCapture {
            frame_id: 0, // Will be set by buffer
            timestamp: std::time::SystemTime::now(),
            terminal_size: self.terminal_size,
            layout: self.layout,
            state: self.state,
            anomalies: self.anomalies,
            rendered_text: self.rendered_text,
        }
    }
}

/// Check for the specific "Shift+Enter" hint anomaly
pub fn check_shift_enter_anomaly(
    builder: &mut FrameCaptureBuilder,
    is_processing: bool,
    input_text: &str,
    hint_shown: bool,
) {
    // The hint should ONLY show when processing AND input is non-empty
    let should_show = is_processing && !input_text.is_empty();

    if hint_shown != should_show {
        builder.anomaly(format!(
            "Shift+Enter hint mismatch: shown={}, should_show={} (is_processing={}, input_len={})",
            hint_shown,
            should_show,
            is_processing,
            input_text.len()
        ));
    }

    // Also check if the hint text appears in the input itself (the bug!)
    if input_text.to_lowercase().contains("shift") && input_text.to_lowercase().contains("enter") {
        builder.anomaly(format!(
            "INPUT CONTAINS 'shift'+'enter' - possible hint leak: {:?}",
            input_text
        ));
    }
}
