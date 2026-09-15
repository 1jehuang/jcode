//! Native TUI elicitation overlay renderer.
//!
//! Renders a centered, bordered panel for structured agent-to-user input
//! (multi-choice, text, boolean, integer, date/time). Follows the same
//! overlay pattern as the session picker and changelog overlays.

use super::{clear_area, dim_color, rgb};
use crate::tui::elicitation_types::{ElicitOverlayState, FieldSpec};
use crate::tui::TuiState;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

/// Render the elicitation overlay as a centered modal panel.
pub(super) fn draw_elicit_overlay(frame: &mut Frame, area: Rect, state: &ElicitOverlayState) {
    // Dim the background.
    clear_area(frame, area);

    let request = &state.request;
    let border_color = request.urgency.border_color();
    let icon = request.urgency.icon();

    // Compute panel dimensions: 60% width, auto-height capped at 70%.
    let panel_w = (area.width * 60 / 100).max(40).min(area.width);
    let panel_h = compute_panel_height(state, panel_w).min(area.height * 70 / 100);

    let panel_x = area.x + (area.width.saturating_sub(panel_w)) / 2;
    let panel_y = area.y + (area.height.saturating_sub(panel_h)) / 2;
    let panel_area = Rect::new(panel_x, panel_y, panel_w, panel_h);

    // Clear the panel area so dimmed background doesn't bleed through.
    frame.render_widget(Clear, panel_area);

    // Build the bordered block.
    let title = format!(" {} {} ", icon, request.title);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(
            title,
            Style::default()
                .fg(border_color)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::from(Span::styled(
            button_footer(request),
            Style::default().fg(dim_color()),
        )));

    let inner = block.inner(panel_area);
    frame.render_widget(block, panel_area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let mut lines: Vec<Line<'static>> = Vec::new();

    // Question / description.
    if let Some(ref question) = request.question {
        for wrapped in text_wrap(question, inner.width as usize) {
            lines.push(Line::from(Span::styled(
                wrapped,
                Style::default().fg(rgb(200, 200, 220)),
            )));
        }
        lines.push(Line::from(""));
    }

    // Field-specific rendering.
    match &request.field {
        FieldSpec::Choice {
            label,
            options,
            default_index,
        } => {
            if !label.is_empty() {
                lines.push(Line::from(Span::styled(
                    label.clone(),
                    Style::default()
                        .fg(rgb(180, 180, 200))
                        .add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(""));
            }
            let selected = state.selected;
            for (i, option) in options.iter().enumerate() {
                let marker = if i == selected { "●" } else { "○" };
                let style = if i == selected {
                    Style::default()
                        .fg(border_color)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(rgb(170, 170, 185))
                };
                let mut spans = vec![
                    Span::styled(format!("  {} ", marker), style),
                    Span::styled(option.label.clone(), style),
                ];
                if let Some(ref desc) = option.description {
                    spans.push(Span::styled(
                        format!("  {}", desc),
                        Style::default().fg(dim_color()),
                    ));
                }
                lines.push(Line::from(spans));
            }
            // Show default hint.
            if let Some(default_idx) = default_index {
                if let Some(default_opt) = options.get(*default_idx) {
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        format!("  Default: {}", default_opt.label),
                        Style::default().fg(dim_color()),
                    )));
                }
            }
        }
        FieldSpec::Boolean { label, default } => {
            if !label.is_empty() {
                lines.push(Line::from(Span::styled(
                    label.clone(),
                    Style::default()
                        .fg(rgb(180, 180, 200))
                        .add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(""));
            }
            // Boolean is rendered as two buttons in the footer.
            let _ = default;
        }
        FieldSpec::Text {
            label,
            default,
            placeholder,
            secret,
            ..
        } => {
            if !label.is_empty() {
                lines.push(Line::from(Span::styled(
                    label.clone(),
                    Style::default()
                        .fg(rgb(180, 180, 200))
                        .add_modifier(Modifier::BOLD),
                )));
            }
            let display_value = if state.text_buffer.is_empty() {
                default
                    .clone()
                    .or_else(|| placeholder.clone())
                    .unwrap_or_default()
            } else if *secret {
                "*".repeat(state.text_buffer.len())
            } else {
                state.text_buffer.clone()
            };
            let input_style = if state.text_buffer.is_empty() {
                Style::default().fg(dim_color())
            } else {
                Style::default().fg(rgb(220, 220, 240))
            };
            lines.push(Line::from(Span::styled(
                format!("  > {}", display_value),
                input_style,
            )));
        }
        FieldSpec::LongText {
            label,
            default,
            ..
        } => {
            if !label.is_empty() {
                lines.push(Line::from(Span::styled(
                    label.clone(),
                    Style::default()
                        .fg(rgb(180, 180, 200))
                        .add_modifier(Modifier::BOLD),
                )));
            }
            let display_value = if state.text_buffer.is_empty() {
                default.clone().unwrap_or_default()
            } else {
                state.text_buffer.clone()
            };
            let input_style = if state.text_buffer.is_empty() {
                Style::default().fg(dim_color())
            } else {
                Style::default().fg(rgb(220, 220, 240))
            };
            for line_text in display_value.lines() {
                lines.push(Line::from(Span::styled(
                    format!("  {}", line_text),
                    input_style,
                )));
            }
            if display_value.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  (Ctrl+Enter to submit)",
                    Style::default().fg(dim_color()),
                )));
            }
        }
        FieldSpec::Integer {
            label,
            default,
            min,
            max,
        } => {
            if !label.is_empty() {
                lines.push(Line::from(Span::styled(
                    label.clone(),
                    Style::default()
                        .fg(rgb(180, 180, 200))
                        .add_modifier(Modifier::BOLD),
                )));
            }
            let value = if state.text_buffer.is_empty() {
                default.map(|v| v.to_string()).unwrap_or_default()
            } else {
                state.text_buffer.clone()
            };
            lines.push(Line::from(Span::styled(
                format!("  > {}", value),
                Style::default().fg(rgb(220, 220, 240)),
            )));
            let range_hint = match (min, max) {
                (Some(lo), Some(hi)) => format!("  Range: {}..{}", lo, hi),
                (Some(lo), None) => format!("  Min: {}", lo),
                (None, Some(hi)) => format!("  Max: {}", hi),
                (None, None) => String::new(),
            };
            if !range_hint.is_empty() {
                lines.push(Line::from(Span::styled(
                    range_hint,
                    Style::default().fg(dim_color()),
                )));
            }
        }
        FieldSpec::DateTime {
            label,
            default,
            ..
        } => {
            if !label.is_empty() {
                lines.push(Line::from(Span::styled(
                    label.clone(),
                    Style::default()
                        .fg(rgb(180, 180, 200))
                        .add_modifier(Modifier::BOLD),
                )));
            }
            let value = if state.text_buffer.is_empty() {
                default.clone().unwrap_or_default()
            } else {
                state.text_buffer.clone()
            };
            lines.push(Line::from(Span::styled(
                format!("  > {}", value),
                Style::default().fg(rgb(220, 220, 240)),
            )));
        }
    }

    // Notes area.
    if let Some(ref notes_spec) = request.notes {
        lines.push(Line::from(""));
        let label = if notes_spec.required {
            format!("{} *", notes_spec.label)
        } else {
            notes_spec.label.clone()
        };
        lines.push(Line::from(Span::styled(
            label,
            Style::default()
                .fg(rgb(180, 180, 200))
                .add_modifier(Modifier::BOLD),
        )));
        let notes_value = notes_spec.default.clone().unwrap_or_default();
        if notes_value.is_empty() {
            lines.push(Line::from(Span::styled(
                "  (optional)",
                Style::default().fg(dim_color()),
            )));
        } else {
            for note_line in notes_value.lines() {
                lines.push(Line::from(Span::styled(
                    format!("  {}", note_line),
                    Style::default().fg(rgb(200, 200, 220)),
                )));
            }
        }
    }

    let paragraph = Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false });
    frame.render_widget(paragraph, inner);
}

/// Compute the panel height based on content.
fn compute_panel_height(state: &ElicitOverlayState, _width: u16) -> u16 {
    let mut h: u16 = 4; // top border + bottom border + padding
    // Question.
    if state.request.question.is_some() {
        h += 2;
    }
    // Field content.
    match &state.request.field {
        FieldSpec::Choice { options, .. } => {
            h += options.len() as u16 + 1;
        }
        FieldSpec::Boolean { .. } => {
            h += 2;
        }
        FieldSpec::Text { .. } | FieldSpec::Integer { .. } | FieldSpec::DateTime { .. } => {
            h += 2;
        }
        FieldSpec::LongText { .. } => {
            h += 4;
        }
    }
    // Notes.
    if state.request.notes.is_some() {
        h += 3;
    }
    h.max(8)
}

/// Build the button footer text.
fn button_footer(request: &crate::tui::elicitation_types::ElicitRequest) -> String {
    let buttons = request
        .buttons
        .as_ref()
        .map(|b| (b.confirm.as_str(), b.cancel.as_str()))
        .unwrap_or(("Confirm", "Cancel"));

    match &request.field {
        FieldSpec::Boolean { .. } => {
            format!(" Y = Yes · N = No · Tab to switch ")
        }
        FieldSpec::Choice { .. } => {
            format!(" ↑↓ Select · Enter {} · Esc {} ", buttons.0, buttons.1)
        }
        _ => {
            format!(" Enter {} · Esc {} ", buttons.0, buttons.1)
        }
    }
}

/// Simple text wrapping for question text.
fn text_wrap(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![text.to_string()];
    }
    let mut result = Vec::new();
    for paragraph in text.split('\n') {
        if paragraph.len() <= max_width {
            result.push(paragraph.to_string());
        } else {
            let mut remaining = paragraph;
            while remaining.len() > max_width {
                // Find last space within max_width.
                let break_at = remaining[..max_width]
                    .rfind(' ')
                    .unwrap_or(max_width);
                result.push(remaining[..break_at].to_string());
                remaining = &remaining[break_at..].trim_start_matches(' ');
            }
            if !remaining.is_empty() {
                result.push(remaining.to_string());
            }
        }
    }
    if result.is_empty() {
        result.push(String::new());
    }
    result
}
