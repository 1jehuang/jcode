use super::*;
use crate::tui::mermaid;
use serde::Serialize;
#[cfg(test)]
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::hash::Hasher as _;

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

fn selection_bg_for(base_bg: Option<Color>) -> Color {
    let fallback = rgb(32, 38, 48);
    blend_color(base_bg.unwrap_or(fallback), accent_color(), 0.34)
}

fn selection_fg_for(base_fg: Option<Color>) -> Option<Color> {
    base_fg.map(|fg| blend_color(fg, Color::White, 0.15))
}

fn highlight_line_selection(
    line: &Line<'static>,
    start_col: usize,
    end_col: usize,
) -> Line<'static> {
    if end_col <= start_col {
        return line.clone();
    }

    let mut rebuilt: Vec<Span<'static>> = Vec::new();
    let mut current_text = String::new();
    let mut current_style: Option<Style> = None;
    let mut col = 0usize;

    let flush = |rebuilt: &mut Vec<Span<'static>>, text: &mut String, style: &mut Option<Style>| {
        if !text.is_empty() {
            let span = match style.take() {
                Some(style) => Span::styled(std::mem::take(text), style),
                None => Span::raw(std::mem::take(text)),
            };
            rebuilt.push(span);
        }
    };

    for span in &line.spans {
        for ch in span.content.chars() {
            let width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            let selected = if width == 0 {
                col > start_col && col <= end_col
            } else {
                col < end_col && col.saturating_add(width) > start_col
            };

            let mut style = span.style;
            if selected {
                style = style.bg(selection_bg_for(style.bg));
                if let Some(fg) = selection_fg_for(style.fg) {
                    style = style.fg(fg);
                }
            }

            if current_style == Some(style) {
                current_text.push(ch);
            } else {
                flush(&mut rebuilt, &mut current_text, &mut current_style);
                current_text.push(ch);
                current_style = Some(style);
            }

            col = col.saturating_add(width);
        }
    }

    flush(&mut rebuilt, &mut current_text, &mut current_style);

    Line {
        spans: rebuilt,
        style: line.style,
        alignment: line.alignment,
    }
}

fn apply_side_selection_highlight(
    app: &dyn TuiState,
    visible_lines: &mut [Line<'static>],
    scroll: usize,
) {
    let Some(range) = app.copy_selection_range().filter(|range| {
        range.start.pane == crate::tui::CopySelectionPane::SidePane
            && range.end.pane == crate::tui::CopySelectionPane::SidePane
    }) else {
        return;
    };

    let (start, end) =
        if (range.start.abs_line, range.start.column) <= (range.end.abs_line, range.end.column) {
            (range.start, range.end)
        } else {
            (range.end, range.start)
        };

    let visible_end = scroll.saturating_add(visible_lines.len());
    for abs_idx in start.abs_line.max(scroll)..=end.abs_line.min(visible_end.saturating_sub(1)) {
        let rel_idx = abs_idx.saturating_sub(scroll);
        if let Some(line) = visible_lines.get_mut(rel_idx) {
            let start_col = if abs_idx == start.abs_line {
                start.column
            } else {
                0
            };
            let end_col = if abs_idx == end.abs_line {
                end.column
            } else {
                line.width()
            };
            *line = highlight_line_selection(line, start_col, end_col);
        }
    }
}

/// Format tokens compactly (1.2M, 45K, 123)
fn format_tokens_compact(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.0}K", tokens as f64 / 1_000.0)
    } else {
        format!("{}", tokens)
    }
}

fn format_usage_line(tokens_str: String, cache_status: Option<String>) -> String {
    let mut parts = Vec::new();
    if !tokens_str.is_empty() {
        parts.push(tokens_str);
    }
    if let Some(cache) = cache_status {
        parts.push(cache);
    }
    if parts.is_empty() {
        String::new()
    } else {
        parts.join(" • ")
    }
}

fn format_cache_status(
    cache_read_tokens: Option<u64>,
    cache_creation_tokens: Option<u64>,
) -> Option<String> {
    match (cache_read_tokens, cache_creation_tokens) {
        (Some(read), _) if read > 0 => {
            let k = read / 1000;
            if k > 0 {
                Some(format!("⚡{}k cached", k))
            } else {
                Some(format!("⚡{} cached", read))
            }
        }
        (_, Some(created)) if created > 0 => {
            let k = created / 1000;
            if k > 0 {
                Some(format!("💾{}k stored", k))
            } else {
                Some(format!("💾{} stored", created))
            }
        }
        _ => None,
    }
}

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

#[derive(Debug, Clone, Default, Serialize)]
pub struct SidePanelDebugStats {
    pub markdown_cache_hits: u64,
    pub markdown_cache_misses: u64,
    pub render_cache_hits: u64,
    pub render_cache_misses: u64,
    pub markdown_cache_entries: usize,
    pub render_cache_entries: usize,
}

#[derive(Default)]
struct SidePanelDebugState {
    stats: SidePanelDebugStats,
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

fn build_side_pane_snapshot_cache(
    lines: &[Line<'static>],
    inner_width: u16,
) -> (
    std::sync::Arc<Vec<String>>,
    std::sync::Arc<Vec<usize>>,
    std::sync::Arc<Vec<String>>,
    std::sync::Arc<Vec<WrappedLineMap>>,
    Vec<u16>,
) {
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
const SIDE_PANEL_FOLLOWING_CONTENT_PREVIEW_MIN_ROWS: u16 = 6;
const SIDE_PANEL_FOLLOWING_CONTENT_PREVIEW_MAX_ROWS: u16 = 10;

static PINNED_CACHE: OnceLock<Mutex<PinnedCacheState>> = OnceLock::new();
static SIDE_PANEL_MARKDOWN_CACHE: OnceLock<Mutex<SidePanelMarkdownCacheState>> = OnceLock::new();
static SIDE_PANEL_RENDER_CACHE: OnceLock<Mutex<SidePanelRenderCacheState>> = OnceLock::new();
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

fn side_panel_markdown_cache() -> &'static Mutex<SidePanelMarkdownCacheState> {
    SIDE_PANEL_MARKDOWN_CACHE.get_or_init(|| Mutex::new(SidePanelMarkdownCacheState::default()))
}

fn side_panel_render_cache() -> &'static Mutex<SidePanelRenderCacheState> {
    SIDE_PANEL_RENDER_CACHE.get_or_init(|| Mutex::new(SidePanelRenderCacheState::default()))
}

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

fn lru_touch<K: PartialEq>(order: &mut VecDeque<K>, key: &K) {
    if let Some(pos) = order.iter().position(|existing| existing == key) {
        order.remove(pos);
    }
}

fn lru_get<K, V>(entries: &HashMap<K, V>, order: &mut VecDeque<K>, key: &K) -> Option<V>
where
    K: Clone + Eq + std::hash::Hash,
    V: Clone,
{
    let value = entries.get(key).cloned();
    if value.is_some() {
        lru_touch(order, key);
        order.push_back(key.clone());
    }
    value
}

fn lru_insert<K, V>(
    entries: &mut HashMap<K, V>,
    order: &mut VecDeque<K>,
    key: K,
    value: V,
    limit: usize,
) where
    K: Clone + Eq + std::hash::Hash,
{
    lru_touch(order, &key);
    entries.insert(key.clone(), value);
    order.push_back(key);
    while order.len() > limit {
        if let Some(oldest) = order.pop_front() {
            entries.remove(&oldest);
        }
    }
}

pub(crate) fn side_panel_debug_stats() -> SidePanelDebugStats {
    let mut stats = with_side_panel_debug(|state| state.stats.clone());
    stats.markdown_cache_entries = with_side_panel_markdown_cache(|cache| cache.entries.len());
    stats.render_cache_entries = with_side_panel_render_cache(|cache| cache.entries.len());
    stats
}

pub(crate) fn reset_side_panel_debug_stats() {
    with_side_panel_debug_mut(|debug| {
        debug.stats = SidePanelDebugStats::default();
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

fn hash_content(content: &str) -> u64 {
    use std::hash::{Hash as _, Hasher as _};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

fn side_panel_content_signature(page: &crate::side_panel::SidePanelPage) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    page.id.hash(&mut hasher);
    page.file_path.hash(&mut hasher);
    page.source.as_str().hash(&mut hasher);
    page.updated_at_ms.hash(&mut hasher);
    page.content.hash(&mut hasher);
    hasher.finish()
}

fn estimate_side_panel_pane_area(
    terminal_width: u16,
    terminal_height: u16,
    ratio_percent: u8,
) -> Option<Rect> {
    const MIN_DIFF_WIDTH: u16 = 30;
    const MIN_CHAT_WIDTH: u16 = 20;

    let max_diff = terminal_width.saturating_sub(MIN_CHAT_WIDTH);
    if max_diff < MIN_DIFF_WIDTH || terminal_height < 3 {
        return None;
    }

    let diff_width = (((terminal_width as u32 * ratio_percent.clamp(25, 100) as u32) / 100) as u16)
        .max(MIN_DIFF_WIDTH)
        .min(max_diff);
    Some(Rect::new(0, 0, diff_width, terminal_height))
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

fn compact_image_label(label: &str) -> String {
    if label.contains('/') {
        return label
            .rsplit('/')
            .take(2)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("/");
    }
    label.to_string()
}

fn div_ceil_u32_local(value: u32, divisor: u32) -> u32 {
    if divisor == 0 {
        return value;
    }
    value.saturating_add(divisor - 1) / divisor
}

fn estimate_inline_image_rows(img_w: u32, img_h: u32, pane_width: u16, pane_height: u16) -> u16 {
    let inner_width = pane_width.max(1) as u32;
    let (cell_w, cell_h) = mermaid::get_font_size().unwrap_or((8, 16));
    let cell_w = cell_w.max(1) as u32;
    let cell_h = cell_h.max(1) as u32;
    let width_px = inner_width.saturating_mul(cell_w);
    let scaled_height_px = div_ceil_u32_local(img_h.max(1).saturating_mul(width_px), img_w.max(1));
    let rows = div_ceil_u32_local(scaled_height_px, cell_h)
        .max(SIDE_PANEL_INLINE_IMAGE_MIN_ROWS as u32)
        .min(pane_height.max(SIDE_PANEL_INLINE_IMAGE_MIN_ROWS) as u32);
    rows as u16
}

pub(super) fn draw_pinned_content_cached(
    frame: &mut Frame,
    area: Rect,
    app: &dyn TuiState,
    scroll: usize,
    line_wrap: bool,
    focused: bool,
) {
    use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};

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

    let mut title_parts = vec![Span::styled(" pinned ", Style::default().fg(tool_color()))];
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
    title_parts.push(Span::styled(" ", Style::default().fg(dim_color())));

    let border_color = if focused { tool_color() } else { dim_color() };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .title(Line::from(title_parts));

    let inner = block.inner(area);
    frame.render_widget(block, area);

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

    if has_protocol {
        for placement in &rendered.image_placements {
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
                }
            }
        }
    }
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

fn is_rendered_table_line(line: &Line<'_>) -> bool {
    let text: String = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    text.contains(" │ ") || text.contains("─┼─")
}

fn estimate_side_panel_image_layout(
    hash: u64,
    inner: Rect,
    lines_before_image: usize,
    has_following_content: bool,
) -> SidePanelImageLayout {
    let Some((_, width, height)) = mermaid::get_cached_png(hash) else {
        return SidePanelImageLayout {
            rows: clamp_side_panel_image_rows(
                inner.height.min(12).max(SIDE_PANEL_INLINE_IMAGE_MIN_ROWS),
                inner.height,
                lines_before_image,
                has_following_content,
            ),
            render_mode: SidePanelImageRenderMode::Fit,
        };
    };

    estimate_side_panel_image_layout_with_font(
        width,
        height,
        inner.width,
        inner.height,
        lines_before_image,
        has_following_content,
        mermaid::get_font_size(),
    )
}

fn estimate_side_panel_image_layout_with_font(
    width: u32,
    height: u32,
    available_width: u16,
    inner_height: u16,
    lines_before_image: usize,
    has_following_content: bool,
    font_size: Option<(u16, u16)>,
) -> SidePanelImageLayout {
    if width == 0 || height == 0 || available_width == 0 {
        return SidePanelImageLayout {
            rows: clamp_side_panel_image_rows(
                SIDE_PANEL_INLINE_IMAGE_MIN_ROWS,
                inner_height,
                lines_before_image,
                has_following_content,
            ),
            render_mode: SidePanelImageRenderMode::Fit,
        };
    }

    let (cell_w, cell_h) = font_size.unwrap_or((8, 16));
    let cell_w = cell_w.max(1) as u32;
    let cell_h = cell_h.max(1) as u32;
    let image_w_cells = super::diagram_pane::div_ceil_u32(width.max(1), cell_w).max(1);
    let image_h_cells = super::diagram_pane::div_ceil_u32(height.max(1), cell_h).max(1);
    let available_width = available_width.max(1) as u32;

    let fit_zoom = if image_w_cells > available_width {
        ((available_width.saturating_mul(100)) / image_w_cells).clamp(1, 100) as u8
    } else {
        100
    };

    if fit_zoom < SIDE_PANEL_INLINE_IMAGE_MIN_ZOOM_PERCENT {
        let zoom_percent = SIDE_PANEL_INLINE_IMAGE_MIN_ZOOM_PERCENT;
        return SidePanelImageLayout {
            rows: scaled_image_rows(image_h_cells, zoom_percent)
                .max(SIDE_PANEL_INLINE_IMAGE_MIN_ROWS),
            render_mode: SidePanelImageRenderMode::ScrollableViewport { zoom_percent },
        };
    }

    let needed = scaled_image_rows(image_h_cells, fit_zoom);
    SidePanelImageLayout {
        rows: clamp_side_panel_image_rows(
            needed
                .max(SIDE_PANEL_INLINE_IMAGE_MIN_ROWS)
                .min(inner_height.max(SIDE_PANEL_INLINE_IMAGE_MIN_ROWS)),
            inner_height,
            lines_before_image,
            has_following_content,
        ),
        render_mode: SidePanelImageRenderMode::Fit,
    }
}

fn scaled_image_rows(image_h_cells: u32, zoom_percent: u8) -> u16 {
    if image_h_cells == 0 || zoom_percent == 0 {
        return 0;
    }

    super::diagram_pane::div_ceil_u32(image_h_cells.saturating_mul(zoom_percent as u32), 100)
        .min(u16::MAX as u32) as u16
}

fn estimate_side_panel_image_rows_with_font(
    width: u32,
    height: u32,
    available_width: u16,
    font_size: Option<(u16, u16)>,
) -> u16 {
    if width == 0 || height == 0 || available_width == 0 {
        return 0;
    }

    let (cell_w, cell_h) = font_size.unwrap_or((8, 16));
    let cell_w = cell_w.max(1) as u32;
    let cell_h = cell_h.max(1) as u32;

    let image_w_cells = super::diagram_pane::div_ceil_u32(width.max(1), cell_w).max(1);
    let image_h_cells = super::diagram_pane::div_ceil_u32(height.max(1), cell_h).max(1);
    let available_width = available_width.max(1) as u32;

    let fitted_h_cells = if image_w_cells > available_width {
        super::diagram_pane::div_ceil_u32(
            image_h_cells.saturating_mul(available_width),
            image_w_cells,
        )
    } else {
        image_h_cells
    }
    .max(1);

    fitted_h_cells.min(u16::MAX as u32) as u16
}

fn side_panel_viewport_scroll_x(
    img_w_px: u32,
    area_width: u16,
    zoom_percent: u8,
    centered: bool,
    font_size: Option<(u16, u16)>,
    pan_x_cells: i32,
) -> i32 {
    if img_w_px == 0 || area_width == 0 || zoom_percent == 0 {
        return 0;
    }

    let (font_w, _) = font_size.unwrap_or((8, 16));
    let font_w = font_w.max(1) as u32;
    let zoom = zoom_percent as u32;
    let view_w_px = (area_width as u32)
        .saturating_mul(font_w)
        .saturating_mul(100)
        / zoom;
    let max_scroll_x_px = img_w_px.saturating_sub(view_w_px);
    if max_scroll_x_px == 0 {
        return 0;
    }

    let cell_w_px = font_w.saturating_mul(100) / zoom;
    if cell_w_px == 0 {
        return 0;
    }

    let base_cells = if centered {
        ((max_scroll_x_px / 2) / cell_w_px).min(i32::MAX as u32) as i32
    } else {
        0
    };
    let max_cells = (max_scroll_x_px / cell_w_px).min(i32::MAX as u32) as i32;
    base_cells.saturating_add(pan_x_cells).clamp(0, max_cells)
}

fn fit_zoom_percent_for_area(
    area: Rect,
    img_w_px: u32,
    img_h_px: u32,
    font_size: Option<(u16, u16)>,
) -> u8 {
    if area.width == 0 || area.height == 0 || img_w_px == 0 || img_h_px == 0 {
        return 100;
    }

    let (font_w, font_h) = font_size.unwrap_or((8, 16));
    let font_w = font_w.max(1) as u32;
    let font_h = font_h.max(1) as u32;
    let zoom_w = area.width as u32 * font_w * 100 / img_w_px.max(1);
    let zoom_h = area.height as u32 * font_h * 100 / img_h_px.max(1);
    zoom_w.min(zoom_h).clamp(1, 200) as u8
}

fn plan_fit_image_render(
    viewport_area: Rect,
    viewport_start: usize,
    image_start: usize,
    reserved_rows: u16,
    img_w_px: u32,
    img_h_px: u32,
    centered: bool,
) -> Option<FitImageRenderPlan> {
    if viewport_area.width == 0
        || viewport_area.height == 0
        || reserved_rows == 0
        || img_w_px == 0
        || img_h_px == 0
    {
        return None;
    }

    let reserved_template = Rect {
        x: viewport_area.x,
        y: 0,
        width: viewport_area.width,
        height: reserved_rows,
    };
    let fitted = fit_side_panel_image_area(reserved_template, img_w_px, img_h_px, centered);
    if fitted.width == 0 || fitted.height == 0 {
        return None;
    }

    let reserved_top = viewport_area.y as i32 + image_start as i32 - viewport_start as i32;
    let fitted_top = reserved_top + fitted.y as i32;
    let fitted_bottom = fitted_top + fitted.height as i32;
    let viewport_top = viewport_area.y as i32;
    let viewport_bottom = viewport_top + viewport_area.height as i32;

    if fitted_bottom <= viewport_top || fitted_top >= viewport_bottom {
        return None;
    }

    let visible_top = fitted_top.max(viewport_top);
    let visible_bottom = fitted_bottom.min(viewport_bottom);
    let visible_height = (visible_bottom - visible_top) as u16;
    if visible_height == 0 {
        return None;
    }

    if visible_height == fitted.height && fitted_top >= 0 {
        return Some(FitImageRenderPlan::Full {
            area: Rect {
                x: fitted.x,
                y: fitted_top as u16,
                width: fitted.width,
                height: fitted.height,
            },
        });
    }

    Some(FitImageRenderPlan::ClippedViewport {
        area: Rect {
            x: fitted.x,
            y: visible_top.max(0) as u16,
            width: fitted.width,
            height: visible_height,
        },
        scroll_y: visible_top.saturating_sub(fitted_top),
        zoom_percent: fit_zoom_percent_for_area(
            fitted,
            img_w_px,
            img_h_px,
            mermaid::get_font_size(),
        ),
    })
}

fn fit_side_panel_image_area(area: Rect, img_w_px: u32, img_h_px: u32, centered: bool) -> Rect {
    fit_image_area_with_font(
        area,
        img_w_px,
        img_h_px,
        mermaid::get_font_size(),
        centered,
        false,
    )
}

fn fit_image_area_with_font(
    area: Rect,
    img_w_px: u32,
    img_h_px: u32,
    font_size: Option<(u16, u16)>,
    centered: bool,
    vcenter: bool,
) -> Rect {
    if area.width == 0 || area.height == 0 || img_w_px == 0 || img_h_px == 0 {
        return area;
    }

    let (font_w, font_h) = match font_size {
        Some(fs) => (fs.0.max(1) as f64, fs.1.max(1) as f64),
        None => return area,
    };

    let area_w_px = area.width as f64 * font_w;
    let area_h_px = area.height as f64 * font_h;
    let scale = (area_w_px / img_w_px as f64).min(area_h_px / img_h_px as f64);
    if !scale.is_finite() || scale <= 0.0 {
        return area;
    }

    let fitted_w_cells = ((img_w_px as f64 * scale) / font_w)
        .ceil()
        .max(1.0)
        .min(area.width as f64) as u16;
    let fitted_h_cells = ((img_h_px as f64 * scale) / font_h)
        .ceil()
        .max(1.0)
        .min(area.height as f64) as u16;

    let x_offset = if centered {
        area.width.saturating_sub(fitted_w_cells) / 2
    } else {
        0
    };
    let y_offset = if vcenter {
        area.height.saturating_sub(fitted_h_cells) / 2
    } else {
        0
    };

    Rect {
        x: area.x + x_offset,
        y: area.y + y_offset,
        width: fitted_w_cells,
        height: fitted_h_cells,
    }
}

fn clamp_side_panel_image_rows(
    estimated_rows: u16,
    inner_height: u16,
    _lines_before_image: usize,
    has_following_content: bool,
) -> u16 {
    let min_rows = SIDE_PANEL_INLINE_IMAGE_MIN_ROWS.min(inner_height.max(1));
    let max_rows = inner_height.max(min_rows);
    let estimated_rows = estimated_rows.max(min_rows).min(max_rows);

    if !has_following_content {
        return estimated_rows;
    }

    let desired_preview_rows = ((inner_height as u32) / 3)
        .max(SIDE_PANEL_FOLLOWING_CONTENT_PREVIEW_MIN_ROWS as u32)
        .min(SIDE_PANEL_FOLLOWING_CONTENT_PREVIEW_MAX_ROWS as u32)
        as u16;
    let preview_rows = desired_preview_rows.min(inner_height.saturating_sub(1));
    // Important: this limit is about leaving a preview of *following* content
    // visible in the current viewport. It must not depend on how many wrapped
    // lines happened to appear earlier in the document, because those lines are
    // scrolled away once the image is in view. Using total preceding lines here
    // causes later diagrams in long side-panel pages to collapse to the minimum
    // height (often 4 rows), which makes multi-chart dashboard pages nearly
    // unreadable.
    let max_rows_for_image = inner_height.saturating_sub(preview_rows).max(min_rows);

    estimated_rows.min(max_rows_for_image)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn clear_side_panel_render_caches() {
        super::clear_side_panel_render_caches();
    }

    fn mermaid_test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn with_mermaid_placeholder_mode<T>(f: impl FnOnce() -> T) -> T {
        struct ResetVideoExportMode;
        impl Drop for ResetVideoExportMode {
            fn drop(&mut self) {
                crate::tui::mermaid::set_video_export_mode(false);
            }
        }

        let _guard = mermaid_test_lock()
            .lock()
            .expect("mermaid placeholder test lock");
        crate::tui::mermaid::set_video_export_mode(true);
        let _reset = ResetVideoExportMode;
        let result = f();
        result
    }

    fn with_serialized_mermaid_state<T>(f: impl FnOnce() -> T) -> T {
        let _guard = mermaid_test_lock().lock().expect("mermaid test lock");
        f()
    }

    fn sample_mermaid_page(content: impl Into<String>) -> crate::side_panel::SidePanelPage {
        use std::hash::{Hash as _, Hasher as _};

        let content = content.into();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        content.hash(&mut hasher);
        let content_hash = hasher.finish();

        crate::side_panel::SidePanelPage {
            id: format!("mermaid_demo_{content_hash:016x}"),
            title: format!("Mermaid Demo {content_hash:016x}"),
            file_path: format!("mermaid_demo_{content_hash:016x}.md"),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content,
            updated_at_ms: content_hash,
        }
    }

    #[test]
    fn clamp_side_panel_image_rows_leaves_room_for_following_content() {
        let rows = clamp_side_panel_image_rows(18, 16, 2, true);
        assert_eq!(rows, 10);
    }

    #[test]
    fn clamp_side_panel_image_rows_preserves_estimate_without_following_content() {
        let rows = clamp_side_panel_image_rows(18, 16, 2, false);
        assert_eq!(rows, 16);
    }

    #[test]
    fn clamp_side_panel_image_rows_keeps_minimum_image_presence() {
        let rows = clamp_side_panel_image_rows(10, 5, 1, true);
        assert_eq!(rows, 4);
    }

    #[test]
    fn clamp_side_panel_image_rows_ignores_preceding_document_length() {
        let near_top = clamp_side_panel_image_rows(18, 16, 2, true);
        let far_down_page = clamp_side_panel_image_rows(18, 16, 200, true);
        assert_eq!(near_top, 10);
        assert_eq!(far_down_page, near_top);
    }

    #[test]
    fn estimate_side_panel_image_rows_uses_actual_inner_width() {
        let rows = estimate_side_panel_image_rows_with_font(999, 1454, 36, Some((8, 16)));
        assert_eq!(rows, 27);
    }

    #[test]
    fn side_panel_mermaid_switches_to_scrollable_viewport_when_fit_would_be_too_small() {
        let layout =
            estimate_side_panel_image_layout_with_font(4000, 2000, 24, 20, 0, false, Some((8, 16)));

        assert_eq!(
            layout.render_mode,
            SidePanelImageRenderMode::ScrollableViewport {
                zoom_percent: SIDE_PANEL_INLINE_IMAGE_MIN_ZOOM_PERCENT,
            }
        );
        assert!(layout.rows > 20, "expected tall scrollable diagram rows");
        assert!(layout.render_mode.is_scrollable());
    }

    #[test]
    fn side_panel_mermaid_keeps_fit_mode_when_zoom_stays_readable() {
        let layout =
            estimate_side_panel_image_layout_with_font(300, 480, 36, 30, 0, true, Some((8, 16)));

        assert_eq!(layout.render_mode, SidePanelImageRenderMode::Fit);
        assert_eq!(layout.rows, 20);
        assert!(!layout.render_mode.is_scrollable());
    }

    #[test]
    fn side_panel_viewport_scroll_x_applies_horizontal_pan_around_center() {
        let centered = side_panel_viewport_scroll_x(4000, 24, 70, true, Some((8, 16)), 0);
        let panned_right = side_panel_viewport_scroll_x(4000, 24, 70, true, Some((8, 16)), 6);
        let panned_left = side_panel_viewport_scroll_x(4000, 24, 70, true, Some((8, 16)), -6);

        assert!(centered > 0, "expected oversized diagram to start centered");
        assert!(
            panned_right > centered,
            "expected positive pan to move viewport right"
        );
        assert!(
            panned_left < centered,
            "expected negative pan to move viewport left"
        );
    }

    #[test]
    fn fit_side_panel_image_area_centers_constrained_image_horizontally() {
        let area = Rect::new(10, 4, 36, 12);
        let fitted = fit_image_area_with_font(area, 999, 1454, Some((8, 16)), true, false);

        assert!(fitted.width < area.width);
        assert!(
            fitted.x > area.x,
            "expected horizontal centering: {:?} within {:?}",
            fitted,
            area
        );
        assert_eq!(
            fitted.y, area.y,
            "inline side-panel images should remain top-aligned"
        );
        assert_eq!(fitted.height, area.height);
    }

    #[test]
    fn fit_side_panel_image_area_preserves_full_width_when_width_constrained() {
        let area = Rect::new(0, 0, 36, 30);
        let fitted = fit_image_area_with_font(area, 999, 1454, Some((8, 16)), true, false);

        assert_eq!(fitted.x, area.x);
        assert_eq!(fitted.width, area.width);
        assert!(fitted.height < area.height);
    }

    #[test]
    fn plan_fit_image_render_uses_clipped_viewport_for_partial_visibility() {
        let viewport = Rect::new(0, 10, 36, 12);
        let plan =
            plan_fit_image_render(viewport, 4, 0, 12, 720, 1440, true).expect("fit render plan");

        match plan {
            FitImageRenderPlan::ClippedViewport {
                area,
                scroll_y,
                zoom_percent,
            } => {
                assert!(
                    area.height < 12,
                    "expected clipped visible height: {area:?}"
                );
                assert!(scroll_y > 0, "expected positive vertical clip offset");
                assert!(zoom_percent > 0);
            }
            other => panic!("expected clipped viewport plan, got {other:?}"),
        }
    }

    #[test]
    fn plan_fit_image_render_uses_full_fit_when_fully_visible() {
        let viewport = Rect::new(0, 10, 36, 12);
        let plan =
            plan_fit_image_render(viewport, 0, 0, 12, 720, 1440, true).expect("fit render plan");

        match plan {
            FitImageRenderPlan::Full { area } => {
                assert_eq!(area.y, viewport.y);
                assert_eq!(area.height, viewport.height);
            }
            other => panic!("expected full fit plan, got {other:?}"),
        }
    }

    #[test]
    fn render_side_panel_markdown_keeps_text_after_mermaid_block() {
        let page = sample_mermaid_page(
            "This is some text above the diagram.\n\n```mermaid\nflowchart TD\n    A[Start] --> B[Do the thing]\n    B --> C[Done]\n```\n\nThis is some text below the diagram.",
        );

        let rendered = with_mermaid_placeholder_mode(|| {
            render_side_panel_markdown_cached(&page, Rect::new(0, 0, 36, 30), true, true)
        });
        let text: Vec<String> = rendered
            .lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();

        assert!(
            text.iter()
                .any(|line| line.contains("This is some text above the diagram.")),
            "expected text above mermaid block in rendered lines: {:?}",
            text
        );
        assert!(
            text.iter()
                .any(|line| line.contains("This is some text below the diagram.")),
            "expected text below mermaid block in rendered lines: {:?}",
            text
        );
        if let Some(placement) = rendered.image_placements.first() {
            assert!(
                placement.rows < 30,
                "image should not consume the full side-panel height when trailing text exists"
            );
        }
    }

    #[test]
    fn render_side_panel_markdown_late_mermaid_keeps_reasonable_rows() {
        let mut content = String::new();
        for i in 0..24 {
            content.push_str(&format!("Paragraph {} before chart.\n\n", i + 1));
        }
        content.push_str(
            "```mermaid\nxychart-beta\n    title \"Volume\"\n    x-axis [A, B, C, D]\n    y-axis \"Count\" 0 --> 100\n    bar [10, 50, 80, 30]\n```\n\nTail text after chart.\n",
        );

        let page = sample_mermaid_page(content);

        let rendered = with_mermaid_placeholder_mode(|| {
            render_side_panel_markdown_cached(&page, Rect::new(0, 0, 48, 30), true, true)
        });

        let placement = rendered
            .image_placements
            .first()
            .expect("expected mermaid image placement");

        assert!(
            placement.rows >= 8,
            "late side-panel mermaid should not collapse to tiny height: {} rows",
            placement.rows
        );
    }

    #[test]
    fn render_side_panel_markdown_reserves_blank_rows_for_mermaid_placement() {
        let page = sample_mermaid_page(
            "Intro text.\n\n```mermaid\nflowchart TD\n    A[Start] --> B[Done]\n```\n",
        );

        let rendered = with_mermaid_placeholder_mode(|| {
            render_side_panel_markdown_cached(&page, Rect::new(0, 0, 36, 24), true, true)
        });

        assert_eq!(
            rendered.image_placements.len(),
            1,
            "expected one mermaid image placement"
        );
        let placement = &rendered.image_placements[0];
        assert!(placement.rows >= SIDE_PANEL_INLINE_IMAGE_MIN_ROWS);
        let reserved = &rendered.lines
            [placement.after_text_line..placement.after_text_line + placement.rows as usize];
        assert!(
            reserved.iter().all(|line| line.width() == 0),
            "expected reserved side-panel image rows to remain blank placeholders: {:?}",
            reserved
        );
    }

    #[test]
    fn render_side_panel_markdown_multiple_mermaids_create_ordered_placements() {
        let page = sample_mermaid_page(
            "Alpha\n\n```mermaid\nflowchart TD\n    A --> B\n```\n\nBetween\n\n```mermaid\nflowchart TD\n    C --> D\n```\n\nOmega\n",
        );

        let rendered = with_mermaid_placeholder_mode(|| {
            render_side_panel_markdown_cached(&page, Rect::new(0, 0, 40, 28), true, true)
        });

        assert_eq!(
            rendered.image_placements.len(),
            2,
            "expected two mermaid placements"
        );
        assert!(
            rendered.image_placements[0].after_text_line
                < rendered.image_placements[1].after_text_line,
            "expected mermaid placements to preserve document order: {:?}",
            rendered
                .image_placements
                .iter()
                .map(|p| (p.after_text_line, p.rows))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn render_side_panel_markdown_without_protocol_falls_back_to_text_placeholder() {
        let page = sample_mermaid_page("```mermaid\nflowchart TD\n    A --> B\n```\n");

        let rendered = with_serialized_mermaid_state(|| {
            render_side_panel_markdown_cached(&page, Rect::new(0, 0, 36, 20), false, true)
        });
        let text: Vec<String> = rendered
            .lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();

        assert!(
            rendered.image_placements.is_empty(),
            "expected no image placement without protocol support: {:?}",
            rendered.image_placements.len()
        );
        assert!(
            text.iter().any(|line| line.contains("mermaid diagram")),
            "expected textual placeholder when image protocols are unavailable: {:?}",
            text
        );
    }

    #[test]
    fn render_side_panel_markdown_trailing_text_reduces_mermaid_rows() {
        let chart = "```mermaid\nxychart-beta\n    title \"Volume\"\n    x-axis [A, B, C, D]\n    y-axis \"Count\" 0 --> 100\n    bar [10, 50, 80, 30]\n```\n";
        let page_without_tail = sample_mermaid_page(chart);
        let page_with_tail = sample_mermaid_page(format!("{chart}\nTail text after chart.\n"));

        let (without_tail, with_tail) = with_mermaid_placeholder_mode(|| {
            (
                render_side_panel_markdown_cached(
                    &page_without_tail,
                    Rect::new(0, 0, 48, 30),
                    true,
                    true,
                ),
                render_side_panel_markdown_cached(
                    &page_with_tail,
                    Rect::new(0, 0, 48, 30),
                    true,
                    true,
                ),
            )
        });

        let rows_without_tail = without_tail
            .image_placements
            .first()
            .expect("expected mermaid placement without trailing text")
            .rows;
        let rows_with_tail = with_tail
            .image_placements
            .first()
            .expect("expected mermaid placement with trailing text")
            .rows;

        assert!(
            rows_without_tail >= rows_with_tail,
            "trailing text should not increase image rows: without tail {}, with tail {}",
            rows_without_tail,
            rows_with_tail
        );
    }

    #[test]
    fn render_side_panel_markdown_wraps_long_text_lines() {
        let page = crate::side_panel::SidePanelPage {
            id: "wrap_demo".to_string(),
            title: "Wrap Demo".to_string(),
            file_path: "wrap_demo.md".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "This is a deliberately long side panel line that should wrap instead of overflowing the pane.".to_string(),
            updated_at_ms: 1,
        };

        let rendered =
            render_side_panel_markdown_cached(&page, Rect::new(0, 0, 18, 30), false, false);

        let non_empty: Vec<&Line<'_>> = rendered
            .lines
            .iter()
            .filter(|line| line.width() > 0)
            .collect();

        assert!(
            non_empty.len() >= 2,
            "expected long side panel text to wrap: {:?}",
            rendered.lines
        );
        assert!(
            non_empty.iter().all(|line| line.width() <= 18),
            "expected wrapped side panel lines to fit width 18: {:?}",
            rendered.lines
        );
    }

    #[test]
    fn render_side_panel_markdown_keeps_table_rows_intact() {
        let page = crate::side_panel::SidePanelPage {
            id: "table_demo".to_string(),
            title: "Table Demo".to_string(),
            file_path: "table_demo.md".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "| # | Principle | Story Ready |\n| - | - | - |\n| 1 | Customer Obsession | unchecked |".to_string(),
            updated_at_ms: 1,
        };

        let rendered =
            render_side_panel_markdown_cached(&page, Rect::new(0, 0, 24, 20), false, false);
        let text: Vec<String> = rendered
            .lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();

        assert!(
            text.iter().any(|line| line.contains("─┼─")),
            "expected separator line to remain intact: {:?}",
            text
        );
        assert!(
            text.iter()
                .any(|line| line.matches('│').count() == 2 && line.contains("Cust")),
            "expected a single intact table row line: {:?}",
            text
        );
    }

    #[test]
    fn render_side_panel_markdown_live_syncs_file_content() {
        let temp = tempfile::tempdir().expect("tempdir");
        let file_path = temp.path().join("live.md");
        std::fs::write(&file_path, "# First").expect("write initial content");

        let mut snapshot = crate::side_panel::SidePanelSnapshot {
            focused_page_id: Some("live_demo".to_string()),
            pages: vec![crate::side_panel::SidePanelPage {
                id: "live_demo".to_string(),
                title: "Live Demo".to_string(),
                file_path: file_path.display().to_string(),
                format: crate::side_panel::SidePanelPageFormat::Markdown,
                source: crate::side_panel::SidePanelPageSource::LinkedFile,
                content: "# Stale".to_string(),
                updated_at_ms: 1,
            }],
        };

        clear_side_panel_render_caches();
        assert!(crate::side_panel::refresh_linked_page_content(
            &mut snapshot,
            None
        ));
        let page = snapshot.focused_page().expect("focused page");

        let first = render_side_panel_markdown_cached(&page, Rect::new(0, 0, 24, 20), false, false);
        let first_text: Vec<String> = first
            .lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(
            first_text.iter().any(|line| line.contains("First")),
            "expected first render to use file content: {:?}",
            first_text
        );

        std::fs::write(&file_path, "# Second").expect("write updated content");

        assert!(crate::side_panel::refresh_linked_page_content(
            &mut snapshot,
            None
        ));
        let page = snapshot.focused_page().expect("focused page");

        let second =
            render_side_panel_markdown_cached(&page, Rect::new(0, 0, 24, 20), false, false);
        let second_text: Vec<String> = second
            .lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(
            second_text.iter().any(|line| line.contains("Second")),
            "expected second render to reflect updated file content: {:?}",
            second_text
        );
    }

    #[test]
    fn render_side_panel_height_change_reuses_markdown_render_cache() {
        clear_side_panel_render_caches();
        let before = markdown::debug_stats().total_renders;
        let page = crate::side_panel::SidePanelPage {
            id: "height_cache_demo".to_string(),
            title: "Height Cache Demo".to_string(),
            file_path: "height_cache_demo.md".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content:
                "# Demo\n\nThis side panel should only parse markdown once for a stable width."
                    .to_string(),
            updated_at_ms: 9,
        };

        let _first =
            render_side_panel_markdown_cached(&page, Rect::new(0, 0, 28, 18), false, false);
        let after_first = markdown::debug_stats().total_renders;
        let _second =
            render_side_panel_markdown_cached(&page, Rect::new(0, 0, 28, 26), false, false);
        let after_second = markdown::debug_stats().total_renders;

        assert!(
            after_first > before,
            "expected initial render to parse markdown"
        );
        assert_eq!(
            after_second, after_first,
            "height-only cache miss should not trigger another markdown render"
        );
    }

    #[test]
    fn render_side_panel_content_change_with_same_revision_invalidates_cache() {
        clear_side_panel_render_caches();

        let first_page = crate::side_panel::SidePanelPage {
            id: "cache_invalidation_demo".to_string(),
            title: "Cache Invalidation Demo".to_string(),
            file_path: "cache_invalidation_demo.md".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "# First version".to_string(),
            updated_at_ms: 1,
        };
        let second_page = crate::side_panel::SidePanelPage {
            content: "# Second version".to_string(),
            ..first_page.clone()
        };

        let first =
            render_side_panel_markdown_cached(&first_page, Rect::new(0, 0, 28, 12), false, false);
        let second =
            render_side_panel_markdown_cached(&second_page, Rect::new(0, 0, 28, 12), false, false);

        let first_text: Vec<String> = first
            .lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        let second_text: Vec<String> = second
            .lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();

        assert!(
            first_text.iter().any(|line| line.contains("First version")),
            "expected first render to contain the original content: {:?}",
            first_text
        );
        assert!(
            second_text
                .iter()
                .any(|line| line.contains("Second version")),
            "expected second render to invalidate the stale cache entry: {:?}",
            second_text
        );
    }

    #[test]
    fn prewarm_focused_side_panel_reuses_markdown_cache_on_first_draw() {
        clear_side_panel_render_caches();
        let before = markdown::debug_stats().total_renders;
        let snapshot = crate::side_panel::SidePanelSnapshot {
            focused_page_id: Some("prewarm_demo".to_string()),
            pages: vec![crate::side_panel::SidePanelPage {
                id: "prewarm_demo".to_string(),
                title: "Prewarm Demo".to_string(),
                file_path: "prewarm_demo.md".to_string(),
                format: crate::side_panel::SidePanelPageFormat::Markdown,
                source: crate::side_panel::SidePanelPageSource::Managed,
                content: "# Demo\n\nThis should be warm before first draw.".to_string(),
                updated_at_ms: 7,
            }],
        };

        assert!(prewarm_focused_side_panel(
            &snapshot, 120, 40, 40, false, false
        ));
        let after_prewarm = markdown::debug_stats().total_renders;
        let page = snapshot.focused_page().expect("focused page");
        let pane_area = estimate_side_panel_pane_area(120, 40, 40).expect("side panel area");
        let inner = side_panel_content_area(pane_area).expect("side panel content area");
        let _ = render_side_panel_markdown_cached(&page, inner, false, false);
        let after_draw = markdown::debug_stats().total_renders;

        assert!(
            after_prewarm > before,
            "expected prewarm to render markdown once"
        );
        assert_eq!(
            after_draw, after_prewarm,
            "expected first draw to reuse prewarmed markdown cache"
        );
    }

    #[test]
    fn render_side_panel_managed_pages_ignore_disk_file_content() {
        let temp = tempfile::tempdir().expect("tempdir");
        let file_path = temp.path().join("managed.md");
        std::fs::write(&file_path, "# Disk Version").expect("write disk content");

        let page = crate::side_panel::SidePanelPage {
            id: "managed_demo".to_string(),
            title: "Managed Demo".to_string(),
            file_path: file_path.display().to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "# In Memory".to_string(),
            updated_at_ms: 42,
        };

        let rendered =
            render_side_panel_markdown_cached(&page, Rect::new(0, 0, 24, 20), false, false);
        let text: Vec<String> = rendered
            .lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();

        assert!(
            text.iter().any(|line| line.contains("In Memory")),
            "expected managed side panel to render snapshot content: {:?}",
            text
        );
        assert!(
            !text.iter().any(|line| line.contains("Disk Version")),
            "managed side panel should not re-read disk content: {:?}",
            text
        );
    }

    #[test]
    fn render_side_panel_linked_file_missing_file_falls_back_to_snapshot_content() {
        let temp = tempfile::tempdir().expect("tempdir");
        let file_path = temp.path().join("linked.md");

        let page = crate::side_panel::SidePanelPage {
            id: "linked_missing_demo".to_string(),
            title: "Linked Missing Demo".to_string(),
            file_path: file_path.display().to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::LinkedFile,
            content: "# Snapshot Fallback".to_string(),
            updated_at_ms: 7,
        };

        let rendered =
            render_side_panel_markdown_cached(&page, Rect::new(0, 0, 24, 20), false, false);
        let text: Vec<String> = rendered
            .lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();

        assert!(
            text.iter().any(|line| line.contains("Snapshot Fallback")),
            "expected linked side panel to fall back to snapshot content when file is missing: {:?}",
            text
        );
    }
}

#[allow(dead_code)]
fn draw_pinned_content(
    frame: &mut Frame,
    area: Rect,
    entries: &[PinnedContentEntry],
    scroll: usize,
    line_wrap: bool,
    focused: bool,
) {
    use ratatui::widgets::{Paragraph, Wrap};

    if area.width < 10 || area.height < 3 {
        return;
    }

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

    let mut title_parts = vec![Span::styled(" pinned ", Style::default().fg(tool_color()))];
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
    title_parts.push(Span::styled(" ", Style::default().fg(dim_color())));

    let border_style = super::right_rail_border_style(focused, tool_color());
    let Some(inner) =
        super::draw_right_rail_chrome(frame, area, Line::from(title_parts), border_style)
    else {
        return;
    };

    let mut text_lines: Vec<Line<'static>> = Vec::new();
    let mut last_image_group: Option<ImageGroup> = None;

    struct ImagePlacement {
        after_text_line: usize,
        hash: u64,
        rows: u16,
    }
    let mut image_placements: Vec<ImagePlacement> = Vec::new();

    let has_protocol = mermaid::protocol_type().is_some();

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
                        let highlighted = markdown::highlight_line(line.content.as_str(), file_ext);
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
                    image_placements.push(ImagePlacement {
                        after_text_line: text_lines.len(),
                        hash: *hash,
                        rows: img_rows,
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

    let total_lines = text_lines.len();
    super::set_pinned_pane_total_lines(total_lines);

    let max_scroll = total_lines.saturating_sub(inner.height as usize);
    let clamped_scroll = scroll.min(max_scroll);
    super::set_last_diff_pane_effective_scroll(clamped_scroll);

    let visible_lines: Vec<Line<'static>> = text_lines.into_iter().skip(clamped_scroll).collect();

    let paragraph = if line_wrap {
        Paragraph::new(visible_lines).wrap(Wrap { trim: false })
    } else {
        Paragraph::new(visible_lines)
    };
    frame.render_widget(paragraph, inner);

    if has_protocol {
        for placement in &image_placements {
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
