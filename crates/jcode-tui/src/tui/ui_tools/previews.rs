use super::*;

/// Source is a dedicated body, independent of the optional technical summary.
/// Bound visual rows, not source lines: agents frequently submit one long line.
pub(in super::super) fn codemode_preview(tool: &ToolCall, width: usize) -> Vec<Line<'static>> {
    if canonical_tool_name(&tool.name) != "codemode" {
        return Vec::new();
    }
    let Some(code) = tool.input.get("code").and_then(|v| v.as_str()) else {
        return Vec::new();
    };
    if code.trim().is_empty() || width == 0 {
        return Vec::new();
    }
    const MAX_ROWS: usize = 6;
    let indent = width.saturating_sub(1).min(4);
    let inner_width = width.saturating_sub(indent).max(1);
    let mut rows = Vec::new();
    let mut source = code.lines().peekable();
    while let Some(line) = source.next() {
        let source_line = Line::from(super::super::markdown::highlight_line(
            &line.replace('\t', "    "),
            Some("js"),
        ));
        let wrapped = super::super::markdown::wrap_lines(vec![source_line], inner_width);
        let available = MAX_ROWS.saturating_sub(rows.len());
        let continued = wrapped.len() > available;
        for mut row in wrapped.into_iter().take(available) {
            row = super::super::truncate_line_with_ellipsis_to_width(&row, inner_width);
            row.spans.insert(0, Span::raw(" ".repeat(indent)));
            rows.push(row);
        }
        if rows.len() == MAX_ROWS {
            let remaining = source.count();
            let label = if continued {
                Some("… code continues".to_string())
            } else if remaining > 0 {
                Some(format!(
                    "… {remaining} more line{}",
                    if remaining == 1 { "" } else { "s" }
                ))
            } else {
                None
            };
            if let Some(label) = label {
                rows.push(super::super::truncate_line_with_ellipsis_to_width(
                    &Line::from(vec![
                        Span::raw(" ".repeat(indent)),
                        Span::styled(label, Style::default().fg(dim_color())),
                    ]),
                    width,
                ));
            }
            break;
        }
    }
    rows
}

pub(in super::super) fn render_batch_subcall_line(
    tool: &ToolCall,
    icon: &str,
    icon_color: Color,
    bash_max_chars: usize,
    max_width: Option<usize>,
    output_content: Option<&str>,
) -> Line<'static> {
    let display_name = resolve_display_tool_name(&tool.name).to_string();
    let token_badge = output_content.map(|content| {
        let tokens = crate::util::estimate_tokens(content);
        let color = match crate::util::approx_tool_output_token_severity(tokens) {
            crate::util::ApproxTokenSeverity::Normal => rgb(118, 118, 118),
            crate::util::ApproxTokenSeverity::Warning => rgb(214, 184, 92),
            crate::util::ApproxTokenSeverity::Danger => rgb(224, 118, 118),
        };
        (
            tool_usage_label(
                &crate::util::format_approx_token_count(tokens),
                None,
                content,
            ),
            color,
        )
    });
    let intent = tool
        .intent
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let intent_display = intent.map(|intent| {
        max_width
            .map(|width| truncate_end_display(intent, (width / 3).max(16)))
            .unwrap_or_else(|| intent.to_string())
    });
    let intent_width = intent_display.as_ref().map_or(0, |intent| {
        UnicodeWidthStr::width(" · ") + UnicodeWidthStr::width(intent.as_str())
    });
    let reserved = UnicodeWidthStr::width(format!("    {} {}", icon, display_name).as_str())
        + 1
        + intent_width
        + token_badge.as_ref().map_or(0, |(label, _)| {
            UnicodeWidthStr::width(format!(" · {label}").as_str())
        });
    let summary_budget = max_width.map(|w| w.saturating_sub(reserved));
    let summary = get_tool_summary_with_budget(tool, bash_max_chars, summary_budget);

    let mut spans = vec![
        Span::styled(format!("    {} ", icon), Style::default().fg(icon_color)),
        Span::styled(display_name, Style::default().fg(tool_color())),
    ];
    if let Some(intent) = intent_display {
        spans.push(Span::styled(" · ", Style::default().fg(dim_color())));
        spans.push(Span::styled(
            intent.clone(),
            Style::default().fg(tool_color()),
        ));
        let show_detail = show_tool_call_details();
        if show_detail && !summary.is_empty() && summary != intent {
            spans.push(Span::styled(" · ", Style::default().fg(dim_color())));
            spans.push(Span::styled(summary, Style::default().fg(dim_color())));
        }
    } else if !summary.is_empty() {
        spans.push(Span::styled(
            format!(" {}", summary),
            Style::default().fg(dim_color()),
        ));
    }
    let token_suffix = token_badge.map(|(label, color)| {
        let mut spans = vec![Span::styled(" · ", Style::default().fg(dim_color()))];
        spans.extend(tool_usage_spans(
            label,
            color,
            None,
            output_content.unwrap_or_default(),
        ));
        Line::from(spans)
    });

    if let (Some(max_width), Some(token_suffix)) = (max_width, token_suffix.as_ref()) {
        return truncate_line_preserving_suffix_to_width(
            &Line::from(spans),
            token_suffix,
            max_width,
        );
    }

    if let Some(token_suffix) = token_suffix {
        spans.extend(token_suffix.spans);
    }

    Line::from(spans)
}

pub(in super::super) fn summarize_batch_running_tools_compact(
    running: &[ToolCall],
) -> Option<String> {
    if running.is_empty() {
        return None;
    }

    let mut running_sorted = running.to_vec();
    running_sorted.sort_by(|a, b| {
        batch_subcall_index(&a.id)
            .unwrap_or(usize::MAX)
            .cmp(&batch_subcall_index(&b.id).unwrap_or(usize::MAX))
            .then_with(|| a.id.cmp(&b.id))
    });

    let first = &running_sorted[0];
    let label = match batch_subcall_index(&first.id) {
        Some(idx) => format!("#{} {}", idx, first.name),
        None => first.name.clone(),
    };

    if running_sorted.len() == 1 {
        Some(label)
    } else {
        Some(format!("{} +{}", label, running_sorted.len() - 1))
    }
}
