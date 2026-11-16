use super::*;

#[test]
fn test_usage_card_reset_balances() {
    let mut app = create_test_app();
    for count in [0, 3] {
        app.handle_usage_report(vec![crate::usage::ProviderUsage {
            provider_name: "OpenAI (work)".into(),
            openai_reset_credits: Some(crate::usage::OpenAiResetCredits {
                available_count: count,
                available_expirations: vec![],
                account_label: Some("work".into()),
                ordinary_usage_allowed: Some(true),
            }),
            ..Default::default()
        }]);
        let content = &app.display_messages().last().unwrap().content;
        assert!(content.contains(&format!("Banked resets: {count} available")));
        assert!(!content.contains("no usage data available"));
    }
    for available in [false, true] {
        app.handle_usage_report(vec![crate::usage::ProviderUsage {
            provider_name: "Claude (personal)".into(),
            anthropic_limit_reset: Some(crate::usage::AnthropicLimitResetOffer {
                account_label: Some("personal".into()),
                available,
                next_available_at: Some("2099-01-01T00:00:00Z".into()),
                resets_per_week: 1,
            }),
            ..Default::default()
        }]);
        let content = &app.display_messages().last().unwrap().content;
        assert!(content.contains(&format!(
            "Session resets: {} available",
            u64::from(available)
        )));
        assert_eq!(content.contains("next in"), !available);
    }
    app.handle_usage_report(vec![crate::usage::ProviderUsage {
        provider_name: "OpenAI (failed)".into(),
        error: Some("fetch failed".into()),
        openai_reset_credits: Some(crate::usage::OpenAiResetCredits {
            available_count: 7,
            available_expirations: vec![],
            account_label: Some("failed".into()),
            ordinary_usage_allowed: None,
        }),
        ..Default::default()
    }]);
    assert!(
        !app.display_messages()
            .last()
            .unwrap()
            .content
            .contains("Banked resets:")
    );
    for error in [None, Some("fetch failed".into())] {
        app.handle_usage_report(vec![crate::usage::ProviderUsage {
            provider_name: "Unknown".into(),
            error,
            ..Default::default()
        }]);
        let content = &app.display_messages().last().unwrap().content;
        assert!(!content.contains("Banked resets:"));
        assert!(!content.contains("Session resets:"));
    }
}

#[test]
fn test_usage_card_responsive_terminal_layout() {
    let mut app = create_test_app();
    app.handle_usage_report(vec![crate::usage::ProviderUsage {
        provider_name: "OpenAI (personal)".into(),
        limits: vec![crate::usage::UsageLimit {
            name: "5-hour window".into(),
            usage_percent: 42.0,
            resets_at: None,
        }],
        openai_reset_credits: Some(crate::usage::OpenAiResetCredits {
            available_count: 3,
            available_expirations: vec![],
            account_label: Some("personal".into()),
            ordinary_usage_allowed: Some(true),
        }),
        ..Default::default()
    }]);
    let mut card_widths = Vec::new();
    for width in [40, 60, 100, 180] {
        let backend = ratatui::backend::TestBackend::new(width, 60);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &app))
            .unwrap();
        let text = buffer_to_text(&terminal);
        let normalized: String = text
            .chars()
            .filter(|ch| !ch.is_whitespace() && *ch != '│')
            .collect();
        assert!(
            normalized.contains("Bankedresets:3available"),
            "{width} cols:\n{text}"
        );
        assert!(text.contains("OpenAI (personal)"), "{width} cols:\n{text}");
        assert!(text.contains("42%"), "{width} cols:\n{text}");
        let border = text
            .lines()
            .find(|line| line.contains('╭') && line.contains("Usage"))
            .unwrap_or_else(|| panic!("missing usage border at {width}:\n{text}"));
        let start = border.find('╭').unwrap();
        let end = border.find('╮').unwrap();
        let card_width =
            unicode_width::UnicodeWidthStr::width(&border[start..end + '╮'.len_utf8()]);
        assert!(card_width <= usize::from(width));
        card_widths.push(card_width);
    }
    assert_eq!(
        card_widths[2], card_widths[3],
        "wide terminals should fit the content, not expand the card"
    );
    assert!(card_widths[0] < card_widths[2]);
}

#[test]
fn test_usage_card_renders_when_loading() {
    let mut app = create_test_app();
    app.open_usage_inline_loading();

    let backend = ratatui::backend::TestBackend::new(120, 40);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    terminal
        .draw(|frame| crate::tui::ui::draw(frame, &app))
        .expect("usage card draw should succeed");

    let text = buffer_to_text(&terminal);
    assert!(
        text.contains("╭"),
        "usage card should render as rounded box, got:\n{text}"
    );
    assert!(
        text.contains("Refreshing usage"),
        "usage card should be visible while loading, got:\n{text}"
    );
    assert!(
        text.contains("Checking connected provider limits"),
        "usage card should include loading details, got:\n{text}"
    );
}

#[test]
fn test_usage_card_does_not_capture_typing() {
    let mut app = create_test_app();
    app.open_usage_inline_loading();
    assert!(app.usage_overlay.is_none());

    app.handle_key(KeyCode::Char('h'), KeyModifiers::empty())
        .expect("type after usage card");

    assert!(app.usage_overlay.is_none());
    assert_eq!(app.input(), "h");
}

#[test]
fn test_usage_report_updates_display_only_card_without_system_message() {
    let mut app = create_test_app();
    app.usage_report_refreshing = true;
    // App::new seeds the provider transcript with the immutable session-context
    // reminder, so assert the usage report adds nothing on top of it rather
    // than expecting an empty transcript.
    let provider_messages_before = app.materialized_provider_messages().len();
    app.handle_usage_report(vec![crate::usage::ProviderUsage {
        provider_name: "OpenAI (ChatGPT)".to_string(),
        limits: vec![crate::usage::UsageLimit {
            name: "5h".to_string(),
            usage_percent: 82.0,
            resets_at: None,
        }],
        extra_info: vec![("plan".to_string(), "pro".to_string())],
        hard_limit_reached: false,
        openai_reset_credits: None,
        anthropic_limit_reset: None,
        error: None,
        last_used_unix_secs: None,
    }]);

    assert!(!app.usage_report_refreshing);
    assert!(app.inline_view_state.is_none());
    assert!(app.usage_overlay.is_none());
    let msg = app.display_messages().last().expect("missing usage card");
    assert_eq!(msg.role, "usage");
    assert!(msg.content.contains("OpenAI (ChatGPT)"));
    assert!(msg.content.contains("5h"));
    assert!(msg.content.contains("82%"));
    assert!(msg.content.contains("plan: pro"));
    assert!(!msg.content.contains("resets: "));
    let leaked = app.materialized_provider_messages();
    assert_eq!(
        leaked.len(),
        provider_messages_before,
        "usage report must not add provider-visible messages: {leaked:#?}"
    );
}

#[test]
fn test_usage_progress_updates_card_incrementally() {
    let mut app = create_test_app();
    app.open_usage_inline_loading();

    app.handle_usage_report_progress(crate::usage::ProviderUsageProgress {
        results: vec![crate::usage::ProviderUsage {
            provider_name: "Anthropic (Claude)".to_string(),
            limits: vec![crate::usage::UsageLimit {
                name: "5-hour window".to_string(),
                usage_percent: 41.0,
                resets_at: None,
            }],
            extra_info: Vec::new(),
            hard_limit_reached: false,
            openai_reset_credits: None,
            anthropic_limit_reset: None,
            error: None,
            last_used_unix_secs: None,
        }],
        completed: 1,
        total: 2,
        done: false,
        from_cache: false,
    });

    assert!(app.usage_report_refreshing);
    assert_eq!(
        app.display_messages()
            .iter()
            .filter(|message| message.role == "usage")
            .count(),
        1
    );
    let detail = &app
        .display_messages()
        .last()
        .expect("missing usage card")
        .content;
    assert!(detail.contains("5-hour window") || detail.contains("Refreshing usage (1/2)"));
}

#[test]
fn test_usage_with_suffix_does_not_open_picker_preview() {
    let mut app = create_test_app();

    for c in "/usage open".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }

    assert!(app.inline_interactive_state.is_none());
    assert_eq!(app.input(), "/usage open");
}
