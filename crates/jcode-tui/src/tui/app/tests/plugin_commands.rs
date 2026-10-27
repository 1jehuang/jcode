use super::*;

#[test]
fn plugin_invalid_arguments_are_handled_without_starting_a_turn() {
    let mut app = create_test_app();
    assert!(super::super::commands_dispatch::dispatch_local_command(
        &mut app,
        "/plugin install"
    ));
    assert!(
        app.display_messages()
            .last()
            .unwrap()
            .content
            .contains("Usage: /plugin")
    );
    assert!(!app.pending_turn && !app.is_processing);
    assert!(app.dialog.is_none());
}

#[test]
fn connected_plugin_invalid_arguments_stay_in_the_tui() {
    let mut app = create_test_app();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();
    app.is_remote = true;
    app.input = "/plugin install".into();
    app.cursor_pos = app.input.len();
    runtime
        .block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote))
        .unwrap();
    assert!(
        app.display_messages()
            .last()
            .unwrap()
            .content
            .contains("Usage: /plugin")
    );
    assert!(!app.pending_turn && !app.is_processing);
    assert!(app.dialog.is_none());
}

#[test]
fn plugin_autocomplete_and_ssh_boundaries_are_registered() {
    let mut app = create_test_app();
    app.input = "/plugi".into();
    app.cursor_pos = app.input.len();
    assert!(app.autocomplete());
    assert_eq!(app.input().trim(), "/plugin");
    for command in [
        "/plugin",
        "/plugin install lint@tools",
        "/plugin marketplace add owner/repo",
    ] {
        assert!(super::super::commands_dispatch::ssh_unsupported_command(
            command
        ));
    }
    assert!(!super::super::commands_dispatch::ssh_unsupported_command(
        "/pluginatic"
    ));
}
