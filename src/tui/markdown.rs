#![allow(dead_code)]

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::prelude::*;
use serde::Serialize;
use std::cell::Cell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{LazyLock, Mutex};
use std::time::Instant;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style as SynStyle, ThemeSet};
use syntect::parsing::SyntaxSet;
use unicode_width::UnicodeWidthStr;

use crate::config::{DiagramDisplayMode, MarkdownSpacingMode, config};
use crate::tui::mermaid;
use crate::tui::ui::{CopyTargetKind, RawCopyTarget};

// Syntax highlighting resources (loaded once)
static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(|| SyntaxSet::load_defaults_newlines());
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

#[derive(Debug, Clone, Default)]
struct MarkdownDebugState {
    stats: MarkdownDebugStats,
}

static MARKDOWN_DEBUG: LazyLock<Mutex<MarkdownDebugState>> =
    LazyLock::new(|| Mutex::new(MarkdownDebugState::default()));

static DIAGRAM_MODE_OVERRIDE: LazyLock<Mutex<Option<DiagramDisplayMode>>> =
    LazyLock::new(|| Mutex::new(None));

thread_local! {
    /// Whether markdown rendering is running in streaming mode.
    /// In this mode mermaid diagrams update an ephemeral side-panel preview
    /// instead of being persisted in ACTIVE_DIAGRAMS history.
    static STREAMING_RENDER_CONTEXT: Cell<bool> = const { Cell::new(false) };
    /// Whether code blocks should be horizontally centered within available width.
    /// Set to true in centered mode, false in left-aligned mode.
    static CENTER_CODE_BLOCKS: Cell<bool> = const { Cell::new(true) };
    /// Optional test/debug override for markdown spacing mode.
    static MARKDOWN_SPACING_MODE_OVERRIDE: Cell<Option<MarkdownSpacingMode>> = const { Cell::new(None) };
}

pub fn set_diagram_mode_override(mode: Option<DiagramDisplayMode>) {
    if let Ok(mut override_mode) = DIAGRAM_MODE_OVERRIDE.lock() {
        *override_mode = mode;
    }
}

pub fn get_diagram_mode_override() -> Option<DiagramDisplayMode> {
    DIAGRAM_MODE_OVERRIDE.lock().ok().and_then(|mode| *mode)
}

fn effective_diagram_mode() -> DiagramDisplayMode {
    if let Ok(mode) = DIAGRAM_MODE_OVERRIDE.lock() {
        if let Some(override_mode) = *mode {
            return override_mode;
        }
    }
    config().display.diagram_mode
}

fn effective_markdown_spacing_mode() -> MarkdownSpacingMode {
    MARKDOWN_SPACING_MODE_OVERRIDE
        .with(|mode| mode.get().unwrap_or(config().display.markdown_spacing))
}

fn with_markdown_spacing_mode_override<T>(
    mode: Option<MarkdownSpacingMode>,
    f: impl FnOnce() -> T,
) -> T {
    MARKDOWN_SPACING_MODE_OVERRIDE.with(|ctx| {
        let prev = ctx.replace(mode);
        struct ResetGuard<'a> {
            cell: &'a Cell<Option<MarkdownSpacingMode>>,
            prev: Option<MarkdownSpacingMode>,
        }
        impl Drop for ResetGuard<'_> {
            fn drop(&mut self) {
                self.cell.set(self.prev);
            }
        }
        let _guard = ResetGuard { cell: ctx, prev };
        f()
    })
}

fn with_streaming_render_context<T>(f: impl FnOnce() -> T) -> T {
    STREAMING_RENDER_CONTEXT.with(|ctx| {
        let prev = ctx.replace(true);
        struct ResetGuard<'a> {
            cell: &'a Cell<bool>,
            prev: bool,
        }
        impl Drop for ResetGuard<'_> {
            fn drop(&mut self) {
                self.cell.set(self.prev);
            }
        }
        let _guard = ResetGuard { cell: ctx, prev };
        f()
    })
}

fn streaming_render_context_enabled() -> bool {
    STREAMING_RENDER_CONTEXT.with(|ctx| ctx.get())
}

pub fn set_center_code_blocks(centered: bool) {
    CENTER_CODE_BLOCKS.with(|ctx| ctx.set(centered));
}

pub fn center_code_blocks() -> bool {
    CENTER_CODE_BLOCKS.with(|ctx| ctx.get())
}

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

    fn update_internal(&mut self, full_text: &str) -> Vec<Line<'static>> {
        // Fast path: text unchanged
        if full_text == self.rendered_text {
            return self.rendered_lines.clone();
        }

        // Fast path: text was only appended
        if full_text.starts_with(&self.rendered_text) {
            // Find a safe re-render point
            // Safe points are after: double newlines (paragraph end), code block end
            let rerender_from = self.find_safe_rerender_point();

            if rerender_from >= self.last_checkpoint {
                // Re-render from the safe point
                let text_to_render = &full_text[rerender_from..];
                let new_lines = render_markdown_with_width(text_to_render, self.max_width);

                // Keep lines up to checkpoint, append new lines
                self.rendered_lines.truncate(self.lines_at_checkpoint);
                self.rendered_lines.extend(new_lines);

                // Update checkpoint only at markdown-safe boundaries.
                // This prevents checkpointing inside fenced code blocks during streaming.
                self.refresh_checkpoint(full_text, false);

                self.rendered_text = full_text.to_string();
                return self.rendered_lines.clone();
            }
        }

        // Slow path: text changed in middle or was truncated
        // Full re-render required
        self.rendered_lines = render_markdown_with_width(full_text, self.max_width);
        self.rendered_text = full_text.to_string();

        // Find checkpoint for next incremental update
        self.refresh_checkpoint(full_text, true);

        self.rendered_lines.clone()
    }

    /// Find a safe point to start re-rendering from
    fn find_safe_rerender_point(&self) -> usize {
        // Start from the last checkpoint
        self.last_checkpoint
    }

    /// Find the last complete block in text
    fn find_last_complete_block(&self, text: &str) -> Option<usize> {
        let mut checkpoint = None;
        let mut line_start = 0usize;
        let mut fence_state: Option<(char, usize)> = None;
        let mut display_math_open = false;

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
                        checkpoint = Some(line_end_including_newline);
                    }
                }
                None => {
                    if display_math_open {
                        let dd_count = count_unescaped_double_dollar(line);
                        if dd_count % 2 == 1 {
                            display_math_open = false;
                            checkpoint = Some(line_end_including_newline);
                        }
                    } else if let Some((fence_char, fence_len)) = parse_opening_fence(line) {
                        fence_state = Some((fence_char, fence_len));
                    } else {
                        let dd_count = count_unescaped_double_dollar(line);
                        if dd_count > 0 {
                            if dd_count % 2 == 1 {
                                display_math_open = true;
                            } else {
                                checkpoint = Some(line_end_including_newline);
                            }
                        } else if line.trim().is_empty() {
                            checkpoint = Some(line_end_including_newline);
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
        let new_checkpoint = self.find_last_complete_block(full_text).unwrap_or(0);
        if !force && new_checkpoint == self.last_checkpoint {
            return;
        }

        self.last_checkpoint = new_checkpoint;
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
    }

    /// Update width constraint, resets if changed
    pub fn set_width(&mut self, max_width: Option<usize>) {
        if self.max_width != max_width {
            self.max_width = max_width;
            self.reset();
        }
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

#[derive(Debug, Clone, Copy)]
struct ListRenderState {
    ordered: bool,
    next_index: u64,
}

fn diagram_side_only() -> bool {
    matches!(effective_diagram_mode(), DiagramDisplayMode::Pinned)
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

fn flush_current_line(lines: &mut Vec<Line<'static>>, current_spans: &mut Vec<Span<'static>>) {
    if !current_spans.is_empty() {
        lines.push(Line::from(std::mem::take(current_spans)));
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

fn center_left_aligned_runs(lines: &mut [Line<'static>], width: usize) {
    if width == 0 {
        return;
    }

    let mut run_start: Option<usize> = None;
    for idx in 0..=lines.len() {
        let in_run = idx < lines.len()
            && lines[idx].alignment == Some(Alignment::Left)
            && !lines[idx].spans.is_empty();

        match (run_start, in_run) {
            (None, true) => run_start = Some(idx),
            (Some(start), false) => {
                let run = &mut lines[start..idx];
                let max_line_width = run.iter().map(Line::width).max().unwrap_or(0);
                let pad = width.saturating_sub(max_line_width) / 2;
                if pad > 0 {
                    let pad_str = " ".repeat(pad);
                    for line in run {
                        line.spans.insert(0, Span::raw(pad_str.clone()));
                    }
                }
                run_start = None;
            }
            _ => {}
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

pub fn debug_stats() -> MarkdownDebugStats {
    if let Ok(state) = MARKDOWN_DEBUG.lock() {
        return state.stats.clone();
    }
    MarkdownDebugStats::default()
}

pub fn debug_stats_json() -> Option<serde_json::Value> {
    serde_json::to_value(debug_stats()).ok()
}

/// Render markdown with optional width constraint for tables
pub fn render_markdown_with_width(text: &str, max_width: Option<usize>) -> Vec<Line<'static>> {
    let render_start = Instant::now();
    let text = escape_currency_dollars(text);
    let text = text.as_str();
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current_spans: Vec<Span<'static>> = Vec::new();
    let side_only = diagram_side_only();
    let streaming_mode = streaming_render_context_enabled();
    let spacing_mode = effective_markdown_spacing_mode();

    // Style stack for nested formatting
    let mut bold = false;
    let mut italic = false;
    let mut strike = false;
    let mut in_code_block = false;
    let mut code_block_lang: Option<String> = None;
    let mut code_block_content = String::new();
    let mut heading_level: Option<u8> = None;
    let mut blockquote_depth = 0usize;
    let mut list_stack: Vec<ListRenderState> = Vec::new();
    let mut link_targets: Vec<String> = Vec::new();
    let mut in_image = false;
    let mut image_url: Option<String> = None;
    let mut image_alt = String::new();
    let mut in_definition_list = false;
    let mut in_definition_item = false;
    let mut in_footnote_definition = false;

    // Table state
    let mut in_table = false;
    let mut table_row: Vec<String> = Vec::new();
    let mut table_rows: Vec<Vec<String>> = Vec::new();
    let mut current_cell = String::new();
    let mut _is_header_row = false;

    // Enable table parsing
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_MATH);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_GFM);
    options.insert(Options::ENABLE_DEFINITION_LIST);
    options.insert(Options::ENABLE_SMART_PUNCTUATION);
    let parser = Parser::new_ext(text, options);

    // Debug counters
    let mut dbg_headings = 0usize;
    let mut dbg_code_blocks = 0usize;
    let mut dbg_mermaid_blocks = 0usize;
    let mut dbg_tables = 0usize;
    let mut dbg_list_items = 0usize;
    let mut dbg_blockquotes = 0usize;

    for event in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                dbg_headings += 1;
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                heading_level = Some(level as u8);
            }
            Event::End(TagEnd::Heading(_)) => {
                if !current_spans.is_empty() {
                    // Choose color based on heading level
                    let color = match heading_level {
                        Some(1) => heading_h1_color(),
                        Some(2) => heading_h2_color(),
                        Some(3) => heading_h3_color(),
                        _ => heading_color(),
                    };

                    let heading_spans: Vec<Span<'static>> = current_spans
                        .drain(..)
                        .map(|s| {
                            Span::styled(s.content.to_string(), Style::default().fg(color).bold())
                        })
                        .collect();
                    lines.push(Line::from(heading_spans));
                    push_block_separator(&mut lines, MarkdownBlockKind::Heading, spacing_mode);
                }
                heading_level = None;
            }

            Event::Start(Tag::Strong) => bold = true,
            Event::End(TagEnd::Strong) => bold = false,

            Event::Start(Tag::Emphasis) => italic = true,
            Event::End(TagEnd::Emphasis) => italic = false,

            Event::Start(Tag::Strikethrough) => strike = true,
            Event::End(TagEnd::Strikethrough) => strike = false,

            Event::Start(Tag::BlockQuote(_)) => {
                dbg_blockquotes += 1;
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                blockquote_depth += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                blockquote_depth = blockquote_depth.saturating_sub(1);
                if blockquote_depth == 0
                    && list_stack.is_empty()
                    && !in_definition_list
                    && !in_footnote_definition
                {
                    push_block_separator(&mut lines, MarkdownBlockKind::BlockQuote, spacing_mode);
                }
            }

            Event::Start(Tag::List(start)) => {
                let state = ListRenderState {
                    ordered: start.is_some(),
                    next_index: start.unwrap_or(1),
                };
                list_stack.push(state);
            }
            Event::End(TagEnd::List(_)) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                list_stack.pop();
                if blockquote_depth == 0
                    && list_stack.is_empty()
                    && !in_definition_list
                    && !in_footnote_definition
                {
                    push_block_separator(&mut lines, MarkdownBlockKind::List, spacing_mode);
                }
            }

            Event::Start(Tag::Link { dest_url, .. }) => {
                link_targets.push(dest_url.to_string());
            }
            Event::End(TagEnd::Link) => {
                if let Some(url) = link_targets.pop() {
                    if !url.is_empty() {
                        current_spans.push(Span::styled(
                            format!(" ({})", url),
                            Style::default().fg(md_dim_color()),
                        ));
                    }
                }
            }

            Event::Start(Tag::Image { dest_url, .. }) => {
                in_image = true;
                image_url = Some(dest_url.to_string());
                image_alt.clear();
            }
            Event::End(TagEnd::Image) => {
                let alt = if image_alt.trim().is_empty() {
                    "image".to_string()
                } else {
                    image_alt.trim().to_string()
                };
                let label = if let Some(url) = image_url.take() {
                    format!("[image: {}] ({})", alt, url)
                } else {
                    format!("[image: {}]", alt)
                };
                if in_table {
                    current_cell.push_str(&label);
                } else {
                    ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                    current_spans.push(Span::styled(label, Style::default().fg(md_dim_color())));
                }
                in_image = false;
                image_alt.clear();
            }

            Event::Start(Tag::FootnoteDefinition(label)) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                in_footnote_definition = true;
                ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                current_spans.push(Span::styled(
                    format!("[^{}]: ", label),
                    Style::default().fg(md_dim_color()),
                ));
            }
            Event::End(TagEnd::FootnoteDefinition) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                in_footnote_definition = false;
            }

            Event::Start(Tag::DefinitionList) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                in_definition_list = true;
            }
            Event::End(TagEnd::DefinitionList) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                in_definition_list = false;
                if blockquote_depth == 0 && list_stack.is_empty() && !in_footnote_definition {
                    push_block_separator(
                        &mut lines,
                        MarkdownBlockKind::DefinitionList,
                        spacing_mode,
                    );
                }
            }
            Event::Start(Tag::DefinitionListTitle) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                current_spans.push(Span::styled("• ", Style::default().fg(md_dim_color())));
            }
            Event::End(TagEnd::DefinitionListTitle) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
            }
            Event::Start(Tag::DefinitionListDefinition) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                current_spans.push(Span::styled("  -> ", Style::default().fg(md_dim_color())));
                in_definition_item = true;
            }
            Event::End(TagEnd::DefinitionListDefinition) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                in_definition_item = false;
            }

            Event::Start(Tag::CodeBlock(kind)) => {
                dbg_code_blocks += 1;
                // Flush current line before code block
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                in_code_block = true;
                code_block_lang = match kind {
                    CodeBlockKind::Fenced(lang) if !lang.is_empty() => Some(lang.to_string()),
                    _ => None,
                };
                // Don't add header here - we'll add it at the end when we know the block width
                code_block_content.clear();
            }
            Event::End(TagEnd::CodeBlock) => {
                // Check if this is a mermaid diagram
                let is_mermaid = code_block_lang
                    .as_ref()
                    .map(|l| mermaid::is_mermaid_lang(l))
                    .unwrap_or(false);

                if is_mermaid {
                    dbg_mermaid_blocks += 1;
                    // Render mermaid diagram.
                    // In streaming mode this updates only the ephemeral preview entry.
                    let terminal_width = max_width.and_then(|w| u16::try_from(w).ok());
                    let result = if streaming_mode {
                        mermaid::render_mermaid_untracked(&code_block_content, terminal_width)
                    } else {
                        mermaid::render_mermaid_sized(&code_block_content, terminal_width)
                    };
                    if streaming_mode {
                        if let mermaid::RenderResult::Image {
                            hash,
                            width,
                            height,
                            ..
                        } = &result
                        {
                            mermaid::set_streaming_preview_diagram(*hash, *width, *height, None);
                        }
                    }
                    match result {
                        mermaid::RenderResult::Image { .. } if side_only => {
                            lines.push(mermaid_sidebar_placeholder("↗ mermaid diagram (sidebar)"));
                        }
                        other => {
                            let mermaid_lines = mermaid::result_to_lines(other, max_width);
                            lines.extend(mermaid_lines);
                        }
                    }
                } else {
                    // Render code block with syntax highlighting (cached)
                    let highlighted =
                        highlight_code_cached(&code_block_content, code_block_lang.as_deref());

                    let lang_label = code_block_lang.as_deref().unwrap_or("");
                    // Add header
                    lines.push(
                        Line::from(Span::styled(
                            format!("┌─ {} ", lang_label),
                            Style::default().fg(md_dim_color()),
                        ))
                        .left_aligned(),
                    );

                    // Add code lines
                    for hl_line in highlighted {
                        let mut spans =
                            vec![Span::styled("│ ", Style::default().fg(md_dim_color()))];
                        spans.extend(hl_line.spans);
                        lines.push(Line::from(spans).left_aligned());
                    }

                    // Add footer
                    lines.push(
                        Line::from(Span::styled("└─", Style::default().fg(md_dim_color())))
                            .left_aligned(),
                    );
                }
                in_code_block = false;
                code_block_lang = None;
                code_block_content.clear();
                if blockquote_depth == 0
                    && list_stack.is_empty()
                    && !in_definition_list
                    && !in_footnote_definition
                {
                    push_block_separator(&mut lines, MarkdownBlockKind::CodeBlock, spacing_mode);
                }
            }

            Event::Code(code) => {
                if in_image {
                    image_alt.push_str(&code);
                    continue;
                }
                // Inline code - handle differently in tables vs regular text
                if in_table {
                    current_cell.push_str(&code);
                } else {
                    ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                    current_spans.push(Span::styled(
                        code.to_string(),
                        apply_inline_decorations(
                            Style::default().fg(code_fg()).bg(code_bg()),
                            strike,
                            !link_targets.is_empty(),
                        ),
                    ));
                }
            }

            Event::InlineMath(math) => {
                if in_image {
                    image_alt.push('$');
                    image_alt.push_str(&math);
                    image_alt.push('$');
                    continue;
                }
                if in_table {
                    current_cell.push('$');
                    current_cell.push_str(&math);
                    current_cell.push('$');
                } else {
                    ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                    current_spans.push(math_inline_span(&math));
                }
            }

            Event::DisplayMath(math) => {
                if in_image {
                    image_alt.push_str("$$");
                    image_alt.push_str(&math);
                    image_alt.push_str("$$");
                    continue;
                }
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                if in_table {
                    current_cell.push_str("$$");
                    current_cell.push_str(&math);
                    current_cell.push_str("$$");
                } else {
                    for line in math_display_lines(&math) {
                        lines.push(with_blockquote_prefix(line, blockquote_depth));
                    }
                    if blockquote_depth == 0
                        && list_stack.is_empty()
                        && !in_definition_list
                        && !in_footnote_definition
                    {
                        push_block_separator(
                            &mut lines,
                            MarkdownBlockKind::DisplayMath,
                            spacing_mode,
                        );
                    }
                }
            }

            Event::Text(text) => {
                if in_code_block {
                    code_block_content.push_str(&text);
                } else if in_image {
                    image_alt.push_str(&text);
                } else if in_table {
                    current_cell.push_str(&text);
                } else {
                    // Check for "Thought for X.Xs" pattern and render dimmed
                    let is_thinking_duration =
                        text.starts_with("Thought for ") && text.ends_with('s');
                    let mut style = if is_thinking_duration {
                        Style::default().fg(md_dim_color()).italic()
                    } else {
                        match (bold, italic) {
                            (true, true) => Style::default().fg(bold_color()).bold().italic(),
                            (true, false) => Style::default().fg(bold_color()).bold(),
                            (false, true) => Style::default().fg(text_color()).italic(),
                            (false, false) => Style::default().fg(text_color()),
                        }
                    };
                    style = apply_inline_decorations(style, strike, !link_targets.is_empty());
                    ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                    current_spans.push(Span::styled(text.to_string(), style));
                }
            }

            Event::SoftBreak => {
                if in_image {
                    image_alt.push(' ');
                } else if !in_code_block {
                    current_spans.push(Span::raw(" "));
                }
            }
            Event::HardBreak => {
                if in_image {
                    image_alt.push(' ');
                } else if !in_code_block {
                    flush_current_line_with_alignment(
                        &mut lines,
                        &mut current_spans,
                        structured_markdown_alignment(
                            blockquote_depth,
                            &list_stack,
                            in_definition_list,
                            in_footnote_definition,
                        ),
                    );
                }
            }

            Event::Rule => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                let width = max_width.unwrap_or(RULE_LEN);
                let rule = Span::styled("─".repeat(width), Style::default().fg(md_dim_color()));
                lines.push(with_blockquote_prefix(
                    Line::from(rule).left_aligned(),
                    blockquote_depth,
                ));
                if blockquote_depth == 0
                    && list_stack.is_empty()
                    && !in_definition_list
                    && !in_footnote_definition
                {
                    push_block_separator(&mut lines, MarkdownBlockKind::Rule, spacing_mode);
                }
            }

            Event::Html(html) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                for raw in html.lines() {
                    let span =
                        Span::styled(raw.to_string(), Style::default().fg(html_fg()).italic());
                    lines.push(with_blockquote_prefix(
                        Line::from(span).left_aligned(),
                        blockquote_depth,
                    ));
                }
                if blockquote_depth == 0
                    && list_stack.is_empty()
                    && !in_definition_list
                    && !in_footnote_definition
                {
                    push_block_separator(&mut lines, MarkdownBlockKind::HtmlBlock, spacing_mode);
                }
            }

            Event::InlineHtml(html) => {
                if in_image {
                    image_alt.push_str(&html);
                } else if in_table {
                    current_cell.push_str(&html);
                } else {
                    ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                    current_spans.push(Span::styled(
                        html.to_string(),
                        Style::default().fg(html_fg()).italic(),
                    ));
                }
            }

            Event::FootnoteReference(label) => {
                if in_image {
                    image_alt.push_str(&format!("[^{}]", label));
                } else if in_table {
                    current_cell.push_str(&format!("[^{}]", label));
                } else {
                    ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                    current_spans.push(Span::styled(
                        format!("[^{}]", label),
                        Style::default().fg(md_dim_color()),
                    ));
                }
            }

            Event::TaskListMarker(checked) => {
                if in_table {
                    current_cell.push_str(if checked { "[x] " } else { "[ ] " });
                } else {
                    ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                    current_spans.push(Span::styled(
                        if checked { "[x] " } else { "[ ] " },
                        Style::default().fg(md_dim_color()),
                    ));
                }
            }

            Event::Start(Tag::Paragraph) => {
                ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                if in_definition_item && current_spans.is_empty() {
                    current_spans.push(Span::styled("  ", Style::default().fg(md_dim_color())));
                }
            }
            Event::End(TagEnd::Paragraph) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                push_block_separator(&mut lines, MarkdownBlockKind::Paragraph, spacing_mode);
            }

            Event::Start(Tag::Item) => {
                dbg_list_items += 1;
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                let depth = list_stack.len().saturating_sub(1);
                let indent = "  ".repeat(depth);
                let marker = if let Some(state) = list_stack.last_mut() {
                    if state.ordered {
                        let idx = state.next_index;
                        state.next_index = state.next_index.saturating_add(1);
                        format!("{}{}. ", indent, idx)
                    } else {
                        format!("{}• ", indent)
                    }
                } else {
                    "• ".to_string()
                };
                current_spans.push(Span::styled(marker, Style::default().fg(md_dim_color())));
            }
            Event::End(TagEnd::Item) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
            }

            // Table handling
            Event::Start(Tag::Table(_)) => {
                dbg_tables += 1;
                // Flush any pending content
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                in_table = true;
                table_rows.clear();
            }
            Event::End(TagEnd::Table) => {
                // Render the collected table
                if !table_rows.is_empty() {
                    let rendered = render_table(&table_rows, max_width);
                    lines.extend(rendered);
                    if blockquote_depth == 0
                        && list_stack.is_empty()
                        && !in_definition_list
                        && !in_footnote_definition
                    {
                        push_block_separator(&mut lines, MarkdownBlockKind::Table, spacing_mode);
                    }
                }
                in_table = false;
                table_rows.clear();
            }
            Event::Start(Tag::TableHead) => {
                _is_header_row = true;
                table_row.clear();
            }
            Event::End(TagEnd::TableHead) => {
                if !table_row.is_empty() {
                    table_rows.push(table_row.clone());
                }
                table_row.clear();
                _is_header_row = false;
            }
            Event::Start(Tag::TableRow) => {
                table_row.clear();
            }
            Event::End(TagEnd::TableRow) => {
                if !table_row.is_empty() {
                    table_rows.push(table_row.clone());
                }
                table_row.clear();
            }
            Event::Start(Tag::TableCell) => {
                current_cell.clear();
            }
            Event::End(TagEnd::TableCell) => {
                table_row.push(current_cell.trim().to_string());
                current_cell.clear();
            }

            Event::Start(Tag::MetadataBlock(_)) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
            }
            Event::End(TagEnd::MetadataBlock(_)) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
            }

            _ => {}
        }
    }

    // Handle incomplete code block (streaming case)
    // If we're still inside a code block, render what we have so far
    if in_code_block && !code_block_content.is_empty() {
        let is_mermaid = code_block_lang
            .as_ref()
            .map(|l| mermaid::is_mermaid_lang(l))
            .unwrap_or(false);

        if is_mermaid {
            if side_only {
                lines.push(mermaid_sidebar_placeholder(
                    "↗ mermaid diagram (sidebar, streaming...)",
                ));
            } else {
                // For mermaid, show "rendering..." placeholder while streaming
                let dim = Style::default().fg(md_dim_color());
                lines.push(Line::from(Span::styled("┌─ mermaid (streaming...) ", dim)));
                // Show first few lines of the diagram source
                for source_line in code_block_content.lines().take(5) {
                    lines.push(Line::from(vec![
                        Span::styled("│ ", dim),
                        Span::styled(source_line.to_string(), Style::default().fg(code_fg())),
                    ]));
                }
                if code_block_content.lines().count() > 5 {
                    lines.push(Line::from(Span::styled("│ ...", dim)));
                }
                lines.push(Line::from(Span::styled("└─", dim)));
            }
        } else {
            // Regular code block - render what we have
            let lang_str = code_block_lang.as_deref().unwrap_or("");
            let header = format!(
                "┌─ {} (streaming...)",
                if lang_str.is_empty() {
                    "code"
                } else {
                    lang_str
                }
            );
            lines.push(Line::from(Span::styled(
                header,
                Style::default().fg(md_dim_color()),
            )));

            // Render code with syntax highlighting
            let highlighted = highlight_code(&code_block_content, code_block_lang.as_deref());
            for line in highlighted {
                let mut prefixed = vec![Span::styled("│ ", Style::default().fg(md_dim_color()))];
                prefixed.extend(line.spans);
                lines.push(Line::from(prefixed));
            }
            // Show cursor to indicate more content coming
            lines.push(Line::from(Span::styled(
                "│ ▌",
                Style::default().fg(md_dim_color()),
            )));
            lines.push(Line::from(Span::styled(
                "└─",
                Style::default().fg(md_dim_color()),
            )));
        }
    }

    // Flush remaining spans
    flush_current_line_with_alignment(
        &mut lines,
        &mut current_spans,
        structured_markdown_alignment(
            blockquote_depth,
            &list_stack,
            in_definition_list,
            in_footnote_definition,
        ),
    );

    normalize_block_separators(&mut lines);

    if center_code_blocks() {
        if let Some(width) = max_width {
            center_left_aligned_runs(&mut lines, width);
        }
    }

    if let Ok(mut state) = MARKDOWN_DEBUG.lock() {
        state.stats.total_renders += 1;
        state.stats.last_render_ms = Some(render_start.elapsed().as_secs_f32() * 1000.0);
        state.stats.last_text_len = Some(text.len());
        state.stats.last_lines = Some(lines.len());
        state.stats.last_headings = dbg_headings;
        state.stats.last_code_blocks = dbg_code_blocks;
        state.stats.last_mermaid_blocks = dbg_mermaid_blocks;
        state.stats.last_tables = dbg_tables;
        state.stats.last_list_items = dbg_list_items;
        state.stats.last_blockquotes = dbg_blockquotes;
    }

    lines
}

fn line_plain_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

pub(crate) fn extract_copy_targets_from_rendered_lines(
    lines: &[Line<'static>],
) -> Vec<RawCopyTarget> {
    let mut targets = Vec::new();

    let mut idx = 0usize;
    while idx < lines.len() {
        let text = line_plain_text(&lines[idx]);
        let trimmed = text.trim_start();
        if let Some(rest) = trimmed.strip_prefix("┌─ ") {
            let label = rest.trim();
            let language = if label.is_empty() || label == "code" {
                None
            } else {
                Some(label.to_string())
            };
            let start = idx;
            let badge_line = idx;
            idx += 1;
            let mut content_lines = Vec::new();
            while idx < lines.len() {
                let line_text = line_plain_text(&lines[idx]);
                let line_trimmed = line_text.trim_start();
                if line_trimmed.starts_with("└─") {
                    idx += 1;
                    break;
                }
                if let Some(code) = line_trimmed.strip_prefix("│ ") {
                    content_lines.push(code.to_string());
                }
                idx += 1;
            }
            targets.push(RawCopyTarget {
                kind: CopyTargetKind::CodeBlock { language },
                content: content_lines.join("\n"),
                start_raw_line: start,
                end_raw_line: idx,
                badge_raw_line: badge_line,
            });
            continue;
        }
        idx += 1;
    }

    targets
}

/// Render a table as ASCII-style lines
/// max_width: Optional maximum width for the entire table
fn render_table(rows: &[Vec<String>], max_width: Option<usize>) -> Vec<Line<'static>> {
    if rows.is_empty() {
        return vec![];
    }

    let mut lines = Vec::new();

    // Calculate column widths
    let num_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut col_widths: Vec<usize> = vec![0; num_cols];

    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < col_widths.len() {
                col_widths[i] = col_widths[i].max(UnicodeWidthStr::width(cell.as_str()));
            }
        }
    }

    // Apply max width constraint if specified
    if let Some(max_w) = max_width {
        // Account for separators: " │ " = 3 chars between each column
        let separator_space = if num_cols > 1 { (num_cols - 1) * 3 } else { 0 };
        let available = max_w.saturating_sub(separator_space);

        if available > 0 && num_cols > 0 {
            let total_width: usize = col_widths.iter().sum();
            if total_width > available {
                // Shrink columns proportionally, with minimum of 5 chars
                let min_col_width = 5;
                let scale = available as f64 / total_width as f64;
                for width in &mut col_widths {
                    *width = (*width as f64 * scale).round() as usize;
                    *width = (*width).max(min_col_width);
                }
            }
        }
    }

    // Render each row
    for (row_idx, row) in rows.iter().enumerate() {
        let mut spans: Vec<Span<'static>> = Vec::new();

        for (i, cell) in row.iter().enumerate() {
            let display_width = UnicodeWidthStr::width(cell.as_str());
            let col_width = col_widths.get(i).copied().unwrap_or(display_width);

            let display_text = if display_width > col_width {
                let mut truncated = String::new();
                let mut w = 0;
                for ch in cell.chars() {
                    let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                    if w + cw + 1 > col_width {
                        break;
                    }
                    truncated.push(ch);
                    w += cw;
                }
                truncated.push('…');
                truncated
            } else {
                cell.clone()
            };
            let text_width = UnicodeWidthStr::width(display_text.as_str());
            let pad = col_width.saturating_sub(text_width);
            let padded = format!("{}{}", display_text, " ".repeat(pad));

            // Header row gets bold styling
            let style = if row_idx == 0 {
                Style::default().fg(bold_color()).bold()
            } else {
                Style::default().fg(text_color())
            };

            if i > 0 {
                spans.push(Span::styled(" │ ", Style::default().fg(table_color())));
            }
            spans.push(Span::styled(padded, style));
        }

        lines.push(Line::from(spans).left_aligned());

        // Add separator after header row
        if row_idx == 0 {
            let separator: String = col_widths
                .iter()
                .map(|&w| "─".repeat(w))
                .collect::<Vec<_>>()
                .join("─┼─");
            lines.push(
                Line::from(Span::styled(separator, Style::default().fg(table_color())))
                    .left_aligned(),
            );
        }
    }

    lines
}

/// Render a table with a specific max width constraint
pub fn render_table_with_width(rows: &[Vec<String>], max_width: usize) -> Vec<Line<'static>> {
    render_table(rows, Some(max_width))
}

/// Highlight a code block with syntax highlighting (cached)
/// This is the primary entry point for code highlighting - uses a cache
/// to avoid re-highlighting the same code multiple times during streaming.
fn highlight_code_cached(code: &str, lang: Option<&str>) -> Vec<Line<'static>> {
    let hash = hash_code(code, lang);

    // Check cache first
    if let Ok(cache) = HIGHLIGHT_CACHE.lock() {
        if let Some(lines) = cache.get(hash) {
            if let Ok(mut state) = MARKDOWN_DEBUG.lock() {
                state.stats.highlight_cache_hits += 1;
            }
            return lines;
        }
    }

    // Cache miss - do the highlighting
    if let Ok(mut state) = MARKDOWN_DEBUG.lock() {
        state.stats.highlight_cache_misses += 1;
    }
    let lines = highlight_code(code, lang);

    // Store in cache
    if let Ok(mut cache) = HIGHLIGHT_CACHE.lock() {
        cache.insert(hash, lines.clone());
    }

    lines
}

/// Highlight a code block with syntax highlighting
fn highlight_code(code: &str, lang: Option<&str>) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Try to find syntax for the language
    let syntax = lang
        .and_then(|l| SYNTAX_SET.find_syntax_by_token(l))
        .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text());

    let theme = &THEME_SET.themes["base16-ocean.dark"];
    let mut highlighter = HighlightLines::new(syntax, theme);

    for line in code.lines() {
        let highlighted = highlighter.highlight_line(line, &SYNTAX_SET);

        match highlighted {
            Ok(ranges) => {
                let spans: Vec<Span<'static>> = ranges
                    .into_iter()
                    .map(|(style, text)| {
                        Span::styled(text.to_string(), syntect_to_ratatui_style(style))
                    })
                    .collect();
                lines.push(Line::from(spans));
            }
            Err(_) => {
                // Fallback to plain text
                lines.push(Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(code_fg()),
                )));
            }
        }
    }

    lines
}

/// Convert syntect style to ratatui style
fn syntect_to_ratatui_style(style: SynStyle) -> Style {
    let fg = rgb(style.foreground.r, style.foreground.g, style.foreground.b);
    Style::default().fg(fg)
}

/// Highlight a single line of code (for diff display)
/// Returns styled spans for the line, or None if highlighting fails
/// `ext` is the file extension (e.g., "rs", "py", "js")
pub fn highlight_line(code: &str, ext: Option<&str>) -> Vec<Span<'static>> {
    let syntax = ext
        .and_then(|e| SYNTAX_SET.find_syntax_by_extension(e))
        .or_else(|| ext.and_then(|e| SYNTAX_SET.find_syntax_by_token(e)))
        .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text());

    let theme = &THEME_SET.themes["base16-ocean.dark"];
    let mut highlighter = HighlightLines::new(syntax, theme);

    match highlighter.highlight_line(code, &SYNTAX_SET) {
        Ok(ranges) => ranges
            .into_iter()
            .map(|(style, text)| Span::styled(text.to_string(), syntect_to_ratatui_style(style)))
            .collect(),
        Err(_) => {
            vec![Span::raw(code.to_string())]
        }
    }
}

/// Highlight a full file and return spans for specific line numbers (1-indexed)
/// Used for comparison logging with single-line approach
pub fn highlight_file_lines(
    content: &str,
    ext: Option<&str>,
    line_numbers: &[usize],
) -> Vec<(usize, Vec<Span<'static>>)> {
    let syntax = ext
        .and_then(|e| SYNTAX_SET.find_syntax_by_extension(e))
        .or_else(|| ext.and_then(|e| SYNTAX_SET.find_syntax_by_token(e)))
        .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text());

    let theme = &THEME_SET.themes["base16-ocean.dark"];
    let mut highlighter = HighlightLines::new(syntax, theme);

    let mut results = Vec::new();
    let lines: Vec<&str> = content.lines().collect();

    for (i, line) in lines.iter().enumerate() {
        let line_num = i + 1; // 1-indexed
        if let Ok(ranges) = highlighter.highlight_line(line, &SYNTAX_SET) {
            if line_numbers.contains(&line_num) {
                let spans: Vec<Span<'static>> = ranges
                    .into_iter()
                    .map(|(style, text)| {
                        Span::styled(text.to_string(), syntect_to_ratatui_style(style))
                    })
                    .collect();
                results.push((line_num, spans));
            }
        }
    }

    results
}

/// Placeholder for code blocks that are not visible
/// Used by lazy rendering to avoid highlighting off-screen code
fn placeholder_code_block(code: &str, lang: Option<&str>) -> Vec<Line<'static>> {
    let line_count = code.lines().count();
    let lang_str = lang.unwrap_or("code");

    // Return placeholder lines that will be replaced when visible
    vec![Line::from(Span::styled(
        format!("  [{} block: {} lines]", lang_str, line_count),
        Style::default().fg(md_dim_color()).italic(),
    ))]
}

/// Check if two ranges overlap
fn ranges_overlap(a: std::ops::Range<usize>, b: std::ops::Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

/// Render markdown with lazy code block highlighting
///
/// Only highlights code blocks that fall within the visible line range.
/// Code blocks outside the visible range are rendered as placeholders.
/// This significantly improves performance for long documents with many code blocks.
pub fn render_markdown_lazy(
    text: &str,
    max_width: Option<usize>,
    visible_range: std::ops::Range<usize>,
) -> Vec<Line<'static>> {
    let text = escape_currency_dollars(text);
    let text = text.as_str();
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current_spans: Vec<Span<'static>> = Vec::new();
    let side_only = diagram_side_only();
    let spacing_mode = effective_markdown_spacing_mode();

    // Style stack for nested formatting
    let mut bold = false;
    let mut italic = false;
    let mut strike = false;
    let mut in_code_block = false;
    let mut code_block_lang: Option<String> = None;
    let mut code_block_content = String::new();
    let mut code_block_start_line: usize = 0;
    let mut heading_level: Option<u8> = None;
    let mut blockquote_depth = 0usize;
    let mut list_stack: Vec<ListRenderState> = Vec::new();
    let mut link_targets: Vec<String> = Vec::new();
    let mut in_image = false;
    let mut image_url: Option<String> = None;
    let mut image_alt = String::new();
    let mut in_definition_list = false;
    let mut in_definition_item = false;
    let mut in_footnote_definition = false;

    // Table state
    let mut in_table = false;
    let mut table_row: Vec<String> = Vec::new();
    let mut table_rows: Vec<Vec<String>> = Vec::new();
    let mut current_cell = String::new();
    let mut _is_header_row = false;

    // Enable table parsing
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_MATH);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_GFM);
    options.insert(Options::ENABLE_DEFINITION_LIST);
    options.insert(Options::ENABLE_SMART_PUNCTUATION);
    let parser = Parser::new_ext(text, options);

    for event in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                heading_level = Some(level as u8);
            }
            Event::End(TagEnd::Heading(_)) => {
                if !current_spans.is_empty() {
                    let color = match heading_level {
                        Some(1) => heading_h1_color(),
                        Some(2) => heading_h2_color(),
                        Some(3) => heading_h3_color(),
                        _ => heading_color(),
                    };

                    let heading_spans: Vec<Span<'static>> = current_spans
                        .drain(..)
                        .map(|s| {
                            Span::styled(s.content.to_string(), Style::default().fg(color).bold())
                        })
                        .collect();
                    lines.push(Line::from(heading_spans));
                    push_block_separator(&mut lines, MarkdownBlockKind::Heading, spacing_mode);
                }
                heading_level = None;
            }

            Event::Start(Tag::Strong) => bold = true,
            Event::End(TagEnd::Strong) => bold = false,

            Event::Start(Tag::Emphasis) => italic = true,
            Event::End(TagEnd::Emphasis) => italic = false,

            Event::Start(Tag::Strikethrough) => strike = true,
            Event::End(TagEnd::Strikethrough) => strike = false,

            Event::Start(Tag::BlockQuote(_)) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                blockquote_depth += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                blockquote_depth = blockquote_depth.saturating_sub(1);
                if blockquote_depth == 0
                    && list_stack.is_empty()
                    && !in_definition_list
                    && !in_footnote_definition
                {
                    push_block_separator(&mut lines, MarkdownBlockKind::BlockQuote, spacing_mode);
                }
            }

            Event::Start(Tag::List(start)) => {
                let state = ListRenderState {
                    ordered: start.is_some(),
                    next_index: start.unwrap_or(1),
                };
                list_stack.push(state);
            }
            Event::End(TagEnd::List(_)) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                list_stack.pop();
                if blockquote_depth == 0
                    && list_stack.is_empty()
                    && !in_definition_list
                    && !in_footnote_definition
                {
                    push_block_separator(&mut lines, MarkdownBlockKind::List, spacing_mode);
                }
            }

            Event::Start(Tag::Link { dest_url, .. }) => {
                link_targets.push(dest_url.to_string());
            }
            Event::End(TagEnd::Link) => {
                if let Some(url) = link_targets.pop() {
                    if !url.is_empty() {
                        current_spans.push(Span::styled(
                            format!(" ({})", url),
                            Style::default().fg(md_dim_color()),
                        ));
                    }
                }
            }

            Event::Start(Tag::Image { dest_url, .. }) => {
                in_image = true;
                image_url = Some(dest_url.to_string());
                image_alt.clear();
            }
            Event::End(TagEnd::Image) => {
                let alt = if image_alt.trim().is_empty() {
                    "image".to_string()
                } else {
                    image_alt.trim().to_string()
                };
                let label = if let Some(url) = image_url.take() {
                    format!("[image: {}] ({})", alt, url)
                } else {
                    format!("[image: {}]", alt)
                };
                if in_table {
                    current_cell.push_str(&label);
                } else {
                    ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                    current_spans.push(Span::styled(label, Style::default().fg(md_dim_color())));
                }
                in_image = false;
                image_alt.clear();
            }

            Event::Start(Tag::FootnoteDefinition(label)) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                in_footnote_definition = true;
                ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                current_spans.push(Span::styled(
                    format!("[^{}]: ", label),
                    Style::default().fg(md_dim_color()),
                ));
            }
            Event::End(TagEnd::FootnoteDefinition) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                in_footnote_definition = false;
            }

            Event::Start(Tag::DefinitionList) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                in_definition_list = true;
            }
            Event::End(TagEnd::DefinitionList) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                in_definition_list = false;
                if blockquote_depth == 0 && list_stack.is_empty() && !in_footnote_definition {
                    push_block_separator(
                        &mut lines,
                        MarkdownBlockKind::DefinitionList,
                        spacing_mode,
                    );
                }
            }
            Event::Start(Tag::DefinitionListTitle) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                current_spans.push(Span::styled("• ", Style::default().fg(md_dim_color())));
            }
            Event::End(TagEnd::DefinitionListTitle) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
            }
            Event::Start(Tag::DefinitionListDefinition) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                current_spans.push(Span::styled("  -> ", Style::default().fg(md_dim_color())));
                in_definition_item = true;
            }
            Event::End(TagEnd::DefinitionListDefinition) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                in_definition_item = false;
            }

            Event::Start(Tag::CodeBlock(kind)) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                in_code_block = true;
                code_block_start_line = lines.len();
                code_block_lang = match kind {
                    CodeBlockKind::Fenced(lang) if !lang.is_empty() => Some(lang.to_string()),
                    _ => None,
                };
                // Don't add header here - we'll add it at the end when we know the block width
                code_block_content.clear();
            }
            Event::End(TagEnd::CodeBlock) => {
                let is_mermaid = code_block_lang
                    .as_ref()
                    .map(|l| mermaid::is_mermaid_lang(l))
                    .unwrap_or(false);

                if is_mermaid {
                    let terminal_width = max_width.and_then(|w| u16::try_from(w).ok());
                    let result = mermaid::render_mermaid_sized(&code_block_content, terminal_width);
                    match result {
                        mermaid::RenderResult::Image { .. } if side_only => {
                            lines.push(mermaid_sidebar_placeholder("↗ mermaid diagram (sidebar)"));
                        }
                        other => {
                            let mermaid_lines = mermaid::result_to_lines(other, max_width);
                            lines.extend(mermaid_lines);
                        }
                    }
                } else {
                    // Calculate the line range this code block will occupy
                    let code_line_count = code_block_content.lines().count();
                    let block_range =
                        code_block_start_line..(code_block_start_line + code_line_count + 2);

                    // Check if this block is visible
                    let is_visible = ranges_overlap(block_range.clone(), visible_range.clone());

                    let lang_label = code_block_lang.as_deref().unwrap_or("");

                    let highlighted = if is_visible {
                        let hl =
                            highlight_code_cached(&code_block_content, code_block_lang.as_deref());
                        Some(hl)
                    } else {
                        None
                    };

                    // Add header
                    lines.push(
                        Line::from(Span::styled(
                            format!("┌─ {} ", lang_label),
                            Style::default().fg(md_dim_color()),
                        ))
                        .left_aligned(),
                    );

                    if let Some(hl_lines) = highlighted {
                        // Render highlighted code
                        for hl_line in hl_lines {
                            let mut spans =
                                vec![Span::styled("│ ", Style::default().fg(md_dim_color()))];
                            spans.extend(hl_line.spans);
                            lines.push(Line::from(spans).left_aligned());
                        }
                    } else {
                        // Use placeholder for off-screen blocks
                        let placeholder =
                            placeholder_code_block(&code_block_content, code_block_lang.as_deref());
                        for pl_line in placeholder {
                            let mut spans =
                                vec![Span::styled("│ ", Style::default().fg(md_dim_color()))];
                            spans.extend(pl_line.spans);
                            lines.push(Line::from(spans).left_aligned());
                        }
                    }

                    // Add footer
                    lines.push(
                        Line::from(Span::styled("└─", Style::default().fg(md_dim_color())))
                            .left_aligned(),
                    );
                }
                in_code_block = false;
                code_block_lang = None;
                code_block_content.clear();
                if blockquote_depth == 0
                    && list_stack.is_empty()
                    && !in_definition_list
                    && !in_footnote_definition
                {
                    push_block_separator(&mut lines, MarkdownBlockKind::CodeBlock, spacing_mode);
                }
            }

            Event::Code(code) => {
                if in_image {
                    image_alt.push_str(&code);
                    continue;
                }
                // Inline code - handle differently in tables vs regular text
                if in_table {
                    current_cell.push_str(&code);
                } else {
                    ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                    current_spans.push(Span::styled(
                        code.to_string(),
                        apply_inline_decorations(
                            Style::default().fg(code_fg()).bg(code_bg()),
                            strike,
                            !link_targets.is_empty(),
                        ),
                    ));
                }
            }

            Event::InlineMath(math) => {
                if in_image {
                    image_alt.push('$');
                    image_alt.push_str(&math);
                    image_alt.push('$');
                    continue;
                }
                if in_table {
                    current_cell.push('$');
                    current_cell.push_str(&math);
                    current_cell.push('$');
                } else {
                    ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                    current_spans.push(math_inline_span(&math));
                }
            }

            Event::DisplayMath(math) => {
                if in_image {
                    image_alt.push_str("$$");
                    image_alt.push_str(&math);
                    image_alt.push_str("$$");
                    continue;
                }
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                if in_table {
                    current_cell.push_str("$$");
                    current_cell.push_str(&math);
                    current_cell.push_str("$$");
                } else {
                    for line in math_display_lines(&math) {
                        lines.push(with_blockquote_prefix(line, blockquote_depth));
                    }
                    if blockquote_depth == 0
                        && list_stack.is_empty()
                        && !in_definition_list
                        && !in_footnote_definition
                    {
                        push_block_separator(
                            &mut lines,
                            MarkdownBlockKind::DisplayMath,
                            spacing_mode,
                        );
                    }
                }
            }

            Event::Text(text) => {
                if in_code_block {
                    code_block_content.push_str(&text);
                } else if in_image {
                    image_alt.push_str(&text);
                } else if in_table {
                    current_cell.push_str(&text);
                } else {
                    let is_thinking_duration =
                        text.starts_with("Thought for ") && text.ends_with('s');
                    let mut style = if is_thinking_duration {
                        Style::default().fg(md_dim_color()).italic()
                    } else {
                        match (bold, italic) {
                            (true, true) => Style::default().fg(bold_color()).bold().italic(),
                            (true, false) => Style::default().fg(bold_color()).bold(),
                            (false, true) => Style::default().fg(text_color()).italic(),
                            (false, false) => Style::default().fg(text_color()),
                        }
                    };
                    style = apply_inline_decorations(style, strike, !link_targets.is_empty());
                    ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                    current_spans.push(Span::styled(text.to_string(), style));
                }
            }

            Event::SoftBreak => {
                if in_image {
                    image_alt.push(' ');
                } else if !in_code_block {
                    current_spans.push(Span::raw(" "));
                }
            }
            Event::HardBreak => {
                if in_image {
                    image_alt.push(' ');
                } else if !in_code_block {
                    flush_current_line_with_alignment(
                        &mut lines,
                        &mut current_spans,
                        structured_markdown_alignment(
                            blockquote_depth,
                            &list_stack,
                            in_definition_list,
                            in_footnote_definition,
                        ),
                    );
                }
            }

            Event::Rule => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                let width = max_width.unwrap_or(RULE_LEN);
                let rule = Span::styled("─".repeat(width), Style::default().fg(md_dim_color()));
                lines.push(with_blockquote_prefix(
                    Line::from(rule).left_aligned(),
                    blockquote_depth,
                ));
                if blockquote_depth == 0
                    && list_stack.is_empty()
                    && !in_definition_list
                    && !in_footnote_definition
                {
                    push_block_separator(&mut lines, MarkdownBlockKind::Rule, spacing_mode);
                }
            }

            Event::Html(html) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                for raw in html.lines() {
                    let span =
                        Span::styled(raw.to_string(), Style::default().fg(html_fg()).italic());
                    lines.push(with_blockquote_prefix(
                        Line::from(span).left_aligned(),
                        blockquote_depth,
                    ));
                }
                if blockquote_depth == 0
                    && list_stack.is_empty()
                    && !in_definition_list
                    && !in_footnote_definition
                {
                    push_block_separator(&mut lines, MarkdownBlockKind::HtmlBlock, spacing_mode);
                }
            }

            Event::InlineHtml(html) => {
                if in_image {
                    image_alt.push_str(&html);
                } else if in_table {
                    current_cell.push_str(&html);
                } else {
                    ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                    current_spans.push(Span::styled(
                        html.to_string(),
                        Style::default().fg(html_fg()).italic(),
                    ));
                }
            }

            Event::FootnoteReference(label) => {
                if in_image {
                    image_alt.push_str(&format!("[^{}]", label));
                } else if in_table {
                    current_cell.push_str(&format!("[^{}]", label));
                } else {
                    ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                    current_spans.push(Span::styled(
                        format!("[^{}]", label),
                        Style::default().fg(md_dim_color()),
                    ));
                }
            }

            Event::TaskListMarker(checked) => {
                if in_table {
                    current_cell.push_str(if checked { "[x] " } else { "[ ] " });
                } else {
                    ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                    current_spans.push(Span::styled(
                        if checked { "[x] " } else { "[ ] " },
                        Style::default().fg(md_dim_color()),
                    ));
                }
            }

            Event::Start(Tag::Paragraph) => {
                ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                if in_definition_item && current_spans.is_empty() {
                    current_spans.push(Span::styled("  ", Style::default().fg(md_dim_color())));
                }
            }
            Event::End(TagEnd::Paragraph) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                push_block_separator(&mut lines, MarkdownBlockKind::Paragraph, spacing_mode);
            }

            Event::Start(Tag::Item) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                ensure_blockquote_prefix(&mut current_spans, blockquote_depth);
                let depth = list_stack.len().saturating_sub(1);
                let indent = "  ".repeat(depth);
                let marker = if let Some(state) = list_stack.last_mut() {
                    if state.ordered {
                        let idx = state.next_index;
                        state.next_index = state.next_index.saturating_add(1);
                        format!("{}{}. ", indent, idx)
                    } else {
                        format!("{}• ", indent)
                    }
                } else {
                    "• ".to_string()
                };
                current_spans.push(Span::styled(marker, Style::default().fg(md_dim_color())));
            }
            Event::End(TagEnd::Item) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
            }

            Event::Start(Tag::Table(_)) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
                in_table = true;
                table_rows.clear();
            }
            Event::End(TagEnd::Table) => {
                if !table_rows.is_empty() {
                    let rendered = render_table(&table_rows, max_width);
                    lines.extend(rendered);
                    if blockquote_depth == 0
                        && list_stack.is_empty()
                        && !in_definition_list
                        && !in_footnote_definition
                    {
                        push_block_separator(&mut lines, MarkdownBlockKind::Table, spacing_mode);
                    }
                }
                in_table = false;
                table_rows.clear();
            }
            Event::Start(Tag::TableHead) => {
                _is_header_row = true;
                table_row.clear();
            }
            Event::End(TagEnd::TableHead) => {
                if !table_row.is_empty() {
                    table_rows.push(table_row.clone());
                }
                table_row.clear();
                _is_header_row = false;
            }
            Event::Start(Tag::TableRow) => {
                table_row.clear();
            }
            Event::End(TagEnd::TableRow) => {
                if !table_row.is_empty() {
                    table_rows.push(table_row.clone());
                }
                table_row.clear();
            }
            Event::Start(Tag::TableCell) => {
                current_cell.clear();
            }
            Event::End(TagEnd::TableCell) => {
                table_row.push(current_cell.trim().to_string());
                current_cell.clear();
            }

            Event::Start(Tag::MetadataBlock(_)) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
            }
            Event::End(TagEnd::MetadataBlock(_)) => {
                flush_current_line_with_alignment(
                    &mut lines,
                    &mut current_spans,
                    structured_markdown_alignment(
                        blockquote_depth,
                        &list_stack,
                        in_definition_list,
                        in_footnote_definition,
                    ),
                );
            }

            _ => {}
        }
    }

    flush_current_line_with_alignment(
        &mut lines,
        &mut current_spans,
        structured_markdown_alignment(
            blockquote_depth,
            &list_stack,
            in_definition_list,
            in_footnote_definition,
        ),
    );

    normalize_block_separators(&mut lines);

    if center_code_blocks() {
        if let Some(width) = max_width {
            center_left_aligned_runs(&mut lines, width);
        }
    }

    lines
}

/// Wrap a line of styled spans to fit within a given width (using unicode display width)
/// Returns multiple lines if wrapping is needed
pub fn wrap_line(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![line];
    }

    // Preserve the original alignment
    let alignment = line.alignment;

    let mut result: Vec<Line<'static>> = Vec::new();
    let mut current_spans: Vec<Span<'static>> = Vec::with_capacity(line.spans.len());
    let mut current_width = 0usize;

    for span in line.spans {
        let style = span.style;
        let text = span.content.as_ref();

        // Process each word/chunk in the span
        let mut remaining = text;
        while !remaining.is_empty() {
            // Find next break point (space or full chunk if no space)
            let (chunk, rest) = if let Some(space_idx) = remaining.find(' ') {
                let (word, after_space) = remaining.split_at(space_idx);
                // Include the space in the word
                if after_space.len() > 1 {
                    let mut buf = String::with_capacity(word.len() + 1);
                    buf.push_str(word);
                    buf.push(' ');
                    (buf, &after_space[1..])
                } else {
                    let mut buf = String::with_capacity(word.len() + 1);
                    buf.push_str(word);
                    buf.push(' ');
                    (buf, "")
                }
            } else {
                (remaining.to_string(), "")
            };
            remaining = rest;

            // Use unicode display width instead of char count
            let chunk_width = chunk.width();

            // If adding this chunk would exceed width, start new line
            if current_width + chunk_width > width && current_width > 0 {
                let mut new_line = Line::from(std::mem::take(&mut current_spans));
                if let Some(align) = alignment {
                    new_line = new_line.alignment(align);
                }
                result.push(new_line);
                current_width = 0;
            }

            // Handle chunks longer than width (force break by grapheme/char with width tracking)
            if chunk_width > width {
                // Build up characters until we hit the width limit
                let mut part = String::new();
                let mut part_width = 0usize;

                for c in chunk.chars() {
                    let char_width = c.to_string().width();

                    // Would this char overflow the available width?
                    if current_width + part_width + char_width > width
                        && (current_width + part_width) > 0
                    {
                        // Push current part if non-empty
                        if !part.is_empty() {
                            current_spans.push(Span::styled(std::mem::take(&mut part), style));
                            current_width += part_width;
                            part_width = 0;
                        }

                        // Start new line if we have content
                        if current_width > 0 {
                            let mut new_line = Line::from(std::mem::take(&mut current_spans));
                            if let Some(align) = alignment {
                                new_line = new_line.alignment(align);
                            }
                            result.push(new_line);
                            current_width = 0;
                        }
                    }

                    part.push(c);
                    part_width += char_width;
                }

                // Don't forget remaining part
                if !part.is_empty() {
                    current_spans.push(Span::styled(part, style));
                    current_width += part_width;
                }
            } else {
                current_spans.push(Span::styled(chunk, style));
                current_width += chunk_width;
            }
        }
    }

    // Don't forget the last line
    if !current_spans.is_empty() {
        let mut new_line = Line::from(current_spans);
        if let Some(align) = alignment {
            new_line = new_line.alignment(align);
        }
        result.push(new_line);
    }

    if result.is_empty() {
        let mut empty_line = Line::from("");
        if let Some(align) = alignment {
            empty_line = empty_line.alignment(align);
        }
        result.push(empty_line);
    }

    result
}

/// Wrap multiple lines to fit within a given width
pub fn wrap_lines(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .flat_map(|line| wrap_line(line, width))
        .collect()
}

/// Create a progress bar string
pub fn progress_bar(progress: f32, width: usize) -> String {
    let filled = (progress * width as f32) as usize;
    let empty = width.saturating_sub(filled);

    let bar: String = std::iter::repeat('█')
        .take(filled)
        .chain(std::iter::repeat('░').take(empty))
        .collect();

    bar
}

/// Create a styled progress bar line
pub fn progress_line(label: &str, progress: f32, width: usize) -> Line<'static> {
    let bar = progress_bar(progress, width.saturating_sub(label.len() + 3));
    let pct = (progress * 100.0) as u8;

    Line::from(vec![
        Span::styled(label.to_string(), Style::default().dim()),
        Span::raw(" "),
        Span::styled(bar, Style::default().fg(rgb(129, 199, 132))),
        Span::styled(format!(" {}%", pct), Style::default().dim()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_to_string(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn render_markdown_with_mode(text: &str, mode: MarkdownSpacingMode) -> Vec<Line<'static>> {
        with_markdown_spacing_mode_override(Some(mode), || render_markdown(text))
    }

    fn render_markdown_with_width_and_mode(
        text: &str,
        width: usize,
        mode: MarkdownSpacingMode,
    ) -> Vec<Line<'static>> {
        with_markdown_spacing_mode_override(Some(mode), || {
            render_markdown_with_width(text, Some(width))
        })
    }

    fn lines_to_string(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(line_to_string)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn test_simple_markdown() {
        let lines = render_markdown("Hello **world**");
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_code_block() {
        let lines = render_markdown("```rust\nfn main() {}\n```");
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_extract_copy_targets_from_rendered_lines_for_code_block() {
        let lines =
            render_markdown("before\n\n```rust\nfn main() {}\nprintln!(\"hi\");\n```\n\nafter");
        let targets = extract_copy_targets_from_rendered_lines(&lines);

        assert_eq!(targets.len(), 1);
        let target = &targets[0];
        assert_eq!(
            target.kind,
            CopyTargetKind::CodeBlock {
                language: Some("rust".to_string())
            }
        );
        assert_eq!(target.content, "fn main() {}\nprintln!(\"hi\");");
        assert_eq!(target.start_raw_line, target.badge_raw_line);
        assert!(target.end_raw_line > target.start_raw_line);
    }

    #[test]
    fn test_progress_bar() {
        let bar = progress_bar(0.5, 10);
        assert_eq!(bar.chars().count(), 10);
    }

    #[test]
    fn test_table_render_basic() {
        let md = "| A | B |\n| - | - |\n| 1 | 2 |";
        let lines = render_markdown(md);
        let rendered: Vec<String> = lines.iter().map(line_to_string).collect();

        assert!(
            rendered
                .iter()
                .any(|l| l.contains('│') && l.contains('A') && l.contains('B'))
        );
        assert!(rendered.iter().any(|l| l.contains('─') && l.contains('┼')));
    }

    #[test]
    fn test_table_width_truncation() {
        let md = "| Column | Value |\n| - | - |\n| very_long_cell_value | 1234567890 |";
        let lines = render_markdown_with_width(md, Some(20));
        let rendered: Vec<String> = lines.iter().map(line_to_string).collect();

        assert!(rendered.iter().any(|l| l.contains('…')));
        let max_len = rendered
            .iter()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(0);
        assert!(max_len <= 20);
    }

    #[test]
    fn test_table_cjk_alignment() {
        let md = "| Issue | You wrote |\n| - | - |\n| 政策 pronunciation | zhēn cí |";
        let lines = render_markdown(md);
        let rendered: Vec<String> = lines.iter().map(line_to_string).collect();

        let non_empty: Vec<&String> = rendered.iter().filter(|l| !l.is_empty()).collect();
        assert!(
            non_empty.len() >= 3,
            "Expected at least 3 non-empty lines, got {}: {:?}",
            non_empty.len(),
            non_empty
        );

        let header = non_empty[0];
        let separator = non_empty[1];
        let data_row = non_empty[2];

        let header_width = UnicodeWidthStr::width(header.as_str());
        let sep_width = UnicodeWidthStr::width(separator.as_str());
        let data_width = UnicodeWidthStr::width(data_row.as_str());

        assert_eq!(
            header_width, sep_width,
            "Header and separator should have same display width: header='{}' ({}) sep='{}' ({})",
            header, header_width, separator, sep_width
        );
        assert_eq!(
            header_width, data_width,
            "Header and data row should have same display width: header='{}' ({}) data='{}' ({})",
            header, header_width, data_row, data_width
        );
    }

    #[test]
    fn test_mermaid_block_detection() {
        // Mermaid blocks should be detected and rendered differently than regular code
        let md = "```mermaid\nflowchart LR\n    A --> B\n```";
        let lines = render_markdown(md);

        // Mermaid rendering can return:
        // 1. Empty lines (image displayed via Kitty/iTerm2 protocol directly to stdout)
        // 2. ASCII fallback lines (if no graphics support)
        // 3. Error lines (if parsing failed)
        // All are valid outcomes

        // Should NOT have the code block border (┌─ mermaid) since mermaid removes it
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();

        // The key test: it should NOT contain syntax-highlighted code (the raw mermaid source)
        // It should either be empty (image displayed) or contain mermaid metadata
        assert!(
            lines.is_empty() || text.contains("mermaid") || text.contains("flowchart"),
            "Expected mermaid handling, got: {}",
            text
        );
    }

    #[test]
    fn test_mixed_code_and_mermaid() {
        // Mixed content should render both correctly
        let md = "```rust\nfn main() {}\n```\n\n```mermaid\nflowchart TD\n    A\n```\n\n```python\nprint('hi')\n```";
        let lines = render_markdown(md);

        // Should have output for all blocks
        assert!(
            lines.len() >= 3,
            "Expected multiple lines for mixed content"
        );
    }

    #[test]
    fn test_inline_math_render() {
        let lines = render_markdown("Area is $a^2$.");
        let rendered = lines_to_string(&lines);
        assert!(rendered.contains("$a^2$"));
    }

    #[test]
    fn test_display_math_render() {
        let lines = render_markdown("$$\nE = mc^2\n$$");
        let rendered = lines_to_string(&lines);
        assert!(rendered.contains("┌─ math"));
        assert!(rendered.contains("E = mc^2"));
        assert!(rendered.contains("└─"));
    }

    #[test]
    fn test_link_strike_and_image_render() {
        let md = "This is ~~old~~ and [docs](https://example.com).\n\n![chart](https://img.example/chart.png)";
        let lines = render_markdown(md);
        let rendered = lines_to_string(&lines);
        assert!(rendered.contains("old"));
        assert!(rendered.contains("docs (https://example.com)"));
        assert!(rendered.contains("[image: chart] (https://img.example/chart.png)"));
    }

    #[test]
    fn test_ordered_and_task_list_render() {
        let md = "1. first\n2. second\n\n- [x] done\n- [ ] todo";
        let lines = render_markdown(md);
        let rendered = lines_to_string(&lines);
        assert!(rendered.contains("1. first"));
        assert!(rendered.contains("2. second"));
        assert!(rendered.contains("[x] done"));
        assert!(rendered.contains("[ ] todo"));
    }

    #[test]
    fn test_blockquote_footnote_and_definition_list_render() {
        let md = "> quote line\n\nRef[^a]\n\n[^a]: footnote body\n\nTerm\n  : definition text";
        let lines = render_markdown(md);
        let rendered = lines_to_string(&lines);
        assert!(rendered.contains("│ quote line"));
        assert!(rendered.contains("[^a]"));
        assert!(rendered.contains("[^a]: footnote body"));
        assert!(rendered.contains("Term"));
        assert!(rendered.contains("definition text"));
    }

    #[test]
    fn test_plain_paragraph_alignment_remains_unset() {
        let lines = render_markdown("plain paragraph");
        let line = lines
            .iter()
            .find(|line| line_to_string(line).contains("plain paragraph"))
            .expect("paragraph line");
        assert_eq!(line.alignment, None);
    }

    #[test]
    fn test_structured_markdown_lines_force_left_alignment() {
        let md = concat!(
            "- [x] done\n",
            "1. numbered\n\n",
            "> quoted\n\n",
            "[^a]: footnote body\n\n",
            "Term\n  : definition text\n\n",
            "| A | B |\n| - | - |\n| 1 | 2 |\n\n",
            "$$\nE = mc^2\n$$\n\n",
            "---\n\n",
            "<div>html</div>"
        );

        let saved = center_code_blocks();
        set_center_code_blocks(true);
        let lines = render_markdown_with_width(md, Some(40));
        set_center_code_blocks(saved);

        let expected = [
            "• [x] done",
            "1. numbered",
            "│ quoted",
            "[^a]: footnote body",
            "• Term",
            "  -> definition text",
            "A │ B",
            "─┼─",
            "1 │ 2",
            "┌─ math",
            "│ E = mc^2",
            "└─",
            "────",
            "<div>html</div>",
        ];

        for snippet in expected {
            let line = lines
                .iter()
                .find(|line| line_to_string(line).contains(snippet))
                .unwrap_or_else(|| panic!("missing line containing '{snippet}' in {lines:?}"));
            assert_eq!(
                line.alignment,
                Some(Alignment::Left),
                "expected left alignment for line containing '{snippet}'"
            );
        }
    }

    #[test]
    fn test_wrapped_left_aligned_list_items_stay_left_aligned() {
        let lines = render_markdown("- this is a long list item that should wrap");
        let wrapped = wrap_lines(lines, 12);

        let non_empty: Vec<&Line<'_>> = wrapped
            .iter()
            .filter(|line| !line.spans.is_empty())
            .collect();
        assert!(
            non_empty.len() >= 2,
            "expected wrapped list item: {wrapped:?}"
        );
        assert!(
            non_empty
                .iter()
                .all(|line| line.alignment == Some(Alignment::Left)),
            "expected wrapped list lines to preserve left alignment: {wrapped:?}"
        );
    }

    #[test]
    fn test_rule_and_inline_html_render() {
        let md = "before\n\n---\n\ninline <span>html</span> tag";
        let lines = render_markdown(md);
        let rendered = lines_to_string(&lines);
        assert!(rendered.contains("────────────────"));
        assert!(rendered.contains("<span>"));
        assert!(rendered.contains("</span>"));
    }

    #[test]
    fn test_compact_spacing_keeps_heading_tight_but_separates_list_from_next_heading() {
        let md = "# Intro\nBody\n\n- one\n- two\n\n# Next\nBody";
        let rendered: Vec<String> = render_markdown_with_mode(md, MarkdownSpacingMode::Compact)
            .iter()
            .map(line_to_string)
            .collect();

        assert_eq!(
            rendered,
            vec!["Intro", "Body", "", "• one", "• two", "", "Next", "Body"]
        );
    }

    #[test]
    fn test_document_spacing_adds_heading_separation() {
        let md = "# Intro\nBody\n\n- one\n- two\n\n# Next\nBody";
        let rendered: Vec<String> = render_markdown_with_mode(md, MarkdownSpacingMode::Document)
            .iter()
            .map(line_to_string)
            .collect();

        assert_eq!(
            rendered,
            vec![
                "Intro", "", "Body", "", "• one", "• two", "", "Next", "", "Body"
            ]
        );
    }

    #[test]
    fn test_compact_spacing_separates_code_block_from_following_heading_without_trailing_blank() {
        let md = "```rust\nfn main() {}\n```\n\n# Next";
        let rendered: Vec<String> = render_markdown_with_mode(md, MarkdownSpacingMode::Compact)
            .iter()
            .map(line_to_string)
            .collect();

        assert_eq!(
            rendered,
            vec!["┌─ rust ", "│ fn main() {}", "└─", "", "Next"]
        );
    }

    #[test]
    fn test_document_spacing_keeps_table_single_spaced_between_blocks() {
        let md = "Before\n\n| A | B |\n| - | - |\n| 1 | 2 |\n\nAfter";
        let rendered: Vec<String> =
            render_markdown_with_width_and_mode(md, 40, MarkdownSpacingMode::Document)
                .iter()
                .map(line_to_string)
                .collect();

        let table_start = rendered
            .iter()
            .position(|line| line.contains('│') && line.contains('A') && line.contains('B'))
            .expect("table header line");
        assert_eq!(rendered[table_start - 1], "");
        assert_eq!(rendered[table_start + 3], "");
        assert_eq!(rendered.last().map(String::as_str), Some("After"));
    }

    #[test]
    fn test_incremental_renderer_basic() {
        let mut renderer = IncrementalMarkdownRenderer::new(Some(80));

        // First render
        let lines1 = renderer.update("Hello **world**");
        assert!(!lines1.is_empty());

        // Same text should return cached result
        let lines2 = renderer.update("Hello **world**");
        assert_eq!(lines1.len(), lines2.len());

        // Appended text should work
        let lines3 = renderer.update("Hello **world**\n\nMore text");
        assert!(lines3.len() > lines1.len());
    }

    #[test]
    fn test_incremental_renderer_streaming() {
        let mut renderer = IncrementalMarkdownRenderer::new(Some(80));

        // Simulate streaming tokens
        let _ = renderer.update("Hello ");
        let _ = renderer.update("Hello world");
        let _ = renderer.update("Hello world\n\n");
        let lines = renderer.update("Hello world\n\nParagraph 2");

        // Should have rendered both paragraphs
        assert!(lines.len() >= 2);
    }

    #[test]
    fn test_incremental_renderer_streaming_inline_math() {
        let mut renderer = IncrementalMarkdownRenderer::new(Some(80));
        let _ = renderer.update("Compute $x");
        let lines = renderer.update("Compute $x$");
        let rendered = lines_to_string(&lines);
        assert!(rendered.contains("$x$"));
    }

    #[test]
    fn test_incremental_renderer_streaming_display_math() {
        let mut renderer = IncrementalMarkdownRenderer::new(Some(80));
        let _ = renderer.update("Intro\n\n$$\nA + B");
        let lines = renderer.update("Intro\n\n$$\nA + B\n$$\n");
        let rendered = lines_to_string(&lines);

        assert!(
            rendered.contains("┌─ math"),
            "expected display math block after closing delimiter: {}",
            rendered
        );
        assert!(rendered.contains("│ A + B"), "expected math body");
        assert!(
            !rendered.contains("$$"),
            "expected raw $$ delimiters to be consumed: {}",
            rendered
        );
    }

    #[test]
    fn test_incremental_renderer_streams_fenced_block_before_close() {
        let mut renderer = IncrementalMarkdownRenderer::new(Some(80));
        let _ = renderer.update("Plan:\n\n```\n");
        let lines = renderer.update("Plan:\n\n```\nProcess A: |████\n");
        let rendered = lines_to_string(&lines);

        assert!(
            rendered.contains("Process A"),
            "Expected streamed code-block content before closing fence: {}",
            rendered
        );
    }

    #[test]
    fn test_checkpoint_does_not_enter_unclosed_fence() {
        let renderer = IncrementalMarkdownRenderer::new(Some(80));
        let text = "Intro\n\n```\nProcess A\n\nProcess B";
        let checkpoint = renderer.find_last_complete_block(text);
        assert_eq!(checkpoint, Some("Intro\n\n".len()));
    }

    #[test]
    fn test_incremental_renderer_replaces_stale_prefix_chars() {
        let mut renderer = IncrementalMarkdownRenderer::new(Some(80));
        let _ = renderer.update("Plan:\n\n```\n[\n");
        let lines = renderer.update("Plan:\n\n```\nProcess A\n");
        let rendered = lines_to_string(&lines);

        assert!(
            !rendered.contains("│ ["),
            "Expected stale '[' to be replaced during streaming: {}",
            rendered
        );
        assert!(rendered.contains("Process A"));
    }

    #[test]
    fn test_streaming_unclosed_bracket_keeps_text_visible() {
        let mut renderer = IncrementalMarkdownRenderer::new(Some(80));
        let lines = renderer.update("[Process A: |████");
        let rendered = lines_to_string(&lines);
        assert!(
            rendered.contains("Process A"),
            "Expected unclosed bracket line to remain visible: {}",
            rendered
        );
    }

    #[test]
    fn test_lazy_rendering_visible_range() {
        let md = "```rust\nfn main() {\n    println!(\"hello\");\n}\n```\n\nSome text\n\n```python\nprint('hi')\n```";

        // Render with full visibility
        let lines_full = render_markdown_lazy(md, Some(80), 0..100);

        // Render with partial visibility (only first code block visible)
        let lines_partial = render_markdown_lazy(md, Some(80), 0..5);

        // Both should produce output
        assert!(!lines_full.is_empty());
        assert!(!lines_partial.is_empty());
    }

    #[test]
    fn test_ranges_overlap() {
        assert!(ranges_overlap(0..10, 5..15));
        assert!(ranges_overlap(5..15, 0..10));
        assert!(!ranges_overlap(0..5, 10..15));
        assert!(!ranges_overlap(10..15, 0..5));
        assert!(ranges_overlap(0..10, 0..10)); // Same range
        assert!(ranges_overlap(0..10, 5..6)); // Contained
    }

    #[test]
    fn test_highlight_cache_performance() {
        // First call should cache
        let code = "fn main() {\n    println!(\"hello\");\n}";
        let lines1 = highlight_code_cached(code, Some("rust"));

        // Second call should hit cache
        let lines2 = highlight_code_cached(code, Some("rust"));

        assert_eq!(lines1.len(), lines2.len());
    }

    #[test]
    fn test_bold_with_dollar_signs() {
        let md = "Meet the **$35 minimum** (local delivery) and delivery is **free**. Below that, expect a **$5.99** fee.";
        let lines = render_markdown(md);
        let rendered = lines_to_string(&lines);
        assert!(
            !rendered.contains("**"),
            "Bold markers should not appear as literal text: {}",
            rendered
        );
        assert!(rendered.contains("$35 minimum"));
        assert!(rendered.contains("$5.99"));
    }

    #[test]
    fn test_escape_currency_preserves_math() {
        assert_eq!(escape_currency_dollars("$x^2$"), "$x^2$");
        assert_eq!(escape_currency_dollars("$$E=mc^2$$"), "$$E=mc^2$$");
        assert_eq!(escape_currency_dollars("costs $35"), "costs \\$35");
        assert_eq!(escape_currency_dollars("`$100`"), "`$100`");
        assert_eq!(escape_currency_dollars("```\n$50\n```"), "```\n$50\n```");
        assert_eq!(escape_currency_dollars("\\$10"), "\\$10");
        assert_eq!(escape_currency_dollars("████████░░░░"), "████████░░░░");
        assert_eq!(escape_currency_dollars("⣿⣿⣿⣀⣀⣀"), "⣿⣿⣿⣀⣀⣀");
        assert_eq!(escape_currency_dollars("▓▓▒▒░░"), "▓▓▒▒░░");
        assert_eq!(escape_currency_dollars("━━━╺━━━"), "━━━╺━━━");
        assert_eq!(escape_currency_dollars("⠋ Loading $5"), "⠋ Loading \\$5");
    }

    #[test]
    fn test_currency_dollars_in_indented_code_block() {
        assert_eq!(
            escape_currency_dollars("   ```\nCost is $35\n```"),
            "   ```\nCost is $35\n```"
        );

        assert_eq!(
            escape_currency_dollars("    ```\nCost is $35\n```"),
            "    ```\nCost is $35\n```"
        );

        assert_eq!(
            escape_currency_dollars("        ```\nCost is $35\n```"),
            "        ```\nCost is $35\n```"
        );
    }

    #[test]
    fn test_fence_closing_not_triggered_mid_line() {
        let md = "```\nvalue = `code` and then ``` in same line\n```";
        let rendered = lines_to_string(&render_markdown(md));

        assert!(rendered.contains("`code`"));
        assert!(rendered.contains("in same line"));
    }
}
