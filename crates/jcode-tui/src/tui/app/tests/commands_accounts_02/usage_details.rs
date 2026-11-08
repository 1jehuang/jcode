#[test]
fn test_openai_account_usage_details_are_discoverable_and_keep_switching() {
    with_temp_jcode_home(|| {
        // New accounts receive canonical labels from the store, not the requested label.
        let labels: Vec<String> = ["openai-otter", "openai-fox"]
            .into_iter()
            .map(|label| {
                crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
                    label: label.to_string(),
                    access_token: "acc".into(),
                    refresh_token: "ref".into(),
                    id_token: None,
                    account_id: Some(format!("acct_{label}")),
                    expires_at: None,
                    email: None,
                })
                .unwrap()
            })
            .collect();
        crate::provider_activity::record_openai_oauth_usage(
            &labels[0],
            "gpt-5.4",
            None,
            Some(12345),
            Some(678),
            Some(123),
        );
        let mut app = create_test_app();
        let settings = app.render_openai_accounts_markdown();
        assert!(
            settings.contains("12,345 in · 678 out · 123 cached"),
            "recorded account usage: {settings}"
        );
        assert!(settings.contains("No recorded usage"));
        assert!(settings.contains("local midnight"));
        assert!(settings.contains("not your subscription bill"));
        for label in &labels {
            assert!(settings.contains(&format!("(`{label}`)")));
            for (key, value) in crate::provider_activity::openai_oauth_usage_summary(label) {
                assert!(settings.contains(&format!("- **{key}:** {value}")));
            }
        }
        for filter in [None, Some("openai")] {
            app.open_account_picker(filter);
            let state = app.inline_interactive_state.as_ref().unwrap();
            for label in &labels {
                assert!(state.entries.iter().any(|entry| matches!(&entry.action,
                    crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::Switch { provider_id, label: actual })
                        if provider_id == "openai" && actual == label)));
            }
            let usage = state
                .entries
                .iter()
                .find(|entry| entry.name == "OpenAI usage details")
                .expect("visible usage action");
            assert!(
                matches!(&usage.action, crate::tui::PickerAction::Usage { detail_lines, .. }
                if detail_lines.join("\n") == settings)
            );
        }
        let state = app.inline_interactive_state.as_mut().unwrap();
        state.selected = state
            .entries
            .iter()
            .position(|entry| entry.name == "OpenAI usage details")
            .unwrap();
        app.handle_key(KeyCode::Enter, KeyModifiers::empty())
            .unwrap();
        assert!(app.inline_interactive_state.is_none());
        let displayed = &app
            .display_messages()
            .last()
            .expect("full usage report")
            .content;
        assert!(displayed.contains(&settings));

        let provider = crate::provider_catalog::login_providers()
            .iter()
            .find(|provider| provider.id == "openai")
            .copied()
            .unwrap();
        let mut items = Vec::new();
        app.append_openai_account_picker_items(&mut items, provider);
        for label in &labels {
            let command = format!("/account openai switch {label}");
            let item = items.iter().find(|item| matches!(&item.command,
                crate::tui::account_picker::AccountPickerCommand::SubmitInput(input) if input == &command)).unwrap();
            assert!(
                item.details
                    .iter()
                    .any(|(_, value)| value == "/account openai settings")
            );
            for row in crate::provider_activity::openai_oauth_usage_summary(label) {
                assert!(item.details.contains(&row));
            }
            assert!(
                !item.subtitle.contains("Lifetime"),
                "full usage is not a truncated subtitle"
            );
        }
    });
}
