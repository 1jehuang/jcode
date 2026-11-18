use super::*;

#[test]
fn daemon_poke_preserves_explicit_local_overnight_campaign() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.is_remote = true;
        // Explicit campaigns remain independent of ordinary todo auto-poke.
        app.auto_poke_default_on = false;
        app.auto_poke_incomplete_todos = false;
        assert!(super::super::commands::handle_session_command(
            &mut app,
            "/overnight 1m campaign regression"
        ));
        assert!(app.overnight_auto_poke.is_some());
        assert_eq!(app.queued_messages.len(), 1);
        app.queued_messages.clear();
        app.pending_queued_dispatch = false;

        // Playback and a queued user redirect must retain priority over the
        // explicit campaign, just as they do for a local in-process session.
        app.is_replay = true;
        assert!(!app.schedule_turn_end_followups());
        app.is_replay = false;
        app.queued_messages.push("user redirect".into());
        assert!(!app.schedule_turn_end_followups());
        app.queued_messages.clear();

        assert!(!app.schedule_auto_poke_followup_if_needed());
        assert!(app.schedule_turn_end_followups_after_response(true));
        assert_eq!(app.queued_messages.len(), 1);
        assert!(app.queued_messages[0].starts_with("Overnight auto-poke for run `"));
        assert!(app.pending_queued_dispatch);
        assert!(!app.auto_poke_incomplete_todos);
    });
}

#[test]
fn daemon_poke_never_schedules_client_continuations() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.is_remote = true;
        app.remote_session_id = Some(app.session.id.clone());
        app.auto_poke_default_on = true;
        app.auto_poke_incomplete_todos = true;
        // Without an explicit campaign, daemon-owned todo policy must not
        // enqueue any second client continuation at the terminal boundary.
        assert!(!app.schedule_turn_end_followups_after_response(false));
        assert!(!app.schedule_turn_end_followups_after_response(true));
        assert!(!app.schedule_auto_poke_followup_if_needed());
        assert!(app.queued_messages.is_empty());
        assert!(!app.pending_queued_dispatch);
        assert!(app.auto_poke_incomplete_todos);
        assert!(app.last_auto_poke_fingerprint.is_none());
    });
}

#[test]
fn daemon_poke_slash_commands_use_control_without_starting_turns() {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _guard = runtime.enter();
    with_temp_jcode_home(|| {
        for (input, args) in [
            ("/poke", vec![]),
            ("/poke on", vec!["on"]),
            ("/poke off", vec!["off"]),
            ("/poke status", vec!["status"]),
            ("/poke invalid", vec!["invalid"]),
        ] {
            let mut app = create_test_app();
            app.is_remote = true;
            app.auto_poke_incomplete_todos = false;
            app.input = input.into();
            app.cursor_pos = app.input.len();
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            let peer = remote.take_dummy_peer().unwrap();
            let mut reader = BufReader::new(peer);
            runtime.block_on(async {
                app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote)
                    .await
                    .unwrap();
                let mut line = String::new();
                tokio::time::timeout(Duration::from_secs(1), reader.read_line(&mut line))
                    .await
                    .unwrap()
                    .unwrap();
                let request: crate::protocol::Request = serde_json::from_str(&line).unwrap();
                assert!(
                    matches!(request,
                    crate::protocol::Request::RuntimeCommand { command, args: wire_args, .. }
                    if command == "poke" && wire_args == args),
                    "{input}"
                );
            });
            assert!(!app.is_processing, "{input}");
            assert!(!app.pending_turn, "{input}");
            assert!(
                !app.auto_poke_incomplete_todos,
                "wait for daemon acknowledgement"
            );
            assert!(app.queued_messages.is_empty(), "{input}");
        }
    });
}
