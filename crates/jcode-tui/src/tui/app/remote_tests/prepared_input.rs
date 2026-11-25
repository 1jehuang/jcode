#[test]
fn submit_prepared_remote_input_defers_until_history_loads() {
    // Regression for the intermittent "first prompt vanishes / weird render"
    // bug: when a manual submit lands before the bootstrap History payload is
    // applied, the History handler's `session_changed` branch calls
    // `clear_display_messages()` and wipes the just-echoed user message. The
    // submit path must hold the prompt until history loads instead of echoing
    // and sending it into that race.
    let mut app = create_test_app();
    app.is_remote = true;
    app.runtime_mode = crate::tui::app::AppRuntimeMode::RemoteClient;

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    // History has NOT loaded yet (fresh connect window).
    assert!(!remote.has_loaded_history());

    let prepared = crate::tui::app::input::PreparedInput {
        image_markers: Default::default(),
        routing_input: None,
        pasted_contents: Vec::new(),
        raw_input: "hi".to_string(),
        expanded: "hi".to_string(),
        images: vec![],
    };
    rt.block_on(crate::tui::app::remote::submit_prepared_remote_input(
        &mut app,
        &mut remote,
        prepared,
    ))
    .expect("submit should not error while history is loading");

    // The prompt must be held, not echoed or sent.
    assert!(
        !app.is_processing,
        "submit must not begin a remote send before history loads"
    );
    assert!(
        app.display_messages().iter().all(|m| m.role != "user"),
        "user message must not be echoed before history loads (would be clobbered)"
    );
    let held = app
        .pending_prompt_before_history
        .as_ref()
        .expect("prompt should be held until history loads");
    assert_eq!(held.raw_input, "hi");

    // Once history loads, the post-connect dispatcher fires the held prompt.
    remote.mark_history_loaded();
    rt.block_on(process_remote_followups(&mut app, &mut remote));

    assert!(
        app.pending_prompt_before_history.is_none(),
        "held prompt should be consumed once history is loaded"
    );
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.role == "user" && m.content == "hi"),
        "the held prompt should be echoed as a user message after history loads"
    );
    assert!(
        app.is_processing,
        "the held prompt should be sent once history is loaded"
    );
}

#[test]
fn remote_skill_invocation_with_prompt_sends_remote_turn() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.runtime_mode = crate::tui::app::AppRuntimeMode::RemoteClient;
    let temp = tempfile::tempdir().expect("create skill dir");
    let skill_dir = temp.path().join(".jcode/skills/remote-skill");
    std::fs::create_dir_all(&skill_dir).expect("create skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: remote-skill\ndescription: Remote prompt regression skill\n---\nUse it.\n",
    )
    .expect("write skill");
    app.session.working_dir = Some(temp.path().to_string_lossy().to_string());

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();
    rt.block_on(crate::tui::app::remote::submit_remote_slash_input(
        &mut app,
        &mut remote,
        crate::tui::app::input::PreparedInput {
            image_markers: Default::default(),
            routing_input: None,
            pasted_contents: Vec::new(),
            raw_input: "/remote-skill explain the change".to_string(),
            expanded: "/remote-skill explain the change".to_string(),
            images: vec![],
        },
    ))
    .expect("remote skill prompt should send");

    assert_eq!(app.active_skill.as_deref(), Some("remote-skill"));
    assert!(app.is_processing, "remote skill prompt should start a turn");
    assert!(
        app.display_messages()
            .iter()
            .any(|message| message.role == "user"
                && message.content == "/remote-skill explain the change"),
        "remote skill prompt should be visible as the submitted user turn"
    );
}
