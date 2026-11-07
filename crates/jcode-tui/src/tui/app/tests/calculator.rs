#[test]
fn calculator_command_keypad_base_selection_and_invalid_input_work() {
    let mut app = create_test_app();
    assert!(super::commands_dispatch::dispatch_local_command(
        &mut app, "/calc"
    ));
    assert!(app.calculator_overlay.is_some());

    for key in [
        crossterm::event::KeyCode::Char('2'),
        crossterm::event::KeyCode::Char('+'),
        crossterm::event::KeyCode::Char('3'),
        crossterm::event::KeyCode::Enter,
    ] {
        app.handle_calculator_key(key).unwrap();
    }
    let overlay = app.calculator_overlay.as_ref().unwrap();
    assert_eq!(overlay.result, Some(5.0));

    app.handle_calculator_key(crossterm::event::KeyCode::F(4))
        .unwrap();
    assert_eq!(
        app.calculator_overlay
            .as_ref()
            .unwrap()
            .formatted_result()
            .as_deref(),
        Some("0x5")
    );

    app.handle_calculator_key(crossterm::event::KeyCode::Char('C'))
        .unwrap();
    for key in [
        crossterm::event::KeyCode::Char('1'),
        crossterm::event::KeyCode::Char('/'),
        crossterm::event::KeyCode::Char('0'),
        crossterm::event::KeyCode::Enter,
    ] {
        app.handle_calculator_key(key).unwrap();
    }
    assert!(app.calculator_overlay.as_ref().unwrap().error.is_some());

    app.handle_calculator_key(crossterm::event::KeyCode::Esc)
        .unwrap();
    assert!(app.calculator_overlay.is_none());
}
