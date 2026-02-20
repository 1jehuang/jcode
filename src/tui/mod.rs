mod app;
pub mod backend;
mod core;
pub mod image;
pub mod info_widget;
mod keybind;
pub mod markdown;
pub mod mermaid;
pub mod permissions;
pub mod screenshot;
pub mod session_picker;
mod stream_buffer;
pub mod test_harness;
mod ui;
pub mod visual_debug;

// ClientApp is deprecated - use App::new_for_remote().run_remote() instead
#[deprecated(note = "Use App::new_for_remote().run_remote() instead")]
pub mod client;

pub use app::{App, DisplayMessage, ProcessingStatus, RunResult};
pub use backend::{DebugEvent, DebugMessage, RemoteConnection};
pub use core::TuiCore;

use crate::message::ToolCall;
use ratatui::prelude::Frame;
use ratatui::text::Line;
use std::time::Duration;

/// Enable Kitty keyboard protocol for unambiguous key reporting.
/// Returns true if successfully enabled, false if the terminal doesn't support it.
pub fn enable_keyboard_enhancement() -> bool {
    use crossterm::event::{KeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
    let result = crossterm::execute!(
        std::io::stdout(),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )
    .is_ok();
    crate::logging::info(&format!(
        "Kitty keyboard protocol: {}",
        if result { "enabled" } else { "FAILED" }
    ));
    result
}

/// Disable Kitty keyboard protocol, restoring default key reporting.
pub fn disable_keyboard_enhancement() {
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::event::PopKeyboardEnhancementFlags
    );
}

/// Trait for TUI state - implemented by both App and ClientApp
/// This allows sharing the UI rendering code between standalone and client modes
pub trait TuiState {
    fn display_messages(&self) -> &[DisplayMessage];
    /// Version counter for display_messages (monotonic, increments on mutation)
    fn display_messages_version(&self) -> u64;
    fn streaming_text(&self) -> &str;
    fn input(&self) -> &str;
    fn cursor_pos(&self) -> usize;
    fn is_processing(&self) -> bool;
    fn queued_messages(&self) -> &[String];
    fn interleave_message(&self) -> Option<&str>;
    /// Message sent as soft interrupt but not yet injected (shown in queue preview)
    fn pending_soft_interrupt(&self) -> Option<&str>;
    fn scroll_offset(&self) -> usize;
    /// Whether auto-scroll to bottom is paused (user scrolled up during streaming)
    fn auto_scroll_paused(&self) -> bool;
    fn provider_name(&self) -> String;
    fn provider_model(&self) -> String;
    /// Upstream provider (e.g., which provider OpenRouter routed to)
    fn upstream_provider(&self) -> Option<String>;
    fn mcp_servers(&self) -> Vec<(String, usize)>;
    fn available_skills(&self) -> Vec<String>;
    fn streaming_tokens(&self) -> (u64, u64);
    fn streaming_cache_tokens(&self) -> (Option<u64>, Option<u64>);
    /// Output tokens per second during streaming (for status bar)
    fn output_tps(&self) -> Option<f32>;
    fn streaming_tool_calls(&self) -> Vec<ToolCall>;
    fn elapsed(&self) -> Option<Duration>;
    fn status(&self) -> ProcessingStatus;
    fn command_suggestions(&self) -> Vec<(String, &'static str)>;
    fn active_skill(&self) -> Option<String>;
    fn subagent_status(&self) -> Option<String>;
    fn time_since_activity(&self) -> Option<Duration>;
    /// Total session token usage (input, output) - used for high usage warnings
    fn total_session_tokens(&self) -> Option<(u64, u64)>;
    /// Whether running in remote (client-server) mode
    fn is_remote_mode(&self) -> bool;
    /// Whether running in canary/self-dev mode
    fn is_canary(&self) -> bool;
    /// Whether running in replay mode
    fn is_replay(&self) -> bool;
    /// Whether to show diffs for edit/write tools
    fn show_diffs(&self) -> bool;
    /// Current session ID (if available)
    fn current_session_id(&self) -> Option<String>;
    /// Session display name (memorable short name like "fox" or "oak")
    fn session_display_name(&self) -> Option<String>;
    /// List of all session IDs on the server (remote mode only)
    fn server_sessions(&self) -> Vec<String>;
    /// Number of connected clients (remote mode only)
    fn connected_clients(&self) -> Option<usize>;
    /// Short-lived notice shown in the status line (e.g., model switch, toggle diff)
    fn status_notice(&self) -> Option<String>;
    /// Time since app started (for startup animations)
    fn animation_elapsed(&self) -> f32;
    /// Time remaining until rate limit resets (if rate limited)
    fn rate_limit_remaining(&self) -> Option<Duration>;
    /// Whether queue mode is enabled (true = wait, false = immediate)
    fn queue_mode(&self) -> bool;
    /// Context info (what's loaded in context window - static + dynamic)
    fn context_info(&self) -> crate::prompt::ContextInfo;
    /// Context window limit in tokens (if known)
    fn context_limit(&self) -> Option<usize>;
    /// Whether a newer client binary is available
    fn client_update_available(&self) -> bool;
    /// Whether a newer server binary is available (remote mode)
    fn server_update_available(&self) -> Option<bool>;
    /// Get info widget data (todos, client count, etc.)
    fn info_widget_data(&self) -> info_widget::InfoWidgetData;
    /// Render streaming text using incremental markdown renderer
    /// This is more efficient than re-rendering on every frame
    fn render_streaming_markdown(&self, width: usize) -> Vec<Line<'static>>;
    /// Whether centered mode is enabled
    fn centered_mode(&self) -> bool;
    /// Authentication status for all supported providers
    fn auth_status(&self) -> crate::auth::AuthStatus;
    /// Update cost calculation based on token usage (for API-key providers)
    fn update_cost(&mut self);
    /// Diagram display mode (none/margin/pinned)
    fn diagram_mode(&self) -> crate::config::DiagramDisplayMode;
    /// Whether the diagram pane is focused (pinned mode)
    fn diagram_focus(&self) -> bool;
    /// Selected diagram index (pinned mode, most-recent = 0)
    fn diagram_index(&self) -> usize;
    /// Diagram scroll offsets in cells (x, y) when focused
    fn diagram_scroll(&self) -> (i32, i32);
    /// Diagram pane width ratio percentage
    fn diagram_pane_ratio(&self) -> u8;
    /// Whether the pinned diagram pane is visible
    fn diagram_pane_enabled(&self) -> bool;
    /// Position of pinned diagram pane (side or top)
    fn diagram_pane_position(&self) -> crate::config::DiagramPanePosition;
    /// Diagram zoom percentage (100 = normal)
    fn diagram_zoom(&self) -> u8;
    /// Interactive model/provider picker state (shown as inline row above input)
    fn picker_state(&self) -> Option<&PickerState>;
    /// Working directory for this session
    fn working_dir(&self) -> Option<String>;
}

/// Unified model/provider picker with three columns
#[derive(Debug, Clone)]
pub struct PickerState {
    /// All unique model entries with their routes
    pub models: Vec<ModelEntry>,
    /// Filtered indices into `models` (by model filter)
    pub filtered: Vec<usize>,
    /// Selected row in filtered list
    pub selected: usize,
    /// Active column: 0=model, 1=provider, 2=via
    pub column: usize,
    /// Filter text (applies to model column)
    pub filter: String,
    /// Preview mode: picker is visible but input stays in main text box
    pub preview: bool,
}

/// A unique model with its available routes
#[derive(Debug, Clone)]
pub struct ModelEntry {
    pub name: String,
    pub routes: Vec<RouteOption>,
    pub selected_route: usize,
    pub is_current: bool,
    pub recommended: bool,
    pub effort: Option<String>,
}

/// A single route to reach a model
#[derive(Debug, Clone)]
pub struct RouteOption {
    pub provider: String,
    pub api_method: String,
    pub available: bool,
    pub detail: String,
}

pub(crate) const REDRAW_FAST: Duration = Duration::from_millis(50);
pub(crate) const REDRAW_IDLE: Duration = Duration::from_millis(250);
pub(crate) const REDRAW_DEEP_IDLE: Duration = Duration::from_millis(1000);
const REDRAW_DEEP_IDLE_AFTER: Duration = Duration::from_secs(30);
pub(crate) const STARTUP_ANIMATION_WINDOW: Duration = Duration::from_millis(3000);

pub(crate) fn startup_animation_active(state: &dyn TuiState) -> bool {
    crate::config::config().display.startup_animation
        && state.animation_elapsed() < STARTUP_ANIMATION_WINDOW.as_secs_f32()
        && !state.is_processing()
        && state.display_messages().is_empty()
        && state.streaming_text().is_empty()
        && state.input().trim().is_empty()
        && state.queued_messages().is_empty()
        && state.interleave_message().is_none()
        && state.pending_soft_interrupt().is_none()
        && state.picker_state().is_none()
}

pub(crate) fn idle_donut_active(state: &dyn TuiState) -> bool {
    crate::config::config().display.idle_animation
        && state.display_messages().is_empty()
        && !state.is_processing()
        && state.streaming_text().is_empty()
        && state.queued_messages().is_empty()
}

pub(crate) fn should_animate(state: &dyn TuiState) -> bool {
    state.is_processing()
        || state.status_notice().is_some()
        || state.rate_limit_remaining().is_some()
        || startup_animation_active(state)
        || idle_donut_active(state)
}

pub(crate) fn redraw_interval(state: &dyn TuiState) -> Duration {
    if should_animate(state) {
        return REDRAW_FAST;
    }

    let deep_idle = state
        .time_since_activity()
        .map(|d| d >= REDRAW_DEEP_IDLE_AFTER)
        .unwrap_or(false);
    if deep_idle {
        REDRAW_DEEP_IDLE
    } else {
        REDRAW_IDLE
    }
}

/// Returns true when cache behavior is unexpected for a multi-turn conversation.
///
/// Anthropic conversation caching is usually warmed on turn 2 (cache creation without reads),
/// so misses are only unexpected from turn 3 onward.
pub(crate) fn is_unexpected_cache_miss(
    user_turn_count: usize,
    cache_read: Option<u64>,
    cache_creation: Option<u64>,
) -> bool {
    user_turn_count > 2 && cache_creation.unwrap_or(0) > 0 && cache_read.unwrap_or(0) == 0
}

pub(crate) fn subscribe_metadata() -> (Option<String>, Option<bool>) {
    let working_dir = std::env::current_dir().ok();
    let working_dir_str = working_dir.as_ref().map(|p| p.display().to_string());

    let mut selfdev = std::env::var("JCODE_SELFDEV_MODE").is_ok();
    if !selfdev {
        if let Some(ref dir) = working_dir {
            let mut current = Some(dir.as_path());
            while let Some(path) = current {
                if crate::build::is_jcode_repo(path) {
                    selfdev = true;
                    break;
                }
                current = path.parent();
            }
        }
    }

    (working_dir_str, if selfdev { Some(true) } else { None })
}

/// Public wrapper to render a single frame (used by benchmarks/tools).
pub fn render_frame(frame: &mut Frame<'_>, state: &dyn TuiState) {
    ui::draw(frame, state);
}

#[cfg(test)]
mod tests {
    use super::is_unexpected_cache_miss;

    #[test]
    fn cache_creation_only_on_turn_two_is_expected() {
        assert!(!is_unexpected_cache_miss(2, Some(0), Some(12_000)));
    }

    #[test]
    fn cache_creation_only_on_later_turns_is_unexpected() {
        assert!(is_unexpected_cache_miss(3, Some(0), Some(12_000)));
    }

    #[test]
    fn cache_reads_disable_miss_warning() {
        assert!(!is_unexpected_cache_miss(3, Some(8_000), Some(12_000)));
    }

    #[test]
    fn no_cache_creation_is_not_a_miss() {
        assert!(!is_unexpected_cache_miss(3, Some(0), Some(0)));
    }
}
