#[test]
fn test_fast_default_on_saves_config_and_updates_session() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let mut app = create_fast_test_app();
    app.input = "/fast default on".to_string();

    app.submit_input();

    let cfg = crate::config::Config::load();
    assert_eq!(
        cfg.provider.openai_service_tier.as_deref(),
        Some("priority")
    );
    assert_eq!(app.provider.service_tier().as_deref(), Some("priority"));
    assert_eq!(app.status_notice(), Some("Fast mode: on".to_string()));
    let last = app.display_messages().last().expect("missing response");
    assert_eq!(last.content, "Saved OpenAI fast mode: **on**.");

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_fast_status_shows_saved_default() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());
    crate::config::Config::set_openai_service_tier(Some("priority")).expect("save fast default");

    let mut app = create_fast_test_app();
    app.input = "/fast status".to_string();

    app.submit_input();

    let last = app.display_messages().last().expect("missing response");
    assert_eq!(
        last.content,
        "Fast mode is off.\nCurrent tier: Standard\nSaved default: on (Fast)\nUse `/fast on`, `/fast off`, or `/fast default on|off`."
    );

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_alignment_command_persists_and_applies_immediately() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.set_centered(false);
        app.input = "/alignment centered".to_string();

        app.submit_input();

        let cfg = crate::config::Config::load();
        assert!(cfg.display.centered);
        assert!(app.centered_mode());
        assert_eq!(app.status_notice(), Some("Layout: Centered".to_string()));

        let last = app.display_messages().last().expect("missing response");
        assert_eq!(last.role, "system");
        assert!(
            last.content
                .contains("Saved default alignment: **centered**")
        );
    });
}

#[test]
fn test_alignment_status_shows_current_and_saved_defaults() {
    with_temp_jcode_home(|| {
        crate::config::Config::set_display_centered(false).expect("save alignment default");

        let mut app = create_test_app();
        app.set_centered(true);
        app.input = "/alignment".to_string();

        app.submit_input();

        let last = app.display_messages().last().expect("missing response");
        assert_eq!(last.role, "system");
        assert!(
            last.content
                .contains("Alignment is currently **centered**.")
        );
        assert!(last.content.contains("Saved default: **left-aligned**."));
        assert!(last.content.contains("/alignment centered"));
        assert!(last.content.contains("Alt+C"));
    });
}

#[test]
fn test_alignment_invalid_usage_shows_error() {
    let mut app = create_test_app();
    app.input = "/alignment diagonal".to_string();

    app.submit_input();

    let last = app.display_messages().last().expect("missing response");
    assert_eq!(last.role, "error");
    assert!(last.content.contains("Usage: `/alignment`"));
}

#[test]
fn test_help_topic_shows_fix_command_details() {
    let mut app = create_test_app();
    app.input = "/help fix".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing help response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("`/fix`"));
}

#[test]
fn test_mask_email_censors_local_part() {
    assert_eq!(mask_email("jeremyh1@uw.edu"), "j***1@uw.edu");
}

#[test]
fn test_subscription_command_shows_jcode_status_scaffold() {
    let _guard = crate::storage::lock_test_env();
    crate::subscription_catalog::clear_runtime_env();
    crate::env::remove_var(crate::subscription_catalog::JCODE_API_KEY_ENV);
    crate::env::remove_var(crate::subscription_catalog::JCODE_API_BASE_ENV);

    let mut app = create_test_app();
    app.input = "/subscription".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing /subscription response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("Jcode Subscription Status"));
    assert!(msg.content.contains("/login jcode"));
    assert!(msg.content.contains("Healer Alpha"));
    assert!(msg.content.contains("Kimi K2.5"));
    assert!(msg.content.contains("$20 Starter"));
    assert!(msg.content.contains("$100 Pro"));
}

#[test]
fn test_usage_report_shows_no_connected_providers_when_results_empty() {
    let mut app = create_test_app();
    app.handle_usage_report(Vec::new());

    let msg = app.display_messages().last().expect("missing usage card");
    assert_eq!(msg.role, "usage");
    assert!(msg.content.contains("No connected providers"));
    assert!(msg.content.contains("/login claude"));
    assert!(msg.content.contains("/login openai"));
}

#[test]
fn test_usage_command_requests_usage_report_with_inline_view() {
    let mut app = create_test_app();

    assert!(super::commands::handle_usage_command(&mut app, "/usage"));

    assert!(app.inline_interactive_state.is_none());
    assert!(app.usage_overlay.is_none());
    assert!(app.inline_view_state.is_none());
    assert_eq!(
        app.display_messages().last().map(|m| m.role.as_str()),
        Some("usage")
    );
    assert!(app.usage_report_refreshing);
}

#[test]
fn test_usage_submit_input_requests_usage_report_with_inline_view() {
    let mut app = create_test_app();
    app.input = "/usage".to_string();

    app.submit_input();

    assert!(app.inline_interactive_state.is_none());
    assert!(app.usage_overlay.is_none());
    assert!(app.inline_view_state.is_none());
    assert_eq!(
        app.display_messages().last().map(|m| m.role.as_str()),
        Some("usage")
    );
    assert!(app.usage_report_refreshing);
}

#[test]
fn test_usage_typing_does_not_open_picker_preview() {
    let mut app = create_test_app();

    for c in "/usage".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .expect("type /usage");
    }

    assert!(app.inline_interactive_state.is_none());
    assert_eq!(app.input(), "/usage");
    assert!(!app.usage_report_refreshing);
}

#[test]
fn test_usage_enter_requests_report_with_inline_view() {
    let mut app = create_test_app();

    for c in "/usage".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .expect("type /usage");
    }

    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .expect("submit /usage");

    assert!(app.inline_interactive_state.is_none());
    assert!(app.usage_overlay.is_none());
    assert!(app.inline_view_state.is_none());
    assert_eq!(app.input(), "");
    assert_eq!(
        app.display_messages().last().map(|m| m.role.as_str()),
        Some("usage")
    );
    assert!(app.usage_report_refreshing);
}
