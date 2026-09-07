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
fn ssh_login_picker_uses_shared_inline_ui_without_local_auth() {
    with_app(|app| {
        assert!(app.handle_ssh_login_command("/login"));
        assert!(matches!(app.pending_login, Some(PendingLogin::Remote)));
        assert!(app.login_picker_overlay.is_none());
        let picker = app.inline_interactive_state.as_ref().unwrap();
        assert_eq!(picker.kind, crate::tui::PickerKind::Login);
        assert_eq!(picker.entries.len(), 8);
        for provider in PROVIDERS {
            assert!(picker.entries.iter().any(|entry| matches!(entry.action,
                crate::tui::PickerAction::RemoteLogin { provider: id, import: false } if id == provider)));
        }
        assert_eq!(picker.entries[0].name, "Import local OpenAI login");
        assert_eq!(picker.entries[1].name, "Import local Claude login");
        assert!(
            picker
                .entries
                .iter()
                .all(|entry| entry.options[0].detail.contains("test-remote"))
        );
        assert!(picker.entries.iter().all(|entry| !entry.is_current));
        app.handle_ssh_login_key(KeyCode::Esc, KeyModifiers::NONE, None);
        assert!(app.pending_login.is_none());
        assert!(app.remote_login.is_none());
        assert!(app.inline_interactive_state.is_none());
    });
}

#[test]
fn ssh_import_requires_explicit_consent_before_any_task_and_masks_private_input() {
    with_app(|app| {
        for provider in ["openai", "claude"] {
            assert!(app.handle_ssh_login_command(&format!("/login --import-local {provider}")));
            let login = app.remote_login.as_ref().unwrap();
            assert!(login.phase == Phase::ImportConsent);
            assert!(login.operation == Some(Operation::Import));
            assert!(login.task.is_none());
            assert_eq!(login.provider, provider);
            assert!(matches!(app.pending_login, Some(PendingLogin::Remote)));
            assert!(app.login_picker_overlay.is_none());
            let warning = &app.display_messages().last().unwrap().content;
            for required in [
                "test-remote",
                provider,
                "usable credentials",
                "selected active local account",
                "token refresh conflicts",
                "one-time copy",
                "no sync",
                "will not be overwritten",
                "No local credentials have been read or exported",
                "Type exactly confirm",
            ] {
                assert!(warning.contains(required), "{required}");
            }
            for rejected in [
                "",
                "yes",
                "CONFIRM",
                "confirm extra",
                "private-not-a-confirmation",
            ] {
                app.handle_paste(rejected.into());
                if !rejected.is_empty() {
                    assert_eq!(app.input, "[hidden login input]");
                }
                assert!(app.pasted_contents.is_empty());
                assert!(
                    !serde_json::to_string(&app.create_debug_snapshot())
                        .unwrap()
                        .contains("private-not-a-confirmation")
                );
                app.handle_ssh_login_key(KeyCode::Enter, KeyModifiers::NONE, None);
                let login = app.remote_login.as_ref().unwrap();
                assert!(login.phase == Phase::ImportConsent);
                assert!(login.task.is_none());
                assert!(!app.pending_turn);
                assert!(app.queued_messages.is_empty());
            }
            app.handle_ssh_login_key(KeyCode::Esc, KeyModifiers::NONE, None);
            assert!(app.remote_login.is_none());
            assert!(app.pending_login.is_none());
            assert_eq!(
                app.display_messages().last().unwrap().content,
                "SSH credential import cancelled. No local credentials were read or copied."
            );
        }
    });
}

#[test]
fn ssh_import_all_consent_cancel_paths_are_local_and_quit_is_preserved() {
    with_app(|app| {
        for cancel in ["/cancel", "/stop", "cancel", "/quit", "/exit"] {
            app.should_quit = false;
            app.handle_ssh_login_command("/login --import-local openai");
            app.handle_paste(cancel.into());
            app.handle_ssh_login_key(KeyCode::Enter, KeyModifiers::NONE, None);
            assert!(app.remote_login.is_none());
            assert!(app.pending_login.is_none());
            assert_eq!(app.should_quit, matches!(cancel, "/quit" | "/exit"));
            assert!(app.input.is_empty());
            assert!(app.pasted_contents.is_empty());
        }
        app.handle_ssh_login_command("/login --import-local claude");
        app.handle_ssh_login_key(KeyCode::Char('c'), KeyModifiers::CONTROL, None);
        assert!(app.remote_login.is_none());
    });
}

#[test]
fn ssh_import_invalid_syntax_never_starts_auth_or_confirmation() {
    with_app(|app| {
        for command in [
            "/login --import-local copilot",
            "/login --import-local openai confirm",
            "/login --import-local --all",
            "/login openai --import-local",
        ] {
            assert!(app.handle_ssh_login_command(command));
            assert!(app.remote_login.is_none());
            assert!(app.pending_login.is_none());
            assert!(app.login_picker_overlay.is_none());
        }
    });
}

#[test]
fn ssh_import_confirm_without_runtime_does_not_export_or_spawn() {
    with_app(|app| {
        assert!(tokio::runtime::Handle::try_current().is_err());
        app.handle_ssh_login_command("/login --import-local openai");
        app.handle_paste("confirm".into());
        app.handle_ssh_login_key(KeyCode::Enter, KeyModifiers::NONE, None);
        assert!(app.remote_login.is_none());
        assert!(app.pending_login.is_none());
        assert!(
            app.display_messages()
                .last()
                .unwrap()
                .content
                .contains("No local credentials were accessed")
        );
    });
}

#[test]
fn ssh_import_success_refreshes_attached_remote_daemon_and_catalog() {
    with_app(|app| {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            for provider in ["openai", "claude"] {
                let mut remote = crate::tui::backend::RemoteConnection::dummy();
                let peer = remote.take_dummy_peer().unwrap();
                let (reader, _) = peer.into_split();
                let mut reader = BufReader::new(reader);
                app.handle_ssh_login_command(&format!("/login --import-local {provider}"));
                let login = app.remote_login.as_mut().unwrap();
                // Synthetic result injection: never confirm/export the user's credentials.
                login.phase = Phase::Completing;
                login.task = Some(Task::ready(Ok(Reply::Imported)));
                assert!(app.poll_ssh_login(&mut remote).await);
                assert!(app.remote_login.is_none());
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
                    assert_eq!(value["type"], expected);
                    if expected == "notify_auth_changed" {
                        assert_eq!(value["provider"], provider);
                    }
                }
                assert!(
                    app.display_messages()
                        .last()
                        .unwrap()
                        .content
                        .contains(&format!(
                            "SSH login: {provider} imported on the remote host."
                        ))
                );
            }
        });
    });
}

#[test]
fn ssh_import_failure_or_cancel_never_enters_oauth_retry_or_cleanup() {
    with_app(|app| {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            for cancelled in [false, true] {
                app.handle_ssh_login_command("/login --import-local openai");
                let login = app.remote_login.as_mut().unwrap();
                login.phase = if cancelled {
                    Phase::Cancelling
                } else {
                    Phase::Completing
                };
                login.task = Some(Task::ready(Err("static import failure")));
                assert!(app.poll_ssh_login(&mut remote).await);
                assert!(app.remote_login.is_none());
                assert!(app.pending_login.is_none());
                let message = &app.display_messages().last().unwrap().content;
                assert!(!message.contains("Paste a fresh completion"));
                assert!(!message.contains("Pending authorization was removed"));
                assert!(message.contains(if cancelled {
                    "stopped locally"
                } else {
                    "No automatic retry or sync"
                }));
            }
        });
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
fn ssh_login_bare_picker_cancel_keys_show_persistent_feedback() {
    with_app(|app| {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            for command in ["/login", "/cancel"] {
                for ch in command.chars() {
                    app.handle_remote_key(KeyCode::Char(ch), KeyModifiers::NONE, &mut remote)
                        .await
                        .unwrap();
                }
                app.handle_remote_key(KeyCode::Enter, KeyModifiers::NONE, &mut remote)
                    .await
                    .unwrap();
                if command == "/login" {
                    assert!(app.remote_login.is_some());
                }
            }
            assert!(app.remote_login.is_none());
            assert!(app.pending_login.is_none());
            assert!(!app.should_quit);
            assert!(!app.pending_turn);
            assert!(app.queued_messages.is_empty());
            assert!(app.input.is_empty());
            let message = app.display_messages().last().unwrap();
            assert_eq!(message.role, "system");
            assert_eq!(
                message.content,
                "SSH login cancelled. No authorization was started."
            );
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
            login.operation = Some(Operation::Callback);
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
            login.operation = Some(Operation::Callback);
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

#[test]
fn ssh_inline_import_selection_requires_consent_and_cancel_reads_nothing() {
    with_app(|app| {
        for (row, provider) in [(0, "openai"), (1, "claude")] {
            app.handle_ssh_login_command("/login");
            for _ in 0..row {
                app.handle_ssh_login_key(KeyCode::Down, KeyModifiers::NONE, None);
            }
            app.handle_ssh_login_key(KeyCode::Enter, KeyModifiers::NONE, None);
            let login = app.remote_login.as_ref().unwrap();
            assert_eq!(login.provider, provider);
            assert!(login.phase == Phase::ImportConsent);
            assert!(login.task.is_none());
            assert!(app.inline_interactive_state.is_none());
            assert!(
                app.display_messages()
                    .last()
                    .unwrap()
                    .content
                    .contains("Type exactly confirm")
            );
            app.handle_ssh_login_key(KeyCode::Esc, KeyModifiers::NONE, None);
            assert!(app.remote_login.is_none());
        }
    });
}

#[test]
fn ssh_inline_import_short_command_lists_only_supported_imports() {
    with_app(|app| {
        app.handle_ssh_login_command("/login --import-local");
        let picker = app.inline_interactive_state.as_ref().unwrap();
        assert_eq!(picker.entries.len(), 2);
        assert!(picker.entries.iter().all(|entry| matches!(
            entry.action,
            crate::tui::PickerAction::RemoteLogin { import: true, .. }
        )));
        app.cancel_ssh_login();
    });
}

#[test]
fn ssh_inline_picker_filter_and_escape_match_local_navigation() {
    with_app(|app| {
        app.handle_ssh_login_command("/login");
        for c in "claude".chars() {
            app.handle_ssh_login_key(KeyCode::Char(c), KeyModifiers::NONE, None);
        }
        let picker = app.inline_interactive_state.as_ref().unwrap();
        assert_eq!(picker.filter, "claude");
        assert_eq!(picker.filtered.len(), 2);
        app.handle_ssh_login_key(KeyCode::Esc, KeyModifiers::NONE, None);
        assert!(
            app.inline_interactive_state
                .as_ref()
                .unwrap()
                .filter
                .is_empty()
        );
        app.handle_ssh_login_key(KeyCode::Esc, KeyModifiers::NONE, None);
        assert!(app.remote_login.is_none());
    });
}

#[test]
fn ssh_inline_picker_status_is_remote_only_and_failure_stays_unknown() {
    with_app(|app| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            app.handle_ssh_login_command("/login");
            app.inline_interactive_state.as_mut().unwrap().selected = 3;
            app.remote_login.as_mut().unwrap().task = Some(Task::ready(Ok(Reply::Status {
                providers: vec![
                    command::ProviderStatus {
                        id: "openai".into(),
                        state: crate::auth::AuthState::Available,
                        method_detail: "OAuth".into(),
                    },
                    command::ProviderStatus {
                        id: "claude".into(),
                        state: crate::auth::AuthState::Expired,
                        method_detail: "OAuth (expired)".into(),
                    },
                ],
            })));
            assert!(app.poll_ssh_login(&mut remote).await);
            let picker = app.inline_interactive_state.as_ref().unwrap();
            assert_eq!(picker.selected, 3);
            assert!(picker.entries[2].is_current);
            assert_eq!(picker.entries[2].options[0].api_method, "configured");
            assert_eq!(picker.entries[3].options[0].api_method, "attention");
            assert_eq!(picker.entries[4].options[0].api_method, "status unknown");
            assert!(!picker.entries[0].is_current);
            app.remote_login.as_mut().unwrap().task = Some(Task::ready(Err("status unavailable")));
            assert!(app.poll_ssh_login(&mut remote).await);
            let picker = app.inline_interactive_state.as_ref().unwrap();
            assert_eq!(picker.entries[2].options[0].api_method, "status unknown");
            assert!(!picker.entries[2].is_current);
            assert!(app.remote_login.as_ref().unwrap().phase == Phase::Choosing);
            app.cancel_ssh_login();
            assert!(app.remote_login.is_none());
        });
    });
}

#[test]
fn ssh_picker_paste_selects_visible_import_action_not_browser_login() {
    with_app(|app| {
        for command in ["/login", "/login --import-local"] {
            for provider in ["openai", "claude"] {
                app.handle_ssh_login_command(command);
                let query = if command == "/login" {
                    format!("import local {provider}")
                } else {
                    provider.to_string()
                };
                app.handle_paste(query.clone());
                assert!(app.remote_login.as_ref().unwrap().input.is_empty());
                assert!(app.input.is_empty());
                assert!(app.pasted_contents.is_empty());
                assert_eq!(
                    app.inline_interactive_state.as_ref().unwrap().filter,
                    query
                );
                app.handle_ssh_login_key(KeyCode::Enter, KeyModifiers::NONE, None);
                let login = app.remote_login.as_ref().unwrap();
                assert!(login.phase == Phase::ImportConsent);
                assert!(login.operation == Some(Operation::Import));
                assert_eq!(login.provider, provider);
                assert!(login.task.is_none());
                app.cancel_ssh_login();
            }
        }
    });
}
