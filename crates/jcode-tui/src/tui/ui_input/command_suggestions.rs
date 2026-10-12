use super::*;

pub(super) fn command_suggestion_lines(
    app: &dyn TuiState,
    suggestions: &[(String, &'static str)],
) -> Vec<Line<'static>> {
    let mode = composer_mode(app.input(), app.is_remote_mode());
    let is_file_mention = mode == ComposerMode::FileMention;

    // For file mentions, extract the @ query for highlighting.
    let needle = if is_file_mention {
        extract_at_query(app.input()).map(|(_, q)| q)
    } else {
        command_suggestion_needle(app.input())
    };
    let highlight = |cmd: &str, base: Style| -> Vec<Span<'static>> {
        highlight_command_spans(cmd, needle.as_deref(), base)
    };

    let mut lines = Vec::new();
    if suggestions.len() == 1 {
        let (cmd, desc) = &suggestions[0];
        let base = Style::default().fg(rgb(255, 213, 128));
        let mut spans = highlight(cmd, base);
        spans.push(Span::styled(format!("  {}", desc), base));
        lines.push(Line::from(spans));
    } else if !suggestions.is_empty() {
        // In file-mention mode the selection may rest on a section-header
        // sentinel; highlight the row Enter would actually accept.
        let selected = effective_selected_row(
            app.command_suggestion_selected(),
            suggestions,
            is_file_mention,
        );
        let window_start = command_suggestion_window_start(selected, suggestions.len());
        let limited: Vec<_> = suggestions
            .iter()
            .skip(window_start)
            .take(app::COMMAND_SUGGESTION_VISIBLE_LIMIT)
            .collect();
        let window_end = window_start + limited.len();
        let more_count = suggestions.len().saturating_sub(window_end);
        let selected_visible = selected.saturating_sub(window_start);

        for (i, (cmd, desc)) in limited.iter().enumerate() {
            let is_selected = i == selected_visible;
            let is_section_header = is_file_mention && cmd.is_empty();

            // Section headers render as dimmed, non-selectable lines.
            if is_section_header {
                lines.push(Line::from(vec![Span::styled(
                    format!("  {}", desc),
                    Style::default()
                        .fg(dim_color())
                        .add_modifier(Modifier::BOLD),
                )]));
                continue;
            }

            let is_dir = is_file_mention && cmd.ends_with('/');

            let description_style = if is_selected {
                Style::default().fg(rgb(255, 213, 128))
            } else {
                Style::default().fg(dim_color())
            };
            // File mention mode: directories blue, files teal.
            let command_style = if is_selected {
                Style::default().fg(rgb(255, 213, 128))
            } else if is_dir {
                Style::default().fg(rgb(120, 180, 220))
            } else {
                Style::default().fg(rgb(128, 203, 196))
            };
            let mut spans = highlight(cmd, command_style);
            if is_dir && !is_selected {
                // Directory visual: just use blue styling, trailing / is enough.
            }
            if !desc.is_empty() {
                spans.push(Span::styled(format!("  {}", desc), description_style));
            }
            if i == 0 && window_start > 0 {
                spans.push(Span::styled(
                    format!("  ↑{}", window_start),
                    Style::default().fg(dim_color()),
                ));
            }
            if i + 1 == limited.len() && more_count > 0 {
                spans.push(Span::styled(
                    format!("  +{} more", more_count),
                    Style::default().fg(dim_color()),
                ));
            }
            lines.push(Line::from(spans));
        }
    }
    lines
}


/// Build spans for a suggestion command, recoloring the characters that the
/// fuzzy matcher aligned with the typed query. Falls back to a single
/// unhighlighted span when there is nothing (useful) to highlight.
pub(super) fn highlight_command_spans(
    cmd: &str,
    needle: Option<&str>,
    base: Style,
) -> Vec<Span<'static>> {
    let positions: Vec<usize> = match needle {
        Some(n) if !n.is_empty() && n != "/" => crate::tui::fuzzy::fuzzy_match_positions(n, cmd),
        _ => Vec::new(),
    };
    if positions.is_empty() {
        return vec![Span::styled(cmd.to_string(), base)];
    }

    // Recolor (rather than underline) the matched characters so they stay in
    // the command palette's own hue: matched chars are a brighter version of
    // the line's base color, while unmatched chars are dimmed so the match
    // visually pops.
    let highlight_style = base
        .fg(brighten_command_color(base.fg))
        .add_modifier(Modifier::BOLD);
    let rest_style = base.fg(dim_command_color(base.fg));
    let chars: Vec<char> = cmd.chars().collect();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run_start = 0usize;
    let mut run_is_match = !chars.is_empty() && positions.contains(&0);
    for i in 1..=chars.len() {
        let cur_is_match = i < chars.len() && positions.contains(&i);
        if cur_is_match != run_is_match || i == chars.len() {
            let chunk: String = chars[run_start..i].iter().collect();
            spans.push(Span::styled(
                chunk,
                if run_is_match {
                    highlight_style
                } else {
                    rest_style
                },
            ));
            run_start = i;
            run_is_match = cur_is_match;
        }
    }
    spans
}
