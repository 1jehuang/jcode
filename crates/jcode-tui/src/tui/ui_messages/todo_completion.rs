//! Snapshot-based completion reports with compact chat status dots.
use super::*;

#[derive(serde::Deserialize)]
struct Snapshot {
    todo_completion_version: u32,
    #[serde(default)]
    observations: Vec<crate::todo::GateObservation>,
    #[serde(default)]
    todos: Vec<crate::todo::TodoItem>,
    #[serde(default)]
    completed_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    plan: CompletionPlan,
}

#[derive(Default, serde::Deserialize)]
struct CompletionPlan {
    #[serde(default)]
    title: Option<String>,
}

pub(crate) fn completed_at(msg: &DisplayMessage) -> Option<chrono::DateTime<chrono::Utc>> {
    if msg.title.as_deref() != Some("Todo completion") || !msg.content.contains("\"completed_at\"")
    {
        return None;
    }
    let snapshot: Snapshot = serde_json::from_str(&msg.content).ok()?;
    if snapshot.todo_completion_version != 1 {
        return None;
    }
    snapshot.completed_at
}

pub(crate) fn format_time_ago_at(
    time: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
) -> String {
    let duration = now.signed_duration_since(time);
    let seconds = duration.num_seconds().max(0);
    if seconds < 60 {
        return format!("{}s ago", seconds);
    }
    let minutes = duration.num_minutes();
    if minutes < 60 {
        return format!("{}m ago", minutes);
    }
    let hours = duration.num_hours();
    if hours < 24 {
        return format!("{}h ago", hours);
    }
    let days = duration.num_days();
    if days < 7 {
        return format!("{}d ago", days);
    }
    if days < 30 {
        return format!("{}w ago", days / 7);
    }
    format!("{}mo ago", days / 30)
}

pub(crate) fn next_age_change_delay(
    completed_at: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
) -> std::time::Duration {
    let secs = now.signed_duration_since(completed_at).num_seconds();
    if secs < 0 {
        return std::time::Duration::from_secs(1);
    }
    let next_secs = if secs < 60 {
        secs + 1
    } else if secs < 3600 {
        (secs / 60 + 1) * 60
    } else if secs < 86_400 {
        (secs / 3600 + 1) * 3600
    } else {
        (secs / 86_400 + 1) * 86_400
    };
    let next_at = completed_at + chrono::Duration::seconds(next_secs);
    next_at
        .signed_duration_since(now)
        .to_std()
        .unwrap_or(std::time::Duration::from_secs(1))
        .max(std::time::Duration::from_millis(50))
}

/// Update an already-rendered todo completion line's trailing `"  finished ..."`
/// span in place so cached transcript frames stay live without rebuilding the
/// body cache.
pub(crate) fn refresh_age_in_line(
    line: &mut Line<'static>,
    completed_at: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
    width: usize,
) -> bool {
    let Some(span) = line
        .spans
        .iter_mut()
        .rev()
        .find(|span| span.content.starts_with("  finished "))
    else {
        return false;
    };
    span.content = std::borrow::Cow::Owned(format!(
        "  finished {}",
        format_time_ago_at(completed_at, now)
    ));
    let alignment = line.alignment;
    let mut updated = super::super::truncate_line_with_ellipsis_to_width(line, width.max(1));
    updated.alignment = alignment;
    *line = updated;
    true
}

pub(super) fn render(
    msg: &DisplayMessage,
    width: u16,
    _diff_mode: crate::config::DiffDisplayMode,
) -> Option<Vec<Line<'static>>> {
    render_with_gate_diagnostics(
        msg,
        width,
        crate::config::config().features.todo_gate_diagnostics,
    )
}

fn render_with_gate_diagnostics(
    msg: &DisplayMessage,
    width: u16,
    show_gate_diagnostics: bool,
) -> Option<Vec<Line<'static>>> {
    if msg.title.as_deref() != Some("Todo completion") {
        return None;
    }
    // Legacy Markdown reports and unknown versions retain the normal system
    // renderer. Never fetch live state while rendering a historic message.
    let snapshot: Snapshot = match serde_json::from_str(&msg.content) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            crate::logging::debug(&format!(
                "Todo completion uses legacy text fallback: {error}"
            ));
            return None;
        }
    };
    if snapshot.todo_completion_version != 1 {
        return None;
    }
    let width = width.max(1) as usize;
    let mut lines =
        render_finished_todo_summary(&snapshot.todos, "", width, Some("Todo completion"));
    // Keep the age on the last lane row, below the optional plan heading.
    if let Some(completed_at) = snapshot.completed_at
        && let Some(last) = lines.last_mut()
    {
        let mut spans = last.spans.clone();
        spans.push(Span::styled(
            format!(
                "  finished {}",
                crate::tui::session_picker::format_time_ago(completed_at)
            ),
            Style::default().fg(todo_meta_color()),
        ));
        *last = super::super::truncate_line_with_ellipsis_to_width(&Line::from(spans), width);
    }
    if let Some(title) = snapshot.plan.title {
        let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
        if !title.is_empty() {
            lines.insert(
                0,
                super::super::truncate_line_with_ellipsis_to_width(
                    &Line::from(Span::styled(
                        title,
                        Style::default().fg(todo_group_color()).bold(),
                    )),
                    width,
                ),
            );
        }
    }
    if show_gate_diagnostics {
        lines.extend(render_gate_categories(&snapshot.observations, width));
    }
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
    fn todo_completion_plan_title_precedes_lanes_and_age() {
        let mut msg = DisplayMessage::system(serde_json::json!({
            "todo_completion_version": 1,
            "plan": {"title": "  Ship\n界 fix  "},
            "todos": [
                {"id":"a", "content":"Hidden", "status":"completed", "priority":"high", "group":"main"},
                {"id":"b", "content":"Hidden", "status":"completed", "priority":"high", "group":"markdown"}
            ],
            "completed_at": chrono::Utc::now()
        }).to_string());
        msg.title = Some("Todo completion".into());
        for width in 1..100 {
            let lines = render_with_gate_diagnostics(&msg, width, false).unwrap();
            assert_eq!(lines.len(), 3);
            assert!(lines.iter().all(|line| line.width() <= width as usize));
            if width >= 60 {
                assert_eq!(lines[0].to_string(), "Ship 界 fix");
                assert!(lines[1].to_string().starts_with("✓ main"));
                assert!(!lines[1].to_string().contains("finished"));
                assert!(lines[2].to_string().contains("finished"));
            }
        }
        for title in [serde_json::Value::Null, serde_json::json!(" \n\t ")] {
            msg.content = serde_json::json!({
                "todo_completion_version": 1, "plan": {"title": title}
            })
            .to_string();
            let lines = render_with_gate_diagnostics(&msg, 80, false).unwrap();
            assert_eq!(lines.len(), 1);
        }
    }

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
        let hidden = render_with_gate_diagnostics(&msg, 80, false).unwrap();
        assert_eq!(hidden.len(), 1);
        assert!(!hidden[0].to_string().contains("gates"));
        let lines = render_with_gate_diagnostics(&msg, 80, true).unwrap();
        let categories = render_gate_categories(&observations, 80);
        assert_eq!(&lines[1..7], categories.as_slice());
        assert_eq!(lines.len(), 7);
        assert_eq!(lines[0].to_string(), "✓ Todo completion  ●");
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
            assert_eq!(lines.len(), 1);
            if width >= 20 {
                assert_eq!(lines[0].to_string(), "✓ Todo completion  ●");
                assert!(
                    lines[0].spans.iter().any(
                        |span| span.content == "●" && span.style.fg == Some(rgb(100, 180, 100))
                    )
                );
            }
            assert!(
                !lines
                    .iter()
                    .any(|line| line.to_string().contains("Completed work"))
            );
            assert!(lines.iter().all(|line| line.width() <= width as usize));
        }
    }

    #[test]
    fn todo_completion_hides_intent_and_fits_inline_dots() {
        for state in ["uncertain", "partial", "clear", "complete"] {
            let mut msg = DisplayMessage::system(serde_json::json!({
                "todo_completion_version": 1,
                "todos": [{"id":"done", "content":"Finished 界 work", "status":"completed", "priority":"high", "details":"Hidden description"}],
                "plan": {"understands_user_intent":state, "user_intention":"  Ship\n the fix 界  "},
                "goals": []
            }).to_string());
            msg.title = Some("Todo completion".into());
            let recorded = msg.content.clone();
            for width in [1, 2, 8, 15, 16, 24, 40, 80] {
                let lines = render(&msg, width, crate::config::DiffDisplayMode::Off).unwrap();
                assert_eq!(lines.len(), 1, "header must never wrap");
                assert!(lines.iter().all(|line| line.width() <= width as usize));
                let header = lines[0].to_string();
                if width >= 24 {
                    assert_eq!(header, "✓ Todo completion  ●");
                }
                let text = lines
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(!text.contains("Finished"));
                assert!(!text.contains("Hidden"));
                assert!(!text.contains("Ship"));
                assert!(!text.contains(&format!("Intent {state}")));
                assert_eq!(msg.content, recorded);
            }
        }
    }

    #[test]
    fn todo_completion_missing_intent_never_substitutes_understanding_state() {
        for intention in [None, Some(" \n\t ")] {
            let mut msg = DisplayMessage::system(serde_json::json!({
                "todo_completion_version":1,
                "todos":[{"id":"done","content":"Finished","status":"completed","priority":"high"}],
                "plan":{"understands_user_intent":"clear","user_intention":intention}
            }).to_string());
            msg.title = Some("Todo completion".into());
            let lines = render(&msg, 80, crate::config::DiffDisplayMode::Off).unwrap();
            assert_eq!(lines[0].to_string(), "✓ Todo completion  ●");
            assert_eq!(lines.len(), 1);
        }
    }

    #[test]
    fn todo_completion_many_dots_stay_on_one_row_and_keep_terminal_statuses() {
        for count in [0, 2, 12, 100] {
            let todos: Vec<_> = (0..count)
                .map(|index| {
                    serde_json::json!({
                        "id":index.to_string(), "content":"Hidden task", "priority":"high",
                        "status":if index == 1 { "cancelled" } else { "completed" }
                    })
                })
                .collect();
            let mut msg = DisplayMessage::system(
                serde_json::json!({
                    "todo_completion_version":1,"todos":todos,
                    "plan":{"user_intention":"Do not render me"}
                })
                .to_string(),
            );
            msg.title = Some("Todo completion".into());
            for width in 1..120 {
                let lines = render(&msg, width, crate::config::DiffDisplayMode::Off).unwrap();
                assert_eq!(lines.len(), 1);
                assert!(lines[0].width() <= width as usize);
                assert!(!lines[0].to_string().contains("Do not render me"));
            }
            if count == 2 {
                let lines = render(&msg, 80, crate::config::DiffDisplayMode::Off).unwrap();
                assert_eq!(lines[0].to_string(), "✓ Todo completion  ●○");
            }
        }
    }

    #[test]
    fn todo_completion_groups_finished_tasks_by_lane_without_item_or_intent_text() {
        let mut msg = DisplayMessage::system(serde_json::json!({
            "todo_completion_version":1,
            "todos":[
                {"id":"a","content":"Hidden item A","status":"completed","priority":"high","group":" Rendering 界 ","stage":1},
                {"id":"b","content":"Hidden item B","status":"completed","priority":"high","group":"Tests","stage":1},
                {"id":"c","content":"Hidden item C","status":"cancelled","priority":"high","group":"Rendering\n界","stage":2},
                {"id":"d","content":"Hidden item D","status":"completed","priority":"high","group":"  "}
            ],
            "plan":{"user_intention":"Hidden long intention"}
        }).to_string());
        msg.title = Some("Todo completion".into());
        let saved = msg.content.clone();
        let lines = render(&msg, 80, crate::config::DiffDisplayMode::Off).unwrap();
        assert_eq!(
            lines.iter().map(ToString::to_string).collect::<Vec<_>>(),
            [
                "✓ Rendering 界  ●○",
                "✓ Tests         ●",
                "✓ other         ●"
            ]
        );
        let dot_columns: Vec<_> = lines
            .iter()
            .map(|line| {
                let text = line.to_string();
                text.split('●').next().unwrap().width()
            })
            .collect();
        assert!(dot_columns.iter().all(|column| *column == dot_columns[0]));
        for line in &lines {
            assert!(line.spans.iter().any(|span| {
                span.content == "✓ " && span.style.fg == Some(todo_score_color())
            }));
            assert!(line.spans.iter().any(|span| {
                span.style.fg == Some(todo_group_color())
                    && span.style.add_modifier.contains(Modifier::BOLD)
            }));
        }
        for width in 1..90 {
            let lines = render(&msg, width, crate::config::DiffDisplayMode::Off).unwrap();
            assert_eq!(lines.len(), 3);
            for line in lines {
                assert!(line.width() <= width as usize, "width={width}, {line}");
                let text = line.to_string();
                assert!(!text.contains("Hidden") && !text.contains("Todo completion"));
                if width >= 4 {
                    assert!(text.contains('●') || text.contains('○'), "{text}");
                }
            }
        }
        assert_eq!(saved, msg.content);
    }

    #[test]
    fn todo_completion_refresh_age_in_line_updates_cached_timestamp_and_delays() {
        let completed_at = chrono::DateTime::parse_from_rfc3339("2026-11-28T07:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let mut msg = DisplayMessage::system(
            serde_json::json!({
                "todo_completion_version": 1,
                "todos": [{"id": "a", "content": "Hidden", "status": "completed", "priority": "high", "group": "main"}],
                "completed_at": completed_at
            })
            .to_string(),
        );
        msg.title = Some("Todo completion".into());
        assert_eq!(super::completed_at(&msg), Some(completed_at));

        let mut line = Line::from(vec![
            Span::raw("✓ main  ●"),
            Span::styled("  finished 0s ago", Style::default().fg(todo_meta_color())),
        ]);
        let five_secs_later = completed_at + chrono::Duration::seconds(5);
        assert!(refresh_age_in_line(
            &mut line,
            completed_at,
            five_secs_later,
            80
        ));
        assert_eq!(line.to_string(), "✓ main  ●  finished 5s ago");
        assert_eq!(
            next_age_change_delay(completed_at, five_secs_later),
            std::time::Duration::from_secs(1)
        );

        let two_mins_later = completed_at + chrono::Duration::seconds(125);
        assert!(refresh_age_in_line(
            &mut line,
            completed_at,
            two_mins_later,
            80
        ));
        assert_eq!(line.to_string(), "✓ main  ●  finished 2m ago");
        assert_eq!(
            next_age_change_delay(completed_at, two_mins_later),
            std::time::Duration::from_secs(55)
        );
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
