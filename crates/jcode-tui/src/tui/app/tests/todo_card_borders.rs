fn render_pinned_band_fixture(
    todos: &[crate::todo::TodoItem],
    width: u16,
    height: u16,
) -> (App, String) {
    let mut app = create_test_app();
    let session_id = app.session.id.clone();
    crate::todo::save_todos(&session_id, todos).unwrap();
    app.refresh_pinned_todos_now();
    assert!(app.pinned_todos_payload_ref().is_some());
    app.pinned_todos_expanded = false;
    app.display_messages = vec![
        DisplayMessage {
            role: "user".to_string(),
            content: "TRANSCRIPT_START marker".to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        },
        DisplayMessage {
            role: "assistant".to_string(),
            content: App::build_scroll_test_content(0, 40, None),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        },
    ];
    app.bump_display_messages_version();
    app.scroll_offset = 0;
    app.auto_scroll_paused = true;
    app.is_processing = false;
    app.status = ProcessingStatus::Idle;
    app.session.short_name = Some("test".to_string());
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    let text = render_and_snap(&app, &mut terminal);
    let _ = crate::todo::save_todos(&session_id, &[]);
    (app, text)
}

#[test]
fn pinned_todo_band_has_rounded_border_with_title() {
    let _env_lock = crate::storage::lock_test_env();
    let _render_lock = crate::tui::ui::render_state_test_lock();
    let _pin = PinTodosEnvGuard::enable();
    let (_app, text) = render_pinned_band_fixture(
        &[
            pinned_band_todo("t1", "first boxed item", "completed"),
            pinned_band_todo("t2", "second boxed item", "in_progress"),
            pinned_band_todo("t3", "third boxed item", "pending"),
        ],
        70,
        30,
    );
    let rows: Vec<&str> = text.lines().collect();
    let top = rows
        .iter()
        .position(|r| r.contains('╭'))
        .unwrap_or_else(|| panic!("missing top border:\n{text}"));
    assert!(rows[top].contains('╮'), "{text}");
    assert!(rows[top].contains(" Todos 1/3 "), "missing title:\n{text}");
    let bottom = rows
        .iter()
        .position(|r| r.contains('╰'))
        .unwrap_or_else(|| panic!("missing bottom border:\n{text}"));
    assert!(rows[bottom].contains('╯'), "{text}");
    for row in &rows[top + 1..bottom] {
        let trimmed = row.trim_end();
        assert!(
            trimmed.trim_start().starts_with('│'),
            "left border: {row:?}\n{text}"
        );
        assert!(trimmed.ends_with('│'), "right border: {row:?}\n{text}");
    }
    let item = rows
        .iter()
        .position(|r| r.contains("second boxed item"))
        .expect("todo row");
    assert!(top < item && item < bottom, "{text}");
    let transcript = rows
        .iter()
        .position(|r| r.contains("TRANSCRIPT_START"))
        .unwrap_or_else(|| panic!("transcript hidden under band:\n{text}"));
    assert!(
        transcript > bottom,
        "transcript must start below the box:\n{text}"
    );
}

#[test]
fn pinned_todo_more_row_click_rect_matches_rendered_row() {
    let _env_lock = crate::storage::lock_test_env();
    let _render_lock = crate::tui::ui::render_state_test_lock();
    let _pin = PinTodosEnvGuard::enable();
    let todos: Vec<_> = (0..12)
        .map(|i| pinned_band_todo(&format!("t{i}"), &format!("many item {i}"), "pending"))
        .collect();
    let (_app, text) = render_pinned_band_fixture(&todos, 70, 24);
    let rows: Vec<&str> = text.lines().collect();
    let more_row = rows
        .iter()
        .position(|r| r.contains("more (todo)"))
        .unwrap_or_else(|| panic!("missing more row:\n{text}"));
    let bottom = rows
        .iter()
        .position(|r| r.contains('╰'))
        .expect("bottom border");
    assert!(
        more_row < bottom,
        "more row must be inside the box:\n{text}"
    );
    let area = crate::tui::ui::viewport::pinned_todo_more_area().expect("more area");
    assert_eq!(area.y as usize, more_row, "{area:?}\n{text}");
    let row_chars: Vec<char> = rows[more_row].chars().collect();
    let border_x = row_chars
        .iter()
        .position(|c| *c == '│')
        .expect("left border");
    assert!(
        area.x as usize > border_x,
        "click rect must be inside the box: {area:?}\n{text}"
    );
    let ellipsis_x = row_chars.iter().position(|c| *c == '…').expect("ellipsis");
    assert!(
        (area.x as usize) <= ellipsis_x && ellipsis_x < (area.x + area.width) as usize,
        "{area:?}\n{text}"
    );
    crate::tui::ui::viewport::set_pinned_todo_more_area_for_test(None);
}

#[test]
fn todo_frame_leaves_transcript_rows_below_a_sticky_prompt() {
    let _env_lock = crate::storage::lock_test_env();
    let _render_lock = crate::tui::ui::render_state_test_lock();
    let _pin = PinTodosEnvGuard::enable();
    assert!(crate::config::config().display.prompt_preview);
    let mut app = create_test_app();
    app.pinned_todos_payload = Some(
        serde_json::json!({"todos": (0..12)
        .map(|i| pinned_band_todo(&format!("t{i}"), &format!("Pending task {i}"), "pending"))
        .collect::<Vec<_>>()})
        .to_string(),
    );
    app.display_messages = vec![
        DisplayMessage::user("a long sticky prompt ".repeat(20)),
        DisplayMessage::assistant(format!(
            "```text\n{}```",
            "VISIBLE_TRANSCRIPT\n".repeat(100)
        )),
    ];
    app.bump_display_messages_version();
    app.auto_scroll_paused = true;
    app.scroll_offset = usize::MAX;
    let mut checked_six_rows = false;
    for height in 8..20 {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, height)).unwrap();
        let text = render_and_snap(&app, &mut terminal);
        let layout = crate::tui::ui::last_layout_snapshot().unwrap();
        let chat_height = layout.messages_area.height;
        if (6..=9).contains(&chat_height) {
            checked_six_rows |= chat_height == 6;
            assert!(
                text.contains("VISIBLE_TRANSCRIPT"),
                "chat_height={chat_height}\n{text}"
            );
            assert!(text.contains("sticky prompt"), "{text}");
        }
    }
    assert!(
        checked_six_rows,
        "fixture must exercise the reviewed six-row viewport"
    );
}
