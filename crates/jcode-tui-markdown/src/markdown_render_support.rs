use super::*;

pub(super) fn line_plain_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

pub fn extract_copy_targets_from_rendered_lines(lines: &[Line<'static>]) -> Vec<RawCopyTarget> {
    let mut targets = Vec::new();

    let mut idx = 0usize;
    while idx < lines.len() {
        let text = line_plain_text(&lines[idx]);
        let trimmed = text.trim_start();
        if let Some(rest) = trimmed.strip_prefix("+- ") {
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
                if line_trimmed.starts_with("+-") {
                    idx += 1;
                    break;
                }
                if let Some(code) = line_trimmed.strip_prefix("| ") {
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
pub(super) fn render_table(rows: &[Vec<String>], max_width: Option<usize>) -> Vec<Line<'static>> {
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
        // Account for separators: " | " = 3 chars between each column
        let separator_space = if num_cols > 1 { (num_cols - 1) * 3 } else { 0 };
        let available = max_w.saturating_sub(separator_space);

        if available > 0 && num_cols > 0 {
            let total_width: usize = col_widths.iter().sum();
            if total_width > available {
                let min_col_width = (available / num_cols).clamp(1, 5);
                for width in &mut col_widths {
                    *width = (*width).max(min_col_width);
                }

                while col_widths.iter().sum::<usize>() > available {
                    if let Some((idx, _)) = col_widths
                        .iter()
                        .enumerate()
                        .filter(|(_, width)| **width > min_col_width)
                        .max_by_key(|(_, width)| **width)
                    {
                        col_widths[idx] -= 1;
                    } else {
                        break;
                    }
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
                spans.push(Span::styled(" | ", Style::default().fg(table_color())));
            }
            spans.push(Span::styled(padded, style));
        }

        lines.push(Line::from(spans).left_aligned());

        // Add separator after header row
        if row_idx == 0 {
            let separator: String = col_widths
                .iter()
                .map(|&w| "-".repeat(w))
                .collect::<Vec<_>>()
                .join("-+-");
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
pub(super) fn highlight_code_cached(code: &str, lang: Option<&str>) -> Vec<Line<'static>> {
    let hash = hash_code(code, lang);

    // Check cache first
    if let Ok(cache) = HIGHLIGHT_CACHE.lock()
        && let Some(lines) = cache.get(hash)
    {
        if let Ok(mut state) = MARKDOWN_DEBUG.lock() {
            state.stats.highlight_cache_hits += 1;
        }
        return lines;
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
pub(super) fn highlight_code(code: &str, lang: Option<&str>) -> Vec<Line<'static>> {
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
        if let Ok(ranges) = highlighter.highlight_line(line, &SYNTAX_SET)
            && line_numbers.contains(&line_num)
        {
            let spans: Vec<Span<'static>> = ranges
                .into_iter()
                .map(|(style, text)| {
                    Span::styled(text.to_string(), syntect_to_ratatui_style(style))
                })
                .collect();
            results.push((line_num, spans));
        }
    }

    results
}

/// Placeholder for code blocks that are not visible
/// Used by lazy rendering to avoid highlighting off-screen code
pub(super) fn placeholder_code_block(code: &str, lang: Option<&str>) -> Vec<Line<'static>> {
    let line_count = code.lines().count();
    let lang_str = lang.unwrap_or("code");

    // Return placeholder lines that will be replaced when visible
    vec![Line::from(Span::styled(
        format!("  [{} block: {} lines]", lang_str, line_count),
        Style::default().fg(md_dim_color()).italic(),
    ))]
}

/// Check if two ranges overlap
pub(super) fn ranges_overlap(a: std::ops::Range<usize>, b: std::ops::Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}
