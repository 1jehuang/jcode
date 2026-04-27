#![cfg_attr(
    test,
    allow(
        clippy::bind_instead_of_map,
        clippy::clone_on_copy,
        clippy::collapsible_if,
        clippy::if_same_then_else,
        clippy::implicit_saturating_sub,
        clippy::items_after_test_module,
        clippy::large_enum_variant,
        clippy::let_and_return,
        clippy::manual_abs_diff,
        clippy::manual_div_ceil,
        clippy::manual_find,
        clippy::manual_is_multiple_of,
        clippy::manual_pattern_char_comparison,
        clippy::manual_repeat_n,
        clippy::manual_strip,
        clippy::map_entry,
        clippy::missing_const_for_thread_local,
        clippy::needless_borrow,
        clippy::needless_borrows_for_generic_args,
        clippy::needless_lifetimes,
        clippy::needless_range_loop,
        clippy::needless_return,
        clippy::question_mark,
        clippy::redundant_closure,
        clippy::too_many_arguments,
        clippy::type_complexity,
        clippy::unnecessary_cast,
        clippy::unnecessary_lazy_evaluations,
        clippy::unnecessary_map_or,
        clippy::unwrap_or_default,
        clippy::while_let_loop
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
use super::{DisplayMessage, ProcessingStatus, TuiState, is_unexpected_cache_miss};
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
#[path = "ui_header.rs"]
mod header;
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
#[path = "ui_overlays.rs"]
mod overlays;
#[path = "ui_pinned.rs"]
mod pinned_ui;
#[path = "ui_prepare.rs"]
mod prepare;
#[path = "ui_tools.rs"]
pub(crate) mod tools_ui;
#[path = "ui_transitions.rs"]
mod transitions;
#[path = "ui_viewport.rs"]
mod viewport;

#[cfg(test)]
use box_utils::truncate_line_to_width;
use box_utils::{
    line_plain_text, render_rounded_box, truncate_line_preserving_suffix_to_width,
    truncate_line_with_ellipsis_to_width,
};
use changelog::get_grouped_changelog;
#[cfg(test)]
use changelog::{ChangelogEntry, group_changelog_entries, parse_changelog_from};
use debug_capture::{
    build_info_widget_summary, capture_widget_placements, rect_within_bounds, rects_overlap,
};
pub use diagram_pane::{
    PinnedDiagramLiveDebugSnapshot, PinnedDiagramProbeRect, debug_probe_pinned_diagram,
};
#[cfg(test)]
use diagram_pane::{
    div_ceil_u32, estimate_pinned_diagram_pane_width_with_font, is_diagram_poor_fit,
    vcenter_fitted_image_with_font,
};
use diagram_pane::{
    draw_pinned_diagram, estimate_pinned_diagram_pane_height, estimate_pinned_diagram_pane_width,
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
pub(crate) use messages::{
    render_assistant_message, render_background_task_message, render_swarm_message,
    render_system_message, render_tool_message, render_usage_message,
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
    collect_pinned_content_cached, draw_pinned_content_cached, draw_side_panel_markdown,
};
#[cfg(test)]
use transitions::extract_line_text;
#[cfg(test)]
use transitions::inline_ui_gap_height;
#[cfg(test)]
use viewport::compute_visible_margins;
use viewport::draw_messages;
/// Last known max scroll value from the renderer. Updated each frame.
/// Scroll handlers use this to clamp scroll_offset and prevent overshoot.
#[cfg(not(test))]
static LAST_MAX_SCROLL: AtomicUsize = AtomicUsize::new(0);
/// Whether the chat viewport used a native scrollbar in the most recent frame.
#[cfg(not(test))]
static LAST_CHAT_SCROLLBAR_VISIBLE: AtomicUsize = AtomicUsize::new(0);
/// Total line count in the pinned diff/content pane (set during render).
#[cfg(not(test))]
static PINNED_PANE_TOTAL_LINES: AtomicUsize = AtomicUsize::new(0);
/// Effective scroll position of the side pane after render-time clamping.
#[cfg(not(test))]
static LAST_DIFF_PANE_EFFECTIVE_SCROLL: AtomicUsize = AtomicUsize::new(0);
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
    static TEST_LAST_USER_PROMPT_POSITIONS: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    static TEST_LAST_LAYOUT: RefCell<Option<LayoutSnapshot>> = const { RefCell::new(None) };
    static TEST_VISIBLE_COPY_TARGETS: RefCell<Vec<VisibleCopyTarget>> = RefCell::new(Vec::new());
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

/// Render context window as vertical list with smart grouping
/// Items < 5% are grouped by category (docs, msgs, etc.)
/// Pre-computed image region from line scanning
#[derive(Clone, Copy)]
struct ImageRegion {
    /// Absolute line index in wrapped_lines
    abs_line_idx: usize,
    /// Absolute exclusive end line of the image placeholder region.
    end_line: usize,
    /// Hash of the mermaid content (for cache lookup)
    hash: u64,
    /// Total height of the image placeholder in lines
    height: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CopyTargetKind {
    CodeBlock { language: Option<String> },
    Error,
    ToolOutput,
}

impl CopyTargetKind {
    fn label(&self) -> String {
        match self {
            Self::CodeBlock { language } => language
                .as_deref()
                .filter(|lang| !lang.is_empty())
                .unwrap_or("code")
                .to_string(),
            Self::Error => "error".to_string(),
            Self::ToolOutput => "output".to_string(),
        }
    }

    fn copied_notice(&self) -> String {
        match self {
            Self::CodeBlock { language } => {
                let label = language
                    .as_deref()
                    .filter(|lang| !lang.is_empty())
                    .unwrap_or("code block");
                format!("Copied {}", label)
            }
            Self::Error => "Copied error".to_string(),
            Self::ToolOutput => "Copied output".to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct RawCopyTarget {
    pub(crate) kind: CopyTargetKind,
    pub(crate) content: String,
    pub(crate) start_raw_line: usize,
    pub(crate) end_raw_line: usize,
    pub(crate) badge_raw_line: usize,
}

#[derive(Clone, Debug)]
struct CopyTarget {
    kind: CopyTargetKind,
    content: String,
    start_line: usize,
    end_line: usize,
    badge_line: usize,
}

#[derive(Clone)]
struct PreparedMessages {
    wrapped_lines: Vec<Line<'static>>,
    wrapped_plain_lines: Arc<Vec<String>>,
    wrapped_copy_offsets: Arc<Vec<usize>>,
    raw_plain_lines: Arc<Vec<String>>,
    wrapped_line_map: Arc<Vec<WrappedLineMap>>,
    wrapped_user_indices: Vec<usize>,
    /// Wrapped line indices where a user prompt line starts
    wrapped_user_prompt_starts: Vec<usize>,
    /// Wrapped line indices where a user prompt line ends (exclusive)
    wrapped_user_prompt_ends: Vec<usize>,
    /// Flattened user prompt text in display order, used by prompt preview without
    /// scanning display_messages on every frame.
    user_prompt_texts: Vec<String>,
    /// Pre-scanned image regions (computed once, not every frame)
    image_regions: Vec<ImageRegion>,
    /// Line ranges for edit tool messages: (msg_index, start_line, end_line)
    /// Used by File diff mode to determine which edit is visible at current scroll
    edit_tool_ranges: Vec<EditToolRange>,
    copy_targets: Vec<CopyTarget>,
}

#[derive(Clone)]
struct PreparedSection {
    kind: PreparedSectionKind,
    prepared: Arc<PreparedMessages>,
    line_start: usize,
    raw_start: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PreparedSectionKind {
    Body,
    Header,
    BatchProgress,
    Streaming,
}

#[derive(Clone)]
struct PreparedChatFrame {
    sections: Vec<PreparedSection>,
    total_wrapped_lines: usize,
    total_raw_lines: usize,
    wrapped_user_indices: Vec<usize>,
    wrapped_user_prompt_starts: Vec<usize>,
    wrapped_user_prompt_ends: Vec<usize>,
    user_prompt_texts: Vec<String>,
    image_regions: Vec<ImageRegion>,
    edit_tool_ranges: Vec<EditToolRange>,
    copy_targets: Vec<CopyTarget>,
}

impl PreparedChatFrame {
    fn from_single(prepared: Arc<PreparedMessages>) -> Self {
        Self::from_sections(vec![(PreparedSectionKind::Body, prepared)])
    }

    fn from_sections(sections: Vec<(PreparedSectionKind, Arc<PreparedMessages>)>) -> Self {
        let mut prepared_sections = Vec::new();
        let mut line_start = 0usize;
        let mut raw_start = 0usize;
        let mut wrapped_user_indices = Vec::new();
        let mut wrapped_user_prompt_starts = Vec::new();
        let mut wrapped_user_prompt_ends = Vec::new();
        let mut user_prompt_texts = Vec::new();
        let mut image_regions = Vec::new();
        let mut edit_tool_ranges = Vec::new();
        let mut copy_targets = Vec::new();

        for (kind, prepared) in sections {
            if prepared.wrapped_lines.is_empty()
                && prepared.raw_plain_lines.is_empty()
                && prepared.image_regions.is_empty()
                && prepared.edit_tool_ranges.is_empty()
                && prepared.copy_targets.is_empty()
            {
                continue;
            }

            wrapped_user_indices.extend(
                prepared
                    .wrapped_user_indices
                    .iter()
                    .map(|idx| idx + line_start),
            );
            wrapped_user_prompt_starts.extend(
                prepared
                    .wrapped_user_prompt_starts
                    .iter()
                    .map(|idx| idx + line_start),
            );
            wrapped_user_prompt_ends.extend(
                prepared
                    .wrapped_user_prompt_ends
                    .iter()
                    .map(|idx| idx + line_start),
            );
            user_prompt_texts.extend(prepared.user_prompt_texts.iter().cloned());
            image_regions.extend(prepared.image_regions.iter().map(|region| ImageRegion {
                abs_line_idx: region.abs_line_idx + line_start,
                end_line: region.end_line + line_start,
                hash: region.hash,
                height: region.height,
            }));
            edit_tool_ranges.extend(prepared.edit_tool_ranges.iter().map(|range| EditToolRange {
                edit_index: range.edit_index,
                msg_index: range.msg_index,
                file_path: range.file_path.clone(),
                start_line: range.start_line + line_start,
                end_line: range.end_line + line_start,
            }));
            copy_targets.extend(prepared.copy_targets.iter().map(|target| CopyTarget {
                kind: target.kind.clone(),
                content: target.content.clone(),
                start_line: target.start_line + line_start,
                end_line: target.end_line + line_start,
                badge_line: target.badge_line + line_start,
            }));
            prepared_sections.push(PreparedSection {
                kind,
                prepared: prepared.clone(),
                line_start,
                raw_start,
            });
            line_start += prepared.wrapped_lines.len();
            raw_start += prepared.raw_plain_lines.len();
        }

        Self {
            sections: prepared_sections,
            total_wrapped_lines: line_start,
            total_raw_lines: raw_start,
            wrapped_user_indices,
            wrapped_user_prompt_starts,
            wrapped_user_prompt_ends,
            user_prompt_texts,
            image_regions,
            edit_tool_ranges,
            copy_targets,
        }
    }

    fn total_wrapped_lines(&self) -> usize {
        self.total_wrapped_lines
    }

    fn wrapped_plain_line_count(&self) -> usize {
        self.total_wrapped_lines
    }

    fn visible_intersects_section(
        &self,
        kind: PreparedSectionKind,
        start: usize,
        end: usize,
    ) -> bool {
        if end <= start {
            return false;
        }

        self.sections.iter().any(|section| {
            if section.kind != kind {
                return false;
            }
            let section_start = section.line_start;
            let section_end = section_start + section.prepared.wrapped_lines.len();
            section_start < end && start < section_end
        })
    }

    fn line_section(&self, abs_line: usize) -> Option<(&PreparedSection, usize)> {
        self.sections.iter().find_map(|section| {
            let local = abs_line.checked_sub(section.line_start)?;
            (local < section.prepared.wrapped_lines.len()).then_some((section, local))
        })
    }

    fn raw_section(&self, raw_line: usize) -> Option<(&PreparedSection, usize)> {
        self.sections.iter().find_map(|section| {
            let local = raw_line.checked_sub(section.raw_start)?;
            (local < section.prepared.raw_plain_lines.len()).then_some((section, local))
        })
    }

    fn wrapped_plain_line(&self, abs_line: usize) -> Option<String> {
        let (section, local) = self.line_section(abs_line)?;
        section.prepared.wrapped_plain_lines.get(local).cloned()
    }

    fn wrapped_copy_offset(&self, abs_line: usize) -> Option<usize> {
        let (section, local) = self.line_section(abs_line)?;
        section.prepared.wrapped_copy_offsets.get(local).copied()
    }

    fn raw_plain_line(&self, raw_line: usize) -> Option<String> {
        let (_, local) = self.raw_section(raw_line)?;
        self.raw_section(raw_line)?
            .0
            .prepared
            .raw_plain_lines
            .get(local)
            .cloned()
    }

    fn wrapped_line_map(&self, abs_line: usize) -> Option<WrappedLineMap> {
        let (section, local) = self.line_section(abs_line)?;
        let map = section.prepared.wrapped_line_map.get(local)?;
        Some(WrappedLineMap {
            raw_line: map.raw_line + section.raw_start,
            start_col: map.start_col,
            end_col: map.end_col,
        })
    }

    fn materialize_line_slice(&self, start: usize, end: usize) -> Vec<Line<'static>> {
        let end = end.min(self.total_wrapped_lines);
        if start >= end {
            return Vec::new();
        }

        let mut lines = Vec::with_capacity(end - start);
        for section in &self.sections {
            let section_start = section.line_start;
            let section_end = section_start + section.prepared.wrapped_lines.len();
            let overlap_start = start.max(section_start);
            let overlap_end = end.min(section_end);
            if overlap_start >= overlap_end {
                continue;
            }
            let local_start = overlap_start - section_start;
            let local_end = overlap_end - section_start;
            lines.extend_from_slice(&section.prepared.wrapped_lines[local_start..local_end]);
        }
        lines
    }

    #[cfg(test)]
    fn materialize_all_lines(&self) -> Vec<Line<'static>> {
        self.materialize_line_slice(0, self.total_wrapped_lines)
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct WrappedLineMap {
    pub(crate) raw_line: usize,
    pub(crate) start_col: usize,
    pub(crate) end_col: usize,
}

#[derive(Clone, Debug)]
struct EditToolRange {
    edit_index: usize,
    msg_index: usize,
    file_path: String,
    start_line: usize,
    end_line: usize,
}

#[derive(Clone, Debug)]
struct ActiveFileDiffContext {
    edit_index: usize,
    msg_index: usize,
    file_path: String,
    start_line: usize,
    end_line: usize,
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
fn visible_copy_targets_state() -> &'static Mutex<Vec<VisibleCopyTarget>> {
    VISIBLE_COPY_TARGETS.get_or_init(|| Mutex::new(Vec::new()))
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
}

#[derive(Clone)]
struct BodyCacheEntry {
    key: BodyCacheKey,
    prepared: Arc<PreparedMessages>,
    prepared_bytes: usize,
    msg_count: usize,
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
        msg_count: usize,
    ) -> Option<(Arc<PreparedMessages>, usize)> {
        let regular = self
            .entries
            .iter()
            .filter(|entry| {
                entry.msg_count > 0
                    && msg_count > entry.msg_count
                    && entry.key.width == key.width
                    && entry.key.diff_mode == key.diff_mode
                    && entry.key.diagram_mode == key.diagram_mode
                    && entry.key.centered == key.centered
            })
            .max_by_key(|entry| entry.msg_count)
            .map(|entry| (entry.prepared.clone(), entry.msg_count));
        let oversized = self
            .oversized_entries
            .iter()
            .filter(|entry| {
                entry.msg_count > 0
                    && msg_count > entry.msg_count
                    && entry.key.width == key.width
                    && entry.key.diff_mode == key.diff_mode
                    && entry.key.diagram_mode == key.diagram_mode
                    && entry.key.centered == key.centered
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
        msg_count: usize,
    ) -> Option<(Arc<PreparedMessages>, usize)> {
        let regular = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry.msg_count > 0
                    && msg_count > entry.msg_count
                    && entry.key.width == key.width
                    && entry.key.diff_mode == key.diff_mode
                    && entry.key.diagram_mode == key.diagram_mode
                    && entry.key.centered == key.centered
            })
            .max_by_key(|(_, entry)| entry.msg_count)
            .map(|(idx, entry)| (false, idx, entry.msg_count));
        let oversized = self
            .oversized_entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry.msg_count > 0
                    && msg_count > entry.msg_count
                    && entry.key.width == key.width
                    && entry.key.diff_mode == key.diff_mode
                    && entry.key.diagram_mode == key.diagram_mode
                    && entry.key.centered == key.centered
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
        Some((entry.prepared, msg_count))
    }

    fn insert(&mut self, key: BodyCacheKey, prepared: Arc<PreparedMessages>, msg_count: usize) {
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
    is_processing: bool,
    streaming_text_len: usize,
    streaming_text_hash: u64,
    batch_progress_hash: u64,
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

#[derive(Clone, Debug, Default, Serialize)]
struct FramePerfStats {
    full_prep_requests: usize,
    full_prep_hits: usize,
    full_prep_oversized_hits: usize,
    full_prep_misses: usize,
    full_prep_last_prepared_bytes: usize,
    full_prep_last_total_wrapped_lines: usize,
    full_prep_last_section_count: usize,
    body_requests: usize,
    body_hits: usize,
    body_oversized_hits: usize,
    body_misses: usize,
    body_incremental_reuses: usize,
    body_last_incremental_base_messages: Option<usize>,
    body_last_prepared_bytes: usize,
    body_last_wrapped_lines: usize,
    body_last_copy_targets: usize,
    body_last_image_regions: usize,
    viewport_scroll: usize,
    viewport_visible_end: usize,
    viewport_visible_lines: usize,
    viewport_total_wrapped_lines: usize,
    viewport_prompt_preview_lines: u16,
    viewport_visible_user_prompts: usize,
    viewport_visible_copy_targets: usize,
    viewport_content_width: u16,
    viewport_stability_hash: u64,
    viewport_visible_streaming_hash: u64,
    viewport_visible_batch_progress_hash: u64,
    chat_area_width: u16,
    chat_area_height: u16,
    messages_area_width: u16,
    messages_area_height: u16,
    content_height: usize,
    initial_content_height: usize,
    chat_scrollbar_visible: bool,
    use_packed_layout: bool,
    has_side_panel_content: bool,
    has_pinned_content: bool,
    has_file_diff_edits: bool,
}

#[derive(Clone, Debug, Serialize)]
struct SlowFrameSample {
    timestamp_ms: u64,
    threshold_ms: f64,
    session_id: Option<String>,
    session_name: Option<String>,
    status: String,
    diff_mode: String,
    centered: bool,
    is_processing: bool,
    auto_scroll_paused: bool,
    display_messages: usize,
    display_messages_version: u64,
    user_messages: usize,
    queued_messages: usize,
    streaming_text_len: usize,
    prepare_ms: f64,
    draw_ms: f64,
    total_ms: f64,
    messages_ms: Option<f64>,
    perf: FramePerfStats,
}

#[derive(Clone, Debug, Serialize)]
struct FlickerFrameSample {
    timestamp_ms: u64,
    session_id: Option<String>,
    session_name: Option<String>,
    display_messages_version: u64,
    diff_mode: String,
    centered: bool,
    is_processing: bool,
    auto_scroll_paused: bool,
    scroll: usize,
    visible_end: usize,
    visible_lines: usize,
    total_wrapped_lines: usize,
    prompt_preview_lines: u16,
    messages_area_width: u16,
    messages_area_height: u16,
    content_width: u16,
    chat_scrollbar_visible: bool,
    visible_hash: u64,
    visible_streaming_hash: u64,
    visible_batch_progress_hash: u64,
    total_ms: f64,
    prepare_ms: f64,
    draw_ms: f64,
}

#[derive(Clone, Debug, Serialize)]
struct FlickerEvent {
    timestamp_ms: u64,
    kind: String,
    session_id: Option<String>,
    session_name: Option<String>,
    previous: FlickerFrameSample,
    current: FlickerFrameSample,
}

#[derive(Clone, Debug)]
pub(crate) struct FlickerUiNotice {
    pub(crate) summary: String,
    pub(crate) hint: String,
}

// Keep this outside h/j/k/l for the same reason as COPY_BADGE_KEYS.
pub(super) const FLICKER_NOTICE_COPY_KEY: char = 'z';

#[derive(Default)]
struct SlowFrameHistory {
    samples: VecDeque<SlowFrameSample>,
    last_log_at_ms: Option<u64>,
}

#[derive(Default)]
struct FlickerFrameHistory {
    samples: VecDeque<FlickerFrameSample>,
    events: VecDeque<FlickerEvent>,
    last_log_at_ms: Option<u64>,
}

const SLOW_FRAME_HISTORY_MAX_SAMPLES: usize = 128;
const SLOW_FRAME_LOG_INTERVAL_MS: u64 = 1_000;
const FLICKER_HISTORY_MAX_SAMPLES: usize = 256;
const FLICKER_HISTORY_MAX_EVENTS: usize = 128;
const FLICKER_LOG_INTERVAL_MS: u64 = 500;
#[cfg(not(test))]
const FLICKER_UI_NOTICE_MAX_AGE_MS: u64 = 30_000;

static FRAME_PERF_STATS: OnceLock<Mutex<FramePerfStats>> = OnceLock::new();
static SLOW_FRAME_HISTORY: OnceLock<Mutex<SlowFrameHistory>> = OnceLock::new();
static FLICKER_FRAME_HISTORY: OnceLock<Mutex<FlickerFrameHistory>> = OnceLock::new();

fn frame_perf_stats() -> &'static Mutex<FramePerfStats> {
    FRAME_PERF_STATS.get_or_init(|| Mutex::new(FramePerfStats::default()))
}

fn slow_frame_history() -> &'static Mutex<SlowFrameHistory> {
    SLOW_FRAME_HISTORY.get_or_init(|| Mutex::new(SlowFrameHistory::default()))
}

fn flicker_frame_history() -> &'static Mutex<FlickerFrameHistory> {
    FLICKER_FRAME_HISTORY.get_or_init(|| Mutex::new(FlickerFrameHistory::default()))
}

fn wall_clock_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn slow_frame_threshold_ms() -> f64 {
    static THRESHOLD_MS: OnceLock<f64> = OnceLock::new();
    *THRESHOLD_MS.get_or_init(|| {
        std::env::var("JCODE_TUI_SLOW_FRAME_MS")
            .ok()
            .and_then(|raw| raw.trim().parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value > 0.0)
            .unwrap_or(40.0)
    })
}

fn flicker_detection_enabled() -> bool {
    #[cfg(test)]
    {
        true
    }

    #[cfg(not(test))]
    {
        static ENABLED: OnceLock<bool> = OnceLock::new();
        *ENABLED.get_or_init(|| {
            std::env::var("JCODE_TUI_FLICKER_DETECTION")
                .ok()
                .map(|raw| {
                    matches!(
                        raw.trim().to_ascii_lowercase().as_str(),
                        "1" | "true" | "yes" | "on"
                    )
                })
                .unwrap_or(false)
        })
    }
}

fn with_frame_perf_stats_mut(f: impl FnOnce(&mut FramePerfStats)) {
    let mut stats = frame_perf_stats()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    f(&mut stats);
}

fn reset_frame_perf_stats() {
    with_frame_perf_stats_mut(|stats| *stats = FramePerfStats::default());
}

fn frame_perf_stats_snapshot() -> FramePerfStats {
    frame_perf_stats()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

fn note_full_prep_request() {
    with_frame_perf_stats_mut(|stats| stats.full_prep_requests += 1);
}

fn note_full_prep_cache_hit(kind: CacheEntryKind, prepared: &PreparedChatFrame) {
    with_frame_perf_stats_mut(|stats| {
        stats.full_prep_hits += 1;
        if matches!(kind, CacheEntryKind::Oversized) {
            stats.full_prep_oversized_hits += 1;
        }
        stats.full_prep_last_prepared_bytes = estimate_prepared_chat_frame_bytes(prepared);
        stats.full_prep_last_total_wrapped_lines = prepared.total_wrapped_lines();
        stats.full_prep_last_section_count = prepared.sections.len();
    });
}

fn note_full_prep_cache_miss() {
    with_frame_perf_stats_mut(|stats| stats.full_prep_misses += 1);
}

fn note_full_prep_built(prepared: &PreparedChatFrame) {
    with_frame_perf_stats_mut(|stats| {
        stats.full_prep_last_prepared_bytes = estimate_prepared_chat_frame_bytes(prepared);
        stats.full_prep_last_total_wrapped_lines = prepared.total_wrapped_lines();
        stats.full_prep_last_section_count = prepared.sections.len();
    });
}

fn note_body_request() {
    with_frame_perf_stats_mut(|stats| stats.body_requests += 1);
}

fn note_body_cache_hit(kind: CacheEntryKind, prepared: &PreparedMessages) {
    with_frame_perf_stats_mut(|stats| {
        stats.body_hits += 1;
        if matches!(kind, CacheEntryKind::Oversized) {
            stats.body_oversized_hits += 1;
        }
        stats.body_last_prepared_bytes = estimate_prepared_messages_bytes(prepared);
        stats.body_last_wrapped_lines = prepared.wrapped_lines.len();
        stats.body_last_copy_targets = prepared.copy_targets.len();
        stats.body_last_image_regions = prepared.image_regions.len();
    });
}

fn note_body_cache_miss() {
    with_frame_perf_stats_mut(|stats| stats.body_misses += 1);
}

fn note_body_incremental_reuse(base_messages: usize) {
    with_frame_perf_stats_mut(|stats| {
        stats.body_incremental_reuses += 1;
        stats.body_last_incremental_base_messages = Some(base_messages);
    });
}

fn note_body_built(prepared: &PreparedMessages) {
    with_frame_perf_stats_mut(|stats| {
        stats.body_last_prepared_bytes = estimate_prepared_messages_bytes(prepared);
        stats.body_last_wrapped_lines = prepared.wrapped_lines.len();
        stats.body_last_copy_targets = prepared.copy_targets.len();
        stats.body_last_image_regions = prepared.image_regions.len();
    });
}

pub(super) struct ChatLayoutMetrics {
    chat_area: Rect,
    messages_area: Rect,
    initial_content_height: usize,
    content_height: usize,
    chat_scrollbar_visible: bool,
    use_packed_layout: bool,
    has_side_panel_content: bool,
    has_pinned_content: bool,
    has_file_diff_edits: bool,
}

fn note_chat_layout(metrics: ChatLayoutMetrics) {
    let ChatLayoutMetrics {
        chat_area,
        messages_area,
        initial_content_height,
        content_height,
        chat_scrollbar_visible,
        use_packed_layout,
        has_side_panel_content,
        has_pinned_content,
        has_file_diff_edits,
    } = metrics;
    with_frame_perf_stats_mut(|stats| {
        stats.chat_area_width = chat_area.width;
        stats.chat_area_height = chat_area.height;
        stats.messages_area_width = messages_area.width;
        stats.messages_area_height = messages_area.height;
        stats.initial_content_height = initial_content_height;
        stats.content_height = content_height;
        stats.chat_scrollbar_visible = chat_scrollbar_visible;
        stats.use_packed_layout = use_packed_layout;
        stats.has_side_panel_content = has_side_panel_content;
        stats.has_pinned_content = has_pinned_content;
        stats.has_file_diff_edits = has_file_diff_edits;
    });
}

pub(super) struct ViewportMetrics {
    pub scroll: usize,
    pub visible_end: usize,
    pub visible_lines: usize,
    pub total_wrapped_lines: usize,
    pub prompt_preview_lines: u16,
    pub visible_user_prompts: usize,
    pub visible_copy_targets: usize,
    pub content_width: u16,
    pub stability_hash: u64,
    pub visible_streaming_hash: u64,
    pub visible_batch_progress_hash: u64,
}

fn note_viewport_metrics(metrics: ViewportMetrics) {
    let ViewportMetrics {
        scroll,
        visible_end,
        visible_lines,
        total_wrapped_lines,
        prompt_preview_lines,
        visible_user_prompts,
        visible_copy_targets,
        content_width,
        stability_hash,
        visible_streaming_hash,
        visible_batch_progress_hash,
    } = metrics;
    with_frame_perf_stats_mut(|stats| {
        stats.viewport_scroll = scroll;
        stats.viewport_visible_end = visible_end;
        stats.viewport_visible_lines = visible_lines;
        stats.viewport_total_wrapped_lines = total_wrapped_lines;
        stats.viewport_prompt_preview_lines = prompt_preview_lines;
        stats.viewport_visible_user_prompts = visible_user_prompts;
        stats.viewport_visible_copy_targets = visible_copy_targets;
        stats.viewport_content_width = content_width;
        stats.viewport_stability_hash = stability_hash;
        stats.viewport_visible_streaming_hash = visible_streaming_hash;
        stats.viewport_visible_batch_progress_hash = visible_batch_progress_hash;
    });
}

pub(super) fn viewport_stability_hash(
    visible_lines: &[Line<'_>],
    visible_user_indices: &[usize],
    content_width: u16,
    prompt_preview_lines: u16,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    content_width.hash(&mut hasher);
    prompt_preview_lines.hash(&mut hasher);
    visible_lines.len().hash(&mut hasher);
    visible_user_indices.hash(&mut hasher);
    for line in visible_lines {
        line.alignment.hash(&mut hasher);
        line_plain_text(line).hash(&mut hasher);
    }
    hasher.finish()
}

fn same_flicker_state_key(a: &FlickerFrameSample, b: &FlickerFrameSample) -> bool {
    a.session_id == b.session_id
        && a.display_messages_version == b.display_messages_version
        && a.diff_mode == b.diff_mode
        && a.centered == b.centered
        && a.is_processing == b.is_processing
        && a.auto_scroll_paused == b.auto_scroll_paused
        && a.scroll == b.scroll
        && a.visible_end == b.visible_end
        && a.visible_lines == b.visible_lines
        && a.total_wrapped_lines == b.total_wrapped_lines
        && a.prompt_preview_lines == b.prompt_preview_lines
        && a.messages_area_width == b.messages_area_width
        && a.messages_area_height == b.messages_area_height
        && a.visible_streaming_hash == b.visible_streaming_hash
        && a.visible_batch_progress_hash == b.visible_batch_progress_hash
}

fn same_flicker_context_key(a: &FlickerFrameSample, b: &FlickerFrameSample) -> bool {
    a.session_id == b.session_id
        && a.display_messages_version == b.display_messages_version
        && a.diff_mode == b.diff_mode
        && a.centered == b.centered
        && a.is_processing == b.is_processing
        && a.auto_scroll_paused == b.auto_scroll_paused
        && a.messages_area_width == b.messages_area_width
        && a.messages_area_height == b.messages_area_height
}

fn sample_has_visible_transient_content(sample: &FlickerFrameSample) -> bool {
    sample.visible_streaming_hash != 0 || sample.visible_batch_progress_hash != 0
}

fn push_flicker_event(history: &mut FlickerFrameHistory, event: FlickerEvent) {
    history.events.push_back(event.clone());
    while history.events.len() > FLICKER_HISTORY_MAX_EVENTS {
        history.events.pop_front();
    }

    let severe = event.kind.contains("oscillation");
    let should_log = severe
        || history
            .last_log_at_ms
            .map(|last| event.timestamp_ms.saturating_sub(last) >= FLICKER_LOG_INTERVAL_MS)
            .unwrap_or(true);
    if should_log {
        history.last_log_at_ms = Some(event.timestamp_ms);
        if let Ok(payload) = serde_json::to_string(&event) {
            crate::logging::warn(&format!("TUI_FLICKER_EVENT {}", payload));
        } else {
            crate::logging::warn(&format!(
                "TUI_FLICKER_EVENT kind={} session={:?}",
                event.kind, event.session_name
            ));
        }
    }
}

fn maybe_record_flicker_event(history: &mut FlickerFrameHistory, current: &FlickerFrameSample) {
    let Some(previous) = history.samples.back().cloned() else {
        return;
    };

    let len = history.samples.len();
    if len >= 2 {
        let earlier = history.samples.get(len - 2).cloned();
        if let Some(earlier) = earlier
            && same_flicker_state_key(&earlier, current)
            && same_flicker_state_key(&earlier, &previous)
            && earlier.visible_hash == current.visible_hash
            && earlier.chat_scrollbar_visible == current.chat_scrollbar_visible
            && earlier.content_width == current.content_width
            && (earlier.chat_scrollbar_visible != previous.chat_scrollbar_visible
                || earlier.content_width != previous.content_width)
        {
            push_flicker_event(
                history,
                FlickerEvent {
                    timestamp_ms: current.timestamp_ms,
                    kind: "layout_oscillation".to_string(),
                    session_id: current.session_id.clone(),
                    session_name: current.session_name.clone(),
                    previous,
                    current: current.clone(),
                },
            );
            return;
        }
    }

    if len >= 2 {
        let earlier = history.samples.get(len - 2).cloned();
        if let Some(earlier) = earlier
            && same_flicker_context_key(&earlier, current)
            && same_flicker_context_key(&earlier, &previous)
            && !current.auto_scroll_paused
            && earlier.visible_hash == current.visible_hash
            && earlier.content_width == current.content_width
            && earlier.chat_scrollbar_visible == current.chat_scrollbar_visible
            && (previous.visible_hash != current.visible_hash
                || previous.content_width != current.content_width
                || previous.chat_scrollbar_visible != current.chat_scrollbar_visible)
        {
            push_flicker_event(
                history,
                FlickerEvent {
                    timestamp_ms: current.timestamp_ms,
                    kind: "layout_feedback_oscillation".to_string(),
                    session_id: current.session_id.clone(),
                    session_name: current.session_name.clone(),
                    previous,
                    current: current.clone(),
                },
            );
            return;
        }
    }

    if same_flicker_state_key(&previous, current) {
        if previous.chat_scrollbar_visible != current.chat_scrollbar_visible
            || previous.content_width != current.content_width
        {
            push_flicker_event(
                history,
                FlickerEvent {
                    timestamp_ms: current.timestamp_ms,
                    kind: "layout_toggle_same_state".to_string(),
                    session_id: current.session_id.clone(),
                    session_name: current.session_name.clone(),
                    previous: previous.clone(),
                    current: current.clone(),
                },
            );
        } else if previous.visible_hash != current.visible_hash
            && !sample_has_visible_transient_content(&previous)
            && !sample_has_visible_transient_content(current)
        {
            push_flicker_event(
                history,
                FlickerEvent {
                    timestamp_ms: current.timestamp_ms,
                    kind: "visible_hash_changed_same_state".to_string(),
                    session_id: current.session_id.clone(),
                    session_name: current.session_name.clone(),
                    previous,
                    current: current.clone(),
                },
            );
        }
    }
}

fn record_flicker_frame_sample(sample: FlickerFrameSample) {
    if !flicker_detection_enabled() {
        return;
    }

    let mut history = flicker_frame_history()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    maybe_record_flicker_event(&mut history, &sample);
    history.samples.push_back(sample);
    while history.samples.len() > FLICKER_HISTORY_MAX_SAMPLES {
        history.samples.pop_front();
    }
}

fn finalize_frame_metrics(
    app: &dyn TuiState,
    total_start: Instant,
    prep_elapsed: Duration,
    draw_elapsed: Duration,
    messages_ms: Option<f64>,
) {
    if profile_enabled() {
        record_profile(prep_elapsed, draw_elapsed, total_start.elapsed());
    }

    let total_elapsed = total_start.elapsed();
    let total_ms = total_elapsed.as_secs_f64() * 1000.0;
    let perf = frame_perf_stats_snapshot();
    record_flicker_frame_sample(FlickerFrameSample {
        timestamp_ms: wall_clock_ms(),
        session_id: app.current_session_id(),
        session_name: app.session_display_name(),
        display_messages_version: app.display_messages_version(),
        diff_mode: format!("{:?}", app.diff_mode()),
        centered: app.centered_mode(),
        is_processing: app.is_processing(),
        auto_scroll_paused: app.auto_scroll_paused(),
        scroll: perf.viewport_scroll,
        visible_end: perf.viewport_visible_end,
        visible_lines: perf.viewport_visible_lines,
        total_wrapped_lines: perf.viewport_total_wrapped_lines,
        prompt_preview_lines: perf.viewport_prompt_preview_lines,
        messages_area_width: perf.messages_area_width,
        messages_area_height: perf.messages_area_height,
        content_width: perf.viewport_content_width,
        chat_scrollbar_visible: perf.chat_scrollbar_visible,
        visible_hash: perf.viewport_stability_hash,
        visible_streaming_hash: perf.viewport_visible_streaming_hash,
        visible_batch_progress_hash: perf.viewport_visible_batch_progress_hash,
        total_ms,
        prepare_ms: prep_elapsed.as_secs_f64() * 1000.0,
        draw_ms: draw_elapsed.as_secs_f64() * 1000.0,
    });

    let threshold_ms = slow_frame_threshold_ms();
    if total_ms >= threshold_ms {
        record_slow_frame_sample(SlowFrameSample {
            timestamp_ms: wall_clock_ms(),
            threshold_ms,
            session_id: app.current_session_id(),
            session_name: app.session_display_name(),
            status: format!("{:?}", app.status()),
            diff_mode: format!("{:?}", app.diff_mode()),
            centered: app.centered_mode(),
            is_processing: app.is_processing(),
            auto_scroll_paused: app.auto_scroll_paused(),
            display_messages: app.display_messages().len(),
            display_messages_version: app.display_messages_version(),
            user_messages: app.display_user_message_count(),
            queued_messages: app.queued_messages().len(),
            streaming_text_len: app.streaming_text().len(),
            prepare_ms: prep_elapsed.as_secs_f64() * 1000.0,
            draw_ms: draw_elapsed.as_secs_f64() * 1000.0,
            total_ms,
            messages_ms,
            perf,
        });
    }
}

pub(crate) fn debug_flicker_frame_history(limit: usize) -> serde_json::Value {
    let history = flicker_frame_history()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let take_samples = limit.clamp(1, FLICKER_HISTORY_MAX_SAMPLES);
    let samples: Vec<FlickerFrameSample> = history
        .samples
        .iter()
        .rev()
        .take(take_samples)
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let events: Vec<FlickerEvent> = history
        .events
        .iter()
        .rev()
        .take(limit.clamp(1, FLICKER_HISTORY_MAX_EVENTS))
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    serde_json::json!({
        "enabled": flicker_detection_enabled(),
        "buffered_samples": history.samples.len(),
        "returned_samples": samples.len(),
        "buffered_events": history.events.len(),
        "returned_events": events.len(),
        "summary": {
            "layout_toggle_events": events.iter().filter(|event| event.kind == "layout_toggle_same_state").count(),
            "layout_oscillation_events": events.iter().filter(|event| event.kind == "layout_oscillation").count(),
            "layout_feedback_oscillation_events": events.iter().filter(|event| event.kind == "layout_feedback_oscillation").count(),
            "visible_hash_change_events": events.iter().filter(|event| event.kind == "visible_hash_changed_same_state").count(),
        },
        "events": events,
        "samples": samples,
    })
}

fn flicker_event_label(kind: &str) -> &str {
    match kind {
        "layout_toggle_same_state" => "layout toggle",
        "layout_oscillation" => "layout oscillation",
        "layout_feedback_oscillation" => "layout feedback oscillation",
        "visible_hash_changed_same_state" => "same-state redraw",
        _ => kind,
    }
}

fn abbreviate_flicker_log_path(path: &std::path::Path) -> String {
    let rendered = path.display().to_string();
    if let Some(home) = dirs::home_dir() {
        let home = home.display().to_string();
        if rendered == home {
            return "~".to_string();
        }
        if let Some(rest) = rendered.strip_prefix(&home) {
            return format!("~{}", rest);
        }
    }
    rendered
}

pub(crate) fn recent_flicker_ui_notice() -> Option<FlickerUiNotice> {
    if !flicker_detection_enabled() {
        return None;
    }

    let history = flicker_frame_history()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let event = history.events.back()?.clone();
    drop(history);

    #[cfg(not(test))]
    {
        let now = wall_clock_ms();
        if now.saturating_sub(event.timestamp_ms) > FLICKER_UI_NOTICE_MAX_AGE_MS {
            return None;
        }
    }

    let log_hint = crate::logging::log_path()
        .map(|path| abbreviate_flicker_log_path(&path))
        .unwrap_or_else(|| "~/.jcode/logs/".to_string());
    let summary = format!("⚠ flicker detected ({})", flicker_event_label(&event.kind));
    let hint = format!("logs: {} · debug: client:flicker-frames 32", log_hint);
    Some(FlickerUiNotice { summary, hint })
}

pub(crate) fn recent_flicker_copy_target_for_key(key: char) -> Option<VisibleCopyTarget> {
    if !key.eq_ignore_ascii_case(&FLICKER_NOTICE_COPY_KEY) {
        return None;
    }

    let notice = recent_flicker_ui_notice()?;
    Some(VisibleCopyTarget {
        key: FLICKER_NOTICE_COPY_KEY,
        kind_label: "flicker hint".to_string(),
        copied_notice: "Copied flicker hint".to_string(),
        content: notice.hint,
    })
}

fn record_slow_frame_sample(sample: SlowFrameSample) {
    let mut history = slow_frame_history()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    history.samples.push_back(sample.clone());
    while history.samples.len() > SLOW_FRAME_HISTORY_MAX_SAMPLES {
        history.samples.pop_front();
    }

    let severe = sample.total_ms >= sample.threshold_ms * 2.0;
    let should_log = severe
        || history
            .last_log_at_ms
            .map(|last| sample.timestamp_ms.saturating_sub(last) >= SLOW_FRAME_LOG_INTERVAL_MS)
            .unwrap_or(true);
    if should_log {
        history.last_log_at_ms = Some(sample.timestamp_ms);
        if let Ok(payload) = serde_json::to_string(&sample) {
            crate::logging::warn(&format!("TUI_SLOW_FRAME {}", payload));
        } else {
            crate::logging::warn(&format!(
                "TUI_SLOW_FRAME total_ms={:.2} prepare_ms={:.2} draw_ms={:.2}",
                sample.total_ms, sample.prepare_ms, sample.draw_ms
            ));
        }
    }
}

pub(crate) fn debug_slow_frame_history(limit: usize) -> serde_json::Value {
    let history = slow_frame_history()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let take = limit.clamp(1, SLOW_FRAME_HISTORY_MAX_SAMPLES);
    let samples: Vec<SlowFrameSample> = history
        .samples
        .iter()
        .rev()
        .take(take)
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    let max_total_ms = samples
        .iter()
        .map(|sample| sample.total_ms)
        .fold(0.0, f64::max);
    let max_prepare_ms = samples
        .iter()
        .map(|sample| sample.prepare_ms)
        .fold(0.0, f64::max);
    let max_draw_ms = samples
        .iter()
        .map(|sample| sample.draw_ms)
        .fold(0.0, f64::max);

    serde_json::json!({
        "threshold_ms": slow_frame_threshold_ms(),
        "buffered_samples": history.samples.len(),
        "returned_samples": samples.len(),
        "summary": {
            "max_total_ms": max_total_ms,
            "max_prepare_ms": max_prepare_ms,
            "max_draw_ms": max_draw_ms,
        },
        "samples": samples,
    })
}

#[cfg(test)]
pub(crate) fn clear_slow_frame_history_for_tests() {
    let mut history = slow_frame_history()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    history.samples.clear();
    history.last_log_at_ms = None;
    reset_frame_perf_stats();
    set_last_chat_scrollbar_visible(false);
}

#[cfg(test)]
pub(crate) fn clear_flicker_frame_history_for_tests() {
    let mut history = flicker_frame_history()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    history.samples.clear();
    history.events.clear();
    history.last_log_at_ms = None;
    set_last_chat_scrollbar_visible(false);
}

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
    update_user_prompt_positions(&[]);
    TEST_LAST_LAYOUT.with(|snapshot| {
        *snapshot.borrow_mut() = None;
    });
    set_visible_copy_targets(Vec::new());
    clear_copy_viewport_snapshot();

    TEST_PROMPT_VIEWPORT_STATE.with(|state| {
        *state.borrow_mut() = PromptViewportState::default();
    });
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

    fn wrapped_plain_line(&self, abs_line: usize) -> Option<String> {
        match &self.data {
            CopyViewportData::Dense {
                wrapped_plain_lines,
                ..
            } => wrapped_plain_lines.get(abs_line).cloned(),
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

    fn raw_plain_line(&self, raw_line: usize) -> Option<String> {
        match &self.data {
            CopyViewportData::Dense {
                raw_plain_lines, ..
            } => raw_plain_lines.get(raw_line).cloned(),
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
}

#[derive(Clone, Default)]
struct CopyViewportSnapshots {
    chat: Option<CopyViewportSnapshot>,
    side: Option<CopyViewportSnapshot>,
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
            }
        })
    }
    #[cfg(not(test))]
    {
        let snapshots = copy_viewport_state().lock().ok()?.clone();
        match pane {
            crate::tui::CopySelectionPane::Chat => snapshots.chat,
            crate::tui::CopySelectionPane::SidePane => snapshots.side,
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
    record_side_pane_snapshot_precomputed(
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
    }
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
    copy_snapshot_for_pane(pane)?.wrapped_plain_line(abs_line)
}

pub(crate) fn copy_viewport_line_text(abs_line: usize) -> Option<String> {
    copy_pane_line_text(crate::tui::CopySelectionPane::Chat, abs_line)
}

pub(crate) fn side_pane_line_text(abs_line: usize) -> Option<String> {
    copy_pane_line_text(crate::tui::CopySelectionPane::SidePane, abs_line)
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

    let mut out = Vec::new();
    for abs_line in start.abs_line..=end.abs_line {
        let text = snapshot.wrapped_plain_line(abs_line)?;
        let line_width = line_display_width(&text);
        let copy_start = snapshot.wrapped_copy_offset(abs_line).unwrap_or(0);
        let start_col = if abs_line == start.abs_line {
            clamp_display_col(&text, start.column).max(copy_start)
        } else {
            copy_start
        };
        let end_col = if abs_line == end.abs_line {
            clamp_display_col(&text, end.column).max(copy_start)
        } else {
            line_width
        };

        if end_col < start_col {
            out.push(String::new());
            continue;
        }

        out.push(display_col_slice(&text, start_col, end_col).to_string());
    }

    Some(out.join("\n"))
}

pub(crate) fn link_target_from_screen(column: u16, row: u16) -> Option<String> {
    let point = copy_point_from_screen(column, row)?;
    let snapshot = copy_snapshot_for_pane(point.pane)?;
    link_target_from_snapshot(&snapshot, point)
}

pub fn draw(frame: &mut Frame, app: &dyn TuiState) {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::tui::markdown::with_deferred_mermaid_render_context(|| draw_inner(frame, app))
    })) {
        Ok(()) => {}
        Err(payload) => render_recovered_panic_frame(frame, &payload),
    }
}

fn draw_inner(frame: &mut Frame, app: &dyn TuiState) {
    let area = frame.area().intersection(*frame.buffer_mut().area());
    if area.width == 0 || area.height == 0 {
        return;
    }

    let total_start = Instant::now();
    reset_frame_perf_stats();

    clear_copy_viewport_snapshot();

    // Clear full frame to prevent stale cells from prior layouts.
    // This is critical on macOS terminals where ratatui's diff-based updates
    // can leave outdated content when layout dimensions change between frames
    // (e.g., diagram pane toggling, streaming text clearing, tool calls finishing).
    // Uses Color::Reset (terminal default bg) so text selection highlighting works
    // natively in all terminal emulators.
    clear_area(frame, area);

    if let Some(scroll) = app.changelog_scroll() {
        overlays::draw_changelog_overlay(frame, area, scroll);
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
        let picker = picker_cell.borrow();
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
        let picker = picker_cell.borrow();
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
    let has_side_panel_content = app.side_panel().focused_page().is_some();
    let suppress_side_diagram =
        has_side_panel_content && pane_position == crate::config::DiagramPanePosition::Side;
    let pinned_diagram = if diagram_mode == crate::config::DiagramDisplayMode::Pinned
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
                const AUTO_DIAGRAM_WIDTH_CAP_PERCENT: u32 = 50;
                let max_diagram = area.width.saturating_sub(MIN_CHAT_WIDTH);
                if max_diagram >= MIN_DIAGRAM_WIDTH {
                    let ratio = app.diagram_pane_ratio().clamp(25, 100) as u32;
                    let ratio_target = ((area.width as u32 * ratio) / 100) as u16;
                    let auto_cap =
                        ((area.width as u32 * AUTO_DIAGRAM_WIDTH_CAP_PERCENT) / 100) as u16;
                    let needed =
                        estimate_pinned_diagram_pane_width(diagram, area.height, MIN_DIAGRAM_WIDTH);
                    let auto_target = needed.min(max_diagram).min(auto_cap.max(MIN_DIAGRAM_WIDTH));
                    let diagram_width = ratio_target
                        .max(auto_target)
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
                    let diagram_height = ratio_target
                        .max(needed.min(max_diagram))
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

    let diff_mode = app.diff_mode();
    let pin_images = app.pin_images();
    let collect_diffs = diff_mode.is_pinned();
    let has_pinned_content = if collect_diffs || pin_images {
        collect_pinned_content_cached(
            app.display_messages(),
            &app.side_pane_images(),
            collect_diffs,
            pin_images,
            app.display_messages_version(),
        )
    } else {
        false
    };
    let has_file_diff_edits = diff_mode.is_file() && app.has_display_edit_tool_messages();

    let needs_side_pane = has_side_panel_content || has_pinned_content || has_file_diff_edits;

    let (chat_area, diff_pane_area) = if needs_side_pane {
        const MIN_DIFF_WIDTH: u16 = 30;
        const MIN_CHAT_WIDTH: u16 = 20;
        let max_diff = chat_area.width.saturating_sub(MIN_CHAT_WIDTH);
        if max_diff >= MIN_DIFF_WIDTH {
            let diff_width = (((chat_area.width as u32
                * app.diagram_pane_ratio().clamp(25, 100) as u32)
                / 100) as u16)
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
    let prepared_wide = prepare::prepare_messages(app, wide_prepare_width, chat_area.height);
    let show_donut = super::idle_donut_active(app);
    let donut_height: u16 = if show_donut { 14 } else { 0 };
    let notification_height: u16 = if app.has_notification() { 1 } else { 0 };
    let fixed_height = 1
        + queued_height
        + notification_height
        + inline_block_height
        + inline_ui_gap_height
        + input_height
        + donut_height; // status + queued + notification + inline UI + gap + input + donut
    let available_height = chat_area.height;

    let initial_content_height = prepared_wide.total_wrapped_lines().max(1) as u16;
    let wide_overflows = app.chat_native_scrollbar()
        && chat_area.width > 1
        && initial_content_height + fixed_height > available_height;
    let (prepared, chat_scrollbar_visible) = if !wide_overflows {
        (prepared_wide, false)
    } else {
        let narrow_prepare_width = wide_prepare_width.saturating_sub(1);
        let prepared_narrow =
            prepare::prepare_messages(app, narrow_prepare_width, chat_area.height);
        let narrow_content_height = prepared_narrow.total_wrapped_lines().max(1) as u16;
        let narrow_overflows = narrow_content_height + fixed_height > available_height;
        if narrow_overflows {
            (prepared_narrow, true)
        } else {
            // Reserving a scrollbar column changed the wrapped content enough to make it fit.
            // Prefer the wide layout without the native scrollbar so the UI does not oscillate
            // between two self-contradictory states across consecutive frames.
            (prepared_wide, false)
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
    let use_packed = content_height + fixed_height <= available_height;

    // Layout: messages (includes header), queued, status, notification, inline UI, gap, input, donut
    // All vertical chunks are within the chat_area (left column).
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(if use_packed {
            vec![
                Constraint::Length(content_height.max(1)), // Messages (exact height)
                Constraint::Length(queued_height),         // Queued messages (above status)
                Constraint::Length(1),                     // Status line
                Constraint::Length(notification_height),   // Notification line
                Constraint::Length(inline_block_height),   // Inline UI
                Constraint::Length(inline_ui_gap_height),  // Inline UI/input spacing
                Constraint::Length(input_height),          // Input
                Constraint::Length(donut_height),          // Donut animation
            ]
        } else {
            vec![
                Constraint::Min(3),                       // Messages (scrollable)
                Constraint::Length(queued_height),        // Queued messages (above status)
                Constraint::Length(1),                    // Status line
                Constraint::Length(notification_height),  // Notification line
                Constraint::Length(inline_block_height),  // Inline UI
                Constraint::Length(inline_ui_gap_height), // Inline UI/input spacing
                Constraint::Length(input_height),         // Input
                Constraint::Length(donut_height),         // Donut animation
            ]
        })
        .split(chat_area);

    // Capture layout info for visual debug
    if let Some(ref mut capture) = debug_capture {
        capture.layout.use_packed = use_packed;
        capture.layout.estimated_content_height = content_height as usize;
        capture.layout.messages_area = Some(chunks[0].into());
        if queued_height > 0 {
            capture.layout.queued_area = Some(chunks[1].into());
        }
        capture.layout.status_area = Some(chunks[2].into());
        capture.layout.input_area = Some(chunks[6].into());
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
    record_layout_snapshot(messages_area, diagram_area, diff_pane_area, Some(chunks[6]));

    let margins = draw_messages(
        frame,
        app,
        messages_area,
        prepared.clone(),
        chat_scrollbar_visible,
    );

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
    if queued_height > 0 {
        if let Some(ref mut capture) = debug_capture {
            capture.render_order.push("draw_queued".to_string());
        }
        input_ui::draw_queued(frame, app, chunks[1], user_count + 1);
    }
    if let Some(ref mut capture) = debug_capture {
        capture.render_order.push("draw_status".to_string());
    }
    input_ui::draw_status(frame, app, chunks[2], pending_count);
    if notification_height > 0 {
        input_ui::draw_notification(frame, app, chunks[3]);
    }
    if let Some(ref mut capture) = debug_capture {
        capture.render_order.push("draw_input".to_string());
    }
    // Draw inline UI if active
    if inline_block_height > 0 {
        draw_inline_ui(frame, app, chunks[4]);
    }

    input_ui::draw_input(
        frame,
        app,
        chunks[6],
        user_count + pending_count + 1,
        &mut debug_capture,
    );

    if donut_height > 0 {
        animations::draw_idle_animation(frame, app, chunks[7]);
    }

    // Draw info widget overlays (skip during idle animation - they look out of place)
    let widget_data = app.info_widget_data();
    let mut widget_render_ms: Option<f32> = None;
    let mut placements: Vec<info_widget::WidgetPlacement> = Vec::new();
    let widget_bounds = messages_area;
    if !widget_data.is_empty() && !show_donut {
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

            // Detect overlaps with message area
            for placement in &placements {
                if rects_overlap(placement.rect, widget_bounds) {
                    capture.anomaly(format!(
                        "Info widget {:?} overlaps messages area",
                        placement.kind
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
    } else if let Some(ref mut capture) = debug_capture {
        capture.info_widgets = Some(InfoWidgetCapture {
            summary: build_info_widget_summary(&widget_data),
            placements: Vec::new(),
        });
    }

    if visual_debug::overlay_enabled() {
        overlays::draw_debug_overlay(frame, &placements, &chunks);
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
