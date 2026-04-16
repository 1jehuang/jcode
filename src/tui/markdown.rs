use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::prelude::*;
use serde::Serialize;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{LazyLock, Mutex};
use std::time::Instant;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style as SynStyle, ThemeSet};
use syntect::parsing::SyntaxSet;
use unicode_width::UnicodeWidthStr;

use crate::config::{DiagramDisplayMode, MarkdownSpacingMode};
use crate::tui::mermaid;
use crate::tui::ui::{CopyTargetKind, RawCopyTarget};
#[path = "markdown_context.rs"]
mod context;
#[path = "markdown_wrap.rs"]
mod wrap;

#[cfg(test)]
pub(crate) use context::with_markdown_spacing_mode_override;
pub use context::{
    center_code_blocks, get_diagram_mode_override, set_center_code_blocks,
    set_diagram_mode_override, with_deferred_mermaid_render_context,
};
use context::{
    deferred_mermaid_render_context_enabled, effective_diagram_mode,
    effective_markdown_spacing_mode, streaming_render_context_enabled,
    with_streaming_render_context,
};

#[path = "markdown_render_full.rs"]
mod render_full;
#[path = "markdown_render_lazy.rs"]
mod render_lazy;
#[path = "markdown_render_support.rs"]
mod render_support;

pub use render_full::render_markdown_with_width;
pub use render_lazy::render_markdown_lazy;
pub(crate) use render_support::extract_copy_targets_from_rendered_lines;
use render_support::{
    highlight_code_cached, line_plain_text, placeholder_code_block, ranges_overlap, render_table,
};
pub use render_support::{highlight_file_lines, highlight_line, render_table_with_width};

// Syntax highlighting resources (loaded once)
static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

// Syntax highlighting cache - keyed by (code content hash, language)
static HIGHLIGHT_CACHE: LazyLock<Mutex<HighlightCache>> =
    LazyLock::new(|| Mutex::new(HighlightCache::new()));

const HIGHLIGHT_CACHE_LIMIT: usize = 256;

#[derive(Debug, Clone, Default, Serialize)]
pub struct MarkdownDebugStats {
    pub total_renders: u64,
    pub last_render_ms: Option<f32>,
    pub last_text_len: Option<usize>,
    pub last_lines: Option<usize>,
    pub last_headings: usize,
    pub last_code_blocks: usize,
    pub last_mermaid_blocks: usize,
    pub last_tables: usize,
    pub last_list_items: usize,
    pub last_blockquotes: usize,
    pub highlight_cache_hits: u64,
    pub highlight_cache_misses: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct MarkdownMemoryProfile {
    pub process_rss_bytes: Option<u64>,
    pub process_peak_rss_bytes: Option<u64>,
    pub process_virtual_bytes: Option<u64>,
    pub highlight_cache_entries: usize,
    pub highlight_cache_limit: usize,
    pub highlight_cache_lines: usize,
    pub highlight_cache_spans: usize,
    pub highlight_cache_text_bytes: usize,
    pub highlight_cache_estimate_bytes: usize,
}

#[derive(Debug, Clone, Default)]
struct MarkdownDebugState {
    stats: MarkdownDebugStats,
}

static MARKDOWN_DEBUG: LazyLock<Mutex<MarkdownDebugState>> =
    LazyLock::new(|| Mutex::new(MarkdownDebugState::default()));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkdownBlockKind {
    Heading,
    Paragraph,
    List,
    BlockQuote,
    DefinitionList,
    CodeBlock,
    DisplayMath,
    Rule,
    HtmlBlock,
    Table,
}

fn spacing_separates_after(kind: MarkdownBlockKind, mode: MarkdownSpacingMode) -> bool {
    match mode {
        MarkdownSpacingMode::Compact => !matches!(kind, MarkdownBlockKind::Heading),
        MarkdownSpacingMode::Document => true,
    }
}

fn line_is_blank(line: &Line<'_>) -> bool {
    line.spans.is_empty()
        || line
            .spans
            .iter()
            .all(|span| span.content.as_ref().is_empty())
}

fn rendered_task_marker_width(text: &str) -> Option<(usize, &str)> {
    if let Some(rest) = text.strip_prefix("[x] ") {
        return Some((UnicodeWidthStr::width("[x] "), rest));
    }
    if let Some(rest) = text.strip_prefix("[ ] ") {
        return Some((UnicodeWidthStr::width("[ ] "), rest));
    }
    None
}

fn rendered_list_marker_width(text: &str) -> Option<usize> {
    if let Some(rest) = text.strip_prefix("• ") {
        let mut width = UnicodeWidthStr::width("• ");
        if let Some((task_width, task_rest)) = rendered_task_marker_width(rest)
            && !task_rest.is_empty()
        {
            width += task_width;
        }
        return (!rest.is_empty()).then_some(width);
    }

    let digit_count = text.chars().take_while(|ch| ch.is_ascii_digit()).count();
    if digit_count == 0 {
        return None;
    }

    let suffix = text.get(digit_count..)?;
    let rest = suffix.strip_prefix(". ")?;
    let mut width = digit_count + UnicodeWidthStr::width(". ");
    if let Some((task_width, task_rest)) = rendered_task_marker_width(rest)
        && !task_rest.is_empty()
    {
        width += task_width;
    }
    (!rest.is_empty()).then_some(width)
}

fn repeated_gutter_prefix(line: &Line<'static>) -> Option<(Vec<Span<'static>>, usize)> {
    let plain = line_plain_text(line);
    let mut leading_width = 0usize;
    let mut prefix_bytes = 0usize;
    for ch in plain.chars() {
        if ch.is_whitespace() {
            leading_width += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            prefix_bytes += ch.len_utf8();
        } else {
            break;
        }
    }

    let mut rest = &plain[prefix_bytes..];
    let mut gutter_count = 0usize;
    while let Some(next) = rest.strip_prefix("│ ") {
        gutter_count += 1;
        rest = next;
    }
    let gutter_width = gutter_count * UnicodeWidthStr::width("│ ");
    let base_prefix_width = leading_width + gutter_width;

    if let Some(marker_width) = rendered_list_marker_width(rest) {
        let total_width = base_prefix_width + marker_width;
        if total_width > 0 {
            let mut spans = leading_spans_for_display_width(line, base_prefix_width);
            spans.push(Span::raw(" ".repeat(marker_width)));
            return Some((spans, total_width));
        }
    }

    if gutter_count > 0 {
        return Some((
            leading_spans_for_display_width(line, base_prefix_width),
            base_prefix_width,
        ));
    }

    if leading_width > 0 && line.alignment == Some(Alignment::Left) {
        return Some((
            leading_spans_for_display_width(line, leading_width),
            leading_width,
        ));
    }

    None
}

fn leading_spans_for_display_width(
    line: &Line<'static>,
    target_width: usize,
) -> Vec<Span<'static>> {
    if target_width == 0 {
        return Vec::new();
    }

    let mut spans = Vec::new();
    let mut collected_width = 0usize;

    for span in &line.spans {
        if collected_width >= target_width {
            break;
        }

        let mut text = String::new();
        let mut span_width = 0usize;
        for ch in span.content.chars() {
            let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if collected_width + span_width + ch_width > target_width {
                break;
            }
            text.push(ch);
            span_width += ch_width;
        }

        if !text.is_empty() {
            spans.push(Span::styled(text, span.style));
            collected_width += span_width;
        }
    }

    spans
}

fn push_blank_separator(lines: &mut Vec<Line<'static>>) {
    if lines.last().map(line_is_blank).unwrap_or(false) {
        return;
    }
    lines.push(Line::default());
}

fn push_block_separator(
    lines: &mut Vec<Line<'static>>,
    kind: MarkdownBlockKind,
    mode: MarkdownSpacingMode,
) {
    if spacing_separates_after(kind, mode) {
        push_blank_separator(lines);
    }
}

fn normalize_block_separators(lines: &mut Vec<Line<'static>>) {
    let mut normalized = Vec::with_capacity(lines.len());
    let mut previous_blank = true;

    for line in lines.drain(..) {
        let is_blank = line_is_blank(&line);
        if is_blank {
            if previous_blank {
                continue;
            }
            normalized.push(Line::default());
        } else {
            normalized.push(line);
        }
        previous_blank = is_blank;
    }

    while normalized.last().map(line_is_blank).unwrap_or(false) {
        normalized.pop();
    }

    *lines = normalized;
}

struct HighlightCache {
    entries: HashMap<u64, Vec<Line<'static>>>,
}

impl HighlightCache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    fn get(&self, hash: u64) -> Option<Vec<Line<'static>>> {
        self.entries.get(&hash).cloned()
    }

    fn insert(&mut self, hash: u64, lines: Vec<Line<'static>>) {
        // Evict if cache is too large
        if self.entries.len() >= HIGHLIGHT_CACHE_LIMIT {
            self.entries.clear();
        }
        self.entries.insert(hash, lines);
    }
}

fn hash_code(code: &str, lang: Option<&str>) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    let mut hasher = DefaultHasher::new();
    code.hash(&mut hasher);
    lang.hash(&mut hasher);
    hasher.finish()
}

/// Incremental markdown renderer for streaming content
///
/// This renderer caches previously rendered lines and only re-renders
/// the portion of text that has changed, significantly improving
/// performance during LLM streaming.
pub struct IncrementalMarkdownRenderer {
    /// Previously rendered lines
    rendered_lines: Vec<Line<'static>>,
    /// Text that was rendered (for comparison)
    rendered_text: String,
    /// Position of last safe checkpoint (after complete block)
    last_checkpoint: usize,
    /// Number of lines at last checkpoint
    lines_at_checkpoint: usize,
    /// Whether a blank separator should be preserved at the checkpoint boundary
    checkpoint_needs_separator: bool,
    /// Width constraint
    max_width: Option<usize>,
}

impl IncrementalMarkdownRenderer {
    pub fn new(max_width: Option<usize>) -> Self {
        Self {
            rendered_lines: Vec::new(),
            rendered_text: String::new(),
            last_checkpoint: 0,
            lines_at_checkpoint: 0,
            checkpoint_needs_separator: false,
            max_width,
        }
    }

    /// Update with new text, returns rendered lines
    ///
    /// This method efficiently handles streaming by:
    /// 1. Detecting if text was only appended (common case)
    /// 2. Finding safe re-render points (after complete blocks)
    /// 3. Only re-rendering from the last safe point
    pub fn update(&mut self, full_text: &str) -> Vec<Line<'static>> {
        with_streaming_render_context(|| self.update_internal(full_text))
    }

    pub fn debug_memory_profile(&self) -> serde_json::Value {
        let rendered_lines_estimate_bytes = estimate_lines_bytes(&self.rendered_lines);
        let rendered_text_bytes = self.rendered_text.capacity();
        serde_json::json!({
            "rendered_lines_count": self.rendered_lines.len(),
            "rendered_lines_estimate_bytes": rendered_lines_estimate_bytes,
            "rendered_text_bytes": rendered_text_bytes,
            "last_checkpoint": self.last_checkpoint,
            "lines_at_checkpoint": self.lines_at_checkpoint,
            "total_estimate_bytes": rendered_lines_estimate_bytes + rendered_text_bytes,
        })
    }

    fn update_internal(&mut self, full_text: &str) -> Vec<Line<'static>> {
        // Fast path: text unchanged
        if full_text == self.rendered_text {
            return self.rendered_lines.clone();
        }

        // Full re-render required.
        //
        // We previously tried to splice newly-appended markdown from a saved checkpoint,
        // but markdown block separators and list continuity make that unsafe without
        // carrying richer parser state across updates. In practice this caused transient
        // streaming artifacts like duplicated/misaligned content. Favor correctness here.
        self.rendered_lines = render_markdown_with_width(full_text, self.max_width);
        self.rendered_text = full_text.to_string();

        // Find checkpoint for next incremental update
        self.refresh_checkpoint(full_text, true);

        self.rendered_lines.clone()
    }

    /// Find the last complete block in text
    #[cfg(test)]
    fn find_last_complete_block(&self, text: &str) -> Option<usize> {
        self.find_last_complete_block_checkpoint(text)
            .map(|checkpoint| checkpoint.offset)
    }

    fn find_last_complete_block_checkpoint(&self, text: &str) -> Option<CompleteBlockCheckpoint> {
        let mut checkpoint = None;
        let mut line_start = 0usize;
        let mut fence_state: Option<(char, usize)> = None;
        let mut display_math_open = false;
        let mut last_nonblank_kind: Option<MarkdownBlockKind> = None;
        let spacing_mode = effective_markdown_spacing_mode();

        while line_start <= text.len() {
            let relative_end = text[line_start..].find('\n');
            let (line_end, line_ends_with_newline) = match relative_end {
                Some(end) => (line_start + end, true),
                None => (text.len(), false),
            };
            let line = &text[line_start..line_end];
            let line_end_including_newline = if line_ends_with_newline {
                line_end + 1
            } else {
                line_end
            };

            match fence_state {
                Some((fence_char, fence_len)) => {
                    if is_closing_fence(line, fence_char, fence_len) {
                        fence_state = None;
                        last_nonblank_kind = Some(MarkdownBlockKind::CodeBlock);
                        checkpoint = Some(CompleteBlockCheckpoint {
                            offset: line_end_including_newline,
                            needs_separator: spacing_separates_after(
                                MarkdownBlockKind::CodeBlock,
                                spacing_mode,
                            ),
                        });
                    }
                }
                None => {
                    if display_math_open {
                        let dd_count = count_unescaped_double_dollar(line);
                        if dd_count % 2 == 1 {
                            display_math_open = false;
                            last_nonblank_kind = Some(MarkdownBlockKind::DisplayMath);
                            checkpoint = Some(CompleteBlockCheckpoint {
                                offset: line_end_including_newline,
                                needs_separator: spacing_separates_after(
                                    MarkdownBlockKind::DisplayMath,
                                    spacing_mode,
                                ),
                            });
                        }
                    } else if let Some((fence_char, fence_len)) = parse_opening_fence(line) {
                        fence_state = Some((fence_char, fence_len));
                    } else {
                        let dd_count = count_unescaped_double_dollar(line);
                        if dd_count > 0 {
                            if dd_count % 2 == 1 {
                                display_math_open = true;
                            } else {
                                last_nonblank_kind = Some(MarkdownBlockKind::DisplayMath);
                                checkpoint = Some(CompleteBlockCheckpoint {
                                    offset: line_end_including_newline,
                                    needs_separator: spacing_separates_after(
                                        MarkdownBlockKind::DisplayMath,
                                        spacing_mode,
                                    ),
                                });
                            }
                        } else if line_ends_with_newline && is_heading_line(line.trim_start()) {
                            last_nonblank_kind = Some(MarkdownBlockKind::Heading);
                            checkpoint = Some(CompleteBlockCheckpoint {
                                offset: line_end_including_newline,
                                needs_separator: spacing_separates_after(
                                    MarkdownBlockKind::Heading,
                                    spacing_mode,
                                ),
                            });
                        } else if line.trim().is_empty() {
                            checkpoint = Some(CompleteBlockCheckpoint {
                                offset: line_end_including_newline,
                                needs_separator: last_nonblank_kind
                                    .map(|kind| spacing_separates_after(kind, spacing_mode))
                                    .unwrap_or(false),
                            });
                        } else {
                            last_nonblank_kind = Some(infer_markdown_line_kind(line));
                        }
                    }
                }
            }

            if !line_ends_with_newline {
                break;
            }
            line_start = line_end + 1;
        }

        checkpoint
    }

    /// Refresh checkpoint metadata from the latest rendered text.
    ///
    /// `force = true` recomputes prefix line counts even when checkpoint byte position is unchanged.
    fn refresh_checkpoint(&mut self, full_text: &str, force: bool) {
        let checkpoint = self.find_last_complete_block_checkpoint(full_text);
        let new_checkpoint = checkpoint.map(|cp| cp.offset).unwrap_or(0);
        let new_checkpoint_needs_separator =
            checkpoint.map(|cp| cp.needs_separator).unwrap_or(false);
        if !force
            && new_checkpoint == self.last_checkpoint
            && new_checkpoint_needs_separator == self.checkpoint_needs_separator
        {
            return;
        }

        self.last_checkpoint = new_checkpoint;
        self.checkpoint_needs_separator = new_checkpoint_needs_separator;
        if new_checkpoint == 0 {
            self.lines_at_checkpoint = 0;
        } else {
            let prefix_lines =
                render_markdown_with_width(&full_text[..new_checkpoint], self.max_width);
            self.lines_at_checkpoint = prefix_lines.len();
        }
    }

    /// Reset the renderer state
    pub fn reset(&mut self) {
        self.rendered_lines.clear();
        self.rendered_text.clear();
        self.last_checkpoint = 0;
        self.lines_at_checkpoint = 0;
        self.checkpoint_needs_separator = false;
    }

    /// Update width constraint, resets if changed
    pub fn set_width(&mut self, max_width: Option<usize>) {
        if self.max_width != max_width {
            self.max_width = max_width;
            self.reset();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CompleteBlockCheckpoint {
    offset: usize,
    needs_separator: bool,
}

fn is_heading_line(line: &str) -> bool {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    hashes > 0 && hashes <= 6 && line.chars().nth(hashes) == Some(' ')
}

fn is_thematic_break_line(line: &str) -> bool {
    let trimmed = line.trim();
    let mut marker: Option<char> = None;
    let mut count = 0usize;

    for ch in trimmed.chars() {
        if ch == ' ' || ch == '\t' {
            continue;
        }
        match marker {
            None if matches!(ch, '-' | '*' | '_') => {
                marker = Some(ch);
                count = 1;
            }
            Some(existing) if ch == existing => count += 1,
            _ => return false,
        }
    }

    count >= 3
}

fn looks_like_ordered_list_item(line: &str) -> bool {
    let trimmed = line.trim_start();
    let digit_count = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
    digit_count > 0
        && matches!(trimmed.chars().nth(digit_count), Some('.' | ')'))
        && matches!(trimmed.chars().nth(digit_count + 1), Some(' ' | '\t'))
}

fn infer_markdown_line_kind(line: &str) -> MarkdownBlockKind {
    let trimmed = line.trim_start();
    if is_heading_line(trimmed) {
        MarkdownBlockKind::Heading
    } else if is_thematic_break_line(trimmed) {
        MarkdownBlockKind::Rule
    } else if trimmed.starts_with('>') {
        MarkdownBlockKind::BlockQuote
    } else if trimmed.starts_with("- ")
        || trimmed.starts_with("* ")
        || trimmed.starts_with("+ ")
        || looks_like_ordered_list_item(trimmed)
    {
        MarkdownBlockKind::List
    } else if trimmed.starts_with('<') {
        MarkdownBlockKind::HtmlBlock
    } else {
        MarkdownBlockKind::Paragraph
    }
}

fn rendered_rule_width(max_width: Option<usize>) -> usize {
    match max_width {
        Some(width) if center_code_blocks() => width.min(RULE_LEN),
        Some(width) => width,
        None => RULE_LEN,
    }
}

// Colors matching ui.rs palette
use super::color_support::rgb;
fn code_bg() -> Color {
    rgb(45, 45, 45)
}
fn code_fg() -> Color {
    rgb(180, 180, 180)
}
fn math_fg() -> Color {
    rgb(130, 210, 235)
}
fn link_fg() -> Color {
    rgb(120, 180, 240)
}
fn html_fg() -> Color {
    rgb(140, 140, 150)
}
fn text_color() -> Color {
    rgb(200, 200, 195)
}
fn bold_color() -> Color {
    rgb(240, 240, 235)
}
fn heading_h1_color() -> Color {
    rgb(255, 215, 100)
}
fn heading_h2_color() -> Color {
    rgb(240, 190, 90)
}
fn heading_h3_color() -> Color {
    rgb(220, 170, 80)
}
fn heading_color() -> Color {
    rgb(200, 155, 75)
}
fn md_dim_color() -> Color {
    rgb(100, 100, 100)
}
const RULE_LEN: usize = 24;

#[derive(Debug, Clone)]
struct ListRenderState {
    ordered: bool,
    next_index: u64,
    item_line_starts: Vec<usize>,
    max_marker_digits: usize,
}

#[derive(Debug, Default)]
struct CenteredStructuredBlockState {
    depth: usize,
    start_line: Option<usize>,
    ranges: Vec<std::ops::Range<usize>>,
}

fn diagram_side_only() -> bool {
    matches!(effective_diagram_mode(), DiagramDisplayMode::Pinned)
}

fn mermaid_should_register_active() -> bool {
    !matches!(effective_diagram_mode(), DiagramDisplayMode::None)
}

fn mermaid_sidebar_placeholder(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::default().fg(md_dim_color()),
    ))
    .left_aligned()
}

fn apply_inline_decorations(mut style: Style, strike: bool, in_link: bool) -> Style {
    if strike {
        style = style.crossed_out();
    }
    if in_link {
        style = style.fg(link_fg()).underlined();
    }
    style
}

fn ensure_blockquote_prefix(current_spans: &mut Vec<Span<'static>>, blockquote_depth: usize) {
    if blockquote_depth == 0 || !current_spans.is_empty() {
        return;
    }
    let prefix = "│ ".repeat(blockquote_depth);
    current_spans.push(Span::styled(prefix, Style::default().fg(md_dim_color())));
}

fn with_blockquote_prefix(line: Line<'static>, blockquote_depth: usize) -> Line<'static> {
    if blockquote_depth == 0 {
        return line;
    }
    let mut spans = vec![Span::styled(
        "│ ".repeat(blockquote_depth),
        Style::default().fg(md_dim_color()),
    )];
    let alignment = line.alignment;
    spans.extend(line.spans);
    let line = Line::from(spans);
    match alignment {
        Some(align) => line.alignment(align),
        None => line.left_aligned(),
    }
}

fn flush_current_line_with_alignment(
    lines: &mut Vec<Line<'static>>,
    current_spans: &mut Vec<Span<'static>>,
    alignment: Option<Alignment>,
) {
    if !current_spans.is_empty() {
        let line = Line::from(std::mem::take(current_spans));
        lines.push(match alignment {
            Some(align) => line.alignment(align),
            None => line,
        });
    }
}

fn enter_centered_structured_block(state: &mut CenteredStructuredBlockState, current_line: usize) {
    if state.depth == 0 {
        state.start_line = Some(current_line);
    }
    state.depth = state.depth.saturating_add(1);
}

fn exit_centered_structured_block(state: &mut CenteredStructuredBlockState, current_line: usize) {
    if state.depth == 0 {
        return;
    }
    state.depth = state.depth.saturating_sub(1);
    if state.depth == 0
        && let Some(start) = state.start_line.take()
        && current_line > start
    {
        state.ranges.push(start..current_line);
    }
}

fn record_centered_standalone_block(
    state: &mut CenteredStructuredBlockState,
    start_line: usize,
    end_line: usize,
) {
    if state.depth == 0 && end_line > start_line {
        state.ranges.push(start_line..end_line);
    }
}

fn finalize_centered_structured_blocks(
    state: &mut CenteredStructuredBlockState,
    current_line: usize,
) {
    if state.depth > 0 {
        state.depth = 0;
        if let Some(start) = state.start_line.take()
            && current_line > start
        {
            state.ranges.push(start..current_line);
        }
    }
}

fn center_structured_block_ranges(
    lines: &mut [Line<'static>],
    width: usize,
    ranges: &[std::ops::Range<usize>],
) {
    if width == 0 {
        return;
    }

    for range in ranges {
        if range.start >= range.end || range.end > lines.len() {
            continue;
        }

        let run = &mut lines[range.start..range.end];
        let max_line_width = run
            .iter()
            .filter(|line| !line_is_blank(line))
            .map(Line::width)
            .max()
            .unwrap_or(0);
        let pad = width.saturating_sub(max_line_width) / 2;
        if pad > 0 {
            let pad_str = " ".repeat(pad);
            for line in run {
                if line_is_blank(line) {
                    continue;
                }
                line.spans.insert(0, Span::raw(pad_str.clone()));
                line.alignment = Some(Alignment::Left);
            }
        }
    }
}

fn leading_raw_padding_width(line: &Line<'_>) -> usize {
    line.spans
        .iter()
        .take_while(|span| {
            span.style == Style::default()
                && !span.content.is_empty()
                && span.content.chars().all(|ch| ch == ' ')
        })
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum()
}

fn strip_leading_raw_padding(line: &mut Line<'static>, trim_width: usize) {
    if trim_width == 0 {
        return;
    }

    let mut remaining = trim_width;
    while remaining > 0 && !line.spans.is_empty() {
        let span = &line.spans[0];
        let is_raw_padding = span.style == Style::default()
            && !span.content.is_empty()
            && span.content.chars().all(|ch| ch == ' ');
        if !is_raw_padding {
            break;
        }

        let span_width = UnicodeWidthStr::width(span.content.as_ref());
        if span_width <= remaining {
            line.spans.remove(0);
            remaining -= span_width;
            continue;
        }

        let keep = span_width.saturating_sub(remaining);
        line.spans[0].content = " ".repeat(keep).into();
        remaining = 0;
    }
}

fn blockquote_gutter_width(text: &str) -> (usize, &str) {
    let mut rest = text;
    let mut width = 0usize;
    while let Some(next) = rest.strip_prefix("│ ") {
        width += UnicodeWidthStr::width("│ ");
        rest = next;
    }
    (width, rest)
}

fn ordered_marker_components(text: &str) -> Option<(usize, usize)> {
    let indent_width = text.chars().take_while(|ch| *ch == ' ').count();
    let suffix = text.get(indent_width..)?;
    let digit_count = suffix.chars().take_while(|ch| ch.is_ascii_digit()).count();
    if digit_count == 0 {
        return None;
    }
    let rest = suffix.get(digit_count..)?;
    rest.strip_prefix(". ")?;
    Some((indent_width, digit_count))
}

fn ordered_marker_info(line: &Line<'_>) -> Option<(usize, usize, usize)> {
    let plain = line_plain_text(line);
    let leading_width = plain
        .chars()
        .take_while(|ch: &char| ch.is_whitespace())
        .count();
    let rest = plain.get(leading_width..)?;
    let (gutter_width, rest) = blockquote_gutter_width(rest);
    let (indent_width, digit_count) = ordered_marker_components(rest)?;
    Some((leading_width + gutter_width, indent_width, digit_count))
}

fn pad_ordered_marker_line(
    line: &mut Line<'static>,
    marker_prefix_width: usize,
    indent_width: usize,
    extra_pad: usize,
) {
    if extra_pad == 0 {
        return;
    }

    let mut consumed_width = 0usize;
    for span in &mut line.spans {
        let span_width = UnicodeWidthStr::width(span.content.as_ref());
        if consumed_width + span_width <= marker_prefix_width {
            consumed_width += span_width;
            continue;
        }

        let content = span.content.as_ref();
        let indent_prefix = " ".repeat(indent_width);
        if let Some(rest) = content.strip_prefix(&indent_prefix) {
            let digit_count = rest.chars().take_while(|ch| ch.is_ascii_digit()).count();
            if digit_count > 0 {
                let mut updated = indent_prefix;
                updated.push_str(&" ".repeat(extra_pad));
                updated.push_str(rest);
                span.content = updated.into();
            }
        }
        break;
    }
}

fn align_ordered_list_markers(
    lines: &mut [Line<'static>],
    item_starts: &[usize],
    max_digits: usize,
) {
    if max_digits <= 1 {
        return;
    }

    for &line_idx in item_starts {
        let Some(line) = lines.get_mut(line_idx) else {
            continue;
        };
        let Some((marker_prefix_width, indent_width, digit_count)) = ordered_marker_info(line)
        else {
            continue;
        };
        let extra_pad = max_digits.saturating_sub(digit_count);
        pad_ordered_marker_line(line, marker_prefix_width, indent_width, extra_pad);
    }
}

pub(crate) fn recenter_structured_blocks_for_display(lines: &mut [Line<'static>], width: usize) {
    if width == 0 {
        return;
    }

    let mut idx = 0usize;
    while idx < lines.len() {
        let is_structured =
            !line_is_blank(&lines[idx]) && lines[idx].alignment == Some(Alignment::Left);
        if !is_structured {
            idx += 1;
            continue;
        }

        let start = idx;
        while idx < lines.len()
            && !line_is_blank(&lines[idx])
            && lines[idx].alignment == Some(Alignment::Left)
        {
            idx += 1;
        }

        let run = &mut lines[start..idx];
        let common_pad = run.iter().map(leading_raw_padding_width).min().unwrap_or(0);
        if common_pad > 0 {
            for line in run.iter_mut() {
                strip_leading_raw_padding(line, common_pad);
            }
        }

        let max_line_width = run.iter().map(Line::width).max().unwrap_or(0);
        let pad = width.saturating_sub(max_line_width) / 2;
        if pad > 0 {
            let pad_str = " ".repeat(pad);
            for line in run.iter_mut() {
                line.spans.insert(0, Span::raw(pad_str.clone()));
                line.alignment = Some(Alignment::Left);
            }
        }
    }
}

fn structured_markdown_alignment(
    blockquote_depth: usize,
    list_stack: &[ListRenderState],
    in_definition_list: bool,
    in_footnote_definition: bool,
) -> Option<Alignment> {
    if blockquote_depth > 0
        || !list_stack.is_empty()
        || in_definition_list
        || in_footnote_definition
    {
        Some(Alignment::Left)
    } else {
        None
    }
}

fn parse_opening_fence(line: &str) -> Option<(char, usize)> {
    let indent = line.chars().take_while(|c| *c == ' ').count();
    if indent > 3 {
        return None;
    }
    let trimmed = &line[indent..];
    let first = trimmed.chars().next()?;
    if first != '`' && first != '~' {
        return None;
    }

    let fence_len = trimmed.chars().take_while(|c| *c == first).count();
    if fence_len < 3 {
        return None;
    }

    Some((first, fence_len))
}

fn is_closing_fence(line: &str, fence_char: char, min_len: usize) -> bool {
    let indent = line.chars().take_while(|c| *c == ' ').count();
    if indent > 3 {
        return false;
    }
    let trimmed = &line[indent..];

    let fence_len = trimmed.chars().take_while(|c| *c == fence_char).count();
    if fence_len < min_len {
        return false;
    }

    trimmed[fence_len..].trim().is_empty()
}

fn count_unescaped_double_dollar(line: &str) -> usize {
    let bytes = line.as_bytes();
    let mut count = 0usize;
    let mut ix = 0usize;

    while ix + 1 < bytes.len() {
        if bytes[ix] == b'\\' {
            ix += 2;
            continue;
        }
        if bytes[ix] == b'$' && bytes[ix + 1] == b'$' {
            count += 1;
            ix += 2;
            continue;
        }
        ix += 1;
    }

    count
}

fn math_inline_span(math: &str) -> Span<'static> {
    Span::styled(format!("${}$", math), Style::default().fg(math_fg()))
}

fn math_display_lines(math: &str) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    let dim = Style::default().fg(md_dim_color());
    out.push(Line::from(Span::styled("┌─ math ", dim)).left_aligned());
    for line in math.lines() {
        out.push(
            Line::from(vec![
                Span::styled("│ ", dim),
                Span::styled(line.to_string(), Style::default().fg(math_fg())),
            ])
            .left_aligned(),
        );
    }
    if math.is_empty() {
        out.push(
            Line::from(vec![
                Span::styled("│ ", dim),
                Span::styled("", Style::default().fg(math_fg())),
            ])
            .left_aligned(),
        );
    }
    out.push(Line::from(Span::styled("└─", dim)).left_aligned());
    out
}
fn table_color() -> Color {
    rgb(150, 150, 150)
}

/// Render markdown text to styled ratatui Lines
pub fn render_markdown(text: &str) -> Vec<Line<'static>> {
    render_markdown_with_width(text, None)
}

/// Escape dollar signs that look like currency amounts so the math parser
/// doesn't swallow them.  Currency: `$` followed by a digit (e.g. `$35`,
/// `$5.99`).  We turn those into `\$` which pulldown-cmark passes through
/// as literal text rather than starting an inline-math span.
///
/// We skip dollars inside code spans/fences and already-escaped `\$`.
fn escape_currency_dollars(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut in_code_fence = false;
    let mut inline_code_len: usize = 0;
    let mut at_line_start = true;
    let mut leading_spaces = 0;

    let count_backticks = |chars: &[char], start: usize| {
        let mut j = start;
        while j < chars.len() && chars[j] == '`' {
            j += 1;
        }
        j - start
    };

    let is_escaped = |chars: &[char], pos: usize| {
        let mut backslashes = 0usize;
        let mut j = pos;
        while j > 0 {
            if chars[j - 1] != '\\' {
                break;
            }
            backslashes += 1;
            j -= 1;
        }
        backslashes % 2 == 1
    };

    while i < len {
        let c = chars[i];

        if c == '\n' {
            at_line_start = true;
            leading_spaces = 0;
            out.push('\n');
            i += 1;
            continue;
        }

        if at_line_start && (c == ' ' || c == '\t') {
            leading_spaces += 1;
            out.push(c);
            i += 1;
            continue;
        }

        let maybe_fence = inline_code_len == 0 && c == '`' && count_backticks(&chars, i) >= 3;
        if maybe_fence && at_line_start && leading_spaces <= 3 {
            let run = count_backticks(&chars, i);
            for _ in 0..run {
                out.push('`');
            }
            i += run;
            in_code_fence = !in_code_fence;
            at_line_start = false;
            leading_spaces = 0;
            continue;
        }

        if c == '`' {
            let run = count_backticks(&chars, i);
            if inline_code_len > 0 {
                if run == inline_code_len {
                    inline_code_len = 0;
                }
                for _ in 0..run {
                    out.push('`');
                }
                i += run;
                at_line_start = false;
                leading_spaces = 0;
                continue;
            }

            inline_code_len = run;
            for _ in 0..run {
                out.push('`');
            }
            i += run;
            at_line_start = false;
            leading_spaces = 0;
            continue;
        }

        if at_line_start {
            at_line_start = false;
        }

        if c == ' ' || c == '\t' {
            out.push(c);
            i += 1;
            continue;
        }

        if in_code_fence || inline_code_len > 0 {
            out.push(c);
            i += 1;
            continue;
        }

        if c == '$' && i + 1 < len && chars[i + 1] == '$' {
            out.push_str("$$");
            i += 2;
            continue;
        }

        if c == '$' && i + 1 < len && chars[i + 1].is_ascii_digit() {
            if is_escaped(&chars, i) {
                out.push('$');
            } else {
                out.push_str("\\$");
            }
            i += 1;
            continue;
        }

        out.push(c);
        i += 1;
    }
    out
}

fn looks_like_line_oriented_transcript_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return false;
    }

    if trimmed.starts_with("tool:")
        || trimmed.starts_with("tools:")
        || trimmed.starts_with("broadcast from ")
    {
        return true;
    }

    matches!(trimmed.chars().next(), Some('✓' | '✗' | '┌' | '│' | '└'))
}

fn preserve_line_oriented_softbreaks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let lines: Vec<&str> = text.split('\n').collect();
    let mut in_code_fence = false;
    let mut fence_char = '\0';
    let mut fence_len = 0usize;

    for (idx, line) in lines.iter().enumerate() {
        let prev_line = idx.checked_sub(1).map(|prev| lines[prev]);
        let prev_log_like = prev_line.is_some_and(looks_like_line_oriented_transcript_line);
        let next_log_like =
            idx + 1 < lines.len() && looks_like_line_oriented_transcript_line(lines[idx + 1]);
        let line_log_like = looks_like_line_oriented_transcript_line(line);
        let entering_log_block = !in_code_fence
            && line_log_like
            && !prev_log_like
            && prev_line.is_some_and(|prev| !prev.trim().is_empty());
        let leaving_log_block = !in_code_fence
            && line_log_like
            && !next_log_like
            && idx + 1 < lines.len()
            && !lines[idx + 1].trim().is_empty();
        let preserve_softbreak = !in_code_fence && line_log_like && next_log_like;

        if entering_log_block && !out.ends_with("\n\n") {
            out.push('\n');
        }

        out.push_str(line);
        if idx + 1 < lines.len() {
            if preserve_softbreak && !line.ends_with("  ") {
                out.push_str("  ");
            }
            out.push('\n');
            if leaving_log_block {
                out.push('\n');
            }
        }

        if in_code_fence {
            if is_closing_fence(line, fence_char, fence_len) {
                in_code_fence = false;
                fence_char = '\0';
                fence_len = 0;
            }
        } else if let Some((marker, min_len)) = parse_opening_fence(line) {
            in_code_fence = true;
            fence_char = marker;
            fence_len = min_len;
        }
    }

    out
}

pub fn debug_stats() -> MarkdownDebugStats {
    if let Ok(state) = MARKDOWN_DEBUG.lock() {
        return state.stats.clone();
    }
    MarkdownDebugStats::default()
}

pub fn debug_memory_profile() -> MarkdownMemoryProfile {
    let process = crate::process_memory::snapshot_with_source("client:markdown:memory");
    let mut profile = MarkdownMemoryProfile {
        process_rss_bytes: process.rss_bytes,
        process_peak_rss_bytes: process.peak_rss_bytes,
        process_virtual_bytes: process.virtual_bytes,
        highlight_cache_limit: HIGHLIGHT_CACHE_LIMIT,
        ..MarkdownMemoryProfile::default()
    };

    if let Ok(cache) = HIGHLIGHT_CACHE.lock() {
        profile.highlight_cache_entries = cache.entries.len();
        for lines in cache.entries.values() {
            profile.highlight_cache_lines += lines.len();
            profile.highlight_cache_estimate_bytes += estimate_lines_bytes(lines);
            for line in lines {
                profile.highlight_cache_spans += line.spans.len();
                profile.highlight_cache_text_bytes += line
                    .spans
                    .iter()
                    .map(|span| span.content.len())
                    .sum::<usize>();
            }
        }
    }

    profile
}

pub fn reset_debug_stats() {
    if let Ok(mut state) = MARKDOWN_DEBUG.lock() {
        state.stats = MarkdownDebugStats::default();
    }
}

fn estimate_lines_bytes(lines: &[Line<'static>]) -> usize {
    lines
        .iter()
        .map(|line| {
            std::mem::size_of::<Line<'static>>()
                + line.spans.len() * std::mem::size_of::<Span<'static>>()
                + line
                    .spans
                    .iter()
                    .map(|span| span.content.len())
                    .sum::<usize>()
        })
        .sum()
}

pub fn debug_stats_json() -> Option<serde_json::Value> {
    serde_json::to_value(debug_stats()).ok()
}

/// Render markdown with optional width constraint for tables

pub(crate) fn wrap_line(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    wrap::wrap_line(line, width, repeated_gutter_prefix)
}

pub fn wrap_lines(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    wrap::wrap_lines(lines, width, repeated_gutter_prefix)
}

pub fn progress_bar(progress: f32, width: usize) -> String {
    wrap::progress_bar(progress, width)
}

pub fn progress_line(label: &str, progress: f32, width: usize) -> Line<'static> {
    wrap::progress_line(label, progress, width)
}

#[cfg(test)]
#[path = "markdown_tests/mod.rs"]
mod tests;
