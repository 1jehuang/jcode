fn swarm_dm_notification(sender: &str, body: &str, tldr: Option<&str>) -> ServerEvent {
    ServerEvent::Notification {
        from_session: format!("session_{sender}_123"),
        from_name: Some(sender.to_string()),
        notification_type: crate::protocol::NotificationType::Message {
            scope: Some("dm".to_string()),
            channel: None,
            tldr: tldr.map(str::to_string),
        },
        message: format!("DM from {sender}: {body}"),
    }
}

fn soft_interrupt_injected(content: &str, display_role: Option<&str>) -> ServerEvent {
    ServerEvent::SoftInterruptInjected {
        content: content.to_string(),
        display_role: display_role.map(str::to_string),
        point: "A".to_string(),
        tools_skipped: None,
        client_ids: Vec::new(),
    }
}

#[test]
fn agent_completion_wakes_never_become_user_prompts() {
    use crate::tui::DisplayMessage;
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();
    let content = "Agent jaguar finished their work and is ready for more.\n\nRecent output (automatically captured):\nAll tests passed.\n\nUse assign_task to give them more work.";
    app.push_display_message(DisplayMessage::swarm(
        "Swarm · jaguar",
        jcode_tui_messages::collapse_long_swarm_content(content),
    ));
    let before = app.display_messages().len();
    for role in [None, Some("user"), Some("system")] {
        handle_server_event(
            &mut app,
            soft_interrupt_injected(content, role),
            &mut remote,
        );
        assert_eq!(app.display_messages().last().unwrap().role, "swarm");
        assert_eq!(app.display_messages().len(), before);
    }
    app.replace_display_messages(vec![
        DisplayMessage::user("actual user prompt"),
        DisplayMessage::user(content),
        DisplayMessage::system("Agent owl stopped.\n\nNo recent assistant output was captured. Use summary/read_context to inspect results or stop to remove them."),
        DisplayMessage::user("Agent jaguar finished their work and is ready for more."),
    ]);
    let roles: Vec<_> = app.display_messages().iter().map(|m| m.role.as_str()).collect();
    assert_eq!(roles, ["user", "swarm", "swarm", "user"]);
}

#[test]
fn lane_aware_completion_handoff_is_a_swarm_row() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();
    let content = "Agent jaguar finished their work and is ready for more.\n\nLane plan:\nPhase 1 · security: 1/2 completed\n- [pending] Live acceptance\n\nFinal response:\nOffline tests passed. Live acceptance unverified.";
    handle_server_event(&mut app, soft_interrupt_injected(content, None), &mut remote);
    let message = app.display_messages().last().unwrap();
    assert_eq!(message.role, "swarm");
    assert!(crate::tui::app::remote_notifications::swarm_display_full_body(&message.content)
        .contains("[pending] Live acceptance"));
}

fn assert_no_long_rows(app: &crate::tui::app::App) {
    for message in app.display_messages() {
        if message.role == "swarm" {
            let lines = crate::tui::ui::render_swarm_message(
                message,
                100,
                crate::config::DiffDisplayMode::Off,
            );
            assert_eq!(lines.len(), 1, "swarm row must be one row: {message:?}");
        } else {
            assert!(
                !message.content.contains("DM from"),
                "raw DM text leaked into a {} row: {:?}",
                message.role,
                message.content
            );
        }
    }
}

#[test]
fn injected_swarm_dm_is_deduped_against_its_notification_card() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    let body = "Finished the parser refactor. ".repeat(15) + "\n\nDetails follow.";
    handle_server_event(
        &mut app,
        swarm_dm_notification("fox", &body, Some("parser refactor done")),
        &mut remote,
    );
    let before = app.display_messages().len();
    handle_server_event(
        &mut app,
        soft_interrupt_injected(&format!("DM from fox: {body}"), Some("system")),
        &mut remote,
    );
    assert_eq!(
        app.display_messages().len(),
        before,
        "injected copy of an already-shown DM must not add a row"
    );
    assert_no_long_rows(&app);
}

#[test]
fn injected_swarm_dm_without_card_renders_one_collapsed_row() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    let body = "word ".repeat(200);
    handle_server_event(
        &mut app,
        soft_interrupt_injected(&format!("DM from owl: {body}"), Some("system")),
        &mut remote,
    );
    let last = app.display_messages().last().expect("row");
    assert_eq!(last.role, "swarm");
    assert_eq!(last.title.as_deref(), Some("DM from owl"));
    assert_no_long_rows(&app);
}

#[test]
fn restored_display_snapshot_normalizes_only_raw_swarm_deliveries() {
    use crate::tui::DisplayMessage;
    let mut app = create_test_app();
    let body = "Detailed implementation report. ".repeat(40);
    let encoded = jcode_tui_messages::encode_collapsible_swarm_content("report", &body);
    let expanded = jcode_tui_messages::toggle_collapsible_swarm_content(&encoded).unwrap();
    app.replace_display_messages(vec![
        DisplayMessage::system(format!("DM from hatchling: {body}")),
        DisplayMessage::user(format!("#parser from owl: {body}")),
        DisplayMessage::assistant(format!("DM from fox: {body}")),
        DisplayMessage::swarm("DM from sheep", expanded.clone()),
    ]);
    let messages = app.display_messages();
    for message in &messages[..2] {
        assert_eq!(message.role, "swarm");
        assert_eq!(
            crate::tui::ui::render_swarm_message(
                message,
                100,
                crate::config::DiffDisplayMode::Off,
            ).len(),
            1,
        );
    }
    assert_eq!(messages[2].role, "assistant");
    assert_eq!(messages[3].content, expanded);
}

#[test]
fn history_restore_renders_swarm_deliveries_as_one_row() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    let mut history = startup_history("history-session");
    if let ServerEvent::History { messages, .. } = &mut history {
        let long = "Investigated the flaky test in depth. ".repeat(20);
        for (role, content) in [
            ("user", format!("DM from fox: {long}")),
            ("system", format!("#parser from owl: {long}")),
            ("user", "please review my change".to_string()),
            ("assistant", format!("DM from fox: {long}")),
        ] {
            messages.push(crate::protocol::HistoryMessage {
                response_stats: None,
                role: role.to_string(),
                content,
                tool_calls: None,
                tool_data: None,
            });
        }
    }
    handle_server_event(&mut app, history, &mut remote);

    let roles: Vec<(String, Option<String>)> = app
        .display_messages()
        .iter()
        .map(|m| (m.role.clone(), m.title.clone()))
        .collect();
    assert!(
        roles.contains(&("swarm".to_string(), Some("DM from fox".to_string()))),
        "{roles:?}"
    );
    assert!(
        roles.contains(&("swarm".to_string(), Some("#parser · owl".to_string()))),
        "{roles:?}"
    );
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.role == "user" && m.content == "please review my change"),
        "ordinary user prompts are untouched: {roles:?}"
    );
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.role == "assistant" && m.content.starts_with("DM from fox")),
        "assistant text is never rewritten: {roles:?}"
    );
    for message in app.display_messages().iter().filter(|m| m.role == "swarm") {
        let lines =
            crate::tui::ui::render_swarm_message(message, 100, crate::config::DiffDisplayMode::Off);
        assert_eq!(lines.len(), 1, "{message:?}");
    }
}
