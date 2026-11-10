fn remote_account_fixture(label: &str) -> crate::protocol::AccountUsageSnapshot {
    crate::protocol::AccountUsageSnapshot {
        additional_windows: Vec::new(),
        label: label.into(),
        email: None,
        exhausted: false,
        primary_label: None,
        five_hour_ratio: None,
        secondary_label: None,
        seven_day_ratio: None,
        resets_at: None,
        error: None,
        banked_resets: None,
        banked_expires_at: None,
        banked_next_at: None,
    }
}

#[test]
fn test_account_hotkeys_advance_from_pending_target_not_stale_header() {
    with_ssh_remote_test_home(|| {
        let mut app = create_test_app();
        app.is_remote = true;
        app.remote_provider_name = Some("openai".into());
        app.remote_provider_oauth_account_label = Some("first".into());
        crate::usage::set_remote_account_usage(
            Vec::new(),
            vec![
                remote_account_fixture("first"),
                remote_account_fixture("second"),
                remote_account_fixture("third"),
            ],
        );
        app.pending_account_switch_target = Some(crate::tui::AccountPickerAction::Switch {
            provider_id: "openai".into(),
            label: "second".into(),
        });
        for (forward, expected) in [(true, "third"), (false, "first")] {
            app.cycle_account(forward);
            assert!(matches!(app.pending_account_picker_action.take(),
                Some(crate::tui::AccountPickerAction::Switch { provider_id, label })
                    if provider_id == "openai" && label == expected
            ));
            assert_eq!(
                app.remote_provider_oauth_account_label.as_deref(),
                Some("first")
            );
        }
        crate::usage::clear_remote_account_usage();
    });
}

#[test]
fn test_account_hotkeys_connected_remote_queue_switch_and_preserve_draft() {
    with_ssh_remote_test_home(|| {
        let mut app = create_test_app();
        app.is_remote = true;
        app.remote_provider_name = Some("openai".into());
        app.remote_provider_oauth_account_label = Some("first".into());
        crate::usage::set_remote_account_usage(
            Vec::new(),
            vec![
                remote_account_fixture("first"),
                remote_account_fixture("second"),
            ],
        );
        let runtime = tokio::runtime::Runtime::new().unwrap();
        for key in ['j', 'k'] {
            app.input = "keep this draft".into();
            app.cursor_pos = 4;
            runtime.block_on(async {
                let mut remote = crate::tui::backend::RemoteConnection::dummy();
                app.handle_remote_key(
                    KeyCode::Char(key),
                    KeyModifiers::CONTROL | KeyModifiers::ALT,
                    &mut remote,
                )
                .await
                .unwrap();
            });
            assert_eq!(app.input, "keep this draft");
            assert_eq!(app.cursor_pos, 4);
            assert!(matches!(app.pending_account_picker_action.take(),
                Some(crate::tui::AccountPickerAction::Switch { provider_id, label })
                    if provider_id == "openai" && label == "second"
            ));
        }
        crate::usage::clear_remote_account_usage();
    });
}

#[test]
fn test_account_local_shared_daemon_retains_inventory_center_and_saved_defaults() {
    with_temp_jcode_home(|| {
        let _env_guard = EnvRestoreGuard::capture(["JCODE_SSH_REMOTE", "GROQ_API_KEY"]);
        crate::env::remove_var("JCODE_SSH_REMOTE");
        crate::env::set_var("GROQ_API_KEY", "shared-local-regression-key");
        crate::auth::AuthStatus::invalidate_cache();
        let mut app = create_test_app();
        app.is_remote = true;
        crate::usage::clear_remote_account_usage();
        app.open_account_picker(None);
        assert!(app.inline_interactive_state.as_ref().unwrap().entries.iter().any(|entry|
            matches!(entry.action, crate::tui::PickerAction::Login(provider) if provider.id == "groq")
        ));
        app.open_account_center(Some("openai"));
        assert!(app.account_picker_overlay.is_some());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            assert!(
                super::auth::handle_account_command_remote(
                    &mut app,
                    "/account default-model shared-local-model",
                    &mut remote
                )
                .await
                .unwrap()
            );
        });
        assert_eq!(
            crate::config::Config::load()
                .provider
                .default_model
                .as_deref(),
            Some("shared-local-model")
        );
        crate::auth::AuthStatus::invalidate_cache();
    });
}

#[test]
fn test_remote_account_picker_pending_empty_and_scoped_inventory() {
    with_ssh_remote_test_home(|| {
        let mut app = create_test_app();
        app.is_remote = true;
        crate::usage::clear_remote_account_usage();
        app.open_account_picker(None);
        assert!(app.inline_interactive_state.is_none());
        assert!(
            app.display_messages()
                .last()
                .unwrap()
                .content
                .contains("not arrived")
        );
        crate::usage::set_remote_account_usage(Vec::new(), Vec::new());
        app.open_account_center(None);
        assert!(app.inline_interactive_state.is_none());
        assert!(
            app.display_messages()
                .last()
                .unwrap()
                .content
                .contains("no named OAuth")
        );
        crate::usage::set_remote_account_usage(
            vec![remote_account_fixture("remote Claude")],
            vec![remote_account_fixture("remote OpenAI")],
        );
        app.open_account_picker(Some("openai"));
        let entries = &app.inline_interactive_state.as_ref().unwrap().entries;
        assert_eq!(entries.len(), 1);
        assert!(
            matches!(&entries[0].action, crate::tui::PickerAction::Account(
            crate::tui::AccountPickerAction::Switch { provider_id, label }
        ) if provider_id == "openai" && label == "remote OpenAI")
        );
        app.open_account_picker(None);
        assert_eq!(
            app.inline_interactive_state.as_ref().unwrap().entries.len(),
            2
        );
        app.open_account_picker(Some("copilot"));
        assert!(app.inline_interactive_state.is_none());
        assert!(app.account_picker_overlay.is_none());
        crate::usage::clear_remote_account_usage();
    });
}

#[test]
fn test_remote_account_ssh_picker_keyboard_queues_host_switch() {
    with_ssh_remote_test_home(|| {
        let mut app = create_test_app();
        app.is_remote = true;
        crate::usage::set_remote_account_usage(
            Vec::new(),
            vec![remote_account_fixture("ssh-only")],
        );
        app.input = "/accounts".into();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            app.handle_remote_key(KeyCode::Enter, KeyModifiers::NONE, &mut remote)
                .await
                .unwrap();
        });
        assert!(app.inline_interactive_state.is_some());
        app.handle_inline_interactive_key(KeyCode::Enter, KeyModifiers::NONE)
            .unwrap();
        assert!(matches!(app.pending_account_picker_action.take(),
            Some(crate::tui::AccountPickerAction::Switch { provider_id, label })
                if provider_id == "openai" && label == "ssh-only"
        ));
        // Priority keys must not save a laptop account pool for remote entries.
        app.open_account_picker(None);
        app.handle_inline_interactive_key(KeyCode::Char(']'), KeyModifiers::NONE)
            .unwrap();
        assert!(
            app.display_messages()
                .last()
                .unwrap()
                .content
                .contains("Nothing was changed on this computer")
        );
        crate::usage::clear_remote_account_usage();
    });
}

#[test]
fn test_remote_accounts_plural_preview_focuses_without_switching() {
    with_ssh_remote_test_home(|| {
        let mut app = create_test_app();
        app.is_remote = true;
        crate::usage::set_remote_account_usage(
            Vec::new(),
            vec![remote_account_fixture("remote-only")],
        );
        for command in ["/account doctor", "/account openai doctor"] {
            app.input = command.into();
            app.sync_model_picker_preview_from_input();
            assert!(app.inline_interactive_state.is_none());
        }
        for command in ["/account", "/accounts"] {
            app.input = command.into();
            app.sync_model_picker_preview_from_input();
            assert!(app.inline_interactive_state.as_ref().unwrap().preview);
            assert!(app.activate_picker_from_preview());
            assert!(app.pending_account_picker_action.is_none());
            assert!(!app.inline_interactive_state.as_ref().unwrap().preview);
        }
        crate::usage::clear_remote_account_usage();
    });
}

#[test]
fn test_remote_account_commands_send_wire_switches_and_host_management() {
    with_ssh_remote_test_home(|| {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut app = create_test_app();
        app.is_remote = true;
        // There is deliberately no matching local credential. Switching must
        // still reach the daemon instead of trying a laptop auth mutation.
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            let peer = remote.take_dummy_peer().unwrap();
            let (reader, _) = peer.into_split();
            let mut reader = BufReader::new(reader);
            for (provider, expected) in [
                ("claude", "set_session_account"),
                ("openai", "set_session_account"),
            ] {
                assert!(
                    super::auth::handle_account_command_remote(
                        &mut app,
                        &format!("/accounts {provider} switch remote-only"),
                        &mut remote,
                    )
                    .await
                    .unwrap()
                );
                let mut line = String::new();
                tokio::time::timeout(
                    std::time::Duration::from_secs(1),
                    reader.read_line(&mut line),
                )
                .await
                .unwrap()
                .unwrap();
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                assert_eq!(request["type"], expected);
                assert_eq!(request["label"], "remote-only");
            }
            for (command, args) in [
                ("/account doctor", vec!["doctor"]),
                ("/account openai settings", vec!["settings", "openai"]),
                (
                    "/account openai remove remote-only",
                    vec!["remove", "openai", "remote-only"],
                ),
                (
                    "/account default-model remote-model",
                    vec!["default-model", "remote-model"],
                ),
                (
                    "/account openai transport https",
                    vec!["openai", "transport", "https"],
                ),
            ] {
                assert!(
                    super::auth::handle_account_command_remote(&mut app, command, &mut remote)
                        .await
                        .unwrap()
                );
                let mut line = String::new();
                tokio::time::timeout(
                    std::time::Duration::from_secs(1),
                    reader.read_line(&mut line),
                )
                .await
                .unwrap()
                .unwrap();
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                assert_eq!(request["type"], "runtime_command", "{command}");
                assert_eq!(request["command"], "ssh_account", "{command}");
                assert_eq!(request["args"], serde_json::json!(args), "{command}");
                assert!(
                    app.account_picker_overlay.is_none(),
                    "no laptop management picker"
                );
            }
            assert!(
                tokio::time::timeout(
                    std::time::Duration::from_millis(30),
                    reader.read_line(&mut String::new())
                )
                .await
                .is_err()
            );
        });
    });
}

#[path = "part_01/usage_cards.rs"]
mod usage_cards;
#[test]
fn test_show_accounts_includes_masked_email_column() {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let accounts = vec![crate::auth::claude::AnthropicAccount {
        label: "work".to_string(),
        access: "acc".to_string(),
        refresh: "ref".to_string(),
        expires: now_ms + 60000,
        email: Some("user@example.com".to_string()),
        scopes: Vec::new(),
        subscription_type: Some("max".to_string()),
    }];

    let mut lines = vec!["**Anthropic Accounts:**\n".to_string()];
    lines.push("| Account | Email | Status | Subscription | Active |".to_string());
    lines.push("|---------|-------|--------|-------------|--------|".to_string());

    for account in &accounts {
        let status = if account.expires > now_ms {
            "✓ valid"
        } else {
            "⚠ expired"
        };
        let email = account
            .email
            .as_deref()
            .map(mask_email)
            .unwrap_or_else(|| "unknown".to_string());
        let sub = account.subscription_type.as_deref().unwrap_or("unknown");
        lines.push(format!(
            "| {} | {} | {} | {} | {} |",
            account.label, email, status, sub, "◉"
        ));
    }

    let output = lines.join("\n");
    assert!(output.contains("| Account | Email | Status | Subscription | Active |"));
    assert!(output.contains("u***r@example.com"));
}

#[test]
fn test_account_openai_command_opens_account_picker() {
    with_temp_jcode_home(|| {
        let now_ms = chrono::Utc::now().timestamp_millis();

        crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
            label: "work".to_string(),
            access_token: "acc".to_string(),
            refresh_token: "ref".to_string(),
            id_token: None,
            account_id: Some("acct_work".to_string()),
            expires_at: Some(now_ms + 60_000),
            email: Some("user@example.com".to_string()),
        })
        .unwrap();

        let mut app = create_test_app();
        app.input = "/accounts openai".to_string();
        app.submit_input();

        assert_accounts_overlay_actions(&mut app, &["openai"]);
    });
}

#[test]
fn test_account_command_opens_account_picker() {
    with_temp_jcode_home(|| {
        let now_ms = chrono::Utc::now().timestamp_millis();

        crate::auth::claude::upsert_account(crate::auth::claude::AnthropicAccount {
            label: "claude-1".to_string(),
            access: "claude_acc".to_string(),
            refresh: "claude_ref".to_string(),
            expires: now_ms + 60_000,
            email: Some("claude@example.com".to_string()),
            scopes: Vec::new(),
            subscription_type: Some("pro".to_string()),
        })
        .unwrap();

        crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
            label: "work".to_string(),
            access_token: "acc".to_string(),
            refresh_token: "ref".to_string(),
            id_token: None,
            account_id: Some("acct_work".to_string()),
            expires_at: Some(now_ms + 60_000),
            email: Some("user@example.com".to_string()),
        })
        .unwrap();

        let mut app = create_test_app();
        app.input = "/accounts".to_string();
        app.submit_input();

        assert_accounts_overlay_actions(&mut app, &["claude", "openai"]);
    });
}

#[test]
fn test_account_picker_arrows_navigate_and_bare_letters_search() {
    with_temp_jcode_home(|| {
        let now_ms = chrono::Utc::now().timestamp_millis();

        crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
            label: "first".to_string(),
            access_token: "acc1".to_string(),
            refresh_token: "ref1".to_string(),
            id_token: None,
            account_id: Some("acct_1".to_string()),
            expires_at: Some(now_ms + 60_000),
            email: Some("first@example.com".to_string()),
        })
        .unwrap();
        crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
            label: "second".to_string(),
            access_token: "acc2".to_string(),
            refresh_token: "ref2".to_string(),
            id_token: None,
            account_id: Some("acct_2".to_string()),
            expires_at: Some(now_ms + 60_000),
            email: Some("second@example.com".to_string()),
        })
        .unwrap();

        let mut app = create_test_app();
        app.open_account_picker(Some("openai"));

        let initial_selected = app
            .inline_interactive_state
            .as_ref()
            .expect("inline account picker should open")
            .selected;
        let picker = app.inline_interactive_state.as_ref().unwrap();
        assert!(
            picker
                .entries
                .iter()
                .any(|entry| entry.name == "OpenAI Otter · first@example.com · unknown plan")
        );
        assert!(
            picker
                .entries
                .iter()
                .any(|entry| entry.name == "OpenAI Fox · second@example.com · unknown plan")
        );

        app.handle_key(KeyCode::Down, KeyModifiers::empty())
            .unwrap();
        let after_arrow = app.inline_interactive_state.as_ref().unwrap().selected;
        assert_eq!(after_arrow, initial_selected + 1);

        app.handle_key(KeyCode::Char('j'), KeyModifiers::empty())
            .unwrap();
        assert_eq!(app.inline_interactive_state.as_ref().unwrap().filter, "j");

        app.handle_key(KeyCode::Char('k'), KeyModifiers::empty())
            .unwrap();
        assert_eq!(app.inline_interactive_state.as_ref().unwrap().filter, "jk");
    });
}

#[test]
fn test_account_picker_preview_from_input_filters_accounts() {
    with_temp_jcode_home(|| {
        let now_ms = chrono::Utc::now().timestamp_millis();

        crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
            label: "first".to_string(),
            access_token: "acc1".to_string(),
            refresh_token: "ref1".to_string(),
            id_token: None,
            account_id: Some("acct_1".to_string()),
            expires_at: Some(now_ms + 60_000),
            email: Some("first@example.com".to_string()),
        })
        .unwrap();
        crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
            label: "second".to_string(),
            access_token: "acc2".to_string(),
            refresh_token: "ref2".to_string(),
            id_token: None,
            account_id: Some("acct_2".to_string()),
            expires_at: Some(now_ms + 60_000),
            email: Some("second@example.com".to_string()),
        })
        .unwrap();

        let mut app = create_test_app();
        for c in "/accounts openai sec".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
                .unwrap();
        }

        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("account preview should open");
        assert!(picker.preview, "account picker should stay in preview mode");
        assert_eq!(picker.kind, crate::tui::PickerKind::Account);
        assert_eq!(picker.filter, "sec");
        assert!(app.account_picker_overlay.is_none());
        assert_eq!(app.input(), "/accounts openai sec");
    });
}

#[test]
fn test_account_picker_preview_stays_closed_for_explicit_subcommands() {
    let mut app = create_test_app();

    for c in "/accounts openai settings".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }

    assert!(app.inline_interactive_state.is_none());
    assert_eq!(app.input(), "/accounts openai settings");
}

#[test]
fn test_account_command_combines_claude_and_openai_accounts() {
    with_temp_jcode_home(|| {
        let now_ms = chrono::Utc::now().timestamp_millis();

        crate::auth::claude::upsert_account(crate::auth::claude::AnthropicAccount {
            label: "claude-1".to_string(),
            access: "claude_acc".to_string(),
            refresh: "claude_ref".to_string(),
            expires: now_ms + 60_000,
            email: Some("claude@example.com".to_string()),
            scopes: Vec::new(),
            subscription_type: Some("pro".to_string()),
        })
        .unwrap();
        crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
            label: "openai-1".to_string(),
            access_token: "acc".to_string(),
            refresh_token: "ref".to_string(),
            id_token: None,
            account_id: Some("acct_openai_1".to_string()),
            expires_at: Some(now_ms + 60_000),
            email: Some("openai@example.com".to_string()),
        })
        .unwrap();

        let mut app = create_test_app();
        app.input = "/accounts".to_string();
        app.submit_input();

        assert_accounts_overlay_actions(&mut app, &["claude", "openai"]);
    });
}

#[test]
fn test_account_picker_renders_each_oauth_email_in_combined_and_scoped_views() {
    use ratatui::{Terminal, backend::TestBackend};

    with_temp_jcode_home(|| {
        let expires = chrono::Utc::now().timestamp_millis() + 60_000;
        let claude_emails = ["claude.personal@example.com", "claude.work@example.com"];
        let openai_emails = ["openai.personal@example.com", "openai.work@example.com"];
        for (index, email) in claude_emails.iter().enumerate() {
            crate::auth::claude::upsert_account(crate::auth::claude::AnthropicAccount {
                label: format!("claude-{}", index + 1),
                access: "access".to_string(),
                refresh: "refresh".to_string(),
                expires,
                email: Some((*email).to_string()),
                scopes: Vec::new(),
                subscription_type: Some("pro".to_string()),
            })
            .unwrap();
        }
        for (index, email) in openai_emails.iter().enumerate() {
            crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
                label: format!("openai-{}", index + 1),
                access_token: "access".to_string(),
                refresh_token: "refresh".to_string(),
                id_token: None,
                account_id: Some(format!("acct_{index}")),
                expires_at: Some(expires),
                email: Some((*email).to_string()),
            })
            .unwrap();
        }
        let mut app = create_test_app();
        for scope in [None, Some("claude"), Some("openai")] {
            app.open_account_picker(scope);
            for preview in [false, true] {
                app.inline_interactive_state.as_mut().unwrap().preview = preview;
                let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
                terminal
                    .draw(|frame| crate::tui::ui::draw(frame, &app))
                    .unwrap();
                let rendered = buffer_to_text(&terminal);
                for email in claude_emails.iter().chain(openai_emails.iter()) {
                    let expected = scope.is_none() || email.starts_with(scope.unwrap());
                    assert_eq!(
                        rendered.contains(email),
                        expected,
                        "scope {scope:?}, preview {preview}: {rendered}"
                    );
                }
            }
        }
    });
}

#[cfg(unix)]
#[test]
fn test_account_command_uses_fast_auth_snapshot_without_running_cursor_status() {
    use std::os::unix::fs::PermissionsExt;

    with_temp_jcode_home(|| {
        let prev_cursor_cli_path = std::env::var_os("JCODE_CURSOR_CLI_PATH");
        let temp = tempfile::TempDir::new().expect("create temp dir");
        let marker = temp.path().join("cursor-status-ran");
        let script = temp.path().join("cursor-agent-mock");

        std::fs::write(
            &script,
            format!("#!/bin/sh\necho ran > \"{}\"\nexit 0\n", marker.display()),
        )
        .expect("write mock cursor agent");
        let mut permissions = std::fs::metadata(&script)
            .expect("stat mock cursor agent")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).expect("chmod mock cursor agent");

        let mut app = create_test_app();

        crate::env::set_var("JCODE_CURSOR_CLI_PATH", &script);
        crate::auth::AuthStatus::invalidate_cache();
        let _ = std::fs::remove_file(&marker);

        app.input = "/accounts".to_string();
        app.submit_input();

        assert!(app.account_picker_overlay.is_some());
        assert!(app.inline_interactive_state.is_none());
        assert!(
            !marker.exists(),
            "/accounts should not execute `cursor-agent status` on open"
        );

        match prev_cursor_cli_path {
            Some(value) => crate::env::set_var("JCODE_CURSOR_CLI_PATH", value),
            None => crate::env::remove_var("JCODE_CURSOR_CLI_PATH"),
        }
        crate::auth::AuthStatus::invalidate_cache();
    });
}

#[test]
fn test_account_switch_shorthand_switches_openai_account_by_label() {
    with_temp_jcode_home(|| {
        let now_ms = chrono::Utc::now().timestamp_millis();

        // `account_store::upsert_account` ignores the requested label for a new
        // account and assigns its own canonical one (`openai-<animal>`), so the
        // switch has to use the label it actually returns.
        let open_account = |account_id: &str, email: &str| crate::auth::codex::OpenAiAccount {
            label: String::new(),
            access_token: "acc".to_string(),
            refresh_token: "ref".to_string(),
            id_token: None,
            account_id: Some(account_id.to_string()),
            expires_at: Some(now_ms + 60_000),
            email: Some(email.to_string()),
        };

        // Two accounts, because a single account is auto-activated on insert:
        // switching to the account that is already active would pass even if the
        // `/accounts switch` command did nothing. The first insert stays active
        // and the switch below has to move it to the second.
        let first =
            crate::auth::codex::upsert_account(open_account("acct_first", "first@example.com"))
                .unwrap();
        let second =
            crate::auth::codex::upsert_account(open_account("acct_second", "second@example.com"))
                .unwrap();
        assert_ne!(first, second, "the two inserts must get distinct labels");
        assert_eq!(
            crate::auth::codex::active_account_label().as_deref(),
            Some(first.as_str()),
            "the first inserted account is the active one before the switch"
        );

        // Window account pins are independent of the stored default and model route.
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            app.input = format!("/accounts switch {second}");
            app.submit_input();

            // Per-window accounts: the switch pins THIS window only. The
            // stored default (used by new windows) stays on the first account.
            assert_eq!(
                crate::auth::codex::active_account_label().as_deref(),
                Some(first.as_str()),
                "/accounts switch must not change the default for other windows"
            );
            let window = app.window_account("openai").expect("window account");
            assert_eq!(window.label.as_deref(), Some(second.as_str()));
            assert!(window.pinned);
            assert_eq!(
                app.session
                    .account_pins
                    .get("openai")
                    .map(|pin| pin.label.as_str()),
                Some(second.as_str()),
                "the pin is persisted on the session"
            );
            let last = &app.display_messages().last().unwrap().content;
            assert!(
                last.contains(&format!("This window now uses {second}"))
                    && last.contains(&format!("still {first}")),
                "{last}"
            );

            app.input = format!("/accounts default {second}");
            app.submit_input();
            assert_eq!(
                crate::auth::codex::active_account_label().as_deref(),
                Some(second.as_str()),
                "/accounts default changes the stored default"
            );
        });
    });
}

#[test]
fn test_account_picker_prompt_new_openai_label_cancel_clears_prompt() {
    let mut app = create_test_app();
    app.prompt_new_account_label(crate::tui::account_picker::AccountProviderKind::OpenAi);

    assert!(matches!(
        app.pending_account_input,
        Some(super::auth::PendingAccountInput::NewAccountLabel { ref provider_id, .. }) if provider_id == "openai"
    ));

    app.input = "/cancel".to_string();
    app.submit_input();

    assert!(app.pending_account_input.is_none());
    assert!(app.pending_login.is_none());
}

#[test]
fn test_login_command_opens_inline_login_picker() {
    let mut app = create_test_app();
    app.input = "/login".to_string();
    app.submit_input();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("/login should open inline login picker");
    assert_eq!(picker.kind, crate::tui::PickerKind::Login);
    assert!(app.pending_login.is_none());
}

#[test]
fn test_account_openai_compatible_settings_renders_provider_settings() {
    let mut app = create_test_app();
    app.input = "/accounts openai-compatible settings".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing settings output");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("OpenAI-compatible"));
    assert!(msg.content.contains("API base"));
    assert!(msg.content.contains("default-model"));
}

#[test]
fn test_account_default_provider_command_saves_config() {
    let _guard = crate::storage::lock_test_env();
    let mut app = create_test_app();
    app.input = "/accounts default-provider openai".to_string();
    app.submit_input();

    let cfg = crate::config::Config::load();
    assert_eq!(cfg.provider.default_provider.as_deref(), Some("openai"));
}

#[test]
fn test_commands_alias_shows_help() {
    let mut app = create_test_app();
    app.input = "/commands".to_string();
    app.submit_input();

    assert!(
        app.help_scroll.is_some(),
        "/commands should open help overlay"
    );
}









#[test]
fn test_account_switch_short_form_runs_on_one_enter() {
    with_temp_jcode_home(|| {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let claude_account = |email: &str| crate::auth::claude::AnthropicAccount {
            label: String::new(),
            access: "acc".to_string(),
            refresh: "ref".to_string(),
            expires: now_ms + 60_000,
            email: Some(email.to_string()),
            subscription_type: Some("max".to_string()),
            scopes: Vec::new(),
        };
        let first = crate::auth::claude::upsert_account(claude_account("a@example.com")).unwrap();
        let second = crate::auth::claude::upsert_account(claude_account("b@example.com")).unwrap();
        assert_ne!(first, second);

        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();

        // The suggestion list offers `/accounts claude switch <label>`, a
        // different spelling of the same command. One Enter must run it.
        app.set_input_for_test(format!("/accounts switch {second}"));
        app.handle_key(KeyCode::Enter, KeyModifiers::empty())
            .unwrap();
        assert!(app.input.is_empty(), "command ran, input: {:?}", app.input);
        let window = app.window_account("claude").expect("window account");
        assert_eq!(window.label.as_deref(), Some(second.as_str()));
        assert!(window.pinned);

        // A partial command still completes on Enter instead of running.
        app.set_input_for_test("/acc");
        app.handle_key(KeyCode::Enter, KeyModifiers::empty())
            .unwrap();
        assert!(app.input.starts_with("/accounts"), "{:?}", app.input);
        assert_ne!(app.input, "/acc");
    });
}

#[test]
fn test_account_ux_overview_focuses_current_claude_code_instance() {
    with_temp_jcode_home(|| {
        let home = crate::storage::jcode_dir().unwrap();
        std::fs::write(
            home.join("config.toml"),
            format!(
                "[provider.claude_code]\ndefault_instance = \"personal\"\n\
                 [[provider.claude_code.instances]]\nid = \"personal\"\nhome = {:?}\n\
                 [[provider.claude_code.instances]]\nid = \"work\"\nhome = {:?}\n",
                home.join("personal").display().to_string(),
                home.join("work").display().to_string(),
            ),
        )
        .unwrap();
        crate::config::invalidate_config_cache();
        let mut app = create_test_app();
        app.is_remote = true;
        app.remote_provider_name = Some("Claude Code".to_string());
        app.replace_window_accounts(vec![crate::protocol::SessionAccountInfo {
            provider: "claude-code".to_string(),
            label: Some("work".to_string()),
            pinned: true,
            is_default: false,
        }]);

        app.open_account_picker(None);
        let state = app.inline_interactive_state.as_ref().unwrap();
        assert!(matches!(
            &state.selected_entry().unwrap().action,
            crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::Switch {
                provider_id, label,
            }) if provider_id == "claude-code" && label == "work"
        ));
        assert!(state.selected_entry().unwrap().is_current);
    });
}

#[test]
fn test_account_ux_overview_includes_implicit_current_claude_code_instance() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.is_remote = true;
        app.remote_provider_name = Some("claude-code".to_string());
        app.open_account_picker(None);
        let state = app.inline_interactive_state.as_ref().unwrap();
        assert!(matches!(
            &state.selected_entry().unwrap().action,
            crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::Switch {
                provider_id, label,
            }) if provider_id == "claude-code" && label == "default"
        ));
    });
}

#[test]
fn test_account_ux_center_does_not_duplicate_provider_settings() {
    with_temp_jcode_home(|| {
        crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
            label: "openai-otter".to_string(),
            access_token: "acc".into(),
            refresh_token: "ref".into(),
            id_token: None,
            account_id: None,
            expires_at: None,
            email: None,
        })
        .unwrap();
        let mut app = create_test_app();
        app.open_account_center(Some("openai"));
        let mut picker = app.account_picker_overlay.as_ref().unwrap().borrow_mut();
        for character in "Saved auth entry".chars() {
            picker
                .handle_overlay_key(KeyCode::Char(character), KeyModifiers::empty())
                .unwrap();
        }
        assert_eq!(picker.debug_memory_profile()["filtered_count"], 0);
        assert!(!matches!(
            picker
                .handle_overlay_key(KeyCode::Enter, KeyModifiers::empty())
                .unwrap(),
            crate::tui::account_picker::OverlayAction::Execute(_)
        ));
        picker
            .handle_overlay_key(KeyCode::Esc, KeyModifiers::empty())
            .unwrap();
        for character in "Provider settings".chars() {
            picker
                .handle_overlay_key(KeyCode::Char(character), KeyModifiers::empty())
                .unwrap();
        }
        assert_eq!(picker.debug_memory_profile()["filtered_count"], 1);
        assert!(matches!(
            picker
                .handle_overlay_key(KeyCode::Enter, KeyModifiers::empty())
                .unwrap(),
            crate::tui::account_picker::OverlayAction::Continue
        ));
        assert_eq!(picker.debug_memory_profile()["view"], "provider_settings");
        for character in "Usage & provider settings".chars() {
            picker
                .handle_overlay_key(KeyCode::Char(character), KeyModifiers::empty())
                .unwrap();
        }
        assert_eq!(picker.debug_memory_profile()["filtered_count"], 1);
        assert!(matches!(
            picker
                .handle_overlay_key(KeyCode::Enter, KeyModifiers::empty())
                .unwrap(),
            crate::tui::account_picker::OverlayAction::Execute(
                crate::tui::account_picker::AccountPickerCommand::SubmitInput(command)
            ) if command == "/accounts openai settings"
        ));
    });
}

#[test]
fn test_account_ux_add_and_refresh_render_exact_target_and_masked_identity() {
    with_temp_jcode_home(|| {
        let _render_lock = crate::tui::ui::render_state_test_lock();
        let (claude_label, _) = store_two_claude_accounts();
        let openai_label = crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
            label: "openai-otter".to_string(),
            access_token: "synthetic-access".into(),
            refresh_token: "synthetic-refresh".into(),
            id_token: None,
            account_id: None,
            expires_at: None,
            email: Some("otter@example.invalid".to_string()),
        })
        .unwrap();
        let mut app = create_test_app();
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(300, 50)).unwrap();

        for (provider, label, email, masked_email) in [
            (
                "claude",
                claude_label,
                "first@example.com",
                "f***t@example.com",
            ),
            (
                "openai",
                openai_label,
                "otter@example.invalid",
                "o***r@example.invalid",
            ),
        ] {
            for (filter, expected_detail, command) in [
                (
                    "Add new account".to_string(),
                    "Existing saved accounts are kept.".to_string(),
                    Some(format!("/accounts {provider} add")),
                ),
                (
                    format!("Re-login account `{label}`"),
                    masked_email.to_string(),
                    None,
                ),
            ] {
                if command.is_some() {
                    app.open_account_add_replace_flow(Some(provider));
                } else {
                    app.open_account_center(Some(provider));
                    let mut picker = app.account_picker_overlay.as_ref().unwrap().borrow_mut();
                    picker.select_account(provider, Some(&label));
                    assert!(matches!(
                        picker
                            .handle_overlay_key(KeyCode::Enter, KeyModifiers::empty())
                            .unwrap(),
                        crate::tui::account_picker::OverlayAction::Continue
                    ));
                }
                {
                    let mut picker = app.account_picker_overlay.as_ref().unwrap().borrow_mut();
                    for character in filter.chars() {
                        picker
                            .handle_overlay_key(KeyCode::Char(character), KeyModifiers::empty())
                            .unwrap();
                    }
                    assert_eq!(picker.debug_memory_profile()["filtered_count"], 1);
                }
                terminal
                    .draw(|frame| crate::tui::ui::draw(frame, &app))
                    .unwrap();
                let text = buffer_to_text(&terminal);
                assert!(text.contains(&filter), "action title missing:\n{text}");
                assert!(
                    text.contains(&expected_detail),
                    "action scope or exact masked identity missing:\n{text}"
                );
                assert!(!text.contains(email), "raw email exposed:\n{text}");
                // Inspect the emitted command without dispatching it or starting OAuth.
                let action = app
                    .account_picker_overlay
                    .as_ref()
                    .unwrap()
                    .borrow_mut()
                    .handle_overlay_key(KeyCode::Enter, KeyModifiers::empty())
                    .unwrap();
                use crate::tui::account_picker::{
                    AccountPickerCommand, AccountProviderKind, OverlayAction,
                };
                match (command, action) {
                    (
                        Some(expected),
                        OverlayAction::Execute(AccountPickerCommand::SubmitInput(actual)),
                    ) => {
                        assert_eq!(actual, expected);
                    }
                    (
                        None,
                        OverlayAction::Execute(AccountPickerCommand::Login {
                            provider: actual_provider,
                            label: actual_label,
                        }),
                    ) => {
                        assert_eq!(actual_label, label);
                        assert!(matches!(
                            (provider, actual_provider),
                            ("claude", AccountProviderKind::Anthropic)
                                | ("openai", AccountProviderKind::OpenAi)
                        ));
                    }
                    _ => panic!("unexpected account action"),
                }
            }
        }
        assert_eq!(crate::auth::claude::list_accounts().unwrap().len(), 2);
        assert_eq!(crate::auth::codex::list_accounts().unwrap().len(), 1);
    });
}

#[test]
fn test_account_ux_saved_account_actions_explain_window_scope() {
    with_temp_jcode_home(|| {
        store_two_claude_accounts();
        let app = create_test_app();
        let provider = crate::provider_catalog::login_providers()
            .iter()
            .find(|provider| provider.id == "claude")
            .copied()
            .unwrap();
        let mut items = Vec::new();
        app.append_anthropic_account_picker_items(&mut items, provider);
        let switches: Vec<_> = items
            .iter()
            .filter(|item| {
                matches!(
                    item.command,
                    crate::tui::account_picker::AccountPickerCommand::Switch { .. }
                )
            })
            .collect();
        assert_eq!(switches.len(), 2);
        assert!(
            switches
                .iter()
                .all(|item| item.title.contains("this window"))
        );
    });
}

#[test]
fn test_account_ux_single_filtered_account_stays_visible_in_full_frame() {
    with_temp_jcode_home(|| {
        let _render_lock = crate::tui::ui::render_state_test_lock();
        store_two_claude_accounts();
        let mut app = create_test_app();
        app.open_account_picker(Some("claude"));
        for character in "Claude Otter".chars() {
            app.handle_key(KeyCode::Char(character), KeyModifiers::empty())
                .unwrap();
        }
        let picker = app.inline_interactive_state.as_ref().unwrap();
        assert_eq!(picker.filtered.len(), 1);
        // The picker shows the login email next to the name (upstream 3b52c5430).
        assert_eq!(
            picker.selected_entry().unwrap().name,
            "Claude Otter · first@example.com · unknown plan"
        );

        let backend = ratatui::backend::TestBackend::new(80, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &app))
            .unwrap();
        let text = buffer_to_text(&terminal);
        assert!(text.contains("Ctrl+D"), "account hint missing:\n{text}");
        assert!(
            text.contains("▸ Claude Otter"),
            "selected account must stay visible alongside its hint:\n{text}"
        );
    });
}

#[test]
fn test_account_ux_empty_search_shows_no_matches_in_full_frame() {
    with_temp_jcode_home(|| {
        let _render_lock = crate::tui::ui::render_state_test_lock();
        store_two_claude_accounts();
        let mut app = create_test_app();
        app.open_account_picker(Some("claude"));
        for character in "zzzzzz".chars() {
            app.handle_key(KeyCode::Char(character), KeyModifiers::empty())
                .unwrap();
        }
        assert!(
            app.inline_interactive_state
                .as_ref()
                .unwrap()
                .filtered
                .is_empty()
        );
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &app))
            .unwrap();
        let text = buffer_to_text(&terminal);
        assert!(
            text.contains("no matches"),
            "empty-state message missing:\n{text}"
        );
        assert!(
            text.contains("Esc clear filter"),
            "recovery hint missing:\n{text}"
        );
    });
}

include!("accounts_overlay_regressions.rs");

mod persistent_accounts {
    include!("accounts_persistent_workflow.rs");
}
#[path = "part_01/improvement_commands.rs"]
mod improvement_commands;
include!("usage_details.rs");

#[test]
fn test_account_picker_marks_routed_session_login_not_global_last_account() {
    with_temp_jcode_home(|| {
        let mut labels = Vec::new();
        for label in ["personal", "work"] {
            labels.push(
                crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
                    label: label.into(),
                    access_token: "acc".into(),
                    refresh_token: "ref".into(),
                    id_token: None,
                    account_id: None,
                    expires_at: None,
                    email: None,
                })
                .unwrap(),
            );
        }
        crate::auth::codex::set_active_account(&labels[1]).unwrap();
        let mut app = create_test_app();
        app.is_remote = true;
        app.remote_provider_name = Some("OpenAI".into());
        app.remote_resolved_credential = Some(jcode_provider_core::ResolvedCredential::Oauth);
        app.remote_provider_oauth_account_label = Some(labels[0].clone());
        crate::usage::set_remote_account_usage(
            Vec::new(),
            labels
                .iter()
                .map(|label| remote_account_fixture(label))
                .collect(),
        );
        app.input = "/account openai".into();
        app.submit_input();
        let picker = app.inline_interactive_state.as_ref().unwrap();
        let active: Vec<_> = picker
            .entries
            .iter()
            .filter(|entry| entry.is_current)
            .filter_map(|entry| entry.account_pool_key())
            .collect();
        assert_eq!(active, [format!("openai:{}", labels[0])]);
        assert_eq!(
            crate::auth::codex::active_account_label().as_deref(),
            Some(labels[1].as_str())
        );
        crate::usage::clear_remote_account_usage();
    });
}


#[test]
fn test_account_picker_includes_all_configured_credentials_even_excluded_and_manual() {
    with_temp_jcode_home(|| {
        let _env_guard = EnvRestoreGuard::capture([
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "OPENROUTER_API_KEY",
            "COPILOT_GITHUB_TOKEN",
            "GROQ_API_KEY",
        ]);
        for key in ["OPENAI_API_KEY", "ANTHROPIC_API_KEY", "OPENROUTER_API_KEY"] {
            crate::env::set_var(key, "picker-test-secret-api-key");
        }
        crate::env::set_var("COPILOT_GITHUB_TOKEN", "picker-test-secret-device-token");
        crate::env::set_var("GROQ_API_KEY", "picker-test-secret-compatible-key");
        crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
            label: "work".into(),
            access_token: "picker-test-secret-oauth-access".into(),
            refresh_token: "picker-test-secret-oauth-refresh".into(),
            id_token: None,
            account_id: None,
            expires_at: Some(chrono::Utc::now().timestamp_millis() + 3_600_000),
            email: None,
        })
        .unwrap();
        crate::auth::google::save_tokens(&crate::auth::google::GoogleTokens {
credentials: Default::default(),
            access_token: "picker-test-secret-google-access".into(),
            refresh_token: "picker-test-secret-google-refresh".into(),
            expires_at: chrono::Utc::now().timestamp_millis() + 3_600_000,
            tier: crate::auth::google::GmailAccessTier::Full,
            email: None,
            services: vec![crate::auth::google::GoogleService::Gmail],
        })
        .unwrap();
        crate::auth::account_pool::AccountPool {
            members: vec![],
            excluded: vec![
                "openai-api".into(),
                "copilot".into(),
                "google".into(),
                "groq".into(),
            ],
            ..Default::default()
        }
        .save()
        .unwrap();
        crate::auth::AuthStatus::invalidate_cache();

        let mut app = create_test_app();
        app.input = "/account".to_string();
        app.submit_input();
        let picker = app.inline_interactive_state.as_ref().unwrap();
        for provider_id in [
            "openai-api",
            "anthropic-api",
            "openrouter",
            "copilot",
            "google",
            "groq",
        ] {
            let entry = picker
                .entries
                .iter()
                .find(|entry| {
                    matches!(
                        entry.action,
                        crate::tui::PickerAction::Login(provider) if provider.id == provider_id
                    )
                })
                .unwrap_or_else(|| panic!("missing configured {provider_id}"));
            assert_eq!(entry.options[0].api_method, "configured");
        }
        assert!(
            !picker
                .entries
                .iter()
                .any(|entry| entry.name == "account center")
        );
        assert!(!picker.entries.iter().any(|entry| matches!(entry.action,
            crate::tui::PickerAction::Login(provider) if provider.id == "auto-import"
        )));
        assert!(!format!("{:?}", picker.entries).contains("picker-test-secret"));
        let status = crate::auth::AuthStatus::check_fast();
        for entry in &picker.entries {
            if let crate::tui::PickerAction::Login(provider) = entry.action {
                assert!(
                    status.assessment_for_provider(provider).is_configured(),
                    "unconfigured provider {} should not be offered",
                    provider.id
                );
            }
        }
        let api_row = picker
            .entries
            .iter()
            .position(|entry| {
                matches!(entry.action,
                    crate::tui::PickerAction::Login(provider) if provider.id == "openai-api"
                )
            })
            .unwrap();
        let new_row = picker
            .entries
            .iter()
            .position(|entry| entry.name == "new Claude account")
            .unwrap();
        assert!(
            api_row < new_row,
            "credentials must precede management actions"
        );

        // A local shared-daemon client must retain its local credential
        // inventory, even though it uses the same wire client as SSH.
        app.is_remote = true;
        for (name, credential, expected) in [
            (
                "OpenAI",
                Some(jcode_provider_core::ResolvedCredential::ApiKey),
                "openai-api",
            ),
            ("Groq", None, "groq"),
        ] {
            app.remote_provider_name = Some(name.into());
            app.remote_resolved_credential = credential;
            app.open_account_picker(None);
            let picker = app.inline_interactive_state.as_ref().unwrap();
            let selected = &picker.entries[picker.selected];
            assert!(selected.is_current);
            assert!(matches!(selected.action,
                crate::tui::PickerAction::Login(provider) if provider.id == expected
            ));
        }
        app.is_remote = false;

        for (scope, expected, unexpected) in [
            ("claude", "anthropic-api", "openai-api"),
            ("openai", "openai-api", "anthropic-api"),
        ] {
            app.open_account_picker(Some(scope));
            let entries = &app.inline_interactive_state.as_ref().unwrap().entries;
            assert!(entries.iter().any(|entry| matches!(entry.action,
                crate::tui::PickerAction::Login(provider) if provider.id == expected
            )));
            assert!(!entries.iter().any(|entry| matches!(entry.action,
                crate::tui::PickerAction::Login(provider) if provider.id == unexpected
            )));
        }
    });
}


#[test]
fn test_account_priority_keys_persist_reset_and_preserve_membership() {
    with_temp_jcode_home(|| {
        let _env_guard = EnvRestoreGuard::capture(["OPENAI_API_KEY"]);
        crate::env::set_var("OPENAI_API_KEY", "test-priority-api-key");
        crate::auth::AuthStatus::invalidate_cache();
        let mut pool = crate::auth::account_pool::AccountPool {
            members: vec!["claude:work".into()],
            excluded: vec!["openai-api".into()],
            ..Default::default()
        };
        pool.set_priority("openai-api", 0);
        pool.save().unwrap();
        let mut app = create_test_app();
        app.open_account_picker(None);
        let picker = app.inline_interactive_state.as_mut().unwrap();
        let selected = picker
            .filtered
            .iter()
            .position(|&index| {
                picker.entries[index].account_pool_key().as_deref() == Some("openai-api")
            })
            .unwrap();
        picker.selected = selected;
        assert_eq!(
            picker.entries[picker.filtered[selected]].account_priority(),
            Some(0)
        );

        for (key, expected) in [(']', 1), ('[', 0), ('[', 0)] {
            app.handle_inline_interactive_key(KeyCode::Char(key), KeyModifiers::NONE)
                .unwrap();
            let saved = crate::auth::account_pool::AccountPool::load();
            assert_eq!(saved.priority("openai-api"), expected);
            assert_eq!(saved.members, pool.members);
            assert_eq!(saved.excluded, pool.excluded);
            let picker = app.inline_interactive_state.as_ref().unwrap();
            assert_eq!(picker.selected, selected);
            assert_eq!(
                picker.entries[picker.filtered[selected]].account_priority(),
                Some(expected)
            );
        }
        pool.set_priority("openai-api", u32::MAX);
        pool.save().unwrap();
        app.handle_inline_interactive_key(KeyCode::Char(']'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(
            crate::auth::account_pool::AccountPool::load().priority("openai-api"),
            u32::MAX
        );
        app.handle_inline_interactive_key(KeyCode::Char('r'), KeyModifiers::CONTROL)
            .unwrap();
        let saved = crate::auth::account_pool::AccountPool::load();
        assert!(!saved.priorities.contains_key("openai-api"));
        assert_eq!(
            saved.priority("openai-api"),
            crate::auth::account_pool::AccountPool::default().priority("openai-api")
        );
        assert_eq!(saved.members, pool.members);
        assert_eq!(saved.excluded, pool.excluded);

        app.open_account_picker(None);
        let picker = app.inline_interactive_state.as_ref().unwrap();
        let entry = picker
            .entries
            .iter()
            .find(|entry| entry.account_pool_key().as_deref() == Some("openai-api"))
            .unwrap();
        assert_eq!(entry.account_priority(), Some(saved.priority("openai-api")));

        // Address the filtered credential, not the unfiltered row index.
        let picker = app.inline_interactive_state.as_mut().unwrap();
        let index = picker
            .entries
            .iter()
            .position(|entry| entry.account_pool_key().as_deref() == Some("openai-api"))
            .unwrap();
        picker.filtered = vec![index];
        picker.selected = 0;
        picker.filter = "OpenAI".into();
        app.handle_inline_interactive_key(KeyCode::Char(']'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(
            app.inline_interactive_state.as_ref().unwrap().filter,
            "OpenAI"
        );
        assert_eq!(
            crate::auth::account_pool::AccountPool::load().priority("openai-api"),
            saved.priority("openai-api") + 1
        );

        app.open_account_picker(None);
        let picker = app.inline_interactive_state.as_mut().unwrap();
        picker.selected = picker
            .filtered
            .iter()
            .position(|&index| picker.entries[index].account_pool_key().is_none())
            .unwrap();
        let before = crate::auth::account_pool::AccountPool::load();
        app.handle_inline_interactive_key(KeyCode::Char(']'), KeyModifiers::NONE)
            .unwrap();
        assert_eq!(crate::auth::account_pool::AccountPool::load(), before);
    });
}


#[test]
fn test_account_picker_supports_arrow_and_vim_navigation() {
    with_temp_jcode_home(|| {
        let now_ms = chrono::Utc::now().timestamp_millis();

        crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
            label: "first".to_string(),
            access_token: "acc1".to_string(),
            refresh_token: "ref1".to_string(),
            id_token: None,
            account_id: Some("acct_1".to_string()),
            expires_at: Some(now_ms + 60_000),
            email: Some("first@example.com".to_string()),
        })
        .unwrap();
        crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
            label: "second".to_string(),
            access_token: "acc2".to_string(),
            refresh_token: "ref2".to_string(),
            id_token: None,
            account_id: Some("acct_2".to_string()),
            expires_at: Some(now_ms + 60_000),
            email: Some("second@example.com".to_string()),
        })
        .unwrap();

        let mut app = create_test_app();
        app.input = "/account openai".to_string();
        app.submit_input();

        let initial_selected = app
            .inline_interactive_state
            .as_ref()
            .expect("inline account picker should open")
            .selected;
        let picker = app.inline_interactive_state.as_ref().unwrap();
        assert!(
            picker
                .entries
                .iter()
                .any(|entry| entry.name == "OpenAI Otter · first@example.com · unknown plan")
        );
        assert!(
            picker
                .entries
                .iter()
                .any(|entry| entry.name == "OpenAI Fox · second@example.com · unknown plan")
        );

        app.handle_key(KeyCode::Down, KeyModifiers::empty())
            .unwrap();
        let after_arrow = app.inline_interactive_state.as_ref().unwrap().selected;
        assert_eq!(after_arrow, initial_selected + 1);

        app.handle_key(KeyCode::Char('j'), KeyModifiers::CONTROL)
            .unwrap();
        let after_vim = app.inline_interactive_state.as_ref().unwrap().selected;
        assert_eq!(after_vim, after_arrow + 1);

        app.handle_key(KeyCode::Char('k'), KeyModifiers::CONTROL)
            .unwrap();
        assert_eq!(
            app.inline_interactive_state.as_ref().unwrap().selected,
            after_arrow
        );
    });
}
