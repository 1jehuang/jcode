#[test]
fn injected_cross_swarm_dm_is_deduped_with_real_sender_icon() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();
    let body = "Details: literal '): delimiter. ".repeat(40);
    let wire = format!("Cross-swarm DM from tulip (swarm 'review: parser'): {body}");
    let mut notification = swarm_dm_notification("tulip", &body, Some("report done"));
    if let ServerEvent::Notification { message, .. } = &mut notification {
        *message = wire.clone();
    }
    handle_server_event(&mut app, notification, &mut remote);
    handle_server_event(
        &mut app,
        soft_interrupt_injected(&wire, Some("system")),
        &mut remote,
    );
    let rows: Vec<_> = app
        .display_messages()
        .iter()
        .filter(|m| m.role == "swarm")
        .collect();
    assert_eq!(rows.len(), 1, "one logical DM row: {rows:?}");
    assert_eq!(
        rows[0].title.as_deref(),
        Some("DM from tulip (swarm 'review: parser')")
    );
    assert_eq!(
        super::super::remote_notifications::swarm_display_full_body(&rows[0].content),
        body.trim()
    );
    let rendered =
        crate::tui::ui::render_swarm_message(rows[0], 100, crate::config::DiffDisplayMode::Off);
    let text: String = rendered[0]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    assert!(text.contains(crate::id::session_icon("tulip")), "{text}");
    assert!(!text.contains('💫'), "{text}");
    assert_no_long_rows(&app);
}

#[test]
fn cross_swarm_dm_injected_and_restored_preserve_sender_and_swarm_identity() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();
    let body = "Details: literal '): delimiter. ".repeat(40);
    for (sender, swarm) in [
        ("tulip", "review: parser"),
        ("tulip", "other"),
        ("fox", "other"),
    ] {
        let wire = format!("Cross-swarm DM from {sender} (swarm '{swarm}'): {body}");
        if swarm == "other" {
            let mut notification = swarm_dm_notification(sender, &body, None);
            if let ServerEvent::Notification { message, .. } = &mut notification {
                *message = wire.clone();
            }
            handle_server_event(&mut app, notification, &mut remote);
        }
        handle_server_event(
            &mut app,
            soft_interrupt_injected(&wire, Some("system")),
            &mut remote,
        );
    }
    let rows: Vec<_> = app
        .display_messages()
        .iter()
        .filter(|m| m.role == "swarm")
        .cloned()
        .collect();
    assert_eq!(rows.len(), 3, "distinct senders or swarms must not merge");
    for (row, sender) in rows.iter().zip(["tulip", "tulip", "fox"]) {
        assert_eq!(
            super::super::remote_notifications::swarm_display_full_body(&row.content),
            body.trim()
        );
        let lines =
            crate::tui::ui::render_swarm_message(row, 100, crate::config::DiffDisplayMode::Off);
        let text: String = lines[0]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert!(text.contains(crate::id::session_icon(sender)), "{text}");
        assert!(!text.contains('💫'), "{text}");
    }
    app.replace_display_messages(vec![
        crate::tui::DisplayMessage::system(format!(
            "Cross-swarm DM from tulip (swarm 'review: parser'): {body}"
        )),
        crate::tui::DisplayMessage::user(
            "Cross-swarm DM from my boss (swarm 'work'): please review",
        ),
        crate::tui::DisplayMessage::user("DM from my boss: please review"),
    ]);
    let restored = &app.display_messages()[0];
    assert_eq!(restored.title, rows[0].title);
    assert_eq!(restored.content, rows[0].content);
    assert_eq!(app.display_messages()[1].role, "user");
    assert_eq!(app.display_messages()[2].role, "user");
    let lines =
        crate::tui::ui::render_swarm_message(restored, 100, crate::config::DiffDisplayMode::Off);
    let text: String = lines[0]
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    assert!(text.contains(crate::id::session_icon("tulip")), "{text}");
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

