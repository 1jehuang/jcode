use super::*;

fn with_app(f: impl FnOnce(&mut App)) {
    let _guard = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    struct Restore(Vec<(&'static str, Option<std::ffi::OsString>)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            for (key, value) in self.0.drain(..) {
                if let Some(value) = value {
                    crate::env::set_var(key, value);
                } else {
                    crate::env::remove_var(key);
                }
            }
        }
    }
    let _restore = Restore(
        [
            "JCODE_HOME",
            "JCODE_SSH_REMOTE",
            "JCODE_SSH_BINARY",
            "JCODE_SSH_WORKING_DIR",
            "JCODE_SSH_SERVER_SOCKET",
        ]
        .into_iter()
        .map(|key| (key, std::env::var_os(key)))
        .collect(),
    );
    crate::env::set_var("JCODE_HOME", home.path());
    crate::env::set_var("JCODE_SSH_REMOTE", "test-remote");
    crate::env::set_var("JCODE_SSH_BINARY", "/remote/jcode");
    crate::env::set_var("JCODE_SSH_WORKING_DIR", "/remote/repo");
    crate::env::remove_var("JCODE_SSH_SERVER_SOCKET");
    let mut app = App::new_for_remote(None);
    f(&mut app);
}

#[test]
fn ssh_login_picker_is_static_and_never_opens_local_login_overlay() {
    with_app(|app| {
        assert!(app.handle_ssh_login_command("/login"));
        assert!(matches!(app.pending_login, Some(PendingLogin::Remote)));
        assert!(app.login_picker_overlay.is_none());
        let display = app
            .display_messages()
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        for provider in PROVIDERS {
            assert!(display.contains(provider));
        }
        assert!(display.contains("SSH login: choose a provider"));
        app.handle_ssh_login_key(KeyCode::Esc, KeyModifiers::NONE, None);
        assert!(app.pending_login.is_none());
        assert!(app.remote_login.is_none());
    });
}

#[test]
fn ssh_login_callback_never_enters_composer_history_debug_or_paste_storage() {
    with_app(|app| {
        app.handle_ssh_login_command("/login");
        app.remote_login.as_mut().unwrap().phase = Phase::Input;
        app.remote_login.as_mut().unwrap().input_kind = "callback_url".into();
        let secret = "http://localhost:1455/auth/callback?code=secret-callback&state=private-state";
        app.handle_paste(secret.into());
        assert_eq!(app.input, "[hidden login input]");
        assert!(app.pasted_contents.is_empty());
        assert!(
            !serde_json::to_string(&app.create_debug_snapshot())
                .unwrap()
                .contains("secret-callback")
        );
        assert!(!app.pending_turn);
        assert!(app.queued_messages.is_empty());
        app.handle_ssh_login_key(KeyCode::Char('u'), KeyModifiers::CONTROL, None);
        assert!(app.remote_login.as_ref().unwrap().input.is_empty());
        assert!(app.input.is_empty());
        // Sensitive input cannot activate a slash command, even through text APIs.
        super::super::input::handle_text_input(app, "/model secret-callback");
        assert_eq!(app.input, "[hidden login input]");
        assert!(app.pending_model_switch.is_none());
    });
}

#[test]
fn ssh_login_enter_preempts_local_preview_and_preserves_pending_command_privacy() {
    with_app(|app| {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            app.input = "/login".into();
            app.handle_remote_key(KeyCode::Enter, KeyModifiers::NONE, &mut remote)
                .await
                .unwrap();
            assert!(app.remote_login.is_some());
            assert!(!app.pending_turn);
            app.handle_paste("/quit".into());
            app.handle_remote_key(KeyCode::Enter, KeyModifiers::NONE, &mut remote)
                .await
                .unwrap();
            assert!(app.should_quit);
            assert!(app.remote_login.is_none());
        });
    });
}

#[test]
fn ssh_login_unsupported_provider_never_starts_local_auth() {
    with_app(|app| {
        assert!(app.handle_ssh_login_command("/login openrouter"));
        assert!(app.remote_login.is_none());
        assert!(app.pending_login.is_none());
        assert!(
            app.display_messages()
                .iter()
                .any(|m| m.content.contains("SSH login supports:"))
        );
    });
}

#[test]
fn ssh_login_disconnected_keys_stay_in_private_flow() {
    with_app(|app| {
        app.handle_ssh_login_command("/login");
        app.handle_paste("/cancel".into());
        super::super::remote::handle_disconnected_key(app, KeyCode::Enter, KeyModifiers::NONE)
            .unwrap();
        assert!(app.remote_login.is_none());
        assert!(app.queued_messages.is_empty());
        assert!(!app.pending_turn);
        assert!(app.input.is_empty());
    });
}

#[test]
fn ssh_login_success_refreshes_attached_daemon_and_catalog_without_local_login_event() {
    with_app(|app| {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            let peer = remote.take_dummy_peer().unwrap();
            let (reader, _) = peer.into_split();
            let mut reader = BufReader::new(reader);
            app.handle_ssh_login_command("/login");
            let login = app.remote_login.as_mut().unwrap();
            login.provider = "openai".into();
            login.phase = Phase::Completing;
            login.task = Some(Task::ready(Ok(Reply::Authenticated {
                validation_warning: true,
            })));
            assert!(app.poll_ssh_login(&mut remote).await);
            assert!(app.pending_login.is_none());
            for expected in ["notify_auth_changed", "get_model_catalog"] {
                let mut line = String::new();
                tokio::time::timeout(
                    std::time::Duration::from_secs(1),
                    reader.read_line(&mut line),
                )
                .await
                .unwrap()
                .unwrap();
                let value: serde_json::Value = serde_json::from_str(&line).unwrap();
                assert_eq!(value["type"], expected, "{line}");
                if expected == "notify_auth_changed" {
                    assert_eq!(value["provider"], "openai");
                }
            }
            assert!(
                app.display_messages()
                    .iter()
                    .any(|m| m.content.contains("Remote credentials were saved"))
            );
        });
    });
}

#[test]
fn ssh_login_failed_completion_stays_private_and_cancel_clears_state() {
    with_app(|app| {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            app.handle_ssh_login_command("/login");
            let login = app.remote_login.as_mut().unwrap();
            login.phase = Phase::Completing;
            login.provider = "openai".into();
            login.task = Some(Task::ready(Err("Remote login was rejected")));
            assert!(app.poll_ssh_login(&mut remote).await);
            assert!(matches!(
                app.remote_login.as_ref().unwrap().phase,
                Phase::Input
            ));
            app.remote_login.as_mut().unwrap().task = Some(Task::ready(Ok(Reply::Cancelled)));
            app.remote_login.as_mut().unwrap().phase = Phase::Cancelling;
            assert!(app.poll_ssh_login(&mut remote).await);
            assert!(app.remote_login.is_none());
            assert!(app.pending_login.is_none());
            assert!(app.input.is_empty());
        });
    });
}

#[test]
fn ssh_login_cancel_after_queued_error_still_requests_remote_cleanup() {
    with_app(|app| {
        // No spawned command can run before this block returns: the ready channel
        // and dummy connection require no yielding on a single-thread runtime.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            app.handle_ssh_login_command("/login");
            let login = app.remote_login.as_mut().unwrap();
            login.provider = "openai".into();
            login.phase = Phase::Completing;
            login.operation = Some(Operation::Callback);
            login.task = Some(Task::ready(Err("Already finished")));
            app.cancel_ssh_login();
            assert!(app.poll_ssh_login(&mut remote).await);
            let login = app.remote_login.as_ref().unwrap();
            assert!(login.phase == Phase::Cancelling);
            assert!(login.operation == Some(Operation::Cancel));
            assert!(login.task.is_some());
        });
        // Destroy queued tasks without executing a real SSH subprocess.
        drop(runtime);
    });
}
