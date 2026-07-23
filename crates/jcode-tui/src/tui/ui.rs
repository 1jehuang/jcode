#![cfg_attr(
    test,
    expect(
        clippy::items_after_test_module,
        clippy::let_and_return,
        clippy::missing_const_for_thread_local,
        clippy::needless_borrow,
        clippy::needless_return,
        clippy::too_many_arguments
    )
)]

use super::info_widget;
use super::markdown;
use super::ui_diff::{
    DiffLineKind, ParsedDiffLine, collect_diff_lines, diff_add_color, diff_change_counts_for_tool,
    diff_del_color, generate_diff_lines_from_tool_input, tint_span_with_diff_color,
};
use super::visual_debug::{
    self, FrameCaptureBuilder, ImageRegionCapture, InfoWidgetCapture, MarginsCapture,
    MessageCapture, RenderTimingCapture,
};
use super::{DisplayMessage, DisplayMessageRoleExt, ProcessingStatus, TuiState};
use crate::message::ToolCall;
use ratatui::{prelude::*, widgets::Paragraph};
use serde::Serialize;
#[cfg(test)]
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
#[cfg(not(test))]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
#[cfg(test)]
use unicode_width::UnicodeWidthStr;

#[path = "ui_animations.rs"]
mod animations;
#[path = "ui_box.rs"]
mod box_utils;
#[path = "ui_changelog.rs"]
mod changelog;
#[path = "ui_debug_capture.rs"]
mod debug_capture;
#[path = "ui_diagram_pane.rs"]
mod diagram_pane;
#[path = "ui_file_diff.rs"]
mod file_diff_ui;
#[path = "ui_frame_metrics.rs"]
mod frame_metrics;
#[path = "ui_header.rs"]
mod header;
#[path = "ui_inline_image.rs"]
pub(crate) mod inline_image_ui;
#[path = "ui_inline_interactive.rs"]
mod inline_interactive_ui;
#[path = "ui_inline.rs"]
mod inline_ui;
#[path = "ui_input.rs"]
pub(crate) mod input_ui;
#[path = "ui_memory_estimates.rs"]
mod memory_estimates;
#[path = "ui_memory.rs"]
mod memory_ui;
#[path = "ui_messages.rs"]
mod messages;
#[path = "ui_onboarding.rs"]
mod onboarding;
#[path = "ui_overlays.rs"]
mod overlays;
#[path = "ui_pinned.rs"]
mod pinned_ui;
#[path = "ui_prepare.rs"]
mod prepare;
#[path = "ui_smoothness.rs"]
mod smoothness;
#[path = "ui_todo_changes.rs"]
mod todo_changes;
#[path = "ui_tools.rs"]
pub(crate) mod tools_ui;
#[path = "ui_transitions.rs"]
mod transitions;
#[path = "ui_viewport.rs"]
mod viewport;

use crate::tui::mermaid;
#[cfg(test)]
pub(crate) use box_utils::truncate_line_to_width;
use box_utils::{
    line_plain_text, render_rounded_box, truncate_line_preserving_suffix_to_width,
    truncate_line_with_ellipsis_to_width,
};
use changelog::get_grouped_changelog;
#[cfg(test)]
use changelog::{ChangelogEntry, group_changelog_entries, parse_changelog_from};
use debug_capture::{
    build_info_widget_summary, capture_widget_placements, rect_within_bounds, rects_overlap,
    widget_overlaps_content,
};
pub use diagram_pane::{
    PinnedDiagramLiveDebugSnapshot, PinnedDiagramProbeRect, debug_probe_pinned_diagram,
};
#[cfg(test)]
use diagram_pane::{
    debug_probe_pinned_diagram_with_font, div_ceil_u32,
    estimate_pinned_diagram_pane_width_with_font, is_diagram_poor_fit,
    vcenter_fitted_image_with_font,
};
use diagram_pane::{
    draw_pinned_diagram, estimate_pinned_diagram_pane_height, estimate_pinned_diagram_pane_width,
    pinned_diagram_preferred_aspect_ratio,
};
pub(crate) use diagram_pane::{pinned_diagram_debug_json, reset_pinned_diagram_debug_snapshot};
use file_diff_ui::active_file_diff_context;
use file_diff_ui::draw_file_diff_view;
#[cfg(test)]
use file_diff_ui::{
    FileDiffCacheKey, FileDiffViewCacheEntry, file_content_signature, file_diff_cache,
};
pub(crate) use header::capitalize;
use inline_ui::{draw_inline_ui, inline_ui_height};
pub(crate) use memory_estimates::{debug_memory_profile, debug_side_panel_memory_profile};
use memory_estimates::{estimate_prepared_chat_frame_bytes, estimate_prepared_messages_bytes};
#[cfg(test)]
use memory_ui::{
    MemoryTileItem, choose_memory_tile_span, parse_memory_display_entries, plan_memory_tile,
};
use memory_ui::{group_into_tiles, render_memory_tiles, split_by_display_width};
use messages::get_cached_message_lines;
#[cfg_attr(test, allow(unused_imports))]
pub(crate) use messages::{
    SWARM_AGENT_SNAPSHOT_TITLE, compact_swarm_await_summary, encode_swarm_agent_snapshot,
    render_assistant_message, render_background_task_message, render_reasoning_message,
    render_swarm_message, render_system_message, render_tool_message, render_usage_message,
};
pub use pinned_ui::{
    SidePanelDebugStats, SidePanelMermaidProbe, SidePanelMermaidProbeRect,
    debug_probe_side_panel_mermaid,
};
pub(crate) use pinned_ui::{
    clear_side_panel_debug_snapshot, clear_side_panel_render_caches, prewarm_focused_side_panel,
    reset_side_panel_debug_stats, side_panel_debug_json, side_panel_debug_stats,
};
use pinned_ui::{
    collect_pinned_diffs_cached, draw_pinned_content_cached, draw_side_panel_markdown,
};
#[cfg(test)]
use transitions::extract_line_text;
#[cfg(test)]
use transitions::inline_ui_gap_height;
#[cfg(test)]
use viewport::compute_visible_margins;
use viewport::draw_messages;
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use viewport::{
    copy_badge_reserved_width, expand_badge_reserved_width, pick_copy_badge_line,
    reserve_copy_badge_margins, truncate_line_for_copy_badge,
    truncate_line_in_place_to_width as truncate_copy_badge_line_to_width,
};
/// Last known max scroll value from the renderer. Updated each frame.
/// Scroll handlers use this to clamp scroll_offset and prevent overshoot.
#[cfg(not(test))]
static LAST_MAX_SCROLL: AtomicUsize = AtomicUsize::new(0);
/// Whether the chat viewport used a native scrollbar in the most recent frame.
///
/// Initialized to `1` (assume visible) so the very first frame of a freshly
/// resumed/loaded session prepares the narrow (scrollbar-reserved) width FIRST.
/// Because narrow wraps at least as much as wide, an overflowing transcript is
/// detected on that single narrow build and kept, avoiding a wasted wide build
/// (~seconds on a long transcript) that would otherwise be discarded. Short
/// transcripts that fit still fall through to a (cheap) second wide build, and
/// the real decision is written back every frame, so steady state is unaffected.
#[cfg(not(test))]
static LAST_CHAT_SCROLLBAR_VISIBLE: AtomicUsize = AtomicUsize::new(1);
/// Total line count in the pinned diff/content pane (set during render).
#[cfg(not(test))]
static PINNED_PANE_TOTAL_LINES: AtomicUsize = AtomicUsize::new(0);
/// Effective scroll position of the side pane after render-time clamping.
#[cfg(not(test))]
static LAST_DIFF_PANE_EFFECTIVE_SCROLL: AtomicUsize = AtomicUsize::new(0);
/// Maximum scroll offset of the side pane on the most recent render frame.
#[cfg(not(test))]
static LAST_DIFF_PANE_MAX_SCROLL: AtomicUsize = AtomicUsize::new(0);
/// Total wrapped line count of the chat transcript on the most recent frame.
/// Used together with `LAST_RESOLVED_CHAT_SCROLL` to anchor the viewport when
/// older compacted history is loaded in (so the content under the reader stays
/// put instead of teleporting to the new absolute top).
#[cfg(not(test))]
static LAST_TOTAL_WRAPPED_LINES: AtomicUsize = AtomicUsize::new(0);
/// The chat scroll offset the renderer actually used on the most recent frame
/// (after clamping and after resolving any pending history anchor). Scroll
/// handlers adopt this so manual scrolling resumes from the on-screen position.
#[cfg(not(test))]
static LAST_RESOLVED_CHAT_SCROLL: AtomicUsize = AtomicUsize::new(0);
/// Whether the tail-follow viewport is mid catch-up slide (a large content
/// append is being scrolled into view over several frames instead of jumping).
/// Drives the redraw loop so the slide completes promptly.
#[cfg(not(test))]
static TAIL_CATCHUP_ACTIVE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
/// Set by explicit user actions that resume bottom-follow (typing, End,
/// submitting a prompt). The next renderer pass consumes this request and snaps
/// directly to the tail instead of mistaking the large offset change for a
/// newly-appended content block that should use catch-up animation.
#[cfg(not(test))]
static TAIL_FOLLOW_SNAP_PENDING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
/// Wrapped line indices where each user prompt starts (updated each render frame).
/// Used by prompt-jump keybindings (Ctrl+5..9, Ctrl+[/]) for accurate positioning.
#[cfg(not(test))]
static LAST_USER_PROMPT_POSITIONS: OnceLock<Mutex<Vec<usize>>> = OnceLock::new();

#[cfg(test)]
thread_local! {
    static TEST_LAST_MAX_SCROLL: Cell<usize> = const { Cell::new(0) };
    static TEST_LAST_CHAT_SCROLLBAR_VISIBLE: Cell<bool> = const { Cell::new(false) };
    static TEST_PINNED_PANE_TOTAL_LINES: Cell<usize> = const { Cell::new(0) };
    static TEST_LAST_DIFF_PANE_EFFECTIVE_SCROLL: Cell<usize> = const { Cell::new(0) };
    static TEST_LAST_DIFF_PANE_MAX_SCROLL: Cell<usize> = const { Cell::new(0) };
    static TEST_LAST_TOTAL_WRAPPED_LINES: Cell<usize> = const { Cell::new(0) };
    static TEST_LAST_RESOLVED_CHAT_SCROLL: Cell<usize> = const { Cell::new(0) };
    static TEST_TAIL_CATCHUP_ACTIVE: Cell<bool> = const { Cell::new(false) };
    static TEST_TAIL_FOLLOW_SNAP_PENDING: Cell<bool> = const { Cell::new(false) };
    static TEST_LAST_USER_PROMPT_POSITIONS: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    static TEST_LAST_LAYOUT: RefCell<Option<LayoutSnapshot>> = const { RefCell::new(None) };
    static TEST_LAST_STATUS_AREA: RefCell<Option<Rect>> = const { RefCell::new(None) };
    static TEST_VISIBLE_COPY_TARGETS: RefCell<Vec<VisibleCopyTarget>> = RefCell::new(Vec::new());
    static TEST_VISIBLE_EXPAND_EDIT_BADGE: Cell<bool> = const { Cell::new(false) };
    static TEST_VISIBLE_EXPAND_EDIT_BADGE_LINE: Cell<Option<usize>> = const { Cell::new(None) };
    static TEST_PROMPT_VIEWPORT_STATE: RefCell<PromptViewportState> = RefCell::new(PromptViewportState::default());
    static TEST_COPY_VIEWPORT: RefCell<CopyViewportSnapshots> = RefCell::new(CopyViewportSnapshots::default());
}

/// Get the last known max scroll value (from the most recent render frame).
/// Returns 0 if no frame has been rendered yet.
pub fn last_max_scroll() -> usize {
    #[cfg(test)]
    {
        return TEST_LAST_MAX_SCROLL.with(Cell::get);
    }
    #[cfg(not(test))]
    {
        LAST_MAX_SCROLL.load(Ordering::Relaxed)
    }
}

fn set_last_chat_scrollbar_visible(visible: bool) {
    #[cfg(test)]
    {
        TEST_LAST_CHAT_SCROLLBAR_VISIBLE.with(|state| state.set(visible));
        return;
    }
    #[cfg(not(test))]
    {
        LAST_CHAT_SCROLLBAR_VISIBLE.store(usize::from(visible), Ordering::Relaxed);
    }
}

/// Whether the chat native scrollbar was visible on the most recent render frame.
/// Used as hysteresis so steady-state frames only prepare a single chat width
/// instead of both the wide and narrow variants (which thrashes the prep caches
/// on long transcripts during streaming).
fn last_chat_scrollbar_visible() -> bool {
    #[cfg(test)]
    {
        return TEST_LAST_CHAT_SCROLLBAR_VISIBLE.with(Cell::get);
    }
    #[cfg(not(test))]
    {
        LAST_CHAT_SCROLLBAR_VISIBLE.load(Ordering::Relaxed) != 0
    }
}

/// Get the total line count from the pinned diff/content pane (set during render).
pub fn pinned_pane_total_lines() -> usize {
    #[cfg(test)]
    {
        return TEST_PINNED_PANE_TOTAL_LINES.with(Cell::get);
    }
    #[cfg(not(test))]
    {
        PINNED_PANE_TOTAL_LINES.load(Ordering::Relaxed)
    }
}

pub fn last_diff_pane_effective_scroll() -> usize {
    #[cfg(test)]
    {
        return TEST_LAST_DIFF_PANE_EFFECTIVE_SCROLL.with(Cell::get);
    }
    #[cfg(not(test))]
    {
        LAST_DIFF_PANE_EFFECTIVE_SCROLL.load(Ordering::Relaxed)
    }
}

/// Maximum scroll offset of the side pane on the most recent render frame
/// (total content lines minus the visible viewport height). Scroll handlers
/// clamp against this so stored offsets cannot accumulate invisible
/// "phantom" overscroll past the bottom of the content.
pub fn last_diff_pane_max_scroll() -> usize {
    #[cfg(test)]
    {
        return TEST_LAST_DIFF_PANE_MAX_SCROLL.with(Cell::get);
    }
    #[cfg(not(test))]
    {
        LAST_DIFF_PANE_MAX_SCROLL.load(Ordering::Relaxed)
    }
}

/// Get the last known user prompt line positions (from the most recent render frame).
/// Returns positions as wrapped line indices from the top of content.
pub fn last_user_prompt_positions() -> Vec<usize> {
    #[cfg(test)]
    {
        return TEST_LAST_USER_PROMPT_POSITIONS.with(|v| v.borrow().clone());
    }
    #[cfg(not(test))]
    {
        LAST_USER_PROMPT_POSITIONS
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .map(|v| v.clone())
            .unwrap_or_default()
    }
}

fn update_user_prompt_positions(positions: &[usize]) {
    #[cfg(test)]
    {
        TEST_LAST_USER_PROMPT_POSITIONS.with(|v| {
            let mut v = v.borrow_mut();
            v.clear();
            v.extend_from_slice(positions);
        });
        return;
    }
    #[cfg(not(test))]
    {
        let mutex = LAST_USER_PROMPT_POSITIONS.get_or_init(|| Mutex::new(Vec::new()));
        if let Ok(mut v) = mutex.lock() {
            v.clear();
            v.extend_from_slice(positions);
        }
    }
}

pub(crate) fn set_last_max_scroll(value: usize) {
    #[cfg(test)]
    {
        TEST_LAST_MAX_SCROLL.with(|cell| cell.set(value));
        return;
    }
    #[cfg(not(test))]
    {
        LAST_MAX_SCROLL.store(value, Ordering::Relaxed);
    }
}

pub(crate) fn set_pinned_pane_total_lines(value: usize) {
    #[cfg(test)]
    {
        TEST_PINNED_PANE_TOTAL_LINES.with(|cell| cell.set(value));
        return;
    }
    #[cfg(not(test))]
    {
        PINNED_PANE_TOTAL_LINES.store(value, Ordering::Relaxed);
    }
}

pub(crate) fn set_last_diff_pane_effective_scroll(value: usize) {
    #[cfg(test)]
    {
        TEST_LAST_DIFF_PANE_EFFECTIVE_SCROLL.with(|cell| cell.set(value));
        return;
    }
    #[cfg(not(test))]
    {
        LAST_DIFF_PANE_EFFECTIVE_SCROLL.store(value, Ordering::Relaxed);
    }
}

pub(crate) fn set_last_diff_pane_max_scroll(value: usize) {
    #[cfg(test)]
    {
        TEST_LAST_DIFF_PANE_MAX_SCROLL.with(|cell| cell.set(value));
        return;
    }
    #[cfg(not(test))]
    {
        LAST_DIFF_PANE_MAX_SCROLL.store(value, Ordering::Relaxed);
    }
}

/// Total wrapped line count of the chat transcript on the most recent frame.
/// Returns 0 if no frame has been rendered yet.
pub fn last_total_wrapped_lines() -> usize {
    #[cfg(test)]
    {
        return TEST_LAST_TOTAL_WRAPPED_LINES.with(Cell::get);
    }
    #[cfg(not(test))]
    {
        LAST_TOTAL_WRAPPED_LINES.load(Ordering::Relaxed)
    }
}

pub(crate) fn set_last_total_wrapped_lines(value: usize) {
    #[cfg(test)]
    {
        TEST_LAST_TOTAL_WRAPPED_LINES.with(|cell| cell.set(value));
        return;
    }
    #[cfg(not(test))]
    {
        LAST_TOTAL_WRAPPED_LINES.store(value, Ordering::Relaxed);
    }
}

/// The chat scroll offset the renderer actually used on the most recent frame
/// (after clamping and after resolving any pending history anchor).
pub fn last_resolved_chat_scroll() -> usize {
    #[cfg(test)]
    {
        return TEST_LAST_RESOLVED_CHAT_SCROLL.with(Cell::get);
    }
    #[cfg(not(test))]
    {
        LAST_RESOLVED_CHAT_SCROLL.load(Ordering::Relaxed)
    }
}

pub(crate) fn set_last_resolved_chat_scroll(value: usize) {
    #[cfg(test)]
    {
        TEST_LAST_RESOLVED_CHAT_SCROLL.with(|cell| cell.set(value));
        return;
    }
    #[cfg(not(test))]
    {
        LAST_RESOLVED_CHAT_SCROLL.store(value, Ordering::Relaxed);
    }
}

/// Whether the tail-follow viewport is still sliding toward the bottom after a
/// large append. The redraw loop keeps animation cadence while this is set.
pub(crate) fn tail_catchup_active() -> bool {
    #[cfg(test)]
    {
        return TEST_TAIL_CATCHUP_ACTIVE.with(Cell::get);
    }
    #[cfg(not(test))]
    {
        TAIL_CATCHUP_ACTIVE.load(Ordering::Relaxed)
    }
}

pub(crate) fn set_tail_catchup_active(active: bool) {
    #[cfg(test)]
    {
        TEST_TAIL_CATCHUP_ACTIVE.with(|cell| cell.set(active));
        return;
    }
    #[cfg(not(test))]
    {
        TAIL_CATCHUP_ACTIVE.store(active, Ordering::Relaxed);
    }
}

/// Request that the next tail-follow render land at the exact bottom.
///
/// This is reserved for explicit navigation or composer actions. Automatic
/// transcript growth does not set it, so large committed blocks still use the
/// bounded catch-up animation.
pub(crate) fn request_tail_follow_snap() {
    #[cfg(test)]
    {
        TEST_TAIL_FOLLOW_SNAP_PENDING.with(|cell| cell.set(true));
        return;
    }
    #[cfg(not(test))]
    {
        TAIL_FOLLOW_SNAP_PENDING.store(true, Ordering::Relaxed);
    }
}

pub(crate) fn take_tail_follow_snap_request() -> bool {
    #[cfg(test)]
    {
        return TEST_TAIL_FOLLOW_SNAP_PENDING.with(|cell| cell.replace(false));
    }
    #[cfg(not(test))]
    {
        TAIL_FOLLOW_SNAP_PENDING.swap(false, Ordering::Relaxed)
    }
}

pub(super) fn hash_text_for_cache(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    std::hash::Hasher::finish(&hasher)
}

#[path = "ui_layout.rs"]
mod layout_support;
#[path = "ui_status.rs"]
mod status_support;
#[path = "ui_theme.rs"]
mod theme_support;
use super::color_support::rgb;
pub(crate) use layout_support::align_if_unset;
use layout_support::{
    centered_content_block_width, clear_area, draw_right_rail_chrome, left_aligned_content_inset,
    left_pad_lines_to_block_width, right_rail_border_style,
};
#[cfg(test)]
pub(crate) use status_support::calculate_input_lines;
use status_support::{
    binary_age, format_status_for_debug, is_running_stable_release, semver, shorten_model_name,
};
use theme_support::{
    accent_color, activity_indicator, activity_indicator_frame_index, ai_color, ai_text,
    animated_tool_color, asap_color, blend_color, dim_color, file_link_color, header_icon_color,
    header_name_color, header_session_color, pending_color, prompt_entry_bg_color,
    prompt_entry_color, prompt_entry_shimmer_color, queued_color, rainbow_prompt_color,
    system_message_color, tool_color, user_bg, user_color, user_text,
};

pub(crate) use jcode_tui_markdown::{CopyTargetKind, RawCopyTarget};
pub(crate) use jcode_tui_messages::{
    CopyTarget, EditToolRange, ImageRegion, MessageBoundary, PreparedChatFrame, PreparedMessages,
    PreparedSection, PreparedSectionKind, WrappedLineMap,
};

#[derive(Clone, Debug)]
struct ActiveFileDiffContext {
    edit_index: usize,
    msg_index: usize,
    file_path: String,
    start_line: usize,
    end_line: usize,
    expandable: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct VisibleCopyTarget {
    pub key: char,
    pub kind_label: String,
    pub copied_notice: String,
    pub content: String,
}

// Copy badges intentionally avoid h/j/k/l so they never shadow vi-style
// movement keys while the user is scanning visible actions.
const COPY_BADGE_KEYS: [char; 12] = ['s', 'd', 'f', 'g', 'w', 'e', 'r', 't', 'x', 'c', 'v', 'b'];

#[cfg(not(test))]
static VISIBLE_COPY_TARGETS: OnceLock<Mutex<Vec<VisibleCopyTarget>>> = OnceLock::new();

#[cfg(not(test))]
static VISIBLE_EXPAND_EDIT_BADGE: OnceLock<Mutex<bool>> = OnceLock::new();

#[cfg(not(test))]
static VISIBLE_EXPAND_EDIT_BADGE_LINE: OnceLock<Mutex<Option<usize>>> = OnceLock::new();

#[cfg(not(test))]
fn visible_copy_targets_state() -> &'static Mutex<Vec<VisibleCopyTarget>> {
    VISIBLE_COPY_TARGETS.get_or_init(|| Mutex::new(Vec::new()))
}

#[cfg(not(test))]
fn visible_expand_edit_badge_state() -> &'static Mutex<bool> {
    VISIBLE_EXPAND_EDIT_BADGE.get_or_init(|| Mutex::new(false))
}

#[cfg(not(test))]
fn visible_expand_edit_badge_line_state() -> &'static Mutex<Option<usize>> {
    VISIBLE_EXPAND_EDIT_BADGE_LINE.get_or_init(|| Mutex::new(None))
}

pub(crate) fn set_visible_expand_edit_badge(visible: bool, line: Option<usize>) {
    #[cfg(test)]
    {
        TEST_VISIBLE_EXPAND_EDIT_BADGE.with(|state| state.set(visible));
        TEST_VISIBLE_EXPAND_EDIT_BADGE_LINE.with(|state| state.set(line));
        return;
    }
    #[cfg(not(test))]
    {
        let mut visible_state = match visible_expand_edit_badge_state().lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *visible_state = visible;

        let mut line_state = match visible_expand_edit_badge_line_state().lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *line_state = line;
    }
}

pub(crate) fn visible_expand_edit_badge() -> bool {
    #[cfg(test)]
    {
        return TEST_VISIBLE_EXPAND_EDIT_BADGE.with(Cell::get);
    }
    #[cfg(not(test))]
    {
        let state = match visible_expand_edit_badge_state().lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *state
    }
}

pub(crate) fn visible_expand_edit_badge_line() -> Option<usize> {
    #[cfg(test)]
    {
        return TEST_VISIBLE_EXPAND_EDIT_BADGE_LINE.with(Cell::get);
    }
    #[cfg(not(test))]
    {
        let state = match visible_expand_edit_badge_line_state().lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *state
    }
}

fn set_visible_copy_targets(targets: Vec<VisibleCopyTarget>) {
    #[cfg(test)]
    {
        TEST_VISIBLE_COPY_TARGETS.with(|state| {
            *state.borrow_mut() = targets;
        });
        return;
    }
    #[cfg(not(test))]
    {
        let mut state = match visible_copy_targets_state().lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *state = targets;
    }
}

pub(crate) fn visible_copy_target_for_key(key: char) -> Option<VisibleCopyTarget> {
    #[cfg(test)]
    {
        TEST_VISIBLE_COPY_TARGETS.with(|state| {
            state
                .borrow()
                .iter()
                .find(|target| target.key.eq_ignore_ascii_case(&key))
                .cloned()
        })
    }
    #[cfg(not(test))]
    {
        let state = match visible_copy_targets_state().lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        state
            .iter()
            .find(|target| target.key.eq_ignore_ascii_case(&key))
            .cloned()
    }
}

#[derive(Clone, Copy)]
struct PromptViewportAnimation {
    line_idx: usize,
    start_ms: u64,
}

#[derive(Clone, Copy, Default)]
struct PromptViewportState {
    initialized: bool,
    last_visible_start: usize,
    last_visible_end: usize,
    active: Option<PromptViewportAnimation>,
}

const PROMPT_ENTRY_ANIMATION_MS: u64 = 450;

#[cfg(not(test))]
static PROMPT_VIEWPORT_STATE: OnceLock<Mutex<PromptViewportState>> = OnceLock::new();

#[cfg(not(test))]
fn prompt_viewport_state() -> &'static Mutex<PromptViewportState> {
    PROMPT_VIEWPORT_STATE.get_or_init(|| Mutex::new(PromptViewportState::default()))
}

fn active_prompt_entry_animation(now_ms: u64) -> Option<PromptViewportAnimation> {
    #[cfg(test)]
    {
        TEST_PROMPT_VIEWPORT_STATE.with(|state| {
            let mut state = state.borrow_mut();
            if let Some(anim) = state.active {
                if now_ms.saturating_sub(anim.start_ms) <= PROMPT_ENTRY_ANIMATION_MS {
                    return Some(anim);
                }
                state.active = None;
            }
            None
        })
    }
    #[cfg(not(test))]
    {
        let mut state = match prompt_viewport_state().lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        if let Some(anim) = state.active {
            if now_ms.saturating_sub(anim.start_ms) <= PROMPT_ENTRY_ANIMATION_MS {
                return Some(anim);
            }
            state.active = None;
        }
        None
    }
}

fn record_prompt_viewport(visible_start: usize, visible_end: usize) {
    #[cfg(test)]
    {
        TEST_PROMPT_VIEWPORT_STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.initialized = true;
            state.last_visible_start = visible_start;
            state.last_visible_end = visible_end;
            state.active = None;
        });
        return;
    }
    #[cfg(not(test))]
    {
        let mut state = match prompt_viewport_state().lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.initialized = true;
        state.last_visible_start = visible_start;
        state.last_visible_end = visible_end;
        state.active = None;
    }
}

fn update_prompt_entry_animation(
    user_prompt_lines: &[usize],
    visible_start: usize,
    visible_end: usize,
    now_ms: u64,
) {
    #[cfg(test)]
    {
        TEST_PROMPT_VIEWPORT_STATE.with(|state| {
            let mut state = state.borrow_mut();

            if !state.initialized {
                state.initialized = true;
                state.last_visible_start = visible_start;
                state.last_visible_end = visible_end;
                return;
            }

            let prev_visible_start = state.last_visible_start;
            let prev_visible_end = state.last_visible_end;
            let viewport_changed =
                prev_visible_start != visible_start || prev_visible_end != visible_end;

            if let Some(anim) = state.active {
                let still_fresh = now_ms.saturating_sub(anim.start_ms) <= PROMPT_ENTRY_ANIMATION_MS;
                let still_visible = anim.line_idx >= visible_start && anim.line_idx < visible_end;
                if still_fresh && still_visible {
                    state.last_visible_start = visible_start;
                    state.last_visible_end = visible_end;
                    return;
                }
                if !still_fresh || !still_visible {
                    state.active = None;
                }
            }

            if viewport_changed && state.active.is_none() {
                let newly_visible = user_prompt_lines.iter().copied().find(|line| {
                    *line >= visible_start
                        && *line < visible_end
                        && (*line < prev_visible_start || *line >= prev_visible_end)
                });
                if let Some(line_idx) = newly_visible {
                    state.active = Some(PromptViewportAnimation {
                        line_idx,
                        start_ms: now_ms,
                    });
                }
            }

            state.last_visible_start = visible_start;
            state.last_visible_end = visible_end;
        });
        return;
    }
    #[cfg(not(test))]
    {
        let mut state = match prompt_viewport_state().lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        if !state.initialized {
            state.initialized = true;
            state.last_visible_start = visible_start;
            state.last_visible_end = visible_end;
            return;
        }

        let prev_visible_start = state.last_visible_start;
        let prev_visible_end = state.last_visible_end;
        let viewport_changed =
            prev_visible_start != visible_start || prev_visible_end != visible_end;

        if let Some(anim) = state.active {
            let still_fresh = now_ms.saturating_sub(anim.start_ms) <= PROMPT_ENTRY_ANIMATION_MS;
            let still_visible = anim.line_idx >= visible_start && anim.line_idx < visible_end;
            if still_fresh && still_visible {
                state.last_visible_start = visible_start;
                state.last_visible_end = visible_end;
                return;
            }
            if !still_fresh || !still_visible {
                state.active = None;
            }
        }

        if viewport_changed && state.active.is_none() {
            let newly_visible = user_prompt_lines.iter().copied().find(|line| {
                *line >= visible_start
                    && *line < visible_end
                    && (*line < prev_visible_start || *line >= prev_visible_end)
            });
            if let Some(line_idx) = newly_visible {
                state.active = Some(PromptViewportAnimation {
                    line_idx,
                    start_ms: now_ms,
                });
            }
        }

        state.last_visible_start = visible_start;
        state.last_visible_end = visible_end;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BodyCacheKey {
    width: u16,
    diff_mode: crate::config::DiffDisplayMode,
    messages_version: u64,
    diagram_mode: crate::config::DiagramDisplayMode,
    centered: bool,
    /// Mermaid render geometry depends on the scoped transcript/pane aspect
    /// profile as well as width. A vertical terminal resize can change this
    /// bucket without changing `width`, so it must invalidate the prepared body.
    mermaid_aspect_bucket: Option<u16>,
    /// Whether inline images render at all (Alt+M hides them).
    pin_images: bool,
    /// Whether inline images render expanded or as collapsed label stubs
    /// (Alt+Shift+I toggles; persisted).
    inline_images_visible: bool,
    /// Signature of the inline image set; anchored images render inside the
    /// body, so the body must rebuild when images arrive or change.
    images_signature: (usize, u64),
    /// Monotonic per-image expand-level version. Anchored images embed their
    /// expand-level geometry into the body, so a level change must rebuild the
    /// body exactly like an image-set change does.
    expanded_images_version: u64,
    /// Live swarm-member data renders beneath the tool call that spawned each
    /// member, so status/todo/tool-intent updates must invalidate the body.
    swarm_members_signature: u64,
}

#[derive(Clone)]
struct BodyCacheEntry {
    key: BodyCacheKey,
    prepared: Arc<PreparedMessages>,
    prepared_bytes: usize,
    msg_count: usize,
    /// `compacted_hidden_user_prompts` at build time. Prepend (suffix) reuse
    /// renders absolute prompt numbers into the reused lines, so a rebuild that
    /// stitches older history above a cached suffix must verify numbering
    /// continuity against the offset the base was built with.
    prompt_offset: usize,
}

const BODY_CACHE_MAX_ENTRIES: usize = 8;
// Keep enough room for a single large transcript snapshot so long sessions do not
// fall off a hard per-entry cache cliff and get rebuilt every frame.
const BODY_CACHE_MAX_BYTES: usize = 32 * 1024 * 1024;
const BODY_OVERSIZED_CACHE_MAX_ENTRIES: usize = 2;

#[derive(Default)]
struct BodyCacheState {
    entries: VecDeque<BodyCacheEntry>,
    oversized_entries: VecDeque<BodyCacheEntry>,
}

impl BodyCacheState {
    fn total_bytes(&self) -> usize {
        self.entries.iter().map(|entry| entry.prepared_bytes).sum()
    }

    fn get_exact_with_kind(
        &mut self,
        key: &BodyCacheKey,
    ) -> Option<(Arc<PreparedMessages>, CacheEntryKind)> {
        if let Some(pos) = self.entries.iter().position(|entry| &entry.key == key) {
            let entry = self.entries.remove(pos)?;
            let prepared = entry.prepared.clone();
            self.entries.push_front(entry);
            Some((prepared, CacheEntryKind::Regular))
        } else {
            let pos = self
                .oversized_entries
                .iter()
                .position(|entry| &entry.key == key)?;
            let entry = self.oversized_entries.remove(pos)?;
            let prepared = entry.prepared.clone();
            self.oversized_entries.push_front(entry);
            Some((prepared, CacheEntryKind::Oversized))
        }
    }

    #[cfg(test)]
    fn get_exact(&mut self, key: &BodyCacheKey) -> Option<Arc<PreparedMessages>> {
        self.get_exact_with_kind(key).map(|(prepared, _)| prepared)
    }

    #[cfg(test)]
    fn best_incremental_base(
        &self,
        key: &BodyCacheKey,
        _msg_count: usize,
    ) -> Option<(Arc<PreparedMessages>, usize)> {
        let regular = self
            .entries
            .iter()
            .filter(|entry| {
                entry.msg_count > 0
                    && entry.key.width == key.width
                    && entry.key.diff_mode == key.diff_mode
                    && entry.key.diagram_mode == key.diagram_mode
                    && entry.key.centered == key.centered
                    && entry.key.mermaid_aspect_bucket == key.mermaid_aspect_bucket
                    // Anchored inline images render inside the body, and a
                    // late-arriving image may target an already-prepared
                    // message; only reuse bases built with the same image set.
                    && entry.key.pin_images == key.pin_images
                    && entry.key.inline_images_visible == key.inline_images_visible
                    && entry.key.images_signature == key.images_signature
                    && entry.key.expanded_images_version == key.expanded_images_version
                    && entry.key.swarm_members_signature == key.swarm_members_signature
            })
            .max_by_key(|entry| entry.msg_count)
            .map(|entry| (entry.prepared.clone(), entry.msg_count));
        let oversized = self
            .oversized_entries
            .iter()
            .filter(|entry| {
                entry.msg_count > 0
                    && entry.key.width == key.width
                    && entry.key.diff_mode == key.diff_mode
                    && entry.key.diagram_mode == key.diagram_mode
                    && entry.key.centered == key.centered
                    && entry.key.mermaid_aspect_bucket == key.mermaid_aspect_bucket
                    // Anchored inline images render inside the body, and a
                    // late-arriving image may target an already-prepared
                    // message; only reuse bases built with the same image set.
                    && entry.key.pin_images == key.pin_images
                    && entry.key.inline_images_visible == key.inline_images_visible
                    && entry.key.images_signature == key.images_signature
                    && entry.key.expanded_images_version == key.expanded_images_version
                    && entry.key.swarm_members_signature == key.swarm_members_signature
            })
            .max_by_key(|entry| entry.msg_count)
            .map(|entry| (entry.prepared.clone(), entry.msg_count));

        match (regular, oversized) {
            (Some(left), Some(right)) => {
                if left.1 >= right.1 {
                    Some(left)
                } else {
                    Some(right)
                }
            }
            (Some(entry), None) | (None, Some(entry)) => Some(entry),
            (None, None) => None,
        }
    }

    fn take_best_incremental_base(
        &mut self,
        key: &BodyCacheKey,
    ) -> Option<(Arc<PreparedMessages>, usize, usize)> {
        let regular = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry.msg_count > 0
                    && entry.key.width == key.width
                    && entry.key.diff_mode == key.diff_mode
                    && entry.key.diagram_mode == key.diagram_mode
                    && entry.key.centered == key.centered
                    && entry.key.mermaid_aspect_bucket == key.mermaid_aspect_bucket
                    // Anchored inline images render inside the body, and a
                    // late-arriving image may target an already-prepared
                    // message; only reuse bases built with the same image set.
                    && entry.key.pin_images == key.pin_images
                    && entry.key.inline_images_visible == key.inline_images_visible
                    && entry.key.images_signature == key.images_signature
                    && entry.key.expanded_images_version == key.expanded_images_version
                    && entry.key.swarm_members_signature == key.swarm_members_signature
            })
            .max_by_key(|(_, entry)| entry.msg_count)
            .map(|(idx, entry)| (false, idx, entry.msg_count));
        let oversized = self
            .oversized_entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry.msg_count > 0
                    && entry.key.width == key.width
                    && entry.key.diff_mode == key.diff_mode
                    && entry.key.diagram_mode == key.diagram_mode
                    && entry.key.centered == key.centered
                    && entry.key.mermaid_aspect_bucket == key.mermaid_aspect_bucket
                    // Anchored inline images render inside the body, and a
                    // late-arriving image may target an already-prepared
                    // message; only reuse bases built with the same image set.
                    && entry.key.pin_images == key.pin_images
                    && entry.key.inline_images_visible == key.inline_images_visible
                    && entry.key.images_signature == key.images_signature
                    && entry.key.expanded_images_version == key.expanded_images_version
                    && entry.key.swarm_members_signature == key.swarm_members_signature
            })
            .max_by_key(|(_, entry)| entry.msg_count)
            .map(|(idx, entry)| (true, idx, entry.msg_count));

        let chosen = match (regular, oversized) {
            (Some(left), Some(right)) => {
                if left.2 >= right.2 {
                    left
                } else {
                    right
                }
            }
            (Some(entry), None) | (None, Some(entry)) => entry,
            (None, None) => return None,
        };

        let (is_oversized, idx, msg_count) = chosen;
        let entry = if is_oversized {
            self.oversized_entries.remove(idx)?
        } else {
            self.entries.remove(idx)?
        };
        Some((entry.prepared, msg_count, entry.prompt_offset))
    }

    fn insert(
        &mut self,
        key: BodyCacheKey,
        prepared: Arc<PreparedMessages>,
        msg_count: usize,
        prompt_offset: usize,
    ) {
        let prepared_bytes = estimate_prepared_messages_bytes(&prepared);
        if prepared_bytes > BODY_CACHE_MAX_BYTES {
            if let Some(pos) = self
                .oversized_entries
                .iter()
                .position(|entry| entry.key == key)
            {
                self.oversized_entries.remove(pos);
            }
            self.oversized_entries.push_front(BodyCacheEntry {
                key,
                prepared,
                prepared_bytes,
                msg_count,
                prompt_offset,
            });
            while self.oversized_entries.len() > BODY_OVERSIZED_CACHE_MAX_ENTRIES {
                self.oversized_entries.pop_back();
            }
            return;
        }
        if let Some(pos) = self
            .oversized_entries
            .iter()
            .position(|entry| entry.key == key)
        {
            self.oversized_entries.remove(pos);
        }
        if let Some(pos) = self.entries.iter().position(|entry| entry.key == key) {
            self.entries.remove(pos);
        }
        self.entries.push_front(BodyCacheEntry {
            key,
            prepared,
            prepared_bytes,
            msg_count,
            prompt_offset,
        });
        while self.entries.len() > BODY_CACHE_MAX_ENTRIES
            || self.total_bytes() > BODY_CACHE_MAX_BYTES
        {
            self.entries.pop_back();
        }
    }
}

static BODY_CACHE: OnceLock<Mutex<BodyCacheState>> = OnceLock::new();

fn body_cache() -> &'static Mutex<BodyCacheState> {
    BODY_CACHE.get_or_init(|| Mutex::new(BodyCacheState::default()))
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FullPrepCacheKey {
    width: u16,
    height: u16,
    diff_mode: crate::config::DiffDisplayMode,
    messages_version: u64,
    diagram_mode: crate::config::DiagramDisplayMode,
    centered: bool,
    /// The scoped Mermaid profile can also change when pane geometry changes
    /// while the transcript rectangle stays the same.
    mermaid_aspect_bucket: Option<u16>,
    is_processing: bool,
    streaming_text_len: usize,
    streaming_text_hash: u64,
    batch_progress_hash: u64,
    inline_images_signature: (usize, u64),
    /// Whether inline images render expanded or as collapsed label stubs.
    inline_images_visible: bool,
    /// Per-image expand-level version; anchored image geometry is embedded in
    /// the prepared frame, so a level change must invalidate it.
    expanded_images_version: u64,
    /// Signature of live swarm member cards embedded beneath spawn tool calls.
    swarm_members_signature: u64,
}

#[derive(Clone)]
struct FullPrepCacheEntry {
    key: FullPrepCacheKey,
    prepared: Arc<PreparedChatFrame>,
    prepared_bytes: usize,
}

const FULL_PREP_CACHE_MAX_ENTRIES: usize = 4;
// Full prepared frames duplicate some body data, so give them enough headroom to
// retain the active large transcript instead of forcing full recomposition.
const FULL_PREP_CACHE_MAX_BYTES: usize = 24 * 1024 * 1024;
const FULL_PREP_OVERSIZED_CACHE_MAX_ENTRIES: usize = 2;

#[derive(Default)]
struct FullPrepCacheState {
    entries: VecDeque<FullPrepCacheEntry>,
    oversized_entries: VecDeque<FullPrepCacheEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
enum CacheEntryKind {
    Regular,
    Oversized,
}

impl FullPrepCacheState {
    fn total_bytes(&self) -> usize {
        self.entries.iter().map(|entry| entry.prepared_bytes).sum()
    }

    fn get_exact_with_kind(
        &mut self,
        key: &FullPrepCacheKey,
    ) -> Option<(Arc<PreparedChatFrame>, CacheEntryKind)> {
        if let Some(pos) = self.entries.iter().position(|entry| &entry.key == key) {
            let entry = self.entries.remove(pos)?;
            let prepared = entry.prepared.clone();
            self.entries.push_front(entry);
            Some((prepared, CacheEntryKind::Regular))
        } else {
            let pos = self
                .oversized_entries
                .iter()
                .position(|entry| &entry.key == key)?;
            let entry = self.oversized_entries.remove(pos)?;
            let prepared = entry.prepared.clone();
            self.oversized_entries.push_front(entry);
            Some((prepared, CacheEntryKind::Oversized))
        }
    }

    #[cfg(test)]
    fn get_exact(&mut self, key: &FullPrepCacheKey) -> Option<Arc<PreparedChatFrame>> {
        self.get_exact_with_kind(key).map(|(prepared, _)| prepared)
    }

    fn insert(&mut self, key: FullPrepCacheKey, prepared: Arc<PreparedChatFrame>) {
        let prepared_bytes = estimate_prepared_chat_frame_bytes(&prepared);
        if prepared_bytes > FULL_PREP_CACHE_MAX_BYTES {
            if let Some(pos) = self
                .oversized_entries
                .iter()
                .position(|entry| entry.key == key)
            {
                self.oversized_entries.remove(pos);
            }
            self.oversized_entries.push_front(FullPrepCacheEntry {
                key,
                prepared,
                prepared_bytes,
            });
            while self.oversized_entries.len() > FULL_PREP_OVERSIZED_CACHE_MAX_ENTRIES {
                self.oversized_entries.pop_back();
            }
            return;
        }
        if let Some(pos) = self
            .oversized_entries
            .iter()
            .position(|entry| entry.key == key)
        {
            self.oversized_entries.remove(pos);
        }
        if let Some(pos) = self.entries.iter().position(|entry| entry.key == key) {
            self.entries.remove(pos);
        }
        self.entries.push_front(FullPrepCacheEntry {
            key,
            prepared,
            prepared_bytes,
        });
        while self.entries.len() > FULL_PREP_CACHE_MAX_ENTRIES
            || self.total_bytes() > FULL_PREP_CACHE_MAX_BYTES
        {
            self.entries.pop_back();
        }
    }
}

static FULL_PREP_CACHE: OnceLock<Mutex<FullPrepCacheState>> = OnceLock::new();

fn full_prep_cache() -> &'static Mutex<FullPrepCacheState> {
    FULL_PREP_CACHE.get_or_init(|| Mutex::new(FullPrepCacheState::default()))
}

#[cfg(not(test))]
static LAST_STATUS_AREA: OnceLock<Mutex<Option<Rect>>> = OnceLock::new();

#[cfg(not(test))]
fn last_status_area_state() -> &'static Mutex<Option<Rect>> {
    LAST_STATUS_AREA.get_or_init(|| Mutex::new(None))
}

pub(crate) fn record_status_area(area: Rect) {
    #[cfg(test)]
    {
        TEST_LAST_STATUS_AREA.with(|snapshot| {
            *snapshot.borrow_mut() = Some(area);
        });
        return;
    }
    #[cfg(not(test))]
    {
        if let Ok(mut snapshot) = last_status_area_state().lock() {
            *snapshot = Some(area);
        }
    }
}

pub(crate) fn last_status_area() -> Option<Rect> {
    #[cfg(test)]
    {
        return TEST_LAST_STATUS_AREA.with(|snapshot| *snapshot.borrow());
    }
    #[cfg(not(test))]
    {
        last_status_area_state()
            .lock()
            .ok()
            .and_then(|snapshot| *snapshot)
    }
}

use frame_metrics::{
    ChatLayoutMetrics, FLICKER_NOTICE_COPY_KEY, FullPrepPhaseMetrics, ViewportMetrics,
    begin_frame_resource_sample, finalize_frame_metrics, note_body_built, note_body_cache_hit,
    note_body_cache_lookup, note_body_cache_miss, note_body_incremental_reuse, note_body_request,
    note_chat_layout, note_full_prep_built, note_full_prep_cache_hit, note_full_prep_cache_lookup,
    note_full_prep_cache_miss, note_full_prep_phase_metrics, note_full_prep_request,
    note_viewport_metrics, reset_frame_perf_stats, viewport_stability_hash,
};
pub(crate) use frame_metrics::{
    DrawCallAttribution, FrameInputAttribution, frame_input_attribution_snapshot,
    record_draw_call_attribution, set_frame_input_attribution, wall_clock_ms,
};
pub(crate) use frame_metrics::{
    debug_draw_call_history, debug_flicker_frame_history, debug_slow_frame_history,
    recent_flicker_copy_target_for_key, recent_flicker_ui_notice,
};
#[cfg(test)]
pub(crate) use smoothness::frame_from_buffer as smoothness_frame_from_buffer;
pub(crate) use smoothness::{report_json as smoothness_report_json, reset as smoothness_reset};

#[cfg(test)]
pub(crate) use frame_metrics::{
    FlickerFrameSample, FramePerfStats, SlowFrameSample, clear_flicker_frame_history_for_tests,
    clear_slow_frame_history_for_tests, record_flicker_frame_sample, record_slow_frame_sample,
};

#[derive(Clone, Copy, Debug)]
pub struct LayoutSnapshot {
    pub messages_area: Rect,
    pub diagram_area: Option<Rect>,
    pub diff_pane_area: Option<Rect>,
    pub input_area: Option<Rect>,
}

#[cfg(not(test))]
static LAST_LAYOUT: OnceLock<Mutex<Option<LayoutSnapshot>>> = OnceLock::new();

#[cfg(not(test))]
fn last_layout_state() -> &'static Mutex<Option<LayoutSnapshot>> {
    LAST_LAYOUT.get_or_init(|| Mutex::new(None))
}

pub fn record_layout_snapshot(
    messages_area: Rect,
    diagram_area: Option<Rect>,
    diff_pane_area: Option<Rect>,
    input_area: Option<Rect>,
) {
    #[cfg(test)]
    {
        TEST_LAST_LAYOUT.with(|snapshot| {
            *snapshot.borrow_mut() = Some(LayoutSnapshot {
                messages_area,
                diagram_area,
                diff_pane_area,
                input_area,
            });
        });
        return;
    }
    #[cfg(not(test))]
    {
        if let Ok(mut snapshot) = last_layout_state().lock() {
            *snapshot = Some(LayoutSnapshot {
                messages_area,
                diagram_area,
                diff_pane_area,
                input_area,
            });
        }
    }
}

pub fn last_layout_snapshot() -> Option<LayoutSnapshot> {
    #[cfg(test)]
    {
        return TEST_LAST_LAYOUT.with(|snapshot| *snapshot.borrow());
    }
    #[cfg(not(test))]
    {
        last_layout_state()
            .lock()
            .ok()
            .and_then(|snapshot| *snapshot)
    }
}

#[cfg(test)]
pub(crate) fn clear_test_render_state_for_tests() {
    set_last_max_scroll(0);
    set_pinned_pane_total_lines(0);
    set_last_diff_pane_effective_scroll(0);
    set_last_diff_pane_max_scroll(0);
    set_last_total_wrapped_lines(0);
    set_last_resolved_chat_scroll(0);
    TEST_TAIL_FOLLOW_SNAP_PENDING.with(|cell| cell.set(false));
    update_user_prompt_positions(&[]);
    // Flicker events recorded by sibling tests add a "⚠ flicker detected"
    // notification line to subsequent renders, shifting every layout-sensitive
    // assertion (click mapping, snapshot rows).
    frame_metrics::clear_flicker_frame_history_for_tests();
    TEST_LAST_LAYOUT.with(|snapshot| {
        *snapshot.borrow_mut() = None;
    });
    TEST_LAST_STATUS_AREA.with(|snapshot| {
        *snapshot.borrow_mut() = None;
    });
    set_visible_copy_targets(Vec::new());
    clear_copy_viewport_snapshot();

    TEST_PROMPT_VIEWPORT_STATE.with(|state| {
        *state.borrow_mut() = PromptViewportState::default();
    });
}

/// Test-only: render just the onboarding welcome screen into `area`, using the
/// exact same code path the live UI uses. Lets onboarding golden/snapshot tests
/// capture the rendered copy without reaching into the private `onboarding`
/// submodule.
#[cfg(test)]
pub(crate) fn draw_onboarding_welcome_for_tests(
    frame: &mut ratatui::Frame,
    app: &dyn crate::tui::TuiState,
    area: ratatui::layout::Rect,
) {
    onboarding::draw_onboarding_welcome(frame, app, area);
}

#[derive(Clone)]
enum CopyViewportData {
    Dense {
        wrapped_plain_lines: Arc<Vec<String>>,
        wrapped_copy_offsets: Arc<Vec<usize>>,
        raw_plain_lines: Arc<Vec<String>>,
        wrapped_line_map: Arc<Vec<WrappedLineMap>>,
    },
    ChatFrame {
        prepared: Arc<PreparedChatFrame>,
    },
}

#[derive(Clone)]
struct CopyViewportSnapshot {
    pane: crate::tui::CopySelectionPane,
    data: CopyViewportData,
    scroll: usize,
    visible_end: usize,
    content_area: Rect,
    left_margins: Vec<u16>,
}

impl CopyViewportSnapshot {
    fn wrapped_plain_line_count(&self) -> usize {
        match &self.data {
            CopyViewportData::Dense {
                wrapped_plain_lines,
                ..
            } => wrapped_plain_lines.len(),
            CopyViewportData::ChatFrame { prepared } => prepared.wrapped_plain_line_count(),
        }
    }

    fn wrapped_plain_line(&self, abs_line: usize) -> Option<&str> {
        match &self.data {
            CopyViewportData::Dense {
                wrapped_plain_lines,
                ..
            } => wrapped_plain_lines.get(abs_line).map(String::as_str),
            CopyViewportData::ChatFrame { prepared } => prepared.wrapped_plain_line(abs_line),
        }
    }

    fn wrapped_copy_offset(&self, abs_line: usize) -> Option<usize> {
        match &self.data {
            CopyViewportData::Dense {
                wrapped_copy_offsets,
                ..
            } => wrapped_copy_offsets.get(abs_line).copied(),
            CopyViewportData::ChatFrame { prepared } => prepared.wrapped_copy_offset(abs_line),
        }
    }

    fn raw_plain_line(&self, raw_line: usize) -> Option<&str> {
        match &self.data {
            CopyViewportData::Dense {
                raw_plain_lines, ..
            } => raw_plain_lines.get(raw_line).map(String::as_str),
            CopyViewportData::ChatFrame { prepared } => prepared.raw_plain_line(raw_line),
        }
    }

    fn raw_plain_line_count(&self) -> usize {
        match &self.data {
            CopyViewportData::Dense {
                raw_plain_lines, ..
            } => raw_plain_lines.len(),
            CopyViewportData::ChatFrame { prepared } => prepared.total_raw_lines,
        }
    }

    fn wrapped_line_map(&self, abs_line: usize) -> Option<WrappedLineMap> {
        match &self.data {
            CopyViewportData::Dense {
                wrapped_line_map, ..
            } => wrapped_line_map.get(abs_line).copied(),
            CopyViewportData::ChatFrame { prepared } => prepared.wrapped_line_map(abs_line),
        }
    }

    /// If `abs_line` is the label line of a visible inline-image region, return
    /// that image's id. The label line sits exactly one wrapped line above the
    /// region's first placeholder line (see `anchored_image_lines`), so we map a
    /// click on the label row back to the image it annotates.
    fn inline_image_id_for_label_line(&self, abs_line: usize) -> Option<u64> {
        let prepared = match &self.data {
            CopyViewportData::ChatFrame { prepared } => prepared,
            CopyViewportData::Dense { .. } => return None,
        };
        prepared
            .image_regions
            .iter()
            .find(|region| {
                region.render == jcode_tui_messages::ImageRegionRender::Fit
                    && region.abs_line_idx == abs_line + 1
            })
            .map(|region| region.hash)
    }
}

#[derive(Clone, Default)]
struct CopyViewportSnapshots {
    chat: Option<CopyViewportSnapshot>,
    side: Option<CopyViewportSnapshot>,
    input: Option<CopyViewportSnapshot>,
}

#[cfg(not(test))]
static LAST_COPY_VIEWPORT: OnceLock<Mutex<CopyViewportSnapshots>> = OnceLock::new();
#[path = "ui/copy_selection.rs"]
mod copy_selection;
#[path = "ui/display_width.rs"]
mod display_width;
#[path = "ui/draw_recovery.rs"]
mod draw_recovery;
#[path = "ui/profile.rs"]
mod profile;
#[path = "ui/selection_highlight.rs"]
pub(crate) mod selection_highlight;
#[path = "ui/url.rs"]
mod url_regex_support;
use self::copy_selection::{
    copy_point_from_snapshot, copy_selection_text_from_raw_lines, link_target_from_snapshot,
};
use self::display_width::{clamp_display_col, display_col_slice, line_display_width};
use self::draw_recovery::render_recovered_panic_frame;
use self::profile::{profile_enabled, record_profile};

#[cfg(not(test))]
fn copy_viewport_state() -> &'static Mutex<CopyViewportSnapshots> {
    LAST_COPY_VIEWPORT.get_or_init(|| Mutex::new(CopyViewportSnapshots::default()))
}

fn copy_snapshot_slot_mut(
    snapshots: &mut CopyViewportSnapshots,
    pane: crate::tui::CopySelectionPane,
) -> &mut Option<CopyViewportSnapshot> {
    match pane {
        crate::tui::CopySelectionPane::Chat => &mut snapshots.chat,
        crate::tui::CopySelectionPane::SidePane => &mut snapshots.side,
        crate::tui::CopySelectionPane::Input => &mut snapshots.input,
    }
}

fn copy_snapshot_for_pane(pane: crate::tui::CopySelectionPane) -> Option<CopyViewportSnapshot> {
    #[cfg(test)]
    {
        TEST_COPY_VIEWPORT.with(|snapshots| {
            let snapshots = snapshots.borrow().clone();
            match pane {
                crate::tui::CopySelectionPane::Chat => snapshots.chat,
                crate::tui::CopySelectionPane::SidePane => snapshots.side,
                crate::tui::CopySelectionPane::Input => snapshots.input,
            }
        })
    }
    #[cfg(not(test))]
    {
        let snapshots = copy_viewport_state().lock().ok()?.clone();
        match pane {
            crate::tui::CopySelectionPane::Chat => snapshots.chat,
            crate::tui::CopySelectionPane::SidePane => snapshots.side,
            crate::tui::CopySelectionPane::Input => snapshots.input,
        }
    }
}

pub(crate) fn clear_copy_viewport_snapshot() {
    #[cfg(test)]
    {
        TEST_COPY_VIEWPORT.with(|state| {
            *state.borrow_mut() = CopyViewportSnapshots::default();
        });
        return;
    }
    #[cfg(not(test))]
    if let Ok(mut state) = copy_viewport_state().lock() {
        *state = CopyViewportSnapshots::default();
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Viewport snapshot helpers carry explicit render state to avoid hidden globals in call sites"
)]
fn record_copy_pane_snapshot(
    pane: crate::tui::CopySelectionPane,
    wrapped_plain_lines: Arc<Vec<String>>,
    wrapped_copy_offsets: Arc<Vec<usize>>,
    raw_plain_lines: Arc<Vec<String>>,
    wrapped_line_map: Arc<Vec<WrappedLineMap>>,
    scroll: usize,
    visible_end: usize,
    content_area: Rect,
    left_margins: &[u16],
) {
    #[cfg(test)]
    {
        TEST_COPY_VIEWPORT.with(|state| {
            *copy_snapshot_slot_mut(&mut state.borrow_mut(), pane) = Some(CopyViewportSnapshot {
                pane,
                data: CopyViewportData::Dense {
                    wrapped_plain_lines,
                    wrapped_copy_offsets,
                    raw_plain_lines,
                    wrapped_line_map,
                },
                scroll,
                visible_end,
                content_area,
                left_margins: left_margins.to_vec(),
            });
        });
        return;
    }
    #[cfg(not(test))]
    if let Ok(mut state) = copy_viewport_state().lock() {
        *copy_snapshot_slot_mut(&mut state, pane) = Some(CopyViewportSnapshot {
            pane,
            data: CopyViewportData::Dense {
                wrapped_plain_lines,
                wrapped_copy_offsets,
                raw_plain_lines,
                wrapped_line_map,
            },
            scroll,
            visible_end,
            content_area,
            left_margins: left_margins.to_vec(),
        });
    }
}

fn record_copy_viewport_frame_snapshot(
    prepared: Arc<PreparedChatFrame>,
    scroll: usize,
    visible_end: usize,
    content_area: Rect,
    left_margins: &[u16],
) {
    #[cfg(test)]
    {
        TEST_COPY_VIEWPORT.with(|state| {
            *copy_snapshot_slot_mut(&mut state.borrow_mut(), crate::tui::CopySelectionPane::Chat) =
                Some(CopyViewportSnapshot {
                    pane: crate::tui::CopySelectionPane::Chat,
                    data: CopyViewportData::ChatFrame { prepared },
                    scroll,
                    visible_end,
                    content_area,
                    left_margins: left_margins.to_vec(),
                });
        });
        return;
    }
    #[cfg(not(test))]
    if let Ok(mut state) = copy_viewport_state().lock() {
        *copy_snapshot_slot_mut(&mut state, crate::tui::CopySelectionPane::Chat) =
            Some(CopyViewportSnapshot {
                pane: crate::tui::CopySelectionPane::Chat,
                data: CopyViewportData::ChatFrame { prepared },
                scroll,
                visible_end,
                content_area,
                left_margins: left_margins.to_vec(),
            });
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Viewport snapshot helpers carry explicit render state to avoid hidden globals in call sites"
)]
pub(crate) fn record_side_pane_snapshot_precomputed(
    wrapped_plain_lines: Arc<Vec<String>>,
    wrapped_copy_offsets: Arc<Vec<usize>>,
    raw_plain_lines: Arc<Vec<String>>,
    wrapped_line_map: Arc<Vec<WrappedLineMap>>,
    scroll: usize,
    visible_end: usize,
    content_area: Rect,
    left_margins: &[u16],
) {
    record_copy_pane_snapshot(
        crate::tui::CopySelectionPane::SidePane,
        wrapped_plain_lines,
        wrapped_copy_offsets,
        raw_plain_lines,
        wrapped_line_map,
        scroll,
        visible_end,
        content_area,
        left_margins,
    );
}

#[expect(
    clippy::too_many_arguments,
    reason = "Viewport snapshot helpers carry explicit render state to avoid hidden globals in call sites"
)]
#[cfg(test)]
pub(crate) fn record_copy_viewport_snapshot(
    wrapped_plain_lines: Arc<Vec<String>>,
    wrapped_copy_offsets: Arc<Vec<usize>>,
    raw_plain_lines: Arc<Vec<String>>,
    wrapped_line_map: Arc<Vec<WrappedLineMap>>,
    scroll: usize,
    visible_end: usize,
    content_area: Rect,
    left_margins: &[u16],
) {
    record_copy_pane_snapshot(
        crate::tui::CopySelectionPane::Chat,
        wrapped_plain_lines,
        wrapped_copy_offsets,
        raw_plain_lines,
        wrapped_line_map,
        scroll,
        visible_end,
        content_area,
        left_margins,
    );
}

/// Record a real `ChatFrame` viewport snapshot for tests. Unlike
/// `record_copy_viewport_snapshot` (which records a `Dense` snapshot that cannot
/// resolve inline-image label lines), this preserves the `PreparedChatFrame` so
/// `inline_image_id_for_label_line` works end to end.
#[cfg(test)]
pub(crate) fn record_copy_viewport_frame_snapshot_for_test(
    prepared: Arc<PreparedChatFrame>,
    scroll: usize,
    visible_end: usize,
    content_area: Rect,
    left_margins: &[u16],
) {
    record_copy_viewport_frame_snapshot(prepared, scroll, visible_end, content_area, left_margins);
}

pub(crate) fn line_left_margins_for_area(lines: &[Line<'static>], area_width: u16) -> Vec<u16> {
    lines
        .iter()
        .map(|line| {
            let used = line.width().min(area_width as usize) as u16;
            let total_margin = area_width.saturating_sub(used);
            match line.alignment.unwrap_or(Alignment::Left) {
                Alignment::Left => 0,
                Alignment::Center => total_margin / 2,
                Alignment::Right => total_margin,
            }
        })
        .collect()
}

pub(crate) fn record_side_pane_snapshot(
    wrapped_lines: &[Line<'static>],
    scroll: usize,
    visible_end: usize,
    content_area: Rect,
) {
    record_pane_snapshot_from_lines(
        crate::tui::CopySelectionPane::SidePane,
        wrapped_lines,
        scroll,
        visible_end,
        content_area,
    );
}

/// Record a copy-selection snapshot for the chat pane from already-wrapped
/// display lines. Used by full-screen overlays (e.g. `/changelog`) that replace
/// the chat viewport but still want drag-to-select-and-copy support. Each
/// display line is treated as a single raw line, so the copied text matches the
/// rendered text verbatim.
pub(crate) fn record_chat_overlay_copy_snapshot(
    wrapped_lines: &[Line<'static>],
    scroll: usize,
    visible_end: usize,
    content_area: Rect,
) {
    record_pane_snapshot_from_lines(
        crate::tui::CopySelectionPane::Chat,
        wrapped_lines,
        scroll,
        visible_end,
        content_area,
    );
}

/// Record a copy-selection snapshot for the prompt composer (input box).
/// Called from `draw_input` each frame with the composer's wrapped rows so a
/// mouse drag over the text being typed selects and copies it, exactly like
/// the chat transcript (issue #430). Raw lines are the logical `\n`-separated
/// input lines, so selections spanning soft wraps copy the original text.
pub(crate) fn record_input_copy_snapshot(
    wrapped_plain_lines: Vec<String>,
    raw_plain_lines: Vec<String>,
    wrapped_line_map: Vec<WrappedLineMap>,
    scroll: usize,
    visible_end: usize,
    content_area: Rect,
    left_margins: &[u16],
) {
    let wrapped_copy_offsets = vec![0usize; wrapped_plain_lines.len()];
    record_copy_pane_snapshot(
        crate::tui::CopySelectionPane::Input,
        Arc::new(wrapped_plain_lines),
        Arc::new(wrapped_copy_offsets),
        Arc::new(raw_plain_lines),
        Arc::new(wrapped_line_map),
        scroll,
        visible_end,
        content_area,
        left_margins,
    );
}

fn record_pane_snapshot_from_lines(
    pane: crate::tui::CopySelectionPane,
    wrapped_lines: &[Line<'static>],
    scroll: usize,
    visible_end: usize,
    content_area: Rect,
) {
    let left_margins = line_left_margins_for_area(wrapped_lines, content_area.width);
    let raw_plain_lines: Vec<String> = wrapped_lines.iter().map(line_plain_text).collect();
    let wrapped_line_map: Vec<WrappedLineMap> = raw_plain_lines
        .iter()
        .enumerate()
        .map(|(raw_line, text)| WrappedLineMap {
            raw_line,
            start_col: 0,
            end_col: line_display_width(text),
        })
        .collect();
    let visible_left_margins = left_margins
        .get(scroll..visible_end.min(left_margins.len()))
        .unwrap_or(&[]);
    record_copy_pane_snapshot(
        pane,
        Arc::new(raw_plain_lines.clone()),
        Arc::new(vec![0; wrapped_lines.len()]),
        Arc::new(raw_plain_lines),
        Arc::new(wrapped_line_map),
        scroll,
        visible_end,
        content_area,
        visible_left_margins,
    );
}

pub(crate) fn copy_point_from_screen(
    column: u16,
    row: u16,
) -> Option<crate::tui::CopySelectionPoint> {
    #[cfg(test)]
    {
        TEST_COPY_VIEWPORT.with(|snapshots| {
            let snapshots = snapshots.borrow().clone();
            snapshots
                .chat
                .as_ref()
                .and_then(|snapshot| copy_point_from_snapshot(snapshot, column, row))
                .or_else(|| {
                    snapshots
                        .side
                        .as_ref()
                        .and_then(|snapshot| copy_point_from_snapshot(snapshot, column, row))
                })
                .or_else(|| {
                    snapshots
                        .input
                        .as_ref()
                        .and_then(|snapshot| copy_point_from_snapshot(snapshot, column, row))
                })
        })
    }
    #[cfg(not(test))]
    {
        let snapshots = copy_viewport_state().lock().ok()?.clone();
        snapshots
            .chat
            .as_ref()
            .and_then(|snapshot| copy_point_from_snapshot(snapshot, column, row))
            .or_else(|| {
                snapshots
                    .side
                    .as_ref()
                    .and_then(|snapshot| copy_point_from_snapshot(snapshot, column, row))
            })
            .or_else(|| {
                snapshots
                    .input
                    .as_ref()
                    .and_then(|snapshot| copy_point_from_snapshot(snapshot, column, row))
            })
    }
}

/// Number of rows at the top/bottom of a pane that act as the browser-style
/// auto-scroll "hot zone". Dragging a selection anywhere inside this band keeps
/// pulling in more transcript, instead of requiring the cursor to land exactly
/// on the boundary row. Scales gently with pane height and is capped so small
/// panes keep a usable middle region.
fn edge_autoscroll_zone_rows(height: u16) -> u16 {
    (height / 4).clamp(1, 3)
}

#[cfg(test)]
mod edge_autoscroll_zone_tests {
    use super::edge_autoscroll_zone_rows;

    #[test]
    fn zone_is_at_least_one_row_for_tiny_panes() {
        // Even a 1-2 row pane should keep a usable hot zone so the edge still triggers.
        assert_eq!(edge_autoscroll_zone_rows(0), 1);
        assert_eq!(edge_autoscroll_zone_rows(1), 1);
        assert_eq!(edge_autoscroll_zone_rows(3), 1);
        assert_eq!(edge_autoscroll_zone_rows(4), 1);
    }

    #[test]
    fn zone_scales_with_height_but_is_capped() {
        assert_eq!(edge_autoscroll_zone_rows(8), 2);
        assert_eq!(edge_autoscroll_zone_rows(12), 3);
        // Capped at 3 so tall panes keep a large neutral middle region.
        assert_eq!(edge_autoscroll_zone_rows(40), 3);
        assert_eq!(edge_autoscroll_zone_rows(200), 3);
    }
}

pub(crate) fn copy_pane_vertical_edge_point(
    pane: crate::tui::CopySelectionPane,
    column: u16,
    row: u16,
) -> Option<(crate::tui::CopySelectionPoint, bool)> {
    // The prompt composer cannot be wheel-scrolled, so it has no browser-style
    // edge auto-scroll. Drags past its edge clamp via `copy_pane_drag_point`.
    if pane == crate::tui::CopySelectionPane::Input {
        return None;
    }
    let snapshot = copy_snapshot_for_pane(pane)?;
    let area = snapshot.content_area;
    if area.width == 0 || area.height == 0 {
        return None;
    }

    // Browser-style edge auto-scroll: terminals clamp the mouse to the visible
    // viewport, so a drag that "leaves" the top/bottom of the pane is reported on
    // the boundary row itself. We additionally treat a small band near each edge
    // as a hot zone, so dragging *near* (not just exactly onto) the top/bottom
    // keeps pulling in more transcript, just like dragging a selection toward the
    // edge of a browser window. The horizontal position is clamped into the pane
    // so the selection extends no matter where along the edge the cursor sits.
    let last_row = area.y.saturating_add(area.height).saturating_sub(1);
    let zone = edge_autoscroll_zone_rows(area.height);
    let top_trigger = area.y.saturating_add(zone);
    let bottom_trigger = last_row.saturating_sub(zone);
    // Only engage the hot zone when there is actually more transcript to pull in
    // that direction. Otherwise dragging into the bottom band while the view is
    // already pinned to the end (the common case) would snap the selection to the
    // last visible line and fight precise highlighting of the bottom rows. When
    // there is nothing to scroll, fall through (`None`) so the caller extends the
    // selection to the exact cell under the cursor instead.
    let can_scroll_up = snapshot.scroll > 0;
    let can_scroll_down = snapshot.visible_end < snapshot.wrapped_plain_line_count();
    let (edge_row, upward) = if row <= top_trigger && can_scroll_up {
        (area.y, true)
    } else if row >= bottom_trigger && can_scroll_down {
        (last_row, false)
    } else {
        return None;
    };

    let clamped_col = column.clamp(area.x, area.x.saturating_add(area.width).saturating_sub(1));

    copy_point_from_snapshot(&snapshot, clamped_col, edge_row).map(|point| (point, upward))
}

/// Resolve the selection point for a drag at `(column, row)`, clamping vertical
/// overshoot to the nearest in-bounds line edge.
///
/// Terminals report a drag that "leaves" the pane on the boundary row, but a
/// drag *into the empty space below the last content line* (common with short
/// transcripts that leave blank rows underneath) lands on a row that maps to no
/// line at all, so `copy_point_from_screen` returns `None`. Native terminal and
/// browser selection treat that as "select through the end of the last line".
/// This mirrors that: dragging below the last visible line snaps to the end of
/// that line, and dragging above the first visible line snaps to its start, so
/// the boundary line is fully covered even when there is nothing more to scroll.
pub(crate) fn copy_pane_drag_point(
    pane: crate::tui::CopySelectionPane,
    column: u16,
    row: u16,
) -> Option<crate::tui::CopySelectionPoint> {
    let snapshot = copy_snapshot_for_pane(pane)?;
    let area = snapshot.content_area;
    if area.width == 0 || area.height == 0 {
        return None;
    }

    // A direct hit on a real line wins: precise per-cell selection.
    if let Some(point) = copy_point_from_snapshot(&snapshot, column, row) {
        return Some(point);
    }

    let line_count = snapshot.wrapped_plain_line_count();
    if line_count == 0 {
        return None;
    }
    let last_line = line_count.saturating_sub(1);
    let last_visible_line = snapshot.visible_end.saturating_sub(1).min(last_line);
    let first_visible_line = snapshot.scroll.min(last_line);

    let last_row = area.y.saturating_add(area.height).saturating_sub(1);
    let clamped_col = column.clamp(area.x, area.x.saturating_add(area.width).saturating_sub(1));

    // Below the visible content: snap to the end of the last visible line.
    if row >= last_row {
        let text = snapshot.wrapped_plain_line(last_visible_line).unwrap_or("");
        return Some(crate::tui::CopySelectionPoint {
            pane,
            abs_line: last_visible_line,
            column: line_display_width(text),
        });
    }

    // Above the visible content: snap to the start of the first visible line.
    if row <= area.y {
        return Some(crate::tui::CopySelectionPoint {
            pane,
            abs_line: first_visible_line,
            column: snapshot
                .wrapped_copy_offset(first_visible_line)
                .unwrap_or(0),
        });
    }

    // Interior row that maps to no line (e.g. a blank gap row between/after
    // content within the visible band): fall back to the boundary-clamped point.
    copy_point_from_snapshot(&snapshot, clamped_col, row.clamp(area.y, last_row)).or(Some(
        crate::tui::CopySelectionPoint {
            pane,
            abs_line: last_visible_line,
            column: line_display_width(
                snapshot.wrapped_plain_line(last_visible_line).unwrap_or(""),
            ),
        },
    ))
}

/// Edge point for tick-driven continuous auto-scroll, where there is no live
/// mouse position. Uses the top/bottom boundary row of the pane and its left
/// content column so the selection keeps extending to the freshly revealed line.
pub(crate) fn copy_pane_autoscroll_edge_point(
    pane: crate::tui::CopySelectionPane,
    upward: bool,
) -> Option<crate::tui::CopySelectionPoint> {
    let snapshot = copy_snapshot_for_pane(pane)?;
    let area = snapshot.content_area;
    if area.width == 0 || area.height == 0 {
        return None;
    }
    let edge_row = if upward {
        area.y
    } else {
        area.y.saturating_add(area.height).saturating_sub(1)
    };
    copy_point_from_snapshot(&snapshot, area.x, edge_row)
}

#[cfg(test)]
pub(crate) fn copy_viewport_point_from_screen(
    column: u16,
    row: u16,
) -> Option<crate::tui::CopySelectionPoint> {
    let point = copy_point_from_screen(column, row)?;
    (point.pane == crate::tui::CopySelectionPane::Chat).then_some(point)
}

#[cfg(test)]
pub(crate) fn side_pane_point_from_screen(
    column: u16,
    row: u16,
) -> Option<crate::tui::CopySelectionPoint> {
    let point = copy_point_from_screen(column, row)?;
    (point.pane == crate::tui::CopySelectionPane::SidePane).then_some(point)
}

fn copy_pane_line_text(pane: crate::tui::CopySelectionPane, abs_line: usize) -> Option<String> {
    copy_snapshot_for_pane(pane)?
        .wrapped_plain_line(abs_line)
        .map(str::to_owned)
}

pub(crate) fn copy_viewport_line_text(abs_line: usize) -> Option<String> {
    copy_pane_line_text(crate::tui::CopySelectionPane::Chat, abs_line)
}

pub(crate) fn side_pane_line_text(abs_line: usize) -> Option<String> {
    copy_pane_line_text(crate::tui::CopySelectionPane::SidePane, abs_line)
}

pub(crate) fn input_pane_line_text(abs_line: usize) -> Option<String> {
    copy_pane_line_text(crate::tui::CopySelectionPane::Input, abs_line)
}

fn copy_pane_line_count(pane: crate::tui::CopySelectionPane) -> Option<usize> {
    Some(copy_snapshot_for_pane(pane)?.wrapped_plain_line_count())
}

pub(crate) fn copy_viewport_line_count() -> Option<usize> {
    copy_pane_line_count(crate::tui::CopySelectionPane::Chat)
}

pub(crate) fn side_pane_line_count() -> Option<usize> {
    copy_pane_line_count(crate::tui::CopySelectionPane::SidePane)
}

pub(crate) fn input_pane_line_count() -> Option<usize> {
    copy_pane_line_count(crate::tui::CopySelectionPane::Input)
}

pub(crate) fn copy_viewport_visible_range() -> Option<(usize, usize)> {
    let snapshot = copy_snapshot_for_pane(crate::tui::CopySelectionPane::Chat)?;
    Some((snapshot.scroll, snapshot.visible_end))
}

#[cfg(test)]
pub(crate) fn side_pane_visible_range() -> Option<(usize, usize)> {
    let snapshot = copy_snapshot_for_pane(crate::tui::CopySelectionPane::SidePane)?;
    Some((snapshot.scroll, snapshot.visible_end))
}

pub(crate) fn copy_pane_first_visible_point(
    pane: crate::tui::CopySelectionPane,
) -> Option<crate::tui::CopySelectionPoint> {
    let snapshot = copy_snapshot_for_pane(pane)?;
    if snapshot.scroll >= snapshot.visible_end
        || snapshot.scroll >= snapshot.wrapped_plain_line_count()
    {
        return None;
    }
    Some(crate::tui::CopySelectionPoint {
        pane,
        abs_line: snapshot.scroll,
        column: 0,
    })
}

pub(crate) fn copy_selection_text(range: crate::tui::CopySelectionRange) -> Option<String> {
    if range.start.pane != range.end.pane {
        return None;
    }
    let snapshot = copy_snapshot_for_pane(range.start.pane)?;
    let (start, end) =
        if (range.start.abs_line, range.start.column) <= (range.end.abs_line, range.end.column) {
            (range.start, range.end)
        } else {
            (range.end, range.start)
        };

    if start.abs_line >= snapshot.wrapped_plain_line_count()
        || end.abs_line >= snapshot.wrapped_plain_line_count()
    {
        return None;
    }

    if let Some(text) = copy_selection_text_from_raw_lines(&snapshot, start, end) {
        return Some(text);
    }

    let selected_lines = end
        .abs_line
        .saturating_sub(start.abs_line)
        .saturating_add(1);
    let mut out = String::new();
    for abs_line in start.abs_line..=end.abs_line {
        if abs_line > start.abs_line {
            out.push('\n');
        }
        let text = snapshot.wrapped_plain_line(abs_line)?;
        if abs_line != start.abs_line && abs_line != end.abs_line {
            let copy_start = snapshot.wrapped_copy_offset(abs_line).unwrap_or(0);
            if copy_start == 0 {
                if abs_line == start.abs_line + 1 {
                    out.reserve(text.len().saturating_mul(selected_lines.min(8)));
                }
                out.push_str(text);
                continue;
            }
        }
        let line_width = line_display_width(text);
        let copy_start = snapshot.wrapped_copy_offset(abs_line).unwrap_or(0);
        let start_col = if abs_line == start.abs_line {
            clamp_display_col(text, start.column).max(copy_start)
        } else {
            copy_start
        };
        let end_col = if abs_line == end.abs_line {
            clamp_display_col(text, end.column).max(copy_start)
        } else {
            line_width
        };

        if end_col < start_col {
            continue;
        }

        let slice = display_col_slice(text, start_col, end_col);
        if abs_line == start.abs_line {
            out.reserve(slice.len().saturating_mul(selected_lines.min(8)));
        }
        out.push_str(slice);
    }

    Some(out)
}

/// Compute `(char_count, line_count)` for the current copy selection without
/// allocating the full joined selection string. Mirrors `copy_selection_text`
/// so the status line "N chars · M lines" matches what would be copied, but is
/// allocation-free so it can run cheaply on every render frame / drag move.
pub(crate) fn copy_selection_metrics(
    range: crate::tui::CopySelectionRange,
) -> Option<(usize, usize)> {
    if range.start.pane != range.end.pane {
        return None;
    }
    let snapshot = copy_snapshot_for_pane(range.start.pane)?;
    let (start, end) =
        if (range.start.abs_line, range.start.column) <= (range.end.abs_line, range.end.column) {
            (range.start, range.end)
        } else {
            (range.end, range.start)
        };

    if start.abs_line >= snapshot.wrapped_plain_line_count()
        || end.abs_line >= snapshot.wrapped_plain_line_count()
    {
        return None;
    }

    if let Some(metrics) =
        copy_selection::copy_selection_metrics_from_raw_lines(&snapshot, start, end)
    {
        return Some(metrics);
    }

    let mut chars = 0usize;
    let mut lines = 0usize;
    for abs_line in start.abs_line..=end.abs_line {
        if abs_line > start.abs_line {
            chars += 1; // joining '\n'
        }
        lines += 1;
        let text = snapshot.wrapped_plain_line(abs_line)?;
        if abs_line != start.abs_line && abs_line != end.abs_line {
            let copy_start = snapshot.wrapped_copy_offset(abs_line).unwrap_or(0);
            if copy_start == 0 {
                chars += text.chars().count();
                continue;
            }
        }
        let line_width = line_display_width(text);
        let copy_start = snapshot.wrapped_copy_offset(abs_line).unwrap_or(0);
        let start_col = if abs_line == start.abs_line {
            clamp_display_col(text, start.column).max(copy_start)
        } else {
            copy_start
        };
        let end_col = if abs_line == end.abs_line {
            clamp_display_col(text, end.column).max(copy_start)
        } else {
            line_width
        };
        if end_col < start_col {
            continue;
        }
        chars += display_col_slice(text, start_col, end_col).chars().count();
    }

    Some((chars, lines.max(1)))
}

pub(crate) fn link_target_from_screen(column: u16, row: u16) -> Option<String> {
    let point = copy_point_from_screen(column, row)?;
    // Clicking a URL you are still composing should reposition the caret, not
    // open the link; only transcript/side-pane links are click-to-open.
    if point.pane == crate::tui::CopySelectionPane::Input {
        return None;
    }
    let snapshot = copy_snapshot_for_pane(point.pane)?;
    link_target_from_snapshot(&snapshot, point)
}

/// If a screen click landed on an inline-image label line, return the image
/// id so the caller can cycle that image's size. The label line is short and
/// single purpose (there is no visible expand badge anymore), so the whole
/// line acts as the click target alongside the image body itself.
pub(crate) fn inline_image_expand_target_from_screen(column: u16, row: u16) -> Option<u64> {
    let point = copy_point_from_screen(column, row)?;
    let snapshot = copy_snapshot_for_pane(point.pane)?;
    snapshot.inline_image_id_for_label_line(point.abs_line)
}

/// If a screen click landed on a collapsed/expanded swarm notification's
/// `▸ expand` / `▾ collapse` badge, return the transcript message index so the
/// caller can toggle that notification. Only clicks on the trailing badge
/// token count, so the tldr text itself stays selectable.
pub(crate) fn swarm_expand_target_from_screen(column: u16, row: u16) -> Option<usize> {
    let point = copy_point_from_screen(column, row)?;
    if point.pane != crate::tui::CopySelectionPane::Chat {
        return None;
    }
    let snapshot = copy_snapshot_for_pane(point.pane)?;
    let prepared = match &snapshot.data {
        CopyViewportData::ChatFrame { prepared } => prepared.clone(),
        CopyViewportData::Dense { .. } => return None,
    };
    let text = snapshot.wrapped_plain_line(point.abs_line)?;
    let trimmed = text.trim_end();
    let badge_start = [
        messages::SWARM_EXPAND_BADGE,
        messages::SWARM_COLLAPSE_BADGE,
        messages::SWARM_DIFF_EXPAND_BADGE,
        messages::SWARM_DIFF_COLLAPSE_BADGE,
    ]
    .iter()
    .find_map(|badge| {
        let prefix = trimmed.strip_suffix(badge)?;
        Some(line_display_width(prefix))
    })?;
    if point.column < badge_start {
        return None;
    }
    prepared.message_index_at_line(point.abs_line)
}

/// If a screen click landed on the rendered body of an inline image (its
/// placeholder rows), return the image id so the caller can cycle that image's
/// size. Together with the label-line hit-test this makes the whole picture
/// clickable.
/// The hit-region is bounded by the image's rendered width (`region.width`,
/// which includes the 2-cell left border), shifted right when `centered` mode
/// horizontally centers the drawn pixels, so clicks in empty space beside a
/// narrow image stay inert.
pub(crate) fn inline_image_body_target_from_screen(
    column: u16,
    row: u16,
    centered: bool,
) -> Option<u64> {
    let point = copy_point_from_screen(column, row)?;
    let snapshot = copy_snapshot_for_pane(point.pane)?;
    let prepared = match &snapshot.data {
        CopyViewportData::ChatFrame { prepared } => prepared,
        CopyViewportData::Dense { .. } => return None,
    };
    let region = prepared.image_regions.iter().find(|region| {
        region.render == jcode_tui_messages::ImageRegionRender::Fit
            && point.abs_line >= region.abs_line_idx
            && point.abs_line < region.end_line
    })?;
    let area = snapshot.content_area;
    let rel_col = column.saturating_sub(area.x);
    // `width == 0` means unknown; treat the rows as fully occupied then.
    let width = if region.width == 0 {
        area.width
    } else {
        region.width.min(area.width)
    };
    // Centered mode draws the border at the left edge but centers the image
    // pixels; accept the full band from the border through the image's right
    // edge so both the border and the picture are clickable.
    let right_edge = if centered {
        let offset = area.width.saturating_sub(width) / 2;
        offset.saturating_add(width)
    } else {
        width
    };
    (rel_col < right_edge).then_some(region.hash)
}

/// Debug dump of the live chat snapshot's inline-image regions plus the screen
/// coordinates of each visible label line (the click target that cycles the
/// image size), so external drivers (debug socket) can compute real click
/// targets against the running TUI.
pub(crate) fn debug_chat_image_regions_json() -> String {
    let Some(snapshot) = copy_snapshot_for_pane(crate::tui::CopySelectionPane::Chat) else {
        return "{\"error\":\"no chat snapshot\"}".to_string();
    };
    let prepared = match &snapshot.data {
        CopyViewportData::ChatFrame { prepared } => prepared.clone(),
        CopyViewportData::Dense { .. } => {
            return "{\"error\":\"dense snapshot (no image regions)\"}".to_string();
        }
    };
    let area = snapshot.content_area;
    let regions: Vec<serde_json::Value> = prepared
        .image_regions
        .iter()
        .map(|region| {
            let label_line = region.abs_line_idx.saturating_sub(1);
            let label_text = snapshot.wrapped_plain_line(label_line).unwrap_or("");
            let label_visible = label_line >= snapshot.scroll && label_line < snapshot.visible_end;
            // The whole label line is clickable now; report its first cell as
            // the badge coordinate so existing drivers keep working.
            let badge_screen = label_visible.then(|| {
                let rel_row = label_line - snapshot.scroll;
                let left_margin = snapshot.left_margins.get(rel_row).copied().unwrap_or(0);
                serde_json::json!({
                    "col": area.x as usize + left_margin as usize,
                    "row": area.y as usize + rel_row,
                })
            });
            serde_json::json!({
                "hash": region.hash,
                "render": format!("{:?}", region.render),
                "abs_line_idx": region.abs_line_idx,
                "end_line": region.end_line,
                "rows": region.height,
                "cols": region.width,
                "label_line": label_line,
                "label_text": label_text,
                "label_visible": label_visible,
                "badge_screen": badge_screen,
            })
        })
        .collect();
    serde_json::to_string_pretty(&serde_json::json!({
        "scroll": snapshot.scroll,
        "visible_end": snapshot.visible_end,
        "content_area": {
            "x": area.x, "y": area.y, "width": area.width, "height": area.height,
        },
        "image_regions": regions,
    }))
    .unwrap_or_else(|_| "{}".to_string())
}

pub fn draw(frame: &mut Frame, app: &dyn TuiState) {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::tui::markdown::with_deferred_mermaid_render_context(|| draw_inner(frame, app))
    })) {
        Ok(()) => {}
        Err(payload) => render_recovered_panic_frame(frame, &payload),
    }
    // Adapt the finished frame for light terminal backgrounds (no-op on dark).
    // Doing this at the buffer level covers every widget and overlay without
    // touching individual color call sites.
    jcode_tui_style::adapt_buffer_for_theme(frame.buffer_mut());
    // Cache eviction/clearing can outlive the last visible image. Carry Kitty
    // deletion commands on any completed frame so terminal-side pixel storage
    // is reclaimed even when no image widget renders again.
    crate::tui::mermaid::render_pending_terminal_image_cleanup(frame.buffer_mut());
}

/// Rows reserved below the input for the decorative idle donut.
///
/// The donut only shows on an (effectively) empty idle screen, which means it
/// is pure negative space. When the composer grows past its resting one-row
/// height (multi-line input, or the `/` command menu adding suggestion rows),
/// take that growth out of the donut's reservation instead of shrinking the
/// transcript above: this keeps the header/tips text and info widgets
/// perfectly still when the slash menu opens on a fresh session. The donut
/// simply renders shorter for as long as the composer is expanded.
fn idle_donut_reserved_height(show_donut: bool, input_height: u16) -> u16 {
    const IDLE_DONUT_HEIGHT: u16 = 14;
    if show_donut {
        let composer_growth = input_height.saturating_sub(1);
        IDLE_DONUT_HEIGHT.saturating_sub(composer_growth)
    } else {
        0
    }
}

fn draw_inner(frame: &mut Frame, app: &dyn TuiState) {
    let area = frame.area().intersection(*frame.buffer_mut().area());
    if area.width == 0 || area.height == 0 {
        return;
    }

    let total_start = Instant::now();
    reset_frame_perf_stats();
    begin_frame_resource_sample();

    clear_copy_viewport_snapshot();

    // Clear full frame to prevent stale cells from prior layouts.
    // This is critical on macOS terminals where ratatui's diff-based updates
    // can leave outdated content when layout dimensions change between frames
    // (e.g., diagram pane toggling, streaming text clearing, tool calls finishing).
    // Uses Color::Reset (terminal default bg) so text selection highlighting works
    // natively in all terminal emulators.
    clear_area(frame, area);

    if let Some(scroll) = app.changelog_scroll() {
        overlays::draw_changelog_overlay(frame, area, scroll, app);
        finalize_frame_metrics(
            app,
            total_start,
            Duration::ZERO,
            total_start.elapsed(),
            None,
        );
        return;
    }

    if let Some(scroll) = app.help_scroll() {
        overlays::draw_help_overlay(frame, area, scroll, app);
        finalize_frame_metrics(
            app,
            total_start,
            Duration::ZERO,
            total_start.elapsed(),
            None,
        );
        return;
    }

    if let Some((scroll, content)) = app.model_status_overlay() {
        overlays::draw_model_status_overlay(frame, area, scroll, content);
        finalize_frame_metrics(
            app,
            total_start,
            Duration::ZERO,
            total_start.elapsed(),
            None,
        );
        return;
    }

    if let Some(picker_cell) = app.session_picker_overlay() {
        let mut picker = picker_cell.borrow_mut();
        picker.render(frame);
        finalize_frame_metrics(
            app,
            total_start,
            Duration::ZERO,
            total_start.elapsed(),
            None,
        );
        return;
    }

    if let Some(picker_cell) = app.login_picker_overlay() {
        let mut picker = picker_cell.borrow_mut();
        picker.render(frame);
        finalize_frame_metrics(
            app,
            total_start,
            Duration::ZERO,
            total_start.elapsed(),
            None,
        );
        return;
    }

    if let Some(picker_cell) = app.account_picker_overlay() {
        let mut picker = picker_cell.borrow_mut();
        picker.render(frame);
        finalize_frame_metrics(
            app,
            total_start,
            Duration::ZERO,
            total_start.elapsed(),
            None,
        );
        return;
    }

    // Initialize visual debug capture if enabled
    let mut debug_capture = if visual_debug::is_enabled() {
        Some(FrameCaptureBuilder::new(area.width, area.height))
    } else {
        None
    };
    let swarm_page_active = app.swarm_panel_full_page();

    // Check diagram display mode and get active diagrams early so we can
    // determine the horizontal split before computing input width etc.
    let diagram_mode = app.diagram_mode();
    let diagrams = super::mermaid::get_active_diagrams();
    let diagram_count = diagrams.len();
    let selected_index = if diagram_count > 0 {
        app.diagram_index().min(diagram_count - 1)
    } else {
        0
    };
    let pane_enabled = app.diagram_pane_enabled();
    let pane_position = app.diagram_pane_position();
    let has_side_panel_content = !swarm_page_active && app.side_panel().focused_page().is_some();
    let diff_mode = app.diff_mode();
    let collect_diffs = diff_mode.is_pinned();
    // Images now render inline in the transcript, so the side panel only handles
    // pinned file diffs. `pin_images` no longer feeds the side-panel surface.
    let has_pinned_content = if collect_diffs && !swarm_page_active {
        collect_pinned_diffs_cached(app.display_messages(), app.display_messages_version())
    } else {
        false
    };
    let has_file_diff_edits =
        !swarm_page_active && diff_mode.is_file() && app.has_display_edit_tool_messages();
    let has_right_side_pane_content =
        has_side_panel_content || has_pinned_content || has_file_diff_edits;
    // The side panel is itself a single right-hand auxiliary surface and can render
    // visual content such as Mermaid diagrams inline. Pinned image/file-diff content
    // also uses that same right-hand surface. Do not also open the global pinned
    // diagram pane while any right-hand side pane is visible, otherwise combinations
    // like pinned images + Mermaid can produce chat + side pane + diagram triple-split
    // layouts.
    let suppress_side_diagram = has_right_side_pane_content;
    let pinned_diagram = if !swarm_page_active
        && diagram_mode == crate::config::DiagramDisplayMode::Pinned
        && pane_enabled
        && !suppress_side_diagram
    {
        diagrams.get(selected_index).cloned()
    } else {
        None
    };
    let diagram_focus = app.diagram_focus();
    let (diagram_scroll_x, diagram_scroll_y) = app.diagram_scroll();

    // Compute layout depending on pane position (Side = right column, Top = above chat).
    let (chat_area, diagram_area) = if let Some(diagram) = pinned_diagram.as_ref() {
        match pane_position {
            crate::config::DiagramPanePosition::Side => {
                const MIN_DIAGRAM_WIDTH: u16 = 24;
                const MIN_CHAT_WIDTH: u16 = 20;
                let max_diagram = area.width.saturating_sub(MIN_CHAT_WIDTH);
                if max_diagram >= MIN_DIAGRAM_WIDTH {
                    let ratio = app.diagram_pane_ratio().clamp(25, 100) as u32;
                    let ratio_target = ((area.width as u32 * ratio) / 100) as u16;
                    let needed =
                        estimate_pinned_diagram_pane_width(diagram, area.height, MIN_DIAGRAM_WIDTH);
                    // The configured ratio is the upper bound for the pane so the
                    // transcript (which still renders the diagram inline) is never
                    // crushed. Shrink below the ratio when a diagram is narrow
                    // enough to need less, but do not grow past it: a large/tall
                    // diagram just scales down to fit the pane instead of eating
                    // the chat column.
                    let diagram_width = ratio_target
                        .min(needed.max(MIN_DIAGRAM_WIDTH))
                        .max(MIN_DIAGRAM_WIDTH)
                        .min(max_diagram);
                    let chat_width = area.width.saturating_sub(diagram_width);
                    if diagram_width > 0 && chat_width > 0 {
                        let chat = Rect {
                            x: area.x,
                            y: area.y,
                            width: chat_width,
                            height: area.height,
                        };
                        let diag = Rect {
                            x: area.x + chat_width,
                            y: area.y,
                            width: diagram_width,
                            height: area.height,
                        };
                        (chat, Some(diag))
                    } else {
                        (area, None)
                    }
                } else {
                    (area, None)
                }
            }
            crate::config::DiagramPanePosition::Top => {
                const MIN_DIAGRAM_HEIGHT: u16 = 6;
                const MIN_CHAT_HEIGHT: u16 = 8;
                let max_diagram = area.height.saturating_sub(MIN_CHAT_HEIGHT);
                if max_diagram >= MIN_DIAGRAM_HEIGHT {
                    let ratio = app.diagram_pane_ratio().clamp(20, 100) as u32;
                    let ratio_target = ((area.height as u32 * ratio) / 100) as u16;
                    let needed = estimate_pinned_diagram_pane_height(
                        diagram,
                        area.width,
                        MIN_DIAGRAM_HEIGHT,
                    );
                    // Cap the pane at the configured ratio so the transcript keeps
                    // its rows; shrink below it when the diagram is short. A tall
                    // diagram scales down to fit rather than swallowing the chat.
                    let diagram_height = ratio_target
                        .min(needed.max(MIN_DIAGRAM_HEIGHT))
                        .max(MIN_DIAGRAM_HEIGHT)
                        .min(max_diagram);
                    let chat_height = area.height.saturating_sub(diagram_height);
                    if diagram_height > 0 && chat_height > 0 {
                        let diag = Rect {
                            x: area.x,
                            y: area.y,
                            width: area.width,
                            height: diagram_height,
                        };
                        let chat = Rect {
                            x: area.x,
                            y: area.y + diagram_height,
                            width: area.width,
                            height: chat_height,
                        };
                        (chat, Some(diag))
                    } else {
                        (area, None)
                    }
                } else {
                    (area, None)
                }
            }
        }
    } else {
        (area, None)
    };

    let needs_side_pane = has_right_side_pane_content;

    let (chat_area, diff_pane_area) = if needs_side_pane {
        const MIN_DIFF_WIDTH: u16 = 30;
        const MIN_CHAT_WIDTH: u16 = 20;
        // Pinned images live in a tall narrow column, so a wide image fits to
        // the pane width and ends up small with empty space below it. When the
        // pane is showing image content (and the user has not manually resized
        // it), widen the default split so images use more of the available
        // horizontal space. Diffs/markdown keep the standard ratio.
        let image_dominant_pane =
            has_pinned_content && !has_file_diff_edits && !has_side_panel_content;
        const ADAPTIVE_IMAGE_RATIO: u32 = 55;
        let base_ratio = app.diagram_pane_ratio().clamp(25, 100) as u32;
        let effective_ratio = if image_dominant_pane && !app.diagram_pane_ratio_user_adjusted() {
            base_ratio.max(ADAPTIVE_IMAGE_RATIO)
        } else {
            base_ratio
        };
        let max_diff = chat_area.width.saturating_sub(MIN_CHAT_WIDTH);
        if max_diff >= MIN_DIFF_WIDTH {
            let diff_width = (((chat_area.width as u32 * effective_ratio) / 100) as u16)
                .max(MIN_DIFF_WIDTH)
                .min(max_diff);
            let new_chat_width = chat_area.width.saturating_sub(diff_width);
            let chat = Rect {
                x: chat_area.x,
                y: chat_area.y,
                width: new_chat_width,
                height: chat_area.height,
            };
            let diff = Rect {
                x: chat_area.x + new_chat_width,
                y: chat_area.y,
                width: diff_width,
                height: chat_area.height,
            };
            (chat, Some(diff))
        } else {
            (chat_area, None)
        }
    } else {
        (chat_area, None)
    };

    // Inline swarm strip: when `swarm_spawn_mode = inline` and this session
    // manages agents, render a compact strip (vertical agent list by default,
    // see `agents.swarm_strip_layout`) directly above the status line instead
    // of a big gallery band. When the panel is focused (alt+n), the selected
    // agent's row expands in place with its live transcript tail and todos;
    // alt+↑/↓ select, alt+o pops out, alt+shift+p opens the swarm prompt,
    // esc exits, and plain typing keeps flowing to the chat input. The strip stands
    // down while the SwarmStatus dock widget (margin HUD) is showing the same
    // agents, unless the panel is focused (keyboard interaction lives here).
    // The stand-down is sticky (anchored blinks count as engaged, plus a short
    // linger after disengagement) because each strip appearance adds a row to
    // the bottom chrome and shoves the transcript up: reacting to raw
    // frame-by-frame dock visibility made the strip pop in and out and the
    // whole screen bounce (flicker).
    let swarm_strip_lines: Vec<Line<'static>> = if !swarm_page_active
        && app.inline_swarm_gallery_active()
        && (app.swarm_panel_focused() || !super::info_widget::swarm_strip_stands_down_for_dock())
    {
        let members = app.inline_swarm_members();
        if chat_area.width >= 24 {
            let focus_key = crate::tui::keybind::swarm_panel_focus_key_label();
            // Use the same smooth cadence as the primary status spinner.
            let spinner_frame = (app.animation_elapsed()
                * jcode_tui_render::swarm_gallery::STRIP_SPINNER_FPS)
                as usize;
            // Focused budget: chips + hints + a ~14-line detail viewport, but
            // never more than a third of the chat column so the transcript
            // stays usable on short terminals.
            let focused_budget = ((chat_area.height as usize) / 3).clamp(3, 16);
            super::info_widget::swarm_gallery::render_swarm_strip_lines(
                &members,
                app.swarm_panel_selected(),
                app.swarm_panel_focused(),
                &focus_key,
                spinner_frame,
                chat_area.width as usize,
                focused_budget,
            )
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    let swarm_strip_height = swarm_strip_lines.len() as u16;

    // Calculate pending messages (queued + interleave) for numbering and layout
    let pending_count = input_ui::pending_prompt_count(app);
    let queued_height = pending_count.min(3) as u16;

    // Count user messages to show next prompt number
    let user_count = app.display_user_message_count();
    let next_prompt = user_count + 1;

    // Calculate input height based on the same wrapping logic used for rendering
    // (max 10 lines visible, scrolls if more).
    let base_input_height =
        input_ui::wrapped_input_line_count(app, chat_area.width, next_prompt).min(10) as u16;
    // Add 1 line for command suggestions, shell mode hints, or the Ctrl+Enter hint.
    let hint_line_height = input_ui::input_hint_line_height(app);
    let inline_block_height: u16 = inline_ui_height(app);
    let inline_ui_gap_height: u16 = if inline_block_height > 0 { 1 } else { 0 };
    let input_height = base_input_height + hint_line_height;

    if let Some(ref mut capture) = debug_capture {
        capture.render_order.push("prepare_messages".to_string());
    }
    let prep_start = Instant::now();
    let chat_left_inset = left_aligned_content_inset(chat_area.width, app.centered_mode());
    let wide_prepare_width = chat_area.width.saturating_sub(chat_left_inset);
    let narrow_prepare_width = wide_prepare_width.saturating_sub(1);
    let pinned_mermaid_aspect_ratio =
        diagram_area.and_then(|area| pinned_diagram_preferred_aspect_ratio(area, pane_position));
    // Aspect-ratio goal for transcript mermaid renders (deferred and
    // synchronous): the pinned pane's aspect wins when the pane is open so
    // inline and pane share one cached PNG; otherwise a terminal-friendly
    // inline goal keeps diagrams within a readable-height budget. Best-effort:
    // falls back to None (today's 4:3 sizing) when font geometry is unknown.
    let transcript_mermaid_aspect_ratio = mermaid::transcript_preferred_aspect_ratio(
        pinned_mermaid_aspect_ratio,
        wide_prepare_width,
        chat_area.height,
    );
    let prepare_at = |width: u16| {
        mermaid::with_preferred_aspect_ratio(transcript_mermaid_aspect_ratio, || {
            prepare::prepare_messages(app, width, chat_area.height)
        })
    };

    let onboarding_welcome = app.onboarding_welcome_active();

    // The guided onboarding phases (login import, OpenAI prompt, continue prompt)
    // are entirely key-driven and own the whole chat column: they render their own
    // telemetry header, a prominent donut, and the welcome body. Suppress the
    // normal chat chrome (status line, input box, notification, idle hint) so the
    // screen stays focused and the donut gets the full height. The resting
    // Suggestions screen keeps the input box so the user can type to start.
    let onboarding_takes_over = onboarding_welcome
        && !matches!(
            app.onboarding_welcome_kind(),
            crate::tui::OnboardingWelcomeKind::Suggestions
        );
    if onboarding_takes_over {
        onboarding::draw_onboarding_welcome(frame, app, chat_area);
        finalize_frame_metrics(
            app,
            total_start,
            prep_start.elapsed(),
            total_start.elapsed(),
            None,
        );
        return;
    }

    let show_donut = !onboarding_welcome && super::idle_donut_active(app);
    let donut_height: u16 = idle_donut_reserved_height(show_donut, input_height);
    let notification_height: u16 = if app.has_notification() { 1 } else { 0 };
    // Elastic overscroll status line revealed when the user scrolls past the
    // bottom of the transcript. Rendered directly below the input line.
    let overscroll_height: u16 = if app.chat_overscroll_active() { 1 } else { 0 };
    let fixed_height = 1
        + queued_height
        + swarm_strip_height
        + notification_height
        + inline_block_height
        + inline_ui_gap_height
        + input_height
        + overscroll_height
        + donut_height; // status + queued + swarm strip + notification + inline UI + gap + input + overscroll + donut
    let available_height = chat_area.height;
    // Overflow decisions (native scrollbar, and thus the wrap width) must not
    // depend on the transient overscroll row. Otherwise revealing the line at
    // the fits/overflows boundary flips the scrollbar on, re-wraps the whole
    // transcript one column narrower, and the extra wrapped lines keep the
    // scrollbar latched after the rebound: the screen visibly re-wraps twice
    // per overscroll and can settle in a different state than it started
    // (flicker). The packed/scrolling choice below still accounts for the real
    // row so the elastic reveal remains a clean one-row slide.
    //
    // When the line is pinned permanently visible by config it is part of the
    // stable layout, not a transient reveal, so it does count here.
    let stable_fixed_height = if app.chat_overscroll_pinned() {
        fixed_height
    } else {
        fixed_height - overscroll_height
    };
    let overflows = |prepared: &PreparedChatFrame| {
        (prepared.total_wrapped_lines().max(1) as u16) + stable_fixed_height > available_height
    };

    // Resolving native-scrollbar overflow can require wrapping the transcript at
    // two different widths (the wide layout, and one column narrower to reserve a
    // scrollbar column). Preparing both every frame doubles the most expensive work
    // and thrashes the prep caches on long transcripts during streaming. Use the
    // previous frame's scrollbar decision as hysteresis so the steady state only
    // prepares a single width; the second width is only built at a visibility
    // transition. This is safe because narrow wraps at least as much as wide, so
    // "narrow fits" implies "wide fits".
    let scrollbar_enabled = app.chat_native_scrollbar() && chat_area.width > 1;
    let initial_content_height;
    let (prepared, chat_scrollbar_visible) = if !scrollbar_enabled {
        let prepared_wide = prepare_at(wide_prepare_width);
        initial_content_height = prepared_wide.total_wrapped_lines().max(1) as u16;
        (prepared_wide, false)
    } else if last_chat_scrollbar_visible() {
        // Scrollbar was visible last frame: prepare the narrow (reserved-column)
        // layout first. If it still overflows we keep it without touching wide.
        let prepared_narrow = prepare_at(narrow_prepare_width);
        initial_content_height = prepared_narrow.total_wrapped_lines().max(1) as u16;
        if overflows(&prepared_narrow) {
            (prepared_narrow, true)
        } else {
            // Content shrank enough to fit even at the narrower width, so the wide
            // layout (which wraps no more) also fits: drop the scrollbar.
            (prepare_at(wide_prepare_width), false)
        }
    } else {
        // No scrollbar last frame: prepare the wide layout first. Only when it
        // overflows do we evaluate the narrow layout to decide on the scrollbar.
        let prepared_wide = prepare_at(wide_prepare_width);
        initial_content_height = prepared_wide.total_wrapped_lines().max(1) as u16;
        if !overflows(&prepared_wide) {
            (prepared_wide, false)
        } else {
            let prepared_narrow = prepare_at(narrow_prepare_width);
            if overflows(&prepared_narrow) {
                (prepared_narrow, true)
            } else {
                // Reserving a scrollbar column changed the wrapped content enough to
                // make it fit. Prefer the wide layout without the native scrollbar so
                // the UI does not oscillate between two self-contradictory states
                // across consecutive frames.
                (prepared_wide, false)
            }
        }
    };
    set_last_chat_scrollbar_visible(chat_scrollbar_visible);
    if let Some(ref mut capture) = debug_capture {
        capture.image_regions = prepared
            .image_regions
            .iter()
            .map(|region| ImageRegionCapture {
                hash: format!("{:016x}", region.hash),
                abs_line_idx: region.abs_line_idx,
                height: region.height,
            })
            .collect();
    }
    let prep_elapsed = prep_start.elapsed();
    let content_height = prepared.total_wrapped_lines().max(1) as u16;

    // Use packed layout when content fits, scrolling layout otherwise
    let use_packed = !swarm_page_active && content_height + fixed_height <= available_height;

    // Layout: messages (includes header), queued, status, notification, inline UI, gap, input, donut
    // All vertical chunks are within the chat_area (left column).
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(if use_packed {
            vec![
                Constraint::Length(content_height.max(1)), // 0 Messages (exact height)
                Constraint::Length(queued_height),         // 1 Queued messages (above status)
                Constraint::Length(swarm_strip_height),    // 2 Swarm strip (above status)
                Constraint::Length(1),                     // 3 Status line
                Constraint::Length(notification_height),   // 4 Notification line
                Constraint::Length(inline_block_height),   // 5 Inline UI
                Constraint::Length(inline_ui_gap_height),  // 6 Inline UI/input spacing
                Constraint::Length(input_height),          // 7 Input
                Constraint::Length(overscroll_height),     // 8 Overscroll status line
                Constraint::Length(donut_height),          // 9 Donut animation
            ]
        } else {
            vec![
                Constraint::Min(3),                       // 0 Messages (scrollable)
                Constraint::Length(queued_height),        // 1 Queued messages (above status)
                Constraint::Length(swarm_strip_height),   // 2 Swarm strip (above status)
                Constraint::Length(1),                    // 3 Status line
                Constraint::Length(notification_height),  // 4 Notification line
                Constraint::Length(inline_block_height),  // 5 Inline UI
                Constraint::Length(inline_ui_gap_height), // 6 Inline UI/input spacing
                Constraint::Length(input_height),         // 7 Input
                Constraint::Length(overscroll_height),    // 8 Overscroll status line
                Constraint::Length(donut_height),         // 9 Donut animation
            ]
        })
        .split(chat_area);
    record_status_area(chunks[3]);

    // Draw the inline swarm strip directly above the status line if present.
    if swarm_strip_height > 0 {
        clear_area(frame, chunks[2]);
        frame.render_widget(Paragraph::new(swarm_strip_lines.clone()), chunks[2]);
    }

    // Capture layout info for visual debug
    if let Some(ref mut capture) = debug_capture {
        capture.layout.use_packed = use_packed;
        capture.layout.estimated_content_height = content_height as usize;
        capture.layout.messages_area = Some(chunks[0].into());
        if queued_height > 0 {
            capture.layout.queued_area = Some(chunks[1].into());
        }
        capture.layout.status_area = Some(chunks[3].into());
        capture.layout.input_area = Some(chunks[7].into());
        capture.layout.input_lines_raw = app.input().lines().count().max(1);
        capture.layout.input_lines_wrapped = base_input_height as usize;

        // Capture state snapshot
        capture.state.is_processing = app.is_processing();
        capture.state.input_len = app.input().len();
        capture.state.input_preview = app.input().chars().take(100).collect();
        capture.state.cursor_pos = app.cursor_pos();
        capture.state.scroll_offset = app.scroll_offset();
        capture.state.queued_count = pending_count;
        capture.state.message_count = app.display_messages().len();
        capture.state.streaming_text_len = app.streaming_text().len();
        capture.state.has_suggestions = !app.command_suggestions().is_empty();
        capture.state.status = format!("{:?}", app.status());
        capture.state.diagram_mode = Some(format!("{:?}", diagram_mode));
        capture.state.diagram_focus = diagram_focus;
        capture.state.diagram_index = selected_index;
        capture.state.diagram_count = diagram_count;
        capture.state.diagram_scroll_x = diagram_scroll_x;
        capture.state.diagram_scroll_y = diagram_scroll_y;
        capture.state.diagram_pane_ratio = app.diagram_pane_ratio();
        capture.state.diagram_pane_enabled = app.diagram_pane_enabled();
        capture.state.diagram_pane_position = Some(format!("{:?}", app.diagram_pane_position()));
        capture.state.diagram_zoom = app.diagram_zoom();

        // Capture rendered content
        // Queued messages
        capture.rendered_text.queued_messages = input_ui::pending_queue_preview(app);

        // Recent display messages (last 5 for context)
        capture.rendered_text.recent_messages = app
            .display_messages()
            .iter()
            .rev()
            .take(5)
            .map(|m| MessageCapture {
                role: m.role.clone(),
                content_preview: m.content.chars().take(200).collect(),
                content_len: m.content.len(),
            })
            .collect();

        // Streaming text preview
        let streaming = app.streaming_text();
        if !streaming.is_empty() {
            capture.rendered_text.streaming_text_preview = streaming.chars().take(500).collect();
        }

        // Status line content
        capture.rendered_text.status_line = format_status_for_debug(app);
    }

    if let Some(ref mut capture) = debug_capture {
        capture.render_order.push("draw_messages".to_string());
    }
    let draw_start = Instant::now();

    // Messages area is chunks[0] within the chat column (already excludes diagram).
    let messages_area = chunks[0];
    let _ = swarm_strip_height;
    note_chat_layout(ChatLayoutMetrics {
        chat_area,
        messages_area,
        initial_content_height: initial_content_height as usize,
        content_height: content_height as usize,
        chat_scrollbar_visible,
        use_packed_layout: use_packed,
        has_side_panel_content,
        has_pinned_content,
        has_file_diff_edits,
    });

    if let Some(ref mut capture) = debug_capture {
        capture.layout.messages_area = Some(messages_area.into());
        capture.layout.diagram_area = diagram_area.map(|r| r.into());
    }
    record_layout_snapshot(messages_area, diagram_area, diff_pane_area, Some(chunks[7]));

    let margins = if onboarding_welcome {
        onboarding::draw_onboarding_welcome(frame, app, messages_area);
        info_widget::Margins {
            right_widths: Vec::new(),
            left_widths: Vec::new(),
            centered: app.centered_mode(),
            ..Default::default()
        }
    } else if swarm_page_active {
        let members = app.inline_swarm_members();
        let spinner_frame =
            (app.animation_elapsed() * jcode_tui_render::swarm_gallery::STRIP_SPINNER_FPS) as usize;
        let lines = super::info_widget::swarm_gallery::render_swarm_page_lines(
            &members,
            app.swarm_panel_selected(),
            spinner_frame,
            messages_area.width as usize,
            messages_area.height as usize,
        );
        clear_area(frame, messages_area);
        frame.render_widget(Paragraph::new(lines), messages_area);
        info_widget::Margins {
            right_widths: Vec::new(),
            left_widths: Vec::new(),
            centered: false,
            ..Default::default()
        }
    } else {
        draw_messages(
            frame,
            app,
            messages_area,
            prepared.clone(),
            chat_scrollbar_visible,
        )
    };

    crate::tui::reset_pinned_diagram_debug_snapshot();
    // Render pinned diagram if we have one
    if let (Some(diagram_info), Some(area)) = (&pinned_diagram, diagram_area) {
        if let Some(ref mut capture) = debug_capture {
            capture.render_order.push("draw_pinned_diagram".to_string());
        }
        draw_pinned_diagram(
            frame,
            diagram_info,
            area,
            selected_index,
            diagram_count,
            diagram_focus,
            diagram_scroll_x,
            diagram_scroll_y,
            app.diagram_zoom(),
            pane_position,
            app.diagram_pane_animating(),
        );
    }

    crate::tui::clear_side_panel_debug_snapshot();
    if let Some(diff_area) = diff_pane_area {
        if has_side_panel_content {
            if let Some(ref mut capture) = debug_capture {
                capture
                    .render_order
                    .push("draw_side_panel_markdown".to_string());
            }
            draw_side_panel_markdown(
                frame,
                diff_area,
                app,
                app.side_panel(),
                app.diff_pane_scroll(),
                app.diff_pane_focus(),
                app.centered_mode(),
            );
        } else if has_file_diff_edits {
            if let Some(ref mut capture) = debug_capture {
                capture.render_order.push("draw_file_diff_view".to_string());
            }
            draw_file_diff_view(
                frame,
                diff_area,
                app,
                prepared.as_ref(),
                app.diff_pane_scroll(),
                app.diff_pane_focus(),
            );
        } else if has_pinned_content {
            if let Some(ref mut capture) = debug_capture {
                capture.render_order.push("draw_pinned_content".to_string());
            }
            draw_pinned_content_cached(
                frame,
                diff_area,
                app,
                app.diff_pane_scroll(),
                app.diff_line_wrap(),
                app.diff_pane_focus(),
            );
        }
    }

    let messages_draw = draw_start.elapsed();

    if let Some(ref mut capture) = debug_capture {
        capture.layout.margins = Some(MarginsCapture {
            left_widths: margins.left_widths.clone(),
            right_widths: margins.right_widths.clone(),
            centered: margins.centered,
        });
    }
    let chrome_start = Instant::now();
    if queued_height > 0 {
        if let Some(ref mut capture) = debug_capture {
            capture.render_order.push("draw_queued".to_string());
        }
        input_ui::draw_queued(frame, app, chunks[1], user_count + 1);
    }
    if let Some(ref mut capture) = debug_capture {
        capture.render_order.push("draw_status".to_string());
    }
    input_ui::draw_status(frame, app, chunks[3], pending_count);
    if notification_height > 0 {
        input_ui::draw_notification(frame, app, chunks[4]);
    }
    if let Some(ref mut capture) = debug_capture {
        capture.render_order.push("draw_input".to_string());
    }
    // Draw inline UI if active
    if inline_block_height > 0 {
        draw_inline_ui(frame, app, chunks[5]);
    }

    let input_cursor = input_ui::draw_input(
        frame,
        app,
        chunks[7],
        user_count + pending_count + 1,
        &mut debug_capture,
    );

    if overscroll_height > 0 {
        input_ui::draw_overscroll_status(frame, app, chunks[8]);
    }

    if donut_height > 0 {
        animations::draw_idle_animation(frame, app, chunks[9]);
    }
    let chrome_elapsed = chrome_start.elapsed();

    // Draw info widget overlays (skip during idle animation - they look out of place)
    let widget_data_start = Instant::now();
    let widget_data = app.info_widget_data();
    let widget_data_elapsed = widget_data_start.elapsed();
    let mut widget_render_ms: Option<f32> = None;
    let mut placements: Vec<info_widget::WidgetPlacement> = Vec::new();
    let widget_bounds = messages_area;
    if app.info_widget_overlays_enabled()
        && !widget_data.is_empty()
        && !show_donut
        && !swarm_page_active
    {
        if let Some(ref mut capture) = debug_capture {
            capture.render_order.push("render_info_widgets".to_string());
        }
        placements = info_widget::calculate_placements(widget_bounds, &margins, &widget_data);

        if let Some(ref mut capture) = debug_capture {
            let placement_captures = capture_widget_placements(&placements);
            capture.layout.widget_placements = placement_captures.clone();
            capture.info_widgets = Some(InfoWidgetCapture {
                summary: build_info_widget_summary(&widget_data),
                placements: placement_captures,
            });

            // Detect overlaps with used content. Info widgets live inside the
            // messages rectangle by design, so a whole-area overlap check is
            // always true and useless; instead verify each placement still fits
            // within the free margin the layout reported for the rows it covers.
            for placement in &placements {
                if widget_overlaps_content(placement, widget_bounds, &margins) {
                    capture.anomaly(format!(
                        "Info widget {:?} intrudes into content (rect {:?})",
                        placement.kind, placement.rect
                    ));
                }
                if !rect_within_bounds(placement.rect, area) {
                    capture.anomaly(format!(
                        "Info widget {:?} out of bounds {:?}",
                        placement.kind, placement.rect
                    ));
                }
                if let Some(diagram_area) = diagram_area
                    && rects_overlap(placement.rect, diagram_area)
                {
                    capture.anomaly(format!(
                        "Info widget {:?} overlaps diagram area",
                        placement.kind
                    ));
                }
            }
            for i in 0..placements.len() {
                for j in (i + 1)..placements.len() {
                    if rects_overlap(placements[i].rect, placements[j].rect) {
                        capture.anomaly(format!(
                            "Info widgets overlap: {:?} and {:?}",
                            placements[i].kind, placements[j].kind
                        ));
                    }
                }
            }
        }

        let widget_start = Instant::now();
        info_widget::render_all(frame, &placements, &widget_data);
        widget_render_ms = Some(widget_start.elapsed().as_secs_f32() * 1000.0);

        // Optional visual overlay for placements
    } else {
        // The widget pass did not run (idle donut takeover or no widget data),
        // so nothing from the previous frame is on screen anymore. Clear the
        // remembered placements/anchors so consumers of last-frame state (the
        // swarm strip stand-down, idle fallback facts) do not keep reacting to
        // widgets that are no longer drawn.
        info_widget::note_widget_pass_skipped();
        if let Some(ref mut capture) = debug_capture {
            capture.info_widgets = Some(InfoWidgetCapture {
                summary: build_info_widget_summary(&widget_data),
                placements: Vec::new(),
            });
        }
    }

    if visual_debug::overlay_enabled() {
        overlays::draw_debug_overlay(frame, &placements, &chunks);
    }

    // Session facts use actual final-frame cells for collision detection. They
    // prefer the composer chrome and may climb into a few transcript-tail rows
    // only when the right suffix is genuinely unused.
    input_ui::draw_right_fact_stack(
        frame,
        app,
        messages_area,
        chunks[7],
        chat_scrollbar_visible,
        input_cursor,
    );

    // Command-suggestion popover: a late overlay pass so the palette floats
    // over existing rows (blank space, pinned footer, or the transcript tail)
    // instead of reserving layout height and shoving everything around.
    input_ui::draw_command_suggestions_overlay(frame, app, chunks[7]);

    // Observe the rendered messages area for the anchor-stability (smoothness)
    // report. Runs on the final buffer so it sees exactly what the user sees.
    smoothness::observe_frame(
        frame.buffer_mut(),
        messages_area,
        app.scroll_offset(),
        !app.auto_scroll_paused(),
    );

    let frame_elapsed = total_start.elapsed();
    if frame_elapsed >= Duration::from_millis(250) {
        crate::logging::warn(&format!(
            "TUI_RENDER_PHASES prepare={}ms messages={}ms chrome={}ms widget_data={}ms widget_render={}ms final={}ms total={}ms",
            prep_elapsed.as_millis(),
            messages_draw.as_millis(),
            chrome_elapsed.as_millis(),
            widget_data_elapsed.as_millis(),
            widget_render_ms.unwrap_or_default(),
            frame_elapsed
                .saturating_sub(prep_elapsed)
                .saturating_sub(messages_draw)
                .saturating_sub(chrome_elapsed)
                .saturating_sub(widget_data_elapsed)
                .saturating_sub(Duration::from_secs_f32(
                    widget_render_ms.unwrap_or_default() / 1000.0,
                ))
                .as_millis(),
            frame_elapsed.as_millis(),
        ));
    }

    // Record the frame capture if enabled
    if let Some(capture) = debug_capture {
        let total_draw = draw_start.elapsed();
        let render_timing = RenderTimingCapture {
            prepare_ms: prep_elapsed.as_secs_f32() * 1000.0,
            draw_ms: total_draw.as_secs_f32() * 1000.0,
            total_ms: total_start.elapsed().as_secs_f32() * 1000.0,
            messages_ms: Some(messages_draw.as_secs_f32() * 1000.0),
            widgets_ms: widget_render_ms,
        };

        let mut capture = capture;
        capture.render_timing = Some(render_timing);
        capture.mermaid = crate::tui::mermaid::debug_stats_json();
        capture.side_panel = crate::tui::side_panel_debug_json();
        capture.markdown = crate::tui::markdown::debug_stats_json();
        capture.theme = overlays::debug_palette_json();
        visual_debug::record_frame(capture.build());
    }

    finalize_frame_metrics(
        app,
        total_start,
        prep_elapsed,
        draw_start.elapsed(),
        Some(messages_draw.as_secs_f64() * 1000.0),
    );
}

pub(crate) fn split_native_scrollbar_area(area: Rect, enabled: bool) -> (Rect, Option<Rect>) {
    if !enabled || area.width <= 1 {
        return (area, None);
    }

    let content = Rect {
        width: area.width.saturating_sub(1),
        ..area
    };
    let scrollbar = Rect {
        x: area.x.saturating_add(area.width.saturating_sub(1)),
        y: area.y,
        width: 1,
        height: area.height,
    };
    (content, Some(scrollbar))
}

pub(crate) fn native_scrollbar_visible(
    enabled: bool,
    total_lines: usize,
    visible_height: usize,
) -> bool {
    enabled && visible_height > 0 && total_lines > visible_height
}

pub(crate) fn render_native_scrollbar(
    frame: &mut Frame,
    area: Rect,
    scroll: usize,
    total_lines: usize,
    visible_height: usize,
    focused: bool,
) {
    if area.width == 0
        || area.height == 0
        || !native_scrollbar_visible(true, total_lines, visible_height)
    {
        return;
    }

    let track_height = area.height as usize;
    let thumb_height = if visible_height == 0 || total_lines == 0 {
        1
    } else if total_lines <= visible_height {
        track_height
    } else {
        ((visible_height * track_height).div_ceil(total_lines)).clamp(1, track_height)
    };
    let max_thumb_offset = track_height.saturating_sub(thumb_height);
    let max_scroll = total_lines.saturating_sub(visible_height);
    let thumb_offset = if max_scroll == 0 {
        0
    } else {
        scroll.min(max_scroll) * max_thumb_offset / max_scroll
    };

    let thumb_color = if focused {
        rgb(188, 208, 240)
    } else {
        rgb(136, 148, 172)
    };

    let mut lines = Vec::with_capacity(track_height);
    for row in 0..track_height {
        let (glyph, color) = if row >= thumb_offset && row < thumb_offset + thumb_height {
            let glyph = if thumb_height == 1 {
                "•"
            } else if row == thumb_offset {
                "╷"
            } else if row + 1 == thumb_offset + thumb_height {
                "╵"
            } else {
                "│"
            };
            (glyph, thumb_color)
        } else {
            (" ", Color::Reset)
        };
        lines.push(Line::from(Span::styled(glyph, Style::default().fg(color))));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

#[cfg(test)]
#[path = "ui_tests/mod.rs"]
mod tests;
