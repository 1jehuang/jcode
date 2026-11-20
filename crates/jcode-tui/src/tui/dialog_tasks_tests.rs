use super::*;
use crate::protocol::SwarmMemberStatus;
use crate::tui::{BackgroundTaskRow, BackgroundTaskRowStatus};
use ratatui::backend::TestBackend;
use unicode_width::UnicodeWidthStr;

fn shell(id: &str, label: &str, status: BackgroundTaskRowStatus) -> ShellTask {
    ShellTask {
        task_id: id.into(),
        label: label.into(),
        status,
        runtime_secs: Some(75),
        exit_code: (status == BackgroundTaskRowStatus::Completed).then_some(0),
        error: None,
        output: vec!["line one".into(), "line two".into()],
    }
}

fn agent(id: &str, name: &str, status: &str) -> SwarmMemberStatus {
    let mut member = SwarmMemberStatus {
        session_id: id.into(),
        friendly_name: Some(name.into()),
        status: status.into(),
        detail: None,
        task_label: None,
        role: None,
        is_headless: None,
        live_attachments: None,
        status_age_secs: None,
        output_tail: Some("thinking\n\nwriting tests".into()),
        report_back_to_session_id: None,
        todo_progress: None,
        todo_items: Vec::new(),
        runtime: Default::default(),
    };
    member.runtime.elapsed_secs = Some(9);
    member
}

fn snapshot() -> TasksSnapshot {
    use BackgroundTaskRowStatus::*;
    TasksSnapshot {
        shells: vec![
            shell("old", "cargo build", Completed),
            shell("bg1", "sleep 40", Running),
        ],
        agents: vec![
            agent("s1", "fox", "running"),
            agent("s2", "owl", "completed"),
        ],
        taken_at: None,
    }
}

fn press(dialog: &mut Dialog, code: KeyCode) -> DialogKey {
    dialog.handle_key(code, KeyModifiers::empty())
}

fn route(dialog: &Dialog) -> Route {
    dialog.stack.last().unwrap().route.clone()
}

fn screen(dialog: &Dialog, width: u16, height: u16) -> Vec<String> {
    let mut terminal = ratatui::Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| dialog.render(frame, frame.area()))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    (0..height)
        .map(|y| {
            // A wide glyph's trailing cell is padding, not a second column.
            let mut row = String::new();
            let mut x = 0;
            while x < width {
                let symbol = buffer[(x, y)].symbol();
                row.push_str(symbol);
                x += symbol.width().max(1) as u16;
            }
            row.trim_end().to_string()
        })
        .collect()
}

#[test]
fn pill_text_counts_shells_agents_and_mixed() {
    assert_eq!(tasks::pill_text(0, 0), None);
    assert_eq!(tasks::pill_text(1, 0).as_deref(), Some("1 shell"));
    assert_eq!(tasks::pill_text(3, 0).as_deref(), Some("3 shells"));
    assert_eq!(
        tasks::pill_text(0, 1).as_deref(),
        Some("1 background agent")
    );
    assert_eq!(
        tasks::pill_text(0, 2).as_deref(),
        Some("2 background agents")
    );
    assert_eq!(
        tasks::pill_text(1, 1).as_deref(),
        Some("2 background tasks")
    );
}

#[test]
fn task_pill_counts_only_running_shells_and_live_agents() {
    let row = |status| BackgroundTaskRow {
        task_id: "t".into(),
        label: "x".into(),
        percent: None,
        status,
        completed_at: None,
    };
    let rows = [
        row(BackgroundTaskRowStatus::Running),
        row(BackgroundTaskRowStatus::Completed),
        row(BackgroundTaskRowStatus::Failed),
    ];
    assert_eq!(task_pill(&rows, &[]).as_deref(), Some("1 shell"));
    let agents = [agent("a", "a", "running"), agent("b", "b", "ready")];
    assert_eq!(
        task_pill(&rows[1..], &agents).as_deref(),
        Some("1 background agent")
    );
    assert_eq!(
        task_pill(&rows, &agents).as_deref(),
        Some("2 background tasks")
    );
    assert_eq!(task_pill(&rows[1..], &agents[1..]), None);
}

#[test]
fn list_groups_running_shells_agents_then_completed() {
    let dialog = Dialog::tasks(snapshot());
    let view = dialog.stack.last().unwrap();
    assert_eq!(view.title, "Background tasks");
    assert_eq!(view.subtitle, "2 background tasks");
    let groups: Vec<_> = view
        .items
        .iter()
        .map(|item| (item.group.clone().unwrap(), item.label.clone()))
        .collect();
    assert_eq!(
        groups,
        [
            ("Shells".to_string(), "sleep 40".to_string()),
            ("Agents".to_string(), "fox".to_string()),
            ("Completed".to_string(), "cargo build".to_string()),
            ("Completed".to_string(), "owl".to_string()),
        ]
    );
    let empty = Dialog::tasks(TasksSnapshot::default());
    assert!(empty.stack.last().unwrap().items.is_empty());
}

#[test]
fn keys_move_open_stop_and_go_back() {
    let mut dialog = Dialog::tasks(snapshot());
    // Selected: the running shell. x stops it.
    assert_eq!(
        press(&mut dialog, KeyCode::Char('x')),
        DialogKey::StopTask(TaskTarget::Shell("bg1".into()))
    );
    // j/k and arrows move; the agent is next.
    assert_eq!(press(&mut dialog, KeyCode::Char('j')), DialogKey::Handled);
    assert_eq!(
        press(&mut dialog, KeyCode::Char('x')),
        DialogKey::StopTask(TaskTarget::Agent("s1".into()))
    );
    // A finished task cannot be stopped.
    press(&mut dialog, KeyCode::Down);
    assert_eq!(press(&mut dialog, KeyCode::Char('x')), DialogKey::Handled);
    press(&mut dialog, KeyCode::Up);
    press(&mut dialog, KeyCode::Char('k'));
    // Enter opens the shell's detail page, Esc goes back, Esc again closes.
    assert_eq!(press(&mut dialog, KeyCode::Enter), DialogKey::Handled);
    assert_eq!(route(&dialog), Route::TaskShell("bg1".into()));
    assert_eq!(
        press(&mut dialog, KeyCode::Char('x')),
        DialogKey::StopTask(TaskTarget::Shell("bg1".into()))
    );
    assert_eq!(press(&mut dialog, KeyCode::Esc), DialogKey::Handled);
    assert_eq!(route(&dialog), Route::Tasks);
    assert_eq!(press(&mut dialog, KeyCode::Esc), DialogKey::Close);
}

#[test]
fn agent_detail_enter_opens_the_agent() {
    let mut dialog = Dialog::tasks(snapshot());
    press(&mut dialog, KeyCode::Down);
    press(&mut dialog, KeyCode::Enter);
    assert_eq!(route(&dialog), Route::TaskAgent("s1".into()));
    assert_eq!(
        press(&mut dialog, KeyCode::Enter),
        DialogKey::EnterAgent("s1".into())
    );
    let rows = screen(&dialog, 60, 20);
    assert!(
        rows.iter()
            .any(|row| row.contains("Status:") && row.contains("running"))
    );
    assert!(rows.iter().any(|row| row.contains("│ writing tests")));
}

#[test]
fn refresh_keeps_detail_and_closes_it_when_the_task_vanishes() {
    let mut dialog = Dialog::tasks(snapshot());
    press(&mut dialog, KeyCode::Enter);
    let mut next = snapshot();
    next.shells[1].output.push("line three".into());
    dialog.set_tasks(next);
    assert_eq!(route(&dialog), Route::TaskShell("bg1".into()));
    let rows = screen(&dialog, 50, 20);
    assert!(
        rows.iter().any(|row| row.contains("│ line three")),
        "{rows:#?}"
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("Runtime:") && row.contains("1m 15s"))
    );
    dialog.set_tasks(TasksSnapshot::default());
    assert_eq!(route(&dialog), Route::Tasks);
}

#[test]
fn rows_and_output_never_exceed_the_width() {
    let mut data = snapshot();
    data.shells[1].label = "npm run build -- --watch 界界界 ".repeat(8);
    data.shells[1].output = vec!["界".repeat(200)];
    let mut dialog = Dialog::tasks(data);
    for width in [12u16, 24, 40, 80] {
        for row in screen(&dialog, width, 20) {
            assert!(row.width() <= usize::from(width), "{width}: {row}");
        }
    }
    let rows = screen(&dialog, 40, 20);
    let shell_row = rows.iter().find(|row| row.contains('❯')).unwrap();
    assert!(
        shell_row.contains('…') && shell_row.ends_with("1m 15s"),
        "{shell_row}"
    );
    press(&mut dialog, KeyCode::Enter);
    for width in [12u16, 24, 40] {
        for row in screen(&dialog, width, 20) {
            assert!(row.width() <= usize::from(width), "{width}: {row}");
        }
    }
}

#[test]
fn output_tail_keeps_last_non_blank_lines_without_escapes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("task.output");
    let mut text = String::new();
    for n in 0..30 {
        text.push_str(&format!("line {n}\n\n"));
    }
    text.push_str("\x1b[31mred\x1b[0m\nprogress 10%\rprogress 90%\n");
    std::fs::write(&path, text).unwrap();
    let tail = read_output_tail(&path, TAIL_LINES);
    assert_eq!(tail.len(), TAIL_LINES);
    assert_eq!(tail[0], "line 22");
    assert_eq!(&tail[8..], ["red", "progress 90%"]);
    assert!(read_output_tail(&dir.path().join("missing"), TAIL_LINES).is_empty());
}

#[test]
fn modified_x_never_stops_a_task() {
    let mut dialog = Dialog::tasks(snapshot());
    for modifiers in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
        assert!(!matches!(
            dialog.handle_key(KeyCode::Char('x'), modifiers),
            DialogKey::StopTask(_)
        ));
    }
    assert_eq!(route(&dialog), Route::Tasks);
}
