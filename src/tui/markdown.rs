#![allow(dead_code)]

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::prelude::*;
use std::collections::HashMap;
use unicode_width::UnicodeWidthStr;
use std::hash::{Hash, Hasher};
use std::sync::{LazyLock, Mutex};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style as SynStyle, ThemeSet};
use syntect::parsing::SyntaxSet;

use crate::tui::mermaid;

// Syntax highlighting resources (loaded once)
static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(|| SyntaxSet::load_defaults_newlines());
static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

// Syntax highlighting cache - keyed by (code content hash, language)
static HIGHLIGHT_CACHE: LazyLock<Mutex<HighlightCache>> =
    LazyLock::new(|| Mutex::new(HighlightCache::new()));

const HIGHLIGHT_CACHE_LIMIT: usize = 256;

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
        // Fast path: text unchanged
        if full_text == self.rendered_text {
            return self.rendered_lines.clone();
        }

        // Fast path: text was only appended
        if full_text.starts_with(&self.rendered_text) {
            let appended = &full_text[self.rendered_text.len()..];

            // Find a safe re-render point
            // Safe points are after: double newlines (paragraph end), code block end
            let rerender_from = self.find_safe_rerender_point(full_text);

            if rerender_from >= self.last_checkpoint {
                // Re-render from the safe point
                let text_to_render = &full_text[rerender_from..];
                let new_lines = render_markdown_with_width(text_to_render, self.max_width);

                // Keep lines up to checkpoint, append new lines
                self.rendered_lines.truncate(self.lines_at_checkpoint);
                self.rendered_lines.extend(new_lines);

                // Update checkpoint if we found a new complete block
                if let Some(new_checkpoint) = self.find_new_checkpoint(full_text, appended) {
                    self.last_checkpoint = new_checkpoint;
                    self.lines_at_checkpoint = self.rendered_lines.len();
                }

                self.rendered_text = full_text.to_string();
                return self.rendered_lines.clone();
            }
        }

        // Slow path: text changed in middle or was truncated
        // Full re-render required
        self.rendered_lines = render_markdown_with_width(full_text, self.max_width);
        self.rendered_text = full_text.to_string();

        // Find checkpoint for next incremental update
        if let Some(checkpoint) = self.find_last_complete_block(full_text) {
            self.last_checkpoint = checkpoint;
            // Count lines up to this point
            let prefix_lines = render_markdown_with_width(&full_text[..checkpoint], self.max_width);
            self.lines_at_checkpoint = prefix_lines.len();
        } else {
            self.last_checkpoint = 0;
            self.lines_at_checkpoint = 0;
        }

        self.rendered_lines.clone()
    }

    /// Find a safe point to start re-rendering from
    fn find_safe_rerender_point(&self, text: &str) -> usize {
        // Start from the last checkpoint
        self.last_checkpoint
    }

    /// Find a new checkpoint after appended text
    fn find_new_checkpoint(&self, full_text: &str, appended: &str) -> Option<usize> {
        // Look for complete blocks in the appended portion
        let start = full_text.len() - appended.len();

        // Check for paragraph end (double newline)
        if let Some(pos) = appended.rfind("\n\n") {
            return Some(start + pos + 2);
        }

        // Check for code block end
        if appended.contains("```") {
            // Find the last code block end - ensure we land on a char boundary
            let mut search_start = start.saturating_sub(10); // Look back a bit
            while search_start > 0 && !full_text.is_char_boundary(search_start) {
                search_start -= 1;
            }
            let search_text = &full_text[search_start..];
            if let Some(pos) = search_text.rfind("\n```\n") {
                return Some(search_start + pos + 5);
            }
            if search_text.ends_with("\n```") {
                return Some(full_text.len());
            }
        }

        None
    }

    /// Find the last complete block in text
    fn find_last_complete_block(&self, text: &str) -> Option<usize> {
        // Find last double newline (paragraph boundary)
        if let Some(pos) = text.rfind("\n\n") {
            return Some(pos + 2);
        }

        // Find last code block end
        if let Some(pos) = text.rfind("\n```\n") {
            return Some(pos + 5);
        }

        None
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
const CODE_BG: Color = Color::Rgb(45, 45, 45);
const CODE_FG: Color = Color::Rgb(180, 180, 180);
const TEXT_COLOR: Color = Color::Rgb(200, 200, 195); // Soft warm white for AI text
const BOLD_COLOR: Color = Color::Rgb(240, 240, 235); // Slightly brighter for bold
                                                     // Heading colors - warm gold/amber gradient by level
const HEADING_H1_COLOR: Color = Color::Rgb(255, 215, 100); // Bright gold for # H1
const HEADING_H2_COLOR: Color = Color::Rgb(240, 190, 90); // Gold for ## H2
const HEADING_H3_COLOR: Color = Color::Rgb(220, 170, 80); // Amber for ### H3
const HEADING_COLOR: Color = Color::Rgb(200, 155, 75); // Darker amber for #### and below
const DIM_COLOR: Color = Color::Rgb(100, 100, 100);
const TABLE_COLOR: Color = Color::Rgb(150, 150, 150); // Table borders/separators

/// Render markdown text to styled ratatui Lines
pub fn render_markdown(text: &str) -> Vec<Line<'static>> {
    render_markdown_with_width(text, None)
}

/// Render markdown with optional width constraint for tables
pub fn render_markdown_with_width(text: &str, max_width: Option<usize>) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current_spans: Vec<Span<'static>> = Vec::new();

    // Style stack for nested formatting
    let mut bold = false;
    let mut italic = false;
    let mut in_code_block = false;
    let mut code_block_lang: Option<String> = None;
    let mut code_block_content = String::new();
    let mut heading_level: Option<u8> = None;

    // Table state
    let mut in_table = false;
    let mut table_row: Vec<String> = Vec::new();
    let mut table_rows: Vec<Vec<String>> = Vec::new();
    let mut current_cell = String::new();
    let mut _is_header_row = false;

    // Enable table parsing
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    let parser = Parser::new_ext(text, options);

    for event in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                if !current_spans.is_empty() {
                    lines.push(Line::from(std::mem::take(&mut current_spans)));
                }
                heading_level = Some(level as u8);
            }
            Event::End(TagEnd::Heading(_)) => {
                if !current_spans.is_empty() {
                    // Choose color based on heading level
                    let color = match heading_level {
                        Some(1) => HEADING_H1_COLOR,
                        Some(2) => HEADING_H2_COLOR,
                        Some(3) => HEADING_H3_COLOR,
                        _ => HEADING_COLOR,
                    };

                    let heading_spans: Vec<Span<'static>> = current_spans
                        .drain(..)
                        .map(|s| {
                            Span::styled(s.content.to_string(), Style::default().fg(color).bold())
                        })
                        .collect();
                    lines.push(Line::from(heading_spans));
                }
                heading_level = None;
            }

            Event::Start(Tag::Strong) => bold = true,
            Event::End(TagEnd::Strong) => bold = false,

            Event::Start(Tag::Emphasis) => italic = true,
            Event::End(TagEnd::Emphasis) => italic = false,

            Event::Start(Tag::CodeBlock(kind)) => {
                // Flush current line before code block
                if !current_spans.is_empty() {
                    lines.push(Line::from(std::mem::take(&mut current_spans)));
                }
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
                    // Render mermaid diagram
                    let result = mermaid::render_mermaid(&code_block_content);
                    let mermaid_lines = mermaid::result_to_lines(result, max_width);
                    lines.extend(mermaid_lines);
                } else {
                    // Render code block with syntax highlighting (cached)
                    let highlighted =
                        highlight_code_cached(&code_block_content, code_block_lang.as_deref());

                    // Calculate the max width of code lines for centering
                    let lang_label = code_block_lang.as_deref().unwrap_or("");
                    let header_width = 3 + lang_label.len(); // "┌─ " + lang
                    let code_widths: Vec<usize> = highlighted
                        .iter()
                        .map(|l| 2 + l.spans.iter().map(|s| s.content.chars().count()).sum::<usize>()) // "│ " + content
                        .collect();
                    let max_code_width = code_widths.iter().copied().max().unwrap_or(0);
                    let block_width = header_width.max(max_code_width).max(2); // at least "└─"

                    // Calculate padding to center the block
                    let padding = if let Some(mw) = max_width {
                        if block_width < mw {
                            (mw - block_width) / 2
                        } else {
                            0
                        }
                    } else {
                        0
                    };
                    let pad_str: String = " ".repeat(padding);

                    // Add header with padding
                    lines.push(Line::from(Span::styled(
                        format!("{}┌─ {} ", pad_str, lang_label),
                        Style::default().fg(DIM_COLOR),
                    )).left_aligned());

                    // Add code lines with padding
                    for hl_line in highlighted {
                        let mut spans = vec![
                            Span::styled(format!("{}│ ", pad_str), Style::default().fg(DIM_COLOR))
                        ];
                        spans.extend(hl_line.spans);
                        lines.push(Line::from(spans).left_aligned());
                    }

                    // Add footer with padding
                    lines.push(
                        Line::from(Span::styled(
                            format!("{}└─", pad_str),
                            Style::default().fg(DIM_COLOR),
                        ))
                        .left_aligned(),
                    );
                }
                in_code_block = false;
                code_block_lang = None;
                code_block_content.clear();
            }

            Event::Code(code) => {
                // Inline code - handle differently in tables vs regular text
                if in_table {
                    current_cell.push_str(&code);
                } else {
                    current_spans.push(Span::styled(
                        code.to_string(),
                        Style::default().fg(CODE_FG).bg(CODE_BG),
                    ));
                }
            }

            Event::Text(text) => {
                if in_code_block {
                    code_block_content.push_str(&text);
                } else if in_table {
                    current_cell.push_str(&text);
                } else {
                    // Check for "Thought for X.Xs" pattern and render dimmed
                    let is_thinking_duration =
                        text.starts_with("Thought for ") && text.ends_with('s');
                    let style = if is_thinking_duration {
                        Style::default().fg(DIM_COLOR).italic()
                    } else {
                        match (bold, italic) {
                            (true, true) => Style::default().fg(BOLD_COLOR).bold().italic(),
                            (true, false) => Style::default().fg(BOLD_COLOR).bold(),
                            (false, true) => Style::default().fg(TEXT_COLOR).italic(),
                            (false, false) => Style::default().fg(TEXT_COLOR),
                        }
                    };
                    current_spans.push(Span::styled(text.to_string(), style));
                }
            }

            Event::SoftBreak => {
                if !in_code_block {
                    current_spans.push(Span::raw(" "));
                }
            }
            Event::HardBreak => {
                if !in_code_block {
                    lines.push(Line::from(std::mem::take(&mut current_spans)));
                }
            }

            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => {
                if !current_spans.is_empty() {
                    lines.push(Line::from(std::mem::take(&mut current_spans)));
                }
                // Add blank line after paragraph for visual separation
                lines.push(Line::default());
            }

            Event::Start(Tag::Item) => {
                current_spans.push(Span::styled("• ", Style::default().fg(DIM_COLOR)));
            }
            Event::End(TagEnd::Item) => {
                if !current_spans.is_empty() {
                    lines.push(Line::from(std::mem::take(&mut current_spans)));
                }
            }

            // Table handling
            Event::Start(Tag::Table(_)) => {
                // Flush any pending content
                if !current_spans.is_empty() {
                    lines.push(Line::from(std::mem::take(&mut current_spans)));
                }
                in_table = true;
                table_rows.clear();
            }
            Event::End(TagEnd::Table) => {
                // Render the collected table with padding
                if !table_rows.is_empty() {
                    lines.push(Line::from("")); // Padding before table
                    let rendered = render_table(&table_rows, max_width);
                    lines.extend(rendered);
                    lines.push(Line::from("")); // Padding after table
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

            _ => {}
        }
    }

    // Flush remaining spans
    if !current_spans.is_empty() {
        lines.push(Line::from(current_spans));
    }

    lines
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
                col_widths[i] = col_widths[i].max(cell.len());
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
            let char_count = cell.chars().count();
            let width = col_widths.get(i).copied().unwrap_or(char_count);

            // Truncate cell content if needed (use char boundaries, not bytes)
            let display_text = if char_count > width {
                let truncated: String = cell.chars().take(width.saturating_sub(1)).collect();
                format!("{}…", truncated)
            } else {
                cell.clone()
            };
            let padded = format!("{:<width$}", display_text, width = width);

            // Header row gets bold styling
            let style = if row_idx == 0 {
                Style::default().fg(BOLD_COLOR).bold()
            } else {
                Style::default().fg(TEXT_COLOR)
            };

            if i > 0 {
                spans.push(Span::styled(" │ ", Style::default().fg(TABLE_COLOR)));
            }
            spans.push(Span::styled(padded, style));
        }

        lines.push(Line::from(spans));

        // Add separator after header row
        if row_idx == 0 {
            let separator: String = col_widths
                .iter()
                .map(|&w| "─".repeat(w))
                .collect::<Vec<_>>()
                .join("─┼─");
            lines.push(Line::from(Span::styled(
                separator,
                Style::default().fg(TABLE_COLOR),
            )));
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
            return lines;
        }
    }

    // Cache miss - do the highlighting
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
                    Style::default().fg(CODE_FG),
                )));
            }
        }
    }

    lines
}

/// Convert syntect style to ratatui style
fn syntect_to_ratatui_style(style: SynStyle) -> Style {
    let fg = Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b);
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
        Style::default().fg(DIM_COLOR).italic(),
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
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current_spans: Vec<Span<'static>> = Vec::new();

    // Style stack for nested formatting
    let mut bold = false;
    let mut italic = false;
    let mut in_code_block = false;
    let mut code_block_lang: Option<String> = None;
    let mut code_block_content = String::new();
    let mut code_block_start_line: usize = 0;
    let mut heading_level: Option<u8> = None;

    // Table state
    let mut in_table = false;
    let mut table_row: Vec<String> = Vec::new();
    let mut table_rows: Vec<Vec<String>> = Vec::new();
    let mut current_cell = String::new();
    let mut _is_header_row = false;

    // Enable table parsing
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    let parser = Parser::new_ext(text, options);

    for event in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                if !current_spans.is_empty() {
                    lines.push(Line::from(std::mem::take(&mut current_spans)));
                }
                heading_level = Some(level as u8);
            }
            Event::End(TagEnd::Heading(_)) => {
                if !current_spans.is_empty() {
                    let color = match heading_level {
                        Some(1) => HEADING_H1_COLOR,
                        Some(2) => HEADING_H2_COLOR,
                        Some(3) => HEADING_H3_COLOR,
                        _ => HEADING_COLOR,
                    };

                    let heading_spans: Vec<Span<'static>> = current_spans
                        .drain(..)
                        .map(|s| {
                            Span::styled(s.content.to_string(), Style::default().fg(color).bold())
                        })
                        .collect();
                    lines.push(Line::from(heading_spans));
                }
                heading_level = None;
            }

            Event::Start(Tag::Strong) => bold = true,
            Event::End(TagEnd::Strong) => bold = false,

            Event::Start(Tag::Emphasis) => italic = true,
            Event::End(TagEnd::Emphasis) => italic = false,

            Event::Start(Tag::CodeBlock(kind)) => {
                if !current_spans.is_empty() {
                    lines.push(Line::from(std::mem::take(&mut current_spans)));
                }
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
                    let result = mermaid::render_mermaid(&code_block_content);
                    let mermaid_lines = mermaid::result_to_lines(result, max_width);
                    lines.extend(mermaid_lines);
                } else {
                    // Calculate the line range this code block will occupy
                    let code_line_count = code_block_content.lines().count();
                    let block_range =
                        code_block_start_line..(code_block_start_line + code_line_count + 2);

                    // Check if this block is visible
                    let is_visible = ranges_overlap(block_range.clone(), visible_range.clone());

                    // Calculate centering padding
                    let lang_label = code_block_lang.as_deref().unwrap_or("");
                    let header_width = 3 + lang_label.len();

                    let (highlighted, code_widths) = if is_visible {
                        let hl = highlight_code_cached(&code_block_content, code_block_lang.as_deref());
                        let widths: Vec<usize> = hl
                            .iter()
                            .map(|l| 2 + l.spans.iter().map(|s| s.content.chars().count()).sum::<usize>())
                            .collect();
                        (Some(hl), widths)
                    } else {
                        // Estimate widths from raw content for placeholder
                        let widths: Vec<usize> = code_block_content.lines().map(|l| 2 + l.chars().count()).collect();
                        (None, widths)
                    };

                    let max_code_width = code_widths.iter().copied().max().unwrap_or(0);
                    let block_width = header_width.max(max_code_width).max(2);

                    let padding = if let Some(mw) = max_width {
                        if block_width < mw {
                            (mw - block_width) / 2
                        } else {
                            0
                        }
                    } else {
                        0
                    };
                    let pad_str: String = " ".repeat(padding);

                    // Add header with padding
                    lines.push(Line::from(Span::styled(
                        format!("{}┌─ {} ", pad_str, lang_label),
                        Style::default().fg(DIM_COLOR),
                    )).left_aligned());

                    if let Some(hl_lines) = highlighted {
                        // Render highlighted code
                        for hl_line in hl_lines {
                            let mut spans = vec![
                                Span::styled(format!("{}│ ", pad_str), Style::default().fg(DIM_COLOR))
                            ];
                            spans.extend(hl_line.spans);
                            lines.push(Line::from(spans).left_aligned());
                        }
                    } else {
                        // Use placeholder for off-screen blocks
                        let placeholder =
                            placeholder_code_block(&code_block_content, code_block_lang.as_deref());
                        for pl_line in placeholder {
                            let mut spans = vec![
                                Span::styled(format!("{}│ ", pad_str), Style::default().fg(DIM_COLOR))
                            ];
                            spans.extend(pl_line.spans);
                            lines.push(Line::from(spans).left_aligned());
                        }
                    }

                    // Add footer with padding
                    lines.push(Line::from(Span::styled(
                        format!("{}└─", pad_str),
                        Style::default().fg(DIM_COLOR),
                    )).left_aligned());
                }
                in_code_block = false;
                code_block_lang = None;
                code_block_content.clear();
            }

            Event::Code(code) => {
                // Inline code - handle differently in tables vs regular text
                if in_table {
                    current_cell.push_str(&code);
                } else {
                    current_spans.push(Span::styled(
                        code.to_string(),
                        Style::default().fg(CODE_FG).bg(CODE_BG),
                    ));
                }
            }

            Event::Text(text) => {
                if in_code_block {
                    code_block_content.push_str(&text);
                } else if in_table {
                    current_cell.push_str(&text);
                } else {
                    let is_thinking_duration =
                        text.starts_with("Thought for ") && text.ends_with('s');
                    let style = if is_thinking_duration {
                        Style::default().fg(DIM_COLOR).italic()
                    } else {
                        match (bold, italic) {
                            (true, true) => Style::default().fg(BOLD_COLOR).bold().italic(),
                            (true, false) => Style::default().fg(BOLD_COLOR).bold(),
                            (false, true) => Style::default().fg(TEXT_COLOR).italic(),
                            (false, false) => Style::default().fg(TEXT_COLOR),
                        }
                    };
                    current_spans.push(Span::styled(text.to_string(), style));
                }
            }

            Event::SoftBreak => {
                if !in_code_block {
                    current_spans.push(Span::raw(" "));
                }
            }
            Event::HardBreak => {
                if !in_code_block {
                    lines.push(Line::from(std::mem::take(&mut current_spans)));
                }
            }

            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => {
                if !current_spans.is_empty() {
                    lines.push(Line::from(std::mem::take(&mut current_spans)));
                }
                lines.push(Line::default());
            }

            Event::Start(Tag::Item) => {
                current_spans.push(Span::styled("• ", Style::default().fg(DIM_COLOR)));
            }
            Event::End(TagEnd::Item) => {
                if !current_spans.is_empty() {
                    lines.push(Line::from(std::mem::take(&mut current_spans)));
                }
            }

            Event::Start(Tag::Table(_)) => {
                if !current_spans.is_empty() {
                    lines.push(Line::from(std::mem::take(&mut current_spans)));
                }
                in_table = true;
                table_rows.clear();
            }
            Event::End(TagEnd::Table) => {
                if !table_rows.is_empty() {
                    lines.push(Line::from(""));
                    let rendered = render_table(&table_rows, max_width);
                    lines.extend(rendered);
                    lines.push(Line::from(""));
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

            _ => {}
        }
    }

    if !current_spans.is_empty() {
        lines.push(Line::from(current_spans));
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
                    if current_width + part_width + char_width > width && (current_width + part_width) > 0 {
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
        Span::styled(bar, Style::default().fg(Color::Rgb(129, 199, 132))),
        Span::styled(format!(" {}%", pct), Style::default().dim()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_to_string(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
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
    fn test_progress_bar() {
        let bar = progress_bar(0.5, 10);
        assert_eq!(bar.chars().count(), 10);
    }

    #[test]
    fn test_table_render_basic() {
        let md = "| A | B |\n| - | - |\n| 1 | 2 |";
        let lines = render_markdown(md);
        let rendered: Vec<String> = lines.iter().map(line_to_string).collect();

        assert!(rendered
            .iter()
            .any(|l| l.contains('│') && l.contains('A') && l.contains('B')));
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
}
