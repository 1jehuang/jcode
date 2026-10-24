//! Snapshot-based completion reports, with the same card as the pinned band.
use super::*;

#[derive(serde::Deserialize)]
struct Snapshot {
    todo_completion_version: u32,
    #[serde(default)]
    observations: Vec<crate::todo::GateObservation>,
    plan: crate::todo::TodoPlan,
    goals: Vec<crate::todo::TodoGoal>,
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
    // The snapshot is also a valid current card payload. Reuse the actual
    // pinned renderer, including its grouping, stages, pips and assessments.
    lines.extend(render_pinned_todos_message(msg, width as u16, diff_mode));
    if let Some(digest) =
        crate::todo::build_gate_digest(&snapshot.observations, &snapshot.plan, &snapshot.goals)
    {
        lines.push(Line::default());
        lines.extend(render_detail_rows(&digest, width));
    }
    // Preserve the pinned card's spans verbatim. Only pathological terminal
    // widths smaller than its indent need an extra wrapping pass.
    Some(
        lines
            .into_iter()
            .flat_map(|line| {
                if line.width() > width {
                    markdown::wrap_lines(vec![line], width)
                } else {
                    vec![line]
                }
            })
            .collect(),
    )
}

fn render_detail_rows(details: &str, width: usize) -> Vec<Line<'static>> {
    let mut lines = markdown::wrap_lines(
        vec![Line::from(Span::styled(
            "  Disabled turn-end gate findings",
            Style::default().fg(todo_group_color()).bold(),
        ))],
        width,
    );
    // Only digest bullets are display data. Its automatic continuation header
    // must never appear here or be dispatched as a model instruction.
    for item in details.lines().filter_map(|line| line.strip_prefix("- ")) {
        let text = format!("• {item}");
        let rendered = markdown::wrap_lines(vec![Line::raw(text)], width.saturating_sub(2).max(1));
        for mut line in rendered {
            line.spans.insert(0, Span::raw("  "));
            for span in &mut line.spans {
                span.style.fg = Some(todo_warning_color());
            }
            lines.extend(markdown::wrap_lines(vec![line], width));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn todo_completion_details_use_compact_warning_rows() {
        let lines = render_detail_rows(
            "[auto] Do not reply or wait for the user.\n- acceptance path blocked",
            80,
        );
        assert_eq!(lines.len(), 2);
        let warning = lines
            .iter()
            .find(|line| line.to_string().contains("acceptance path blocked"))
            .unwrap();
        assert!(
            warning
                .spans
                .iter()
                .all(|span| span.style.fg == Some(todo_warning_color()))
        );
        assert!(
            lines[0]
                .to_string()
                .contains("Disabled turn-end gate findings")
        );
        assert!(!lines.iter().any(|line| line.to_string().contains("[auto]")));
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
            let card =
                render_pinned_todos_message(&msg, width, crate::config::DiffDisplayMode::Off);
            assert!(!card.is_empty());
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
