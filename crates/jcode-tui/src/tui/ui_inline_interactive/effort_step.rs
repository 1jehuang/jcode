use super::*;

pub(super) fn model_effort_step_lines(
    picker: &crate::tui::InlineInteractiveState,
    step: &crate::tui::ModelEffortStep,
    available_rows: usize,
) -> Vec<Line<'static>> {
    let dim = Style::default().fg(dim_color());
    let accent = Style::default().fg(rgb(255, 213, 128));
    let mut lines = Vec::new();
    let mut header = vec![
        Span::styled(
            if step.save_default {
                "Default reasoning for "
            } else {
                "Reasoning for "
            },
            dim,
        ),
        Span::styled(picker_entry_pretty_name(&step.model), accent),
    ];
    if let Some(route) = step.model.active_option() {
        header.push(Span::styled(
            format!(
                "  via {} · {}",
                route_provider_display(&route.provider, &route.api_method),
                api_method_display(&route.api_method)
            ),
            dim,
        ));
    }
    let header_rows = usize::from(available_rows > 1);
    if header_rows > 0 {
        lines.push(Line::from(header));
    }
    let hint_rows = usize::from(available_rows > 2);
    let visible = available_rows
        .saturating_sub(header_rows + hint_rows)
        .min(picker.filtered.len());
    if visible == 0 {
        lines.truncate(available_rows);
        return lines;
    }
    let selected = picker.selected.min(picker.filtered.len() - 1);
    let start = selected.saturating_sub(visible - 1);
    for row in start..start + visible {
        let entry = &picker.entries[picker.filtered[row]];
        let style = if row == selected {
            accent
        } else {
            Style::default().fg(rgb(128, 203, 196))
        };
        let mut spans = vec![Span::styled(
            format!(
                "{} {}",
                picker_row_marker(row == selected, false, false),
                entry.name
            ),
            style,
        )];
        if entry.is_current {
            spans.push(Span::styled(" current", dim));
        }
        if entry.recommended {
            spans.push(Span::styled(" ★", dim));
        }
        lines.push(Line::from(spans));
    }
    if hint_rows > 0 {
        let confirm = if step.save_default {
            "Enter save default"
        } else {
            "Enter switch"
        };
        lines.push(Line::from(Span::styled(
            format!("↑↓ level · {confirm} · Esc back to models"),
            dim,
        )));
    }
    lines
}
