use super::{create_test_app, handle_server_event, process_remote_followups, startup_history};
use crate::protocol::{Request, ServerEvent};
use crate::tui::app::tui_lifecycle::remote_auto_server_reload;
use crate::tui::backend::RemoteConnection;
use crossterm::event::{KeyCode, KeyModifiers};
use tokio::io::AsyncReadExt;

fn update_history(session_id: &str) -> ServerEvent {
    let mut event = startup_history(session_id);
    if let ServerEvent::History {
        server_has_update,
        server_version,
        ..
    } = &mut event
    {
        *server_has_update = Some(true);
        // Avoid any release-channel repair side effects, including on RED.
        *server_version = Some("v0.1.0-dev (old-handoff)".to_string());
    }
    event
}

#[test]
fn client_reload_safety_handoff_applies_history_without_either_reload_queue() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = rt.enter();
    // The old b6 pair and either independently recognized handoff marker.
    for (resuming, fast_start) in [(true, true), (true, false), (false, true)] {
        let mut app = create_test_app();
        let session_id = app.session.id.clone();
        app.is_remote = true;
        app.resume_session_id = Some(session_id.clone());
        app.remote_session_id = Some(session_id.clone());
        app.auto_server_reload = remote_auto_server_reload(true, true, resuming, fast_start);
        let mut remote = RemoteConnection::dummy();

        // Reconnects receive another bootstrap. The captured policy must last
        // after the startup marker has been consumed, not just for one History.
        for _ in 0..2 {
            handle_server_event(&mut app, update_history(&session_id), &mut remote);
            assert!(
                remote.has_loaded_history(),
                "early deferral must be bypassed"
            );
            assert_eq!(app.remote_session_id.as_deref(), Some(session_id.as_str()));
            assert!(
                !app.pending_server_reload,
                "late update queue must be suppressed"
            );
            assert_eq!(app.server_auto_reload_attempts, 0);
            remote = RemoteConnection::dummy();
        }
    }
}

#[test]
fn client_reload_safety_disabled_config_applies_history_without_reload_queue() {
    let mut app = create_test_app();
    let session_id = app.session.id.clone();
    app.auto_server_reload = remote_auto_server_reload(false, true, false, false);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = rt.enter();
    let mut remote = RemoteConnection::dummy();

    handle_server_event(&mut app, update_history(&session_id), &mut remote);

    assert!(remote.has_loaded_history());
    assert_eq!(app.remote_session_id.as_deref(), Some(session_id.as_str()));
    assert!(!app.pending_server_reload);
}

#[test]
fn client_reload_safety_ordinary_startup_retains_history_reload_policy() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = rt.enter();
    // A fresh launch and an ordinary --resume keep automatic updates enabled.
    for has_resume_target in [false, true] {
        let mut app = create_test_app();
        app.auto_server_reload = remote_auto_server_reload(true, has_resume_target, false, false);
        let mut remote = RemoteConnection::dummy();
        handle_server_event(&mut app, update_history("fresh-session"), &mut remote);
        assert!(!remote.has_loaded_history());
        assert!(app.pending_server_reload);
        assert_eq!(
            app.pending_reload_session_id.as_deref(),
            Some("fresh-session")
        );
    }
}

#[test]
fn client_reload_safety_pending_auto_reload_writes_no_request() {
    let mut app = create_test_app();
    app.auto_server_reload = remote_auto_server_reload(true, true, true, true);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        // Also protect a pre-existing pending reload before History is loaded.
        for history_loaded in [false, true] {
            let mut remote = RemoteConnection::dummy();
            if history_loaded {
                remote.mark_history_loaded();
            }
            let mut peer = remote.take_dummy_peer().expect("isolated socketpair peer");
            app.pending_server_reload = true;
            process_remote_followups(&mut app, &mut remote).await;
            assert!(!app.pending_server_reload);
            assert_eq!(app.server_auto_reload_attempts, 0);
            drop(remote);
            let mut bytes = Vec::new();
            peer.read_to_end(&mut bytes).await.expect("read closed peer");
            assert!(bytes.is_empty(), "automatic path must write no request");
        }
    });
}

#[test]
fn client_reload_safety_explicit_reload_still_writes_forced_reload() {
    let mut app = create_test_app();
    app.auto_server_reload = remote_auto_server_reload(true, true, true, true);
    app.is_remote = true;
    app.remote_server_has_update = Some(true);
    // Deterministically suppress client exec detection and its on-disk save.
    app.client_binary_mtime = None;
    app.input = "/reload".to_string();
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let mut remote = RemoteConnection::dummy();
        remote.mark_history_loaded();
        let mut peer = remote.take_dummy_peer().expect("isolated socketpair peer");
        super::super::handle_remote_key(
            &mut app,
            KeyCode::Enter,
            KeyModifiers::empty(),
            &mut remote,
        )
        .await
        .expect("explicit reload");
        drop(remote);
        let mut bytes = Vec::new();
        peer.read_to_end(&mut bytes).await.expect("read closed peer");
        let wire = String::from_utf8(bytes).expect("JSON request");
        let requests: Vec<Request> = wire
            .lines()
            .map(|line| serde_json::from_str(line).expect("request"))
            .collect();
        assert_eq!(requests.len(), 1);
        assert!(matches!(requests[0], Request::Reload { force: true, .. }));
    });
    assert!(
        !app.auto_server_reload,
        "manual reload must not re-enable automatic policy"
    );
    assert!(
        !app.should_quit,
        "manual server reload must not exec the test client"
    );
}
