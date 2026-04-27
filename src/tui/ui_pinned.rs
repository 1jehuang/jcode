use super::*;
mod ui_pinned_table;
use ui_pinned_table::is_rendered_table_line;

#[path = "ui_pinned_layout.rs"]
mod layout_support;
#[path = "ui_pinned_utils.rs"]
mod util_support;
use crate::tui::mermaid;
#[cfg(test)]
use layout_support::{clamp_side_panel_image_rows, estimate_side_panel_image_rows_with_font};
use layout_support::{
    estimate_side_panel_image_layout, estimate_side_panel_image_layout_with_font,
    fit_image_area_with_font, plan_fit_image_render, side_panel_viewport_scroll_x,
};
#[cfg(test)]
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use util_support::{
    compact_image_label, estimate_inline_image_rows, estimate_side_panel_pane_area, lru_touch,
    side_panel_content_signature,
};

const SIDE_PANEL_HEADER_HEIGHT: u16 = 1;

fn side_panel_border_style(focused: bool) -> Style {
    let border_color = if focused { tool_color() } else { dim_color() };
    Style::default().fg(border_color)
}

fn side_panel_inner(area: Rect) -> Rect {
    ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::LEFT)
        .inner(area)
}

fn side_panel_content_area(area: Rect) -> Option<Rect> {
    let inner = side_panel_inner(area);
    if inner.width == 0 || inner.height <= SIDE_PANEL_HEADER_HEIGHT {
        return None;
    }

    Some(Rect {
        x: inner.x,
        y: inner.y + SIDE_PANEL_HEADER_HEIGHT,
        width: inner.width,
        height: inner.height - SIDE_PANEL_HEADER_HEIGHT,
    })
}

#[path = "ui_pinned_selection.rs"]
mod selection_support;
use selection_support::apply_side_selection_highlight;

enum PinnedContentEntry {
    Diff {
        file_path: String,
        lines: Vec<ParsedDiffLine>,
        additions: usize,
        deletions: usize,
    },
    Image {
        label: String,
        media_type: String,
        source: crate::session::RenderedImageSource,
        hash: u64,
        width: u32,
        height: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImageGroup {
    Inputs,
    Tools,
    Other,
}

fn image_group_for(source: &crate::session::RenderedImageSource) -> ImageGroup {
    match source {
        crate::session::RenderedImageSource::UserInput => ImageGroup::Inputs,
        crate::session::RenderedImageSource::ToolResult { .. } => ImageGroup::Tools,
        crate::session::RenderedImageSource::Other { .. } => ImageGroup::Other,
    }
}

fn image_group_heading(group: ImageGroup) -> (&'static str, Color) {
    match group {
        ImageGroup::Inputs => ("inputs", rgb(138, 180, 248)),
        ImageGroup::Tools => ("tools", accent_color()),
        ImageGroup::Other => ("other", dim_color()),
    }
}

fn image_source_badge(source: &crate::session::RenderedImageSource) -> String {
    match source {
        crate::session::RenderedImageSource::UserInput => "input".to_string(),
        crate::session::RenderedImageSource::ToolResult { tool_name } => {
            format!("tool:{}", tool_name)
        }
        crate::session::RenderedImageSource::Other { role } => role.clone(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PinnedCacheKey {
    messages_version: u64,
    collect_diffs: bool,
    collect_images: bool,
}

#[derive(Default)]
struct PinnedCacheState {
    key: Option<PinnedCacheKey>,
    entries: Vec<PinnedContentEntry>,
    rendered_lines: Option<PinnedRenderedCache>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SidePanelMarkdownKey {
    page_id: String,
    content_signature: u64,
    inner_width: u16,
    has_protocol: bool,
    centered: bool,
    mermaid_epoch: u64,
}

#[derive(Default)]
struct SidePanelMarkdownCacheState {
    entries: HashMap<SidePanelMarkdownKey, RenderedSidePanelMarkdown>,
    order: VecDeque<SidePanelMarkdownKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SidePanelRenderKey {
    page_id: String,
    content_signature: u64,
    inner_width: u16,
    inner_height: u16,
    has_protocol: bool,
    centered: bool,
    mermaid_epoch: u64,
}

#[derive(Default)]
struct SidePanelRenderCacheState {
    entries: HashMap<SidePanelRenderKey, PinnedRenderedCache>,
    order: VecDeque<SidePanelRenderKey>,
}

#[path = "ui_pinned_mermaid_debug.rs"]
mod mermaid_debug_support;
pub use mermaid_debug_support::{
    SidePanelDebugStats, SidePanelLiveDebugSnapshot, SidePanelMermaidProbe,
    SidePanelMermaidProbeRect, SidePanelVisibleMermaidDebug, debug_probe_side_panel_mermaid,
};
use mermaid_debug_support::{build_side_panel_mermaid_probe_from_image, probe_rect};

#[derive(Default)]
struct SidePanelDebugState {
    stats: SidePanelDebugStats,
    live_snapshot: Option<SidePanelLiveDebugSnapshot>,
}

#[derive(Clone)]
struct RenderedSidePanelMarkdown {
    rendered_markdown: Vec<Line<'static>>,
    placeholder_hashes: Vec<Option<u64>>,
    has_following_content_after: Vec<bool>,
}

#[derive(Clone)]
struct PinnedRenderedCache {
    inner_width: u16,
    line_wrap: bool,
    lines: Vec<Line<'static>>,
    wrapped_plain_lines: std::sync::Arc<Vec<String>>,
    wrapped_copy_offsets: std::sync::Arc<Vec<usize>>,
    raw_plain_lines: std::sync::Arc<Vec<String>>,
    wrapped_line_map: std::sync::Arc<Vec<WrappedLineMap>>,
    left_margins: Vec<u16>,
    image_placements: Vec<PinnedImagePlacement>,
    has_scrollable_images: bool,
}

fn estimate_lines_bytes(lines: &[Line<'static>]) -> usize {
    lines
        .iter()
        .map(|line| {
            std::mem::size_of::<Line<'static>>()
                + line.spans.capacity() * std::mem::size_of::<Span<'static>>()
                + line
                    .spans
                    .iter()
                    .map(|span| span.content.len())
                    .sum::<usize>()
        })
        .sum()
}

fn estimate_arc_string_vec_bytes(values: &std::sync::Arc<Vec<String>>) -> usize {
    std::mem::size_of::<Vec<String>>()
        + values.capacity() * std::mem::size_of::<String>()
        + values.iter().map(|value| value.capacity()).sum::<usize>()
}

fn estimate_arc_usize_vec_bytes(values: &std::sync::Arc<Vec<usize>>) -> usize {
    std::mem::size_of::<Vec<usize>>() + values.capacity() * std::mem::size_of::<usize>()
}

fn estimate_arc_wrapped_line_map_bytes(values: &std::sync::Arc<Vec<WrappedLineMap>>) -> usize {
    std::mem::size_of::<Vec<WrappedLineMap>>()
        + values.capacity() * std::mem::size_of::<WrappedLineMap>()
}

fn estimate_pinned_rendered_cache_bytes(cache: &PinnedRenderedCache) -> usize {
    estimate_lines_bytes(&cache.lines)
        + estimate_arc_string_vec_bytes(&cache.wrapped_plain_lines)
        + estimate_arc_usize_vec_bytes(&cache.wrapped_copy_offsets)
        + estimate_arc_string_vec_bytes(&cache.raw_plain_lines)
        + estimate_arc_wrapped_line_map_bytes(&cache.wrapped_line_map)
        + cache.left_margins.capacity() * std::mem::size_of::<u16>()
        + cache.image_placements.capacity() * std::mem::size_of::<PinnedImagePlacement>()
}

fn estimate_rendered_side_panel_markdown_bytes(value: &RenderedSidePanelMarkdown) -> usize {
    estimate_lines_bytes(&value.rendered_markdown)
        + value.placeholder_hashes.capacity() * std::mem::size_of::<Option<u64>>()
        + value.has_following_content_after.capacity() * std::mem::size_of::<bool>()
}

fn estimate_pinned_content_entry_bytes(entry: &PinnedContentEntry) -> usize {
    match entry {
        PinnedContentEntry::Diff {
            file_path, lines, ..
        } => {
            file_path.capacity()
                + lines.capacity() * std::mem::size_of::<crate::tui::ui_diff::ParsedDiffLine>()
                + lines
                    .iter()
                    .map(|line| line.prefix.capacity() + line.content.capacity())
                    .sum::<usize>()
        }
        PinnedContentEntry::Image {
            label,
            media_type,
            source,
            ..
        } => {
            let source_bytes = match source {
                crate::session::RenderedImageSource::UserInput => 0,
                crate::session::RenderedImageSource::ToolResult { tool_name } => {
                    tool_name.capacity()
                }
                crate::session::RenderedImageSource::Other { role } => role.capacity(),
            };
            label.capacity() + media_type.capacity() + source_bytes
        }
    }
}

fn estimate_side_panel_markdown_key_bytes(key: &SidePanelMarkdownKey) -> usize {
    key.page_id.capacity()
}

fn estimate_side_panel_render_key_bytes(key: &SidePanelRenderKey) -> usize {
    key.page_id.capacity()
}

pub(crate) fn debug_memory_profile() -> serde_json::Value {
    let (pinned_entries_count, pinned_entries_bytes, pinned_rendered_lines_bytes) = {
        let cache = pinned_cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let entries_bytes = cache
            .entries
            .iter()
            .map(estimate_pinned_content_entry_bytes)
            .sum::<usize>()
            + cache.entries.capacity() * std::mem::size_of::<PinnedContentEntry>();
        let rendered_lines_bytes = cache
            .rendered_lines
            .as_ref()
            .map(estimate_pinned_rendered_cache_bytes)
            .unwrap_or(0);
        (cache.entries.len(), entries_bytes, rendered_lines_bytes)
    };

    let (markdown_cache_entries_count, markdown_cache_bytes, markdown_cache_key_bytes) =
        with_side_panel_markdown_cache(|cache| {
            let entry_bytes = cache
                .entries
                .values()
                .map(estimate_rendered_side_panel_markdown_bytes)
                .sum::<usize>();
            let key_bytes = cache
                .entries
                .keys()
                .map(estimate_side_panel_markdown_key_bytes)
                .sum::<usize>()
                + cache
                    .order
                    .iter()
                    .map(estimate_side_panel_markdown_key_bytes)
                    .sum::<usize>();
            (cache.entries.len(), entry_bytes, key_bytes)
        });

    let (render_cache_entries_count, render_cache_bytes, render_cache_key_bytes) =
        with_side_panel_render_cache(|cache| {
            let entry_bytes = cache
                .entries
                .values()
                .map(estimate_pinned_rendered_cache_bytes)
                .sum::<usize>();
            let key_bytes = cache
                .entries
                .keys()
                .map(estimate_side_panel_render_key_bytes)
                .sum::<usize>()
                + cache
                    .order
                    .iter()
                    .map(estimate_side_panel_render_key_bytes)
                    .sum::<usize>();
            (cache.entries.len(), entry_bytes, key_bytes)
        });

    serde_json::json!({
        "pinned_cache": {
            "entries_count": pinned_entries_count,
            "entries_bytes": pinned_entries_bytes,
            "rendered_lines_bytes": pinned_rendered_lines_bytes,
        },
        "side_panel_markdown_cache": {
            "entries_count": markdown_cache_entries_count,
            "entries_bytes": markdown_cache_bytes,
            "key_bytes": markdown_cache_key_bytes,
        },
        "side_panel_render_cache": {
            "entries_count": render_cache_entries_count,
            "entries_bytes": render_cache_bytes,
            "key_bytes": render_cache_key_bytes,
        },
        "total_estimate_bytes": pinned_entries_bytes
            + pinned_rendered_lines_bytes
            + markdown_cache_bytes
            + markdown_cache_key_bytes
            + render_cache_bytes
            + render_cache_key_bytes,
    })
}

#[derive(Clone)]
struct PinnedImagePlacement {
    after_text_line: usize,
    hash: u64,
    rows: u16,
    render_mode: SidePanelImageRenderMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidePanelImageRenderMode {
    Fit,
    ScrollableViewport { zoom_percent: u8 },
}

impl SidePanelImageRenderMode {
    fn is_scrollable(self) -> bool {
        matches!(self, Self::ScrollableViewport { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SidePanelImageLayout {
    rows: u16,
    render_mode: SidePanelImageRenderMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FitImageRenderPlan {
    Full {
        area: Rect,
    },
    ClippedViewport {
        area: Rect,
        scroll_y: i32,
        zoom_percent: u8,
    },
}

const SIDE_PANEL_INLINE_IMAGE_MIN_ROWS: u16 = 4;
const SIDE_PANEL_INLINE_IMAGE_MIN_ZOOM_PERCENT: u8 = 70;

type SidePaneSnapshotCache = (
    std::sync::Arc<Vec<String>>,
    std::sync::Arc<Vec<usize>>,
    std::sync::Arc<Vec<String>>,
    std::sync::Arc<Vec<WrappedLineMap>>,
    Vec<u16>,
);

fn build_side_pane_snapshot_cache(
    lines: &[Line<'static>],
    inner_width: u16,
) -> SidePaneSnapshotCache {
    let plain_lines: Vec<String> = lines.iter().map(super::line_plain_text).collect();
    let wrapped_line_map: Vec<WrappedLineMap> = plain_lines
        .iter()
        .enumerate()
        .map(|(raw_line, text)| WrappedLineMap {
            raw_line,
            start_col: 0,
            end_col: unicode_width::UnicodeWidthStr::width(text.as_str()),
        })
        .collect();
    let copy_offsets = vec![0; plain_lines.len()];
    let left_margins = line_left_margins_for_area(lines, inner_width);
    let plain_lines = std::sync::Arc::new(plain_lines);
    (
        plain_lines.clone(),
        std::sync::Arc::new(copy_offsets),
        plain_lines,
        std::sync::Arc::new(wrapped_line_map),
        left_margins,
    )
}
static PINNED_CACHE: OnceLock<Mutex<PinnedCacheState>> = OnceLock::new();
#[cfg(not(test))]
static SIDE_PANEL_MARKDOWN_CACHE: OnceLock<Mutex<SidePanelMarkdownCacheState>> = OnceLock::new();
#[cfg(not(test))]
static SIDE_PANEL_RENDER_CACHE: OnceLock<Mutex<SidePanelRenderCacheState>> = OnceLock::new();
#[cfg(not(test))]
static SIDE_PANEL_DEBUG: OnceLock<Mutex<SidePanelDebugState>> = OnceLock::new();

#[cfg(test)]
thread_local! {
    static TEST_SIDE_PANEL_MARKDOWN_CACHE: RefCell<SidePanelMarkdownCacheState> = RefCell::new(SidePanelMarkdownCacheState::default());
    static TEST_SIDE_PANEL_RENDER_CACHE: RefCell<SidePanelRenderCacheState> = RefCell::new(SidePanelRenderCacheState::default());
    static TEST_SIDE_PANEL_DEBUG: RefCell<SidePanelDebugState> = RefCell::new(SidePanelDebugState::default());
}

const SIDE_PANEL_MARKDOWN_CACHE_LIMIT: usize = 12;
const SIDE_PANEL_RENDER_CACHE_LIMIT: usize = 12;

fn pinned_cache() -> &'static Mutex<PinnedCacheState> {
    PINNED_CACHE.get_or_init(|| Mutex::new(PinnedCacheState::default()))
}

#[cfg(not(test))]
fn side_panel_markdown_cache() -> &'static Mutex<SidePanelMarkdownCacheState> {
    SIDE_PANEL_MARKDOWN_CACHE.get_or_init(|| Mutex::new(SidePanelMarkdownCacheState::default()))
}

#[cfg(not(test))]
fn side_panel_render_cache() -> &'static Mutex<SidePanelRenderCacheState> {
    SIDE_PANEL_RENDER_CACHE.get_or_init(|| Mutex::new(SidePanelRenderCacheState::default()))
}

#[cfg(not(test))]
fn side_panel_debug() -> &'static Mutex<SidePanelDebugState> {
    SIDE_PANEL_DEBUG.get_or_init(|| Mutex::new(SidePanelDebugState::default()))
}

fn with_side_panel_markdown_cache<R>(f: impl FnOnce(&SidePanelMarkdownCacheState) -> R) -> R {
    #[cfg(test)]
    {
        return TEST_SIDE_PANEL_MARKDOWN_CACHE.with(|state| f(&state.borrow()));
    }
    #[cfg(not(test))]
    {
        let state = side_panel_markdown_cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&state)
    }
}

fn with_side_panel_markdown_cache_mut<R>(
    f: impl FnOnce(&mut SidePanelMarkdownCacheState) -> R,
) -> R {
    #[cfg(test)]
    {
        return TEST_SIDE_PANEL_MARKDOWN_CACHE.with(|state| f(&mut state.borrow_mut()));
    }
    #[cfg(not(test))]
    {
        let mut state = side_panel_markdown_cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&mut state)
    }
}

fn with_side_panel_render_cache<R>(f: impl FnOnce(&SidePanelRenderCacheState) -> R) -> R {
    #[cfg(test)]
    {
        return TEST_SIDE_PANEL_RENDER_CACHE.with(|state| f(&state.borrow()));
    }
    #[cfg(not(test))]
    {
        let state = side_panel_render_cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&state)
    }
}

fn with_side_panel_render_cache_mut<R>(f: impl FnOnce(&mut SidePanelRenderCacheState) -> R) -> R {
    #[cfg(test)]
    {
        return TEST_SIDE_PANEL_RENDER_CACHE.with(|state| f(&mut state.borrow_mut()));
    }
    #[cfg(not(test))]
    {
        let mut state = side_panel_render_cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&mut state)
    }
}

fn with_side_panel_debug<R>(f: impl FnOnce(&SidePanelDebugState) -> R) -> R {
    #[cfg(test)]
    {
        return TEST_SIDE_PANEL_DEBUG.with(|state| f(&state.borrow()));
    }
    #[cfg(not(test))]
    {
        let state = side_panel_debug()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&state)
    }
}

fn with_side_panel_debug_mut<R>(f: impl FnOnce(&mut SidePanelDebugState) -> R) -> R {
    #[cfg(test)]
    {
        return TEST_SIDE_PANEL_DEBUG.with(|state| f(&mut state.borrow_mut()));
    }
    #[cfg(not(test))]
    {
        let mut state = side_panel_debug()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&mut state)
    }
}

pub(crate) fn side_panel_debug_stats() -> SidePanelDebugStats {
    let mut stats = with_side_panel_debug(|state| state.stats.clone());
    stats.markdown_cache_entries = with_side_panel_markdown_cache(|cache| cache.entries.len());
    stats.render_cache_entries = with_side_panel_render_cache(|cache| cache.entries.len());
    stats
}

pub(crate) fn side_panel_debug_json() -> Option<serde_json::Value> {
    let stats = side_panel_debug_stats();
    let live_snapshot = with_side_panel_debug(|state| state.live_snapshot.clone());
    serde_json::to_value(serde_json::json!({
        "stats": stats,
        "live": live_snapshot,
    }))
    .ok()
}

pub(crate) fn clear_side_panel_debug_snapshot() {
    with_side_panel_debug_mut(|debug| {
        debug.live_snapshot = None;
    });
}

pub(crate) fn reset_side_panel_debug_stats() {
    with_side_panel_debug_mut(|debug| {
        debug.stats = SidePanelDebugStats::default();
        debug.live_snapshot = None;
    });
}

pub(crate) fn clear_side_panel_render_caches() {
    with_side_panel_markdown_cache_mut(|cache| {
        *cache = SidePanelMarkdownCacheState::default();
    });
    with_side_panel_render_cache_mut(|cache| {
        *cache = SidePanelRenderCacheState::default();
    });
}

pub(crate) fn prewarm_focused_side_panel(
    snapshot: &crate::side_panel::SidePanelSnapshot,
    terminal_width: u16,
    terminal_height: u16,
    ratio_percent: u8,
    has_protocol: bool,
    centered: bool,
) -> bool {
    let Some(page) = snapshot.focused_page() else {
        return false;
    };
    let Some(area) = estimate_side_panel_pane_area(terminal_width, terminal_height, ratio_percent)
    else {
        return false;
    };
    let Some(inner) = side_panel_content_area(area) else {
        return false;
    };
    if inner.width == 0 || inner.height == 0 {
        return false;
    }
    let _ = render_side_panel_markdown_cached(page, inner, has_protocol, centered);
    true
}

pub(super) fn collect_pinned_content_cached(
    messages: &[DisplayMessage],
    images: &[crate::session::RenderedImage],
    collect_diffs: bool,
    collect_images: bool,
    messages_version: u64,
) -> bool {
    let key = PinnedCacheKey {
        messages_version,
        collect_diffs,
        collect_images,
    };

    let mut cache = match pinned_cache().lock() {
        Ok(c) => c,
        Err(poisoned) => poisoned.into_inner(),
    };

    if cache.key.as_ref() == Some(&key) {
        return !cache.entries.is_empty();
    }

    let entries = collect_pinned_content(messages, images, collect_diffs, collect_images);
    let has_entries = !entries.is_empty();
    cache.key = Some(key);
    cache.entries = entries;
    cache.rendered_lines = None;
    has_entries
}

fn collect_pinned_content(
    messages: &[DisplayMessage],
    images: &[crate::session::RenderedImage],
    collect_diffs: bool,
    collect_images: bool,
) -> Vec<PinnedContentEntry> {
    let mut entries = Vec::new();

    if collect_images {
        let mut user_entries = Vec::new();
        let mut tool_entries = Vec::new();
        let mut other_entries = Vec::new();
        for image in images {
            let Some((hash, width, height)) =
                mermaid::register_inline_image(&image.media_type, &image.data)
            else {
                continue;
            };

            let entry = PinnedContentEntry::Image {
                label: image
                    .label
                    .clone()
                    .unwrap_or_else(|| image.media_type.clone()),
                media_type: image.media_type.clone(),
                source: image.source.clone(),
                hash,
                width,
                height,
            };

            match &image.source {
                crate::session::RenderedImageSource::UserInput => user_entries.push(entry),
                crate::session::RenderedImageSource::ToolResult { .. } => tool_entries.push(entry),
                crate::session::RenderedImageSource::Other { .. } => other_entries.push(entry),
            }
        }

        entries.extend(user_entries);
        entries.extend(tool_entries);
        entries.extend(other_entries);
    }

    for msg in messages {
        if msg.role != "tool" {
            continue;
        }
        let Some(ref tc) = msg.tool_data else {
            continue;
        };

        if !collect_diffs {
            continue;
        }

        if !tools_ui::is_edit_tool_name(&tc.name) {
            continue;
        }

        let file_path = tc
            .input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| {
                tc.input
                    .get("patch_text")
                    .and_then(|v| v.as_str())
                    .and_then(|patch_text| match tools_ui::canonical_tool_name(&tc.name) {
                        "apply_patch" => tools_ui::extract_apply_patch_primary_file(patch_text),
                        "patch" => tools_ui::extract_unified_patch_primary_file(patch_text),
                        _ => None,
                    })
            })
            .unwrap_or_else(|| "unknown".to_string());

        let change_lines = {
            let from_content = collect_diff_lines(&msg.content);
            if !from_content.is_empty() {
                from_content
            } else {
                generate_diff_lines_from_tool_input(tc)
            }
        };
        if change_lines.is_empty() {
            continue;
        }

        let additions = change_lines
            .iter()
            .filter(|l| l.kind == DiffLineKind::Add)
            .count();
        let deletions = change_lines
            .iter()
            .filter(|l| l.kind == DiffLineKind::Del)
            .count();

        entries.push(PinnedContentEntry::Diff {
            file_path,
            lines: change_lines,
            additions,
            deletions,
        });
    }
    entries
}

pub(super) fn draw_pinned_content_cached(
    frame: &mut Frame,
    area: Rect,
    app: &dyn TuiState,
    scroll: usize,
    line_wrap: bool,
    focused: bool,
) {
    use ratatui::widgets::{Paragraph, Wrap};

    if area.width < 10 || area.height < 3 {
        return;
    }

    let mut cache = match pinned_cache().lock() {
        Ok(c) => c,
        Err(poisoned) => poisoned.into_inner(),
    };

    if cache.entries.is_empty() {
        return;
    }

    let entries = &cache.entries;
    let total_diffs = entries
        .iter()
        .filter(|e| matches!(e, PinnedContentEntry::Diff { .. }))
        .count();
    let total_images = entries
        .iter()
        .filter(|e| matches!(e, PinnedContentEntry::Image { .. }))
        .count();
    let total_additions: usize = entries
        .iter()
        .map(|e| match e {
            PinnedContentEntry::Diff { additions, .. } => *additions,
            _ => 0,
        })
        .sum();
    let total_deletions: usize = entries
        .iter()
        .map(|e| match e {
            PinnedContentEntry::Diff { deletions, .. } => *deletions,
            _ => 0,
        })
        .sum();

    let mut title_parts = vec![Span::styled(" side ", Style::default().fg(tool_color()))];
    title_parts.push(Span::styled(
        "Pinned",
        Style::default()
            .fg(rgb(180, 200, 255))
            .add_modifier(ratatui::style::Modifier::BOLD),
    ));
    title_parts.push(Span::styled(" ", Style::default().fg(dim_color())));
    if total_diffs > 0 {
        title_parts.push(Span::styled(
            format!("+{}", total_additions),
            Style::default().fg(diff_add_color()),
        ));
        title_parts.push(Span::styled(" ", Style::default().fg(dim_color())));
        title_parts.push(Span::styled(
            format!("-{}", total_deletions),
            Style::default().fg(diff_del_color()),
        ));
        title_parts.push(Span::styled(
            format!(" {}f", total_diffs),
            Style::default().fg(dim_color()),
        ));
    }
    if total_images > 0 {
        if total_diffs > 0 {
            title_parts.push(Span::styled(" ", Style::default().fg(dim_color())));
        }
        title_parts.push(Span::styled(
            format!("📷{}", total_images),
            Style::default().fg(dim_color()),
        ));
    }
    let border_style = side_panel_border_style(focused);
    let Some(inner) =
        super::draw_right_rail_chrome(frame, area, Line::from(title_parts), border_style)
    else {
        return;
    };

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let needs_rebuild = match &cache.rendered_lines {
        Some(rendered) => rendered.inner_width != inner.width || rendered.line_wrap != line_wrap,
        None => true,
    };

    if needs_rebuild {
        let has_protocol = mermaid::protocol_type().is_some();
        let mut text_lines: Vec<Line<'static>> = Vec::new();
        let mut image_placements: Vec<PinnedImagePlacement> = Vec::new();
        let mut last_image_group: Option<ImageGroup> = None;

        for (i, entry) in entries.iter().enumerate() {
            if i > 0 {
                text_lines.push(Line::from(""));
            }

            match entry {
                PinnedContentEntry::Diff {
                    file_path,
                    lines: diff_lines,
                    additions,
                    deletions,
                } => {
                    let short_path = file_path
                        .rsplit('/')
                        .take(2)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect::<Vec<_>>()
                        .join("/");

                    let file_ext = std::path::Path::new(file_path)
                        .extension()
                        .and_then(|e| e.to_str());

                    text_lines.push(Line::from(vec![
                        Span::styled("── ", Style::default().fg(dim_color())),
                        Span::styled(
                            short_path,
                            Style::default()
                                .fg(rgb(180, 200, 255))
                                .add_modifier(ratatui::style::Modifier::BOLD),
                        ),
                        Span::styled(" (", Style::default().fg(dim_color())),
                        Span::styled(
                            format!("+{}", additions),
                            Style::default().fg(diff_add_color()),
                        ),
                        Span::styled(" ", Style::default().fg(dim_color())),
                        Span::styled(
                            format!("-{}", deletions),
                            Style::default().fg(diff_del_color()),
                        ),
                        Span::styled(")", Style::default().fg(dim_color())),
                    ]));

                    for line in diff_lines {
                        let base_color = if line.kind == DiffLineKind::Add {
                            diff_add_color()
                        } else {
                            diff_del_color()
                        };

                        let mut spans: Vec<Span<'static>> = vec![Span::styled(
                            line.prefix.clone(),
                            Style::default().fg(base_color),
                        )];

                        if !line.content.is_empty() {
                            let highlighted =
                                markdown::highlight_line(line.content.as_str(), file_ext);
                            for span in highlighted {
                                let tinted = tint_span_with_diff_color(span, base_color);
                                spans.push(tinted);
                            }
                        }

                        text_lines.push(Line::from(spans));
                    }
                }
                PinnedContentEntry::Image {
                    label,
                    media_type,
                    source,
                    hash,
                    width: img_w,
                    height: img_h,
                } => {
                    let group = image_group_for(source);
                    if last_image_group != Some(group) {
                        let (group_label, group_color) = image_group_heading(group);
                        text_lines.push(Line::from(vec![
                            Span::styled("   ", Style::default().fg(dim_color())),
                            Span::styled(
                                group_label.to_uppercase(),
                                Style::default()
                                    .fg(group_color)
                                    .add_modifier(ratatui::style::Modifier::BOLD),
                            ),
                        ]));
                        last_image_group = Some(group);
                    }

                    let short_label = compact_image_label(label);
                    let source_badge = image_source_badge(source);

                    text_lines.push(Line::from(vec![
                        Span::styled("── 📷 ", Style::default().fg(dim_color())),
                        Span::styled(
                            short_label,
                            Style::default()
                                .fg(rgb(180, 200, 255))
                                .add_modifier(ratatui::style::Modifier::BOLD),
                        ),
                        Span::styled(
                            format!(" {}×{}", img_w, img_h),
                            Style::default().fg(dim_color()),
                        ),
                        Span::styled(
                            format!(" [{}]", source_badge),
                            Style::default().fg(match group {
                                ImageGroup::Inputs => rgb(138, 180, 248),
                                ImageGroup::Tools => accent_color(),
                                ImageGroup::Other => dim_color(),
                            }),
                        ),
                    ]));
                    text_lines.push(Line::from(vec![
                        Span::styled("   ", Style::default().fg(dim_color())),
                        Span::styled(media_type.clone(), Style::default().fg(dim_color())),
                        Span::styled(" • exact model artifact", Style::default().fg(dim_color())),
                    ]));

                    if has_protocol {
                        let img_rows =
                            estimate_inline_image_rows(*img_w, *img_h, inner.width, inner.height);
                        image_placements.push(PinnedImagePlacement {
                            after_text_line: text_lines.len(),
                            hash: *hash,
                            rows: img_rows,
                            render_mode: SidePanelImageRenderMode::Fit,
                        });
                        for _ in 0..img_rows {
                            text_lines.push(Line::from(""));
                        }
                    }
                }
            }
        }

        if text_lines.is_empty() {
            text_lines.push(Line::from(Span::styled(
                "No content yet",
                Style::default().fg(dim_color()),
            )));
        }

        let (
            wrapped_plain_lines,
            wrapped_copy_offsets,
            raw_plain_lines,
            wrapped_line_map,
            left_margins,
        ) = build_side_pane_snapshot_cache(&text_lines, inner.width);

        cache.rendered_lines = Some(PinnedRenderedCache {
            inner_width: inner.width,
            line_wrap,
            lines: text_lines,
            wrapped_plain_lines,
            wrapped_copy_offsets,
            raw_plain_lines,
            wrapped_line_map,
            left_margins,
            image_placements,
            has_scrollable_images: false,
        });
    }

    let Some(rendered) = cache.rendered_lines.as_ref() else {
        return;
    };
    let total_lines = rendered.lines.len();
    super::set_pinned_pane_total_lines(total_lines);

    let max_scroll = total_lines.saturating_sub(inner.height as usize);
    let clamped_scroll = scroll.min(max_scroll);
    super::set_last_diff_pane_effective_scroll(clamped_scroll);

    let mut visible_lines: Vec<Line<'static>> = rendered
        .lines
        .iter()
        .skip(clamped_scroll)
        .take(inner.height as usize)
        .cloned()
        .collect();
    let visible_end = clamped_scroll + visible_lines.len();
    let visible_left_margins = rendered
        .left_margins
        .get(clamped_scroll..visible_end.min(rendered.left_margins.len()))
        .unwrap_or(&[]);
    record_side_pane_snapshot_precomputed(
        rendered.wrapped_plain_lines.clone(),
        rendered.wrapped_copy_offsets.clone(),
        rendered.raw_plain_lines.clone(),
        rendered.wrapped_line_map.clone(),
        clamped_scroll,
        visible_end,
        inner,
        visible_left_margins,
    );
    apply_side_selection_highlight(app, &mut visible_lines, clamped_scroll);
    super::clear_area(frame, inner);

    let paragraph = if line_wrap {
        Paragraph::new(visible_lines).wrap(Wrap { trim: false })
    } else {
        Paragraph::new(visible_lines)
    };
    frame.render_widget(paragraph, inner);

    let has_protocol = mermaid::protocol_type().is_some();
    if has_protocol {
        for placement in &rendered.image_placements {
            let image_start = placement.after_text_line;
            let image_end = image_start.saturating_add(placement.rows as usize);
            let viewport_start = clamped_scroll;
            let viewport_end = clamped_scroll.saturating_add(inner.height as usize);
            if image_end <= viewport_start || image_start >= viewport_end {
                continue;
            }

            let visible_start = image_start.max(viewport_start);
            let visible_end = image_end.min(viewport_end);
            let y_in_inner = visible_start.saturating_sub(viewport_start) as u16;
            let avail_rows = visible_end.saturating_sub(visible_start) as u16;
            if avail_rows < 2 {
                continue;
            }
            let img_area = Rect {
                x: inner.x,
                y: inner.y + y_in_inner,
                width: inner.width,
                height: avail_rows,
            };
            if let Some((_, width, height)) = mermaid::get_cached_png(placement.hash) {
                if let Some(plan) = plan_fit_image_render(
                    inner,
                    clamped_scroll,
                    image_start,
                    placement.rows,
                    width,
                    height,
                    false,
                ) {
                    match plan {
                        FitImageRenderPlan::Full { area } => {
                            mermaid::render_image_widget_fit(
                                placement.hash,
                                area,
                                frame.buffer_mut(),
                                false,
                                false,
                            );
                        }
                        FitImageRenderPlan::ClippedViewport {
                            area,
                            scroll_y,
                            zoom_percent,
                        } => {
                            mermaid::render_image_widget_viewport(
                                placement.hash,
                                area,
                                frame.buffer_mut(),
                                0,
                                scroll_y,
                                zoom_percent,
                                false,
                            );
                        }
                    }
                }
            } else {
                mermaid::render_image_widget_fit(
                    placement.hash,
                    img_area,
                    frame.buffer_mut(),
                    false,
                    false,
                );
            }
        }
    }
}

pub(super) fn draw_side_panel_markdown(
    frame: &mut Frame,
    area: Rect,
    app: &dyn TuiState,
    snapshot: &crate::side_panel::SidePanelSnapshot,
    scroll: usize,
    focused: bool,
    centered: bool,
) {
    if area.width < 10 || area.height < 3 {
        return;
    }

    let Some(page) = snapshot.focused_page() else {
        return;
    };

    let page_index = snapshot
        .pages
        .iter()
        .position(|candidate| candidate.id == page.id)
        .map(|idx| idx + 1)
        .unwrap_or(1);
    let page_count = snapshot.pages.len();

    let border_style = side_panel_border_style(focused);
    let Some(content_shell_area) = side_panel_content_area(area) else {
        return;
    };
    let has_protocol = mermaid::protocol_type().is_some();
    let rendered_full_width =
        render_side_panel_markdown_cached(page, content_shell_area, has_protocol, centered);

    let mut title_parts = vec![Span::styled(" side ", Style::default().fg(tool_color()))];
    title_parts.push(Span::styled(
        page.title.clone(),
        Style::default()
            .fg(rgb(180, 200, 255))
            .add_modifier(ratatui::style::Modifier::BOLD),
    ));
    title_parts.push(Span::styled(
        format!(" {}/{} ", page_index, page_count),
        Style::default().fg(dim_color()),
    ));
    if rendered_full_width.has_scrollable_images {
        title_parts.push(Span::styled(
            " readable ",
            Style::default()
                .fg(accent_color())
                .add_modifier(ratatui::style::Modifier::BOLD),
        ));
        title_parts.push(Span::styled(" scroll ", Style::default().fg(dim_color())));
        if focused {
            title_parts.push(Span::styled(" h/l pan ", Style::default().fg(dim_color())));
        }
    }

    let Some(content_shell_area) =
        super::draw_right_rail_chrome(frame, area, Line::from(title_parts), border_style)
    else {
        return;
    };
    let show_native_scrollbar = super::native_scrollbar_visible(
        app.side_panel_native_scrollbar() && content_shell_area.width > 1,
        rendered_full_width.lines.len(),
        content_shell_area.height as usize,
    );
    let (content_inner, scrollbar_area) =
        super::split_native_scrollbar_area(content_shell_area, show_native_scrollbar);
    if content_inner.width == 0 || content_inner.height == 0 {
        return;
    }
    let rendered = if show_native_scrollbar {
        render_side_panel_markdown_cached(page, content_inner, has_protocol, centered)
    } else {
        rendered_full_width
    };

    super::set_pinned_pane_total_lines(rendered.lines.len());
    let max_scroll = rendered
        .lines
        .len()
        .saturating_sub(content_inner.height as usize);
    let clamped_scroll = scroll.min(max_scroll);
    super::set_last_diff_pane_effective_scroll(clamped_scroll);

    let mut visible_lines: Vec<Line<'static>> = rendered
        .lines
        .iter()
        .skip(clamped_scroll)
        .take(content_inner.height as usize)
        .cloned()
        .collect();
    let visible_end = clamped_scroll + visible_lines.len();
    let visible_left_margins = rendered
        .left_margins
        .get(clamped_scroll..visible_end.min(rendered.left_margins.len()))
        .unwrap_or(&[]);
    record_side_pane_snapshot_precomputed(
        rendered.wrapped_plain_lines.clone(),
        rendered.wrapped_copy_offsets.clone(),
        rendered.raw_plain_lines.clone(),
        rendered.wrapped_line_map.clone(),
        clamped_scroll,
        visible_end,
        content_inner,
        visible_left_margins,
    );
    apply_side_selection_highlight(app, &mut visible_lines, clamped_scroll);
    super::clear_area(frame, content_inner);
    frame.render_widget(Paragraph::new(visible_lines), content_inner);

    if let Some(scrollbar_area) = scrollbar_area {
        super::clear_area(frame, scrollbar_area);
        super::render_native_scrollbar(
            frame,
            scrollbar_area,
            clamped_scroll,
            rendered.lines.len(),
            content_inner.height as usize,
            focused,
        );
    }

    let mut visible_mermaids: Vec<SidePanelVisibleMermaidDebug> = Vec::new();
    if has_protocol {
        let font_size_px = mermaid::get_font_size().unwrap_or((8, 16));
        for (image_index, placement) in rendered.image_placements.iter().enumerate() {
            let image_start = placement.after_text_line;
            let image_end = image_start.saturating_add(placement.rows as usize);
            let viewport_start = clamped_scroll;
            let viewport_end = clamped_scroll.saturating_add(content_inner.height as usize);
            if image_end <= viewport_start || image_start >= viewport_end {
                continue;
            }

            let visible_start = image_start.max(viewport_start);
            let visible_end = image_end.min(viewport_end);
            let y_in_inner = visible_start.saturating_sub(viewport_start) as u16;
            let avail_rows = visible_end.saturating_sub(visible_start) as u16;
            if avail_rows < 2 {
                continue;
            }
            let img_area = Rect {
                x: content_inner.x,
                y: content_inner.y + y_in_inner,
                width: content_inner.width,
                height: avail_rows,
            };
            match placement.render_mode {
                SidePanelImageRenderMode::Fit => {
                    if let Some((_, width, height)) = mermaid::get_cached_png(placement.hash) {
                        if let Some(plan) = plan_fit_image_render(
                            content_inner,
                            clamped_scroll,
                            image_start,
                            placement.rows,
                            width,
                            height,
                            centered,
                        ) {
                            let visible_widget_rect = match plan {
                                FitImageRenderPlan::Full { area } => {
                                    mermaid::render_image_widget_scale(
                                        placement.hash,
                                        area,
                                        frame.buffer_mut(),
                                        false,
                                    );
                                    area
                                }
                                FitImageRenderPlan::ClippedViewport {
                                    area,
                                    scroll_y,
                                    zoom_percent,
                                } => {
                                    mermaid::render_image_widget_viewport(
                                        placement.hash,
                                        area,
                                        frame.buffer_mut(),
                                        0,
                                        scroll_y,
                                        zoom_percent,
                                        false,
                                    );
                                    area
                                }
                            };

                            let probe = build_side_panel_mermaid_probe_from_image(
                                width,
                                height,
                                content_inner.width,
                                content_inner.height,
                                font_size_px,
                                centered,
                            );
                            let visible_widget = probe_rect(
                                Rect::new(
                                    0,
                                    0,
                                    visible_widget_rect.width,
                                    visible_widget_rect.height,
                                ),
                                content_inner.width,
                                content_inner.height,
                            );
                            visible_mermaids.push(SidePanelVisibleMermaidDebug {
                                image_index,
                                hash: format!("{:016x}", placement.hash),
                                reserved_rows: placement.rows,
                                visible_rows: avail_rows,
                                render_mode: probe.render_mode.clone(),
                                rendered_png_width_px: width,
                                rendered_png_height_px: height,
                                layout_fit: probe.layout_fit,
                                widget_fit: probe.widget_fit,
                                visible_widget: visible_widget.clone(),
                                log: format!(
                                    "image#{image_index} {} visible={}x{} cells ({:.1}% area)",
                                    probe.render_mode,
                                    visible_widget.width_cells,
                                    visible_widget.height_cells,
                                    visible_widget.area_utilization_percent,
                                ),
                            });
                        }
                    } else {
                        mermaid::render_image_widget_scale(
                            placement.hash,
                            img_area,
                            frame.buffer_mut(),
                            false,
                        );
                    }
                }
                SidePanelImageRenderMode::ScrollableViewport { zoom_percent } => {
                    let scroll_y = visible_start.saturating_sub(image_start) as i32;
                    let side_pane_scroll_x = app.diff_pane_scroll_x();
                    let scroll_x = mermaid::get_cached_png(placement.hash)
                        .map(|(_, width, _)| {
                            side_panel_viewport_scroll_x(
                                width,
                                img_area.width,
                                zoom_percent,
                                centered,
                                mermaid::get_font_size(),
                                side_pane_scroll_x,
                            )
                        })
                        .unwrap_or(0);
                    mermaid::render_image_widget_viewport(
                        placement.hash,
                        img_area,
                        frame.buffer_mut(),
                        scroll_x,
                        scroll_y,
                        zoom_percent,
                        false,
                    );
                    if let Some((_, width, height)) = mermaid::get_cached_png(placement.hash) {
                        let probe = build_side_panel_mermaid_probe_from_image(
                            width,
                            height,
                            content_inner.width,
                            content_inner.height,
                            font_size_px,
                            centered,
                        );
                        let visible_widget = probe_rect(
                            Rect::new(0, 0, img_area.width, img_area.height),
                            content_inner.width,
                            content_inner.height,
                        );
                        visible_mermaids.push(SidePanelVisibleMermaidDebug {
                            image_index,
                            hash: format!("{:016x}", placement.hash),
                            reserved_rows: placement.rows,
                            visible_rows: avail_rows,
                            render_mode: probe.render_mode.clone(),
                            rendered_png_width_px: width,
                            rendered_png_height_px: height,
                            layout_fit: probe.layout_fit,
                            widget_fit: probe.widget_fit,
                            visible_widget: visible_widget.clone(),
                            log: format!(
                                "image#{image_index} {} visible={}x{} cells ({:.1}% area)",
                                probe.render_mode,
                                visible_widget.width_cells,
                                visible_widget.height_cells,
                                visible_widget.area_utilization_percent,
                            ),
                        });
                    }
                }
            }
        }
    }

    with_side_panel_debug_mut(|debug| {
        debug.live_snapshot = Some(SidePanelLiveDebugSnapshot {
            page_id: page.id.clone(),
            page_title: page.title.clone(),
            pane_width_cells: content_inner.width,
            pane_height_cells: content_inner.height,
            total_lines: rendered.lines.len(),
            scroll_offset: clamped_scroll,
            max_scroll,
            total_mermaids: rendered.image_placements.len(),
            visible_mermaids,
        });
    });
}

fn render_side_panel_markdown_cached(
    page: &crate::side_panel::SidePanelPage,
    inner: Rect,
    has_protocol: bool,
    centered: bool,
) -> PinnedRenderedCache {
    let content_signature = side_panel_content_signature(page);
    let key = SidePanelRenderKey {
        page_id: page.id.clone(),
        content_signature,
        inner_width: inner.width,
        inner_height: inner.height,
        has_protocol,
        centered,
        mermaid_epoch: crate::tui::mermaid::deferred_render_epoch(),
    };

    if let Some(rendered) = with_side_panel_render_cache_mut(|cache| {
        let rendered = cache.entries.get(&key).cloned();
        if rendered.is_some() {
            lru_touch(&mut cache.order, &key);
            cache.order.push_back(key.clone());
        }
        rendered
    }) {
        with_side_panel_debug_mut(|debug| {
            debug.stats.render_cache_hits += 1;
        });
        return rendered;
    }
    with_side_panel_debug_mut(|debug| {
        debug.stats.render_cache_misses += 1;
    });

    let rendered_markdown = render_side_panel_markdown_lines_cached(
        page,
        content_signature,
        inner.width,
        has_protocol,
        centered,
    );

    let align = if centered {
        Alignment::Center
    } else {
        Alignment::Left
    };
    let mut text_lines: Vec<Line<'static>> = Vec::new();
    let mut image_placements: Vec<PinnedImagePlacement> = Vec::new();

    for (idx, line) in rendered_markdown.rendered_markdown.iter().enumerate() {
        if let Some(hash) = rendered_markdown.placeholder_hashes[idx] {
            let image_layout = estimate_side_panel_image_layout(
                hash,
                inner,
                text_lines.len(),
                rendered_markdown.has_following_content_after[idx],
            );
            image_placements.push(PinnedImagePlacement {
                after_text_line: text_lines.len(),
                hash,
                rows: image_layout.rows,
                render_mode: image_layout.render_mode,
            });
            for _ in 0..image_layout.rows {
                text_lines.push(Line::from(""));
            }
            continue;
        }
        text_lines.push(align_if_unset(line.clone(), align));
    }

    if centered {
        crate::tui::markdown::recenter_structured_blocks_for_display(
            &mut text_lines,
            inner.width as usize,
        );
    }

    if text_lines.is_empty() {
        text_lines.push(Line::from(Span::styled(
            "No side panel content yet",
            Style::default().fg(dim_color()),
        )));
    }

    let has_scrollable_images = image_placements
        .iter()
        .any(|placement| placement.render_mode.is_scrollable());

    let (
        wrapped_plain_lines,
        wrapped_copy_offsets,
        raw_plain_lines,
        wrapped_line_map,
        left_margins,
    ) = build_side_pane_snapshot_cache(&text_lines, inner.width);

    let rendered = PinnedRenderedCache {
        inner_width: inner.width,
        line_wrap: false,
        lines: text_lines,
        wrapped_plain_lines,
        wrapped_copy_offsets,
        raw_plain_lines,
        wrapped_line_map,
        left_margins,
        image_placements,
        has_scrollable_images,
    };

    with_side_panel_render_cache_mut(|cache| {
        lru_touch(&mut cache.order, &key);
        cache.entries.insert(key.clone(), rendered.clone());
        cache.order.push_back(key);
        while cache.order.len() > SIDE_PANEL_RENDER_CACHE_LIMIT {
            if let Some(oldest) = cache.order.pop_front() {
                cache.entries.remove(&oldest);
            }
        }
    });

    rendered
}

fn render_side_panel_markdown_lines_cached(
    page: &crate::side_panel::SidePanelPage,
    content_signature: u64,
    inner_width: u16,
    has_protocol: bool,
    centered: bool,
) -> RenderedSidePanelMarkdown {
    let key = SidePanelMarkdownKey {
        page_id: page.id.clone(),
        content_signature,
        inner_width,
        has_protocol,
        centered,
        mermaid_epoch: crate::tui::mermaid::deferred_render_epoch(),
    };

    if let Some(rendered) = with_side_panel_markdown_cache_mut(|cache| {
        let rendered = cache.entries.get(&key).cloned();
        if rendered.is_some() {
            lru_touch(&mut cache.order, &key);
            cache.order.push_back(key.clone());
        }
        rendered
    }) {
        with_side_panel_debug_mut(|debug| {
            debug.stats.markdown_cache_hits += 1;
        });
        return rendered;
    }
    with_side_panel_debug_mut(|debug| {
        debug.stats.markdown_cache_misses += 1;
    });

    let saved_override = markdown::get_diagram_mode_override();
    let saved_centered = markdown::center_code_blocks();
    markdown::set_diagram_mode_override(Some(crate::config::DiagramDisplayMode::None));
    markdown::set_center_code_blocks(centered);
    let lines = wrap_side_panel_markdown_lines(
        markdown::render_markdown_with_width(&page.content, Some(inner_width as usize)),
        inner_width as usize,
    );
    markdown::set_center_code_blocks(saved_centered);
    markdown::set_diagram_mode_override(saved_override);

    let placeholder_hashes: Vec<Option<u64>> = if has_protocol {
        lines.iter().map(mermaid::parse_image_placeholder).collect()
    } else {
        vec![None; lines.len()]
    };
    let mut has_following_content_after = vec![false; lines.len()];
    let mut seen_non_image_content = false;
    for idx in (0..lines.len()).rev() {
        has_following_content_after[idx] = seen_non_image_content;
        if placeholder_hashes[idx].is_none() && lines[idx].width() > 0 {
            seen_non_image_content = true;
        }
    }

    let rendered = RenderedSidePanelMarkdown {
        rendered_markdown: lines,
        placeholder_hashes,
        has_following_content_after,
    };

    with_side_panel_markdown_cache_mut(|cache| {
        lru_touch(&mut cache.order, &key);
        cache.entries.insert(key.clone(), rendered.clone());
        cache.order.push_back(key);
        while cache.order.len() > SIDE_PANEL_MARKDOWN_CACHE_LIMIT {
            if let Some(oldest) = cache.order.pop_front() {
                cache.entries.remove(&oldest);
            }
        }
    });

    rendered
}

fn wrap_side_panel_markdown_lines(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .flat_map(|line| {
            if is_rendered_table_line(&line) {
                vec![line]
            } else {
                markdown::wrap_line(line, width)
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "ui_pinned_tests.rs"]
mod tests;
