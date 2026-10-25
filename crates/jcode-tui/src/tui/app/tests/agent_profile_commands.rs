use super::*;
use crate::tui::app::agent_profiles::agent_profile_args;

#[test]
fn agent_profile_tokens_do_not_capture_agents_or_paths() {
    assert_eq!(agent_profile_args("/agent"), Some(vec![]));
    assert_eq!(agent_profile_args(" /agent\tresearch "), Some(vec!["research".into()]));
    for input in ["/agents", "/agents review", "/agent.md", "/agent/research", "hello /agent"] {
        assert!(agent_profile_args(input).is_none(), "{input}");
    }
}

#[test]
fn agent_profile_registry_completion_help_and_offline_dispatch() {
    let mut app = create_test_app();
    assert!(crate::tui::app::registered_command_names().any(|name| name == "/agent"));
    assert!(App::command_accepts_args("/agent"));
    assert!(app.get_suggestions_for("/agent re").iter().any(|(name, _)| name == "/agent reload"));
    assert!(app.command_help("agent").unwrap().contains("next turn"));
    assert!(app.command_help("agents").unwrap().contains("role"));
    assert!(crate::tui::app::commands_dispatch::dispatch_local_command(&mut app, "/agent research"));
    assert!(app.display_messages().last().unwrap().content.contains("connected jcode server"));
    assert!(!app.pending_turn);
}

#[test]
fn remote_agent_profile_commands_are_control_traffic_even_while_busy() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut app = create_test_app();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();
    app.is_remote = true;
    app.is_processing = true;
    app.current_message_id = Some(999);
    app.input = "/agent research".into();
    app.cursor_pos = app.input.len();
    rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote)).unwrap();
    assert_eq!(app.agent_profile_request_ids.len(), 1);
    assert!(!app.pending_turn);
    let id = *app.agent_profile_request_ids.iter().next().unwrap();
    app.handle_server_event(crate::protocol::ServerEvent::Error {
        id,
        message: "Session is busy. Retry after the current turn.".into(),
        retry_after_secs: None,
        server_resumes: false,
    }, &mut remote);
    assert!(app.agent_profile_request_ids.is_empty());
    assert!(app.is_processing);
    assert_eq!(app.current_message_id, Some(999));
    assert!(app.display_messages().last().unwrap().content.contains("busy"));

    app.input = "/agent".into();
    app.cursor_pos = app.input.len();
    rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote)).unwrap();
    let id = *app.agent_profile_request_ids.iter().next().unwrap();
    app.handle_server_event(crate::protocol::ServerEvent::RuntimeCommandResult {
        id,
        command: "agent".into(),
        data: serde_json::json!({"current": "research", "available": ["research"]}),
        message: "Current agent profile: research".into(),
    }, &mut remote);
    assert!(app.agent_profile_request_ids.is_empty());
    assert!(app.is_processing);
    assert_eq!(app.current_message_id, Some(999));
    assert_eq!(app.display_messages().last().unwrap().content, "Current agent profile: research");
}

#[test]
fn prepared_agent_profile_input_uses_runtime_not_a_prompt() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut app = create_test_app();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    app.is_remote = true;
    rt.block_on(crate::tui::app::remote::submit_remote_slash_input(
        &mut app,
        &mut remote,
        crate::tui::app::input::PreparedInput {
            image_markers: Default::default(),
            routing_input: None,
            pasted_contents: vec![],
            raw_input: "/agent off".into(),
            expanded: "/agent off".into(),
            images: vec![],
        },
    )).unwrap();
    assert_eq!(app.agent_profile_request_ids.len(), 1);
    assert!(!app.pending_turn);
    assert!(!app.is_processing);
}
