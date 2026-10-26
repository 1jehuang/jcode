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

fn profile_reply(id: u64, current: Option<&str>, names: &[&str]) -> crate::protocol::ServerEvent {
    profile_reply_with_sources(id, current, names, vec![
        serde_json::json!({"id":"file:/workspace/AGENTS.md", "kind":"agents_md", "label":"Project AGENTS.md", "path":"/workspace/AGENTS.md", "available":true, "included":false}),
        serde_json::json!({"id":"file:/home/test/AGENTS.md", "kind":"agents_md", "label":"Home AGENTS.md", "path":"/home/test/AGENTS.md", "available":true, "included":false}),
    ])
}

fn profile_reply_with_sources(id: u64, current: Option<&str>, names: &[&str], sources: Vec<serde_json::Value>) -> crate::protocol::ServerEvent {
    let included: Vec<_> = sources.iter().filter(|source| source["included"] == true)
        .map(|source| source["id"].clone()).collect();
    crate::protocol::ServerEvent::RuntimeCommandResult {
        id,
        command: "agent".into(),
        data: serde_json::json!({"current": current, "available": names, "changed": false,
            "included_sources":included, "instruction_sources":sources,
            "instruction_source_notice":"Provider-native instructions may still apply outside Jcode control."}),
        message: "Profiles listed".into(),
    }
}

async fn open_profiles_from_agents(app: &mut App, remote: &mut crate::tui::backend::RemoteConnection) -> u64 {
    app.input = "/agents".into();
    app.cursor_pos = app.input.len();
    app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), remote).await.unwrap();
    let picker = app.inline_interactive_state.as_mut().expect("agents root");
    assert!(picker.entries.iter().any(|entry| matches!(entry.action, crate::tui::PickerAction::AgentTarget(_))));
    picker.selected = picker.entries.iter().position(|entry| matches!(entry.action, crate::tui::PickerAction::AgentProfiles)).unwrap();
    app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), remote).await.unwrap();
    app.agent_profile_picker_request_id.expect("server discovery request")
}

async fn read_profile_wire(reader: &mut (impl tokio::io::AsyncBufRead + Unpin)) -> serde_json::Value {
    use tokio::io::AsyncBufReadExt;
    let mut line = String::new();
    tokio::time::timeout(std::time::Duration::from_secs(2), reader.read_line(&mut line)).await.unwrap().unwrap();
    let request: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(request["type"], "runtime_command");
    assert_eq!(request["command"], "agent");
    request
}

fn render_profiles(app: &App) -> String {
    render_profiles_at(app, 120, 40)
}

fn render_profiles_at(app: &App, width: u16, height: u16) -> String {
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| crate::tui::ui::draw(frame, app as &dyn crate::tui::TuiState)).unwrap();
    let buffer = terminal.backend().buffer();
    (0..height).map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect::<String>()).collect::<Vec<_>>().join("\n")
}

#[test]
fn agents_profiles_render_none_and_apply_only_explicit_runtime_choice() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app();
    rt.block_on(async {
        app.onboarding_flow = None;
        app.is_remote = true;
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.mark_history_loaded();
        let mut reader = tokio::io::BufReader::new(remote.take_dummy_peer().unwrap());
        let id = open_profiles_from_agents(&mut app, &mut remote).await;
        assert_eq!(read_profile_wire(&mut reader).await["args"], serde_json::json!([]));
        app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote).await.unwrap();
        assert_eq!(app.agent_profile_request_ids.len(), 1, "loading Enter is inert");
        app.handle_server_event(profile_reply(id, None, &["reviewer", "research"]), &mut remote);
        let picker = app.inline_interactive_state.as_ref().unwrap();
        assert_eq!(picker.entries[0].name, "None");
        assert_eq!(picker.selected, 0);
        assert!(picker.entries[0].is_current);
        assert!(picker.entries.iter().all(|entry| !entry.is_default));
        assert!(app.agent_profile_request_ids.is_empty());
        assert!(app.session.agent_profile.is_none());
        let rendered = render_profiles(&app);
        assert!(rendered.contains("None"), "{rendered}");
        assert!(rendered.contains("research"), "{rendered}");
        app.handle_remote_key(KeyCode::Down, KeyModifiers::empty(), &mut remote).await.unwrap();
        app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote).await.unwrap();
        assert!(app.agent_profile_request_ids.is_empty(), "choosing a profile only opens a draft");
        assert!(app.showing_agent_profile_sources());
        let rendered = render_profiles(&app);
        for text in ["Instruction sources: research", "Profile alone by default", "Jcode-loaded Markdown", "Base instructions, tool protocol", "Provider-native", "[ ] Project AGENTS.md", "[ ] Home AGENTS.md", "Apply", "Cancel"] {
            assert!(rendered.contains(text), "missing {text}: {rendered}");
        }
        assert_eq!(app.inline_interactive_state.as_ref().unwrap().selected_entry().unwrap().name, "Apply");
        app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote).await.unwrap();
        let request = read_profile_wire(&mut reader).await;
        assert_eq!(request["args"], serde_json::json!(["--select", "research", "[]"]));
        assert!(app.showing_agent_profile_sources(), "wait for authoritative success");
        app.handle_server_event(profile_reply(request["id"].as_u64().unwrap(), Some("research"), &["research"]), &mut remote);
        assert!(app.inline_interactive_state.is_none());
        assert!(!app.pending_turn);
        assert!(!app.is_processing);
        assert!(app.session.agent_profile.is_none(), "no optimistic local mutation");
    });
}

#[test]
fn agents_profiles_saved_current_is_session_only_and_none_sends_off_while_busy() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app();
    rt.block_on(async {
        app.is_remote = true;
        app.is_processing = true;
        app.current_message_id = Some(999);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.mark_history_loaded();
        let mut reader = tokio::io::BufReader::new(remote.take_dummy_peer().unwrap());
        let id = open_profiles_from_agents(&mut app, &mut remote).await;
        read_profile_wire(&mut reader).await;
        app.handle_server_event(profile_reply(id, Some("deleted-snapshot"), &["research"]), &mut remote);
        let picker = app.inline_interactive_state.as_ref().unwrap();
        assert_eq!(picker.selected_entry().unwrap().name, "deleted-snapshot");
        assert!(picker.selected_entry().unwrap().is_current);
        assert!(!picker.schema().shows_default_shortcut_hint);
        app.agent_models_global_scope = true;
        app.handle_remote_key(KeyCode::Char('g'), KeyModifiers::CONTROL, &mut remote).await.unwrap();
        assert!(app.agent_models_global_scope);
        assert!(app.showing_agent_profiles());
        // Selecting current edits its sources without implicitly reloading a deleted snapshot.
        app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote).await.unwrap();
        assert!(app.agent_profile_request_ids.is_empty());
        assert!(app.showing_agent_profile_sources());
        app.handle_remote_key(KeyCode::Esc, KeyModifiers::empty(), &mut remote).await.unwrap();
        app.handle_remote_key(KeyCode::Esc, KeyModifiers::empty(), &mut remote).await.unwrap();
        assert!(app.inline_interactive_state.is_none());

        let id = open_profiles_from_agents(&mut app, &mut remote).await;
        read_profile_wire(&mut reader).await;
        app.handle_server_event(profile_reply(id, Some("deleted-snapshot"), &[]), &mut remote);
        app.handle_remote_key(KeyCode::Home, KeyModifiers::empty(), &mut remote).await.unwrap();
        app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote).await.unwrap();
        let request = read_profile_wire(&mut reader).await;
        assert_eq!(request["args"], serde_json::json!(["off"]));
        let id = request["id"].as_u64().unwrap();
        app.handle_server_event(crate::protocol::ServerEvent::Error {
            id, message: "Session is busy".into(), retry_after_secs: None, server_resumes: false,
        }, &mut remote);
        assert!(app.is_processing);
        assert_eq!(app.current_message_id, Some(999));
        assert!(!app.pending_turn);
        assert!(app.agent_profile_request_ids.is_empty());
    });
}

#[test]
fn agents_profiles_cancel_stale_discovery_and_session_switch_do_not_mutate() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app();
    rt.block_on(async {
        app.is_remote = true;
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.mark_history_loaded();
        let first = open_profiles_from_agents(&mut app, &mut remote).await;
        app.handle_remote_key(KeyCode::Esc, KeyModifiers::empty(), &mut remote).await.unwrap();
        assert!(app.agent_profile_picker_request_id.is_none());
        assert!(app.inline_interactive_state.is_none());
        let second = open_profiles_from_agents(&mut app, &mut remote).await;
        app.handle_server_event(profile_reply(first, Some("old"), &["old"]), &mut remote);
        assert_eq!(app.agent_profile_picker_request_id, Some(second));
        assert!(!app.inline_interactive_state.as_ref().unwrap().entries.iter().any(|entry| entry.name == "old"));
        app.handle_server_event(crate::protocol::ServerEvent::SessionId { session_id: "different-session".into() }, &mut remote);
        assert!(app.agent_profile_picker_request_id.is_none());
        app.is_processing = true;
        app.current_message_id = Some(999);
        app.handle_server_event(crate::protocol::ServerEvent::Error {
            id: second, message: "Unknown runtime command".into(), retry_after_secs: None, server_resumes: false,
        }, &mut remote);
        assert!(app.is_processing);
        assert_eq!(app.current_message_id, Some(999));
        assert!(app.inline_interactive_state.is_none());
        assert!(app.session.agent_profile.is_none());
        assert!(app.agent_profile_request_ids.is_empty());
    });
}

#[test]
fn agents_profiles_old_server_error_stays_visible_without_ending_turn() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app();
    rt.block_on(async {
        app.onboarding_flow = None;
        app.is_remote = true;
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.mark_history_loaded();
        let id = open_profiles_from_agents(&mut app, &mut remote).await;
        app.is_processing = true;
        app.current_message_id = Some(999);
        app.handle_server_event(crate::protocol::ServerEvent::Error {
            id, message: "Unknown runtime command: agent".into(), retry_after_secs: None, server_resumes: false,
        }, &mut remote);
        assert!(app.showing_agent_profiles());
        let picker = app.inline_interactive_state.as_ref().unwrap();
        assert_eq!(picker.entries[0].name, "Instruction profiles unavailable");
        let option = picker.entries[0].active_option().unwrap();
        assert!(!option.available);
        assert!(option.detail.contains("Unknown runtime command: agent"));
        assert!(option.detail.starts_with("Server upgrade required"));
        let rendered = render_profiles(&app);
        assert!(rendered.contains("Instruction profiles unavailable"));
        assert!(rendered.contains("Server upgrade required"), "{rendered}");
        app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote).await.unwrap();
        assert!(app.showing_agent_profiles());
        assert!(app.agent_profile_request_ids.is_empty());
        assert!(app.session.agent_profile.is_none());
        app.handle_remote_key(KeyCode::Esc, KeyModifiers::empty(), &mut remote).await.unwrap();
        assert!(app.inline_interactive_state.is_none());
        let id = open_profiles_from_agents(&mut app, &mut remote).await;
        app.handle_server_event(crate::protocol::ServerEvent::Error {
            id, message: "Agent is busy; try again".into(), retry_after_secs: None, server_resumes: false,
        }, &mut remote);
        let rendered = render_profiles(&app);
        assert!(rendered.contains("Reopen /agents to retry"), "{rendered}");
        assert!(!rendered.contains("Server upgrade required"), "{rendered}");
        assert!(app.is_processing);
        assert_eq!(app.current_message_id, Some(999));
        assert!(!app.pending_turn);
    });
}

async fn profile_key(app: &mut App, remote: &mut crate::tui::backend::RemoteConnection, code: KeyCode) {
    app.handle_remote_key(code, KeyModifiers::empty(), remote).await.unwrap();
}

async fn assert_no_profile_wire(reader: &mut (impl tokio::io::AsyncBufRead + Unpin)) {
    use tokio::io::AsyncBufReadExt;
    let mut line = String::new();
    assert!(tokio::time::timeout(std::time::Duration::from_millis(25), reader.read_line(&mut line)).await.is_err(), "unexpected wire request: {line}");
}

#[test]
fn agent_profile_sources_cancel_current_saved_options_and_switch_defaults_are_drafts() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app();
    rt.block_on(async {
        app.onboarding_flow = None;
        app.is_remote = true;
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.mark_history_loaded();
        let mut reader = tokio::io::BufReader::new(remote.take_dummy_peer().unwrap());
        let id = open_profiles_from_agents(&mut app, &mut remote).await;
        read_profile_wire(&mut reader).await;
        app.handle_server_event(profile_reply_with_sources(id, Some("research"), &["research", "reviewer"], vec![
            serde_json::json!({"id":"file:/workspace/AGENTS.md", "kind":"agents_md", "label":"Project AGENTS.md", "path":"/workspace/AGENTS.md", "available":true, "included":true}),
        ]), &mut remote);
        profile_key(&mut app, &mut remote, KeyCode::Enter).await;
        assert!(render_profiles(&app).contains("[x] Project AGENTS.md"));
        profile_key(&mut app, &mut remote, KeyCode::Home).await;
        profile_key(&mut app, &mut remote, KeyCode::Char(' ')).await;
        assert!(render_profiles(&app).contains("[ ] Project AGENTS.md"));
        profile_key(&mut app, &mut remote, KeyCode::Esc).await;
        assert!(app.showing_agent_profiles());
        assert_eq!(app.inline_interactive_state.as_ref().unwrap().selected_entry().unwrap().name, "research");
        profile_key(&mut app, &mut remote, KeyCode::Enter).await;
        assert!(render_profiles(&app).contains("[x] Project AGENTS.md"), "cancel must discard edits");
        profile_key(&mut app, &mut remote, KeyCode::End).await;
        profile_key(&mut app, &mut remote, KeyCode::Enter).await;
        assert!(app.showing_agent_profiles(), "Cancel returns to the unchanged list");
        profile_key(&mut app, &mut remote, KeyCode::Down).await;
        profile_key(&mut app, &mut remote, KeyCode::Enter).await;
        let rendered = render_profiles(&app);
        assert!(rendered.contains("Instruction sources: reviewer"), "{rendered}");
        assert!(rendered.contains("[ ] Project AGENTS.md"));
        assert!(!rendered.contains("[x] Project AGENTS.md"));
        assert!(app.agent_profile_request_ids.is_empty());
        assert!(app.session.agent_profile.is_none());
        assert_no_profile_wire(&mut reader).await;
    });
}

#[test]
fn agent_profile_sources_apply_error_retry_and_session_stale_reply_preserve_state() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app();
    rt.block_on(async {
        app.onboarding_flow = None;
        app.is_remote = true;
        app.is_processing = true;
        app.current_message_id = Some(999);
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.mark_history_loaded();
        let mut reader = tokio::io::BufReader::new(remote.take_dummy_peer().unwrap());
        let id = open_profiles_from_agents(&mut app, &mut remote).await;
        read_profile_wire(&mut reader).await;
        app.handle_server_event(profile_reply(id, None, &["research"]), &mut remote);
        profile_key(&mut app, &mut remote, KeyCode::Down).await;
        profile_key(&mut app, &mut remote, KeyCode::Enter).await;
        profile_key(&mut app, &mut remote, KeyCode::Home).await;
        profile_key(&mut app, &mut remote, KeyCode::Char(' ')).await;
        profile_key(&mut app, &mut remote, KeyCode::End).await;
        profile_key(&mut app, &mut remote, KeyCode::Up).await;
        profile_key(&mut app, &mut remote, KeyCode::Enter).await;
        let first = read_profile_wire(&mut reader).await;
        assert_eq!(first["args"], serde_json::json!(["--select", "research", "[\"file:/workspace/AGENTS.md\"]"]));
        profile_key(&mut app, &mut remote, KeyCode::Enter).await;
        assert_no_profile_wire(&mut reader).await;
        let first_id = first["id"].as_u64().unwrap();
        app.handle_server_event(crate::protocol::ServerEvent::Error {
            id: first_id, message: "Session is busy; try again".into(), retry_after_secs: None, server_resumes: false,
        }, &mut remote);
        let rendered = render_profiles(&app);
        assert!(rendered.contains("Session is busy; try again"), "{rendered}");
        assert!(rendered.contains("[x] Project AGENTS.md"));
        assert!(app.is_processing);
        assert_eq!(app.current_message_id, Some(999));
        profile_key(&mut app, &mut remote, KeyCode::Enter).await;
        let second = read_profile_wire(&mut reader).await;
        let second_id = second["id"].as_u64().unwrap();
        assert_eq!(second["args"], first["args"]);
        app.handle_server_event(profile_reply(first_id, Some("old"), &["old"]), &mut remote);
        assert!(app.showing_agent_profile_sources(), "a stale success cannot close a newer Apply");
        app.handle_server_event(crate::protocol::ServerEvent::SessionId { session_id: "other-session".into() }, &mut remote);
        assert!(app.inline_interactive_state.is_none());
        let next = open_profiles_from_agents(&mut app, &mut remote).await;
        read_profile_wire(&mut reader).await;
        app.handle_server_event(profile_reply(next, None, &["reviewer"]), &mut remote);
        profile_key(&mut app, &mut remote, KeyCode::Down).await;
        profile_key(&mut app, &mut remote, KeyCode::Enter).await;
        app.handle_server_event(profile_reply(second_id, Some("research"), &["research"]), &mut remote);
        let rendered = render_profiles(&app);
        assert!(rendered.contains("Instruction sources: reviewer"), "{rendered}");
        assert!(app.showing_agent_profile_sources());
        assert!(app.agent_profile_request_ids.is_empty());
        assert!(app.is_processing);
        assert_eq!(app.current_message_id, Some(999));
        assert!(app.session.agent_profile.is_none());
        profile_key(&mut app, &mut remote, KeyCode::Esc).await;
        assert_no_profile_wire(&mut reader).await;
    });
}

#[test]
fn agent_profile_sources_current_apply_preserves_snapshot_and_rejects_stale_profile() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app();
    rt.block_on(async {
        app.onboarding_flow = None;
        app.is_remote = true;
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.mark_history_loaded();
        let mut reader = tokio::io::BufReader::new(remote.take_dummy_peer().unwrap());
        let id = open_profiles_from_agents(&mut app, &mut remote).await;
        read_profile_wire(&mut reader).await;
        app.handle_server_event(profile_reply_with_sources(id, Some("deleted-profile"), &[], vec![
            serde_json::json!({"id":"file:/gone/AGENTS.md", "kind":"agents_md", "label":"Saved AGENTS.md", "path":"/gone/AGENTS.md", "available":false, "included":true}),
            serde_json::json!({"id":"file:/missing/overlay.md", "kind":"prompt_overlay", "label":"Missing overlay", "path":"/missing/overlay.md", "available":false, "included":false}),
        ]), &mut remote);
        profile_key(&mut app, &mut remote, KeyCode::Enter).await;
        let rendered = render_profiles(&app);
        assert!(rendered.contains("[x] Saved AGENTS.md"));
        assert!(rendered.contains("saved snapshot, file unavailable"));
        profile_key(&mut app, &mut remote, KeyCode::Enter).await;
        let request = read_profile_wire(&mut reader).await;
        assert_eq!(request["args"], serde_json::json!(["--sources", "deleted-profile", "[\"file:/gone/AGENTS.md\"]"]));
        app.handle_server_event(crate::protocol::ServerEvent::Error {
            id: request["id"].as_u64().unwrap(), message: "Current profile changed; reopen /agents before editing sources".into(), retry_after_secs: None, server_resumes: false,
        }, &mut remote);
        let rendered = render_profiles(&app);
        assert!(rendered.contains("Current profile changed"), "{rendered}");
        assert!(rendered.contains("[x] Saved AGENTS.md"), "stale-profile rejection keeps the draft");
        assert!(app.session.agent_profile.is_none(), "no optimistic mutation of the other client's profile");
        profile_key(&mut app, &mut remote, KeyCode::Home).await;
        profile_key(&mut app, &mut remote, KeyCode::Char(' ')).await;
        profile_key(&mut app, &mut remote, KeyCode::Down).await;
        profile_key(&mut app, &mut remote, KeyCode::Char(' ')).await;
        assert!(render_profiles(&app).contains("[ ] Missing overlay"), "unavailable extras cannot be enabled");
        profile_key(&mut app, &mut remote, KeyCode::End).await;
        profile_key(&mut app, &mut remote, KeyCode::Up).await;
        profile_key(&mut app, &mut remote, KeyCode::Enter).await;
        assert_eq!(read_profile_wire(&mut reader).await["args"], serde_json::json!(["--sources", "deleted-profile", "[]"]));
    });
}

#[test]
fn agent_profile_sources_old_additive_daemon_metadata_fails_closed() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app();
    rt.block_on(async {
        app.onboarding_flow = None;
        app.is_remote = true;
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.mark_history_loaded();
        let mut reader = tokio::io::BufReader::new(remote.take_dummy_peer().unwrap());
        let id = open_profiles_from_agents(&mut app, &mut remote).await;
        read_profile_wire(&mut reader).await;
        app.handle_server_event(crate::protocol::ServerEvent::RuntimeCommandResult {
            id, command: "agent".into(), data: serde_json::json!({"current":null,"available":["research"]}), message:"legacy list".into(),
        }, &mut remote);
        let rendered = render_profiles(&app);
        assert!(rendered.contains("Server upgrade required"), "{rendered}");
        assert!(rendered.contains("Instruction profiles unavailable"));
        profile_key(&mut app, &mut remote, KeyCode::Enter).await;
        assert!(app.agent_profile_request_ids.is_empty());
        assert!(app.session.agent_profile.is_none());
        assert_no_profile_wire(&mut reader).await;
    });
}

#[test]
fn agent_profile_sources_eighty_columns_wrap_scope_and_keep_controls_visible() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut app = create_test_app();
    rt.block_on(async {
        app.onboarding_flow = None;
        app.is_remote = true;
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.mark_history_loaded();
        let mut reader = tokio::io::BufReader::new(remote.take_dummy_peer().unwrap());
        let id = open_profiles_from_agents(&mut app, &mut remote).await;
        read_profile_wire(&mut reader).await;
        let mut reply = profile_reply_with_sources(id, None, &["research"], (0..30).map(|index|
            serde_json::json!({"id":format!("file:/workspace/{index}/AGENTS.md"), "kind":"agents_md",
                "label":format!("Project {index} AGENTS.md"), "path":format!("/workspace/{index}/AGENTS.md"),
                "available":true,"included":false})).collect());
        if let crate::protocol::ServerEvent::RuntimeCommandResult { data, .. } = &mut reply {
            data["instruction_source_notice"] = serde_json::json!("This controls Jcode-loaded instructions only. The provider still loads and manages its own native instruction files.");
        }
        app.handle_server_event(reply, &mut remote);
        profile_key(&mut app, &mut remote, KeyCode::Down).await;
        profile_key(&mut app, &mut remote, KeyCode::Enter).await;
        let rendered = render_profiles_at(&app, 80, 24);
        for text in ["Instruction sources: research", "Profile alone by default", "Jcode-loaded Markdown", "Base instructions, tool protocol", "provider still loads", "native instruction files", "[ ] Project", "▸ Apply", "Cancel"] {
            assert!(rendered.contains(text), "missing {text}: {rendered}");
        }
        assert!(!rendered.contains("[x]"), "new profile extras must all be OFF");
        profile_key(&mut app, &mut remote, KeyCode::Home).await;
        assert!(render_profiles_at(&app, 80, 24).contains("▸ [ ] Project 0 AGENTS.md"));
        let global = app.agent_models_global_scope;
        app.handle_remote_key(KeyCode::Char('g'), KeyModifiers::CONTROL, &mut remote).await.unwrap();
        assert_eq!(app.agent_models_global_scope, global);
        profile_key(&mut app, &mut remote, KeyCode::End).await;
        assert!(render_profiles_at(&app, 80, 24).contains("▸ Cancel"));
        profile_key(&mut app, &mut remote, KeyCode::Enter).await;
        assert!(app.showing_agent_profiles());
        assert_no_profile_wire(&mut reader).await;
    });
}
