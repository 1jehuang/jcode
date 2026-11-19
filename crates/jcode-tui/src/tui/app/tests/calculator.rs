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
    ] {
        app.handle_calculator_key(key).unwrap();
    }
    let overlay = app.calculator_overlay.as_ref().unwrap();
    assert_eq!(overlay.result, Some(5.0));
    assert_eq!(
        overlay.theme,
        super::CalculatorTheme::Light,
        "calculator should open in a white theme"
    );

    app.handle_calculator_key(crossterm::event::KeyCode::Tab)
        .unwrap();
    assert_eq!(
        app.calculator_overlay.as_ref().unwrap().mode,
        super::CalculatorMode::Programmer
    );
    assert_eq!(app.calculator_overlay.as_ref().unwrap().expression, "2+3");
    app.handle_calculator_key(crossterm::event::KeyCode::Tab)
        .unwrap();

    app.handle_calculator_key(crossterm::event::KeyCode::Char('m'))
        .unwrap();
    assert_eq!(
        app.calculator_overlay.as_ref().unwrap().theme,
        super::CalculatorTheme::Dark
    );
    app.handle_calculator_key(crossterm::event::KeyCode::Char('m'))
        .unwrap();
    app.handle_calculator_key(crossterm::event::KeyCode::Enter)
        .unwrap();

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

    app.handle_calculator_key(crossterm::event::KeyCode::F(9))
        .unwrap();
    assert!(app.calculator_overlay.as_ref().unwrap().history_visible);
    app.handle_calculator_key(crossterm::event::KeyCode::Char('2'))
        .unwrap();
    let overlay = app.calculator_overlay.as_ref().unwrap();
    assert!(!overlay.filtered_history_indices().is_empty());
    assert_eq!(
        overlay.history[overlay.filtered_history_indices()[0]].expression,
        "2+3"
    );
    app.handle_calculator_key(crossterm::event::KeyCode::Enter)
        .unwrap();
    assert_eq!(app.calculator_overlay.as_ref().unwrap().expression, "2+3");
    assert!(!app.calculator_overlay.as_ref().unwrap().history_visible);

    app.handle_calculator_key(crossterm::event::KeyCode::Esc)
        .unwrap();
    assert!(app.calculator_overlay.is_none());
}
