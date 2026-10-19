use super::*;
use crate::todo::TodoItem;

fn todo(id: &str, content: &str, status: &str, wave: Option<u32>) -> TodoItem {
    TodoItem {
        id: id.to_string(),
        content: content.to_string(),
        status: status.to_string(),
        priority: "high".to_string(),
        wave,
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

#[test]
fn sequential_plan_renders_without_rails() {
    let todos = vec![
        todo("1", "read types", "completed", Some(1)),
        todo("2", "todo schema", "in_progress", Some(2)),
        todo("3", "e2e", "pending", None),
    ];
    let rows = render(&todos, &[], 60);
    let joined = rows.join("\n");
    assert!(!joined.contains('┏') && !joined.contains('┃') && !joined.contains('┗'));
    assert!(rows[0].contains("●●○"), "plain pips expected: {rows:?}");
}

#[test]
fn parallel_wave_gets_rail_and_gapped_pips() {
    let todos = vec![
        todo("1", "read types", "completed", Some(1)),
        todo("2", "wave field", "completed", Some(2)),
        todo("3", "pinned band", "in_progress", Some(2)),
        todo("4", "status chips", "pending", Some(2)),
        todo("5", "integrate", "pending", Some(3)),
    ];
    let rows = render(&todos, &[], 60);
    eprintln!("{}", rows.join("\n"));
    assert!(rows[0].contains("● ●●○ ○"), "gapped pips: {rows:?}");
    assert!(
        rows[1].trim_start().starts_with('✓'),
        "no rail on solo: {rows:?}"
    );
    assert!(
        rows[2].trim_start().starts_with("┏ ✓ wave field"),
        "{rows:?}"
    );
    assert!(
        rows[3].trim_start().starts_with("┃ ● pinned band"),
        "{rows:?}"
    );
    assert!(
        rows[4].trim_start().starts_with("┗ ○ status chips"),
        "{rows:?}"
    );
    // Solo rows stay aligned with railed rows (compare display columns).
    let col = |row: &str, glyph: char| row.split(glyph).next().unwrap().width();
    assert_eq!(col(&rows[1], '✓'), col(&rows[2], '✓'));
}

#[test]
fn agent_emoji_chip_and_activity_render() {
    let todos = vec![
        todo("1", "read types", "completed", Some(1)),
        assigned(todo("2", "wave field", "completed", Some(2)), "sheep"),
        assigned(
            todo("3", "pinned band rendering", "in_progress", Some(2)),
            "fox",
        ),
        assigned(
            todo("4", "swarm sync-back", "in_progress", Some(2)),
            "session_crab_1700000000",
        ),
        todo("5", "integrate", "pending", Some(3)),
    ];
    let agents = vec![
        agent("sheep", "completed", None, Some(120)),
        agent("fox", "running", Some("justrust test"), Some(64)),
        agent("crab", "failed", Some("build failed: E0308"), Some(5)),
    ];
    for width in [95u16, 60, 36] {
        let rows = render(&todos, &agents, width);
        eprintln!("--- {width}\n{}", rows.join("\n"));
        assert_fits(&rows, width);
        let joined = rows.join("\n");
        assert!(rows[0].contains("🐑") && rows[0].contains("🦊") && rows[0].contains("🦀"));
        assert!(joined.contains("🦊"), "fox chip at {width}");
        assert!(rows[4].contains("🦀✗"), "failed mark at {width}: {rows:?}");
    }
    let wide = render(&todos, &agents, 95);
    assert!(wide[3].contains("🦊 justrust test · 1m"), "{wide:?}");
    // Completed agent rows don't show stale activity.
    assert!(wide[2].trim_end().ends_with("🐑"), "{wide:?}");
}

#[test]
fn wave_field_parses_loose_values() {
    let parse = |value: serde_json::Value| -> Option<u32> {
        serde_json::from_value::<TodoItem>(serde_json::json!({
            "content": "x", "status": "pending", "priority": "high", "id": "1", "wave": value
        }))
        .unwrap()
        .wave
    };
    assert_eq!(parse(serde_json::json!(2)), Some(2));
    assert_eq!(parse(serde_json::json!(2.0)), Some(2));
    assert_eq!(parse(serde_json::json!("3")), Some(3));
    assert_eq!(parse(serde_json::json!("wave 4")), Some(4));
    assert_eq!(parse(serde_json::json!(0)), None);
    assert_eq!(parse(serde_json::json!("")), None);
    assert_eq!(parse(serde_json::json!(null)), None);
    assert_eq!(parse(serde_json::json!(-1)), None);
}
