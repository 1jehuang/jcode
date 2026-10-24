//! Snapshot-based completion reports with compact chat status dots.
use super::*;

#[derive(serde::Deserialize)]
struct Snapshot {
    todo_completion_version: u32,
    #[serde(default)]
    observations: Vec<crate::todo::GateObservation>,
    #[serde(default)]
    plan: crate::todo::TodoPlan,
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
    let mut header = vec![Span::styled(
        "Todo completion",
        Style::default().fg(todo_score_color()).bold(),
    )];
    if let Some(state) = snapshot.plan.understands_user_intent {
        let color = match state {
            crate::todo::IntentUnderstanding::Uncertain => todo_failure_color(),
            crate::todo::IntentUnderstanding::Partial => todo_warning_color(),
            crate::todo::IntentUnderstanding::Clear
            | crate::todo::IntentUnderstanding::Complete => todo_score_color(),
        };
        header.extend([
            Span::styled("  Intent ", Style::default().fg(todo_label_color())),
            Span::styled(state.as_str().to_owned(), Style::default().fg(color)),
        ]);
    }
    let mut lines = vec![super::super::truncate_line_with_ellipsis_to_width(
        &Line::from(header),
        width,
    )];
    lines.extend(render_gate_categories(&snapshot.observations, width));
    // Render the recorded payload through the same compact chat card path.
    // The pinned band intentionally retains its detailed presentation.
    lines.extend(render_todos_message(msg, width as u16, diff_mode));
    Some(lines)
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
        let categories = render_gate_categories(&observations, 80);
        assert_eq!(&lines[1..7], categories.as_slice());
        let card = render_todos_message(&msg, 80, crate::config::DiffDisplayMode::Off);
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
    fn todo_completion_reuses_styled_chat_dots_at_all_widths() {
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
        for width in [1, 2, 10, 20, 40, 80] {
            let lines = render_system_message(&msg, width, crate::config::DiffDisplayMode::Off);
            let card = render_todos_message(&msg, width, crate::config::DiffDisplayMode::Off);
            assert!(!card.is_empty());
            // Line equality checks semantic styles as well as actual text.
            assert_eq!(&lines[1..1 + card.len()], card.as_slice());
            assert_eq!(
                lines.len(),
                1 + card.len(),
                "legacy snapshots have no appendix"
            );
            assert_eq!(card.len(), 1);
            assert!(!lines
                .iter()
                .any(|line| line.to_string().contains("Completed work")));
            assert!(lines.iter().all(|line| line.width() <= width as usize));
        }
    }

    #[test]
    fn todo_completion_intent_is_inline_state_derived_and_fitted() {
        for (state, color) in [
            ("uncertain", todo_failure_color()),
            ("partial", todo_warning_color()),
            ("clear", todo_score_color()),
            ("complete", todo_score_color()),
        ] {
            let mut msg = DisplayMessage::system(serde_json::json!({
                "todo_completion_version": 1,
                "todos": [{"id":"done", "content":"Finished 界 work", "status":"completed", "priority":"high", "details":"Hidden description"}],
                "plan": {"understands_user_intent":state, "user_intention":"Hidden intention 界"},
                "goals": []
            }).to_string());
            msg.title = Some("Todo completion".into());
            let recorded = msg.content.clone();
            for width in [1, 2, 8, 15, 16, 24, 40, 80] {
                let lines = render(&msg, width, crate::config::DiffDisplayMode::Off).unwrap();
                assert_eq!(lines.len(), 2, "header must never wrap");
                assert!(lines.iter().all(|line| line.width() <= width as usize));
                let header = lines[0].to_string();
                if width >= 40 {
                    assert_eq!(header, format!("Todo completion  Intent {state}"));
                    assert!(lines[0]
                        .spans
                        .iter()
                        .any(|span| span.content == state && span.style.fg == Some(color)));
                } else {
                    assert!(header.ends_with('…'));
                }
                let text = lines
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(!text.contains("Finished"));
                assert!(!text.contains("Hidden"));
                assert!(!lines[1].to_string().contains("Intent"));
                assert_eq!(msg.content, recorded);
            }
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
