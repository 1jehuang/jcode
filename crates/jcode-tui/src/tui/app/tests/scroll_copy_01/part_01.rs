// Scroll testing with rendering verification
// ====================================================================

/// Extract plain text from a TestBackend buffer after rendering.
fn buffer_to_text(terminal: &ratatui::Terminal<ratatui::backend::TestBackend>) -> String {
    let buf = terminal.backend().buffer();
    let width = buf.area.width as usize;
    let height = buf.area.height as usize;
    let mut lines = Vec::with_capacity(height);
    for y in 0..height {
        let mut line = String::with_capacity(width);
        for x in 0..width {
            let cell = &buf[(x as u16, y as u16)];
            line.push_str(cell.symbol());
        }
        lines.push(line.trim_end().to_string());
    }
    // Trim trailing empty lines
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// Create a test app pre-populated with scrollable content (text + mermaid diagrams).
fn create_scroll_test_app(
    width: u16,
    height: u16,
    diagrams: usize,
    padding: usize,
) -> (App, ratatui::Terminal<ratatui::backend::TestBackend>) {
    crate::tui::mermaid::clear_active_diagrams();
    crate::tui::mermaid::clear_streaming_preview_diagram();

    let mut app = create_test_app();
    let content = App::build_scroll_test_content(diagrams, padding, None);
    app.display_messages = vec![
        DisplayMessage {
            role: "user".to_string(),
            content: "Scroll test".to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        },
        DisplayMessage {
            role: "assistant".to_string(),
            content,
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        },
    ];
    app.bump_display_messages_version();
    app.scroll_offset = 0;
    app.auto_scroll_paused = false;
    app.is_processing = false;
    app.streaming.streaming_text.clear();
    app.status = ProcessingStatus::Idle;
    // Set deterministic session name for snapshot stability
    app.session.short_name = Some("test".to_string());

    let backend = ratatui::backend::TestBackend::new(width, height);
    let terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    (app, terminal)
}

fn create_copy_test_app() -> (App, ratatui::Terminal<ratatui::backend::TestBackend>) {
    let mut app = create_test_app();
    app.display_messages = vec![
        DisplayMessage {
            role: "user".to_string(),
            content: "Show me some code".to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        },
        DisplayMessage {
            role: "assistant".to_string(),
            content: "```rust\nfn main() {\n    println!(\"hello\");\n}\n```".to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        },
    ];
    app.bump_display_messages_version();
    app.scroll_offset = 0;
    app.auto_scroll_paused = false;
    app.is_processing = false;
    app.streaming.streaming_text.clear();
    app.status = ProcessingStatus::Idle;
    app.session.short_name = Some("test".to_string());

    let backend = ratatui::backend::TestBackend::new(100, 30);
    let terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    (app, terminal)
}

fn create_blockquote_copy_test_app() -> (App, ratatui::Terminal<ratatui::backend::TestBackend>) {
    let mut app = create_test_app();
    app.display_messages = vec![
        DisplayMessage {
            role: "user".to_string(),
            content: "Quote something".to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        },
        DisplayMessage {
            role: "assistant".to_string(),
            content: "As they say:\n\n> the quick brown fox\n> jumps over the lazy dog\n\nDone."
                .to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        },
    ];
    app.bump_display_messages_version();
    app.scroll_offset = 0;
    app.auto_scroll_paused = false;
    app.is_processing = false;
    app.streaming.streaming_text.clear();
    app.status = ProcessingStatus::Idle;
    app.session.short_name = Some("test".to_string());

    let backend = ratatui::backend::TestBackend::new(100, 30);
    let terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    (app, terminal)
}

fn create_error_copy_test_app() -> (App, ratatui::Terminal<ratatui::backend::TestBackend>) {
    let mut app = create_test_app();
    app.display_messages = vec![
        DisplayMessage::user("Show me the last error"),
        DisplayMessage::error("permission denied while opening ~/.jcode/config.toml"),
    ];
    app.bump_display_messages_version();
    app.scroll_offset = 0;
    app.auto_scroll_paused = false;
    app.is_processing = false;
    app.streaming.streaming_text.clear();
    app.status = ProcessingStatus::Idle;
    app.session.short_name = Some("test".to_string());

    let backend = ratatui::backend::TestBackend::new(100, 30);
    let terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    (app, terminal)
}

fn create_tool_error_copy_test_app() -> (App, ratatui::Terminal<ratatui::backend::TestBackend>) {
    let mut app = create_test_app();
    app.display_messages = vec![
        DisplayMessage::user("Run the command"),
        DisplayMessage::tool(
            "Error: permission denied",
            crate::message::ToolCall {
                id: "tool_1".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"command": "cat /root/secret"}),
                intent: None, thought_signature: None, },
        ),
    ];
    app.bump_display_messages_version();
    app.scroll_offset = 0;
    app.auto_scroll_paused = false;
    app.is_processing = false;
    app.streaming.streaming_text.clear();
    app.status = ProcessingStatus::Idle;
    app.session.short_name = Some("test".to_string());

    let backend = ratatui::backend::TestBackend::new(100, 30);
    let terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    (app, terminal)
}

fn create_tool_failed_output_copy_test_app()
-> (App, ratatui::Terminal<ratatui::backend::TestBackend>) {
    let mut app = create_test_app();
    app.display_messages = vec![
        DisplayMessage::user("Run the command"),
        DisplayMessage::tool(
            "cat: /root/secret: Permission denied\n\nExit code: 1",
            crate::message::ToolCall {
                id: "tool_1".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"command": "cat /root/secret"}),
                intent: None, thought_signature: None, },
        ),
    ];
    app.bump_display_messages_version();
    app.scroll_offset = 0;
    app.auto_scroll_paused = false;
    app.is_processing = false;
    app.streaming.streaming_text.clear();
    app.status = ProcessingStatus::Idle;
    app.session.short_name = Some("test".to_string());

    let backend = ratatui::backend::TestBackend::new(100, 30);
    let terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    (app, terminal)
}

/// Get the configured scroll up key binding (code, modifiers).
fn scroll_up_key(app: &App) -> (KeyCode, KeyModifiers) {
    (
        app.scroll_keys.up.code.clone(),
        app.scroll_keys.up.modifiers,
    )
}

/// Get the configured scroll down key binding (code, modifiers).
fn scroll_down_key(app: &App) -> (KeyCode, KeyModifiers) {
    (
        app.scroll_keys.down.code.clone(),
        app.scroll_keys.down.modifiers,
    )
}

/// Get the configured scroll up fallback key, or primary scroll up key.
fn scroll_up_fallback_key(app: &App) -> (KeyCode, KeyModifiers) {
    app.scroll_keys
        .up_fallback
        .as_ref()
        .map(|binding| (binding.code.clone(), binding.modifiers))
        .unwrap_or_else(|| scroll_up_key(app))
}

/// Get the configured scroll down fallback key, or primary scroll down key.
fn scroll_down_fallback_key(app: &App) -> (KeyCode, KeyModifiers) {
    app.scroll_keys
        .down_fallback
        .as_ref()
        .map(|binding| (binding.code.clone(), binding.modifiers))
        .unwrap_or_else(|| scroll_down_key(app))
}

/// Get the configured prompt-up key binding (code, modifiers).
fn prompt_up_key(app: &App) -> (KeyCode, KeyModifiers) {
    (
        app.scroll_keys.prompt_up.code.clone(),
        app.scroll_keys.prompt_up.modifiers,
    )
}

fn scroll_render_test_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};

    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Render app to TestBackend and return the buffer text.
fn render_and_snap(
    app: &App,
    terminal: &mut ratatui::Terminal<ratatui::backend::TestBackend>,
) -> String {
    terminal
        .draw(|f| crate::tui::ui::draw(f, app))
        .expect("draw failed");
    buffer_to_text(terminal)
}

#[test]
fn test_armed_new_session_mode_shows_input_hint_and_indicator() {
    let _lock = scroll_render_test_lock();

    let mut app = create_test_app();
    app.input = "draft prompt".to_string();
    app.cursor_pos = app.input.len();
    app.handle_key(KeyCode::Char(' '), KeyModifiers::SUPER)
        .expect("Super+Space should arm new-session mode");

    let backend = ratatui::backend::TestBackend::new(60, 8);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    let rendered = render_and_snap(&app, &mut terminal);

    assert!(
        rendered.contains("↗ Next prompt opens a new session"),
        "rendered UI should show armed-mode hint, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("↗"),
        "rendered UI should show armed-mode indicator icon, got:\n{}",
        rendered
    );
}

#[test]
fn test_chat_native_scrollbar_hidden_when_content_fits() {
    let _lock = scroll_render_test_lock();

    let mut app = create_test_app();
    app.chat_native_scrollbar = true;
    app.display_messages = vec![DisplayMessage {
        role: "assistant".to_string(),
        content: "short response".to_string(),
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: None,
    }];
    app.bump_display_messages_version();
    app.session.short_name = Some("test".to_string());
    app.is_processing = false;
    app.status = ProcessingStatus::Idle;

    let backend = ratatui::backend::TestBackend::new(60, 24);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    let text = render_and_snap(&app, &mut terminal);

    assert_eq!(crate::tui::ui::last_max_scroll(), 0);
    for glyph in ["╷", "╵", "╎"] {
        assert!(
            !text.contains(glyph),
            "did not expect scrollbar glyph {glyph:?} when content fits:\n{text}"
        );
    }
}

#[test]
fn test_chat_native_scrollbar_hides_scroll_counters() {
    let _lock = scroll_render_test_lock();

    let (mut app, mut terminal) = create_scroll_test_app(50, 12, 0, 24);
    app.chat_native_scrollbar = true;
    app.auto_scroll_paused = true;

    let _ = render_and_snap(&app, &mut terminal);
    let max_scroll = crate::tui::ui::last_max_scroll();
    assert!(
        max_scroll > 2,
        "expected scrollable content, got max_scroll={max_scroll}"
    );

    app.scroll_offset = max_scroll / 2;
    let text = render_and_snap(&app, &mut terminal);
    let scroll = app.scroll_offset.min(crate::tui::ui::last_max_scroll());
    let remaining = crate::tui::ui::last_max_scroll().saturating_sub(scroll);

    assert!(
        text.contains('╷') || text.contains('•'),
        "expected native scrollbar thumb to render:\n{text}"
    );
    assert!(
        !text.contains('╎'),
        "did not expect dotted scrollbar track to render:\n{text}"
    );
    assert!(
        !text.contains(&format!("↑{scroll}")),
        "top scroll counter should be hidden when native scrollbar is visible:\n{text}"
    );
    assert!(
        !text.contains(&format!("↓{remaining}")),
        "bottom scroll counter should be hidden when native scrollbar is visible:\n{text}"
    );
}

#[test]
fn test_streaming_repaint_does_not_leave_bracket_artifact() {
    let mut app = create_test_app();
    let backend = ratatui::backend::TestBackend::new(90, 20);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");

    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.streaming.streaming_text = "[".to_string();
    let _ = render_and_snap(&app, &mut terminal);

    app.streaming.streaming_text = "Process A: |██████████|".to_string();
    let text = render_and_snap(&app, &mut terminal);

    assert!(
        text.contains("Process A:"),
        "expected updated streaming prefix to be visible"
    );
    assert!(
        text.contains("████"),
        "expected updated streaming progress bar to be visible"
    );
    assert!(
        !text.lines().any(|line| line.trim() == "["),
        "stale independent '[' artifact should not persist after repaint"
    );
}

#[test]
fn test_chat_mouse_scroll_requests_immediate_redraw_during_streaming() {
    let _lock = scroll_render_test_lock();

    let (mut app, mut terminal) = create_scroll_test_app(50, 12, 0, 36);
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;

    let before = render_and_snap(&app, &mut terminal);
    assert!(
        crate::tui::ui::last_max_scroll() > 2,
        "expected scrollable chat content"
    );

    let scroll_only = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 10,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });

    assert!(app.auto_scroll_paused, "scroll state should update immediately");
    assert_ne!(app.scroll_offset, 0, "scroll offset should change immediately");
    assert!(
        !scroll_only,
        "chat mouse wheel scrolls should request immediate redraw while streaming"
    );

    let after = render_and_snap(&app, &mut terminal);
    assert_ne!(after, before, "immediate redraw should make scroll visible");
}

#[test]
fn test_chat_mouse_wheel_scroll_does_not_recall_prompt_history() {
    let _lock = scroll_render_test_lock();

    let (mut app, mut terminal) = create_scroll_test_app(50, 12, 0, 36);
    render_and_snap(&app, &mut terminal);
    assert!(app.input.is_empty());
    assert!(
        crate::tui::ui::last_max_scroll() > 2,
        "expected scrollable chat content"
    );

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 10,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });

    assert!(
        app.input.is_empty(),
        "mouse-wheel scrolling must not copy the previous prompt into the editor"
    );
    assert!(app.auto_scroll_paused, "wheel-up should pause auto-scroll");
    assert_ne!(app.scroll_offset, 0, "wheel-up should move the transcript");
}

#[test]
fn test_chat_mouse_scroll_down_reaches_bottom_without_dead_zone() {
    let _lock = scroll_render_test_lock();

    let (mut app, mut terminal) = create_scroll_test_app(50, 12, 0, 36);
    render_and_snap(&app, &mut terminal);
    let bottom_scroll = crate::tui::ui::last_resolved_chat_scroll();

    assert!(
        crate::tui::ui::last_max_scroll() > 2,
        "expected scrollable chat content"
    );

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 10,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });
    render_and_snap(&app, &mut terminal);
    let scrolled_up_scroll = crate::tui::ui::last_resolved_chat_scroll();
    assert!(
        scrolled_up_scroll < bottom_scroll,
        "first wheel-up should move the resolved transcript viewport"
    );
    assert!(app.auto_scroll_paused);

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 10,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });
    render_and_snap(&app, &mut terminal);
    let back_at_bottom_scroll = crate::tui::ui::last_resolved_chat_scroll();

    assert_eq!(
        back_at_bottom_scroll, bottom_scroll,
        "one opposite wheel detent should return to bottom"
    );
    assert!(
        !app.auto_scroll_paused,
        "state should follow bottom as soon as the rendered viewport reaches bottom"
    );
}

#[test]
fn test_queued_file_activity_repaint_does_not_leave_trailing_digit_artifact() {
    let _lock = scroll_render_test_lock();

    let mut app = create_test_app();
    let backend = ratatui::backend::TestBackend::new(140, 20);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");

    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.pending_soft_interrupts = vec![
        "⚠️ File activity: /home/jeremy/jcode/src/lib.rs - amber previously read this file: read lines 1-9999"
            .to_string(),
    ];
    let first = render_and_snap(&app, &mut terminal);
    assert!(
        first.contains("1-9999"),
        "expected initial queued alert to render fully"
    );

    app.pending_soft_interrupts = vec![
        "⚠️ File activity: /home/jeremy/jcode/src/lib.rs - amber previously read this file: read lines 1-9"
            .to_string(),
    ];
    let second = render_and_snap(&app, &mut terminal);

    assert!(
        second.contains("⚠ File activity:"),
        "expected queued alert to use width-stable warning glyph, got:\n{second}"
    );
    assert!(
        !second.contains("⚠️ File activity:"),
        "queued alert should not use emoji warning presentation in repaint-sensitive UI:\n{second}"
    );
    assert!(
        second.contains("read lines 1-9"),
        "expected updated queued alert to render, got:\n{second}"
    );
    assert!(
        !second.contains("1-9999"),
        "stale trailing digits from the previous queued alert should not persist after repaint:\n{second}"
    );
}

#[test]
fn test_notification_file_activity_repaint_does_not_leave_trailing_digit_artifact() {
    let _lock = scroll_render_test_lock();

    let mut app = create_test_app();
    let backend = ratatui::backend::TestBackend::new(140, 20);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");

    app.status_notice = Some((
        "File activity · /home/jeremy/jcode/src/lib.rs · read lines 1-9999".to_string(),
        std::time::Instant::now(),
    ));
    let first = render_and_snap(&app, &mut terminal);
    assert!(
        first.contains("1-9999"),
        "expected initial notification to render fully"
    );

    app.status_notice = Some((
        "File activity · /home/jeremy/jcode/src/lib.rs · read lines 1-9".to_string(),
        std::time::Instant::now(),
    ));
    let second = render_and_snap(&app, &mut terminal);

    assert!(
        second.contains("read lines 1-9"),
        "expected updated notification to render, got:\n{second}"
    );
    assert!(
        !second.contains("1-9999"),
        "stale trailing digits from the previous notification should not persist after repaint:\n{second}"
    );
}

#[test]
fn test_file_activity_scroll_reproduces_trailing_ghost_after_native_scroll_like_mutation() {
    let _lock = scroll_render_test_lock();

    let mut app = create_test_app();
    let backend = ratatui::backend::TestBackend::new(120, 12);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");

    let mut lines = vec![
        "⚠️ File activity: /home/jeremy/jcode/src/lib.rs - amber previously read this file: read lines 1-9"
            .to_string(),
    ];
    for idx in 1..=40 {
        lines.push(format!("filler line {idx:02}"));
    }

    // Join as separate markdown paragraphs: the repro depends on the file
    // activity line owning its row with trailing blank cells (so a blank->blank
    // diff skips repainting the injected ghost). Single newlines now soft-wrap
    // into one flowing paragraph, which would repaint over the ghost cells.
    app.display_messages = vec![DisplayMessage::assistant(lines.join("\n\n"))];
    app.bump_display_messages_version();
    app.auto_scroll_paused = true;
    app.scroll_offset = 0;

    // The transcript begins with the persistent header, which can be taller
    // than this 12-row viewport. Scroll until the file activity line is
    // actually on screen instead of assuming it sits at the top.
    let mut clean = render_and_snap(&app, &mut terminal);
    while !clean.contains("read lines") && app.scroll_offset < 200 {
        app.scroll_offset += 1;
        clean = render_and_snap(&app, &mut terminal);
    }
    assert!(
        !clean.contains('Z'),
        "ghost marker must not be present before injection:\n{clean}"
    );
    let target_row = clean
        .lines()
        .position(|line| line.contains("read lines"))
        .unwrap_or_else(|| panic!("expected file activity line to be visible, got:\n{clean}"));
    let target_line = clean.lines().nth(target_row).expect("target line text");
    let trail_start = target_line
        .find("read lines 1-9")
        .expect("expected file activity suffix")
        + "read lines 1-9".len();

    let ghost = ratatui::buffer::Buffer::with_lines(["ZZZZ"]);
    let updates = ghost
        .content()
        .iter()
        .enumerate()
        .map(|(idx, cell)| (trail_start as u16 + idx as u16, target_row as u16, cell));
    terminal
        .backend_mut()
        .draw(updates)
        .expect("inject trailing nines after file activity line");

    app.scroll_offset += 1;
    let scrolled = render_and_snap(&app, &mut terminal);

    assert!(
        scrolled.contains('Z'),
        "expected an injected ghost marker to remain after scroll-like repaint:\n{scrolled}"
    );
}

#[test]
fn test_remote_typing_resumes_bottom_follow_mode() {
    let mut app = create_test_app();
    app.scroll_offset = 7;
    app.auto_scroll_paused = true;

    app.handle_remote_char_input('x');

    assert_eq!(app.input, "x");
    assert_eq!(app.cursor_pos, 1);
    assert_eq!(app.scroll_offset, 0);
    assert!(
        !app.auto_scroll_paused,
        "typing in remote mode should follow newest content, not pin top"
    );
}

#[test]
fn test_remote_shift_slash_preserves_layout_translated_slash() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(app.handle_remote_key(KeyCode::Char('/'), KeyModifiers::SHIFT, &mut remote))
        .unwrap();

    assert_eq!(app.input(), "/");
    assert_eq!(app.cursor_pos(), 1);
}

#[test]
fn test_remote_key_event_shift_slash_preserves_layout_translated_slash() {
    use crossterm::event::{KeyEvent, KeyEventKind};

    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(remote::handle_remote_key_event(
        &mut app,
        KeyEvent::new_with_kind(KeyCode::Char('/'), KeyModifiers::SHIFT, KeyEventKind::Press),
        &mut remote,
    ))
    .unwrap();

    assert_eq!(app.input(), "/");
    assert_eq!(app.cursor_pos(), 1);
}

#[test]
fn test_remote_control_alt_symbol_inserts_layout_translated_text() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(app.handle_remote_key(
        KeyCode::Char('@'),
        KeyModifiers::CONTROL | KeyModifiers::ALT,
        &mut remote,
    ))
    .unwrap();

    assert_eq!(app.input(), "@");
    assert_eq!(app.cursor_pos(), 1);
}

#[test]
fn test_local_alt_s_toggles_typing_scroll_lock() {
    let mut app = create_test_app();

    app.handle_key(KeyCode::Char('s'), KeyModifiers::ALT)
        .unwrap();
    assert_eq!(
        app.status_notice(),
        Some("Typing scroll lock: ON - typing stays at current chat position".to_string())
    );

    app.handle_key(KeyCode::Char('s'), KeyModifiers::ALT)
        .unwrap();
    assert_eq!(
        app.status_notice(),
        Some("Typing scroll lock: OFF - typing follows chat bottom".to_string())
    );
}

#[test]
fn test_local_alt_m_toggles_side_panel_visibility() {
    let mut app = create_test_app();
    app.side_panel = test_side_panel_snapshot("plan", "Plan");
    app.last_side_panel_focus_id = Some("plan".to_string());

    app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT)
        .unwrap();
    assert_eq!(app.side_panel.focused_page_id, None);
    assert_eq!(app.status_notice(), Some("Side panel: OFF".to_string()));

    app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT)
        .unwrap();
    assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("plan"));
    assert_eq!(app.status_notice(), Some("Side panel: Plan".to_string()));
}

#[test]
fn test_local_alt_m_hidden_side_panel_stays_hidden_across_snapshot_update() {
    let mut app = create_test_app();
    app.side_panel = test_side_panel_snapshot("plan", "Plan");
    app.last_side_panel_focus_id = Some("plan".to_string());

    app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT)
        .unwrap();
    assert_eq!(app.side_panel.focused_page_id, None);

    app.set_side_panel_snapshot(test_side_panel_snapshot("plan", "Updated plan"));
    assert_eq!(app.side_panel.focused_page_id, None);
    assert_eq!(app.side_panel.pages[0].title, "Updated plan");

    app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT)
        .unwrap();
    assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("plan"));
    assert_eq!(app.status_notice(), Some("Side panel: Updated plan".to_string()));
}

#[test]
fn test_local_alt_m_falls_back_to_diagram_pane_when_side_panel_is_empty() {
    let mut app = create_test_app();
    app.side_panel = crate::side_panel::SidePanelSnapshot::default();
    app.diagram_pane_enabled = true;

    app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT)
        .unwrap();

    assert!(!app.diagram_pane_enabled);
    assert_eq!(app.status_notice(), Some("Diagram pane: OFF".to_string()));
}

#[test]
fn test_images_do_not_drive_side_panel_visibility() {
    // Images now render inline in the transcript flow, so they must not flip the
    // side panel on, arm an auto-hide timer, or otherwise behave like the old
    // pinned-image side pane.
    let mut app = create_test_app();
    app.is_remote = true;
    app.side_panel = crate::side_panel::SidePanelSnapshot::default();
    app.remote_side_pane_images.push(crate::session::RenderedImage {
        media_type: "image/png".to_string(),
        data: "image-data".to_string(),
        label: Some("preview.png".to_string()),
        source: crate::session::RenderedImageSource::UserInput,
        anchor: None,
    });

    // Auto-hide bookkeeping is now a no-op for images.
    assert!(!app.update_pinned_images_auto_hide());
    assert!(app.pinned_images_auto_hide_deadline.is_none());
    assert!(!app.side_panel_user_hidden);
}

#[test]
fn test_remote_alt_m_toggles_side_panel_visibility() {
    let mut app = create_test_app();
    app.side_panel = test_side_panel_snapshot("plan", "Plan");
    app.last_side_panel_focus_id = Some("plan".to_string());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(app.handle_remote_key(KeyCode::Char('m'), KeyModifiers::ALT, &mut remote))
        .unwrap();
    assert_eq!(app.side_panel.focused_page_id, None);
    assert_eq!(app.status_notice(), Some("Side panel: OFF".to_string()));

    rt.block_on(app.handle_remote_key(KeyCode::Char('m'), KeyModifiers::ALT, &mut remote))
        .unwrap();
    assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("plan"));
    assert_eq!(app.status_notice(), Some("Side panel: Plan".to_string()));
}

#[test]
fn test_remote_typing_scroll_lock_preserves_scroll_position() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.scroll_offset = 7;
    app.auto_scroll_paused = true;

    rt.block_on(app.handle_remote_key(KeyCode::Char('s'), KeyModifiers::ALT, &mut remote))
        .unwrap();
    app.handle_remote_char_input('x');

    assert_eq!(app.input, "x");
    assert_eq!(app.cursor_pos, 1);
    assert_eq!(app.scroll_offset, 7);
    assert!(
        app.auto_scroll_paused,
        "typing scroll lock should preserve paused scroll state"
    );
}

#[test]
fn test_remote_typing_scroll_lock_can_be_toggled_back_off() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.scroll_offset = 7;
    app.auto_scroll_paused = true;

    rt.block_on(app.handle_remote_key(KeyCode::Char('s'), KeyModifiers::ALT, &mut remote))
        .unwrap();
    rt.block_on(app.handle_remote_key(KeyCode::Char('s'), KeyModifiers::ALT, &mut remote))
        .unwrap();
    app.handle_remote_char_input('x');

    assert_eq!(app.scroll_offset, 0);
    assert!(
        !app.auto_scroll_paused,
        "typing should resume following chat bottom after disabling the lock"
    );
}

#[test]
fn test_should_allow_reconnect_takeover_only_after_successful_attach() {
    let mut app = create_test_app();
    let state = super::remote::RemoteRunState {
        reconnect_attempts: 1,
        ..Default::default()
    };

    app.resume_session_id = Some("ses_resume_only".to_string());
    assert!(!super::remote::should_allow_reconnect_takeover(
        &app,
        &state,
        app.resume_session_id.as_deref(),
    ));

    app.remote_session_id = Some("ses_other".to_string());
    assert!(!super::remote::should_allow_reconnect_takeover(
        &app,
        &state,
        app.resume_session_id.as_deref(),
    ));

    app.remote_session_id = Some("ses_resume_only".to_string());
    assert!(super::remote::should_allow_reconnect_takeover(
        &app,
        &state,
        app.resume_session_id.as_deref(),
    ));
    assert!(!super::remote::should_allow_reconnect_takeover(
        &app,
        &super::remote::RemoteRunState::default(),
        app.resume_session_id.as_deref(),
    ));
    assert!(!super::remote::should_allow_reconnect_takeover(
        &app, &state, None,
    ));
}

#[test]
fn test_reconnect_target_prefers_remote_session_id() {
    let mut app = create_test_app();
    app.resume_session_id = Some("ses_resume_idle".to_string());
    app.remote_session_id = Some("ses_remote_active".to_string());

    assert_eq!(
        app.reconnect_target_session_id().as_deref(),
        Some("ses_remote_active")
    );
}

#[test]
fn test_reconnect_target_uses_resume_when_remote_missing() {
    let mut app = create_test_app();
    app.resume_session_id = Some("ses_resume_only".to_string());
    app.remote_session_id = None;

    assert_eq!(
        app.reconnect_target_session_id().as_deref(),
        Some("ses_resume_only")
    );
}

#[test]
fn test_reconnect_target_does_not_consume_resume_session_id() {
    let mut app = create_test_app();
    app.resume_session_id = Some("ses_resume_persistent".to_string());
    app.remote_session_id = None;

    let first = app.reconnect_target_session_id();
    let second = app.reconnect_target_session_id();

    assert_eq!(first.as_deref(), Some("ses_resume_persistent"));
    assert_eq!(second.as_deref(), Some("ses_resume_persistent"));
    assert_eq!(
        app.resume_session_id.as_deref(),
        Some("ses_resume_persistent")
    );
}

#[test]
fn test_prompt_jump_ctrl_brackets() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 20);

    // Seed max scroll estimates before key handling.
    render_and_snap(&app, &mut terminal);

    assert_eq!(app.scroll_offset, 0);
    assert!(!app.auto_scroll_paused);

    app.handle_key(KeyCode::Char('['), KeyModifiers::CONTROL)
        .unwrap();
    assert!(app.auto_scroll_paused);
    assert!(app.scroll_offset > 0);

    let after_up = app.scroll_offset;
    app.handle_key(KeyCode::Char(']'), KeyModifiers::CONTROL)
        .unwrap();
    assert!(app.scroll_offset <= after_up);
}

// NOTE: test_prompt_jump_ctrl_digits_by_recency was removed because it relied on
// pre-render prompt positions that no longer exist. The render-based version
// test_prompt_jump_ctrl_digit_is_recency_rank_in_app covers this functionality.

#[cfg(target_os = "macos")]
#[test]
fn test_prompt_jump_ctrl_esc_fallback_on_macos() {
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 20);

    render_and_snap(&app, &mut terminal);

    assert_eq!(app.scroll_offset, 0);
    app.handle_key(KeyCode::Esc, KeyModifiers::CONTROL).unwrap();
    assert!(app.auto_scroll_paused);
    assert!(app.scroll_offset > 0);
}

#[test]
fn test_ctrl_digit_side_panel_preset_in_app() {
    let mut app = create_test_app();

    app.handle_key(KeyCode::Char('1'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.diagram_pane_ratio_target, 25);

    app.handle_key(KeyCode::Char('2'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.diagram_pane_ratio_target, 50);

    app.handle_key(KeyCode::Char('3'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.diagram_pane_ratio_target, 75);

    app.handle_key(KeyCode::Char('4'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.diagram_pane_ratio_target, 100);
}

#[test]
fn test_chat_overscroll_reveals_status_line_then_rebounds() {
    let _lock = scroll_render_test_lock();

    let (mut app, mut terminal) = create_scroll_test_app(80, 14, 0, 36);

    // Give the app some context so the overscroll line has a percentage to show.
    app.context_info = crate::prompt::ContextInfo {
        total_chars: 40_000,
        ..Default::default()
    };
    app.context_limit = 200_000;

    // Pinned to the bottom: no overscroll line yet. (The idle status line now
    // renders its own short ▰▱ context bar, so the overscroll-specific
    // affordance to assert on is the `(overscroll x.x)` countdown, not the
    // glyphs alone.)
    let pinned = render_and_snap(&app, &mut terminal);
    assert!(!app.chat_overscroll_active(), "should start without overscroll");
    assert!(
        !pinned.contains("(overscroll"),
        "overscroll countdown should be hidden while pinned: {pinned:?}"
    );

    // Scroll down at the bottom => overscroll registered, line revealed.
    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 10,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });
    assert!(
        app.chat_overscroll_active(),
        "overscroll should be active after scrolling down at the bottom"
    );
    let revealed = render_and_snap(&app, &mut terminal);
    assert!(
        revealed.contains("(overscroll"),
        "overscroll status line should show the countdown affordance: {revealed:?}"
    );

    // Scrolling up cancels the overscroll line immediately.
    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 10,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });
    assert!(
        !app.chat_overscroll_active(),
        "scrolling up should cancel the overscroll line"
    );
}
