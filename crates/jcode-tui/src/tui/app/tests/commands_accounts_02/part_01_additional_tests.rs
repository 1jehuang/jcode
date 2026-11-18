use super::*;

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
