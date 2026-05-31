//! Screenshot Automation Support
//!
//! Provides hooks for autonomous screenshot capture by emitting signals
//! that external capture scripts can watch for.

use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

/// Whether screenshot automation is enabled
static SCREENSHOT_MODE: AtomicBool = AtomicBool::new(false);

/// Get the screenshot signal directory
fn signal_dir() -> PathBuf {
    crate::storage::runtime_dir().join("jcode-screenshots")
}

/// Enable screenshot automation mode
pub fn enable() {
    SCREENSHOT_MODE.store(true, Ordering::SeqCst);
    let dir = signal_dir();
    let _ = fs::create_dir_all(&dir);
    crate::logging::info(&format!("Screenshot mode enabled. Signal dir: {:?}", dir));
}

/// Disable screenshot automation mode
pub fn disable() {
    SCREENSHOT_MODE.store(false, Ordering::SeqCst);
}

/// Check if screenshot mode is enabled
pub fn is_enabled() -> bool {
    SCREENSHOT_MODE.load(Ordering::SeqCst)
}

/// Signal that a specific UI state is ready for capture
///
/// This writes a signal file that capture scripts can watch for.
/// The signal file contains metadata about the state.
///
/// # Example
/// ```ignore
/// screenshot::signal_ready("streaming", json!({
///     "tokens": 150,
///     "elapsed_ms": 2500,
/// }));
/// ```
pub fn signal_ready(state_name: &str, metadata: serde_json::Value) {
    if !is_enabled() {
        return;
    }

    let dir = signal_dir();
    let signal_path = dir.join(format!("{}.ready", state_name));

    let content = serde_json::json!({
        "state": state_name,
        "timestamp": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0),
        "metadata": metadata,
    });

    if let Ok(mut file) = File::create(&signal_path) {
        let _ = file.write_all(content.to_string().as_bytes());
        crate::logging::debug(&format!("Screenshot signal: {}", state_name));
    }
}

/// Clear a signal (called after screenshot is taken)
pub fn clear_signal(state_name: &str) {
    let signal_path = signal_dir().join(format!("{}.ready", state_name));
    let _ = fs::remove_file(signal_path);
}

/// Clear all signals
pub fn clear_all_signals() {
    if let Ok(entries) = fs::read_dir(signal_dir()) {
        for entry in entries.flatten() {
            if entry
                .path()
                .extension()
                .map(|e| e == "ready")
                .unwrap_or(false)
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

/// Predefined screenshot states that can be triggered
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenshotState {
    /// Clean main UI with InfoWidget visible
    MainUi,
    /// Command palette open (after typing /)
    CommandPalette,
    /// Session picker open
    SessionPicker,
    /// During streaming response (mid-stream)
    Streaming,
    /// Streaming complete
    StreamingComplete,
    /// Tool execution in progress
    ToolRunning,
    /// Info widget expanded
    InfoWidgetExpanded,
    /// Error state
    Error,
}

impl ScreenshotState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MainUi => "main-ui",
            Self::CommandPalette => "command-palette",
            Self::SessionPicker => "session-picker",
            Self::Streaming => "streaming",
            Self::StreamingComplete => "streaming-complete",
            Self::ToolRunning => "tool-running",
            Self::InfoWidgetExpanded => "info-widget-expanded",
            Self::Error => "error",
        }
    }

    /// Signal this state is ready for capture
    pub fn signal(&self, metadata: serde_json::Value) {
        signal_ready(self.as_str(), metadata);
    }
}
