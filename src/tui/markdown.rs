#![allow(dead_code)]
#![allow(dead_code)]

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ratatui::prelude::*;
use std::sync::LazyLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style as SynStyle, ThemeSet};
use syntect::parsing::SyntaxSet;

// Syntax highlighting resources (loaded once)
static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(|| SyntaxSet::load_defaults_newlines());
static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

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
                // Add code block start indicator
                let lang_label = code_block_lang.as_deref().unwrap_or("");
                lines.push(Line::from(Span::styled(
                    format!("┌─ {} ", lang_label),
                    Style::default().fg(DIM_COLOR),
                )));
                code_block_content.clear();
            }
            Event::End(TagEnd::CodeBlock) => {
                // Render code block with syntax highlighting
                let highlighted = highlight_code(&code_block_content, code_block_lang.as_deref());
                for hl_line in highlighted {
                    // Add left border to code lines
                    let mut spans = vec![Span::styled("│ ", Style::default().fg(DIM_COLOR))];
                    spans.extend(hl_line.spans);
                    lines.push(Line::from(spans));
                }
                // Add code block end indicator
                lines.push(Line::from(Span::styled(
                    "└─",
                    Style::default().fg(DIM_COLOR),
                )));
                in_code_block = false;
                code_block_lang = None;
                code_block_content.clear();
            }

            Event::Code(code) => {
                // Inline code with subtle background
                current_spans.push(Span::styled(
                    format!("`{}`", code),
                    Style::default().fg(CODE_FG).bg(CODE_BG),
                ));
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

/// Wrap a line of styled spans to fit within a given width
/// Returns multiple lines if wrapping is needed
pub fn wrap_line(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![line];
    }

    let mut result: Vec<Line<'static>> = Vec::new();
    let mut current_spans: Vec<Span<'static>> = Vec::new();
    let mut current_width = 0;

    for span in line.spans {
        let style = span.style;
        let text = span.content.to_string();

        // Process each word/chunk in the span
        let mut remaining = text.as_str();
        while !remaining.is_empty() {
            // Find next break point (space or full chunk if no space)
            let (chunk, rest) = if let Some(space_idx) = remaining.find(' ') {
                let (word, after_space) = remaining.split_at(space_idx);
                // Include the space in the word
                if after_space.len() > 1 {
                    (format!("{} ", word), &after_space[1..])
                } else {
                    (format!("{} ", word), "")
                }
            } else {
                (remaining.to_string(), "")
            };
            remaining = rest;

            let chunk_width = chunk.chars().count();

            // If adding this chunk would exceed width, start new line
            if current_width + chunk_width > width && current_width > 0 {
                result.push(Line::from(std::mem::take(&mut current_spans)));
                current_width = 0;
            }

            // Handle chunks longer than width (force break)
            if chunk_width > width {
                let chars: Vec<char> = chunk.chars().collect();
                let mut pos = 0;
                while pos < chars.len() {
                    let available = width.saturating_sub(current_width);
                    let take = available.min(chars.len() - pos);
                    let part: String = chars[pos..pos + take].iter().collect();
                    current_spans.push(Span::styled(part, style));
                    current_width += take;
                    pos += take;

                    if current_width >= width && pos < chars.len() {
                        result.push(Line::from(std::mem::take(&mut current_spans)));
                        current_width = 0;
                    }
                }
            } else {
                current_spans.push(Span::styled(chunk, style));
                current_width += chunk_width;
            }
        }
    }

    // Don't forget the last line
    if !current_spans.is_empty() {
        result.push(Line::from(current_spans));
    }

    if result.is_empty() {
        result.push(Line::from(""));
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

        assert!(rendered.iter().any(|l| l.contains('│') && l.contains('A') && l.contains('B')));
        assert!(rendered.iter().any(|l| l.contains('─') && l.contains('┼')));
    }

    #[test]
    fn test_table_width_truncation() {
        let md = "| Column | Value |\n| - | - |\n| very_long_cell_value | 1234567890 |";
        let lines = render_markdown_with_width(md, Some(20));
        let rendered: Vec<String> = lines.iter().map(line_to_string).collect();

        assert!(rendered.iter().any(|l| l.contains('…')));
        let max_len = rendered.iter().map(|l| l.chars().count()).max().unwrap_or(0);
        assert!(max_len <= 20);
    }
}
