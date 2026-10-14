use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};

use super::create_test_app;

#[test]
fn local_korean_jamo_ctrl_chord_runs_the_latin_shortcut() {
    let mut app = create_test_app();
    app.set_input_for_test("hello world again");
    app.handle_key(KeyCode::Left, KeyModifiers::CONTROL)
        .unwrap();
    app.handle_key_press_event(jcode_tui_core::korean_input::normalize_key_event(
        KeyEvent::new(KeyCode::Char('ㅏ'), KeyModifiers::CONTROL),
    ))
    .unwrap();
    assert_eq!(app.input(), "hello world ");
}

#[test]
fn local_korean_jamo_cmd_chord_does_not_type_the_jamo() {
    let mut app = create_test_app();
    app.set_input_for_test("hi");
    app.handle_key_press_event(jcode_tui_core::korean_input::normalize_key_event(
        KeyEvent::new(KeyCode::Char('ㅍ'), KeyModifiers::SUPER),
    ))
    .unwrap();
    assert_eq!(app.input(), "hi");
}

#[test]
fn local_plain_korean_jamo_still_types() {
    let mut app = create_test_app();
    app.handle_key_press_event(jcode_tui_core::korean_input::normalize_key_event(
        KeyEvent::new(KeyCode::Char('ㅍ'), KeyModifiers::NONE),
    ))
    .unwrap();
    assert_eq!(app.input(), "ㅍ");
}

#[test]
fn connected_remote_korean_chord_and_plain_jamo() {
    let mut app = create_test_app();
    app.set_input_for_test("hello world again");
    app.handle_key(KeyCode::Left, KeyModifiers::CONTROL)
        .unwrap();
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(super::super::apply_terminal_event(
        &mut app,
        &mut terminal,
        &mut remote,
        Some(Ok(Event::Key(KeyEvent::new(
            KeyCode::Char('ㅏ'),
            KeyModifiers::CONTROL,
        )))),
    ))
    .unwrap();
    assert_eq!(app.input(), "hello world ");

    rt.block_on(super::super::apply_terminal_event(
        &mut app,
        &mut terminal,
        &mut remote,
        Some(Ok(Event::Key(KeyEvent::new(
            KeyCode::Char('ㅍ'),
            KeyModifiers::NONE,
        )))),
    ))
    .unwrap();
    assert_eq!(app.input(), "hello world ㅍ");
}

#[test]
fn disconnected_remote_korean_chord_and_plain_jamo() {
    let mut app = create_test_app();
    app.set_input_for_test("hello world again");
    app.handle_key(KeyCode::Left, KeyModifiers::CONTROL)
        .unwrap();
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();

    super::super::handle_terminal_event_while_disconnected(
        &mut app,
        &mut terminal,
        Some(Ok(Event::Key(KeyEvent::new(
            KeyCode::Char('ㅏ'),
            KeyModifiers::CONTROL,
        )))),
    )
    .unwrap();
    assert_eq!(app.input(), "hello world ");

    super::super::handle_terminal_event_while_disconnected(
        &mut app,
        &mut terminal,
        Some(Ok(Event::Key(KeyEvent::new(
            KeyCode::Char('ㅍ'),
            KeyModifiers::NONE,
        )))),
    )
    .unwrap();
    assert_eq!(app.input(), "hello world ㅍ");
}
