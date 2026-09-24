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

#[test]
fn merge_remote_release_command_starts_synthetic_turn() {
    let mut app = create_test_app();
    app.input = "/merge-remote-release".to_string();
    app.submit_input();

    assert!(app.is_processing);
    assert!(app.pending_turn);
    assert_eq!(
        app.display_messages().last().unwrap().content,
        super::commands::merge_remote_release_launch_notice(false)
    );
    let message = app.session.messages.last().unwrap();
    assert!(message.content.iter().any(|block| matches!(
        block,
        crate::message::ContentBlock::Text { text, .. }
            if text == &super::commands::build_merge_remote_release_prompt()
    )));
}

#[test]
fn merge_remote_release_command_interrupts_and_queues_when_busy() {
    let mut app = create_test_app();
    app.is_processing = true;
    app.input = "/merge-remote-release".to_string();
    app.submit_input();

    assert!(app.cancel_requested);
    assert!(!app.pending_turn);
    assert_eq!(
        app.queued_messages,
        vec![super::commands::build_merge_remote_release_prompt()]
    );
    assert_eq!(
        app.display_messages().last().unwrap().content,
        super::commands::merge_remote_release_launch_notice(true)
    );
}

#[test]
fn merge_remote_release_command_is_discoverable_with_help() {
    let mut app = create_test_app();
    for prefix in ["/mer", "/merge-remote"] {
        app.input = prefix.to_string();
        assert!(
            app.command_suggestions()
                .iter()
                .any(|(name, _)| name == "/merge-remote-release"),
            "missing combined command completion for {prefix}"
        );
    }

    app.input = "/help merge-remote-release".to_string();
    app.submit_input();
    let help = &app.display_messages().last().unwrap().content;
    for text in [
        "/merge-remote-release",
        "/remote-release",
        "main/master",
        "HEAD",
        "clean worktree",
        "conflicts",
        "failed validation",
        "already being on the destination branch",
        "remote release conventions",
        "scripts/quick-release.sh --remote",
    ] {
        assert!(
            help.contains(text),
            "missing {text} in combined command help"
        );
    }
    assert!(!app.is_processing);
    assert!(!app.pending_turn);
}

#[test]
fn merge_remote_release_prompt_gates_release_on_verified_merge() {
    let prompt = super::commands::build_merge_remote_release_prompt();
    let merge = super::commands::build_merge_prompt();
    let release = super::commands::build_remote_release_prompt();
    let phase_one = prompt
        .find("Phase 1 (merge only, no push or release):")
        .unwrap();
    let merge_start = prompt.find(&merge).expect("must preserve the merge prompt");
    let gate = prompt.find("Gate: proceed to Phase 2 only after").unwrap();
    let phase_two = prompt.find("Phase 2 (remote release):").unwrap();
    let release_start = prompt
        .find(&release)
        .expect("must preserve the release prompt");
    assert!(phase_one < merge_start);
    assert!(merge_start + merge.len() <= gate);
    assert!(gate < phase_two);
    assert!(phase_two < release_start);

    for rule in [
        "all post-merge validation passes",
        "HEAD is attached to the selected destination",
        "the worktree is clean",
        "the recorded source commit is an ancestor of HEAD",
        "If Phase 1 stops for any reason (including already being on the destination branch)",
        "has conflicts, fails validation",
        "or needs clarification, stop the entire workflow without pushing, tagging, or releasing",
        "The no-push rule and nothing-pushed report above apply to Phase 1 only",
        "release its merged HEAD, never the original feature branch",
        "Do not auto-commit any unexpected work that appears between phases",
        "Stop if the destination branch or HEAD changes unexpectedly",
        "Stop on any push failure before creating a tag or triggering a release",
        "Distinguish a triggered remote workflow from a completed publication",
    ] {
        assert!(
            prompt.contains(rule),
            "missing combined workflow safety instruction: {rule}"
        );
    }
}

#[test]
fn merge_remote_release_command_remote_sends_same_prompt_idle_and_busy() {
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
            app.input = "/merge-remote-release".to_string();
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
            .expect("merge-remote-release must send a wire request")
            .unwrap();
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_eq!(
                request["content"],
                super::commands::build_merge_remote_release_prompt()
            );
            assert_eq!(
                request["type"],
                if busy { "soft_interrupt" } else { "message" }
            );
            assert!(
                !app.pending_turn,
                "remote command must not start a local turn"
            );
            assert!(app.display_messages().iter().any(|message| {
                message.content == super::commands::merge_remote_release_launch_notice(busy)
            }));
        });
    }
}
