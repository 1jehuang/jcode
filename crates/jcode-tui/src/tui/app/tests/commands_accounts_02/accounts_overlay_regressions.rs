// Inspect emitted actions without starting OAuth or changing saved credentials.
fn assert_accounts_overlay_actions(app: &mut App, providers: &[&str]) {
    use crate::tui::account_picker::{AccountPickerCommand, AccountProviderKind, OverlayAction};
    assert!(app.inline_interactive_state.is_none());
    assert!(app.account_picker_overlay.is_some());
    let _render_lock = crate::tui::ui::render_state_test_lock();
    for provider_id in providers {
        let labels: Vec<String> = match *provider_id {
            "claude" => crate::auth::claude::list_accounts()
                .unwrap()
                .into_iter()
                .map(|a| a.label)
                .collect(),
            "openai" => crate::auth::codex::list_accounts()
                .unwrap()
                .into_iter()
                .map(|a| a.label)
                .collect(),
            _ => unreachable!(),
        };
        assert!(!labels.is_empty());
        app.open_account_center(Some(provider_id));
        let profile = app
            .account_picker_overlay
            .as_ref()
            .unwrap()
            .borrow()
            .debug_memory_profile();
        assert_eq!(profile["view"], "accounts");
        let rows = profile["visible_items"].as_array().unwrap();
        assert_eq!(
            rows.iter()
                .filter(|row| row["command"].as_str().unwrap().starts_with("Switch {"))
                .count(),
            labels.len()
        );
        assert!(rows.iter().all(
            |row| !row["command"].as_str().unwrap().starts_with("Rename {")
                && !row["command"].as_str().unwrap().starts_with("Login {")
                && !row["command"].as_str().unwrap().starts_with("Remove {")
        ));
        for label in &labels {
            for title in [
                "Use",
                "Rename account",
                "Re-login account",
                "Replace login",
                "Remove account",
                "Make default",
            ] {
                app.open_account_center(Some(provider_id));
                {
                    let mut picker = app.account_picker_overlay.as_ref().unwrap().borrow_mut();
                    picker.select_account(provider_id, Some(label));
                    assert!(matches!(
                        picker
                            .handle_overlay_key(KeyCode::Enter, KeyModifiers::empty())
                            .unwrap(),
                        OverlayAction::Continue
                    ));
                    let profile = picker.debug_memory_profile();
                    assert_eq!(profile["view"], "account");
                    assert_eq!(profile["filtered_count"], 6);
                    if title == "Use" {
                        picker.select_account(provider_id, Some(label));
                    } else {
                        for c in title.chars() {
                            picker
                                .handle_overlay_key(KeyCode::Char(c), KeyModifiers::empty())
                                .unwrap();
                        }
                        assert_eq!(
                            picker.debug_memory_profile()["filtered_count"],
                            1,
                            "{title}"
                        );
                    }
                }
                let mut terminal =
                    ratatui::Terminal::new(ratatui::backend::TestBackend::new(240, 50)).unwrap();
                terminal
                    .draw(|frame| crate::tui::ui::draw(frame, app))
                    .unwrap();
                let text = buffer_to_text(&terminal);
                assert!(text.contains(title), "missing {title}:\n{text}");
                let mut picker = app.account_picker_overlay.as_ref().unwrap().borrow_mut();
                let mut action = picker
                    .handle_overlay_key(KeyCode::Enter, KeyModifiers::empty())
                    .unwrap();
                if matches!(title, "Remove account" | "Replace login") {
                    assert!(matches!(action, OverlayAction::Continue));
                    action = picker
                        .handle_overlay_key(KeyCode::Char('y'), KeyModifiers::empty())
                        .unwrap();
                }
                let OverlayAction::Execute(command) = action else {
                    panic!("expected {title}")
                };
                let (actual_provider, actual_label) = match (title, command) {
                    ("Use", AccountPickerCommand::Switch { provider, label })
                    | ("Rename account", AccountPickerCommand::Rename { provider, label })
                    | ("Re-login account", AccountPickerCommand::Login { provider, label })
                    | ("Replace login", AccountPickerCommand::Replace { provider, label })
                    | ("Remove account", AccountPickerCommand::Remove { provider, label })
                    | ("Make default", AccountPickerCommand::SetDefault { provider, label }) => {
                        (provider, label)
                    }
                    _ => panic!("wrong typed action for {title}"),
                };
                assert_eq!(&actual_label, label);
                assert!(matches!(
                    (*provider_id, actual_provider),
                    ("claude", AccountProviderKind::Anthropic)
                        | ("openai", AccountProviderKind::OpenAi)
                ));
            }
        }
        app.open_account_add_replace_flow(Some(provider_id));
        let mut picker = app.account_picker_overlay.as_ref().unwrap().borrow_mut();
        assert_eq!(picker.debug_memory_profile()["filtered_count"], 2);
        for c in "Add new account".chars() {
            picker
                .handle_overlay_key(KeyCode::Char(c), KeyModifiers::empty())
                .unwrap();
        }
        assert!(
            matches!(picker.handle_overlay_key(KeyCode::Enter,KeyModifiers::empty()).unwrap(), OverlayAction::Execute(AccountPickerCommand::SubmitInput(command)) if command == format!("/accounts {provider_id} add"))
        );
    }
}

#[test]
fn test_accounts_four_anthropic_accounts_have_separate_scoped_actions() {
    with_temp_jcode_home(|| {
        store_two_claude_accounts();
        for n in 3..=4 {
            crate::auth::claude::upsert_account(crate::auth::claude::AnthropicAccount {
                label: format!("work {n}"),
                access: format!("access-{n}"),
                refresh: format!("refresh-{n}"),
                expires: chrono::Utc::now().timestamp_millis() + 60_000,
                email: Some(format!("work{n}@example.invalid")),
                scopes: Vec::new(),
                subscription_type: Some("max".into()),
            })
            .unwrap();
        }
        assert_eq!(crate::auth::claude::list_accounts().unwrap().len(), 4);
        let mut app = create_test_app();
        app.input = "/accounts claude".into();
        app.submit_input();
        assert_accounts_overlay_actions(&mut app, &["claude"]);
        assert_eq!(crate::auth::claude::list_accounts().unwrap().len(), 4);
    });
}

#[test]
fn test_accounts_claude_command_opens_full_overlay() {
    with_temp_jcode_home(|| {
        store_two_claude_accounts();
        let mut app = create_test_app();
        app.input = "/accounts claude".into();
        app.submit_input();
        assert_accounts_overlay_actions(&mut app, &["claude"]);
    });
}

#[test]
fn test_accounts_typing_does_not_choose_first_login_on_enter() {
    with_temp_jcode_home(|| {
        let (first, _) = store_two_claude_accounts();
        let openai = crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
            label: String::new(),
            access_token: "synthetic-access".into(),
            refresh_token: "synthetic-refresh".into(),
            id_token: None,
            account_id: Some("keyboard-account".into()),
            expires_at: None,
            email: Some("keyboard@example.invalid".into()),
        })
        .unwrap();
        for command in [
            "/accounts",
            "/accounts ",
            "/accounts openai",
            "/accounts openai ",
            "/accounts claude",
        ] {
            let mut app = create_test_app();
            for character in command.chars() {
                app.handle_key(KeyCode::Char(character), KeyModifiers::empty())
                    .unwrap();
            }
            assert!(
                app.inline_interactive_state.is_none(),
                "unexpected preview for {command:?}"
            );
            assert!(app.account_picker_overlay.is_none());
            assert_eq!(app.input(), command);
            assert!(app.pending_login.is_none());
            app.handle_key(KeyCode::Enter, KeyModifiers::empty())
                .unwrap();
            assert!(
                app.account_picker_overlay.is_some(),
                "Enter must open center for {command:?}"
            );
            assert!(app.inline_interactive_state.is_none());
            assert!(app.pending_login.is_none());
            assert!(
                app.pending_account_input.is_none(),
                "Enter must not start Add for {command:?}"
            );
            assert!(!app.session.account_pins.contains_key("claude"));
            assert!(!app.session.account_pins.contains_key("openai"));
            assert_eq!(
                crate::auth::claude::active_account_label().as_deref(),
                Some(first.as_str())
            );
            assert_eq!(
                crate::auth::codex::active_account_label().as_deref(),
                Some(openai.as_str())
            );
            let providers: &[&str] = if command.contains("openai") {
                &["openai"]
            } else if command.contains("claude") {
                &["claude"]
            } else {
                &["claude", "openai"]
            };
            // Require the full center, not merely an Add/reauth overlay.
            assert_accounts_overlay_actions(&mut app, providers);
        }
    });
}

#[test]
fn test_accounts_suggestions_and_registry_use_unique_plural_command() {
    let names: Vec<_> = super::state_ui_input_helpers::registered_command_names().collect();
    let unique: std::collections::HashSet<_> = names.iter().copied().collect();
    assert_eq!(
        names.len(),
        unique.len(),
        "duplicate registered slash commands"
    );
    assert_eq!(names.iter().filter(|name| **name == "/accounts").count(), 1);
    assert!(!names.contains(&"/account"));
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.set_input_for_test("/acc");
        let suggestions = app.command_suggestions();
        assert!(
            suggestions
                .iter()
                .any(|(command, _)| command == "/accounts")
        );
        assert!(
            !suggestions
                .iter()
                .any(|(command, _)| command.split_whitespace().next() == Some("/account"))
        );
    });
}

#[test]
fn test_accounts_rename_prompt_cancel_empty_and_spaced_name() {
    use crate::tui::account_picker::{AccountPickerCommand, AccountProviderKind};
    with_temp_jcode_home(|| {
        let (original, other) = store_two_claude_accounts();
        let mut app = create_test_app();
        let prompt = |app: &mut App, label: &str| {
            app.handle_account_picker_command(AccountPickerCommand::Rename {
                provider: AccountProviderKind::Anthropic,
                label: label.into(),
            });
            assert!(matches!(&app.pending_account_input,
                Some(super::auth::PendingAccountInput::Rename { provider_id, label: target })
                    if provider_id == "claude" && target == label));
        };
        prompt(&mut app, &original);
        app.input = "/cancel".into();
        app.submit_input();
        assert!(app.pending_account_input.is_none());
        assert!(
            crate::auth::claude::list_accounts()
                .unwrap()
                .iter()
                .any(|a| a.label == original)
        );
        prompt(&mut app, &original);
        app.input = "   ".into();
        app.submit_input();
        assert!(matches!(&app.pending_account_input,
            Some(super::auth::PendingAccountInput::Rename { label, .. }) if label == &original));
        app.input = "Personal  Research Team".into();
        app.submit_input();
        assert!(app.pending_account_input.is_none());
        let accounts = crate::auth::claude::list_accounts().unwrap();
        assert_eq!(accounts.len(), 2);
        assert!(accounts.iter().any(|a| a.label == "Personal  Research Team"
            && a.email.as_deref() == Some("first@example.com")));
        assert!(accounts.iter().any(|a| a.label == other));
        assert!(!accounts.iter().any(|a| a.label == original));
        // A typed action must preserve an already-renamed target containing spaces.
        prompt(&mut app, "Personal  Research Team");
        app.input = "Final  Account Name".into();
        app.submit_input();
        assert!(app.pending_account_input.is_none());
        assert!(
            crate::auth::claude::list_accounts()
                .unwrap()
                .iter()
                .any(|a| a.label == "Final  Account Name")
        );
        assert!(app.pending_login.is_none());
        app.input = "/accounts claude".into();
        app.submit_input();
        assert_accounts_overlay_actions(&mut app, &["claude"]);
    });
}

#[test]
fn test_accounts_direct_rename_preserves_spaces_and_uses_longest_saved_label() {
    for prefix in ["/accounts rename", "/accounts claude rename"] {
        with_temp_jcode_home(|| {
            let (first, second) = store_two_claude_accounts();
            crate::auth::claude::rename_account(&first, "Work").unwrap();
            crate::auth::claude::rename_account(&second, "Work  Research Team").unwrap();
            let mut app = create_test_app();
            app.input = format!("{prefix} Work  Research Team New  Personal Name");
            app.submit_input();

            assert!(app.pending_account_input.is_none());
            assert!(app.pending_login.is_none());
            let accounts = crate::auth::claude::list_accounts().unwrap();
            assert_eq!(accounts.len(), 2);
            assert!(accounts.iter().any(|account| account.label == "Work"
                && account.email.as_deref() == Some("first@example.com")));
            assert!(
                accounts
                    .iter()
                    .any(|account| account.label == "New  Personal Name"
                        && account.email.as_deref() == Some("second@example.com"))
            );
            assert!(
                !accounts
                    .iter()
                    .any(|account| account.label == "Work  Research Team")
            );
        });
    }
}

#[test]
fn test_accounts_remote_rename_prompt_never_submits_a_model_message() {
    use crate::tui::account_picker::{AccountPickerCommand, AccountProviderKind};
    use tokio::io::AsyncBufReadExt;

    for (value, enter_modifiers) in [
        ("Remote  Research Team", KeyModifiers::empty()),
        ("Remote  Research Team", KeyModifiers::CONTROL),
        ("/cancel", KeyModifiers::empty()),
        ("", KeyModifiers::empty()),
        ("   ", KeyModifiers::empty()),
    ] {
        with_temp_jcode_home(|| {
            let (original, other) = store_two_claude_accounts();
            crate::auth::claude::set_active_account(&other).unwrap();
            let mut app = create_test_app();
            app.is_remote = true;
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let mut connection = crate::tui::backend::RemoteConnection::dummy();
                let peer = connection.take_dummy_peer().expect("real socketpair peer");
                let mut reader = tokio::io::BufReader::new(peer);
                app.handle_account_picker_command(AccountPickerCommand::Rename {
                    provider: AccountProviderKind::Anthropic,
                    label: original.clone(),
                });
                assert!(matches!(&app.pending_account_input,
                    Some(super::auth::PendingAccountInput::Rename { label, .. }) if label == &original));
                let provider_messages_before = app.materialized_provider_messages().len();
                let session_messages_before = app.session.messages.len();
                let user_messages_before = app.display_messages().iter()
                    .filter(|message| message.role == "user").count();
                let prompt_history_before = app.persisted_prompt_history.clone();
                let request_id_before = connection.next_request_id_for_test();

                for character in value.chars() {
                    remote::handle_remote_key(&mut app, KeyCode::Char(character), KeyModifiers::empty(), &mut connection)
                        .await.unwrap();
                }
                remote::handle_remote_key(&mut app, KeyCode::Enter, enter_modifiers, &mut connection)
                    .await.unwrap();

                assert!(!app.is_processing(), "prompt value started model processing: {value:?}");
                assert_eq!(app.materialized_provider_messages().len(), provider_messages_before);
                assert_eq!(app.session.messages.len(), session_messages_before);
                assert_eq!(app.display_messages().iter().filter(|message| message.role == "user").count(), user_messages_before);
                assert_eq!(app.persisted_prompt_history, prompt_history_before);
                assert_eq!(connection.next_request_id_for_test(), request_id_before);
                let mut line = String::new();
                assert!(tokio::time::timeout(std::time::Duration::from_millis(50), reader.read_line(&mut line))
                    .await.is_err(), "account input leaked a wire request: {line}");

                let accounts = crate::auth::claude::list_accounts().unwrap();
                assert_eq!(accounts.len(), 2);
                assert!(accounts.iter().any(|account| account.label == other));
                assert_eq!(crate::auth::claude::default_account_label().as_deref(), Some(other.as_str()));
                if value.trim().is_empty() {
                    assert!(matches!(&app.pending_account_input,
                        Some(super::auth::PendingAccountInput::Rename { label, .. }) if label == &original));
                    assert!(accounts.iter().any(|account| account.label == original));
                    assert!(app.display_messages().last().unwrap().content.contains("cannot be empty"));

                    let retry_label = "Remote Retry Account";
                    for character in retry_label.chars() {
                        remote::handle_remote_key(&mut app, KeyCode::Char(character), KeyModifiers::empty(), &mut connection)
                            .await.unwrap();
                    }
                    remote::handle_remote_key(&mut app, KeyCode::Enter, KeyModifiers::empty(), &mut connection)
                        .await.unwrap();
                    assert!(app.pending_account_input.is_none());
                    let accounts = crate::auth::claude::list_accounts().unwrap();
                    assert_eq!(accounts.len(), 2);
                    assert!(accounts.iter().any(|account| account.label == retry_label
                        && account.email.as_deref() == Some("first@example.com")));
                    assert!(!accounts.iter().any(|account| account.label == original));
                    assert!(accounts.iter().any(|account| account.label == other));
                    assert_eq!(crate::auth::claude::default_account_label().as_deref(), Some(other.as_str()));
                    assert!(!app.is_processing());
                    assert_eq!(app.materialized_provider_messages().len(), provider_messages_before);
                    assert_eq!(app.session.messages.len(), session_messages_before);
                    assert_eq!(app.display_messages().iter().filter(|message| message.role == "user").count(), user_messages_before);
                    assert_eq!(app.persisted_prompt_history, prompt_history_before);
                    assert_eq!(connection.next_request_id_for_test(), request_id_before);
                    assert!(tokio::time::timeout(std::time::Duration::from_millis(50), reader.read_line(&mut line))
                        .await.is_err(), "retry input leaked a wire request: {line}");
                } else if value == "/cancel" {
                    assert!(app.pending_account_input.is_none());
                    assert!(accounts.iter().any(|account| account.label == original));
                    assert!(app.display_messages().last().unwrap().content.contains("cancelled"));
                } else {
                    assert!(app.pending_account_input.is_none());
                    assert!(accounts.iter().any(|account| account.label == value
                        && account.email.as_deref() == Some("first@example.com")));
                    assert!(!accounts.iter().any(|account| account.label == original));
                }
            });
        });
    }
}
