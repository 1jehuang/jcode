use super::*;
use crate::tui::app::auth::{AccountCommand, execute_account_command_remote};

struct AccountContextHome(Option<std::ffi::OsString>);

impl Drop for AccountContextHome {
    fn drop(&mut self) {
        if let Some(home) = self.0.take() {
            crate::env::set_var("JCODE_HOME", home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
        crate::auth::claude::set_active_account_override(None);
        crate::auth::codex::set_active_account_override(None);
        crate::auth::AuthStatus::invalidate_cache();
        crate::config::invalidate_config_cache();
        crate::tui::app::helpers::clear_ambient_info_cache_for_tests();
    }
}

fn assert_context_and_budget(app: &App, expected: u64) {
    assert_eq!(app.context_limit, expected);
    assert_eq!(
        app.registry.compaction().try_read().unwrap().token_budget(),
        expected as usize,
    );
}

fn authoritative_context_event(provider: &str, window: u64) -> ServerEvent {
    ServerEvent::ModelChanged {
        id: 0,
        model: "remote-million-context".to_string(),
        provider_name: Some(provider.to_string()),
        context_window: Some(window),
        error: None,
        resolved_credential: None,
        provider_oauth_email: None,
        reasoning_effort: None,
    }
}

#[test]
fn remote_account_switch_paths_preserve_authoritative_context_and_budget() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _restore = AccountContextHome(std::env::var_os("JCODE_HOME"));
    crate::env::set_var("JCODE_HOME", home.path());
    crate::auth::claude::set_active_account_override(None);
    crate::auth::codex::set_active_account_override(None);
    crate::auth::AuthStatus::invalidate_cache();
    crate::config::invalidate_config_cache();
    let expires = chrono::Utc::now().timestamp_millis() + 60_000;
    let claude = crate::auth::claude::upsert_account(crate::auth::claude::AnthropicAccount {
        label: String::new(),
        access: "account-context-claude".to_string(),
        refresh: "refresh".to_string(),
        expires,
        email: Some("claude-context@example.com".to_string()),
        scopes: Vec::new(),
        subscription_type: Some("pro".to_string()),
    })
    .unwrap();
    let openai = crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
        label: String::new(),
        access_token: "account-context-openai".to_string(),
        refresh_token: "refresh".to_string(),
        id_token: None,
        account_id: Some("account-context".to_string()),
        expires_at: Some(expires),
        email: Some("openai-context@example.com".to_string()),
    })
    .unwrap();

    for (provider, label) in [("claude", claude), ("openai", openai)] {
        for path in ["explicit", "shorthand", "picker"] {
            let mut app = crate::tui::app::tests::create_test_app();
            assert_eq!(app.provider.context_window(), 200_000);
            app.is_remote = true;
            let runtime = tokio::runtime::Runtime::new().unwrap();
            runtime.block_on(async {
                let mut remote = RemoteConnection::dummy();
                remote.mark_history_loaded();
                app.handle_server_event(
                    authoritative_context_event(provider, 1_000_000),
                    &mut remote,
                );
                assert_context_and_budget(&app, 1_000_000);
                let before = app.display_messages().len();
                match path {
                    "picker" => {
                        app.pending_account_picker_action =
                            Some(crate::tui::AccountPickerAction::Switch {
                                provider_id: provider.to_string(),
                                label: label.clone(),
                            });
                        let mut terminal =
                            Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
                        apply_terminal_event(
                            &mut app,
                            &mut terminal,
                            &mut remote,
                            Some(Ok(Event::Key(KeyEvent::new(
                                KeyCode::Null,
                                KeyModifiers::NONE,
                            )))),
                        )
                        .await
                        .unwrap();
                        assert!(app.pending_account_picker_action.is_none());
                    }
                    _ => {
                        let command = if path == "explicit" {
                            AccountCommand::Switch {
                                provider_id: provider.to_string(),
                                label: label.clone(),
                            }
                        } else {
                            AccountCommand::SwitchShorthand {
                                label: label.clone(),
                            }
                        };
                        execute_account_command_remote(&mut app, command, &mut remote)
                            .await
                            .unwrap();
                    }
                }
                assert!(
                    app.display_messages()[before..]
                        .iter()
                        .any(|message| message.content.contains("Switched to")),
                    "{provider} {path} must execute the switch"
                );
                assert_context_and_budget(&app, 1_000_000);
                app.handle_server_event(
                    ServerEvent::AvailableModelsUpdated {
                        provider_name: Some(provider.to_string()),
                        provider_model: Some("remote-million-context".to_string()),
                        available_models: vec!["remote-million-context".to_string()],
                        available_model_routes: Vec::new(),
                    },
                    &mut remote,
                );
                app.handle_server_event(ServerEvent::Done { id: 1 }, &mut remote);
                assert_context_and_budget(&app, 1_000_000);
                app.handle_server_event(
                    authoritative_context_event(provider, 800_000),
                    &mut remote,
                );
                assert_context_and_budget(&app, 800_000);
            });
        }
    }
}
