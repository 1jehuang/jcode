//! Focused worker headings regression tests.
use super::*;

#[test]
fn grouped_checklist_shows_owner_and_activity_only_in_heading() {
    let mut todos = vec![
        assigned(todo("1", "Read types", "completed", None), "fox"),
        assigned(todo("2", "Update callers", "in_progress", None), "fox"),
        assigned(todo("3", "Add tests", "pending", None), "fox"),
    ];
    for todo in &mut todos {
        todo.group = Some("Parser 界".into());
    }
    let agents = vec![agent("fox", "running", Some("Checking callers"), Some(12))];
    let rows = render(&todos, &agents, 120);
    let text = rows.join("\n");
    assert_eq!(text.matches('🦊').count(), 1, "{rows:?}");
    assert_eq!(text.matches("Checking callers").count(), 1, "{rows:?}");
    let heading = rows.iter().find(|row| row.contains("Parser 界")).unwrap();
    assert!(
        heading.contains('🦊') && heading.contains("Checking callers"),
        "{rows:?}"
    );
    for width in [8, 14, 24, 40, 80] {
        let narrow = render(&todos, &agents, width);
        assert_fits(&narrow, width);
        assert_eq!(narrow.len(), rows.len(), "no wrapping at {width}");
    }
}

#[test]
fn plain_checklist_only_active_row_has_worker_and_duplicate_activity_is_hidden() {
    let todos = vec![
        assigned(todo("1", "Read types", "completed", None), "fox"),
        assigned(todo("2", "Update callers", "in_progress", None), "fox"),
        assigned(todo("3", "Add tests", "pending", None), "fox"),
    ];
    let agents = vec![agent(
        "fox",
        "running",
        Some(" Update   callers "),
        Some(12),
    )];
    let rows = render(&todos, &agents, 120);
    assert_eq!(rows.join("\n").matches('🦊').count(), 1, "{rows:?}");
    assert!(
        rows.iter()
            .find(|r| r.contains("Update callers"))
            .unwrap()
            .contains('🦊')
    );
    assert!(!rows.join("\n").contains(" Update   callers "), "{rows:?}");
    assert!(
        rows.iter().any(|row| row.contains("12s")),
        "age is retained: {rows:?}"
    );
}

#[test]
fn compact_lane_hides_activity_identical_to_a_task_title() {
    let mut todos = live_lane_todos();
    let agents = vec![agent("fox", "running", Some("Update callers"), Some(12))];
    let rows = render(&todos, &agents, 160);
    assert_eq!(
        rows.join("\n").matches("Update callers").count(),
        1,
        "{rows:?}"
    );
    for todo in &mut todos {
        todo.stage = None;
    }
    let rows = render(&todos, &agents, 160);
    assert_eq!(
        rows.join("\n").matches("Update callers").count(),
        1,
        "{rows:?}"
    );
}

#[test]
fn mixed_list_keeps_unrepresented_owners_without_repeating_lane_owners() {
    let mut grouped = assigned(todo("1", "Read types", "in_progress", None), "fox");
    grouped.group = Some("Parser".into());
    let todos = vec![
        grouped,
        assigned(todo("2", "Follow up", "pending", None), "fox"),
        assigned(todo("3", "Documentation", "pending", None), "sheep"),
    ];
    let agents = vec![
        agent("fox", "running", Some("Inspect types"), None),
        agent("sheep", "queued", None, None),
    ];
    let rows = render(&todos, &agents, 120);
    assert!(!rows[0].contains('🦊'), "{rows:?}");
    assert!(rows[0].contains('🐑'), "unstarted owner retained: {rows:?}");
    assert_eq!(rows.join("\n").matches('🦊').count(), 1, "{rows:?}");
    assert_eq!(rows.join("\n").matches('🐑').count(), 1, "{rows:?}");
}

#[test]
fn group_heading_keeps_distinct_owners_once_and_hides_duplicate_failure_reason() {
    let mut todos = vec![
        assigned(todo("1", "修复 parser", "in_progress", None), "fox"),
        assigned(todo("2", "Add tests", "pending", None), "fox"),
        assigned(todo("3", "Review", "pending", None), "sheep"),
    ];
    for todo in &mut todos {
        todo.group = Some("解析器".into());
    }
    let agents = vec![
        agent("fox", "failed", Some("修复 parser"), None),
        agent("sheep", "queued", None, None),
    ];
    let refs: Vec<_> = todos.iter().collect();
    for width in [1, 8, 14, 40, 120] {
        let row = render_group_header("解析器", &refs, &agents, "", width);
        assert!(row.width() <= width);
        let text: String = row.spans.iter().map(|span| span.content.as_ref()).collect();
        assert!(!text.contains("修复 parser"), "{text}");
        if width == 120 {
            assert_eq!(text.matches('🦊').count(), 1, "{text}");
            assert_eq!(text.matches('🐑').count(), 1, "{text}");
            assert!(
                text.contains("failed") && text.contains("swarm retry"),
                "{text}"
            );
        }
    }
}
