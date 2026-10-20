use super::*;
use crate::todo::TodoItem;

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
fn stages_get_numbered_pips_and_gutter_numbers() {
    let todos = vec![
        todo("1", "read types", "completed", Some(1)),
        todo("2", "stage field", "completed", Some(2)),
        todo("3", "pinned band", "in_progress", Some(2)),
        todo("4", "integrate", "pending", Some(3)),
    ];
    let rows = render(&todos, &[], 60);
    eprintln!("{}", rows.join("\n"));
    assert!(
        rows[0].contains("1 ●  2 ●●  3 ○"),
        "numbered pips: {rows:?}"
    );
    let joined = rows.join("\n");
    for glyph in ['┏', '┃', '┗', '│', '─'] {
        assert!(!joined.contains(glyph), "no lines ({glyph}): {joined}");
    }
    assert!(
        rows.iter()
            .any(|r| r.trim_start().starts_with("1 ✓ read types")),
        "{rows:?}"
    );
    let parallel = rows.iter().find(|r| r.contains("stage field")).unwrap();
    assert!(
        parallel.trim_start().starts_with("2 ✓ stage field") && parallel.contains("● pinned band"),
        "ungrouped parallel items sit side by side: {rows:?}"
    );
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
fn wide_stage_lays_lanes_out_as_columns_under_group_names() {
    let rows = render(&grid_todos(), &[], 100);
    eprintln!("{}", rows.join("\n"));
    assert_fits(&rows, 100);
    assert!(rows[0].contains("1 ●●○●●●  2 ○○○"), "{rows:?}");
    let header = rows.iter().find(|r| r.contains("parser")).unwrap();
    assert!(
        header.contains("ui") && header.contains("docs"),
        "one header row: {rows:?}"
    );
    let first = rows.iter().find(|r| r.contains("Extract lexer")).unwrap();
    assert!(
        first.contains("Rename rails") && first.contains("Update tool desc"),
        "lanes side by side: {rows:?}"
    );
    let col = |row: &str, text: &str| row[..row.find(text).unwrap()].width();
    let second = rows.iter().find(|r| r.contains("Update callers")).unwrap();
    assert_eq!(col(first, "Rename rails"), col(second, "Draw lane headers"));
    assert_eq!(col(header, "ui"), col(first, "✓ Rename") + 0);
    // Stage two has its own lanes, so it gets fresh headers.
    let header2 = rows.iter().find(|r| r.contains("integration")).unwrap();
    assert!(
        header2.contains("release") && header2.trim_start().starts_with("2 "),
        "{rows:?}"
    );
    // Grid is shorter than a stacked list.
    assert!(rows.len() < 1 + 9 + 5, "{} rows", rows.len());
}

#[test]
fn narrow_stage_stacks_lanes_under_headings() {
    for width in [50u16, 30, 18] {
        let rows = render(&grid_todos(), &[], width);
        eprintln!("--- {width}\n{}", rows.join("\n"));
        assert_fits(&rows, width);
    }
    let rows = render(&grid_todos(), &[], 50);
    let parser = rows
        .iter()
        .position(|r| r.trim_start().starts_with("1 parser"))
        .unwrap();
    assert!(rows[parser + 1].contains("Extract lexer"), "{rows:?}");
    assert!(rows.iter().any(|r| r.trim() == "ui"), "{rows:?}");
    // Numbered pips collapse to per-stage counts when they do not fit.
    let rows = render(&grid_todos(), &[], 14);
    assert!(
        rows[0].contains("1 4/6") || rows[0].contains('●'),
        "{rows:?}"
    );
}

#[test]
fn long_lane_folds_into_more_line_keeping_active_item() {
    let mut todos: Vec<TodoItem> = (0..10)
        .map(|i| {
            laned(
                &format!("step {i}"),
                if i < 7 { "completed" } else { "pending" },
                1,
                "long",
            )
        })
        .collect();
    todos[7].status = "in_progress".into();
    todos.push(laned("other", "pending", 1, "short"));
    let rows = render(&todos, &[], 100);
    eprintln!("{}", rows.join("\n"));
    let joined = rows.join("\n");
    assert!(
        joined.contains("step 7") && joined.contains("+5 more"),
        "{joined}"
    );
}

#[test]
fn lane_header_shows_agent_emoji_and_activity() {
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
            rows[0].contains("🦊") && rows[0].contains("🐑"),
            "roster: {rows:?}"
        );
    }
    let rows = render(&todos, &agents, 120);
    let header = rows.iter().find(|r| r.contains("parser")).unwrap();
    assert!(header.contains("parser 🦊 justrust test"), "{rows:?}");
    assert!(header.contains("docs 🦀✗"), "{rows:?}");
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
