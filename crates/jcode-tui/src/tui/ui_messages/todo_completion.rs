//! Snapshot-based completion reports, with the same card as the pinned band.
use super::*;

#[derive(serde::Deserialize)]
struct Snapshot {
    todo_completion_version: u32,
    #[serde(default)]
    observations: Vec<crate::todo::GateObservation>,
}

pub(super) fn render(
    msg: &DisplayMessage,
    width: u16,
    diff_mode: crate::config::DiffDisplayMode,
) -> Option<Vec<Line<'static>>> {
    if msg.title.as_deref() != Some("Todo completion") {
        return None;
    }
    // Legacy Markdown reports and unknown versions retain the normal system
    // renderer. Never fetch live state while rendering a historic message.
    let snapshot: Snapshot = serde_json::from_str(&msg.content).ok()?;
    if snapshot.todo_completion_version != 1 {
        return None;
    }
    let width = width.max(1) as usize;
    let mut lines = markdown::wrap_lines(
        vec![Line::from(Span::styled(
            "Todo completion",
            Style::default().fg(todo_score_color()).bold(),
        ))],
        width,
    );
    lines.extend(render_gate_categories(&snapshot.observations, width));
    // The snapshot is also a valid current card payload. Reuse the actual
    // pinned renderer, including its grouping, stages, pips and assessments.
    lines.extend(render_pinned_todos_message(msg, width as u16, diff_mode));
    // Preserve the pinned card's spans verbatim. Only pathological terminal
    // widths smaller than its indent need an extra wrapping pass.
    let lines: Vec<Line<'static>> = lines
        .into_iter()
        .flat_map(|line| {
            if line.width() > width {
                markdown::wrap_lines(vec![line], width)
            } else {
                vec![line]
            }
        })
        .collect();
    Some(highlight_panel(lines, width))
}

/// Completion reports are milestones, so lift them off the transcript with a
/// tinted panel, the same treatment user prompts get from `user_bg()`, nudged
/// toward the todo accent green. Lines are padded to the full width so the
/// panel reads as one block.
fn highlight_panel(lines: Vec<Line<'static>>, width: usize) -> Vec<Line<'static>> {
    let bg = jcode_tui_style::theme::blend_color(
        jcode_tui_style::theme::user_bg(),
        todo_score_color(),
        0.12,
    );
    lines
        .into_iter()
        .map(|mut line| {
            for span in &mut line.spans {
                if span.style.bg.is_none() {
                    span.style = span.style.bg(bg);
                }
            }
            let pad = width.saturating_sub(line.width());
            if pad > 0 {
                line.spans
                    .push(Span::styled(" ".repeat(pad), Style::default().bg(bg)));
            }
            line
        })
        .collect()
}

fn render_gate_categories(
    observations: &[crate::todo::GateObservation],
    width: usize,
) -> Vec<Line<'static>> {
    use crate::todo::GateObservationKind::*;
    if observations.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![super::super::truncate_line_with_ellipsis_to_width(
        &Line::from(Span::styled(
            "  Disabled gates that would have fired",
            Style::default().fg(todo_group_color()).bold(),
        )),
        width,
    )];
    for (kind, label) in [
        (IntentUnderstanding, "Intent understanding"),
        (ClosedFeedbackLoop, "Feedback loop closure"),
        (FeedbackLoopRelevance, "Feedback loop relevance"),
        (FeedbackLoopCoverage, "Feedback loop coverage"),
        (
            FeedbackLoopTraceability,
            "Requirement-to-check traceability",
        ),
    ] {
        if observations
            .iter()
            .any(|observation| observation.kind == kind)
        {
            lines.push(super::super::truncate_line_with_ellipsis_to_width(
                &Line::from(Span::styled(
                    format!("  • {label}"),
                    Style::default().fg(todo_warning_color()),
                )),
                width,
            ));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn todo_completion_categories_are_unique_and_above_cards() {
        use crate::todo::{GateObservation, GateObservationKind::*};
        let observations: Vec<_> = [
            ClosedFeedbackLoop,
            FeedbackLoopTraceability,
            ClosedFeedbackLoop,
            IntentUnderstanding,
            FeedbackLoopRelevance,
            FeedbackLoopCoverage,
        ]
        .into_iter()
        .map(|kind| GateObservation {
            kind,
            group: Some("hidden group".into()),
            state: Some("hidden state".into()),
        })
        .collect();
        assert!(render_gate_categories(&[], 80).is_empty());
        for width in [1, 20, 40, 80] {
            let lines = render_gate_categories(&observations, width);
            assert_eq!(lines.len(), 6);
            assert!(lines.iter().all(|line| line.width() <= width));
        }
        let mut msg = DisplayMessage::system(serde_json::json!({
            "todo_completion_version": 1, "observations": observations,
            "todos": [{"id":"done", "content":"Completed work", "status":"completed", "priority":"high"}],
            "plan": {}, "goals": []
        }).to_string());
        msg.title = Some("Todo completion".into());
        let lines = render_system_message(&msg, 80, crate::config::DiffDisplayMode::Off);
        let categories = highlight_panel(render_gate_categories(&observations, 80), 80);
        assert_eq!(&lines[1..7], categories.as_slice());
        let card = highlight_panel(
            render_pinned_todos_message(&msg, 80, crate::config::DiffDisplayMode::Off),
            80,
        );
        assert_eq!(&lines[7..], card.as_slice());
        let text = lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(text.matches("• Feedback loop closure").count(), 1);
        assert!(text.contains("• Requirement-to-check traceability"));
        assert!(!text.contains("hidden group"));
        assert!(!text.contains("hidden state"));
        assert!(!text.contains("flagged"));
        assert!(!text.contains("Run every"));
    }

    #[test]
    fn todo_completion_reuses_styled_pinned_cards_at_all_widths() {
        let mut msg = DisplayMessage::system(
            serde_json::json!({
                "todo_completion_version": 1,
                "todos": [{"id": "done", "content": "Completed work", "status": "completed",
                    "priority": "high", "completion_confidence": "verified"}],
                "plan": {},
                "goals": []
            })
            .to_string(),
        );
        msg.title = Some("Todo completion".into());
        for width in [20, 40, 80] {
            let lines = render_system_message(&msg, width, crate::config::DiffDisplayMode::Off);
            let card = highlight_panel(
                render_pinned_todos_message(&msg, width, crate::config::DiffDisplayMode::Off),
                width as usize,
            );
            assert!(!card.is_empty());
            // The whole report sits on one full-width tinted panel.
            assert!(lines.iter().all(|line| {
                line.width() == width as usize
                    && line.spans.iter().all(|span| span.style.bg.is_some())
            }));
            // Line equality checks semantic styles as well as actual text.
            assert_eq!(&lines[1..1 + card.len()], card.as_slice());
            assert_eq!(
                lines.len(),
                1 + card.len(),
                "legacy snapshots have no appendix"
            );
            assert!(lines.iter().all(|line| line.width() <= width as usize));
        }
    }

    #[test]
    fn todo_completion_legacy_and_unknown_payloads_fall_back() {
        for content in [
            "# Todo completion report\n\nHistoric Markdown evidence",
            r#"{"todo_completion_version":2,"todos":[],"plan":{},"goals":[]}"#,
            "malformed snapshot",
        ] {
            let mut msg = DisplayMessage::system(content.to_owned());
            msg.title = Some("Todo completion".into());
            assert!(render(&msg, 40, crate::config::DiffDisplayMode::Off).is_none());
            assert!(
                !render_system_message(&msg, 40, crate::config::DiffDisplayMode::Off).is_empty()
            );
        }
    }
}
