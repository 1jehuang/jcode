use super::super::*;
use tokio::io::AsyncBufReadExt;

async fn choose_effort(app: &mut App, remote: &mut RemoteConnection, effort: &str) {
    app.input = format!("/effort {effort}");
    app.cursor_pos = app.input.len();
    key_handling::submit_remote_enter_input(app, remote)
        .await
        .unwrap();
}

async fn read_effort(reader: &mut tokio::io::BufReader<crate::transport::Stream>) -> u64 {
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    match serde_json::from_str::<crate::protocol::Request>(&line).unwrap() {
        crate::protocol::Request::SetReasoningEffort { id, .. } => id,
        other => panic!("unexpected request: {other:?}"),
    }
}

fn effort_reply(
    app: &mut App,
    remote: &mut RemoteConnection,
    id: u64,
    effort: Option<&str>,
    error: bool,
) {
    server_events::handle_server_event(
        app,
        ServerEvent::ReasoningEffortChanged {
            id,
            effort: effort.map(str::to_owned),
            model: None,
            error: error.then(|| "effort rejected".to_string()),
        },
        remote,
    );
}

#[test]
fn rejected_remote_effort_keeps_confirmed_default_or_value() {
    let mut app = crate::tui::app::tests::create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut remote = RemoteConnection::dummy();
        let mut reader = tokio::io::BufReader::new(remote.take_dummy_peer().unwrap());
        app.is_remote = true;
        app.remote_provider_name = Some("OpenAI".into());
        app.remote_provider_model = Some("gpt-5.5".into());
        for confirmed in [None, Some("low")] {
            app.remote_reasoning_effort = confirmed.map(str::to_owned);
            choose_effort(&mut app, &mut remote, "high").await;
            let id = read_effort(&mut reader).await;
            effort_reply(&mut app, &mut remote, id, None, true);
            assert_eq!(app.remote_reasoning_effort.as_deref(), confirmed);
        }
    });
}

#[test]
fn remote_effort_replies_correlate_rapid_requests() {
    let mut app = crate::tui::app::tests::create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut remote = RemoteConnection::dummy();
        let mut reader = tokio::io::BufReader::new(remote.take_dummy_peer().unwrap());
        app.is_remote = true;
        app.remote_provider_name = Some("OpenAI".into());
        app.remote_provider_model = Some("gpt-5.5".into());
        app.remote_reasoning_effort = Some("low".into());
        choose_effort(&mut app, &mut remote, "medium").await;
        let first = read_effort(&mut reader).await;
        choose_effort(&mut app, &mut remote, "high").await;
        let second = read_effort(&mut reader).await;
        effort_reply(&mut app, &mut remote, first, Some("medium"), false);
        assert_eq!(app.remote_reasoning_effort_hint().as_deref(), Some("high"));
        effort_reply(&mut app, &mut remote, first, None, true);
        assert_eq!(app.remote_reasoning_effort_hint().as_deref(), Some("high"));
        effort_reply(&mut app, &mut remote, second, None, true);
        assert_eq!(app.remote_reasoning_effort.as_deref(), Some("medium"));
        choose_effort(&mut app, &mut remote, "high").await;
        let third = read_effort(&mut reader).await;
        effort_reply(&mut app, &mut remote, third, Some("high"), false);
        effort_reply(&mut app, &mut remote, first, Some("medium"), false);
        assert_eq!(app.remote_reasoning_effort.as_deref(), Some("high"));
    });
}

#[test]
fn remote_model_authority_fences_effort_replies_without_clearing_later_requests() {
    let mut app = crate::tui::app::tests::create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut remote = RemoteConnection::dummy();
        let mut reader = tokio::io::BufReader::new(remote.take_dummy_peer().unwrap());
        app.is_remote = true;
        app.remote_provider_name = Some("OpenAI".into());
        app.remote_provider_model = Some("gpt-5.5".into());
        choose_effort(&mut app, &mut remote, "high").await;
        let id = read_effort(&mut reader).await;
        let model_event = |id| {
            serde_json::from_value(serde_json::json!({
                "type": "model_changed", "id": id, "model": "gpt-5.5",
                "provider_name": "OpenAI", "reasoning_effort": null
            }))
            .unwrap()
        };
        // The preceding model-switch acknowledgement must keep its staged effort.
        server_events::handle_server_event(&mut app, model_event(id - 1), &mut remote);
        assert_eq!(app.remote_reasoning_effort_hint().as_deref(), Some("high"));
        effort_reply(&mut app, &mut remote, id, Some("high"), false);
        choose_effort(&mut app, &mut remote, "low").await;
        let next = read_effort(&mut reader).await;
        server_events::handle_server_event(&mut app, model_event(next + 1), &mut remote);
        effort_reply(&mut app, &mut remote, next, Some("low"), false);
        assert_eq!(app.remote_reasoning_effort, None);
        assert!(app.remote_effort_state.requested.is_none());
    });
}

#[test]
fn remote_effort_transport_failure_preserves_confirmed_default() {
    let mut app = crate::tui::app::tests::create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut remote = RemoteConnection::dummy();
        drop(remote.take_dummy_peer().unwrap());
        assert!(
            super::request_remote_effort(&mut app, &mut remote, "high")
                .await
                .is_err()
        );
        assert_eq!(app.remote_reasoning_effort, None);
        assert!(app.remote_effort_state.requested.is_none());
    });
}

#[test]
fn session_id_change_discards_pending_effort_before_history_arrives() {
    let mut app = crate::tui::app::tests::create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut remote = RemoteConnection::dummy();
        let mut reader = tokio::io::BufReader::new(remote.take_dummy_peer().unwrap());
        app.is_remote = true;
        app.remote_session_id = Some("previous-effort-session".into());
        app.remote_provider_name = Some("OpenAI".into());
        app.remote_provider_model = Some("gpt-5.5".into());
        choose_effort(&mut app, &mut remote, "high").await;
        let id = read_effort(&mut reader).await;
        server_events::handle_server_event(
            &mut app,
            ServerEvent::SessionId {
                session_id: "different-effort-session".into(),
            },
            &mut remote,
        );
        effort_reply(&mut app, &mut remote, id, Some("high"), false);
        assert_eq!(app.remote_reasoning_effort, None);
        assert!(app.remote_effort_state.requested.is_none());
    });
}

#[test]
fn initial_session_id_does_not_discard_bootstrap_effort_request() {
    let mut app = crate::tui::app::tests::create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut remote = RemoteConnection::dummy();
        let mut reader = tokio::io::BufReader::new(remote.take_dummy_peer().unwrap());
        app.is_remote = true;
        app.remote_provider_name = Some("OpenAI".into());
        app.remote_provider_model = Some("gpt-5.5".into());
        choose_effort(&mut app, &mut remote, "high").await;
        let id = read_effort(&mut reader).await;
        server_events::handle_server_event(
            &mut app,
            ServerEvent::SessionId {
                session_id: "initial-effort-session".into(),
            },
            &mut remote,
        );
        effort_reply(&mut app, &mut remote, id, Some("high"), false);
        assert_eq!(app.remote_reasoning_effort.as_deref(), Some("high"));
    });
}

#[test]
fn remote_effort_server_updates_do_not_ack_numbered_requests() {
    let mut app = crate::tui::app::tests::create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut remote = RemoteConnection::dummy();
        let mut reader = tokio::io::BufReader::new(remote.take_dummy_peer().unwrap());
        app.is_remote = true;
        app.remote_provider_name = Some("OpenAI".into());
        app.remote_provider_model = Some("gpt-5.5".into());
        app.remote_reasoning_effort = Some("low".into());
        effort_reply(&mut app, &mut remote, 0, Some("none"), false);
        assert_eq!(app.remote_reasoning_effort.as_deref(), Some("none"));
        choose_effort(&mut app, &mut remote, "low").await;
        let first = read_effort(&mut reader).await;
        effort_reply(&mut app, &mut remote, first, Some("low"), false);
        choose_effort(&mut app, &mut remote, "medium").await;
        let pending = read_effort(&mut reader).await;
        app.suppress_next_reasoning_effort_message = true;
        effort_reply(&mut app, &mut remote, 0, Some("none"), false);
        assert_eq!(app.remote_reasoning_effort.as_deref(), Some("none"));
        assert_eq!(
            app.remote_reasoning_effort_hint().as_deref(),
            Some("medium")
        );
        assert_eq!(
            app.remote_effort_state
                .requested
                .as_ref()
                .map(|(id, _)| *id),
            Some(pending)
        );
        assert!(app.suppress_next_reasoning_effort_message);
        effort_reply(&mut app, &mut remote, first, Some("low"), false);
        assert_eq!(app.remote_reasoning_effort.as_deref(), Some("none"));
        effort_reply(&mut app, &mut remote, pending, Some("medium"), false);
        assert_eq!(app.remote_reasoning_effort.as_deref(), Some("medium"));
        assert!(app.remote_effort_state.requested.is_none());
    });
}

#[test]
fn remote_effort_same_session_history_preserves_pending_key_intent() {
    let mut app = crate::tui::app::tests::create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut remote = RemoteConnection::dummy();
        let mut reader = tokio::io::BufReader::new(remote.take_dummy_peer().unwrap());
        app.is_remote = true;
        app.remote_session_id = Some("pending-effort-history".into());
        app.remote_provider_name = Some("OpenAI".into());
        app.remote_provider_model = Some("gpt-5.5".into());
        app.remote_reasoning_effort = Some("low".into());
        app.effort_switch_keys.increase = crate::tui::keybind::KeyBinding {
            code: crossterm::event::KeyCode::F(22),
            modifiers: crossterm::event::KeyModifiers::NONE,
        };
        for expected in ["medium", "high"] {
            app.handle_remote_key(
                crossterm::event::KeyCode::F(22),
                crossterm::event::KeyModifiers::NONE,
                &mut remote,
            )
            .await
            .unwrap();
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            match serde_json::from_str::<crate::protocol::Request>(&line).unwrap() {
                crate::protocol::Request::SetReasoningEffort { effort, .. } => {
                    assert_eq!(effort, expected)
                }
                other => panic!("unexpected request: {other:?}"),
            }
            let history = serde_json::from_value(serde_json::json!({
                "type": "history", "id": 0, "session_id": "pending-effort-history",
                "messages": [], "provider_name": "OpenAI", "provider_model": "gpt-5.5",
                "reasoning_effort": "low"
            }))
            .unwrap();
            server_events::handle_server_event(&mut app, history, &mut remote);
        }
        assert_eq!(app.remote_reasoning_effort.as_deref(), Some("low"));
        assert_eq!(app.remote_reasoning_effort_hint().as_deref(), Some("high"));
        let pending = app.remote_effort_state.requested.as_ref().unwrap().0;
        let different_session = serde_json::from_value(serde_json::json!({
            "type": "history", "id": 0, "session_id": "different-effort-history",
            "messages": [], "provider_name": "OpenAI", "provider_model": "gpt-5.5",
            "reasoning_effort": "low"
        }))
        .unwrap();
        server_events::handle_server_event(&mut app, different_session, &mut remote);
        assert!(app.remote_effort_state.requested.is_none());
        effort_reply(&mut app, &mut remote, pending, Some("high"), false);
        assert_eq!(app.remote_reasoning_effort.as_deref(), Some("low"));
    });
}
