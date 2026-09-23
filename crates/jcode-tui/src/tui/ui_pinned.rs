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
    fit_image_area_with_font, plan_fit_image_render, scaled_image_rows,
    side_panel_viewport_scroll_x,
};
#[cfg(test)]
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use util_support::{estimate_side_panel_pane_area, lru_touch, side_panel_content_signature};

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

fn side_panel_content_may_contain_mermaid(content: &str) -> bool {
    content.lines().any(|line| {
        line.trim_start()
            .strip_prefix("```")
            .map(|lang| mermaid::is_mermaid_lang(lang.trim()))
            .unwrap_or(false)
    })
}

fn side_panel_mermaid_preferred_aspect_ratio(
    page: &crate::side_panel::SidePanelPage,
    inner: Rect,
    has_protocol: bool,
) -> Option<f32> {
    if !has_protocol || !side_panel_content_may_contain_mermaid(&page.content) {
        return None;
    }
    super::diagram_pane::content_area_preferred_aspect_ratio(inner)
}

fn side_panel_mermaid_profile_area(inner: Rect, reserve_native_scrollbar: bool) -> Rect {
    if reserve_native_scrollbar && inner.width > 1 {
        Rect {
            width: inner.width - 1,
            ..inner
        }
    } else {
        inner
    }
}

#[path = "ui_pinned_selection.rs"]
mod selection_support;
use selection_support::apply_side_selection_highlight;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SidePanelMarkdownKey {
    page_id: String,
    content_signature: u64,
    inner_width: u16,
    has_protocol: bool,
    centered: bool,
    mermaid_epoch: u64,
    mermaid_aspect_bucket: Option<u16>,
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
    image_zoom_percent: u8,
    mermaid_epoch: u64,
    mermaid_aspect_bucket: Option<u16>,
}

#[derive(Default)]
struct SidePanelRenderCacheState {
    entries: HashMap<SidePanelRenderKey, RenderedSidePanelCache>,
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
struct RenderedSidePanelCache {
    lines: Vec<Line<'static>>,
    wrapped_plain_lines: std::sync::Arc<Vec<String>>,
    wrapped_copy_offsets: std::sync::Arc<Vec<usize>>,
    raw_plain_lines: std::sync::Arc<Vec<String>>,
    wrapped_line_map: std::sync::Arc<Vec<WrappedLineMap>>,
    left_margins: Vec<u16>,
    image_placements: Vec<SidePanelImagePlacement>,
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

fn estimate_side_panel_rendered_cache_bytes(cache: &RenderedSidePanelCache) -> usize {
    estimate_lines_bytes(&cache.lines)
        + estimate_arc_string_vec_bytes(&cache.wrapped_plain_lines)
        + estimate_arc_usize_vec_bytes(&cache.wrapped_copy_offsets)
        + estimate_arc_string_vec_bytes(&cache.raw_plain_lines)
        + estimate_arc_wrapped_line_map_bytes(&cache.wrapped_line_map)
        + cache.left_margins.capacity() * std::mem::size_of::<u16>()
        + cache.image_placements.capacity() * std::mem::size_of::<SidePanelImagePlacement>()
}

fn estimate_rendered_side_panel_markdown_bytes(value: &RenderedSidePanelMarkdown) -> usize {
    estimate_lines_bytes(&value.rendered_markdown)
        + value.placeholder_hashes.capacity() * std::mem::size_of::<Option<u64>>()
        + value.has_following_content_after.capacity() * std::mem::size_of::<bool>()
}

fn estimate_side_panel_markdown_key_bytes(key: &SidePanelMarkdownKey) -> usize {
    key.page_id.capacity()
}

fn estimate_side_panel_render_key_bytes(key: &SidePanelRenderKey) -> usize {
    key.page_id.capacity()
}

pub(crate) fn debug_memory_profile() -> serde_json::Value {
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
                .map(estimate_side_panel_rendered_cache_bytes)
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
        "total_estimate_bytes": markdown_cache_bytes
            + markdown_cache_key_bytes
            + render_cache_bytes
            + render_cache_key_bytes,
    })
}

#[derive(Clone)]
struct SidePanelImagePlacement {
    after_text_line: usize,
    hash: u64,
    rows: u16,
    render_mode: SidePanelImageRenderMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidePanelImageRenderMode {
    Fit,
    ScrollableViewport { zoom_percent: u16 },
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
const SIDE_PANEL_INLINE_IMAGE_MIN_ZOOM_PERCENT: u16 = 70;

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
    let image_zoom_percent = app.side_panel_image_zoom_percent();
    // The first render measures whether a native scrollbar is needed. When one
    // is enabled, use the eventual one-column-narrower content area for Mermaid's
    // aspect profile on that measurement pass too. Otherwise each diagram queues
    // one cold render at full width and another after the scrollbar is reserved.
    let reserve_native_scrollbar =
        app.side_panel_native_scrollbar() && content_shell_area.width > 1;
    let mermaid_profile_area =
        side_panel_mermaid_profile_area(content_shell_area, reserve_native_scrollbar);
    let rendered_full_width = render_side_panel_markdown_cached_with_zoom_and_profile_area(
        page,
        content_shell_area,
        mermaid_profile_area,
        has_protocol,
        centered,
        image_zoom_percent,
    );

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
    title_parts.push(Span::styled(
        format!(
            " {} hide ",
            crate::tui::keybind::side_panel_toggle_key_label()
        ),
        Style::default().fg(dim_color()),
    ));
    if focused {
        title_parts.push(Span::styled(
            " j/k scroll ",
            Style::default().fg(dim_color()),
        ));
        if page_count > 1 {
            title_parts.push(Span::styled(
                " Tab/Shift-Tab pages ",
                Style::default().fg(dim_color()),
            ));
        }
        title_parts.push(Span::styled(
            " Esc focus chat ",
            Style::default().fg(dim_color()),
        ));
    }
    if rendered_full_width.has_scrollable_images {
        title_parts.push(Span::styled(
            " readable ",
            Style::default()
                .fg(accent_color())
                .add_modifier(ratatui::style::Modifier::BOLD),
        ));
        title_parts.push(Span::styled(" scroll ", Style::default().fg(dim_color())));
        if focused {
            title_parts.push(Span::styled(
                " h/l pan +/- zoom ",
                Style::default().fg(dim_color()),
            ));
        }
    }
    if image_zoom_percent != 100 {
        title_parts.push(Span::styled(
            format!(" zoom {}% ", image_zoom_percent),
            Style::default().fg(accent_color()),
        ));
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
        render_side_panel_markdown_cached_with_zoom(
            page,
            content_inner,
            has_protocol,
            centered,
            image_zoom_percent,
        )
    } else {
        rendered_full_width
    };

    super::set_pinned_pane_total_lines(rendered.lines.len());
    let max_scroll = rendered
        .lines
        .len()
        .saturating_sub(content_inner.height as usize);
    super::set_last_diff_pane_max_scroll(max_scroll);
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
        let mermaid_aspect_ratio =
            side_panel_mermaid_preferred_aspect_ratio(page, content_inner, true);
        mermaid::with_preferred_aspect_ratio(mermaid_aspect_ratio, || {
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
                                        super::panel_image_preview::record_image(
                                            area,
                                            placement.hash,
                                        );
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
                                        super::panel_image_preview::record_image(
                                            area,
                                            placement.hash,
                                        );
                                        mermaid::render_image_widget_viewport_precise(
                                            placement.hash,
                                            area,
                                            frame.buffer_mut(),
                                            0,
                                            scroll_y,
                                            zoom_percent as u16,
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
                            super::panel_image_preview::record_image(img_area, placement.hash);
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
                        super::panel_image_preview::record_image(img_area, placement.hash);
                        mermaid::render_image_widget_viewport_precise(
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
        });
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
) -> RenderedSidePanelCache {
    render_side_panel_markdown_cached_with_zoom(page, inner, has_protocol, centered, 100)
}

fn render_side_panel_markdown_cached_with_zoom(
    page: &crate::side_panel::SidePanelPage,
    inner: Rect,
    has_protocol: bool,
    centered: bool,
    image_zoom_percent: u8,
) -> RenderedSidePanelCache {
    render_side_panel_markdown_cached_with_zoom_and_profile_area(
        page,
        inner,
        inner,
        has_protocol,
        centered,
        image_zoom_percent,
    )
}

fn render_side_panel_markdown_cached_with_zoom_and_profile_area(
    page: &crate::side_panel::SidePanelPage,
    inner: Rect,
    mermaid_profile_area: Rect,
    has_protocol: bool,
    centered: bool,
    image_zoom_percent: u8,
) -> RenderedSidePanelCache {
    let content_signature = side_panel_content_signature(page);
    let mermaid_aspect_ratio =
        side_panel_mermaid_preferred_aspect_ratio(page, mermaid_profile_area, has_protocol);
    let mermaid_aspect_bucket = mermaid::preferred_aspect_ratio_bucket(mermaid_aspect_ratio);
    let key = SidePanelRenderKey {
        page_id: page.id.clone(),
        content_signature,
        inner_width: inner.width,
        inner_height: inner.height,
        has_protocol,
        centered,
        image_zoom_percent,
        mermaid_epoch: crate::tui::mermaid::deferred_render_epoch(),
        mermaid_aspect_bucket,
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
        mermaid_aspect_ratio,
        mermaid_aspect_bucket,
    );

    let align = if centered {
        Alignment::Center
    } else {
        Alignment::Left
    };
    let mut text_lines: Vec<Line<'static>> = Vec::new();
    let mut image_placements: Vec<SidePanelImagePlacement> = Vec::new();

    for (idx, line) in rendered_markdown.rendered_markdown.iter().enumerate() {
        if let Some(hash) = rendered_markdown.placeholder_hashes[idx] {
            let mut image_layout = estimate_side_panel_image_layout(
                hash,
                inner,
                text_lines.len(),
                rendered_markdown.has_following_content_after[idx],
            );
            if image_zoom_percent != 100
                && let Some((_, _, height)) = mermaid::get_cached_png(hash)
            {
                let (_, cell_h) = mermaid::get_font_size().unwrap_or((8, 16));
                let image_h_cells =
                    super::diagram_pane::div_ceil_u32(height.max(1), cell_h.max(1) as u32).max(1);
                let rows = scaled_image_rows(image_h_cells, image_zoom_percent as u16)
                    .max(SIDE_PANEL_INLINE_IMAGE_MIN_ROWS);
                image_layout = SidePanelImageLayout {
                    rows,
                    render_mode: SidePanelImageRenderMode::ScrollableViewport {
                        zoom_percent: image_zoom_percent as u16,
                    },
                };
            }
            image_placements.push(SidePanelImagePlacement {
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

    let rendered = RenderedSidePanelCache {
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
    mermaid_aspect_ratio: Option<f32>,
    mermaid_aspect_bucket: Option<u16>,
) -> RenderedSidePanelMarkdown {
    let key = SidePanelMarkdownKey {
        page_id: page.id.clone(),
        content_signature,
        inner_width,
        has_protocol,
        centered,
        mermaid_epoch: crate::tui::mermaid::deferred_render_epoch(),
        mermaid_aspect_bucket,
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

    let saved_centered = markdown::center_code_blocks();
    markdown::set_center_code_blocks(centered);
    // Pin the diagram mode for this render only (thread-local scope): the
    // side panel always renders diagrams inline. Using the process-global
    // override here would race concurrent renders/tests that read or set it.
    let rendered_lines =
        markdown::with_diagram_mode_scope(crate::config::DiagramDisplayMode::None, || {
            mermaid::with_preferred_aspect_ratio(mermaid_aspect_ratio, || {
                markdown::render_markdown_with_width(&page.content, Some(inner_width as usize))
            })
        });
    let rendered_lines = if has_protocol {
        rendered_lines
            .into_iter()
            .map(|line| markdown_image_line_to_placeholder(page, line).unwrap_or_else(|line| line))
            .collect()
    } else {
        rendered_lines
    };
    let lines = wrap_side_panel_markdown_lines(rendered_lines, inner_width as usize);
    markdown::set_center_code_blocks(saved_centered);

    let placeholder_hashes: Vec<Option<u64>> = if has_protocol {
        lines
            .iter()
            .map(|line| {
                mermaid::parse_image_placeholder(line).or_else(|| {
                    // Mermaid diagrams now emit inline-fit markers (same as
                    // raster images); the side panel draws them through its
                    // own placement machinery, keyed by hash.
                    mermaid::parse_inline_image_placeholder(line).map(|(hash, _, _)| hash)
                })
            })
            .collect()
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
            if is_rendered_table_line(&line)
                || mermaid::parse_image_placeholder(&line).is_some()
                || mermaid::parse_inline_image_placeholder(&line).is_some()
            {
                vec![line]
            } else {
                markdown::wrap_line(line, width)
            }
        })
        .collect()
}

fn markdown_image_line_to_placeholder(
    page: &crate::side_panel::SidePanelPage,
    line: Line<'static>,
) -> Result<Line<'static>, Line<'static>> {
    let text = super::line_plain_text(&line);
    let Some(path_text) = parse_rendered_markdown_image_path(&text) else {
        return Err(line);
    };
    let path = resolve_side_panel_image_path(page, path_text);
    let Ok((width, height)) = ::image::image_dimensions(&path) else {
        return Err(line);
    };

    let hash = mermaid::register_external_image(&path, width, height);
    let marker = mermaid::image_widget_placeholder_markdown(hash)
        .trim_end()
        .to_string();
    Ok(Line::from(Span::styled(
        marker,
        Style::default().fg(Color::Black).bg(Color::Black),
    )))
}

fn parse_rendered_markdown_image_path(text: &str) -> Option<&str> {
    let text = text.trim();
    if !text.starts_with("[image:") || !text.ends_with(')') {
        return None;
    }

    let start = text.rfind("] (")? + 3;
    let path = text.get(start..text.len().saturating_sub(1))?.trim();
    if path.is_empty()
        || path.starts_with("http://")
        || path.starts_with("https://")
        || path.starts_with("data:")
    {
        return None;
    }

    let lower = path.to_ascii_lowercase();
    if matches!(
        std::path::Path::new(&lower)
            .extension()
            .and_then(|extension| extension.to_str()),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico")
    ) {
        Some(path)
    } else {
        None
    }
}

fn resolve_side_panel_image_path(
    page: &crate::side_panel::SidePanelPage,
    path_text: &str,
) -> std::path::PathBuf {
    let path = std::path::Path::new(path_text);
    if path.is_absolute() {
        return path.to_path_buf();
    }

    std::path::Path::new(&page.file_path)
        .parent()
        .map(|parent| parent.join(path))
        .unwrap_or_else(|| path.to_path_buf())
}

#[cfg(test)]
#[path = "ui_pinned_tests.rs"]
mod tests;
