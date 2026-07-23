pub mod account_picker;
pub(crate) mod app;

#[derive(Clone)]
pub struct ContextSnapshot {
    pub info: Option<crate::prompt::ContextInfo>,
    pub revision: u64,
    pub fresh: bool,
}

pub mod backend;
pub(crate) mod color_support;
mod core;
pub(crate) mod fuzzy;
// Terminal image display + metadata helpers now live in the dependency-free
// `jcode-terminal-image` crate (shared with the `read` tool). Re-exported here
// so existing `crate::tui::image` / `crate::tui::image_metadata` paths keep working.
pub use jcode_terminal_image::display as image;
use jcode_terminal_image::metadata as image_metadata;
pub mod info_widget;
mod info_widget_layout;
mod info_widget_overview;
pub mod info_widget_stability;
pub mod keybind;
mod layout_utils;
pub mod login_picker;
pub mod markdown;
mod memory_profile;
pub mod mermaid;
pub mod permissions {
    pub use jcode_tui_permissions::*;
}
mod remote_diff;
pub mod screenshot;
pub(crate) mod session_facts;
pub mod session_picker;
mod stream_buffer;
pub mod test_harness;
pub mod theme_detect;
mod ui;
mod ui_diff;
pub mod usage_overlay;
pub mod visual_debug;
pub mod workspace_client;
pub use jcode_tui_workspace::workspace_map;
pub use jcode_tui_workspace::workspace_map_widget;

pub use crate::generated_image::{
    generated_image_side_panel_markdown, generated_image_side_panel_page_id,
    write_generated_image_side_panel_page,
};
pub use app::{App, CopyBadgeUiState, ProcessingStatus, RunResult};

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

pub(crate) use self::core::DisplayMessageRoleExt;
pub use jcode_tui_core::{
    CopySelectionPane, CopySelectionPoint, CopySelectionRange, CopySelectionStatus,
};
pub use jcode_tui_messages::DisplayMessage;

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
/// not yet expose kitty's associated text / alternate key data, so we cannot safely reconstruct
/// shifted symbols for every keyboard layout. Prefer the terminal-delivered printable character
/// and only synthesize ASCII letter casing in the input fallback.
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

/// Hash a rendered image's transcript anchor into `hasher`. Shared by the
/// default and `App` implementations of `side_pane_images_signature` so both
/// stay in lockstep.
pub(crate) fn hash_rendered_image_anchor(
    anchor: Option<&crate::session::RenderedImageAnchor>,
    hasher: &mut impl std::hash::Hasher,
) {
    use std::hash::Hash;
    match anchor {
        None => 0u8.hash(hasher),
        Some(crate::session::RenderedImageAnchor::ToolCall { id }) => {
            1u8.hash(hasher);
            id.hash(hasher);
        }
        Some(crate::session::RenderedImageAnchor::UserPrompt { ordinal }) => {
            2u8.hash(hasher);
            ordinal.hash(hasher);
        }
    }
}

/// Hash every field that affects inline image rendering. The production App
/// memoizes this signature until its image set changes, so exact payload hashing
/// happens on image updates rather than during scrolling. Correctness matters
/// here: sampling can miss a same-length change and reuse a stale prepared frame.
pub(crate) fn hash_rendered_image_signature_fields(
    image: &crate::session::RenderedImage,
    hasher: &mut impl std::hash::Hasher,
) {
    use std::hash::Hash;

    image.media_type.hash(hasher);
    image.data.hash(hasher);
    image.label.hash(hasher);
    hash_rendered_image_anchor(image.anchor.as_ref(), hasher);
}

/// Trait for TUI state consumed by the shared renderer.
///
/// This is a wide (114-method) presentation interface: the read-only surface the
/// renderer needs from `App`. The methods are grouped into the domain sections
/// below (transcript, input, scroll, stream/status, provider, session/server,
/// workspace, diagram pane, diff pane, side panel, inline, overlay, copy
/// selection, onboarding, misc). See `docs/TUISTATE_TRAIT_DECOMPOSITION.md` for
/// the incremental plan to split these into composable sub-traits.
pub trait TuiState {
    // ---- Transcript ----
    fn display_messages(&self) -> &[DisplayMessage];
    fn display_user_message_count(&self) -> usize;
    /// Number of user prompts hidden before the first visible message because of
    /// compacted-history truncation. Used to keep prompt numbers absolute.
    fn compacted_hidden_user_prompts(&self) -> usize {
        0
    }
    fn has_display_edit_tool_messages(&self) -> bool;
    fn side_pane_images(&self) -> Vec<crate::session::RenderedImage>;
    /// Cheap signature of the current inline-image set: `(count, content_hash)`.
    /// Used by the prepared-frame cache so the inline image section invalidates
    /// when images are added/removed without cloning the payloads every frame.
    /// The default implementation derives it from `side_pane_images`; overrides
    /// can provide a cheaper path.
    fn side_pane_images_signature(&self) -> (usize, u64) {
        use std::hash::Hasher;
        let images = self.side_pane_images();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for image in &images {
            hash_rendered_image_signature_fields(image, &mut hasher);
        }
        (images.len(), hasher.finish())
    }
    /// Version counter for display_messages (monotonic, increments on mutation)
    fn display_messages_version(&self) -> u64;
    fn streaming_text(&self) -> &str;

    // ---- Input ----
    fn input(&self) -> &str;
    fn cursor_pos(&self) -> usize;
    fn is_processing(&self) -> bool;
    fn queued_messages(&self) -> &[String];
    fn interleave_message(&self) -> Option<&str>;
    /// Messages sent as soft interrupt but not yet injected (shown in queue preview)
    fn pending_soft_interrupts(&self) -> &[String];

    // ---- Scroll ----
    fn scroll_offset(&self) -> usize;
    /// Whether auto-scroll to bottom is paused (user scrolled up during streaming)
    fn auto_scroll_paused(&self) -> bool;
    /// When older compacted history is being loaded in, this is the reader's
    /// captured distance (in wrapped lines) from the bottom of the transcript.
    /// The renderer uses it to keep the viewport anchored to the same content as
    /// older messages are prepended above, instead of snapping to the new top.
    fn pending_history_anchor_lines_from_bottom(&self) -> Option<usize> {
        None
    }
    /// Whether the elastic overscroll status line (revealed by scrolling past
    /// the bottom of the transcript) is currently shown.
    fn chat_overscroll_active(&self) -> bool {
        false
    }
    /// Whether the overscroll status line is pinned permanently visible by
    /// config (`display.overscroll_status = "on"`). A pinned line is part of
    /// the stable layout, unlike the transient elastic reveal.
    fn chat_overscroll_pinned(&self) -> bool {
        false
    }
    /// Seconds remaining in the overscroll dwell window, used to render the
    /// `(overscroll x.x)` countdown. `None` when not shown.
    fn chat_overscroll_remaining(&self) -> Option<f32> {
        None
    }
    /// Whether a mouse drag-selection is currently held at the top/bottom edge of
    /// a pane and should keep auto-scrolling on every tick (browser-style). When
    /// true the redraw loop must stay responsive even if the transcript is
    /// otherwise idle, since the terminal sends no further events while the mouse
    /// is held still.
    fn copy_selection_edge_autoscroll_active(&self) -> bool {
        false
    }

    // ---- Provider ----
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

    // ---- Stream / status ----
    fn streaming_tokens(&self) -> (u64, u64);
    fn streaming_cache_tokens(&self) -> (Option<u64>, Option<u64>);
    /// Output tokens per second during streaming (for status bar)
    fn output_tps(&self) -> Option<f32>;
    fn streaming_tool_calls(&self) -> Vec<ToolCall>;
    fn elapsed(&self) -> Option<Duration>;
    /// Time since the current connection phase (authenticating/connecting/
    /// waiting for response/retrying) began. Used to decide when a connection
    /// attempt has been suspiciously long and should render yellow, measured
    /// per-attempt rather than inheriting the whole-turn elapsed time. Defaults
    /// to `elapsed()` for impls that do not track per-phase timing.
    fn connection_phase_elapsed(&self) -> Option<Duration> {
        self.elapsed()
    }
    fn status(&self) -> ProcessingStatus;
    fn command_suggestions(&self) -> Vec<(String, &'static str)>;
    fn command_suggestion_selected(&self) -> usize {
        0
    }
    fn active_skill(&self) -> Option<String>;
    fn subagent_status(&self) -> Option<String>;
    /// Progress of a currently-running batch tool call.
    fn batch_progress(&self) -> Option<crate::bus::BatchProgress>;
    fn time_since_activity(&self) -> Option<Duration>;
    /// Whether the client terminal currently has focus. Decorative animations and
    /// periodic idle redraws pause while unfocused so backgrounded windows/tabs do
    /// not burn CPU. Defaults to true for state impls that do not track focus.
    fn client_focused(&self) -> bool {
        true
    }
    /// Whether the provider/server has ended the visible assistant message while turn cleanup
    /// still finishes in the background.
    fn stream_message_ended(&self) -> bool {
        false
    }
    /// Total session token usage (input, output) - used for high usage warnings
    fn total_session_tokens(&self) -> Option<(u64, u64)>;
    /// Number of jcode compactions already applied to this session, when known.
    fn session_compaction_count(&self) -> usize {
        0
    }

    // ---- Session / server ----
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
    /// Server binary version (e.g., "v0.25.19-dev (abc1234)") - remote mode only
    fn server_display_version(&self) -> Option<String> {
        None
    }
    /// List of all session IDs on the server (remote mode only)
    fn server_sessions(&self) -> Vec<String>;
    /// Number of connected clients (remote mode only)
    fn connected_clients(&self) -> Option<usize>;
    /// Short-lived notice shown in the status line (e.g., model switch, toggle diff)
    fn status_notice(&self) -> Option<String>;
    /// Distinct learned-keybinding nudge shown in its own pop-out color, e.g.
    /// "you usually do X the slow way, press <key>". Separate from
    /// [`status_notice`] so the UI can style it differently.
    fn learn_hint(&self) -> Option<String> {
        None
    }
    /// Inline hotkey feedback: "you just pressed X → does Y" for rarely-used
    /// known chords, or "X isn't bound · nearest ..." for unknown chords.
    fn hotkey_feedback(&self) -> Option<String> {
        None
    }
    /// First-use experimental feature warning for the currently active operation.
    fn active_experimental_feature_notice(&self) -> Option<String> {
        None
    }
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
    /// Authoritative, freshness-tagged context snapshot used by widgets.
    fn context_snapshot(&self) -> ContextSnapshot {
        let info = self.context_info();
        ContextSnapshot {
            info: (info.total_chars > 0).then_some(info),
            revision: 0,
            fresh: true,
        }
    }
    /// Context window limit in tokens (if known)
    fn context_limit(&self) -> Option<usize>;
    /// Whether floating information widgets should be drawn for this state.
    /// Implementations normally use the default; deterministic render fixtures
    /// can suppress overlays without mutating process-global widget settings.
    fn info_widget_overlays_enabled(&self) -> bool {
        true
    }
    /// Whether a newer client binary is available
    fn client_update_available(&self) -> bool;
    /// Whether a newer server binary is available (remote mode)
    fn server_update_available(&self) -> Option<bool>;
    /// Get info widget data (todos, client count, etc.)
    fn info_widget_data(&self) -> info_widget::InfoWidgetData;

    /// Whether the inline swarm gallery band should be shown above the chat.
    /// Active when `agents.swarm_spawn_mode = inline` and the swarm has members.
    fn inline_swarm_gallery_active(&self) -> bool {
        false
    }
    /// Members to render in the inline swarm gallery band.
    fn inline_swarm_members(&self) -> Vec<crate::protocol::SwarmMemberStatus> {
        Vec::new()
    }
    /// Members available for cards embedded beneath swarm spawn tool calls.
    ///
    /// This may be broader than `inline_swarm_members`: the gallery is scoped by
    /// the current ownership tree, while a transcript card can be matched safely
    /// using the exact spawned session ID recorded in the tool result.
    fn swarm_members_for_transcript(&self) -> Vec<crate::protocol::SwarmMemberStatus> {
        self.inline_swarm_members()
    }
    /// Selected agent index in the inline swarm panel (display order).
    fn swarm_panel_selected(&self) -> usize {
        0
    }
    /// Whether the inline swarm panel currently has keyboard focus.
    fn swarm_panel_focused(&self) -> bool {
        false
    }
    /// Whether the live swarm page currently replaces the transcript viewport.
    fn swarm_panel_full_page(&self) -> bool {
        false
    }

    // ---- Workspace ----
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
    // ---- Diagram pane ----
    fn diagram_mode(&self) -> crate::config::DiagramDisplayMode;
    /// Whether the diagram pane is focused (pinned mode)
    fn diagram_focus(&self) -> bool;
    /// Selected diagram index (pinned mode, most-recent = 0)
    fn diagram_index(&self) -> usize;
    /// Diagram scroll offsets in cells (x, y) when focused
    fn diagram_scroll(&self) -> (i32, i32);
    /// Diagram pane width ratio percentage
    fn diagram_pane_ratio(&self) -> u8;
    /// Whether the user has manually resized the diagram/side pane width.
    fn diagram_pane_ratio_user_adjusted(&self) -> bool;
    /// Whether the diagram pane ratio is currently animating
    fn diagram_pane_animating(&self) -> bool;
    /// Whether the pinned diagram pane is visible
    fn diagram_pane_enabled(&self) -> bool;
    /// Position of pinned diagram pane (side or top)
    fn diagram_pane_position(&self) -> crate::config::DiagramPanePosition;
    /// Diagram zoom percentage (100 = normal)
    fn diagram_zoom(&self) -> u8;
    /// Scroll offset for pinned diff pane (line index)
    // ---- Diff pane ----
    fn diff_pane_scroll(&self) -> usize;
    /// Horizontal pan offset for the shared right pane (side-panel diagrams)
    fn diff_pane_scroll_x(&self) -> i32;
    /// Zoom percentage for image widgets rendered inside the side panel.
    fn side_panel_image_zoom_percent(&self) -> u8;
    /// Whether the pinned diff pane is focused
    fn diff_pane_focus(&self) -> bool;
    /// Session-scoped side panel state managed by the side_panel tool
    // ---- Side panel ----
    fn side_panel(&self) -> &crate::side_panel::SidePanelSnapshot;
    /// Whether to pin read images to a side pane
    fn pin_images(&self) -> bool;
    /// Whether inline transcript images render expanded. When false, each
    /// image collapses to a one-line label stub with a `show image` badge.
    /// Persisted across restarts/resume via UI preferences.
    fn inline_images_visible(&self) -> bool {
        true
    }
    /// Per-image inline expand level for `image_id` (Fit when never expanded).
    /// Cycled by clicking the per-image `expand` badge.
    fn image_expand_level(&self, _image_id: u64) -> ImageExpandLevel {
        ImageExpandLevel::Fit
    }
    /// Monotonic counter bumped whenever any image's expand level changes, so
    /// prepared-frame caches that embed anchored image geometry invalidate.
    fn expanded_images_version(&self) -> u64 {
        0
    }
    /// Remaining seconds before the pinned image side pane auto-hides.
    fn pinned_images_auto_hide_remaining_secs(&self) -> Option<u64> {
        None
    }
    /// Whether to show a native terminal scrollbar for the chat viewport
    fn chat_native_scrollbar(&self) -> bool;
    /// Whether to show a native terminal scrollbar for the side panel
    fn side_panel_native_scrollbar(&self) -> bool;
    /// Whether to wrap lines in the pinned diff pane
    fn diff_line_wrap(&self) -> bool;
    /// Interactive inline UI state (picker-like flows shown above input)
    // ---- Inline ----
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
    // ---- Overlay ----
    fn changelog_scroll(&self) -> Option<usize>;
    /// Help overlay scroll offset (None = not showing)
    fn help_scroll(&self) -> Option<usize>;
    /// Model status overlay scroll offset and markdown content (None = not showing)
    fn model_status_overlay(&self) -> Option<(usize, &str)> {
        None
    }
    /// Session picker overlay for /resume command
    fn session_picker_overlay(&self) -> Option<&std::cell::RefCell<session_picker::SessionPicker>>;
    /// Login picker overlay for /login command
    fn login_picker_overlay(&self) -> Option<&std::cell::RefCell<login_picker::LoginPicker>>;
    /// Account picker overlay for /account command
    fn account_picker_overlay(&self) -> Option<&std::cell::RefCell<account_picker::AccountPicker>>;
    /// Usage overlay for /usage command
    fn usage_overlay(&self) -> Option<&std::cell::RefCell<usage_overlay::UsageOverlay>>;
    /// Working directory for this session
    // ---- Misc ----
    fn working_dir(&self) -> Option<String>;
    /// Monotonic clock for viewport animations
    fn now_millis(&self) -> u64;
    /// UI state for live copy badge highlighting / feedback
    // ---- Copy selection ----
    fn copy_badge_ui(&self) -> crate::tui::CopyBadgeUiState;
    /// Whether modal in-app copy selection mode is active.
    fn copy_selection_mode(&self) -> bool;
    /// Current in-app copy selection range, if any.
    fn copy_selection_range(&self) -> Option<CopySelectionRange>;
    /// Persistent status for in-app copy selection mode.
    fn copy_selection_status(&self) -> Option<CopySelectionStatus>;
    /// Whether the first-run onboarding empty state is being previewed in this session.
    // ---- Onboarding ----
    fn onboarding_preview_mode(&self) -> bool {
        false
    }
    /// Whether to render the dedicated first-run onboarding welcome screen
    /// (gray telemetry header, prominent donut, welcome text, and the login
    /// prompt). True for brand-new installs / unauthenticated users, or when
    /// previewing onboarding.
    fn onboarding_welcome_active(&self) -> bool {
        self.onboarding_preview_mode()
    }
    /// What the onboarding welcome screen should render in its body. Returns
    /// `Suggestions` by default (the starter cards); the guided flow overrides
    /// this to drive the model-select and continue-prompt phases.
    fn onboarding_welcome_kind(&self) -> OnboardingWelcomeKind {
        OnboardingWelcomeKind::Suggestions
    }
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
        if crate::tui::ui::recent_flicker_ui_notice().is_some() {
            return true;
        }
        if self.status_notice().is_some() {
            return true;
        }
        if self.learn_hint().is_some() {
            return true;
        }
        if self.hotkey_feedback().is_some() {
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
                && (cache_info.is_cold || cache_info.expiring_soon())
            {
                return true;
            }
        }
        false
    }
}

#[cfg(feature = "dev-bins")]
pub fn debug_copy_selection_text_for_bench(range: CopySelectionRange) -> Option<String> {
    ui::copy_selection_text(range)
}

pub(crate) fn connection_type_icon(connection_type: Option<&str>) -> Option<&'static str> {
    let normalized = connection_type?.trim().to_ascii_lowercase();
    if normalized.contains("websocket") || normalized == "ws" || normalized == "wss" {
        // 🔌 is a single emoji-default codepoint. The previous 🕸️ (U+1F578 +
        // VS16) is text-default and rendered as a monochrome outline/tofu in
        // macOS window titles (Ghostty/Terminal ignore the VS16 selector there).
        Some("🔌")
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
    /// How long ago the cache went cold, in seconds (0 while warm)
    pub cold_for_secs: u64,
    /// Estimated cached tokens (from last response's input tokens)
    pub cached_tokens: Option<u64>,
}

/// Compact human age like `30s`, `5m`, `1h 1m`, `2d 3h` for "went cold N ago"
/// annotations. Keeps at most two units so it stays glanceable.
pub(crate) fn format_compact_age(secs: u64) -> String {
    if secs < 60 {
        return format!("{}s", secs);
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{}m", mins);
    }
    let hours = mins / 60;
    let rem_mins = mins % 60;
    if hours < 24 {
        return if rem_mins == 0 {
            format!("{}h", hours)
        } else {
            format!("{}h {}m", hours, rem_mins)
        };
    }
    let days = hours / 24;
    let rem_hours = hours % 24;
    if rem_hours == 0 {
        format!("{}d", days)
    } else {
        format!("{}d {}h", days, rem_hours)
    }
}

impl CacheTtlInfo {
    /// How long before expiry the `⏳ cache ...` countdown should appear.
    ///
    /// A fixed 60s window is fine for a 5-minute TTL but far too easy to miss
    /// on a 1-hour (or 24-hour) TTL where stepping away is exactly the failure
    /// mode. Scale with the TTL (10%) but keep it within 60s..10min so short
    /// TTLs keep their old behavior and long TTLs don't nag for hours.
    pub fn warn_window_secs(&self) -> u64 {
        (self.ttl_secs / 10).clamp(60, 600)
    }

    /// Whether the cache is warm but close enough to expiry that the
    /// countdown should be shown (and idle redraws kept alive).
    pub fn expiring_soon(&self) -> bool {
        !self.is_cold && self.remaining_secs <= self.warn_window_secs()
    }
}

/// Prompt cache TTL helpers now live in `crate::provider` (provider
/// cache-retention policy); re-exported here for existing tui call sites.
pub use crate::provider::{cache_ttl_for_provider, cache_ttl_for_provider_model};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KvCacheProblemKind {
    /// The provider explicitly reported new cache creation on a turn where we expected
    /// an already-warm cache to be read instead.
    UnexpectedCacheCreation,
    /// The provider explicitly reported zero cached input tokens on a turn where this
    /// provider family should report cached tokens for a warm, cacheable conversation.
    ExpectedCacheReadMissing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KvCacheProblem {
    pub kind: KvCacheProblemKind,
    pub affected_tokens: Option<u64>,
}

impl KvCacheProblem {
    pub(crate) fn log_reason(self) -> &'static str {
        match self.kind {
            KvCacheProblemKind::UnexpectedCacheCreation => "unexpected_cache_creation",
            KvCacheProblemKind::ExpectedCacheReadMissing => "expected_cache_read_missing",
        }
    }
}

fn normalized_provider_matches(provider: &str, needle: &str) -> bool {
    provider.trim().to_ascii_lowercase().contains(needle)
}

fn provider_stack_contains(provider: &str, upstream_provider: Option<&str>, needle: &str) -> bool {
    let needle = &needle.to_ascii_lowercase();
    normalized_provider_matches(provider, needle)
        || upstream_provider
            .map(|upstream| normalized_provider_matches(upstream, needle))
            .unwrap_or(false)
}

fn provider_stack_contains_any(
    provider: &str,
    upstream_provider: Option<&str>,
    needles: &[&str],
) -> bool {
    needles
        .iter()
        .any(|needle| provider_stack_contains(provider, upstream_provider, needle))
}

fn supports_reliable_zero_cache_read_warning(
    provider: &str,
    upstream_provider: Option<&str>,
) -> bool {
    if provider_stack_contains_any(
        provider,
        upstream_provider,
        &["openai", "anthropic", "claude", "gemini", "google"],
    ) {
        return true;
    }

    // OpenRouter/Jcode-subscription routes can only be treated as reliable for zero-read
    // warnings once the upstream provider identifies a known cache-reporting family.
    // A bare OpenRouter route with cached_tokens=0 is not enough: some upstreams simply
    // do not implement prompt caching, and warning on those would make the UI untrustworthy.
    false
}

fn min_cacheable_input_tokens(provider: &str, upstream_provider: Option<&str>) -> u64 {
    if provider_stack_contains_any(provider, upstream_provider, &["gemini", "google"]) {
        // Be conservative for Gemini-style implicit caching. Several Gemini models have
        // higher minimums than OpenAI/Anthropic; a higher UI threshold avoids warning on
        // prompts that might legitimately be below the provider's cacheable size.
        4_096
    } else {
        1_024
    }
}

fn cache_expected_warm(cache_ttl: Option<&CacheTtlInfo>) -> bool {
    cache_ttl.map(|info| !info.is_cold).unwrap_or(false)
}

/// Detect a KV/prompt-cache problem that is reliable enough to surface in the UI.
///
/// This intentionally does **not** warn merely because a cache-hit metric is absent. A warning
/// requires all of the following:
/// - a multi-turn conversation where cache reuse should be possible;
/// - a prior completed turn still within the provider's expected cache TTL;
/// - explicit provider telemetry showing either a cache rewrite without a read, or an explicit
///   zero cache-read count from a known cache-reporting provider family;
/// - enough input tokens to be cacheable for read-only providers.
pub(crate) fn detect_kv_cache_problem(
    provider: &str,
    upstream_provider: Option<&str>,
    user_turn_count: usize,
    input_tokens: u64,
    cache_read: Option<u64>,
    cache_creation: Option<u64>,
    cache_ttl: Option<&CacheTtlInfo>,
) -> Option<KvCacheProblem> {
    if user_turn_count <= 2 || !cache_expected_warm(cache_ttl) {
        return None;
    }

    let cache_read_tokens = cache_read.unwrap_or(0);
    let cache_creation_tokens = cache_creation.unwrap_or(0);

    // Strongest signal: the provider explicitly says it created cache but read none.
    if cache_creation_tokens > 0 && cache_read_tokens == 0 {
        return Some(KvCacheProblem {
            kind: KvCacheProblemKind::UnexpectedCacheCreation,
            affected_tokens: Some(cache_creation_tokens),
        });
    }

    // Read-only telemetry providers (OpenAI/Gemini and known OpenRouter upstreams) do not expose
    // cache creation tokens. For those, an explicit zero read on a warm, cacheable conversation is
    // the reliable signal. Absence of the metric is ignored.
    if cache_read != Some(0) {
        return None;
    }

    if !supports_reliable_zero_cache_read_warning(provider, upstream_provider) {
        return None;
    }

    if input_tokens < min_cacheable_input_tokens(provider, upstream_provider) {
        return None;
    }

    Some(KvCacheProblem {
        kind: KvCacheProblemKind::ExpectedCacheReadMissing,
        affected_tokens: Some(input_tokens),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerKind {
    Model,
    Account,
    Login,
    Usage,
}

/// What the first-run onboarding welcome screen should render in its body,
/// driven by the active onboarding flow phase. `Suggestions` is the default
/// resting state (the starter prompt cards).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnboardingWelcomeKind {
    /// Ask the user to log in first. Shown on a fresh install that booted
    /// without working credentials.
    ///
    /// When `import` is `Some`, we detected importable external logins and are
    /// walking the user through them one at a time (a yes/no prompt per login).
    /// When `None` and `importing` is false, there was nothing to import and the
    /// card points the user at the provider picker. When `None` and `importing`
    /// is true, the user just committed the import and it is running, so the card
    /// shows an "Importing your logins..." progress state. When `error` is
    /// `Some`, a prior import failed and the recovery copy explains what went
    /// wrong plus the concrete next step.
    Login {
        import: Option<LoginImportPrompt>,
        importing: bool,
        error: Option<String>,
        /// When a prior import failed and we detected a coding agent the user
        /// recently used, its display label (e.g. "Codex"). The recovery screen
        /// offers "Press H to have <label> help fix this". `None` hides that
        /// option.
        repair_agent_label: Option<String>,
    },
    /// Ask the user whether to log in to OpenAI (no detected imports). A
    /// highlightable Yes/No selector; `yes_highlighted` reflects the current
    /// choice. Yes starts the OpenAI sign-in, No skips login and finishes
    /// onboarding (the user can run `/login` later).
    LoginOpenAi { yes_highlighted: bool },
    /// "Continue where you left off in <cli>?" with a highlightable Yes/No
    /// selector and a live decision countdown (seconds remaining).
    ContinuePrompt {
        cli_label: String,
        yes_highlighted: bool,
        seconds_left: u64,
    },
    /// The starter prompt-suggestion cards (default).
    Suggestions,
}

/// Render-friendly snapshot of the single-screen login-import checkbox list.
/// Carries every detected login plus which ones are checked and which row the
/// cursor is on, so the welcome card can draw the whole list at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginImportPrompt {
    /// One entry per detected login, in display order.
    pub rows: Vec<LoginImportRow>,
    /// Index of the row the cursor is currently on.
    pub cursor: usize,
    /// When `true`, the navigable "Continue" pill is focused. On the summary
    /// screen this is the preselected default; in choose mode it means focus is
    /// on the pill rather than a login row, so Enter commits the import.
    pub continue_focused: bool,
    /// `false` = the default summary screen (detected logins listed read-only,
    /// with Continue / Choose pills). `true` = the per-login checkbox list.
    pub choosing: bool,
    /// How many rows are currently checked for import.
    pub checked_count: usize,
    /// Seconds left before the screen auto-imports all checked logins.
    pub seconds_left: u64,
}

/// One row in the login-import checkbox list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginImportRow {
    /// Human-readable provider summary (e.g. "OpenAI/Codex").
    pub provider_summary: String,
    /// Where the credentials came from (e.g. "Codex auth.json").
    pub source_name: String,
    /// Whether this login is checked for import.
    pub checked: bool,
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
                primary_label: "MODEL",
                secondary_label: "PROVIDER",
                secondary_preview_label: "PROVIDER",
                tertiary_label: "METHOD",
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
            Self::Model => {
                let route = entry.active_option();
                let provider = route.map(|option| option.provider.as_str()).unwrap_or("");
                let method = route.map(|option| option.api_method.as_str()).unwrap_or("");
                let detail = route.map(|option| option.detail.as_str()).unwrap_or("");
                format!("{} {} {} {}", entry.name, provider, method, detail)
            }
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
    Logout(crate::provider_catalog::LoginProviderDescriptor),
    LogoutAll,
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
        | PickerAction::AgentModelChoice { .. }
        | PickerAction::LogoutAll => 0,
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
        PickerAction::Login(descriptor) | PickerAction::Logout(descriptor) => {
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
    pub is_favorite: bool,
    pub recommended: bool,
    pub recommendation_rank: usize,
    pub usage_score: u32,
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
pub(crate) const REDRAW_DEEP_IDLE: Duration = Duration::from_millis(5000);
pub(crate) const REDRAW_REMOTE_STARTUP: Duration = Duration::from_millis(1000);
pub(crate) const REDRAW_PASSIVE_LIVENESS: Duration = Duration::from_millis(1000);
pub(crate) const REDRAW_DEEP_IDLE_AFTER: Duration = Duration::from_secs(30);

fn idle_donut_active_with_policy(
    state: &dyn TuiState,
    policy: &crate::perf::TuiPerfPolicy,
) -> bool {
    if state.remote_startup_phase_active() {
        return false;
    }

    // Decorative animations are purely visual; never spin them while the terminal
    // window/tab is backgrounded. A swarm of unfocused sessions would otherwise
    // each render a full-screen 3D scene at animation FPS, saturating every core.
    if !state.client_focused() {
        return false;
    }

    // The onboarding welcome screen draws the same live donut, but it also
    // shows a welcome/login card so `display_messages()` is not empty.  Keep the
    // animation loop running smoothly while that screen is up (even past the
    // deep-idle threshold) so the donut spins as an attention grab instead of
    // only repainting on input events.
    if state.onboarding_welcome_active() {
        return policy.enable_decorative_animations
            && crate::config::config().display.idle_animation
            && policy.tier.idle_animation_enabled();
    }

    // The idle donut is decorative.  Leaving many dormant tabs/sessions open
    // should not keep every TUI repainting forever, especially when those tabs
    // are hidden behind a terminal multiplexer or kitty single-instance window.
    if state
        .time_since_activity()
        .map(|d| d >= REDRAW_DEEP_IDLE_AFTER)
        .unwrap_or(false)
    {
        return false;
    }

    policy.enable_decorative_animations
        && crate::config::config().display.idle_animation
        && policy.tier.idle_animation_enabled()
        && !has_started_conversation(state)
        && !state.is_processing()
        && state.streaming_text().is_empty()
        && state.queued_messages().is_empty()
}

/// Whether the transcript contains any real conversation yet (a user prompt or
/// an assistant/tool/reasoning reply). A fresh screen that only holds
/// non-conversational notices (e.g. the "run /login when you're ready" system
/// message left after onboarding is declined) is still "idle", so the decorative
/// donut should keep spinning until the user actually starts chatting.
fn has_started_conversation(state: &dyn TuiState) -> bool {
    state
        .display_messages()
        .iter()
        .any(|m| matches!(m.role.as_str(), "user" | "assistant" | "tool" | "reasoning"))
}

pub(crate) fn idle_donut_active(state: &dyn TuiState) -> bool {
    let policy = crate::perf::tui_policy();
    idle_donut_active_with_policy(state, &policy)
}

fn rate_limit_countdown_redraw_active(state: &dyn TuiState) -> bool {
    state
        .rate_limit_remaining()
        .map(|remaining| remaining <= Duration::from_secs(60))
        .unwrap_or(false)
}

/// The notification line shows a live prompt-cache indicator (`⏳ cache Ns`
/// while warm in the final minute, `🧊 cache cold` once expired). Both states
/// emerge long after the 30s deep-idle cutoff, so without a dedicated wakeup
/// the idle loop never repaints to reveal them. Keep redrawing whenever the
/// cache is within the last-minute countdown window or has just gone cold so
/// the warning actually appears before the next prompt.
fn cache_cold_countdown_redraw_active(state: &dyn TuiState) -> bool {
    if state.is_processing() {
        return false;
    }
    state
        .cache_ttl_status()
        .map(|info| info.is_cold || info.expiring_soon())
        .unwrap_or(false)
}

fn full_frame_status_animation_active_with_policy(
    state: &dyn TuiState,
    policy: &crate::perf::TuiPerfPolicy,
) -> bool {
    if !policy.enable_decorative_animations {
        return false;
    }

    // These animations are rendered as part of the full status line, not by the
    // spinner-only cell renderer in app/run_shell.rs, so they need the normal
    // active redraw loop while visible.
    matches!(state.status(), ProcessingStatus::RunningTool(_))
        || rate_limit_countdown_redraw_active(state)
        || crate::build::read_build_progress().is_some()
}

fn primary_status_spinner_fast_path_available_with_policy(
    state: &dyn TuiState,
    _policy: &crate::perf::TuiPerfPolicy,
) -> bool {
    // The single-cell spinner fast path is available in every performance tier,
    // including Minimal/SSH/WSL where decorative animations are off. Keep these
    // conditions in sync with `app::run_shell::status_spinner_only_symbol`, which
    // is what actually gates the spinner-only tick in the run loop.
    state.is_processing()
        && app::run_shell::status_uses_primary_spinner(&state.status())
        && state.streaming_text().is_empty()
        && !state.centered_mode()
        && !state.has_pending_mouse_scroll_animation()
        && !state.remote_startup_phase_active()
}

fn primary_status_spinner_needs_full_redraw_with_policy(
    state: &dyn TuiState,
    policy: &crate::perf::TuiPerfPolicy,
) -> bool {
    // The primary spinner only needs the more expensive full-redraw cadence when
    // the cheap single-cell fast path cannot run (e.g. centered composer). When
    // the fast path is available we keep full redraws at the slow passive-liveness
    // rate and let the one-cell renderer animate the spinner.
    state.is_processing()
        && app::run_shell::status_uses_primary_spinner(&state.status())
        && state.streaming_text().is_empty()
        && !primary_status_spinner_fast_path_available_with_policy(state, policy)
}

/// Redraw cadence while an inline swarm or session-picker spinner is active.
/// This matches the glyph's wall-clock cadence and the primary status spinner:
/// faster wastes unchanged frames, while slower makes the motion visibly step.
pub(crate) const REDRAW_SWARM_SPINNER: Duration =
    Duration::from_millis(jcode_tui_render::swarm_gallery::STRIP_SPINNER_FRAME_MS);

/// Whether the swarm strip (above the status line) or the SwarmStatus dock
/// widget is currently animating a status spinner for an active agent.
///
/// Both surfaces derive the spinner glyph from the wall clock, but managed
/// agents keep running long after the coordinator session itself goes quiet.
/// Without a dedicated wakeup the idle loop stops repainting (deep idle stops
/// it entirely) and the spinner freezes, only twitching when a bus update
/// happens to arrive. Unfocused clients skip this so backgrounded windows do
/// not burn CPU animating a glyph nobody can see; terminal statuses render
/// fixed glyphs and need no animation frames.
fn swarm_spinner_redraw_active(state: &dyn TuiState) -> bool {
    state.client_focused()
        && state
            .inline_swarm_members()
            .iter()
            .any(|m| jcode_tui_render::swarm_gallery::is_active_status(&m.status))
}

/// Whether the open `/resume` picker is showing at least one running session.
/// The picker uses the same 8 fps spinner cells as the swarm strip, so it needs
/// an explicit wakeup even when the session underneath the overlay is idle.
fn session_picker_spinner_redraw_active(state: &dyn TuiState) -> bool {
    state.client_focused()
        && state.session_picker_overlay().is_some_and(|picker| {
            picker
                .try_borrow()
                .ok()
                .is_some_and(|picker| picker.has_visible_running_sessions())
        })
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

    // A retained/collapsing reasoning trace used to need animation cadence here;
    // anchored traces are static transcript messages now. The tail-follow
    // catch-up slide still needs smooth frames and must skip the deep-idle
    // short-circuits below.
    if ui::tail_catchup_active() {
        return match policy.tier {
            crate::perf::PerformanceTier::Minimal => fast_interval,
            _ => animation_interval,
        };
    }

    // The elastic overscroll line shows a live `(overscroll x.x)` countdown that
    // depletes over ~1.5s. Without a dedicated branch it falls through to the
    // 250ms idle cadence and ticks in coarse, steppy jumps. Drive it at the
    // smooth animation cadence so the countdown reads as continuous. A line
    // pinned on by config has no countdown (`remaining` is None) and must not
    // pin the redraw loop at animation cadence forever.
    if state.chat_overscroll_remaining().is_some() {
        return match policy.tier {
            crate::perf::PerformanceTier::Minimal => fast_interval,
            _ => animation_interval,
        };
    }

    // While the terminal is backgrounded (FocusLost), an idle session has nothing
    // worth a fast tick: decorative animations are paused and the run loop only
    // repaints throttled idle frames. Use the slow deep-idle interval so the
    // event loop sleeps instead of spinning on shared-server bus chatter. Sessions
    // with live output keep a responsive cadence below.
    if !state.client_focused()
        && !state.is_processing()
        && state.streaming_text().is_empty()
        && !state.has_pending_mouse_scroll_animation()
        && !state.copy_selection_edge_autoscroll_active()
        && !state.remote_startup_phase_active()
        && !rate_limit_countdown_redraw_active(state)
        && crate::build::read_build_progress().is_none()
    {
        return REDRAW_DEEP_IDLE;
    }

    let deep_idle = state
        .time_since_activity()
        .map(|d| d >= REDRAW_DEEP_IDLE_AFTER)
        .unwrap_or(false);

    if deep_idle
        && !state.is_processing()
        && state.streaming_text().is_empty()
        && !state.has_pending_mouse_scroll_animation()
        && !state.copy_selection_edge_autoscroll_active()
        && !state.remote_startup_phase_active()
        && !rate_limit_countdown_redraw_active(state)
        && !cache_cold_countdown_redraw_active(state)
        && crate::build::read_build_progress().is_none()
        && !state.onboarding_welcome_active()
        && !swarm_spinner_redraw_active(state)
        && !session_picker_spinner_redraw_active(state)
    {
        return REDRAW_DEEP_IDLE;
    }

    if idle_donut_active_with_policy(state, policy) {
        return match policy.tier {
            crate::perf::PerformanceTier::Minimal => fast_interval,
            _ => animation_interval,
        };
    }

    if full_frame_status_animation_active_with_policy(state, policy) {
        return match policy.tier {
            crate::perf::PerformanceTier::Minimal => REDRAW_IDLE,
            _ => fast_interval,
        };
    }

    if primary_status_spinner_needs_full_redraw_with_policy(state, policy) {
        return match policy.tier {
            crate::perf::PerformanceTier::Minimal => REDRAW_IDLE,
            _ => fast_interval,
        };
    }

    // Swarm status spinners animate at a fixed 12.5 fps off the wall clock.
    // Streaming/scroll branches below already repaint faster than this, but
    // both the quiet-coordinator case and the processing-without-streaming
    // case (which otherwise idles at the 1s passive-liveness cadence) need
    // this to keep agent spinners smooth while the swarm works.
    if (swarm_spinner_redraw_active(state) || session_picker_spinner_redraw_active(state))
        && state.streaming_text().is_empty()
        && !state.has_pending_mouse_scroll_animation()
    {
        return match policy.tier {
            // Minimal tier drops decorative animation; a liveness-rate tick
            // still advances the glyph so agents never look frozen.
            crate::perf::PerformanceTier::Minimal => REDRAW_PASSIVE_LIVENESS,
            _ => REDRAW_SWARM_SPINNER,
        };
    }

    if !state.has_pending_mouse_scroll_animation()
        && state.streaming_text().is_empty()
        && (state.is_processing() || rate_limit_countdown_redraw_active(state))
    {
        return REDRAW_PASSIVE_LIVENESS;
    }

    if state.is_processing()
        || !state.streaming_text().is_empty()
        || state.status_notice().is_some()
        || state.learn_hint().is_some()
        || state.has_pending_mouse_scroll_animation()
        || state.copy_selection_edge_autoscroll_active()
        || state.has_notification()
        || rate_limit_countdown_redraw_active(state)
    {
        return match policy.tier {
            crate::perf::PerformanceTier::Minimal => REDRAW_IDLE,
            _ => fast_interval,
        };
    }

    if state.remote_startup_phase_active() {
        return REDRAW_REMOTE_STARTUP;
    }

    if deep_idle {
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

    let deep_idle = state
        .time_since_activity()
        .map(|d| d >= REDRAW_DEEP_IDLE_AFTER)
        .unwrap_or(false);

    if deep_idle
        && !state.is_processing()
        && state.streaming_text().is_empty()
        && !state.has_pending_mouse_scroll_animation()
        && !state.copy_selection_edge_autoscroll_active()
        // Only the elastic countdown needs ticks; a config-pinned line is static.
        && state.chat_overscroll_remaining().is_none()
        && !state.remote_startup_phase_active()
        && !rate_limit_countdown_redraw_active(state)
        && !cache_cold_countdown_redraw_active(state)
        && crate::build::read_build_progress().is_none()
        && !state.onboarding_welcome_active()
        && !swarm_spinner_redraw_active(state)
        && !session_picker_spinner_redraw_active(state)
    {
        return false;
    }

    if idle_donut_active_with_policy(state, &policy) {
        return true;
    }

    if full_frame_status_animation_active_with_policy(state, &policy) {
        return true;
    }

    if swarm_spinner_redraw_active(state) {
        return true;
    }

    if session_picker_spinner_redraw_active(state) {
        return true;
    }

    if state.is_processing()
        || !state.streaming_text().is_empty()
        || ui::tail_catchup_active()
        || state.status_notice().is_some()
        || state.learn_hint().is_some()
        || state.has_pending_mouse_scroll_animation()
        || state.copy_selection_edge_autoscroll_active()
        || state.chat_overscroll_remaining().is_some()
        || state.has_notification()
        || rate_limit_countdown_redraw_active(state)
        || state.remote_startup_phase_active()
    {
        return true;
    }

    false
}

pub(crate) fn subscribe_metadata(
    remote_working_dir: Option<&str>,
) -> (Option<String>, Option<bool>) {
    let working_dir = std::env::current_dir().ok();
    resolve_subscribe_metadata(
        working_dir.as_deref(),
        remote_working_dir,
        jcode_selfdev_types::client_selfdev_requested(),
    )
}

pub(crate) fn resolve_subscribe_metadata(
    client_working_dir: Option<&std::path::Path>,
    remote_working_dir: Option<&str>,
    client_selfdev_requested: bool,
) -> (Option<String>, Option<bool>) {
    let working_dir_str = remote_working_dir
        .map(str::to_string)
        .or_else(|| client_working_dir.map(|p| p.display().to_string()));

    let mut selfdev = client_selfdev_requested;
    if !selfdev && let Some(dir) = client_working_dir {
        let mut current = Some(dir);
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

pub use ui::inline_image_ui::ImageExpandLevel;
pub use ui::{
    PinnedDiagramLiveDebugSnapshot, PinnedDiagramProbeRect, SidePanelDebugStats,
    SidePanelMermaidProbe, SidePanelMermaidProbeRect, debug_probe_pinned_diagram,
    debug_probe_side_panel_mermaid,
};

pub fn display_messages_from_session(session: &crate::session::Session) -> Vec<DisplayMessage> {
    let mut messages = jcode_tui_messages::display_messages_from_rendered_messages(
        crate::session::render_messages(session),
    );
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

pub fn side_panel_debug_json() -> Option<serde_json::Value> {
    ui::side_panel_debug_json()
}

pub fn pinned_diagram_debug_json() -> Option<serde_json::Value> {
    ui::pinned_diagram_debug_json()
}

pub(crate) fn clear_side_panel_debug_snapshot() {
    ui::clear_side_panel_debug_snapshot();
}

pub fn reset_side_panel_debug_stats() {
    ui::reset_side_panel_debug_stats();
}

pub fn reset_pinned_diagram_debug_snapshot() {
    ui::reset_pinned_diagram_debug_snapshot();
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
        CacheTtlInfo, KvCacheProblemKind, connection_type_icon, detect_kv_cache_problem,
        keyboard_enhancement_flags, resolve_subscribe_metadata, scheduled_notification_text,
    };
    use crate::ambient::AmbientStatus;
    use crate::tui::info_widget::AmbientWidgetData;
    use crossterm::event::KeyboardEnhancementFlags;

    fn warm_cache_ttl() -> CacheTtlInfo {
        CacheTtlInfo {
            remaining_secs: 240,
            ttl_secs: 300,
            is_cold: false,
            cold_for_secs: 0,
            cached_tokens: Some(12_000),
        }
    }

    fn cold_cache_ttl() -> CacheTtlInfo {
        CacheTtlInfo {
            remaining_secs: 0,
            ttl_secs: 300,
            is_cold: true,
            cold_for_secs: 90,
            cached_tokens: Some(12_000),
        }
    }

    #[test]
    fn subscribe_metadata_prefers_remote_working_dir_override() {
        let local_dir = std::path::Path::new("/client/project");
        let (working_dir, selfdev) =
            resolve_subscribe_metadata(Some(local_dir), Some("/server/project"), false);

        assert_eq!(working_dir.as_deref(), Some("/server/project"));
        assert_eq!(selfdev, None);
    }

    #[test]
    fn subscribe_metadata_uses_client_cwd_without_override() {
        let local_dir = std::path::Path::new("/client/project");
        let (working_dir, _selfdev) = resolve_subscribe_metadata(Some(local_dir), None, false);

        assert_eq!(working_dir.as_deref(), Some("/client/project"));
    }

    #[test]
    fn format_compact_age_is_glanceable() {
        use super::format_compact_age;
        assert_eq!(format_compact_age(0), "0s");
        assert_eq!(format_compact_age(45), "45s");
        assert_eq!(format_compact_age(60), "1m");
        assert_eq!(format_compact_age(3_660), "1h 1m");
        assert_eq!(format_compact_age(7_200), "2h");
        assert_eq!(format_compact_age(90_000), "1d 1h");
        assert_eq!(format_compact_age(172_800), "2d");
    }

    #[test]
    fn anthropic_cache_creation_on_turn_two_is_warmup_not_problem() {
        let ttl = warm_cache_ttl();
        assert_eq!(
            detect_kv_cache_problem(
                "anthropic",
                None,
                2,
                12_000,
                Some(0),
                Some(12_000),
                Some(&ttl)
            ),
            None
        );
    }

    #[test]
    fn anthropic_cache_creation_without_read_on_warm_later_turn_is_problem() {
        let ttl = warm_cache_ttl();
        let problem = detect_kv_cache_problem(
            "anthropic",
            None,
            3,
            12_000,
            Some(0),
            Some(12_000),
            Some(&ttl),
        )
        .expect("expected explicit cache creation without read to warn");
        assert_eq!(problem.kind, KvCacheProblemKind::UnexpectedCacheCreation);
        assert_eq!(problem.affected_tokens, Some(12_000));
    }

    #[test]
    fn cache_read_suppresses_cache_creation_warning() {
        let ttl = warm_cache_ttl();
        assert_eq!(
            detect_kv_cache_problem(
                "anthropic",
                None,
                3,
                12_000,
                Some(8_000),
                Some(4_000),
                Some(&ttl)
            ),
            None
        );
    }

    #[test]
    fn cold_cache_suppresses_cache_warning() {
        let ttl = cold_cache_ttl();
        assert_eq!(
            detect_kv_cache_problem(
                "anthropic",
                None,
                3,
                12_000,
                Some(0),
                Some(12_000),
                Some(&ttl)
            ),
            None
        );
    }

    #[test]
    fn openai_explicit_zero_cache_read_on_warm_cacheable_turn_is_problem() {
        let ttl = warm_cache_ttl();
        let problem = detect_kv_cache_problem("openai", None, 3, 8_000, Some(0), None, Some(&ttl))
            .expect("expected explicit zero cached tokens to warn");
        assert_eq!(problem.kind, KvCacheProblemKind::ExpectedCacheReadMissing);
        assert_eq!(problem.affected_tokens, Some(8_000));
    }

    #[test]
    fn missing_cache_read_metric_is_not_a_warning() {
        let ttl = warm_cache_ttl();
        assert_eq!(
            detect_kv_cache_problem("openai", None, 3, 8_000, None, None, Some(&ttl)),
            None
        );
    }

    #[test]
    fn read_only_warning_requires_cacheable_input_size() {
        let ttl = warm_cache_ttl();
        assert_eq!(
            detect_kv_cache_problem("openai", None, 3, 800, Some(0), None, Some(&ttl)),
            None
        );
    }

    #[test]
    fn openrouter_zero_cache_read_requires_known_cache_capable_upstream() {
        let ttl = warm_cache_ttl();
        assert_eq!(
            detect_kv_cache_problem("openrouter", None, 3, 8_000, Some(0), None, Some(&ttl)),
            None
        );

        let problem = detect_kv_cache_problem(
            "openrouter",
            Some("OpenAI"),
            3,
            8_000,
            Some(0),
            None,
            Some(&ttl),
        )
        .expect("known OpenAI upstream should make explicit zero read actionable");
        assert_eq!(problem.kind, KvCacheProblemKind::ExpectedCacheReadMissing);
    }

    #[test]
    fn unsupported_provider_zero_cache_read_does_not_warn_even_if_metric_present() {
        let ttl = warm_cache_ttl();
        assert_eq!(
            detect_kv_cache_problem("copilot", None, 3, 8_000, Some(0), None, Some(&ttl)),
            None
        );
    }

    #[test]
    fn gemini_zero_cache_read_uses_conservative_minimum() {
        let ttl = warm_cache_ttl();
        assert_eq!(
            detect_kv_cache_problem("gemini", None, 3, 3_000, Some(0), None, Some(&ttl)),
            None
        );

        let problem = detect_kv_cache_problem("gemini", None, 3, 5_000, Some(0), None, Some(&ttl))
            .expect("large Gemini prompt with explicit zero cached content should warn");
        assert_eq!(problem.kind, KvCacheProblemKind::ExpectedCacheReadMissing);
    }

    #[test]
    fn connection_type_icon_uses_protocol_specific_icons() {
        assert_eq!(connection_type_icon(Some("websocket")), Some("🔌"));
        assert_eq!(connection_type_icon(Some("wss")), Some("🔌"));
        assert_eq!(connection_type_icon(Some("https")), Some("🌐"));
        assert_eq!(connection_type_icon(Some("https/sse")), Some("🌐"));
        assert_eq!(connection_type_icon(Some("http")), Some("🌐"));
        assert_eq!(connection_type_icon(Some("unknown")), None);
        assert_eq!(connection_type_icon(None), None);
    }

    #[test]
    fn connection_type_icons_avoid_vs16_sequences() {
        // macOS window/tab title fonts ignore the VS16 emoji-presentation
        // selector, so title icons must be single emoji-default codepoints.
        for connection in ["websocket", "wss", "https", "https/sse", "http"] {
            let icon = connection_type_icon(Some(connection)).unwrap();
            assert_eq!(
                icon.chars().count(),
                1,
                "connection icon for '{connection}' must be a single codepoint, got {icon:?}"
            );
            assert!(
                !icon.contains('\u{FE0F}'),
                "connection icon for '{connection}' must not need VS16, got {icon:?}"
            );
        }
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
