use super::*;
use crate::protocol::Request;
use tokio::io::{AsyncBufReadExt, BufReader};

async fn read_request(reader: &mut BufReader<crate::transport::ReadHalf>) -> Request {
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut line))
        .await
        .expect("remote control request timeout")
        .unwrap();
    serde_json::from_str(&line).unwrap()
}

fn history(session_id: &str) -> ServerEvent {
    serde_json::from_value(serde_json::json!({"type": "history", "id": 42,
        "session_id": session_id, "messages": [], "mcp_servers": [], "skills": []}))
    .unwrap()
}

#[test]
fn ssh_session_tokens_do_not_claim_skills_or_paths() {
    for command in [
        "/resume",
        "/sessions",
        "/session",
        "/active",
        "/catchup",
        "/back",
        "/save",
        "/unsave",
        "/fork",
        "/split",
        "/btw",
        "/transfer",
        "/transcript",
    ] {
        assert_eq!(token(command), Some((command, "")));
    }
    assert_eq!(
        token(" /btw  why does this work? "),
        Some(("/btw", "why does this work?"))
    );
    for input in [
        "/resume-helper",
        "/sessions/new",
        "/home/me/session",
        "/saved",
        "ordinary text",
    ] {
        assert!(token(input).is_none(), "{input}");
    }
}

#[test]
fn ssh_fork_hotkey_forks_on_the_host_and_stages_the_prompt() {
    if crate::tui::app::commands_dispatch::ssh_test_runs_in_child(
        "ssh_fork_hotkey_forks_on_the_host_and_stages_the_prompt",
    ) {
        return;
    }
    use crossterm::event::{KeyCode, KeyModifiers};
    crate::env::set_var("JCODE_SSH_BINARY", "/opt/jcode");
    crate::env::set_var("JCODE_SSH_WORKING_DIR", "/srv/repo");
    crate::env::remove_var("JCODE_SSH_SERVER_SOCKET");
    let mut app = crate::tui::app::tests::create_test_app();
    app.is_remote = true;
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let mut remote = RemoteConnection::dummy();
        remote.set_session_id("remote-parent".into());
        remote.mark_history_loaded();
        let peer = remote.take_dummy_peer().unwrap();
        let (reader, _writer) = peer.into_split();
        let mut reader = BufReader::new(reader);

        app.handle_remote_key(KeyCode::Char(' '), KeyModifiers::SUPER, &mut remote)
            .await
            .unwrap();
        assert!(app.route_next_prompt_to_new_session, "hotkey must arm over SSH");
        app.input = "work on this in a fork".into();
        app.cursor_pos = app.input.len();
        app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote)
            .await
            .unwrap();
        let id = match read_request(&mut reader).await {
            Request::Split { id } => id,
            other => panic!("{other:?}"),
        };
        assert!(app.input.is_empty());

        let response = ServerEvent::SplitResponse {
            id,
            new_session_id: "remote-fork".into(),
            new_session_name: "fork".into(),
        };
        assert!(!handle_event(&mut app, &mut remote, &response).await.unwrap());
        handle_server_event(&mut app, response, &mut remote);
        assert!(app.pending_split_prompt.is_none());
        assert!(!app.is_processing, "the parent stays idle");
        assert_eq!(remote.session_id(), Some("remote-parent"), "parent stays attached");
        let staged = App::restore_input_for_reload("remote-fork").expect("prompt staged for child");
        assert_eq!(staged.input, "work on this in a fork");
        assert!(staged.submit_on_restore);
        let shown = &app.display_messages.last().unwrap().content;
        assert!(!shown.contains("unavailable in SSH mode"), "{shown}");
        assert!(
            shown.contains("jcode --ssh test-remote --ssh-binary /opt/jcode --remote-working-dir /srv/repo --resume remote-fork"),
            "{shown}"
        );
    });
}

#[test]
fn ssh_session_wire_controls_and_navigation() {
    if crate::tui::app::commands_dispatch::ssh_test_runs_in_child(
        "ssh_session_wire_controls_and_navigation",
    ) {
        return;
    }
    let mut app = crate::tui::app::tests::create_test_app();
    app.is_remote = true;
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let mut remote = RemoteConnection::dummy();
        remote.set_session_id("remote-parent".into());
        remote.mark_history_loaded();
        let peer = remote.take_dummy_peer().unwrap();
        let (reader, _writer) = peer.into_split();
        let mut reader = BufReader::new(reader);

        for (input, command) in [("/resume", "sessions"), ("/sessions saved", "sessions"),
            ("/session", "session"), ("/active", "active"), ("/catchup", "catchup"),
            ("/transcript", "transcript"), ("/transcript path", "transcript")] {
            assert!(handle_command(&mut app, &mut remote, input).await.unwrap());
            assert!(matches!(read_request(&mut reader).await,
                Request::RuntimeCommand { command: actual, .. } if actual == command));
        }
        handle_command(&mut app, &mut remote, "/save useful remote work").await.unwrap();
        assert!(matches!(read_request(&mut reader).await,
            Request::SetSessionSaved { saved: true, label: Some(label), .. } if label == "useful remote work"));
        handle_command(&mut app, &mut remote, "/unsave").await.unwrap();
        assert!(matches!(read_request(&mut reader).await, Request::SetSessionSaved { saved: false, .. }));

        handle_command(&mut app, &mut remote, "/resume remote-only-child").await.unwrap();
        assert!(matches!(read_request(&mut reader).await,
            Request::ResumeSession { session_id, client_has_local_history: false, allow_session_takeover: false, .. }
            if session_id == "remote-only-child"));
        assert!(!remote.has_loaded_history());
        assert!(app.ssh_sessions.back.is_empty(), "do not commit navigation before success");
        assert!(!handle_event(&mut app, &mut remote, &history("remote-only-child")).await.unwrap());
        app.set_remote_prompt_history_session("remote-only-child");
        remote.set_session_id("remote-only-child".into());
        assert_eq!(app.ssh_sessions.back, ["remote-parent"]);

        handle_command(&mut app, &mut remote, "/back").await.unwrap();
        let back_id = match read_request(&mut reader).await {
            Request::ResumeSession { id, session_id, .. } => { assert_eq!(session_id, "remote-parent"); id },
            other => panic!("unexpected {other:?}"),
        };
        handle_event(&mut app, &mut remote, &ServerEvent::Error { id: back_id,
            message: "still attached elsewhere".into(), retry_after_secs: None }).await.unwrap();
        app.fail_prompt_history_switch(back_id);
        assert_eq!(app.ssh_sessions.back, ["remote-parent"], "failed back must be retryable");
        assert!(matches!(read_request(&mut reader).await, Request::GetHistory { .. }));
        assert!(app.ssh_sessions.navigation.is_none());

        handle_command(&mut app, &mut remote, "/back").await.unwrap();
        assert!(matches!(read_request(&mut reader).await, Request::ResumeSession { .. }));
        handle_event(&mut app, &mut remote, &history("remote-parent")).await.unwrap();
        app.set_remote_prompt_history_session("remote-parent");
        remote.set_session_id("remote-parent".into());
        assert!(app.ssh_sessions.back.is_empty());

        handle_command(&mut app, &mut remote, "/btw a remote side question").await.unwrap();
        let split_id = match read_request(&mut reader).await { Request::Split { id } => id, other => panic!("{other:?}") };
        let response = ServerEvent::SplitResponse { id: split_id, new_session_id: "remote-fork".into(), new_session_name: "fork".into() };
        assert!(handle_event(&mut app, &mut remote, &response).await.unwrap());
        assert!(matches!(read_request(&mut reader).await, Request::ResumeSession { session_id, .. } if session_id == "remote-fork"));
        assert!(app.ssh_sessions.ready_prompt.is_none(), "never send side question before child attachment");
        handle_event(&mut app, &mut remote, &history("remote-fork")).await.unwrap();
        assert_eq!(app.ssh_sessions.ready_prompt.as_deref(), Some("a remote side question"));
        assert_eq!(app.ssh_sessions.back, ["remote-parent"]);
        assert!(app.pending_split_startup_message.is_none(), "must not stage a local terminal child launch");
        app.ssh_sessions.ready_prompt = None;
        app.set_remote_prompt_history_session("remote-fork");
        remote.set_session_id("remote-fork".into());

        handle_command(&mut app, &mut remote, "/transfer").await.unwrap();
        let transfer_id = match read_request(&mut reader).await { Request::Transfer { id } => id, other => panic!("{other:?}") };
        handle_event(&mut app, &mut remote, &ServerEvent::Error { id: transfer_id,
            message: "compaction failed".into(), retry_after_secs: None }).await.unwrap();
        assert!(app.ssh_sessions.child.is_none());
        handle_command(&mut app, &mut remote, "/catchup current").await.unwrap();
        assert!(matches!(read_request(&mut reader).await, Request::GetHistory { .. }));
        assert!(!remote.has_loaded_history());
    });
}

#[test]
fn ssh_session_switches_preserve_pending_journal_owner() {
    if crate::tui::app::commands_dispatch::ssh_test_runs_in_child(
        "ssh_session_switches_preserve_pending_journal_owner",
    ) {
        return;
    }
    let runtime = tokio::runtime::Runtime::new().unwrap();
    for success in [false, true] {
        let mut app = crate::tui::app::tests::create_test_app();
        runtime.block_on(async {
            app.is_remote = true;
            app.set_remote_prompt_history_session("ssh-journal-parent");
            app.record_prompt_history("parent prompt");
            let mut remote = RemoteConnection::dummy();
            remote.set_session_id("ssh-journal-parent".into());
            remote.mark_history_loaded();
            let peer = remote.take_dummy_peer().unwrap();
            let (reader, _writer) = peer.into_split();
            let mut reader = BufReader::new(reader);
            app.clear_session(&mut remote).await.unwrap();
            let clear_id = match read_request(&mut reader).await {
                Request::Clear { id } => id,
                other => panic!("{other:?}"),
            };
            app.record_prompt_history("prompt while clear is pending");
            assert!(
                handle_command(&mut app, &mut remote, "/resume ssh-wrong-target")
                    .await
                    .unwrap()
            );
            assert_eq!(
                app.prompt_history_scope.switch_request,
                Some(clear_id),
                "SSH resume must not overwrite the pending clear owner"
            );
            assert!(app.ssh_sessions.navigation.is_none());
            let mut unexpected = String::new();
            assert!(
                tokio::time::timeout(Duration::from_millis(20), reader.read_line(&mut unexpected))
                    .await
                    .is_err(),
                "{unexpected}"
            );
            if success {
                app.set_remote_prompt_history_session("ssh-journal-cleared");
                remote.set_session_id("ssh-journal-cleared".into());
                assert_eq!(
                    app.merged_prompt_history(),
                    ["prompt while clear is pending"]
                );
                app.set_remote_prompt_history_session("ssh-journal-parent");
                assert_eq!(app.merged_prompt_history(), ["parent prompt"]);
                app.set_remote_prompt_history_session("ssh-journal-cleared");
            } else {
                app.fail_prompt_history_switch(clear_id);
                assert_eq!(
                    app.merged_prompt_history(),
                    ["parent prompt", "prompt while clear is pending"]
                );
            }
            handle_command(&mut app, &mut remote, "/resume ssh-next-target")
                .await
                .unwrap();
            assert!(matches!(read_request(&mut reader).await,
                Request::ResumeSession { session_id, client_has_local_history: false, .. }
                if session_id == "ssh-next-target"));
        });
    }

    let mut app = crate::tui::app::tests::create_test_app();
    runtime.block_on(async {
        app.is_remote = true;
        app.set_remote_prompt_history_session("ssh-child-parent");
        let mut remote = RemoteConnection::dummy();
        remote.set_session_id("ssh-child-parent".into());
        remote.mark_history_loaded();
        let peer = remote.take_dummy_peer().unwrap();
        let (reader, _writer) = peer.into_split();
        let mut reader = BufReader::new(reader);
        handle_command(&mut app, &mut remote, "/btw retain this side question")
            .await
            .unwrap();
        let split_id = match read_request(&mut reader).await {
            Request::Split { id } => id,
            other => panic!("{other:?}"),
        };
        app.clear_session(&mut remote).await.unwrap();
        assert!(
            app.prompt_history_scope.switch_request.is_none(),
            "clear cannot start while an SSH child is pending"
        );
        assert!(app.ssh_sessions.child.is_some());
        handle_event(
            &mut app,
            &mut remote,
            &ServerEvent::SplitResponse {
                id: split_id,
                new_session_id: "ssh-child".into(),
                new_session_name: "child".into(),
            },
        )
        .await
        .unwrap();
        let resume_id = match read_request(&mut reader).await {
            Request::ResumeSession { id, session_id, .. } => {
                assert_eq!(session_id, "ssh-child");
                id
            }
            other => panic!("{other:?}"),
        };
        app.clear_session(&mut remote).await.unwrap();
        assert_eq!(app.prompt_history_scope.switch_request, Some(resume_id));
        handle_event(&mut app, &mut remote, &history("ssh-child"))
            .await
            .unwrap();
        assert_eq!(
            app.ssh_sessions.ready_prompt.as_deref(),
            Some("retain this side question")
        );
    });
}

fn assert_child_wait_keeps_clear_draft(command: &str, test_name: &str) {
    if crate::tui::app::commands_dispatch::ssh_test_runs_in_child(test_name) {
        return;
    }
    use crate::tui::TuiState;
    use crossterm::event::{KeyCode, KeyModifiers};
    let runtime = tokio::runtime::Runtime::new().unwrap();
    for queued in [false, true] {
        for collapsed in [false, true] {
            let mut app = crate::tui::app::tests::create_test_app();
            runtime.block_on(async {
                app.is_remote = true;
                app.remote_session_id = Some("ssh-clear-parent".into());
                app.set_remote_prompt_history_session("ssh-clear-parent");
                app.record_prompt_history("parent prompt");
                let mut remote = RemoteConnection::dummy();
                remote.set_session_id("ssh-clear-parent".into());
                remote.mark_history_loaded();
                let peer = remote.take_dummy_peer().unwrap();
                let (reader, _writer) = peer.into_split();
                let mut reader = BufReader::new(reader);
                app.input = command.into();
                app.cursor_pos = app.input.len();
                app.handle_remote_key(KeyCode::Enter, KeyModifiers::NONE, &mut remote)
                    .await
                    .unwrap();
                let child_id = match read_request(&mut reader).await {
                    Request::Split { id } if !command.starts_with("/transfer") => id,
                    Request::Transfer { id } if command == "/transfer" => id,
                    other => panic!("{other:?}"),
                };
                assert!(app.prompt_history_scope.switch_request.is_none());
                let child_prompt = app.ssh_sessions.child.as_ref().unwrap().prompt.clone();
                let journal = app.merged_prompt_history();

                app.input = "earlier draft".into();
                app.cursor_pos = 3;
                app.remember_input_undo_state();
                app.input.clear();
                app.cursor_pos = 0;
                if collapsed {
                    let paste = if queued {
                        "unsent\nmultiline\ncomposer\nwith\npayload"
                    } else {
                        "/clear\n\n\n\n\n"
                    };
                    crate::tui::app::input::handle_text_paste(&mut app, paste.into());
                } else {
                    crate::tui::app::input::insert_input_text(
                        &mut app,
                        if queued { "unsent composer" } else { "/clear" },
                    );
                }
                app.cursor_pos = 2;
                if !queued && !collapsed {
                    // Keep Enter on /clear with the caret inside its token.
                    app.command_suggestion_selected = app
                        .command_suggestions()
                        .iter()
                        .position(|(command, _)| command == "/clear")
                        .unwrap();
                }
                // Restored drafts may own image bytes before a marker is rendered.
                app.pending_images = vec![("image/png".into(), "fixture-image".into())];
                app.history_draft = Some(("history draft".into(), 4));
                app.input_typing_undo = Some((Instant::now(), app.cursor_pos));
                if queued {
                    app.queued_commands.push("/clear".into());
                }
                let undo = app.input_undo_stack.last().unwrap().clone();
                let before = (
                    app.input.clone(),
                    app.cursor_pos,
                    app.pasted_contents.clone(),
                    app.pending_images.clone(),
                    app.input_undo_stack.clone(),
                    app.input_typing_undo,
                    app.history_draft.clone(),
                    app.queued_commands.clone(),
                );
                if queued {
                    super::super::run_next_queued_remote_command(&mut app, &mut remote).await;
                } else {
                    app.handle_remote_key(KeyCode::Enter, KeyModifiers::NONE, &mut remote)
                        .await
                        .unwrap();
                }
                assert_eq!(
                    (
                        app.input.clone(),
                        app.cursor_pos,
                        app.pasted_contents.clone(),
                        app.pending_images.clone(),
                        app.input_undo_stack.clone(),
                        app.input_typing_undo,
                        app.history_draft.clone(),
                        app.queued_commands.clone(),
                    ),
                    before,
                    "{command}, queued={queued}, collapsed={collapsed}"
                );
                assert_eq!(app.merged_prompt_history(), journal);
                assert_eq!(remote.session_id(), Some("ssh-clear-parent"));
                assert!(app.prompt_history_scope.switch_request.is_none());
                assert_eq!(
                    app.ssh_sessions.child.as_ref().unwrap().request_id,
                    child_id
                );
                assert_eq!(
                    app.ssh_sessions.child.as_ref().unwrap().prompt,
                    child_prompt
                );
                assert!(app.status_notice().unwrap().contains("Wait for"));

                let response = ServerEvent::SplitResponse {
                    id: child_id,
                    new_session_id: "ssh-clear-child".into(),
                    new_session_name: "child".into(),
                };
                assert!(
                    handle_event(&mut app, &mut remote, &response)
                        .await
                        .unwrap()
                );
                // A stray Clear would arrive before this ordered Resume request.
                assert!(matches!(read_request(&mut reader).await,
                    Request::ResumeSession { session_id, .. } if session_id == "ssh-clear-child"));
                app.undo_input_change();
                assert_eq!((app.input.clone(), app.cursor_pos), undo);
                handle_event(&mut app, &mut remote, &history("ssh-clear-child"))
                    .await
                    .unwrap();
                assert_eq!(app.ssh_sessions.ready_prompt, child_prompt);
                assert_eq!(app.ssh_sessions.back, ["ssh-clear-parent"]);
            });
        }
    }
}

#[test]
fn ssh_clear_wait_btw_keeps_complete_draft() {
    assert_child_wait_keeps_clear_draft(
        "/btw keep this side question",
        "ssh_clear_wait_btw_keeps_complete_draft",
    );
}

#[test]
fn ssh_clear_wait_fork_keeps_complete_draft() {
    assert_child_wait_keeps_clear_draft(
        "/fork keep this fork question",
        "ssh_clear_wait_fork_keeps_complete_draft",
    );
}

#[test]
fn ssh_clear_wait_transfer_keeps_complete_draft() {
    assert_child_wait_keeps_clear_draft(
        "/transfer",
        "ssh_clear_wait_transfer_keeps_complete_draft",
    );
}
