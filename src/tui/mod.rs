pub mod account_picker;
mod app;
pub mod backend;
pub(crate) mod color_support;
mod core;
pub mod image;
pub mod info_widget;
mod info_widget_layout;
mod info_widget_overview;
mod keybind;
mod layout_utils;
pub mod login_picker;
pub mod markdown;
mod memory_profile;
pub mod mermaid;
pub mod permissions;
mod remote_diff;
pub mod screenshot;
pub mod session_picker;
mod stream_buffer;
pub mod test_harness;
mod ui;
mod ui_diff;
pub mod usage_overlay;
pub mod visual_debug;
pub mod workspace_client;
pub use jcode_tui_workspace::workspace_map;
pub use jcode_tui_workspace::workspace_map_widget;

pub use app::{App, CopyBadgeUiState, DisplayMessage, ProcessingStatus, RunResult};

use crate::message::ToolCall;
use ratatui::prelude::Frame;
use ratatui::text::Line;
use std::time::Duration;

pub(crate) fn scheduled_notification_text(
    info: Option<&info_widget::AmbientWidgetData>,
) -> Option<String> {
    let info = info?;
    if info.reminder_count == 0 {
        return None;
    }
    let next = info.next_reminder_wake.as_deref()?;
    let suffix = if info.reminder_count > 1 {
        format!(" · {} queued", info.reminder_count)
    } else {
        String::new()
    };
    Some(format!("⏰ next scheduled task {}{}", next, suffix))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopySelectionPane {
    Chat,
    SidePane,
}

impl CopySelectionPane {
    pub fn label(self) -> &'static str {
        match self {
            Self::Chat => "Chat",
            Self::SidePane => "Side pane",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CopySelectionPoint {
    pub pane: CopySelectionPane,
    pub abs_line: usize,
    pub column: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CopySelectionRange {
    pub start: CopySelectionPoint,
    pub end: CopySelectionPoint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopySelectionStatus {
    pub pane: CopySelectionPane,
    pub has_action: bool,
    pub selected_chars: usize,
    pub selected_lines: usize,
    pub dragging: bool,
}

fn keyboard_enhancement_flags() -> crossterm::event::KeyboardEnhancementFlags {
    use crossterm::event::KeyboardEnhancementFlags;

    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
        | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
}

/// Enable Kitty keyboard protocol for unambiguous key reporting.
///
/// Intentionally avoid REPORT_ALL_KEYS_AS_ESCAPE_CODES for now. When that flag is enabled,
/// terminals such as kitty/Alacritty/Warp can report printable keys as a base key plus
/// modifiers instead of the final text produced by the active keyboard layout. Crossterm does
/// not yet expose kitty's associated text / alternate key data, so our printable fallback would
/// reconstruct characters using a US-centric shift map and break international layouts (for
/// example German macOS keyboards).
///
/// Returns true if successfully enabled, false if the terminal doesn't support it.
pub fn enable_keyboard_enhancement() -> bool {
    use crossterm::event::PushKeyboardEnhancementFlags;
    let result = crossterm::execute!(
        std::io::stdout(),
        PushKeyboardEnhancementFlags(keyboard_enhancement_flags())
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

/// Trait for TUI state consumed by the shared renderer.
pub trait TuiState {
    fn display_messages(&self) -> &[DisplayMessage];
    fn display_user_message_count(&self) -> usize;
    fn has_display_edit_tool_messages(&self) -> bool;
    fn side_pane_images(&self) -> Vec<crate::session::RenderedImage>;
    /// Version counter for display_messages (monotonic, increments on mutation)
    fn display_messages_version(&self) -> u64;
    fn streaming_text(&self) -> &str;
    fn input(&self) -> &str;
    fn cursor_pos(&self) -> usize;
    fn is_processing(&self) -> bool;
    fn queued_messages(&self) -> &[String];
    fn interleave_message(&self) -> Option<&str>;
    /// Messages sent as soft interrupt but not yet injected (shown in queue preview)
    fn pending_soft_interrupts(&self) -> &[String];
    fn scroll_offset(&self) -> usize;
    /// Whether auto-scroll to bottom is paused (user scrolled up during streaming)
    fn auto_scroll_paused(&self) -> bool;
    fn provider_name(&self) -> String;
    fn provider_model(&self) -> String;
    /// Upstream provider (e.g., which provider OpenRouter routed to)
    fn upstream_provider(&self) -> Option<String>;
    /// Active transport/connection type (websocket/https/etc.)
    fn connection_type(&self) -> Option<String>;
    /// Provider-supplied human-readable status detail for the current stream.
    fn status_detail(&self) -> Option<String>;
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
    /// Progress of a currently-running batch tool call.
    fn batch_progress(&self) -> Option<crate::bus::BatchProgress>;
    fn time_since_activity(&self) -> Option<Duration>;
    /// Whether the provider/server has ended the visible assistant message while turn cleanup
    /// still finishes in the background.
    fn stream_message_ended(&self) -> bool {
        false
    }
    /// Total session token usage (input, output) - used for high usage warnings
    fn total_session_tokens(&self) -> Option<(u64, u64)>;
    /// Whether running in remote (client-server) mode
    fn is_remote_mode(&self) -> bool;
    /// Whether running in canary/self-dev mode
    fn is_canary(&self) -> bool;
    /// Whether running in replay mode
    fn is_replay(&self) -> bool;
    /// Diff display mode (off/inline/full-inline/pinned/file)
    fn diff_mode(&self) -> crate::config::DiffDisplayMode;
    /// Current session ID (if available)
    fn current_session_id(&self) -> Option<String>;
    /// Session display name (memorable short name like "fox" or "oak")
    fn session_display_name(&self) -> Option<String>;
    /// Server display name (modifier like "running" or "blazing") - only set in remote mode
    fn server_display_name(&self) -> Option<String>;
    /// Server icon (e.g., "🔥", "🌫️") - only set in remote mode
    fn server_display_icon(&self) -> Option<String>;
    /// List of all session IDs on the server (remote mode only)
    fn server_sessions(&self) -> Vec<String>;
    /// Number of connected clients (remote mode only)
    fn connected_clients(&self) -> Option<usize>;
    /// Short-lived notice shown in the status line (e.g., model switch, toggle diff)
    fn status_notice(&self) -> Option<String>;
    /// Whether a transient remote startup phase is active and should keep redraws responsive.
    fn remote_startup_phase_active(&self) -> bool;
    /// Whether mouse-wheel smoothing has queued lines to animate.
    fn has_pending_mouse_scroll_animation(&self) -> bool {
        false
    }
    /// Optional configured keybinding label for external dictation.
    fn dictation_key_label(&self) -> Option<String>;
    /// Time since app started (for startup animations)
    fn animation_elapsed(&self) -> f32;
    /// Time remaining until rate limit resets (if rate limited)
    fn rate_limit_remaining(&self) -> Option<Duration>;
    /// Whether queue mode is enabled (true = wait, false = immediate)
    fn queue_mode(&self) -> bool;
    /// Whether the next normal prompt will be routed into a new headed session.
    fn next_prompt_new_session_armed(&self) -> bool {
        false
    }
    /// Whether there is a stashed input (saved via Ctrl+S)
    fn has_stashed_input(&self) -> bool;
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
    /// Whether workspace mode is enabled for this client.
    fn workspace_mode_enabled(&self) -> bool {
        false
    }
    /// Visible Niri-style workspace rows for the workspace-map widget.
    fn workspace_map_rows(&self) -> Vec<workspace_map::VisibleWorkspaceRow> {
        Vec::new()
    }
    /// Animation tick used for lightweight workspace map animation.
    fn workspace_animation_tick(&self) -> u64 {
        0
    }
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
    /// Whether the diagram pane ratio is currently animating
    fn diagram_pane_animating(&self) -> bool;
    /// Whether the pinned diagram pane is visible
    fn diagram_pane_enabled(&self) -> bool;
    /// Position of pinned diagram pane (side or top)
    fn diagram_pane_position(&self) -> crate::config::DiagramPanePosition;
    /// Diagram zoom percentage (100 = normal)
    fn diagram_zoom(&self) -> u8;
    /// Scroll offset for pinned diff pane (line index)
    fn diff_pane_scroll(&self) -> usize;
    /// Horizontal pan offset for the shared right pane (side-panel diagrams)
    fn diff_pane_scroll_x(&self) -> i32;
    /// Whether the pinned diff pane is focused
    fn diff_pane_focus(&self) -> bool;
    /// Session-scoped side panel state managed by the side_panel tool
    fn side_panel(&self) -> &crate::side_panel::SidePanelSnapshot;
    /// Whether to pin read images to a side pane
    fn pin_images(&self) -> bool;
    /// Whether to show a native terminal scrollbar for the chat viewport
    fn chat_native_scrollbar(&self) -> bool;
    /// Whether to show a native terminal scrollbar for the side panel
    fn side_panel_native_scrollbar(&self) -> bool;
    /// Whether to wrap lines in the pinned diff pane
    fn diff_line_wrap(&self) -> bool;
    /// Interactive inline UI state (picker-like flows shown above input)
    fn inline_interactive_state(&self) -> Option<&InlineInteractiveState>;
    /// Passive inline UI state (informational views shown above input)
    fn inline_view_state(&self) -> Option<&InlineViewState> {
        None
    }
    /// General inline UI state shown above input.
    fn inline_ui_state(&self) -> Option<InlineUiStateRef<'_>> {
        self.inline_interactive_state()
            .map(InlineUiStateRef::Interactive)
            .or_else(|| self.inline_view_state().map(InlineUiStateRef::View))
    }
    /// Changelog overlay scroll offset (None = not showing)
    fn changelog_scroll(&self) -> Option<usize>;
    /// Help overlay scroll offset (None = not showing)
    fn help_scroll(&self) -> Option<usize>;
    /// Session picker overlay for /resume command
    fn session_picker_overlay(&self) -> Option<&std::cell::RefCell<session_picker::SessionPicker>>;
    /// Login picker overlay for /login command
    fn login_picker_overlay(&self) -> Option<&std::cell::RefCell<login_picker::LoginPicker>>;
    /// Account picker overlay for /account command
    fn account_picker_overlay(&self) -> Option<&std::cell::RefCell<account_picker::AccountPicker>>;
    /// Usage overlay for /usage command
    fn usage_overlay(&self) -> Option<&std::cell::RefCell<usage_overlay::UsageOverlay>>;
    /// Working directory for this session
    fn working_dir(&self) -> Option<String>;
    /// Monotonic clock for viewport animations
    fn now_millis(&self) -> u64;
    /// UI state for live copy badge highlighting / feedback
    fn copy_badge_ui(&self) -> crate::tui::CopyBadgeUiState;
    /// Whether modal in-app copy selection mode is active.
    fn copy_selection_mode(&self) -> bool;
    /// Current in-app copy selection range, if any.
    fn copy_selection_range(&self) -> Option<CopySelectionRange>;
    /// Persistent status for in-app copy selection mode.
    fn copy_selection_status(&self) -> Option<CopySelectionStatus>;
    /// Suggestion prompts for new users (shown in initial empty state).
    /// Returns (label, prompt_text) pairs. Empty if user is experienced or not authenticated.
    fn suggestion_prompts(&self) -> Vec<(String, String)>;
    /// Cache TTL status - shows whether the prompt cache is warm/cold based on idle time
    fn cache_ttl_status(&self) -> Option<CacheTtlInfo>;
    /// Whether the notification line has content to show
    fn has_notification(&self) -> bool {
        if self.copy_selection_status().is_some() {
            return true;
        }
        if self.status_notice().is_some() {
            return true;
        }
        if self.has_stashed_input() {
            return true;
        }
        if !self.is_processing() {
            let info = self.info_widget_data();
            if scheduled_notification_text(info.ambient_info.as_ref()).is_some() {
                return true;
            }
            if let Some(cache_info) = self.cache_ttl_status()
                && (cache_info.is_cold || cache_info.remaining_secs <= 60)
            {
                return true;
            }
        }
        false
    }
}

pub(crate) fn connection_type_icon(connection_type: Option<&str>) -> Option<&'static str> {
    let normalized = connection_type?.trim().to_ascii_lowercase();
    if normalized.contains("websocket") || normalized == "ws" || normalized == "wss" {
        Some("🕸️")
    } else if normalized.contains("http") {
        Some("🌐")
    } else {
        None
    }
}

/// Cache TTL information for the current provider
#[derive(Debug, Clone)]
pub struct CacheTtlInfo {
    /// Seconds until cache expires (0 = already expired)
    pub remaining_secs: u64,
    /// Total TTL for this provider in seconds
    pub ttl_secs: u64,
    /// Whether the cache is expired (cold)
    pub is_cold: bool,
    /// Estimated cached tokens (from last response's input tokens)
    pub cached_tokens: Option<u64>,
}

/// Get the prompt cache TTL in seconds for a given provider name.
/// Returns None if the provider doesn't support prompt caching or TTL is unknown.
pub fn cache_ttl_for_provider(provider: &str) -> Option<u64> {
    match provider.to_lowercase().as_str() {
        "anthropic" | "claude" => Some(300),
        "openai" => Some(300),
        "openrouter" => Some(300),
        "copilot" => None,
        "cursor" => None,
        "antigravity" => None,
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerKind {
    Model,
    Account,
    Login,
    Usage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineInteractiveLayout {
    Compact,
    ThreeColumn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InlineInteractiveSchema {
    pub layout: InlineInteractiveLayout,
    pub primary_label: &'static str,
    pub secondary_label: &'static str,
    pub secondary_preview_label: &'static str,
    pub tertiary_label: &'static str,
    pub preview_submit_hint: &'static str,
    pub active_submit_hint: &'static str,
    pub shows_default_shortcut_hint: bool,
    pub preview_activation_column: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InlineViewState {
    pub title: String,
    pub status: Option<String>,
    pub lines: Vec<String>,
}

impl InlineViewState {
    pub fn debug_memory_profile(&self) -> serde_json::Value {
        let title_bytes = self.title.capacity();
        let status_bytes = self
            .status
            .as_ref()
            .map(|value| value.capacity())
            .unwrap_or(0);
        let lines_bytes: usize = self.lines.iter().map(|value| value.capacity()).sum();
        serde_json::json!({
            "lines_count": self.lines.len(),
            "title_bytes": title_bytes,
            "status_bytes": status_bytes,
            "lines_bytes": lines_bytes,
            "total_estimate_bytes": title_bytes + status_bytes + lines_bytes,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub enum InlineUiStateRef<'a> {
    View(&'a InlineViewState),
    Interactive(&'a InlineInteractiveState),
}

impl PickerKind {
    pub fn schema(&self) -> InlineInteractiveSchema {
        match self {
            Self::Model => InlineInteractiveSchema {
                layout: InlineInteractiveLayout::ThreeColumn,
                primary_label: "ITEM",
                secondary_label: "PROVIDER",
                secondary_preview_label: "PROVIDER",
                tertiary_label: "ACTION",
                preview_submit_hint: "  ↵ open",
                active_submit_hint: "  ↑↓ ←→ ↵ Esc",
                shows_default_shortcut_hint: true,
                preview_activation_column: 2,
            },
            Self::Account => InlineInteractiveSchema {
                layout: InlineInteractiveLayout::Compact,
                primary_label: "ACCOUNT",
                secondary_label: "STATE",
                secondary_preview_label: "STATE",
                tertiary_label: "",
                preview_submit_hint: "  ↵ select",
                active_submit_hint: "  ↑↓/jk ↵ Esc",
                shows_default_shortcut_hint: false,
                preview_activation_column: 0,
            },
            Self::Login => InlineInteractiveSchema {
                layout: InlineInteractiveLayout::ThreeColumn,
                primary_label: "ITEM",
                secondary_label: "PROVIDER",
                secondary_preview_label: "PROVIDER",
                tertiary_label: "ACTION",
                preview_submit_hint: "  ↵ open",
                active_submit_hint: "  ↑↓ ←→ ↵ Esc",
                shows_default_shortcut_hint: true,
                preview_activation_column: 2,
            },
            Self::Usage => InlineInteractiveSchema {
                layout: InlineInteractiveLayout::ThreeColumn,
                primary_label: "ITEM",
                secondary_label: "STATUS",
                secondary_preview_label: "ITEM",
                tertiary_label: "WINDOW",
                preview_submit_hint: "  ↵ inspect",
                active_submit_hint: "  ↑↓ ←→ ↵ Esc",
                shows_default_shortcut_hint: false,
                preview_activation_column: 2,
            },
        }
    }

    pub fn uses_compact_navigation(&self) -> bool {
        self.schema().layout == InlineInteractiveLayout::Compact
    }

    pub fn filter_text(&self, entry: &PickerEntry) -> String {
        match self {
            Self::Account => {
                let provider = entry
                    .active_option()
                    .map(|option| option.provider.as_str())
                    .unwrap_or("");
                let state = entry.account_state_label().unwrap_or("");
                format!("{} {} {}", entry.name, provider, state)
            }
            Self::Login => {
                let auth_kind = entry
                    .active_option()
                    .map(|option| option.provider.as_str())
                    .unwrap_or("");
                let state = entry
                    .active_option()
                    .map(|option| option.api_method.as_str())
                    .unwrap_or("");
                let detail = entry
                    .active_option()
                    .map(|option| option.detail.as_str())
                    .unwrap_or("");
                format!("{} {} {} {}", entry.name, auth_kind, state, detail)
            }
            Self::Usage => {
                let status = entry
                    .active_option()
                    .map(|option| option.provider.as_str())
                    .unwrap_or("");
                let window = entry
                    .active_option()
                    .map(|option| option.api_method.as_str())
                    .unwrap_or("");
                let detail = entry
                    .active_option()
                    .map(|option| option.detail.as_str())
                    .unwrap_or("");
                format!("{} {} {} {}", entry.name, status, window, detail)
            }
            Self::Model => entry.name.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountPickerAction {
    Switch { provider_id: String, label: String },
    Add { provider_id: String },
    Replace { provider_id: String, label: String },
    OpenCenter { provider_filter: Option<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentModelTarget {
    Swarm,
    Review,
    Judge,
    Memory,
    Ambient,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerAction {
    Model,
    Account(AccountPickerAction),
    Login(crate::provider_catalog::LoginProviderDescriptor),
    Usage {
        id: String,
        title: String,
        subtitle: String,
        status: crate::tui::usage_overlay::UsageOverlayStatus,
        detail_lines: Vec<String>,
    },
    AgentTarget(AgentModelTarget),
    AgentModelChoice {
        target: AgentModelTarget,
        clear_override: bool,
    },
}

/// Unified inline picker with three columns.
#[derive(Debug, Clone)]
pub struct InlineInteractiveState {
    /// Which inline picker is currently active.
    pub kind: PickerKind,
    /// All visible picker entries and their available actions/options.
    pub entries: Vec<PickerEntry>,
    /// Filtered indices into `entries`.
    pub filtered: Vec<usize>,
    /// Selected row in filtered list
    pub selected: usize,
    /// Active column: 0=primary item, 1=secondary option, 2=tertiary option.
    pub column: usize,
    /// Filter text applied to the picker kind's searchable text.
    pub filter: String,
    /// Preview mode: picker is visible but input stays in main text box
    pub preview: bool,
}

impl InlineInteractiveState {
    pub fn debug_memory_profile(&self) -> serde_json::Value {
        let entries_bytes: usize = self.entries.iter().map(estimate_picker_entry_bytes).sum();
        let filtered_bytes = self.filtered.capacity() * std::mem::size_of::<usize>();
        let filter_bytes = self.filter.capacity();
        serde_json::json!({
            "entries_count": self.entries.len(),
            "filtered_count": self.filtered.len(),
            "entries_bytes": entries_bytes,
            "filtered_bytes": filtered_bytes,
            "filter_bytes": filter_bytes,
            "total_estimate_bytes": entries_bytes + filtered_bytes + filter_bytes,
        })
    }
}

fn estimate_picker_action_bytes(action: &PickerAction) -> usize {
    match action {
        PickerAction::Model
        | PickerAction::AgentTarget(_)
        | PickerAction::AgentModelChoice { .. } => 0,
        PickerAction::Account(AccountPickerAction::Switch { provider_id, label }) => {
            provider_id.capacity() + label.capacity()
        }
        PickerAction::Account(AccountPickerAction::Add { provider_id }) => provider_id.capacity(),
        PickerAction::Account(AccountPickerAction::Replace { provider_id, label }) => {
            provider_id.capacity() + label.capacity()
        }
        PickerAction::Account(AccountPickerAction::OpenCenter { provider_filter }) => {
            provider_filter
                .as_ref()
                .map(|value| value.capacity())
                .unwrap_or(0)
        }
        PickerAction::Login(descriptor) => {
            descriptor.id.len()
                + descriptor.display_name.len()
                + descriptor
                    .aliases
                    .iter()
                    .map(|value| value.len())
                    .sum::<usize>()
                + descriptor.menu_detail.len()
        }
        PickerAction::Usage {
            id,
            title,
            subtitle,
            detail_lines,
            ..
        } => {
            id.capacity()
                + title.capacity()
                + subtitle.capacity()
                + detail_lines
                    .iter()
                    .map(|value| value.capacity())
                    .sum::<usize>()
        }
    }
}

fn estimate_picker_option_bytes(option: &PickerOption) -> usize {
    option.provider.capacity() + option.api_method.capacity() + option.detail.capacity()
}

fn estimate_picker_entry_bytes(entry: &PickerEntry) -> usize {
    entry.name.capacity()
        + entry
            .options
            .iter()
            .map(estimate_picker_option_bytes)
            .sum::<usize>()
        + estimate_picker_action_bytes(&entry.action)
        + entry
            .created_date
            .as_ref()
            .map(|value| value.capacity())
            .unwrap_or(0)
        + entry
            .effort
            .as_ref()
            .map(|value| value.capacity())
            .unwrap_or(0)
}

impl InlineInteractiveState {
    pub fn schema(&self) -> InlineInteractiveSchema {
        if self.is_agent_target_picker() {
            InlineInteractiveSchema {
                layout: InlineInteractiveLayout::ThreeColumn,
                primary_label: "TARGET",
                secondary_label: "MODEL",
                secondary_preview_label: "MODEL",
                tertiary_label: "CONFIG",
                preview_submit_hint: "  ↵ open",
                active_submit_hint: "  ↑↓ ←→ ↵ Esc",
                shows_default_shortcut_hint: false,
                preview_activation_column: 2,
            }
        } else {
            self.kind.schema()
        }
    }

    pub fn selected_entry_index(&self) -> Option<usize> {
        self.filtered.get(self.selected).copied()
    }

    pub fn selected_entry(&self) -> Option<&PickerEntry> {
        self.selected_entry_index()
            .and_then(|index| self.entries.get(index))
    }

    pub fn selected_entry_mut(&mut self) -> Option<&mut PickerEntry> {
        self.selected_entry_index()
            .and_then(|index| self.entries.get_mut(index))
    }

    pub fn is_agent_target_picker(&self) -> bool {
        self.kind == PickerKind::Model
            && !self.entries.is_empty()
            && self
                .entries
                .iter()
                .all(|entry| matches!(entry.action, PickerAction::AgentTarget(_)))
    }

    pub fn uses_compact_navigation(&self) -> bool {
        self.schema().layout == InlineInteractiveLayout::Compact
    }

    pub fn preview_submit_hint(&self) -> &'static str {
        self.schema().preview_submit_hint
    }

    pub fn active_submit_hint(&self) -> &'static str {
        self.schema().active_submit_hint
    }

    pub fn preview_activation_column(&self) -> usize {
        self.schema().preview_activation_column
    }

    pub fn max_navigable_column(&self) -> usize {
        match self.schema().layout {
            InlineInteractiveLayout::Compact => 0,
            InlineInteractiveLayout::ThreeColumn => 2,
        }
    }

    pub fn header_layout(&self, preview: bool) -> ([&'static str; 3], [usize; 3]) {
        if self.uses_compact_navigation() {
            (
                [self.primary_label(), self.secondary_label(preview), ""],
                [0, 0, 0],
            )
        } else if preview {
            (
                [
                    self.secondary_label(true),
                    self.primary_label(),
                    self.tertiary_label(),
                ],
                [1, 0, 2],
            )
        } else {
            (
                [
                    self.primary_label(),
                    self.secondary_label(false),
                    self.tertiary_label(),
                ],
                [0, 1, 2],
            )
        }
    }

    pub fn filter_text(&self, entry: &PickerEntry) -> String {
        if self.is_agent_target_picker() {
            let model = entry
                .active_option()
                .map(|option| option.provider.as_str())
                .unwrap_or("");
            let config = entry
                .active_option()
                .map(|option| option.api_method.as_str())
                .unwrap_or("");
            let detail = entry
                .active_option()
                .map(|option| option.detail.as_str())
                .unwrap_or("");
            format!("{} {} {} {}", entry.name, model, config, detail)
        } else {
            self.kind.filter_text(entry)
        }
    }

    pub fn primary_label(&self) -> &'static str {
        self.schema().primary_label
    }

    pub fn secondary_label(&self, preview: bool) -> &'static str {
        let schema = self.schema();
        if preview {
            schema.secondary_preview_label
        } else {
            schema.secondary_label
        }
    }

    pub fn tertiary_label(&self) -> &'static str {
        self.schema().tertiary_label
    }

    pub fn shows_default_shortcut_hint(&self) -> bool {
        self.schema().shows_default_shortcut_hint
    }
}

/// A reusable picker entry with one or more available actions/options.
#[derive(Debug, Clone)]
pub struct PickerEntry {
    pub name: String,
    pub options: Vec<PickerOption>,
    pub action: PickerAction,
    pub selected_option: usize,
    pub is_current: bool,
    pub is_default: bool,
    pub recommended: bool,
    pub recommendation_rank: usize,
    pub old: bool,
    /// Human-readable created date (e.g. "Jan 2026") for OpenRouter models
    pub created_date: Option<String>,
    pub effort: Option<String>,
}

impl PickerEntry {
    pub fn active_option(&self) -> Option<&PickerOption> {
        self.options.get(self.selected_option)
    }

    pub fn active_option_mut(&mut self) -> Option<&mut PickerOption> {
        self.options.get_mut(self.selected_option)
    }

    pub fn option_count(&self) -> usize {
        self.options.len()
    }

    pub fn account_state_label(&self) -> Option<&'static str> {
        match &self.action {
            PickerAction::Account(AccountPickerAction::Switch { .. }) => {
                Some(if self.is_current { "active" } else { "saved" })
            }
            PickerAction::Account(AccountPickerAction::Add { .. }) => Some("add"),
            PickerAction::Account(AccountPickerAction::Replace { .. }) => Some("replace"),
            PickerAction::Account(AccountPickerAction::OpenCenter { .. }) => Some("manage"),
            _ => None,
        }
    }
}

/// A single available option for a picker entry.
#[derive(Debug, Clone)]
pub struct PickerOption {
    pub provider: String,
    pub api_method: String,
    pub available: bool,
    pub detail: String,
    pub estimated_reference_cost_micros: Option<u64>,
}

pub(crate) const REDRAW_IDLE: Duration = Duration::from_millis(250);
pub(crate) const REDRAW_DEEP_IDLE: Duration = Duration::from_millis(1000);
pub(crate) const REDRAW_REMOTE_STARTUP: Duration = Duration::from_millis(1000);
pub(crate) const REDRAW_PASSIVE_LIVENESS: Duration = Duration::from_millis(1000);
const REDRAW_DEEP_IDLE_AFTER: Duration = Duration::from_secs(30);

fn idle_donut_active_with_policy(
    state: &dyn TuiState,
    policy: &crate::perf::TuiPerfPolicy,
) -> bool {
    if state.remote_startup_phase_active() {
        return false;
    }

    policy.enable_decorative_animations
        && crate::config::config().display.idle_animation
        && policy.tier.idle_animation_enabled()
        && state.display_messages().is_empty()
        && !state.is_processing()
        && state.streaming_text().is_empty()
        && state.queued_messages().is_empty()
}

pub(crate) fn idle_donut_active(state: &dyn TuiState) -> bool {
    let policy = crate::perf::tui_policy();
    idle_donut_active_with_policy(state, &policy)
}

fn fps_to_duration(fps: u32) -> Duration {
    Duration::from_millis((1000 / fps.max(1)) as u64)
}

pub(crate) fn redraw_interval_with_policy(
    state: &dyn TuiState,
    policy: &crate::perf::TuiPerfPolicy,
) -> Duration {
    let animation_interval = fps_to_duration(policy.animation_fps);
    let fast_interval = fps_to_duration(policy.redraw_fps);

    if idle_donut_active_with_policy(state, policy) {
        return match policy.tier {
            crate::perf::PerformanceTier::Minimal => fast_interval,
            _ => animation_interval,
        };
    }

    if !policy.enable_decorative_animations
        && !state.has_pending_mouse_scroll_animation()
        && state.status_notice().is_none()
        && !state.has_notification()
        && (state.is_processing() || state.rate_limit_remaining().is_some())
    {
        return REDRAW_PASSIVE_LIVENESS;
    }

    if state.is_processing()
        || !state.streaming_text().is_empty()
        || state.status_notice().is_some()
        || state.has_pending_mouse_scroll_animation()
        || state.has_notification()
        || state.rate_limit_remaining().is_some()
    {
        return match policy.tier {
            crate::perf::PerformanceTier::Minimal => REDRAW_IDLE,
            _ => fast_interval,
        };
    }

    if state.remote_startup_phase_active() {
        return REDRAW_REMOTE_STARTUP;
    }

    let deep_idle = state
        .time_since_activity()
        .map(|d| d >= REDRAW_DEEP_IDLE_AFTER)
        .unwrap_or(false);

    let cache_counting_down = state
        .cache_ttl_status()
        .map(|c| !c.is_cold && c.remaining_secs <= 60)
        .unwrap_or(false);

    if deep_idle && !cache_counting_down {
        REDRAW_DEEP_IDLE
    } else {
        REDRAW_IDLE
    }
}

pub(crate) fn redraw_interval(state: &dyn TuiState) -> Duration {
    let policy = crate::perf::tui_policy();
    redraw_interval_with_policy(state, &policy)
}

pub(crate) fn periodic_redraw_required(state: &dyn TuiState) -> bool {
    let policy = crate::perf::tui_policy();

    if idle_donut_active_with_policy(state, &policy) {
        return true;
    }

    if state.is_processing()
        || !state.streaming_text().is_empty()
        || state.status_notice().is_some()
        || state.has_pending_mouse_scroll_animation()
        || state.has_notification()
        || state.rate_limit_remaining().is_some()
        || state.remote_startup_phase_active()
    {
        return true;
    }

    state
        .cache_ttl_status()
        .map(|c| !c.is_cold && c.remaining_secs <= 60)
        .unwrap_or(false)
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

    let mut selfdev = crate::cli::selfdev::client_selfdev_requested();
    if !selfdev && let Some(ref dir) = working_dir {
        let mut current = Some(dir.as_path());
        while let Some(path) = current {
            if crate::build::is_jcode_repo(path) {
                selfdev = true;
                break;
            }
            current = path.parent();
        }
    }

    (working_dir_str, if selfdev { Some(true) } else { None })
}

/// Public wrapper to render a single frame (used by benchmarks/tools).
pub fn render_frame(frame: &mut Frame<'_>, state: &dyn TuiState) {
    ui::draw(frame, state);
}

pub use ui::SidePanelDebugStats;

pub fn display_messages_from_session(session: &crate::session::Session) -> Vec<DisplayMessage> {
    let mut messages: Vec<DisplayMessage> = crate::session::render_messages(session)
        .into_iter()
        .map(|item| DisplayMessage {
            role: item.role,
            content: item.content,
            tool_calls: item.tool_calls,
            duration_secs: None,
            title: None,
            tool_data: item.tool_data,
        })
        .collect();
    app::compact_display_messages_for_storage(&mut messages);
    messages
}

pub fn transcript_memory_profile(
    session: &crate::session::Session,
    resident_provider_messages: &[crate::message::Message],
    materialized_provider_messages: &[crate::message::Message],
    provider_view_source: &str,
    display_messages: &[DisplayMessage],
    side_panel: &crate::side_panel::SidePanelSnapshot,
) -> serde_json::Value {
    memory_profile::build_transcript_memory_profile(
        session,
        resident_provider_messages,
        materialized_provider_messages,
        provider_view_source,
        display_messages,
        side_panel,
    )
}

pub fn side_panel_debug_stats() -> SidePanelDebugStats {
    ui::side_panel_debug_stats()
}

pub fn reset_side_panel_debug_stats() {
    ui::reset_side_panel_debug_stats();
}

pub fn clear_side_panel_render_caches() {
    ui::clear_side_panel_render_caches();
}

pub fn prewarm_focused_side_panel(
    snapshot: &crate::side_panel::SidePanelSnapshot,
    terminal_width: u16,
    terminal_height: u16,
    ratio_percent: u8,
    has_protocol: bool,
    centered: bool,
) -> bool {
    ui::prewarm_focused_side_panel(
        snapshot,
        terminal_width,
        terminal_height,
        ratio_percent,
        has_protocol,
        centered,
    )
}

#[cfg(test)]
mod tests {
    use super::{
        connection_type_icon, is_unexpected_cache_miss, keyboard_enhancement_flags,
        scheduled_notification_text,
    };
    use crate::ambient::AmbientStatus;
    use crate::tui::info_widget::AmbientWidgetData;
    use crossterm::event::KeyboardEnhancementFlags;

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

    #[test]
    fn connection_type_icon_uses_protocol_specific_icons() {
        assert_eq!(connection_type_icon(Some("websocket")), Some("🕸️"));
        assert_eq!(connection_type_icon(Some("wss")), Some("🕸️"));
        assert_eq!(connection_type_icon(Some("https")), Some("🌐"));
        assert_eq!(connection_type_icon(Some("https/sse")), Some("🌐"));
        assert_eq!(connection_type_icon(Some("http")), Some("🌐"));
        assert_eq!(connection_type_icon(Some("unknown")), None);
        assert_eq!(connection_type_icon(None), None);
    }

    #[test]
    fn scheduled_notification_text_uses_session_reminder_count_only() {
        let info = AmbientWidgetData {
            show_widget: false,
            status: AmbientStatus::Disabled,
            queue_count: 88,
            next_queue_preview: Some("ambient backlog".to_string()),
            reminder_count: 2,
            next_reminder_preview: Some("follow up".to_string()),
            last_run_ago: None,
            last_summary: None,
            next_wake: Some("in 0s".to_string()),
            next_reminder_wake: Some("in 5m".to_string()),
            budget_percent: None,
        };

        assert_eq!(
            scheduled_notification_text(Some(&info)).as_deref(),
            Some("⏰ next scheduled task in 5m · 2 queued")
        );
    }

    #[test]
    fn keyboard_enhancement_flags_avoid_report_all_keys_escape_mode() {
        let flags = keyboard_enhancement_flags();

        assert!(flags.contains(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES));
        assert!(flags.contains(KeyboardEnhancementFlags::REPORT_EVENT_TYPES));
        assert!(!flags.contains(KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES));
    }
}
