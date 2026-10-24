//! Snapshot-based completion reports, with the same card as the pinned band.
use super::*;

#[derive(serde::Deserialize)]
struct Snapshot {
    todo_completion_version: u32,
    todos: Vec<crate::todo::TodoItem>,
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
    lines.push(Line::default());

    // Cards intentionally abbreviate prose. Keep the full evidence underneath
    // in compact semantic rows rather than a second, large Markdown document.
    let details = crate::todo::completion_report::render_completion_report(
        &snapshot.todos,
        &snapshot.plan,
        &snapshot.goals,
    );
    lines.extend(render_detail_rows(&details, width));
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
    let mut lines = Vec::new();
    let mut weaknesses = false;
    for raw in details.lines().filter(|line| !line.is_empty()) {
        if raw.starts_with("# ") {
            continue;
        }
        let (text, color, bold) = if let Some(heading) = raw.strip_prefix("## ") {
            weaknesses = heading.starts_with("Current weaknesses")
                || heading.starts_with("Earlier weak states");
            lines.push(Line::default());
            (heading, todo_group_color(), true)
        } else if let Some(heading) = raw.strip_prefix("### ") {
            (heading, todo_group_color(), true)
        } else if let Some(item) = raw.strip_prefix("- ") {
            (
                item,
                if weaknesses {
                    todo_warning_color()
                } else {
                    todo_meta_color()
                },
                false,
            )
        } else {
            (raw, todo_meta_color(), false)
        };
        // Decode the formatter's escaped data through the existing Markdown
        // parser. It cannot become active markup because punctuation is escaped.
        let rendered = markdown::render_markdown_with_width(text, Some(width));
        for mut line in rendered {
            line.spans.insert(0, Span::raw("  "));
            for span in &mut line.spans {
                span.style.fg = Some(color);
                if bold {
                    span.style = span.style.bold();
                }
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
            "# Todo completion report\n\n## Current weaknesses\n\n- acceptance path blocked\n\n## Plan\n\n- **User intention:** Preserve all evidence\n- **Intent:** clear\n",
            80,
        );
        assert_eq!(lines.len(), 7);
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
        assert!(lines.iter().any(|line| {
            line.to_string()
                .contains("User intention: Preserve all evidence")
        }));
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
