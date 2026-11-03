use super::*;

#[test]
fn test_remote_alt_e_toggles_prompt_history_without_typing() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    let before = crate::tui::ui::viewport::prompt_history_expanded();
    rt.block_on(app.handle_remote_key(KeyCode::Char('e'), KeyModifiers::ALT, &mut remote))
        .unwrap();
    assert!(app.input.is_empty(), "Alt+E must not type: {:?}", app.input);
    assert_ne!(crate::tui::ui::viewport::prompt_history_expanded(), before);
    crate::tui::ui::viewport::toggle_prompt_history_expanded();
}
