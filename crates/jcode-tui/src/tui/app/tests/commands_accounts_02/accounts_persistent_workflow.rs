use super::*;
use crate::tui::account_picker::{AccountPickerCommand, AccountProviderKind};

fn key(app: &mut App, code: KeyCode) {
    app.handle_key(code, KeyModifiers::empty()).unwrap();
}
fn type_value(app: &mut App, value: &str) {
    app.handle_key(KeyCode::Char('u'), KeyModifiers::CONTROL)
        .unwrap();
    for c in value.chars() {
        key(app, KeyCode::Char(c));
    }
}
fn root(app: &App) {
    let picker = app
        .account_picker_overlay
        .as_ref()
        .expect("Accounts must stay open")
        .borrow();
    assert_eq!(picker.debug_memory_profile()["view"], "accounts");
    assert!(!picker.prompt_is_open());
}
fn rename(app: &mut App, label: &str) {
    app.handle_account_picker_command(AccountPickerCommand::Rename {
        provider: AccountProviderKind::Anthropic,
        label: label.into(),
    });
    assert!(
        app.account_picker_overlay
            .as_ref()
            .unwrap()
            .borrow()
            .prompt_is_open()
    );
}

#[test]
fn account_manager_rename_retries_and_success_stay_in_page_until_explicit_escape() {
    with_temp_jcode_home(|| {
        let (first, second) = store_two_claude_accounts();
        let mut app = create_test_app();
        app.open_account_center(Some("claude"));
        rename(&mut app, &first);
        type_value(&mut app, "");
        key(&mut app, KeyCode::Enter);
        assert!(app.pending_account_input.is_some());
        assert!(
            app.account_picker_overlay
                .as_ref()
                .unwrap()
                .borrow()
                .prompt_is_open()
        );
        type_value(&mut app, &second);
        key(&mut app, KeyCode::Enter);
        assert!(
            app.pending_account_input.is_some(),
            "duplicate must be retryable"
        );
        type_value(&mut app, "Renamed Research");
        key(&mut app, KeyCode::Enter);
        root(&app);
        assert!(app.pending_account_input.is_none());
        let accounts = crate::auth::claude::list_accounts().unwrap();
        assert_eq!(accounts.len(), 2);
        assert!(
            accounts.iter().any(|a| a.label == "Renamed Research"
                && a.email.as_deref() == Some("first@example.com"))
        );
        assert!(accounts.iter().any(|a| a.label == second));
        assert!(
            app.account_picker_overlay
                .as_ref()
                .unwrap()
                .borrow()
                .debug_memory_profile()["visible_items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["command"].as_str().unwrap().contains("Renamed Research"))
        );
        rename(&mut app, "Renamed Research");
        type_value(&mut app, "Cancelled label");
        key(&mut app, KeyCode::Esc);
        root(&app);
        assert!(
            crate::auth::claude::list_accounts()
                .unwrap()
                .iter()
                .all(|a| a.label != "Cancelled label")
        );
        key(&mut app, KeyCode::Esc);
        assert!(app.account_picker_overlay.is_none());
        assert!(app.input().is_empty());
    });
}

#[test]
fn account_manager_remove_and_add_chooser_return_to_refreshed_accounts() {
    with_temp_jcode_home(|| {
        let (first, second) = store_two_claude_accounts();
        let mut app = create_test_app();
        app.open_account_center(Some("claude"));
        app.handle_account_picker_command(AccountPickerCommand::Remove {
            provider: AccountProviderKind::Anthropic,
            label: second.clone(),
        });
        root(&app);
        assert_eq!(crate::auth::claude::list_accounts().unwrap().len(), 1);
        assert_eq!(
            crate::auth::claude::list_accounts().unwrap()[0].label,
            first
        );
        let rows = app
            .account_picker_overlay
            .as_ref()
            .unwrap()
            .borrow()
            .debug_memory_profile();
        assert!(
            rows["visible_items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| !r["command"].as_str().unwrap().contains(&second))
        );
        app.open_account_add_replace_flow(Some("claude"));
        key(&mut app, KeyCode::Esc);
        root(&app);
        key(&mut app, KeyCode::Esc);
        assert!(app.account_picker_overlay.is_none());
    });
}

fn pending(label: &str, state: u8) -> (PendingLogin, std::sync::Arc<std::sync::atomic::AtomicU8>) {
    let attempt = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(state));
    (
        PendingLogin::ClaudeAccount {
            verifier: "in-page-test-verifier".into(),
            label: label.into(),
            expected_existing: true,
            expected_identity: Some("first@example.com".into()),
            replace: true,
            attempt: attempt.clone(),
            redirect_uri: None,
        },
        attempt,
    )
}

#[test]
fn account_manager_login_cancel_blocks_inflight_work_but_never_claims_cancel_after_commit() {
    with_temp_jcode_home(|| {
        let (first, _) = store_two_claude_accounts();
        let mut app = create_test_app();
        app.open_account_center(None);
        let (login, guard) = pending(&first, 4);
        app.pending_login = Some(login);
        app.finish_account_manager_action(app.display_messages.len(), None);
        assert!(
            app.account_picker_overlay
                .as_ref()
                .unwrap()
                .borrow()
                .prompt_is_open()
        );
        key(&mut app, KeyCode::Esc);
        root(&app);
        assert!(app.pending_login.is_none());
        assert_eq!(guard.load(std::sync::atomic::Ordering::Acquire), 2);
        let (login, guard) = pending(&first, 1);
        app.pending_login = Some(login);
        app.finish_account_manager_action(app.display_messages.len(), None);
        let start = app.display_messages.len();
        key(&mut app, KeyCode::Esc);
        assert!(app.pending_login.is_some());
        assert!(
            app.account_picker_overlay
                .as_ref()
                .unwrap()
                .borrow()
                .prompt_is_open()
        );
        assert_eq!(guard.load(std::sync::atomic::Ordering::Acquire), 1);
        assert!(
            app.display_messages[start..]
                .iter()
                .all(|m| !m.content.contains("cancelled"))
        );
    });
}

#[test]
fn account_manager_sensitive_paste_and_login_results_stay_in_page() {
    with_temp_jcode_home(|| {
        let (first, _) = store_two_claude_accounts();
        let mut app = create_test_app();
        app.open_account_center(None);
        let (login, _) = pending(&first, 0);
        app.pending_login = Some(login);
        app.finish_account_manager_action(app.display_messages.len(), None);
        let history = app.persisted_prompt_history.clone();
        assert!(app.paste_account_manager_prompt("secret-callback-code"));
        assert!(app.input().is_empty());
        assert_eq!(app.persisted_prompt_history, history);
        let debug = app
            .account_picker_overlay
            .as_ref()
            .unwrap()
            .borrow()
            .debug_memory_profile()
            .to_string();
        assert!(!debug.contains("secret-callback-code"));
        app.pending_login = None;
        app.finish_account_manager_login("claude", false, "Login rejected: wrong identity");
        root(&app);
        app.finish_account_manager_login("claude", true, "Login saved");
        root(&app);
    });
}

#[test]
fn account_manager_remote_form_keyboard_never_leaks_into_model_or_chat() {
    with_temp_jcode_home(|| {
        let (first, _) = store_two_claude_accounts();
        let mut app = create_test_app();
        app.is_remote = true;
        app.open_account_center(Some("claude"));
        rename(&mut app, &first);
        let history = app.persisted_prompt_history.clone();
        let session_messages = app.session.messages.len();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            let peer = remote.take_dummy_peer().unwrap();
            let mut reader = tokio::io::BufReader::new(peer);
            let before = remote.next_request_id_for_test();
            app.handle_remote_key(KeyCode::Char('u'), KeyModifiers::CONTROL, &mut remote)
                .await
                .unwrap();
            for c in "Remote in-page label".chars() {
                app.handle_remote_key(KeyCode::Char(c), KeyModifiers::empty(), &mut remote)
                    .await
                    .unwrap();
            }
            app.handle_remote_key(KeyCode::Enter, KeyModifiers::CONTROL, &mut remote)
                .await
                .unwrap();
            root(&app);
            assert_eq!(remote.next_request_id_for_test(), before);
            assert_eq!(app.session.messages.len(), session_messages);
            assert_eq!(app.persisted_prompt_history, history);
            assert!(app.input().is_empty());
            assert!(!app.is_processing());
            let mut line = String::new();
            use tokio::io::AsyncBufReadExt;
            assert!(
                tokio::time::timeout(
                    std::time::Duration::from_millis(50),
                    reader.read_line(&mut line)
                )
                .await
                .is_err()
            );
        });
        assert!(
            crate::auth::claude::list_accounts()
                .unwrap()
                .iter()
                .any(|a| a.label == "Remote in-page label")
        );
    });
}
