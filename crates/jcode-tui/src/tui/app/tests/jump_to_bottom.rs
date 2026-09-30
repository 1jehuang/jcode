// "Jump to bottom" pill + scroll_to_bottom hotkey.

/// Render a long transcript, scroll well away from the bottom, and return the
/// app, terminal, and the rendered screen text.
fn jump_to_bottom_scrolled_up_app() -> (
    App,
    ratatui::Terminal<ratatui::backend::TestBackend>,
    String,
) {
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 0, 60);
    render_and_snap(&app, &mut terminal);
    app.scroll_up(20);
    assert!(app.auto_scroll_paused, "scrolling up should pause auto-follow");
    let screen = render_and_snap(&app, &mut terminal);
    (app, terminal, screen)
}

/// Locate the screen cell (column, row) of the first occurrence of `needle`.
fn find_on_screen(screen: &str, needle: &str) -> Option<(u16, u16)> {
    screen.lines().enumerate().find_map(|(row, line)| {
        line.find(needle).map(|byte_idx| {
            let col = unicode_width::UnicodeWidthStr::width(&line[..byte_idx]);
            (col as u16, row as u16)
        })
    })
}

#[test]
fn jump_to_bottom_pill_shows_key_when_scrolled_up_and_hides_at_bottom() {
    // The config test below swaps JCODE_HOME, which reloads the keybindings.
    let _env_lock = crate::storage::lock_test_env();
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal, screen) = jump_to_bottom_scrolled_up_app();
    let label = app
        .scroll_keys
        .to_bottom_label()
        .expect("scroll_to_bottom has a default binding");
    let expected = format!("Jump to bottom ({label}) ↓");
    assert!(
        screen.contains(&expected),
        "scrolled-up frame should show the pill `{expected}`:\n{screen}"
    );
    // Rounded box around the pill.
    assert!(screen.contains('╭') && screen.contains('╰'), "{screen}");

    app.follow_chat_bottom();
    let bottom = render_and_snap(&app, &mut terminal);
    assert!(
        !bottom.contains("Jump to bottom"),
        "pill must not render at the bottom:\n{bottom}"
    );
    assert!(crate::tui::ui::viewport::jump_to_bottom_area().is_none());
}

#[test]
fn jump_to_bottom_pill_is_hidden_at_tiny_heights() {
    // The config test below swaps JCODE_HOME, which reloads the keybindings.
    let _env_lock = crate::storage::lock_test_env();
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(100, 9, 0, 60);
    render_and_snap(&app, &mut terminal);
    app.scroll_up(20);
    let screen = render_and_snap(&app, &mut terminal);
    assert!(!screen.contains("Jump to bottom"), "{screen}");
}

#[test]
fn scroll_to_bottom_key_follows_chat_bottom_and_clears_bookmark() {
    // The config test below swaps JCODE_HOME, which reloads the keybindings.
    let _env_lock = crate::storage::lock_test_env();
    let _render_lock = scroll_render_test_lock();
    let (mut app, _terminal, _screen) = jump_to_bottom_scrolled_up_app();
    app.scroll_bookmark = Some(7);

    app.handle_key(KeyCode::End, KeyModifiers::CONTROL).unwrap();

    assert_eq!(app.scroll_offset, 0, "Ctrl+End should jump to the bottom");
    assert!(!app.auto_scroll_paused, "Ctrl+End should resume auto-follow");
    assert_eq!(app.scroll_bookmark, None, "Ctrl+End clears the bookmark");
}

#[test]
fn scroll_to_bottom_key_works_in_remote_mode() {
    // The config test below swaps JCODE_HOME, which reloads the keybindings.
    let _env_lock = crate::storage::lock_test_env();
    let _render_lock = scroll_render_test_lock();
    let (mut app, _terminal, _screen) = jump_to_bottom_scrolled_up_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(app.handle_remote_key(KeyCode::End, KeyModifiers::CONTROL, &mut remote))
        .unwrap();

    assert_eq!(app.scroll_offset, 0);
    assert!(!app.auto_scroll_paused);
}

#[test]
fn clicking_jump_to_bottom_pill_jumps_to_bottom() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

    // The config test below swaps JCODE_HOME, which reloads the keybindings.
    let _env_lock = crate::storage::lock_test_env();
    let _render_lock = scroll_render_test_lock();
    let (mut app, _terminal, screen) = jump_to_bottom_scrolled_up_app();
    let (col, row) = find_on_screen(&screen, "Jump to bottom")
        .unwrap_or_else(|| panic!("pill not rendered:\n{screen}"));

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: col + 3,
        row,
        modifiers: KeyModifiers::NONE,
    });

    assert_eq!(app.scroll_offset, 0, "clicking the pill jumps to bottom");
    assert!(!app.auto_scroll_paused);
    assert!(
        app.copy_selection_anchor.is_none(),
        "a click on the pill must not start a text selection"
    );
}

#[test]
fn clicking_outside_jump_to_bottom_pill_does_not_jump() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

    // The config test below swaps JCODE_HOME, which reloads the keybindings.
    let _env_lock = crate::storage::lock_test_env();
    let _render_lock = scroll_render_test_lock();
    let (mut app, _terminal, screen) = jump_to_bottom_scrolled_up_app();
    assert!(screen.contains("Jump to bottom"), "{screen}");
    let before = app.scroll_offset;

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 1,
        row: 2,
        modifiers: KeyModifiers::NONE,
    });

    assert_eq!(app.scroll_offset, before);
    assert!(app.auto_scroll_paused);
}

#[test]
fn scroll_to_bottom_keybinding_is_read_from_config() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());
    crate::config::Config::invalidate_cache();

    let config_path = crate::config::Config::path().expect("config path");
    std::fs::create_dir_all(config_path.parent().expect("config parent"))
        .expect("create config parent");
    std::fs::write(
        &config_path,
        "[keybindings]\nscroll_to_bottom = \"alt+z, ctrl+shift+end\"\n",
    )
    .expect("write config");

    let app = create_test_app();
    let keys = app.scroll_keys.clone();

    match prev_home {
        Some(home) => crate::env::set_var("JCODE_HOME", home),
        None => crate::env::remove_var("JCODE_HOME"),
    }
    crate::config::Config::invalidate_cache();

    assert!(keys.is_to_bottom(KeyCode::Char('z'), KeyModifiers::ALT));
    assert!(keys.is_to_bottom(
        KeyCode::End,
        KeyModifiers::CONTROL | KeyModifiers::SHIFT
    ));
    assert!(
        !keys.is_to_bottom(KeyCode::End, KeyModifiers::CONTROL),
        "configured bindings replace the default"
    );
    assert!(!keys.is_to_bottom(KeyCode::Char('q'), KeyModifiers::ALT));
    let expected_label = format!("{}+Z", jcode_tui_core::keybind::alt_label());
    assert_eq!(keys.to_bottom_label(), Some(expected_label));
}

#[test]
fn scroll_to_bottom_default_binds_ctrl_end_and_alt_q() {
    let _env_lock = crate::storage::lock_test_env();
    let keys = crate::tui::keybind::load_scroll_keys();
    assert!(keys.is_to_bottom(KeyCode::End, KeyModifiers::CONTROL));
    assert!(keys.is_to_bottom(KeyCode::Char('q'), KeyModifiers::ALT));
    assert_eq!(keys.to_bottom_label().as_deref(), Some("Ctrl+End"));
    // Ctrl+G stays the two-way bookmark.
    assert!(!keys.is_to_bottom(KeyCode::Char('g'), KeyModifiers::CONTROL));
    assert!(keys.is_bookmark(KeyCode::Char('g'), KeyModifiers::CONTROL));
}
