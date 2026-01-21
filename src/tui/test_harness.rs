//! TUI Test Harness
//!
//! Comprehensive testing infrastructure for autonomous TUI testing.
//! Provides deterministic clock, event replay, log bundles, and headless rendering.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

// ============================================================================
// Deterministic Clock
// ============================================================================

/// Global test clock for deterministic timing in tests.
/// When enabled, all time queries go through this clock instead of system time.
static TEST_CLOCK: OnceLock<RwLock<TestClock>> = OnceLock::new();
static TEST_CLOCK_ENABLED: AtomicBool = AtomicBool::new(false);

/// A controllable clock for deterministic testing.
#[derive(Debug)]
pub struct TestClock {
    /// Current simulated time in milliseconds since epoch
    current_ms: AtomicU64,
    /// Base instant for relative timing
    base_instant: Instant,
}

impl TestClock {
    pub fn new() -> Self {
        Self {
            current_ms: AtomicU64::new(0),
            base_instant: Instant::now(),
        }
    }

    /// Get the simulated current time in milliseconds.
    pub fn now_ms(&self) -> u64 {
        self.current_ms.load(Ordering::SeqCst)
    }

    /// Advance the clock by the given duration.
    pub fn advance(&self, duration: Duration) {
        let ms = duration.as_millis() as u64;
        self.current_ms.fetch_add(ms, Ordering::SeqCst);
    }

    /// Set the clock to a specific time.
    pub fn set(&self, ms: u64) {
        self.current_ms.store(ms, Ordering::SeqCst);
    }

    /// Get a simulated Instant relative to base.
    pub fn instant(&self) -> SimulatedInstant {
        SimulatedInstant {
            offset_ms: self.now_ms(),
        }
    }
}

impl Default for TestClock {
    fn default() -> Self {
        Self::new()
    }
}

/// A simulated Instant for deterministic timing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SimulatedInstant {
    offset_ms: u64,
}

impl SimulatedInstant {
    pub fn elapsed(&self) -> Duration {
        let now = get_test_clock()
            .map(|c| c.read().unwrap().now_ms())
            .unwrap_or(0);
        Duration::from_millis(now.saturating_sub(self.offset_ms))
    }

    pub fn duration_since(&self, earlier: SimulatedInstant) -> Duration {
        Duration::from_millis(self.offset_ms.saturating_sub(earlier.offset_ms))
    }
}

/// Enable the test clock for deterministic timing.
pub fn enable_test_clock() {
    TEST_CLOCK.get_or_init(|| RwLock::new(TestClock::new()));
    TEST_CLOCK_ENABLED.store(true, Ordering::SeqCst);
}

/// Disable the test clock (return to system time).
pub fn disable_test_clock() {
    TEST_CLOCK_ENABLED.store(false, Ordering::SeqCst);
}

/// Check if test clock is enabled.
pub fn is_test_clock_enabled() -> bool {
    TEST_CLOCK_ENABLED.load(Ordering::SeqCst)
}

/// Get the test clock if enabled.
pub fn get_test_clock() -> Option<&'static RwLock<TestClock>> {
    if is_test_clock_enabled() {
        TEST_CLOCK.get()
    } else {
        None
    }
}

/// Advance the test clock by the given duration.
pub fn advance_clock(duration: Duration) {
    if let Some(clock) = get_test_clock() {
        clock.read().unwrap().advance(duration);
    }
}

/// Get current time (uses test clock if enabled, otherwise system time).
pub fn now_ms() -> u64 {
    if let Some(clock) = get_test_clock() {
        clock.read().unwrap().now_ms()
    } else {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }
}

// ============================================================================
// Event Recording & Replay
// ============================================================================

/// Global event recorder.
static EVENT_RECORDER: OnceLock<Mutex<EventRecorder>> = OnceLock::new();

/// Types of events that can be recorded/replayed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "data")]
pub enum TestEvent {
    /// Key press event
    Key {
        code: String,
        modifiers: Vec<String>,
    },
    /// Mouse event (click, scroll)
    Mouse { kind: String, x: u16, y: u16 },
    /// Terminal resize
    Resize { width: u16, height: u16 },
    /// Paste event
    Paste { text: String },
    /// Focus change
    Focus { gained: bool },
    /// Debug command injected
    DebugCommand { command: String },
    /// Message submitted
    MessageSubmit { content: String },
    /// Wait/delay marker
    Wait { ms: u64 },
    /// Checkpoint marker (for assertions)
    Checkpoint { name: String },
}

/// A recorded event with timestamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedEvent {
    /// Time offset from recording start (ms)
    pub offset_ms: u64,
    /// The event that occurred
    pub event: TestEvent,
}

/// Event recorder for capturing test sequences.
#[derive(Debug, Serialize, Deserialize)]
pub struct EventRecorder {
    events: Vec<RecordedEvent>,
    start_time: Option<u64>,
    is_recording: bool,
}

impl EventRecorder {
    pub fn new() -> Self {
        Self {
            events: Vec::new(),
            start_time: None,
            is_recording: false,
        }
    }

    /// Start recording events.
    pub fn start(&mut self) {
        self.events.clear();
        self.start_time = Some(now_ms());
        self.is_recording = true;
    }

    /// Stop recording events.
    pub fn stop(&mut self) {
        self.is_recording = false;
    }

    /// Record an event.
    pub fn record(&mut self, event: TestEvent) {
        if !self.is_recording {
            return;
        }
        let start = self.start_time.unwrap_or_else(now_ms);
        let offset_ms = now_ms().saturating_sub(start);
        self.events.push(RecordedEvent { offset_ms, event });
    }

    /// Get all recorded events.
    pub fn events(&self) -> &[RecordedEvent] {
        &self.events
    }

    /// Export events to JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&self.events).unwrap_or_else(|_| "[]".to_string())
    }

    /// Import events from JSON.
    pub fn from_json(json: &str) -> Result<Vec<RecordedEvent>, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Check if recording.
    pub fn is_recording(&self) -> bool {
        self.is_recording
    }
}

impl Default for EventRecorder {
    fn default() -> Self {
        Self::new()
    }
}

/// Get or initialize the global event recorder.
pub fn get_event_recorder() -> &'static Mutex<EventRecorder> {
    EVENT_RECORDER.get_or_init(|| Mutex::new(EventRecorder::new()))
}

/// Start global event recording.
pub fn start_recording() {
    get_event_recorder().lock().unwrap().start();
}

/// Stop global event recording.
pub fn stop_recording() {
    get_event_recorder().lock().unwrap().stop();
}

/// Record an event globally.
pub fn record_event(event: TestEvent) {
    get_event_recorder().lock().unwrap().record(event);
}

/// Get recorded events as JSON.
pub fn get_recorded_events_json() -> String {
    get_event_recorder().lock().unwrap().to_json()
}

/// Event player for replaying recorded sequences.
#[derive(Debug)]
pub struct EventPlayer {
    events: VecDeque<RecordedEvent>,
    start_time: Option<u64>,
}

impl EventPlayer {
    /// Create a new player from recorded events.
    pub fn new(events: Vec<RecordedEvent>) -> Self {
        Self {
            events: events.into_iter().collect(),
            start_time: None,
        }
    }

    /// Load events from JSON.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        let events = EventRecorder::from_json(json)?;
        Ok(Self::new(events))
    }

    /// Start playback.
    pub fn start(&mut self) {
        self.start_time = Some(now_ms());
    }

    /// Get the next event if it's time to play it.
    /// Returns None if no event is ready or playback hasn't started.
    pub fn next_event(&mut self) -> Option<TestEvent> {
        let start = self.start_time?;
        let elapsed = now_ms().saturating_sub(start);

        if let Some(next) = self.events.front() {
            if next.offset_ms <= elapsed {
                return self.events.pop_front().map(|e| e.event);
            }
        }
        None
    }

    /// Check if playback is complete.
    pub fn is_complete(&self) -> bool {
        self.events.is_empty()
    }

    /// Get remaining event count.
    pub fn remaining(&self) -> usize {
        self.events.len()
    }
}

// ============================================================================
// Test Log Bundle
// ============================================================================

/// A comprehensive test log bundle for debugging and CI.
#[derive(Debug, Serialize, Deserialize)]
pub struct TestBundle {
    /// Test name/description
    pub name: String,
    /// Start timestamp
    pub started_at: String,
    /// End timestamp (if complete)
    pub ended_at: Option<String>,
    /// Test duration in ms
    pub duration_ms: Option<u64>,
    /// Overall pass/fail status
    pub passed: Option<bool>,
    /// Recorded events
    pub events: Vec<RecordedEvent>,
    /// Captured frames (normalized)
    pub frames: Vec<serde_json::Value>,
    /// Debug trace events
    pub trace: Vec<serde_json::Value>,
    /// Assertion results
    pub assertions: Vec<serde_json::Value>,
    /// Stdout captured
    pub stdout: Vec<String>,
    /// Stderr captured
    pub stderr: Vec<String>,
    /// App logs captured
    pub app_logs: Vec<String>,
    /// Error messages
    pub errors: Vec<String>,
    /// Arbitrary metadata
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

impl TestBundle {
    /// Create a new test bundle.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            started_at: chrono_now(),
            ended_at: None,
            duration_ms: None,
            passed: None,
            events: Vec::new(),
            frames: Vec::new(),
            trace: Vec::new(),
            assertions: Vec::new(),
            stdout: Vec::new(),
            stderr: Vec::new(),
            app_logs: Vec::new(),
            errors: Vec::new(),
            metadata: serde_json::Map::new(),
        }
    }

    /// Mark the test as complete.
    pub fn complete(&mut self, passed: bool) {
        self.ended_at = Some(chrono_now());
        self.passed = Some(passed);
        // Duration would be calculated from timestamps
    }

    /// Add an event.
    pub fn add_event(&mut self, event: RecordedEvent) {
        self.events.push(event);
    }

    /// Add a frame capture.
    pub fn add_frame(&mut self, frame: serde_json::Value) {
        self.frames.push(frame);
    }

    /// Add a trace event.
    pub fn add_trace(&mut self, trace: serde_json::Value) {
        self.trace.push(trace);
    }

    /// Add an assertion result.
    pub fn add_assertion(&mut self, assertion: serde_json::Value) {
        self.assertions.push(assertion);
    }

    /// Add stdout line.
    pub fn add_stdout(&mut self, line: &str) {
        self.stdout.push(line.to_string());
    }

    /// Add stderr line.
    pub fn add_stderr(&mut self, line: &str) {
        self.stderr.push(line.to_string());
    }

    /// Add app log line.
    pub fn add_log(&mut self, line: &str) {
        self.app_logs.push(line.to_string());
    }

    /// Add error.
    pub fn add_error(&mut self, error: &str) {
        self.errors.push(error.to_string());
    }

    /// Set metadata value.
    pub fn set_metadata(&mut self, key: &str, value: serde_json::Value) {
        self.metadata.insert(key.to_string(), value);
    }

    /// Export to JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// Save to file.
    pub fn save(&self, path: &PathBuf) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = File::create(path)?;
        file.write_all(self.to_json().as_bytes())?;
        Ok(())
    }

    /// Load from file.
    pub fn load(path: &PathBuf) -> std::io::Result<Self> {
        let content = fs::read_to_string(path)?;
        serde_json::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Get default bundle output path.
    pub fn default_path(name: &str) -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("jcode")
            .join("test-bundles")
            .join(format!("{}.json", sanitize_filename(name)))
    }
}

fn chrono_now() -> String {
    // Simple ISO 8601 timestamp
    let now = std::time::SystemTime::now();
    let duration = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}ms", duration.as_millis())
}

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// ============================================================================
// Headless Renderer
// ============================================================================

/// A headless rendering backend for CI/testing.
/// Renders to an in-memory buffer instead of a real terminal.
#[derive(Debug)]
pub struct HeadlessBuffer {
    width: u16,
    height: u16,
    cells: Vec<Vec<Cell>>,
}

/// A single cell in the headless buffer.
#[derive(Debug, Clone, Default)]
pub struct Cell {
    pub char: char,
    pub fg: Option<String>,
    pub bg: Option<String>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

impl HeadlessBuffer {
    /// Create a new headless buffer with the given dimensions.
    pub fn new(width: u16, height: u16) -> Self {
        let cells = vec![vec![Cell::default(); width as usize]; height as usize];
        Self {
            width,
            height,
            cells,
        }
    }

    /// Get the dimensions.
    pub fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    /// Resize the buffer.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        self.cells = vec![vec![Cell::default(); width as usize]; height as usize];
    }

    /// Clear the buffer.
    pub fn clear(&mut self) {
        for row in &mut self.cells {
            for cell in row {
                *cell = Cell::default();
            }
        }
    }

    /// Set a cell.
    pub fn set(&mut self, x: u16, y: u16, cell: Cell) {
        if (x as usize) < self.width as usize && (y as usize) < self.height as usize {
            self.cells[y as usize][x as usize] = cell;
        }
    }

    /// Get a cell.
    pub fn get(&self, x: u16, y: u16) -> Option<&Cell> {
        self.cells.get(y as usize)?.get(x as usize)
    }

    /// Render to plain text (no styles).
    pub fn to_text(&self) -> String {
        self.cells
            .iter()
            .map(|row| {
                row.iter()
                    .map(|c| if c.char == '\0' { ' ' } else { c.char })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Get text from a rectangular region.
    pub fn get_region_text(&self, x: u16, y: u16, width: u16, height: u16) -> String {
        let mut lines = Vec::new();
        for row in y..(y + height).min(self.height) {
            let mut line = String::new();
            for col in x..(x + width).min(self.width) {
                if let Some(cell) = self.get(col, row) {
                    line.push(if cell.char == '\0' { ' ' } else { cell.char });
                }
            }
            lines.push(line);
        }
        lines.join("\n")
    }

    /// Search for text in the buffer.
    pub fn find_text(&self, needle: &str) -> Vec<(u16, u16)> {
        let mut results = Vec::new();
        let text = self.to_text();
        for (y, line) in text.lines().enumerate() {
            if let Some(x) = line.find(needle) {
                results.push((x as u16, y as u16));
            }
        }
        results
    }

    /// Check if text exists anywhere in the buffer.
    pub fn contains_text(&self, needle: &str) -> bool {
        !self.find_text(needle).is_empty()
    }
}

// ============================================================================
// Widget IDs (Stable Identifiers)
// ============================================================================

/// Stable widget identifiers for testing.
/// These IDs remain consistent across renders for reliable assertions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WidgetId {
    // Main layout areas
    MessagesArea,
    InputArea,
    StatusLine,
    QueuedMessages,

    // Status line components
    Spinner,
    TokenCounter,
    ElapsedTime,
    ModelName,
    SessionName,

    // Input components
    InputText,
    InputCursor,
    InputHint,

    // Message components
    MessageUser(u32),
    MessageAssistant(u32),
    MessageSystem(u32),
    MessageTool(u32),

    // Scroll indicators
    ScrollUp,
    ScrollDown,
    ScrollPosition,

    // Popups/overlays
    CommandPalette,
    SessionPicker,
    HelpOverlay,
    ErrorBanner,
    ReloadBanner,
}

impl WidgetId {
    /// Get a string representation for assertions.
    pub fn as_str(&self) -> &'static str {
        match self {
            WidgetId::MessagesArea => "messages_area",
            WidgetId::InputArea => "input_area",
            WidgetId::StatusLine => "status_line",
            WidgetId::QueuedMessages => "queued_messages",
            WidgetId::Spinner => "spinner",
            WidgetId::TokenCounter => "token_counter",
            WidgetId::ElapsedTime => "elapsed_time",
            WidgetId::ModelName => "model_name",
            WidgetId::SessionName => "session_name",
            WidgetId::InputText => "input_text",
            WidgetId::InputCursor => "input_cursor",
            WidgetId::InputHint => "input_hint",
            WidgetId::MessageUser(_) => "message_user",
            WidgetId::MessageAssistant(_) => "message_assistant",
            WidgetId::MessageSystem(_) => "message_system",
            WidgetId::MessageTool(_) => "message_tool",
            WidgetId::ScrollUp => "scroll_up",
            WidgetId::ScrollDown => "scroll_down",
            WidgetId::ScrollPosition => "scroll_position",
            WidgetId::CommandPalette => "command_palette",
            WidgetId::SessionPicker => "session_picker",
            WidgetId::HelpOverlay => "help_overlay",
            WidgetId::ErrorBanner => "error_banner",
            WidgetId::ReloadBanner => "reload_banner",
        }
    }
}

/// Widget location information for testing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WidgetInfo {
    pub id: WidgetId,
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    pub visible: bool,
    pub focused: bool,
    pub text_content: Option<String>,
}

/// Registry of widget locations for the current frame.
#[derive(Debug, Default)]
pub struct WidgetRegistry {
    widgets: Vec<WidgetInfo>,
}

impl WidgetRegistry {
    pub fn new() -> Self {
        Self {
            widgets: Vec::new(),
        }
    }

    /// Register a widget.
    pub fn register(&mut self, info: WidgetInfo) {
        self.widgets.push(info);
    }

    /// Find a widget by ID.
    pub fn find(&self, id: WidgetId) -> Option<&WidgetInfo> {
        self.widgets.iter().find(|w| w.id == id)
    }

    /// Get all widgets.
    pub fn all(&self) -> &[WidgetInfo] {
        &self.widgets
    }

    /// Clear the registry (call at start of each render).
    pub fn clear(&mut self) {
        self.widgets.clear();
    }

    /// Export to JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&self.widgets).unwrap_or_else(|_| "[]".to_string())
    }
}

// ============================================================================
// Test Script DSL
// ============================================================================

/// A test script for automated testing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestScript {
    /// Script name
    pub name: String,
    /// Description
    pub description: Option<String>,
    /// Steps to execute
    pub steps: Vec<TestStep>,
    /// Setup commands (run before steps)
    pub setup: Vec<String>,
    /// Teardown commands (run after steps)
    pub teardown: Vec<String>,
}

/// A single step in a test script.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action")]
pub enum TestStep {
    /// Send a message
    Message { content: String },
    /// Inject key presses
    Keys { keys: String },
    /// Set input text directly
    SetInput { text: String },
    /// Submit current input
    Submit,
    /// Wait for processing to complete
    WaitIdle { timeout_ms: Option<u64> },
    /// Wait fixed time
    Wait { ms: u64 },
    /// Run assertions
    Assert { assertions: Vec<serde_json::Value> },
    /// Take a snapshot
    Snapshot { name: String },
    /// Add checkpoint marker
    Checkpoint { name: String },
    /// Run arbitrary debug command
    Command { cmd: String },
    /// Scroll the view
    Scroll { direction: String },
}

impl TestScript {
    /// Create a new empty script.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            description: None,
            steps: Vec::new(),
            setup: Vec::new(),
            teardown: Vec::new(),
        }
    }

    /// Add a step.
    pub fn step(mut self, step: TestStep) -> Self {
        self.steps.push(step);
        self
    }

    /// Export to JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// Load from JSON.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

// ============================================================================
// Utility Functions
// ============================================================================

/// Strip ANSI escape codes from text.
pub fn strip_ansi(s: &str) -> String {
    // Simple regex-free ANSI stripper
    let mut result = String::new();
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip escape sequence
            if chars.peek() == Some(&'[') {
                chars.next(); // consume '['
                              // Skip until we hit a letter
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            result.push(c);
        }
    }

    result
}

/// Compare two strings ignoring whitespace differences.
pub fn strings_equal_normalized(a: &str, b: &str) -> bool {
    let normalize = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    normalize(a) == normalize(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clock_advance() {
        enable_test_clock();
        let clock = get_test_clock().unwrap();
        clock.write().unwrap().set(0);

        assert_eq!(now_ms(), 0);
        advance_clock(Duration::from_secs(1));
        assert_eq!(now_ms(), 1000);

        disable_test_clock();
    }

    #[test]
    fn test_event_recording() {
        let mut recorder = EventRecorder::new();
        recorder.start();

        recorder.record(TestEvent::Key {
            code: "a".to_string(),
            modifiers: vec![],
        });
        recorder.record(TestEvent::Key {
            code: "b".to_string(),
            modifiers: vec!["ctrl".to_string()],
        });

        recorder.stop();

        assert_eq!(recorder.events().len(), 2);
    }

    #[test]
    fn test_headless_buffer() {
        let mut buffer = HeadlessBuffer::new(80, 24);
        buffer.set(
            0,
            0,
            Cell {
                char: 'H',
                ..Default::default()
            },
        );
        buffer.set(
            1,
            0,
            Cell {
                char: 'i',
                ..Default::default()
            },
        );

        assert!(buffer.contains_text("Hi"));
        assert!(!buffer.contains_text("Hello"));
    }

    #[test]
    fn test_strip_ansi() {
        let input = "\x1b[32mgreen\x1b[0m text";
        assert_eq!(strip_ansi(input), "green text");
    }
}
