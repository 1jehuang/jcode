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
        remote.set_session_id("remote-only-child".into());
        assert_eq!(app.ssh_sessions.back, ["remote-parent"]);

        handle_command(&mut app, &mut remote, "/back").await.unwrap();
        let back_id = match read_request(&mut reader).await {
            Request::ResumeSession { id, session_id, .. } => { assert_eq!(session_id, "remote-parent"); id },
            other => panic!("unexpected {other:?}"),
        };
        handle_event(&mut app, &mut remote, &ServerEvent::Error { id: back_id,
            message: "still attached elsewhere".into(), retry_after_secs: None }).await.unwrap();
        assert_eq!(app.ssh_sessions.back, ["remote-parent"], "failed back must be retryable");
        assert!(matches!(read_request(&mut reader).await, Request::GetHistory { .. }));
        assert!(app.ssh_sessions.navigation.is_none());

        handle_command(&mut app, &mut remote, "/back").await.unwrap();
        assert!(matches!(read_request(&mut reader).await, Request::ResumeSession { .. }));
        handle_event(&mut app, &mut remote, &history("remote-parent")).await.unwrap();
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
