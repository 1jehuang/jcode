#[test]
fn merge_command_starts_synthetic_turn() {
    let mut app = create_test_app();
    app.input = "/merge".to_string();
    app.submit_input();

    assert!(app.is_processing);
    assert!(app.pending_turn);
    assert_eq!(
        app.display_messages().last().unwrap().content,
        super::commands::merge_launch_notice(false)
    );
    let message = app.session.messages.last().unwrap();
    assert!(message.content.iter().any(|block| matches!(
        block,
        crate::message::ContentBlock::Text { text, .. }
            if text == &super::commands::build_merge_prompt()
    )));
}

#[test]
fn merge_command_interrupts_and_queues_when_busy() {
    let mut app = create_test_app();
    app.is_processing = true;
    app.input = "/merge".to_string();
    app.submit_input();

    assert!(app.cancel_requested);
    assert!(!app.pending_turn);
    assert_eq!(
        app.queued_messages,
        vec![super::commands::build_merge_prompt()]
    );
    assert_eq!(
        app.display_messages().last().unwrap().content,
        super::commands::merge_launch_notice(true)
    );
}

#[test]
fn merge_command_is_discoverable_with_help() {
    let mut app = create_test_app();
    app.input = "/mer".to_string();
    assert!(
        app.command_suggestions()
            .iter()
            .any(|(name, _)| name == "/merge")
    );

    app.input = "/help merge".to_string();
    app.submit_input();
    let help = &app.display_messages().last().unwrap().content;
    for text in [
        "/merge",
        "main/master",
        "HEAD",
        "clean worktree",
        "conflicts",
        "Nothing is pushed",
    ] {
        assert!(help.contains(text), "missing {text} in merge help");
    }
    assert!(!app.is_processing);
}

#[test]
fn merge_prompt_preserves_work_and_checks_result() {
    let prompt = super::commands::build_merge_prompt();
    for rule in [
        "leave HEAD attached",
        "staged, unstaged, and untracked",
        "detached/unborn HEAD",
        "Do not auto-commit, stash, clean, or discard work",
        "If both exist",
        "otherwise ask which to use",
        "If neither exists, stop",
        "already on the destination branch",
        "checked out in another worktree",
        "Stop if validation fails",
        "both branch tips are unchanged",
        "git switch",
        "git merge --no-edit",
        "Never reset, rebase, squash, force-update refs, bypass hooks, push, delete branches",
        "abort only the merge you just started",
        "return to the original branch when safe",
        "validation against the combined result",
        "leave the completed merge intact",
        "source commit is an ancestor of HEAD",
    ] {
        assert!(
            prompt.contains(rule),
            "missing merge safety instruction: {rule}"
        );
    }
}

#[test]
fn merge_command_remote_sends_same_prompt_idle_and_busy() {
    use tokio::io::AsyncBufReadExt;

    let rt = tokio::runtime::Runtime::new().unwrap();
    for busy in [false, true] {
        let mut app = create_test_app();
        rt.block_on(async {
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            let peer = remote.take_dummy_peer().unwrap();
            let mut reader = tokio::io::BufReader::new(peer);
            app.is_remote = true;
            app.is_processing = busy;
            app.input = "/merge".to_string();
            app.cursor_pos = app.input.len();

            app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote)
                .await
                .unwrap();

            let mut line = String::new();
            tokio::time::timeout(
                std::time::Duration::from_secs(2),
                reader.read_line(&mut line),
            )
            .await
            .expect("merge must send a wire request")
            .unwrap();
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["content"], super::commands::build_merge_prompt());
            assert_eq!(
                request["type"],
                if busy { "soft_interrupt" } else { "message" }
            );
            assert!(
                !app.pending_turn,
                "remote command must not start a local turn"
            );
            assert!(
                app.display_messages().iter().any(|message| {
                    message.content == super::commands::merge_launch_notice(busy)
                })
            );
        });
    }
}
