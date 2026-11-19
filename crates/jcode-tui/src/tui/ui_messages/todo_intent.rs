//! Plan-level intent rendering for the todo card.
//!
//! The intent is the operator's own objective, so it has to stay readable: at
//! narrow (compact) widths the card keeps the single-line ellipsized contract,
//! and at wide widths it wraps instead of being cut in half.

use super::*;

/// Plan-level assessment lines shown once above the todo groups.
pub(super) fn push_todo_plan_details(
    lines: &mut Vec<Line<'static>>,
    plan: &crate::todo::TodoPlan,
    base_indent: &str,
    inner_width: usize,
    compact_details: bool,
) {
    let intention = plan
        .user_intention
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let Some(state) = plan.understands_user_intent {
        let state_color = match state {
            crate::todo::IntentUnderstanding::Uncertain => todo_failure_color(),
            crate::todo::IntentUnderstanding::Partial => todo_warning_color(),
            crate::todo::IntentUnderstanding::Clear
            | crate::todo::IntentUnderstanding::Complete => todo_score_color(),
        };
        // Compact mode keeps the single-line ellipsized contract; wide widths wrap
        // instead, because `todo_card_line` re-truncates and smashed the objective.
        if let Some(intention) = intention
            && !compact_details
        {
            push_wrapped_intent(
                lines,
                state.as_str(),
                state_color,
                intention,
                base_indent,
                inner_width,
            );
            return;
        }
        let mut spans = vec![
            Span::styled("Intent ", Style::default().fg(todo_label_color())),
            Span::styled(state.as_str().to_string(), Style::default().fg(state_color)),
            Span::styled(": ", Style::default().fg(todo_label_color())),
        ];
        if let Some(intention) = intention {
            spans.push(Span::styled(
                intention.to_string(),
                Style::default().fg(todo_meta_color()),
            ));
        }
        lines.push(todo_card_line(spans, base_indent, inner_width));
    } else if let Some(intention) = intention {
        push_todo_detail(
            lines,
            "Intent",
            intention,
            base_indent,
            inner_width,
            compact_details,
        );
    }
}

/// Wrap the plan intent, keeping the state name in its own span.
///
/// The state name stays its own span so the semantic-colour assertions can match
/// `span.content == state.as_str()`, and the body is emitted line by line rather
/// than through `todo_card_line`, whose re-truncation to the card width would
/// ellipsize it again.
fn push_wrapped_intent(
    lines: &mut Vec<Line<'static>>,
    state: &str,
    state_color: Color,
    intention: &str,
    base_indent: &str,
    inner_width: usize,
) {
    let prefix = format!("Intent {state}: ");
    let prefix_width = prefix.width();
    let available = inner_width
        .saturating_sub(prefix_width)
        .saturating_sub(base_indent.width())
        .max(1);
    for (index, chunk) in wrap_todo_detail(intention, available)
        .into_iter()
        .enumerate()
    {
        let mut spans: Vec<Span<'static>> = vec![Span::raw(base_indent.to_string())];
        if index == 0 {
            spans.push(Span::styled(
                "Intent ",
                Style::default().fg(todo_label_color()),
            ));
            spans.push(Span::styled(
                state.to_string(),
                Style::default().fg(state_color),
            ));
            spans.push(Span::styled(": ", Style::default().fg(todo_label_color())));
        } else {
            // Continuation lines indent under the prefix so the body stays aligned.
            spans.push(Span::styled(
                " ".repeat(prefix_width),
                Style::default().fg(todo_label_color()),
            ));
        }
        spans.push(Span::styled(chunk, Style::default().fg(todo_meta_color())));
        lines.push(Line::from(spans));
    }
}
