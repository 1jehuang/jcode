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
use serde::Serialize;
use serde_json::Value;

/// Global flag to enable visual debugging (set via /debug-visual command)
static VISUAL_DEBUG_ENABLED: AtomicBool = AtomicBool::new(false);
/// Global flag to enable overlay drawing
static VISUAL_DEBUG_OVERLAY: AtomicBool = AtomicBool::new(false);

/// Maximum number of frames to keep in the ring buffer
const MAX_FRAMES: usize = 100;

/// Global frame buffer
static FRAME_BUFFER: OnceLock<Mutex<FrameBuffer>> = OnceLock::new();

fn get_frame_buffer() -> &'static Mutex<FrameBuffer> {
    FRAME_BUFFER.get_or_init(|| Mutex::new(FrameBuffer::new()))
}

/// A captured frame with all render context
#[derive(Debug, Clone, Serialize)]
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
    /// Render timing information (milliseconds)
    pub render_timing: Option<RenderTimingCapture>,
    /// Info widget placements and summary data
    pub info_widgets: Option<InfoWidgetCapture>,
    /// Render order for major phases
    pub render_order: Vec<String>,
    /// Mermaid debug stats snapshot (if available)
    pub mermaid: Option<Value>,
    /// Markdown debug stats snapshot (if available)
    pub markdown: Option<Value>,
    /// Theme/palette snapshot (if available)
    pub theme: Option<Value>,
}

/// Captured layout computation
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
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
    /// Margin widths for info widgets (per visible row)
    pub margins: Option<MarginsCapture>,
    /// Info widget placements
    pub widget_placements: Vec<WidgetPlacementCapture>,
}

/// Rect capture (serializable)
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct RectCapture {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

/// Margin widths captured for debug
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct MarginsCapture {
    pub left_widths: Vec<u16>,
    pub right_widths: Vec<u16>,
    pub centered: bool,
}

/// Info widget placement capture
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct WidgetPlacementCapture {
    pub kind: String,
    pub side: String,
    pub rect: RectCapture,
}

/// Render timing capture (milliseconds)
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct RenderTimingCapture {
    pub prepare_ms: f32,
    pub draw_ms: f32,
    pub total_ms: f32,
    pub messages_ms: Option<f32>,
    pub widgets_ms: Option<f32>,
}

/// Info widget summary capture
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct InfoWidgetSummary {
    pub todos_total: usize,
    pub todos_done: usize,
    pub context_total_chars: Option<usize>,
    pub context_limit: Option<usize>,
    pub queue_mode: Option<bool>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub session_count: Option<usize>,
    pub client_count: Option<usize>,
    pub memory_total: Option<usize>,
    pub memory_project: Option<usize>,
    pub memory_global: Option<usize>,
    pub memory_activity: Option<bool>,
    pub swarm_session_count: Option<usize>,
    pub swarm_member_count: Option<usize>,
    pub swarm_subagent_status: Option<String>,
    pub background_running: Option<usize>,
    pub background_tasks: Option<usize>,
    pub usage_available: Option<bool>,
    pub usage_provider: Option<String>,
    pub tokens_per_second: Option<f32>,
    pub auth_method: Option<String>,
    pub upstream_provider: Option<String>,
}

/// Info widget capture (summary + placements)
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct InfoWidgetCapture {
    pub summary: InfoWidgetSummary,
    pub placements: Vec<WidgetPlacementCapture>,
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
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
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
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
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
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
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

/// Enable or disable overlay drawing
pub fn set_overlay(enabled: bool) {
    VISUAL_DEBUG_OVERLAY.store(enabled, Ordering::SeqCst);
}

/// Check if overlay drawing is enabled
pub fn overlay_enabled() -> bool {
    VISUAL_DEBUG_OVERLAY.load(Ordering::SeqCst)
}

/// Check if visual debugging is enabled
pub fn is_enabled() -> bool {
    VISUAL_DEBUG_ENABLED.load(Ordering::SeqCst)
}

/// Record a frame capture (skips if identical to previous frame)
pub fn record_frame(frame: FrameCapture) {
    if !is_enabled() {
        return;
    }

    let mut buffer = get_frame_buffer().lock().unwrap();

    // Skip duplicate frames - only capture when something changes
    // Always capture frames with anomalies
    if let Some(last) = buffer.frames.back() {
        let dominated = frame.state == last.state
            && frame.rendered_text == last.rendered_text
            && frame.layout == last.layout
            && frame.info_widgets == last.info_widgets
            && frame.anomalies.is_empty();
        if dominated {
            return;
        }
    }

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
    writeln!(file, "Generated: {:?}", std::time::SystemTime::now())?;
    writeln!(file, "Total frames captured: {}", buffer.next_frame_id)?;
    writeln!(file, "Frames in buffer: {}", buffer.frames.len())?;
    writeln!(file)?;

    // First, show frames with anomalies
    let anomaly_frames = buffer.frames_with_anomalies();
    if !anomaly_frames.is_empty() {
        writeln!(
            file,
            "=== FRAMES WITH ANOMALIES ({}) ===",
            anomaly_frames.len()
        )?;
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

/// Return the most recent frame capture.
pub fn latest_frame() -> Option<FrameCapture> {
    let buffer = get_frame_buffer().lock().ok()?;
    buffer.frames.back().cloned()
}

/// Return the most recent frame as a JSON string.
pub fn latest_frame_json() -> Option<String> {
    let frame = latest_frame()?;
    serde_json::to_string_pretty(&frame).ok()
}

/// Return the most recent frame as a normalized JSON string (for stable diffs).
/// Strips timestamps, UUIDs, session IDs, and other non-deterministic values.
pub fn latest_frame_json_normalized() -> Option<String> {
    let frame = latest_frame()?;
    let normalized = normalize_frame(&frame);
    serde_json::to_string_pretty(&normalized).ok()
}

/// Normalize a frame capture for stable comparisons.
/// Replaces timestamps, UUIDs, session IDs, and other volatile values with placeholders.
pub fn normalize_frame(frame: &FrameCapture) -> serde_json::Value {
    let json = serde_json::to_value(frame).unwrap_or(serde_json::Value::Null);
    normalize_json_value(json)
}

/// Recursively normalize JSON values, replacing volatile content.
fn normalize_json_value(value: serde_json::Value) -> serde_json::Value {
    use serde_json::Value;

    match value {
        Value::String(s) => Value::String(normalize_string(&s)),
        Value::Array(arr) => Value::Array(arr.into_iter().map(normalize_json_value).collect()),
        Value::Object(map) => {
            let mut new_map = serde_json::Map::new();
            for (k, v) in map {
                // Skip timestamp fields entirely or normalize them
                if k == "timestamp" || k == "created_at" || k == "updated_at" {
                    new_map.insert(k, Value::String("<TIMESTAMP>".to_string()));
                } else if k == "frame_id" {
                    // Keep frame_id but note it's sequential
                    new_map.insert(k, v);
                } else {
                    new_map.insert(k, normalize_json_value(v));
                }
            }
            Value::Object(new_map)
        }
        other => other,
    }
}

/// Normalize a string by replacing volatile patterns with placeholders.
fn normalize_string(s: &str) -> String {
    use regex::Regex;
    use std::sync::OnceLock;

    // Cached regex patterns for performance
    static UUID_RE: OnceLock<Regex> = OnceLock::new();
    static SESSION_ID_RE: OnceLock<Regex> = OnceLock::new();
    static TIMESTAMP_RE: OnceLock<Regex> = OnceLock::new();
    static ISO_DATE_RE: OnceLock<Regex> = OnceLock::new();
    static DURATION_RE: OnceLock<Regex> = OnceLock::new();
    static PATH_RE: OnceLock<Regex> = OnceLock::new();
    static ELAPSED_RE: OnceLock<Regex> = OnceLock::new();
    static TOKENS_RE: OnceLock<Regex> = OnceLock::new();

    let uuid_re = UUID_RE.get_or_init(|| {
        Regex::new(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")
            .unwrap()
    });
    let session_id_re = SESSION_ID_RE.get_or_init(|| Regex::new(r"session_[0-9a-zA-Z_]+").unwrap());
    let timestamp_re = TIMESTAMP_RE.get_or_init(|| {
        Regex::new(r"\d{10,13}").unwrap() // Unix timestamps (10-13 digits)
    });
    let iso_date_re =
        ISO_DATE_RE.get_or_init(|| Regex::new(r"\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}").unwrap());
    let duration_re = DURATION_RE.get_or_init(|| {
        Regex::new(r"\d+(\.\d+)?s").unwrap() // Duration like "1.5s"
    });
    let path_re = PATH_RE.get_or_init(|| {
        Regex::new(r"/(?:home|Users)/[^/\s]+").unwrap() // Home directory paths
    });
    let elapsed_re = ELAPSED_RE.get_or_init(|| {
        Regex::new(r"\d+m?\d*s").unwrap() // Elapsed time like "5s" or "1m30s"
    });
    let tokens_re = TOKENS_RE.get_or_init(|| {
        Regex::new(r"\d+[kK]? tokens?").unwrap() // Token counts
    });

    let mut result = s.to_string();

    // Replace in order of specificity (most specific first)
    result = uuid_re.replace_all(&result, "<UUID>").to_string();
    result = session_id_re
        .replace_all(&result, "<SESSION_ID>")
        .to_string();
    result = iso_date_re.replace_all(&result, "<ISO_DATE>").to_string();
    result = elapsed_re.replace_all(&result, "<ELAPSED>").to_string();
    result = tokens_re.replace_all(&result, "<TOKENS>").to_string();
    result = duration_re.replace_all(&result, "<DURATION>").to_string();
    result = path_re.replace_all(&result, "<HOME>").to_string();

    // Only replace long timestamps that aren't part of other patterns
    if result.len() < 20 {
        result = timestamp_re.replace_all(&result, "<TIMESTAMP>").to_string();
    }

    result
}

/// Compare two frames for semantic equality (ignoring volatile fields).
pub fn frames_equal_normalized(a: &FrameCapture, b: &FrameCapture) -> bool {
    let norm_a = normalize_frame(a);
    let norm_b = normalize_frame(b);
    norm_a == norm_b
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
    writeln!(
        file,
        "  streaming_text_len: {}",
        frame.state.streaming_text_len
    )?;
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
    if let Some(margins) = &frame.layout.margins {
        writeln!(
            file,
            "  margins: centered={} left_rows={} right_rows={}",
            margins.centered,
            margins.left_widths.len(),
            margins.right_widths.len()
        )?;
    }
    if !frame.layout.widget_placements.is_empty() {
        writeln!(file, "  widget_placements:")?;
        for placement in &frame.layout.widget_placements {
            let r = placement.rect;
            writeln!(
                file,
                "    {} ({}) at ({}, {}) {}x{}",
                placement.kind, placement.side, r.x, r.y, r.width, r.height
            )?;
        }
    }

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

    // Render timing
    if let Some(timing) = &frame.render_timing {
        writeln!(
            file,
            "Timing: prepare={:.2}ms draw={:.2}ms total={:.2}ms messages={:?} widgets={:?}",
            timing.prepare_ms,
            timing.draw_ms,
            timing.total_ms,
            timing.messages_ms,
            timing.widgets_ms
        )?;
    }

    // Info widget summary
    if let Some(info) = &frame.info_widgets {
        writeln!(file, "InfoWidgets:")?;
        writeln!(
            file,
            "  todos: {}/{} done, context_chars: {:?}, model: {:?}",
            info.summary.todos_done,
            info.summary.todos_total,
            info.summary.context_total_chars,
            info.summary.model
        )?;
        writeln!(
            file,
            "  session_count: {:?}, client_count: {:?}, swarm_members: {:?}",
            info.summary.session_count, info.summary.client_count, info.summary.swarm_member_count
        )?;
    }

    if !frame.render_order.is_empty() {
        writeln!(file, "Render order:")?;
        for step in &frame.render_order {
            writeln!(file, "  - {}", step)?;
        }
    }

    if let Some(mermaid) = &frame.mermaid {
        writeln!(file, "Mermaid: {}", mermaid)?;
    }
    if let Some(markdown) = &frame.markdown {
        writeln!(file, "Markdown: {}", markdown)?;
    }
    if let Some(theme) = &frame.theme {
        writeln!(file, "Theme: {}", theme)?;
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
    pub render_timing: Option<RenderTimingCapture>,
    pub info_widgets: Option<InfoWidgetCapture>,
    pub render_order: Vec<String>,
    pub mermaid: Option<Value>,
    pub markdown: Option<Value>,
    pub theme: Option<Value>,
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
            render_timing: self.render_timing,
            info_widgets: self.info_widgets,
            render_order: self.render_order,
            mermaid: self.mermaid,
            markdown: self.markdown,
            theme: self.theme,
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
