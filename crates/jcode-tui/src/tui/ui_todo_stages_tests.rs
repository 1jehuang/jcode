use super::*;
use crate::todo::TodoItem;

#[test]
fn shortcut_badges_follow_payload_digits_and_respect_lane_width() {
    let mut item = assigned(todo("1", "Read types", "in_progress", Some(1)), "fox");
    item.group = Some("Parser".into());
    let mut worker = agent("fox", "running", Some("Reading types"), Some(12));
    for digit in [1, 9, 0] {
        worker.shortcut = Some(digit);
        let rows = render(&[item.clone()], &[worker.clone()], 120);
        assert!(
            rows.join("\n").contains(&format!("🦊 [alt+{digit}]")),
            "{rows:?}"
        );
        for width in 1..60 {
            for row in render(&[item.clone()], &[worker.clone()], width) {
                assert!(row.width() <= usize::from(width), "width {width}: {row:?}");
            }
        }
    }
    worker.shortcut = Some(10);
    assert!(worker.shortcut_span().is_none());
}

#[test]
fn shortcut_payload_is_optional_and_round_trips() {
    let mut worker: TodoCardAgent = serde_json::from_value(serde_json::json!({
        "session_id": "session_fox_1700000000", "status": "running"
    }))
    .unwrap();
    assert_eq!(worker.shortcut, None);
    assert!(
        serde_json::to_value(&worker)
            .unwrap()
            .get("shortcut")
            .is_none()
    );
    worker.shortcut = Some(0);
    let restored: TodoCardAgent =
        serde_json::from_value(serde_json::to_value(&worker).unwrap()).unwrap();
    assert_eq!(restored.shortcut, Some(0));
}

#[test]
fn inactive_lanes_keep_ownership_without_live_activity() {
    for status in ["completed", "pending"] {
        let mut item = assigned(todo("1", "Read types", status, None), "fox");
        item.group = Some("Parser".into());
        let agents = vec![agent("fox", "running", Some("Working elsewhere"), Some(12))];
        let rows = render(&[item], &agents, 120);
        assert_eq!(rows.join("\n").matches('🦊').count(), 1, "{rows:?}");
        assert!(!rows.join("\n").contains("Working elsewhere"), "{rows:?}");
        assert!(!rows.join("\n").contains("12s"), "{rows:?}");
    }
    let mut done = assigned(todo("1", "Finished", "completed", None), "fox");
    done.group = Some("Parser".into());
    let agents = vec![agent("fox", "failed", Some("Failure elsewhere"), Some(12))];
    let rows = render(&[done], &agents, 120);
    assert!(!rows.join("\n").contains("Failure elsewhere"), "{rows:?}");
}

fn todo(id: &str, content: &str, status: &str, stage: Option<u32>) -> TodoItem {
    TodoItem {
        id: id.to_string(),
        content: content.to_string(),
        status: status.to_string(),
        priority: "high".to_string(),
        stage,
        ..Default::default()
    }
}

fn assigned(mut item: TodoItem, who: &str) -> TodoItem {
    item.assigned_to = Some(who.to_string());
    item
}

fn agent(name: &str, status: &str, activity: Option<&str>, age: Option<u64>) -> TodoCardAgent {
    TodoCardAgent {
        session_id: format!("session_{name}_1700000000"),
        name: Some(name.to_string()),
        status: status.to_string(),
        activity: activity.map(str::to_string),
        age_secs: age,
        activity_age_secs: age,
        todo_statuses: Vec::new(),
        shortcut: None,
    }
}

#[test]
fn lane_dots_use_worker_items_once_and_fall_back_without_items() {
    let mut first = assigned(todo("1", "Parser task", "in_progress", Some(1)), "fox");
    first.group = Some("Parser".into());
    let mut second = assigned(todo("2", "Another task", "pending", Some(1)), "fox");
    second.group = Some("Parser".into());
    let mut worker = agent("fox", "running", None, None);
    worker.todo_statuses = vec![
        "completed".into(),
        "in_progress".into(),
        "pending".into(),
        "pending".into(),
    ];
    let items = [&first, &second];
    let agents = [worker];
    let statuses = lane_item_statuses(&items, &agents);
    assert_eq!(
        statuses,
        vec![
            ("completed", true),
            ("in_progress", true),
            ("pending", true),
            ("pending", true)
        ]
    );
    let strip = lane_strip_spans(&statuses, 4);
    assert_eq!(strip[0].content, "●");
    assert_eq!(strip[1].content, LIVE_DOT_SENTINEL);
    assert_eq!(strip[2].content, "○");
    assert_eq!(strip[3].content, "○");
    let rows = render(&[first.clone(), second.clone()], &agents, 120);
    let row = rows
        .iter()
        .find(|row| row.contains("Parser") && row.contains('🦊'))
        .unwrap();
    assert!(row.contains("○○"), "{rows:?}");
    assert_eq!(
        lane_item_statuses(&items, &[]),
        vec![("in_progress", false), ("pending", false)]
    );
}

#[test]
fn worker_dots_prefer_complete_status_snapshot() {
    let mut member: crate::protocol::SwarmMemberStatus =
        serde_json::from_value(serde_json::json!({
            "session_id": "worker", "status": "running",
            "todo_items": [{"content": "visible detail", "status": "in_progress"}],
            "runtime": {"todo_statuses": ["completed", "completed", "in_progress", "pending"]}
        }))
        .unwrap();
    let snapshot = TodoCardAgent::from_member(&member);
    assert_eq!(
        snapshot.todo_statuses,
        ["completed", "completed", "in_progress", "pending"]
    );
    member.runtime.todo_statuses.clear();
    assert_eq!(
        TodoCardAgent::from_member(&member).todo_statuses,
        ["in_progress"]
    );
    let old: TodoCardAgent = serde_json::from_value(serde_json::json!({
        "session_id": "old-worker", "status": "ready"
    }))
    .unwrap();
    assert!(old.todo_statuses.is_empty());
}

#[test]
fn worker_dot_strip_fits_narrow_width() {
    let statuses = vec![("pending", false); 100];
    for width in 1..40 {
        let strip = lane_strip_spans(&statuses, width);
        assert!(width_of(&strip) <= width + 2);
    }
}

fn render(todos: &[TodoItem], agents: &[TodoCardAgent], width: u16) -> Vec<String> {
    crate::tui::markdown::set_center_code_blocks(false);
    let mut payload = serde_json::json!({ "todos": todos, "plan": {}, "goals": [] });
    if !agents.is_empty() {
        payload["agents"] = serde_json::to_value(agents).unwrap();
    }
    let msg = DisplayMessage::todos(payload.to_string());
    render_pinned_todos_message(&msg, width, crate::config::DiffDisplayMode::Off)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

fn assert_fits(rows: &[String], width: u16) {
    for row in rows {
        assert!(
            row.width() <= width as usize,
            "row exceeds {width} cells ({}): {row:?}",
            row.width()
        );
    }
}

fn laned(id: &str, status: &str, stage: u32, group: &str) -> TodoItem {
    let mut item = todo(id, id, status, Some(stage));
    item.group = Some(group.to_string());
    item
}

#[test]
fn unstaged_list_renders_as_plain_checklist() {
    let todos = vec![
        todo("1", "read types", "completed", None),
        todo("2", "todo schema", "in_progress", None),
        todo("3", "e2e", "pending", None),
    ];
    let rows = render(&todos, &[], 60);
    assert!(rows[0].contains("●●○"), "plain pips expected: {rows:?}");
    assert!(!rows[0].contains("1 "), "no stage numbers: {rows:?}");
    assert!(rows[1].trim_start().starts_with("✓ read types"), "{rows:?}");
}

#[test]
fn header_labels_each_phase_with_its_pips() {
    let todos = vec![
        todo("1", "read types", "completed", Some(1)),
        todo("2", "stage field", "completed", Some(2)),
        todo("3", "pinned band", "in_progress", Some(2)),
        todo("4", "integrate", "pending", Some(3)),
    ];
    let rows = render(&todos, &[], 60);
    eprintln!("{}", rows.join("\n"));
    assert_eq!(
        rows[0].trim(),
        "phase 1 ●  phase 2 ●●  phase 3 ○",
        "{rows:?}"
    );
    let trimmed: Vec<&str> = rows.iter().map(|r| r.trim()).collect();
    // Every phase is listed; ungrouped todos are lanes of their own, each
    // led by its status.
    assert_eq!(
        &trimmed[1..],
        &[
            "●  read types",
            "●  stage field",
            "●  pinned band",
            "○  integrate"
        ],
        "{rows:?}"
    );
    let joined = rows.join("\n");
    for glyph in ['┏', '┃', '┗', '│', '─'] {
        assert!(!joined.contains(glyph), "no lines ({glyph}): {joined}");
    }
}

fn grid_todos() -> Vec<TodoItem> {
    vec![
        laned("Extract lexer", "completed", 1, "parser"),
        laned("Update callers", "in_progress", 1, "parser"),
        laned("Add lexer tests", "pending", 1, "parser"),
        laned("Rename rails", "completed", 1, "ui"),
        laned("Draw lane headers", "completed", 1, "ui"),
        laned("Update tool desc", "completed", 1, "docs"),
        laned("Wire lanes into plan", "pending", 2, "integration"),
        laned("End-to-end swarm test", "pending", 2, "integration"),
        laned("Build and commit", "pending", 2, "release"),
    ]
}

#[test]
fn each_lane_row_leads_with_dots_then_name_then_titles() {
    let rows = render(&grid_todos(), &[], 100);
    eprintln!("{}", rows.join("\n"));
    assert_fits(&rows, 100);
    let trimmed: Vec<&str> = rows.iter().map(|r| r.trim()).collect();
    assert_eq!(trimmed[0], "phase 1 ●●○●●●  phase 2 ○○○");
    assert_eq!(
        &trimmed[1..],
        &[
            "●●○  parser       Extract lexer, Update callers, Add lexer tests",
            "●●   ui           Rename rails, Draw lane headers",
            "●    docs         Update tool desc",
            "○○   integration  Wire lanes into plan, End-to-end swarm test",
            "○    release      Build and commit",
        ],
        "{rows:?}"
    );
}

#[test]
fn narrow_rows_fold_finished_titles_and_never_wrap() {
    for width in [60u16, 40, 24, 14] {
        let rows = render(&grid_todos(), &[], width);
        eprintln!("--- {width}\n{}", rows.join("\n"));
        assert_fits(&rows, width);
        assert_eq!(rows.len(), 6, "one row per lane at {width}");
    }
    let rows = render(&grid_todos(), &[], 50);
    let parser = rows.iter().find(|r| r.contains("parser")).unwrap();
    // Long rows cap each title rather than hiding later ones.
    assert!(
        parser.contains("parser       Extract…, Update…, Add lex…")
            || parser.matches(", ").count() == 2,
        "{rows:?}"
    );
    assert!(render(&grid_todos(), &[], 30)[0].contains("1 ●●○●●●  2 ○○○"));
    assert!(render(&grid_todos(), &[], 18)[0].contains("1 4/6  2 0/3"));
}

#[test]
fn lane_row_shows_agent_emoji_and_activity() {
    let mut todos = grid_todos();
    todos[1].assigned_to = Some("fox".into());
    todos[3].assigned_to = Some("sheep".into());
    todos[5].assigned_to = Some("session_crab_1700000000".into());
    let agents = vec![
        agent("sheep", "completed", None, Some(120)),
        agent("fox", "running", Some("justrust test"), Some(64)),
        agent("crab", "failed", Some("build failed"), Some(5)),
    ];
    for width in [120u16, 60, 30] {
        let rows = render(&todos, &agents, width);
        eprintln!("--- {width}\n{}", rows.join("\n"));
        assert_fits(&rows, width);
        assert!(
            !rows[0].contains("🦊") && !rows[0].contains("🐑"),
            "lane owners should not repeat in header: {rows:?}"
        );
    }
    let rows = render(&todos, &agents, 120);
    let parser = rows.iter().find(|r| r.contains("parser")).unwrap();
    assert!(
        parser.contains("parser 🦊") && parser.contains("justrust test · 1m"),
        "{rows:?}"
    );
    assert!(rows.iter().any(|r| r.contains("docs 🦀✗")), "{rows:?}");
}

#[test]
fn finished_phases_keep_their_lanes() {
    let mut todos = grid_todos();
    todos.push(laned("Wrap up", "pending", 3, "release"));
    for todo in todos.iter_mut().take(9) {
        todo.status = "completed".into();
    }
    let rows = render(&todos, &[], 100);
    eprintln!("{}", rows.join("\n"));
    assert_eq!(rows[0].trim(), "phase 1 ●●●●●●  phase 2 ●●●  phase 3 ○");
    assert_eq!(rows.len(), 1 + 6, "every phase keeps its lanes: {rows:?}");
    assert!(rows.iter().any(|r| r.contains("Extract lexer")), "{rows:?}");
    assert!(rows.last().unwrap().contains("Wrap up"), "{rows:?}");
}

#[test]
fn unstaged_agent_rows_keep_inline_chip() {
    let todos = vec![
        assigned(
            todo("1", "pinned band rendering", "in_progress", None),
            "fox",
        ),
        todo("2", "integrate", "pending", None),
    ];
    let agents = vec![agent("fox", "running", Some("justrust test"), Some(64))];
    let rows = render(&todos, &agents, 95);
    assert!(
        rows.iter().any(|r| r.contains("🦊 justrust test · 1m")),
        "{rows:?}"
    );
}

#[test]
fn stage_field_parses_loose_values_and_legacy_wave_key() {
    let parse = |key: &str, value: serde_json::Value| -> Option<u32> {
        serde_json::from_value::<TodoItem>(serde_json::json!({
            "content": "x", "status": "pending", "priority": "high", "id": "1", key: value
        }))
        .unwrap()
        .stage
    };
    assert_eq!(parse("stage", serde_json::json!(2)), Some(2));
    assert_eq!(parse("stage", serde_json::json!(2.0)), Some(2));
    assert_eq!(parse("stage", serde_json::json!("3")), Some(3));
    assert_eq!(parse("stage", serde_json::json!("stage 4")), Some(4));
    assert_eq!(parse("wave", serde_json::json!(5)), Some(5));
    assert_eq!(parse("stage", serde_json::json!(0)), None);
    assert_eq!(parse("stage", serde_json::json!("")), None);
    assert_eq!(parse("stage", serde_json::json!(null)), None);
    assert_eq!(parse("stage", serde_json::json!(-1)), None);
}

#[test]
fn unknown_agent_falls_back_to_name_emoji_and_never_overflows() {
    let long = "x".repeat(200);
    let todos = vec![
        assigned(
            todo("1", &long, "in_progress", None),
            "session_fox_1700000000_abc",
        ),
        assigned(todo("2", "short", "in_progress", None), "zzz-not-a-session"),
    ];
    for width in [95u16, 40, 20] {
        let rows = render(&todos, &[], width);
        assert_fits(&rows, width);
        assert!(
            rows[1].contains("🦊"),
            "fallback emoji at {width}: {rows:?}"
        );
    }
    let rows = render(&todos, &[], 95);
    assert!(
        rows[2].trim_end().ends_with('z') && !rows[2].contains('💫'),
        "{rows:?}"
    );
}

#[test]
fn lanes_of_later_phases_are_dimmed() {
    crate::tui::markdown::set_center_code_blocks(false);
    let payload = serde_json::json!({ "todos": grid_todos(), "plan": {}, "goals": [] });
    let msg = DisplayMessage::todos(payload.to_string());
    let lines = render_pinned_todos_message(&msg, 100, crate::config::DiffDisplayMode::Off);
    let style_of = |needle: &str| {
        let line = lines
            .iter()
            .find(|line| line.spans.iter().any(|span| span.content.contains(needle)))
            .unwrap();
        line.spans
            .iter()
            .find(|span| span.content.contains(needle))
            .unwrap()
            .style
    };
    let dim = Some(rgb(105, 100, 135));
    assert_ne!(style_of("parser").fg, dim, "active phase lane stays bright");
    assert_eq!(
        style_of("integration").fg,
        dim,
        "later phase lane is dimmed"
    );
    assert_eq!(style_of("Build and commit").fg, dim);
    // Lane names wear their phase label's colour and weight.
    let bold =
        |style: ratatui::style::Style| style.add_modifier.contains(ratatui::style::Modifier::BOLD);
    assert_eq!(style_of("parser").fg, style_of("phase 1").fg);
    assert!(bold(style_of("parser")) && bold(style_of("phase 1")));
    assert_eq!(style_of("integration").fg, style_of("phase 2").fg);
    assert!(!bold(style_of("integration")) && !bold(style_of("phase 2")));
}

fn live_lane_todos() -> Vec<TodoItem> {
    let mut todos = grid_todos();
    todos[1].assigned_to = Some("fox".into()); // parser: in progress
    todos[3].assigned_to = Some("sheep".into()); // ui: all done
    todos
}

#[test]
fn live_worker_dot_spins_and_quiet_worker_turns_amber() {
    let agents = vec![
        agent("fox", "running", Some("justrust test"), Some(12)),
        agent("sheep", "completed", None, Some(300)),
    ];
    let rows = render(&live_lane_todos(), &agents, 120);
    let parser = rows.iter().find(|r| r.contains("parser")).unwrap();
    assert!(
        parser.starts_with("  ●") && parser.contains(LIVE_DOT_SENTINEL),
        "{rows:?}"
    );
    assert!(parser.contains("justrust test · 12s"), "{rows:?}");
    assert!(
        !rows
            .iter()
            .find(|r| r.contains("ui 🐑"))
            .unwrap()
            .contains(LIVE_DOT_SENTINEL),
        "finished lane does not spin: {rows:?}"
    );

    // The placeholder is swapped for a spinner frame on draw.
    crate::tui::markdown::set_center_code_blocks(false);
    let payload = serde_json::json!({
        "todos": live_lane_todos(), "plan": {}, "goals": [], "agents": agents
    });
    let msg = DisplayMessage::todos(payload.to_string());
    let mut lines = render_pinned_todos_message(&msg, 120, crate::config::DiffDisplayMode::Off);
    animate_lane_spinners(&mut lines);
    let text: String = lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
        .collect();
    assert!(!text.contains(LIVE_DOT_SENTINEL), "sentinel replaced");

    // Quiet for over two minutes: amber age instead of a plain one.
    let quiet = vec![agent("fox", "running", Some("justrust test"), Some(200))];
    let rows = render(&live_lane_todos(), &quiet, 120);
    assert!(
        rows.iter().any(|r| r.contains("justrust test · quiet 3m")),
        "{rows:?}"
    );
}

#[test]
fn roster_groups_working_agents_before_finished_ones() {
    let agents = vec![
        agent("fox", "running", Some("x"), Some(3)),
        agent("sheep", "completed", None, Some(300)),
    ];
    let todos = vec![
        assigned(todo("1", "queued", "pending", None), "fox"),
        assigned(todo("2", "finished", "completed", None), "sheep"),
    ];
    let rows = render(&todos, &agents, 120);
    let header = &rows[0];
    let fox = header.find('🦊').unwrap();
    let done = header.find("· ✓").unwrap();
    let sheep = header.find('🐑').unwrap();
    assert!(fox < done && done < sheep, "{header}");
    // Nobody working: just the done group.
    let all_done = vec![
        agent("fox", "completed", None, None),
        agent("sheep", "completed", None, None),
    ];
    let rows = render(&todos, &all_done, 120);
    assert!(
        rows[0].contains("✓ 🦊🐑") && !rows[0].contains('·'),
        "{:?}",
        rows[0]
    );
}

#[test]
fn failed_lane_blocks_its_phase_and_offers_retry() {
    let mut todos = grid_todos();
    todos[5].status = "in_progress".into(); // docs lane still open
    todos[5].assigned_to = Some("session_crab_1700000000".into());
    let agents = vec![agent("crab", "failed", Some("build failed"), Some(5))];
    let rows = render(&todos, &agents, 120);
    assert!(rows[0].contains("⏸ phase 1 blocked"), "{rows:?}");
    let docs = rows.iter().find(|r| r.contains("docs")).unwrap();
    assert!(docs.contains("✗ build failed · swarm retry"), "{rows:?}");
    // No failure: no blocked marker.
    let rows = render(&grid_todos(), &[], 120);
    assert!(!rows[0].contains("blocked"), "{rows:?}");
}

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
