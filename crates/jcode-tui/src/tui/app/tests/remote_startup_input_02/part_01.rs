#[test]
fn test_model_picker_copilot_selection_prefixes_model() {
    let mut app = create_test_app();
    configure_test_remote_models_with_copilot(&mut app);

    app.open_model_picker();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");

    // Find grok-code-fast-1 (which should only be a copilot route)
    let grok_idx = picker
        .entries
        .iter()
        .position(|m| m.name == "grok-code-fast-1")
        .expect("grok-code-fast-1 should be in picker");

    // Navigate to it and select
    let filtered_pos = picker
        .filtered
        .iter()
        .position(|&i| i == grok_idx)
        .expect("grok-code-fast-1 should be in filtered list");

    // Set the selected position to grok's position
    app.inline_interactive_state.as_mut().unwrap().selected = filtered_pos;

    // Press Enter to select
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    // In remote mode, selection should produce a pending_model_switch with copilot: prefix
    if let Some(ref spec) = app.pending_model_switch {
        assert!(
            spec.starts_with("copilot:"),
            "copilot model should be prefixed with 'copilot:', got: {}",
            spec
        );
    }
    // Picker should be closed
    assert!(app.inline_interactive_state.is_none());
}

#[test]
fn test_model_picker_cursor_models_have_cursor_route() {
    let mut app = create_test_app();
    configure_test_remote_models_with_cursor(&mut app);

    app.open_model_picker();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");

    let composer_entry = picker
        .entries
        .iter()
        .find(|m| m.name == "composer-2-fast")
        .expect("composer-2-fast should be in picker");

    assert!(
        composer_entry
            .options
            .iter()
            .any(|r| r.api_method == "cursor"),
        "composer-2-fast should have a cursor route, got: {:?}",
        composer_entry.options
    );
}

#[test]
fn test_model_picker_cursor_selection_prefixes_model() {
    let mut app = create_test_app();
    configure_test_remote_models_with_cursor(&mut app);

    app.open_model_picker();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");

    let composer_idx = picker
        .entries
        .iter()
        .position(|m| m.name == "composer-2-fast")
        .expect("composer-2-fast should be in picker");

    let filtered_pos = picker
        .filtered
        .iter()
        .position(|&i| i == composer_idx)
        .expect("composer-2-fast should be in filtered list");

    app.inline_interactive_state.as_mut().unwrap().selected = filtered_pos;

    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    assert_eq!(
        app.pending_model_switch.as_deref(),
        Some("cursor:composer-2-fast")
    );
    assert!(app.inline_interactive_state.is_none());
}

#[test]
fn test_model_picker_bedrock_selection_prefixes_model() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_available_entries = vec!["amazon.nova-pro-v1:0".to_string()];
    app.remote_model_options = vec![crate::provider::ModelRoute {
        model: "amazon.nova-pro-v1:0".to_string(),
        provider: "AWS Bedrock".to_string(),
        api_method: "bedrock".to_string(),
        available: true,
        detail: String::new(),
        cheapness: None,
    }];

    app.open_model_picker();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");
    let model_idx = picker
        .entries
        .iter()
        .position(|m| m.name == "amazon.nova-pro-v1:0")
        .expect("Bedrock model should be in picker");
    let filtered_pos = picker
        .filtered
        .iter()
        .position(|&i| i == model_idx)
        .expect("Bedrock model should be in filtered list");

    app.inline_interactive_state.as_mut().unwrap().selected = filtered_pos;
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    assert_eq!(
        app.pending_model_switch.as_deref(),
        Some("bedrock:amazon.nova-pro-v1:0")
    );
    assert!(app.inline_interactive_state.is_none());
}

#[test]
fn test_model_picker_bedrock_arn_selection_prefixes_model() {
    let mut app = create_test_app();
    app.is_remote = true;
    let model = "arn:aws:bedrock:us-east-2:302154194530:inference-profile/us.deepseek.r1-v1:0";
    app.remote_available_entries = vec![model.to_string()];
    app.remote_model_options = vec![crate::provider::ModelRoute {
        model: model.to_string(),
        provider: "AWS Bedrock".to_string(),
        api_method: "bedrock".to_string(),
        available: true,
        detail: String::new(),
        cheapness: None,
    }];

    app.open_model_picker();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");
    let model_idx = picker
        .entries
        .iter()
        .position(|m| m.name == model)
        .expect("Bedrock ARN should be in picker");
    let filtered_pos = picker
        .filtered
        .iter()
        .position(|&i| i == model_idx)
        .expect("Bedrock ARN should be in filtered list");

    app.inline_interactive_state.as_mut().unwrap().selected = filtered_pos;
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    let expected = format!("bedrock:{model}");
    assert_eq!(app.pending_model_switch.as_deref(), Some(expected.as_str()));
    assert!(app.inline_interactive_state.is_none());
}

#[test]
fn test_remote_fallback_bedrock_arn_does_not_create_openrouter_route() {
    let mut app = create_test_app();
    app.is_remote = true;
    let model = "arn:aws:bedrock:us-east-2:302154194530:inference-profile/us.deepseek.r1-v1:0";
    app.remote_available_entries = vec![model.to_string()];
    app.remote_model_options.clear();

    let routes = app.build_remote_model_routes_fallback();

    assert!(routes.iter().any(|route| {
        route.model == model && route.api_method == "bedrock" && route.provider == "AWS Bedrock"
    }));
    assert!(
        !routes
            .iter()
            .any(|route| route.model == model && route.api_method == "openrouter")
    );
}

#[test]
fn test_remote_current_fpt_live_model_uses_fpt_route_not_copilot_without_cache() {
    with_temp_jcode_home(|| {
        crate::env::set_var("FPT_API_KEY", "test-fpt-key");

        let mut app = create_test_app();
        app.is_remote = true;
        app.remote_provider_name = Some("FPT AI Marketplace".to_string());
        app.remote_available_entries = vec!["GLM-5.1".to_string()];
        app.remote_model_options.clear();

        let routes = app.build_remote_model_routes_fallback();

        assert!(
            routes.iter().any(|route| {
                route.model == "GLM-5.1"
                    && route.provider == "FPT AI Marketplace"
                    && route.api_method == "openai-compatible:fpt"
            }),
            "FPT current-provider live model should use FPT route, got {routes:?}"
        );
        assert!(
            !routes
                .iter()
                .any(|route| route.model == "GLM-5.1" && route.api_method == "copilot"),
            "FPT current-provider live model must not be guessed as Copilot: {routes:?}"
        );

        crate::env::remove_var("FPT_API_KEY");
    });
}

#[test]
fn test_remote_fallback_claude_model_gets_api_key_route_without_oauth() {
    // A newly released Claude model can reach the picker via the names-only
    // catalog fallback (oversized route frames are downgraded to model names).
    // With only ANTHROPIC_API_KEY configured, the fallback must synthesize a
    // claude-api route; previously it only ever emitted claude-oauth routes.
    with_temp_jcode_home(|| {
        crate::env::set_var("ANTHROPIC_API_KEY", "sk-ant-test-key");
        crate::auth::AuthStatus::invalidate_cache();

        let mut app = create_test_app();
        app.is_remote = true;
        app.remote_available_entries = vec!["claude-fable-5".to_string()];
        app.remote_model_options.clear();

        let routes = app.build_remote_model_routes_fallback();

        assert!(
            routes.iter().any(|route| {
                route.model == "claude-fable-5"
                    && route.provider == "Anthropic"
                    && route.api_method == "claude-api"
                    && route.available
            }),
            "claude model with only an API key should get a claude-api fallback route, got {routes:?}"
        );

        crate::env::remove_var("ANTHROPIC_API_KEY");
        crate::auth::AuthStatus::invalidate_cache();
    });
}

#[test]
fn test_remote_cached_oauth_only_claude_route_gains_api_key_route_in_picker() {
    struct AnthropicApiKeyGuard(Option<String>);

    impl Drop for AnthropicApiKeyGuard {
        fn drop(&mut self) {
            if let Some(value) = self.0.take() {
                crate::env::set_var("ANTHROPIC_API_KEY", value);
            } else {
                crate::env::remove_var("ANTHROPIC_API_KEY");
            }
            crate::auth::AuthStatus::invalidate_cache();
        }
    }

    // A stale persisted catalog can carry an OAuth-only route for a newly
    // released Claude model. When an Anthropic API key is configured, opening
    // the picker must add the claude-api route instead of trusting the stale
    // single-route cache forever.
    with_temp_jcode_home(|| {
        let _api_key_guard =
            AnthropicApiKeyGuard(std::env::var("ANTHROPIC_API_KEY").ok());
        crate::env::set_var("ANTHROPIC_API_KEY", "sk-ant-test-key");
        crate::auth::AuthStatus::invalidate_cache();

        let mut app = create_test_app();
        app.is_remote = true;
        app.remote_available_entries = vec!["claude-fable-5".to_string()];
        app.remote_model_options = vec![crate::provider::ModelRoute {
            model: "claude-fable-5".to_string(),
            provider: "Anthropic".to_string(),
            api_method: "claude-oauth".to_string(),
            available: true,
            detail: String::new(),
            cheapness: None,
        }];

        app.open_model_picker();

        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("model picker should be open");
        let fable_entries = picker
            .entries
            .iter()
            .filter(|entry| {
                entry.name == "claude-fable-5"
                    || entry.name.starts_with("claude-fable-5 (")
            })
            .collect::<Vec<_>>();
        assert!(!fable_entries.is_empty(), "fable should be in the picker");
        assert!(
            fable_entries.iter().any(|entry| entry.options.iter().any(
                |option| option.api_method == "claude-api" && option.available
            )),
            "stale oauth-only cached route should be augmented with claude-api, got {:?}",
            fable_entries
        );

    });
}

#[test]
fn test_remote_jcode_subscription_catalog_is_not_augmented_with_local_auth_routes() {
    with_temp_jcode_home(|| {
        let previous_anthropic_key = std::env::var_os("ANTHROPIC_API_KEY");
        crate::env::set_var("ANTHROPIC_API_KEY", "sk-ant-test-key");
        crate::auth::AuthStatus::invalidate_cache();

        let mut app = create_test_app();
        app.is_remote = true;
        app.remote_provider_name =
            Some(crate::subscription_catalog::JCODE_PROVIDER_DISPLAY_NAME.to_string());
        app.remote_available_entries = vec![
            "claude-opus-4-8".to_string(),
            "gpt-5.5".to_string(),
            "gpt-5.6-sol".to_string(),
        ];
        app.remote_model_options = vec![crate::provider::ModelRoute {
            model: "claude-opus-4-8".to_string(),
            provider: "Anthropic".to_string(),
            api_method: "claude-api".to_string(),
            available: true,
            detail: "stale cached route".to_string(),
            cheapness: None,
        }];

        app.open_model_picker();

        match previous_anthropic_key {
            Some(value) => crate::env::set_var("ANTHROPIC_API_KEY", value),
            None => crate::env::remove_var("ANTHROPIC_API_KEY"),
        }
        crate::auth::AuthStatus::invalidate_cache();

        assert_eq!(app.remote_model_options.len(), 3);
        assert!(app.remote_model_options.iter().all(|route| {
            route.provider == crate::subscription_catalog::JCODE_PROVIDER_DISPLAY_NAME
                && route.api_method == crate::subscription_catalog::JCODE_ROUTE_API_METHOD
                && route.available
        }));
    });
}

#[test]
fn test_remote_mixed_catalog_keeps_jcode_subscription_separate_from_other_providers() {
    ensure_test_jcode_home_if_unset();
    clear_persisted_test_ui_state();
    crate::tui::ui::clear_test_render_state_for_tests();

    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_provider_name = Some("Claude".to_string());
    app.remote_available_entries = vec![
        "claude-fable-5".to_string(),
        "claude-opus-4-8".to_string(),
        "gpt-5.5".to_string(),
        "gpt-5.6-sol".to_string(),
        "deepseek/deepseek-v4-pro".to_string(),
    ];
    app.remote_model_options = vec![
        crate::provider::ModelRoute {
            model: "claude-fable-5".to_string(),
            provider: "Anthropic".to_string(),
            api_method: "claude-oauth".to_string(),
            available: true,
            detail: String::new(),
            cheapness: None,
        },
        crate::provider::ModelRoute {
            model: "claude-opus-4-8".to_string(),
            provider: crate::subscription_catalog::JCODE_PROVIDER_DISPLAY_NAME.to_string(),
            api_method: crate::subscription_catalog::JCODE_ROUTE_API_METHOD.to_string(),
            available: true,
            detail: "managed subscription route".to_string(),
            cheapness: None,
        },
        crate::provider::ModelRoute {
            model: "gpt-5.5".to_string(),
            provider: crate::subscription_catalog::JCODE_PROVIDER_DISPLAY_NAME.to_string(),
            api_method: crate::subscription_catalog::JCODE_ROUTE_API_METHOD.to_string(),
            available: true,
            detail: "managed subscription route".to_string(),
            cheapness: None,
        },
        crate::provider::ModelRoute {
            model: "gpt-5.6-sol".to_string(),
            provider: crate::subscription_catalog::JCODE_PROVIDER_DISPLAY_NAME.to_string(),
            api_method: crate::subscription_catalog::JCODE_ROUTE_API_METHOD.to_string(),
            available: true,
            detail: "managed subscription route".to_string(),
            cheapness: None,
        },
        crate::provider::ModelRoute {
            model: "deepseek/deepseek-v4-pro".to_string(),
            provider: "auto".to_string(),
            api_method: "openrouter".to_string(),
            available: true,
            detail: String::new(),
            cheapness: None,
        },
    ];

    app.open_model_picker();

    assert_eq!(app.remote_model_options.len(), 5);
    let jcode_routes = app
        .remote_model_options
        .iter()
        .filter(|route| {
            route.provider == crate::subscription_catalog::JCODE_PROVIDER_DISPLAY_NAME
                && route.api_method == crate::subscription_catalog::JCODE_ROUTE_API_METHOD
        })
        .collect::<Vec<_>>();
    assert_eq!(jcode_routes.len(), 3);
    assert_eq!(
        jcode_routes
            .iter()
            .map(|route| route.model.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from(["claude-opus-4-8", "gpt-5.5", "gpt-5.6-sol",])
    );
    assert!(app.remote_model_options.iter().any(|route| {
        route.model == "claude-fable-5"
            && route.provider == "Anthropic"
            && route.api_method == "claude-oauth"
    }));
    assert!(app.remote_model_options.iter().any(|route| {
        route.model == "deepseek/deepseek-v4-pro"
            && route.provider == "auto"
            && route.api_method == "openrouter"
    }));
    assert!(app.remote_model_options.iter().all(|route| {
        route.provider != crate::subscription_catalog::JCODE_PROVIDER_DISPLAY_NAME
            || matches!(
                route.model.as_str(),
                "claude-opus-4-8" | "gpt-5.5" | "gpt-5.6-sol"
            )
    }));
}

#[test]
fn test_model_picker_ctrl_b_bedrock_selection_saves_bedrock_default() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.is_remote = true;
        app.remote_available_entries = vec!["amazon.nova-pro-v1:0".to_string()];
        app.remote_model_options = vec![crate::provider::ModelRoute {
            model: "amazon.nova-pro-v1:0".to_string(),
            provider: "AWS Bedrock".to_string(),
            api_method: "bedrock".to_string(),
            available: true,
            detail: String::new(),
            cheapness: None,
        }];

        app.open_model_picker();

        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("model picker should be open");
        let model_idx = picker
            .entries
            .iter()
            .position(|m| m.name == "amazon.nova-pro-v1:0")
            .expect("Bedrock model should be in picker");
        let filtered_pos = picker
            .filtered
            .iter()
            .position(|&i| i == model_idx)
            .expect("Bedrock model should be in filtered list");
        app.inline_interactive_state.as_mut().unwrap().selected = filtered_pos;

        // Ctrl+O replaced Ctrl+B so the picker no longer steals tmux's prefix.
        app.handle_key(KeyCode::Char('o'), KeyModifiers::CONTROL)
            .unwrap();

        let cfg = crate::config::Config::load();
        assert_eq!(
            cfg.provider.default_model.as_deref(),
            Some("bedrock:amazon.nova-pro-v1:0")
        );
        assert_eq!(cfg.provider.default_provider.as_deref(), Some("bedrock"));
    });
}

#[test]
fn test_handle_key_cursor_movement() {
    let mut app = create_test_app();

    app.handle_key(KeyCode::Char('a'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('b'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('c'), KeyModifiers::empty())
        .unwrap();

    assert_eq!(app.cursor_pos(), 3);

    app.handle_key(KeyCode::Left, KeyModifiers::empty())
        .unwrap();
    assert_eq!(app.cursor_pos(), 2);

    app.handle_key(KeyCode::Home, KeyModifiers::empty())
        .unwrap();
    assert_eq!(app.cursor_pos(), 0);

    app.handle_key(KeyCode::End, KeyModifiers::empty()).unwrap();
    assert_eq!(app.cursor_pos(), 3);
}

#[test]
fn test_handle_key_ctrl_word_movement_and_delete() {
    let mut app = create_test_app();
    app.set_input_for_test("hello world again");

    app.handle_key(KeyCode::Left, KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.cursor_pos(), "hello world ".len());

    app.handle_key(KeyCode::Left, KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.cursor_pos(), "hello ".len());

    app.handle_key(KeyCode::Right, KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.cursor_pos(), "hello world ".len());

    app.handle_key(KeyCode::Backspace, KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.input(), "hello again");
    assert_eq!(app.cursor_pos(), "hello ".len());
}

#[test]
fn test_handle_key_ctrl_backspace_csi_u_char_fallback_deletes_word() {
    let mut app = create_test_app();
    app.set_input_for_test("hello world again");

    app.handle_key(KeyCode::Char('\u{8}'), KeyModifiers::CONTROL)
        .unwrap();

    assert_eq!(app.input(), "hello world ");
    assert_eq!(app.cursor_pos(), "hello world ".len());
}

#[test]
fn test_handle_key_super_backspace_deletes_previous_word() {
    let mut app = create_test_app();
    app.set_input_for_test("hello world again");

    app.handle_key(KeyCode::Left, KeyModifiers::CONTROL)
        .unwrap();
    app.handle_key(KeyCode::Backspace, KeyModifiers::SUPER)
        .unwrap();

    // Cmd+Backspace deletes the previous word, leaving the cursor at the new boundary.
    assert_eq!(app.input(), "hello again");
    assert_eq!(app.cursor_pos(), "hello ".len());
}

#[test]
fn test_handle_key_super_delete_aliases_delete_previous_word() {
    for code in [KeyCode::Delete, KeyCode::Char('\u{7f}')] {
        let mut app = create_test_app();
        app.set_input_for_test("hello world again");

        app.handle_key(KeyCode::Left, KeyModifiers::CONTROL)
            .unwrap();
        app.handle_key(code, KeyModifiers::SUPER).unwrap();

        assert_eq!(app.input(), "hello again");
        assert_eq!(app.cursor_pos(), "hello ".len());
    }
}

#[test]
fn test_handle_key_alt_delete_aliases_delete_previous_word() {
    for code in [KeyCode::Backspace, KeyCode::Delete, KeyCode::Char('\u{7f}')] {
        let mut app = create_test_app();
        app.set_input_for_test("hello world again");

        app.handle_key(KeyCode::Left, KeyModifiers::CONTROL)
            .unwrap();
        app.handle_key(code, KeyModifiers::ALT).unwrap();

        assert_eq!(app.input(), "hello again");
        assert_eq!(app.cursor_pos(), "hello ".len());
    }
}

#[test]
fn test_remote_super_backspace_deletes_previous_word() {
    let mut app = create_test_app();
    app.set_input_for_test("hello world again");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_key(KeyCode::Left, KeyModifiers::CONTROL)
        .unwrap();
    rt.block_on(app.handle_remote_key(KeyCode::Backspace, KeyModifiers::SUPER, &mut remote))
        .unwrap();

    assert_eq!(app.input(), "hello again");
    assert_eq!(app.cursor_pos(), "hello ".len());
}

#[test]
fn test_handle_key_ctrl_k_deletes_to_end() {
    let mut app = create_test_app();
    app.set_input_for_test("hello world again");

    app.handle_key(KeyCode::Left, KeyModifiers::CONTROL)
        .unwrap();
    app.handle_key(KeyCode::Char('k'), KeyModifiers::CONTROL)
        .unwrap();

    assert_eq!(app.input(), "hello world ");
    assert_eq!(app.cursor_pos(), "hello world ".len());
}

#[test]
fn test_handle_key_super_left_right_move_to_edges() {
    let mut app = create_test_app();
    app.set_input_for_test("hello world");

    if cfg!(target_os = "macos") {
        // On macOS, Cmd+Left/Right default to effort cycling, so the cursor
        // must NOT move; Home/End still jump to the edges.
        let before = app.cursor_pos();
        app.handle_key(KeyCode::Left, KeyModifiers::SUPER).unwrap();
        assert_eq!(app.cursor_pos(), before);

        app.handle_key(KeyCode::Home, KeyModifiers::empty()).unwrap();
        assert_eq!(app.cursor_pos(), 0);

        app.handle_key(KeyCode::End, KeyModifiers::empty()).unwrap();
        assert_eq!(app.cursor_pos(), "hello world".len());
    } else {
        app.handle_key(KeyCode::Left, KeyModifiers::SUPER).unwrap();
        assert_eq!(app.cursor_pos(), 0);

        app.handle_key(KeyCode::Right, KeyModifiers::SUPER).unwrap();
        assert_eq!(app.cursor_pos(), "hello world".len());
    }
}

#[test]
fn test_handle_key_alt_left_right_move_by_word() {
    // On non-macOS platforms Alt+Left/Right default to effort cycling, so the
    // word-move behavior only applies where Cmd+Left/Right own effort cycling.
    if !cfg!(target_os = "macos") {
        return;
    }
    let mut app = create_test_app();
    app.set_input_for_test("hello world");

    app.handle_key(KeyCode::Left, KeyModifiers::ALT).unwrap();
    assert_eq!(app.cursor_pos(), "hello ".len());

    app.handle_key(KeyCode::Left, KeyModifiers::ALT).unwrap();
    assert_eq!(app.cursor_pos(), 0);

    // Forward lands at the start of the next word, matching Alt+F.
    app.handle_key(KeyCode::Right, KeyModifiers::ALT).unwrap();
    assert_eq!(app.cursor_pos(), "hello ".len());

    app.handle_key(KeyCode::Right, KeyModifiers::ALT).unwrap();
    assert_eq!(app.cursor_pos(), "hello world".len());
}

#[test]
fn test_handle_key_super_z_undoes_input_change() {
    let mut app = create_test_app();

    app.handle_key(KeyCode::Char('a'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('b'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('z'), KeyModifiers::SUPER)
        .unwrap();

    assert_eq!(app.input(), "a");
    assert_eq!(app.cursor_pos(), 1);
}

#[test]
fn test_handle_key_ctrl_h_does_not_insert_text() {
    let mut app = create_test_app();
    app.set_input_for_test("hello");

    app.handle_key(KeyCode::Char('h'), KeyModifiers::CONTROL)
        .unwrap();

    assert_eq!(app.input(), "hello");
    assert_eq!(app.cursor_pos(), "hello".len());
}

#[test]
fn test_handle_key_escape_clears_input() {
    let mut app = create_test_app();

    app.handle_key(KeyCode::Char('t'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('e'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('s'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('t'), KeyModifiers::empty())
        .unwrap();

    assert_eq!(app.input(), "test");

    app.handle_key(KeyCode::Esc, KeyModifiers::empty()).unwrap();

    assert!(app.input().is_empty());
    assert_eq!(app.cursor_pos(), 0);
    assert_eq!(
        app.status_notice(),
        Some("Input cleared - Ctrl+Z to restore".to_string())
    );
}

#[test]
fn test_handle_key_ctrl_z_restores_escaped_input() {
    let mut app = create_test_app();

    app.handle_key(KeyCode::Char('t'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('e'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('s'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('t'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Esc, KeyModifiers::empty()).unwrap();

    app.handle_key(KeyCode::Char('z'), KeyModifiers::CONTROL)
        .unwrap();

    assert_eq!(app.input(), "test");
    assert_eq!(app.cursor_pos(), 4);
    assert_eq!(app.status_notice(), Some("↶ Input restored".to_string()));
}

#[test]
fn test_handle_key_ctrl_z_undoes_typing() {
    let mut app = create_test_app();

    app.handle_key(KeyCode::Char('a'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('b'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('c'), KeyModifiers::empty())
        .unwrap();

    app.handle_key(KeyCode::Char('z'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.input(), "ab");
    assert_eq!(app.cursor_pos(), 2);

    app.handle_key(KeyCode::Char('z'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.input(), "a");
    assert_eq!(app.cursor_pos(), 1);
}

#[test]
fn test_handle_key_ctrl_u_clears_input() {
    let mut app = create_test_app();

    app.handle_key(KeyCode::Char('t'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('e'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('s'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('t'), KeyModifiers::empty())
        .unwrap();

    app.handle_key(KeyCode::Char('u'), KeyModifiers::CONTROL)
        .unwrap();

    assert!(app.input().is_empty());
    assert_eq!(app.cursor_pos(), 0);
}

#[test]
fn test_submit_input_adds_message() {
    let mut app = create_test_app();

    // Type and submit
    app.handle_key(KeyCode::Char('h'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('i'), KeyModifiers::empty())
        .unwrap();
    app.submit_input();

    // Check message was added to display
    assert_eq!(app.display_messages().len(), 1);
    assert_eq!(app.display_messages()[0].role, "user");
    assert_eq!(app.display_messages()[0].content, "hi");

    // Check processing state
    assert!(app.is_processing());
    assert!(app.pending_turn);
    assert!(app.session_save_pending);
    assert!(matches!(app.status(), ProcessingStatus::Sending));
    assert!(app.elapsed().is_some());

    // Input should be cleared
    assert!(app.input().is_empty());
}

#[test]
fn test_submit_input_commits_pending_streaming_assistant_text_before_user_message() {
    let mut app = create_test_app();
    app.display_messages.push(DisplayMessage::tool(
        "file contents",
        crate::message::ToolCall {
            id: "tool_read".to_string(),
            name: "read".to_string(),
            input: serde_json::json!({"file_path": "src/main.rs"}),
            intent: None, thought_signature: None, },
    ));
    app.bump_display_messages_version();
    app.streaming.streaming_text = "Here is the final paragraph".to_string();
    // Mirror the real streaming caller: append any paced chunk the buffer reveals.
    // The paced StreamBuffer may reveal part of the text immediately, so commit
    // (below) must still flush the remainder.
    let ops = app.stream_buffer.push_text(" that was still buffered.");
    app.apply_stream_ops(ops);

    app.input = "follow up".to_string();
    app.cursor_pos = app.input.len();
    app.submit_input();

    assert_eq!(app.display_messages().len(), 3);
    assert_eq!(app.display_messages()[0].role, "tool");
    assert_eq!(app.display_messages()[1].role, "assistant");
    assert_eq!(
        app.display_messages()[1].content,
        "Here is the final paragraph that was still buffered."
    );
    assert_eq!(app.display_messages()[2].role, "user");
    assert_eq!(app.display_messages()[2].content, "follow up");
    assert!(app.streaming_text().is_empty());
    assert!(app.stream_buffer.is_empty());
}

#[test]
fn test_queue_message_while_processing() {
    let mut app = create_test_app();
    app.queue_mode = true;

    // Simulate processing state
    app.is_processing = true;

    // Type a message
    app.handle_key(KeyCode::Char('t'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('e'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('s'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('t'), KeyModifiers::empty())
        .unwrap();

    // Press Enter should queue, not submit
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    assert_eq!(app.queued_count(), 1);
    assert!(app.input().is_empty());

    // Queued messages are stored in queued_messages, not display_messages
    assert_eq!(app.queued_messages()[0], "test");
    assert!(app.display_messages().is_empty());
}

#[test]
fn test_ctrl_tab_toggles_queue_mode() {
    let mut app = create_test_app();

    assert!(!app.queue_mode);

    app.handle_key(KeyCode::Char('t'), KeyModifiers::CONTROL)
        .unwrap();
    assert!(app.queue_mode);

    app.handle_key(KeyCode::Char('t'), KeyModifiers::CONTROL)
        .unwrap();
    assert!(!app.queue_mode);
}

#[test]
fn test_auto_poke_starts_enabled_by_default() {
    let app = create_test_app();

    assert!(app.auto_poke_incomplete_todos);
}

#[test]
fn test_ctrl_p_toggles_auto_poke_locally() {
    let mut app = create_test_app();

    assert!(app.auto_poke_incomplete_todos);

    app.handle_key(KeyCode::Char('p'), KeyModifiers::CONTROL)
        .unwrap();
    assert!(!app.auto_poke_incomplete_todos);
    assert_eq!(app.status_notice(), Some("Poke: OFF".to_string()));

    app.handle_key(KeyCode::Char('p'), KeyModifiers::CONTROL)
        .unwrap();
    assert!(app.auto_poke_incomplete_todos);
    assert_eq!(app.status_notice(), Some("Poke: ON".to_string()));
    assert!(app.display_messages().iter().any(|msg| {
        msg.content
            .contains("Auto-poke enabled. No incomplete todos found right now.")
    }));
}

#[test]
fn test_transfer_command_queues_pause_while_processing_locally() {
    let mut app = create_test_app();
    app.is_processing = true;

    super::commands::handle_transfer_command_local(&mut app);

    assert!(app.pending_transfer_request);
    let pause_message = super::commands::transfer_pause_message();
    assert_eq!(
        app.interleave_message.as_deref(),
        Some(pause_message.as_str())
    );
    assert_eq!(
        app.status_notice(),
        Some("Transfer queued after current turn".to_string())
    );
}

#[test]
fn test_create_transfer_session_from_parent_copies_todos_and_uses_compacted_context_only() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.session.working_dir = Some("/tmp".to_string());
        app.session.model = Some("test-model".to_string());
        app.session.provider_key = Some("test-provider".to_string());
        app.session.messages.push(crate::session::StoredMessage {
            id: "msg-1".to_string(),
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "full transcript should not be copied".to_string(),
                cache_control: None,
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        });
        let transfer_compaction = crate::session::StoredCompactionState {
            summary_text: "Compacted handoff summary".to_string(),
            openai_encrypted_content: None,
            covers_up_to_turn: 1,
            original_turn_count: 1,
            compacted_count: 0,
        };
        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                group: None,
                id: "todo-1".to_string(),
                content: "Carry this forward".to_string(),
                status: "pending".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
                confidence: None,
                completion_confidence: None,
                confidence_history: Vec::new(),
            }],
        )
        .expect("save todos");

        let (child_id, _) = super::commands::create_transfer_session_from_parent(
            &app.session.id,
            &app.session,
            Some(transfer_compaction.clone()),
        )
        .expect("create transfer session");
        let child = crate::session::Session::load(&child_id).expect("load child session");
        let child_todos = crate::todo::load_todos(&child_id).expect("load child todos");

        assert_eq!(child.parent_id.as_deref(), Some(app.session.id.as_str()));
        assert!(child.messages.is_empty());
        assert_eq!(child.compaction, Some(transfer_compaction));
        assert_eq!(child.model.as_deref(), Some("test-model"));
        assert_eq!(child.provider_key.as_deref(), Some("test-provider"));
        assert_eq!(child.working_dir.as_deref(), Some("/tmp"));
        assert_eq!(child_todos.len(), 1);
        assert_eq!(child_todos[0].content, "Carry this forward");
    });
}

#[test]
fn test_shift_enter_inserts_newline() {
    let mut app = create_test_app();
    app.is_processing = true;

    app.handle_key(KeyCode::Char('h'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Enter, KeyModifiers::SHIFT).unwrap();
    app.handle_key(KeyCode::Char('i'), KeyModifiers::empty())
        .unwrap();

    assert_eq!(app.input(), "h\ni");
    assert_eq!(app.queued_count(), 0);
    assert_eq!(app.interleave_message.as_deref(), None);
}

#[test]
fn test_alt_enter_inserts_newline() {
    let mut app = create_test_app();
    app.is_processing = true;

    app.handle_key(KeyCode::Char('h'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Enter, KeyModifiers::ALT).unwrap();
    app.handle_key(KeyCode::Char('i'), KeyModifiers::empty())
        .unwrap();

    assert_eq!(app.input(), "h\ni");
    assert_eq!(app.queued_count(), 0);
    assert_eq!(app.interleave_message.as_deref(), None);
}
#[test]
fn test_ctrl_enter_opposite_send_mode() {
    let mut app = create_test_app();
    app.is_processing = true;

    // Default immediate mode: Ctrl+Enter should queue
    app.handle_key(KeyCode::Char('h'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('i'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Enter, KeyModifiers::CONTROL)
        .unwrap();

    assert_eq!(app.queued_count(), 1);
    assert_eq!(app.interleave_message.as_deref(), None);
    assert!(app.input().is_empty());

    // Queue mode: Ctrl+Enter should interleave (sets interleave_message, not queued)
    app.queue_mode = true;
    app.handle_key(KeyCode::Char('y'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('o'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Enter, KeyModifiers::CONTROL)
        .unwrap();

    // Interleave now sets interleave_message instead of adding to queue
    assert_eq!(app.queued_count(), 1); // Still just "hi" in queue
    assert_eq!(app.interleave_message.as_deref(), Some("yo")); // "yo" is for interleave
}

#[test]
fn test_cmd_enter_opposite_send_mode() {
    let mut app = create_test_app();
    app.is_processing = true;

    // Default immediate mode: Cmd+Enter should queue, matching Ctrl+Enter
    app.handle_key(KeyCode::Char('h'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('i'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Enter, KeyModifiers::SUPER).unwrap();

    assert_eq!(app.queued_count(), 1);
    assert_eq!(app.interleave_message.as_deref(), None);
    assert!(app.input().is_empty());

    // Queue mode: Cmd+Enter should interleave (sets interleave_message, not queued)
    app.queue_mode = true;
    app.handle_key(KeyCode::Char('y'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('o'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Enter, KeyModifiers::SUPER).unwrap();

    assert_eq!(app.queued_count(), 1); // Still just "hi" in queue
    assert_eq!(app.interleave_message.as_deref(), Some("yo")); // "yo" is for interleave
}

#[test]
fn test_typing_during_processing() {
    let mut app = create_test_app();
    app.is_processing = true;

    // Should still be able to type
    app.handle_key(KeyCode::Char('a'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('b'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('c'), KeyModifiers::empty())
        .unwrap();

    assert_eq!(app.input(), "abc");
}

#[test]
fn test_ctrl_c_requests_cancel_while_processing() {
    let mut app = create_test_app();
    app.is_processing = true;
    app.interleave_message = Some("queued interrupt".to_string());
    app.pending_soft_interrupts
        .push("pending soft interrupt".to_string());

    app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL)
        .unwrap();

    assert!(app.cancel_requested);
    assert!(app.interleave_message.is_none());
    assert!(app.pending_soft_interrupts.is_empty());
    assert_eq!(app.status_notice(), Some("Interrupting...".to_string()));
}

#[test]
fn test_escape_interrupt_disables_auto_poke_while_processing() {
    let mut app = create_test_app();
    app.is_processing = true;
    app.auto_poke_incomplete_todos = true;
    app.queued_messages
        .push(super::commands::build_poke_message(&[
            crate::todo::TodoItem {
                group: None,
                id: "todo-1".to_string(),
                content: "keep going".to_string(),
                status: "pending".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
                confidence: None,
                completion_confidence: None,
                confidence_history: Vec::new(),
            },
        ]));

    app.handle_key(KeyCode::Esc, KeyModifiers::empty()).unwrap();

    assert!(app.cancel_requested);
    assert!(!app.auto_poke_incomplete_todos);
    assert!(app.queued_messages.is_empty());
    assert_eq!(
        app.status_notice(),
        Some("Interrupting... Auto-poke OFF".to_string())
    );
}

#[test]
fn test_ctrl_c_still_arms_quit_when_idle() {
    let mut app = create_test_app();

    app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL)
        .unwrap();

    assert!(!app.cancel_requested);
    assert!(app.quit_pending.is_some());
    assert_eq!(
        app.status_notice(),
        Some("Press Ctrl+C again to quit".to_string())
    );
}

#[test]
fn test_ctrl_x_cuts_entire_input_line_to_clipboard() {
    let mut app = create_test_app();
    app.input = "hello world".to_string();
    app.cursor_pos = 5;

    let copied = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let copied_for_closure = copied.clone();

    let cut = super::input::cut_input_line_to_clipboard_with(&mut app, |text| {
        *copied_for_closure.lock().unwrap() = text.to_string();
        true
    });

    assert!(cut);
    assert_eq!(&*copied.lock().unwrap(), "hello world");
    assert!(app.input().is_empty());
    assert_eq!(app.cursor_pos(), 0);
    assert_eq!(app.status_notice(), Some("✂ Cut input line".to_string()));

    app.handle_key(KeyCode::Char('z'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.input(), "hello world");
    assert_eq!(app.cursor_pos(), 5);
}

#[test]
fn test_ctrl_x_preserves_input_when_clipboard_copy_fails() {
    let mut app = create_test_app();
    app.input = "hello world".to_string();
    app.cursor_pos = 5;

    let cut = super::input::cut_input_line_to_clipboard_with(&mut app, |_text| false);

    assert!(!cut);
    assert_eq!(app.input(), "hello world");
    assert_eq!(app.cursor_pos(), 5);
    assert_eq!(
        app.status_notice(),
        Some("Failed to copy input line".to_string())
    );
}

#[test]
fn test_ctrl_a_keeps_home_behavior_when_input_present() {
    let mut app = create_test_app();
    app.input = "hello world".to_string();
    app.cursor_pos = app.input.len();

    app.handle_key(KeyCode::Char('a'), KeyModifiers::CONTROL)
        .unwrap();

    assert_eq!(app.input(), "hello world");
    assert_eq!(app.cursor_pos(), 0);
}

#[test]
fn test_retrieve_pending_message_edits_queued_message() {
    let mut app = create_test_app();
    app.queue_mode = true;
    app.is_processing = true;

    // Type and queue a message
    app.handle_key(KeyCode::Char('h'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('e'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('l'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('l'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('o'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    assert_eq!(app.queued_count(), 1);
    assert!(app.input().is_empty());

    app.handle_key(KeyCode::Up, KeyModifiers::CONTROL).unwrap();

    assert_eq!(app.queued_count(), 0);
    assert_eq!(app.input(), "hello");
    assert_eq!(app.cursor_pos(), 5); // Cursor at end
}

#[test]
fn test_retrieve_pending_message_with_alt_and_super_up() {
    // Ctrl+Up, Alt(Option)+Up and Cmd(Super)+Up must all recall a queued message
    // so the gesture works regardless of which modifier the terminal forwards.
    for modifier in [
        KeyModifiers::CONTROL,
        KeyModifiers::ALT,
        KeyModifiers::SUPER,
    ] {
        let mut app = create_test_app();
        app.queue_mode = true;
        app.is_processing = true;

        for c in "hello".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
                .unwrap();
        }
        app.handle_key(KeyCode::Enter, KeyModifiers::empty())
            .unwrap();

        assert_eq!(app.queued_count(), 1, "modifier {modifier:?}");
        assert!(app.input().is_empty(), "modifier {modifier:?}");

        app.handle_key(KeyCode::Up, modifier).unwrap();

        assert_eq!(app.queued_count(), 0, "modifier {modifier:?}");
        assert_eq!(app.input(), "hello", "modifier {modifier:?}");
        assert_eq!(app.cursor_pos(), 5, "modifier {modifier:?}");
    }
}

#[test]
fn test_retrieve_pending_message_prefers_pending_interleave_for_editing() {
    let mut app = create_test_app();
    app.is_processing = true;
    app.queue_mode = false; // Enter=interleave, Ctrl+Enter=queue

    for c in "urgent".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    for c in "later".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }
    app.handle_key(KeyCode::Enter, KeyModifiers::CONTROL)
        .unwrap();

    assert_eq!(app.interleave_message.as_deref(), Some("urgent"));
    assert_eq!(app.queued_count(), 1);

    app.retrieve_pending_message_for_edit();

    assert_eq!(app.input(), "urgent\n\nlater");
    assert_eq!(app.interleave_message.as_deref(), None);
    assert_eq!(app.queued_count(), 0);
}

#[test]
fn test_send_action_modes() {
    let mut app = create_test_app();
    app.is_processing = true;
    app.queue_mode = false;

    assert_eq!(app.send_action(false), SendAction::Interleave);
    assert_eq!(app.send_action(true), SendAction::Queue);

    app.queue_mode = true;
    assert_eq!(app.send_action(false), SendAction::Queue);
    assert_eq!(app.send_action(true), SendAction::Interleave);

    app.is_processing = false;
    assert_eq!(app.send_action(false), SendAction::Submit);
}

#[test]
fn test_send_action_submits_bang_commands_while_processing() {
    let mut app = create_test_app();
    app.is_processing = true;
    app.input = "!pwd".to_string();

    assert_eq!(app.send_action(false), SendAction::Submit);
    assert_eq!(app.send_action(true), SendAction::Submit);
}

#[test]
fn test_handle_input_shell_completed_renders_markdown_blocks() {
    let mut app = create_test_app();
    let event = BusEvent::InputShellCompleted(InputShellCompleted {
        session_id: app.session.id.clone(),
        result: crate::message::InputShellResult {
            command: "ls -la".to_string(),
            cwd: Some("/tmp/project".to_string()),
            output: "Cargo.toml\nsrc\n".to_string(),
            exit_code: Some(0),
            duration_ms: 42,
            truncated: false,
            failed_to_start: false,
        },
    });

    super::local::handle_bus_event(&mut app, Ok(event));

    let rendered = app.display_messages().last().expect("shell result message");
    assert_eq!(rendered.role, "system");
    assert!(rendered.content.contains("Shell command"));
    assert!(rendered.content.contains("ls -la"));
    assert!(rendered.content.contains("Cargo.toml"));
    assert_eq!(
        app.status_notice(),
        Some("Shell command completed".to_string())
    );
}

/// Regression for issue #427: selecting an effort-variant model row (e.g.
/// "gpt-5.5 (high)") in the remote model picker must stage the chosen effort
/// alongside the pending model switch. Previously only the model spec was
/// staged, so the server kept its configured default effort (low) and the
/// session silently ran gpt-5.5 at low effort.
#[test]
fn test_model_picker_effort_variant_selection_stages_effort_in_remote_mode() {
    let mut app = create_test_app();
    configure_test_remote_models_with_openai_recommendations(&mut app);

    app.open_model_picker();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");

    let entry_idx = picker
        .entries
        .iter()
        .position(|m| m.name == "gpt-5.5 (high)")
        .expect("gpt-5.5 (high) should be in picker");
    assert_eq!(
        picker.entries[entry_idx].effort.as_deref(),
        Some("high"),
        "effort variant rows must carry their effort"
    );

    let filtered_pos = picker
        .filtered
        .iter()
        .position(|&i| i == entry_idx)
        .expect("gpt-5.5 (high) should be in filtered list");
    app.inline_interactive_state.as_mut().unwrap().selected = filtered_pos;

    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    assert!(app.inline_interactive_state.is_none(), "picker should close");
    assert!(
        app.pending_route_selection.is_some(),
        "model switch should be staged for the remote dispatcher"
    );
    assert_eq!(
        app.pending_reasoning_effort.as_deref(),
        Some("high"),
        "the picked effort variant must be staged so it reaches the server (issue #427)"
    );
}

#[test]
fn test_model_picker_effort_variants_follow_each_route_vocabulary() {
    let mut app = create_test_app();
    configure_test_remote_models_with_openai_recommendations(&mut app);
    app.remote_model_options.push(crate::provider::ModelRoute {
        model: "gpt-5.5".to_string(),
        provider: "OpenRouter".to_string(),
        api_method: "openrouter".to_string(),
        available: true,
        detail: String::new(),
        cheapness: None,
    });

    app.open_model_picker();
    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");
    let has_route_effort = |api_method: &str, effort: &str| {
        picker.entries.iter().any(|entry| {
            entry.name.starts_with("gpt-5.5 (")
                && entry.effort.as_deref() == Some(effort)
                && entry
                    .options
                    .first()
                    .is_some_and(|route| route.api_method == api_method)
        })
    };

    assert!(has_route_effort("openai-oauth", "max"));
    assert!(has_route_effort("openai-oauth", "minimal"));
    assert!(has_route_effort("openrouter", "xhigh"));
    assert!(has_route_effort("openrouter", "minimal"));
    assert!(
        !has_route_effort("openrouter", "max"),
        "OpenRouter must not advertise max as a distinct rung because it aliases xhigh"
    );
}

/// Plain model rows (no effort suffix) must not stage a reasoning effort.
/// Routes whose runtime cannot apply a reasoning effort (e.g. Copilot) get
/// plain rows even for models that have an effort ladder elsewhere.
#[test]
fn test_model_picker_plain_selection_stages_no_effort_in_remote_mode() {
    let mut app = create_test_app();
    configure_test_remote_models_with_openai_recommendations(&mut app);
    // A Copilot-backed route cannot apply per-request reasoning effort, so it
    // must render as a plain row (issue #458 route gating).
    app.remote_model_options.push(crate::provider::ModelRoute {
        model: "claude-opus-4-8".to_string(),
        provider: "Copilot".to_string(),
        api_method: "copilot".to_string(),
        available: true,
        detail: String::new(),
        cheapness: None,
    });

    app.open_model_picker();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");

    let entry_idx = picker
        .entries
        .iter()
        .position(|m| m.name == "claude-opus-4-8" && m.effort.is_none())
        .expect("claude-opus-4-8 should be in picker without an effort variant");

    let filtered_pos = picker
        .filtered
        .iter()
        .position(|&i| i == entry_idx)
        .expect("claude-opus-4-8 should be in filtered list");
    app.inline_interactive_state.as_mut().unwrap().selected = filtered_pos;

    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    assert!(app.inline_interactive_state.is_none(), "picker should close");
    assert!(
        app.pending_reasoning_effort.is_none(),
        "plain rows must not override the server's effort"
    );
}
