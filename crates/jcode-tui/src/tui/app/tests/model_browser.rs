// `/model` + Enter opens a browsable, searchable model picker instead of
// switching to the first listed model. `@provider` tokens and Ctrl+P scope
// the list to one provider. `/model <name>` still switches directly.

#[test]
fn clearing_model_search_preserves_selected_identity_and_route() {
    let mut app = create_test_app();
    app.inline_interactive_state = Some(model_browser_state(false));
    type_into(&mut app, "opus");
    {
        let picker = app.inline_interactive_state.as_mut().unwrap();
        let index = picker.selected_entry_index().unwrap();
        picker.choose_route(index, 1);
        picker.column = picker.max_navigable_column();
    }
    press(&mut app, KeyCode::Esc);
    let picker = app.inline_interactive_state.as_ref().unwrap();
    assert!(picker.filter.is_empty());
    let selected = picker.selected_entry().unwrap();
    assert_eq!(selected.name, "claude-opus-4-8");
    assert_eq!(selected.active_option().unwrap().api_method, "openrouter");
    assert_eq!(picker.column, picker.max_navigable_column());
    assert!(app.pending_route_selection.is_none());
}

#[test]
fn backspace_clearing_model_search_preserves_selected_identity() {
    let mut app = create_test_app();
    app.inline_interactive_state = Some(model_browser_state(false));
    type_into(&mut app, "v");
    assert_eq!(selected_row_name(&app), "deepseek-v4");
    press(&mut app, KeyCode::Backspace);
    assert_eq!(selected_row_name(&app), "deepseek-v4");
    assert!(app.inline_interactive_state.as_ref().unwrap().filter.is_empty());
}

#[test]
fn backspace_broadening_model_search_preserves_selected_identity_and_route() {
    let mut app = create_test_app();
    let mut picker = model_browser_state(false);
    picker.entries = vec![
        model_browser_entry(
            "ux-alpha",
            &[("OpenAI-compatible", "openai-compatible:openai-compatible")],
        ),
        model_browser_entry(
            "ux-beta",
            &[
                ("OpenAI-compatible", "openai-compatible:openai-compatible"),
                ("OpenRouter", "openrouter"),
            ],
        ),
    ];
    picker.entries[0].is_current = true;
    picker.entries[0].usage_score = 100;
    picker.filtered = vec![0, 1];
    app.inline_interactive_state = Some(picker);
    type_into(&mut app, "ux-beta");
    {
        let picker = app.inline_interactive_state.as_mut().unwrap();
        let index = picker.selected_entry_index().unwrap();
        picker.choose_route(index, 1);
    }
    for _ in 0.."ux-beta".len() {
        press(&mut app, KeyCode::Backspace);
        let picker = app.inline_interactive_state.as_ref().unwrap();
        let selected = picker.selected_entry().unwrap();
        assert_eq!(selected.name, "ux-beta", "filter={}", picker.filter);
        assert_eq!(selected.active_option().unwrap().api_method, "openrouter");
    }
    assert!(
        app.inline_interactive_state
            .as_ref()
            .unwrap()
            .filter
            .is_empty()
    );
}

#[test]
fn empty_model_search_preview_enter_preserves_query() {
    for direct_activation in [false, true] {
        let mut app = create_test_app();
        configure_available_model_browser_routes(&mut app);
        let input = "/model zzz-not-found";
        type_into(&mut app, input);
        assert!(app.inline_interactive_state.as_ref().unwrap().filtered.is_empty());
        if direct_activation {
            assert!(app.activate_picker_from_preview());
        } else {
            press(&mut app, KeyCode::Enter);
        }
        let picker = app.inline_interactive_state.as_ref().expect("empty model preview stays open");
        assert!(picker.preview);
        assert!(picker.filtered.is_empty());
        assert_eq!(picker.filter, "zzz-not-found");
        assert_eq!(app.input(), input);
        assert!(app.pending_route_selection.is_none());
    }
}

#[test]
fn unavailable_model_enter_keeps_search_and_explains_why() {
    let mut app = create_test_app();
    let mut picker = model_browser_state(false);
    picker.entries[0].options[0].available = false;
    picker.entries[0].options[0].detail = "Sign in to OpenAI".to_string();
    app.inline_interactive_state = Some(picker);
    type_into(&mut app, "gpt");
    press(&mut app, KeyCode::Enter);
    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("unavailable stays open");
    assert_eq!(picker.filter, "gpt");
    assert_eq!(picker.selected_entry().unwrap().name, "gpt-5.5");
    assert!(picker.effort_step.is_none());
    let notice = app.status_notice().unwrap();
    assert!(notice.contains("unavailable"), "{notice}");
    assert!(notice.contains("Sign in to OpenAI"), "{notice}");
    assert!(app.pending_route_selection.is_none());
}

#[test]
fn unavailable_model_cannot_be_saved_as_default() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        let mut picker = model_browser_state(false);
        picker.entries[0].options[0].available = false;
        app.inline_interactive_state = Some(picker);
        app.handle_key(KeyCode::Char('o'), KeyModifiers::CONTROL)
            .unwrap();
        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("unavailable stays open");
        assert!(picker.effort_step.is_none());
        assert!(
            crate::config::Config::load()
                .provider
                .default_model
                .is_none()
        );
        assert!(app.status_notice().unwrap().contains("unavailable"));
    });
}

#[test]
fn initial_model_selection_prefers_current_then_available_default_then_available() {
    let mut entries = model_browser_state(false).entries;
    entries[0].options[0].available = false;
    entries[0].is_default = true;
    assert_eq!(
        super::inline_interactive::model_picker_initial_selection(&entries),
        1
    );
    entries[2].is_default = true;
    assert_eq!(
        super::inline_interactive::model_picker_initial_selection(&entries),
        2
    );
    let mut preview = model_browser_state(true);
    preview.entries = entries.clone();
    preview.selected = 2;
    assert!(super::inline_interactive::picker_enter_should_focus_model_browser(&preview));
    entries[0].is_current = true;
    assert_eq!(
        super::inline_interactive::model_picker_initial_selection(&entries),
        0
    );
    for entry in &mut entries {
        entry.is_current = false;
        for option in &mut entry.options {
            option.available = false;
        }
    }
    assert_eq!(
        super::inline_interactive::model_picker_initial_selection(&entries),
        0
    );
}

/// These interaction tests need real, selectable routes. A names-only
/// catalog is intentionally conservative about availability and cannot prove
/// that Enter actually stages a switch.
fn configure_available_model_browser_routes(app: &mut App) {
    configure_test_remote_models(app);
    app.remote_provider_name = Some("OpenAI".to_string());
    app.remote_model_options = app.remote_available_entries.iter().map(|model| crate::provider::ModelRoute {
        catalog_metadata: None,
        model: model.clone(),
        provider: "OpenAI".to_string(),
        api_method: "openai-oauth".to_string(),
        available: true,
        detail: String::new(),
        usage: None,
        cheapness: None,
    }).collect();
}

fn model_browser_entry(name: &str, routes: &[(&str, &str)]) -> crate::tui::PickerEntry {
    crate::tui::PickerEntry {
        name: name.to_string(),
        options: routes
            .iter()
            .map(|(provider, api_method)| crate::tui::PickerOption {
                provider: provider.to_string(),
                api_method: api_method.to_string(),
                available: true,
                detail: String::new(),
                estimated_reference_cost_micros: None,
            })
            .collect(),
        action: crate::tui::PickerAction::Model,
        selected_option: 0,
        is_current: false,
        is_default: false,
        is_favorite: false,
        recommended: false,
        recommendation_rank: usize::MAX,
        usage_score: 0,
        old: false,
        created_date: None,
        effort: None,
    }
}

fn model_browser_state(preview: bool) -> crate::tui::InlineInteractiveState {
    let entries = vec![
        model_browser_entry("gpt-5.5", &[("OpenAI", "openai-oauth")]),
        model_browser_entry(
            "claude-opus-4-8",
            &[("Anthropic", "claude-oauth"), ("Fireworks", "openrouter")],
        ),
        model_browser_entry("gemini-3-pro", &[("Gemini", "code-assist-oauth")]),
        model_browser_entry("deepseek-v4", &[("DeepSeek", "openrouter")]),
    ];
    crate::tui::InlineInteractiveState {
        kind: crate::tui::PickerKind::Model,
        filtered: (0..entries.len()).collect(),
        entries,
        selected: 0,
        column: 0,
        filter: String::new(),
        preview,
        scoped_route_restore: Vec::new(),
        effort_step: None,
    }
}

fn visible_model_names(app: &App) -> Vec<String> {
    let picker = app.inline_interactive_state.as_ref().expect("picker open");
    picker
        .filtered
        .iter()
        .map(|&i| picker.entries[i].name.clone())
        .collect()
}

fn type_into(app: &mut App, text: &str) {
    for c in text.chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }
}

#[test]
fn bare_model_enter_opens_focused_browser_instead_of_switching() {
    let mut app = create_test_app();
    configure_available_model_browser_routes(&mut app);
    let model_before = app.remote_provider_model.clone();

    type_into(&mut app, "/model");
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("bare /model + Enter must keep the picker open");
    assert!(!picker.preview, "picker should be focused for browsing");
    assert_eq!(picker.column, 0);
    assert!(picker.filter.is_empty());
    assert_eq!(app.input(), "");
    assert_eq!(app.remote_provider_model, model_before, "no model switch");
}

#[test]
fn focused_browser_typing_searches_and_enter_selects() {
    let mut app = create_test_app();
    configure_available_model_browser_routes(&mut app);

    type_into(&mut app, "/model");
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();
    type_into(&mut app, "g52c");

    let picker = app.inline_interactive_state.as_ref().unwrap();
    assert_eq!(picker.filter, "g52c");
    assert!(
        visible_model_names(&app)
            .first()
            .is_some_and(|name| name.starts_with("gpt-5.2-codex")),
        "{:?}",
        visible_model_names(&app)
    );

    enter_and_confirm_level(&mut app);
    assert!(app.pending_route_selection.is_some(), "Enter stages an available route");
    assert!(
        app.inline_interactive_state.is_none(),
        "Enter on a searched row selects it and closes the browser"
    );
}

#[test]
fn model_with_name_still_previews_and_enter_selects() {
    let mut app = create_test_app();
    configure_available_model_browser_routes(&mut app);

    type_into(&mut app, "/model g52c");
    enter_and_confirm_level(&mut app);

    assert!(app.pending_route_selection.is_some(), "Enter stages an available route");
    assert!(app.inline_interactive_state.is_none());
    assert!(app.input().is_empty());
}

#[test]
fn preview_enter_after_arrow_navigation_selects_that_row() {
    let mut app = create_test_app();
    configure_available_model_browser_routes(&mut app);

    type_into(&mut app, "/model");
    app.handle_key(KeyCode::Down, KeyModifiers::empty()).unwrap();
    enter_and_confirm_level(&mut app);

    assert!(app.pending_route_selection.is_some(), "Enter stages an available route");
    assert!(
        app.inline_interactive_state.is_none(),
        "an explicit row choice is still selected by Enter"
    );
}

#[test]
fn at_provider_token_filters_rows_and_scopes_route() {
    let mut picker = model_browser_state(false);
    picker.filter = "@anthropic".to_string();
    App::apply_inline_interactive_filter(&mut picker);

    let names: Vec<&str> = picker
        .filtered
        .iter()
        .map(|&i| picker.entries[i].name.as_str())
        .collect();
    assert_eq!(names, vec!["claude-opus-4-8"]);
    let entry = &picker.entries[picker.filtered[0]];
    assert_eq!(entry.active_option().unwrap().provider, "Anthropic");

    // `@openrouter` groups every route reached through OpenRouter.
    picker.filter = "@openrouter".to_string();
    App::apply_inline_interactive_filter(&mut picker);
    let names: Vec<&str> = picker
        .filtered
        .iter()
        .map(|&i| picker.entries[i].name.as_str())
        .collect();
    assert_eq!(names, vec!["claude-opus-4-8", "deepseek-v4"]);

    // Provider scope plus search text.
    picker.filter = "@openrouter deep".to_string();
    App::apply_inline_interactive_filter(&mut picker);
    assert_eq!(picker.filtered.len(), 1);
    assert_eq!(picker.entries[picker.filtered[0]].name, "deepseek-v4");

    // Direct `name@provider` syntax is plain search text, not a scope.
    let (scopes, search) = split_model_picker_filter("claude-opus-4-8@Fireworks");
    assert!(scopes.is_empty());
    assert_eq!(search, "claude-opus-4-8@Fireworks");
}

/// Review #1567: `@fireworks` must not match an OpenRouter route that only
/// names Fireworks upstream. Selecting it would send the request through
/// OpenRouter, a different provider than the filter names.
#[test]
fn provider_scope_never_matches_an_openrouter_upstream_label() {
    let mut picker = model_browser_state(false);
    picker.filter = "@fireworks".to_string();
    App::apply_inline_interactive_filter(&mut picker);
    assert!(
        picker.filtered.is_empty(),
        "no route is reached through Fireworks itself: {:?}",
        picker
            .filtered
            .iter()
            .map(|&i| &picker.entries[i].name)
            .collect::<Vec<_>>()
    );

    // The OpenRouter route is still reachable under the provider it uses.
    picker.filter = "@openrouter".to_string();
    App::apply_inline_interactive_filter(&mut picker);
    let entry = &picker.entries[picker.filtered[0]];
    assert_eq!(entry.name, "claude-opus-4-8");
    assert_eq!(entry.active_option().unwrap().api_method, "openrouter");
}

/// Review #1567: a scope switches a row to that provider's route only while
/// the scope applies. Clearing it (Esc, Backspace, Ctrl+P back to all) must
/// give the row back the route it had, so a later unscoped selection goes
/// where it went before the filter.
#[test]
fn clearing_a_provider_scope_restores_the_previous_route() {
    let mut app = create_test_app();
    app.inline_interactive_state = Some(model_browser_state(false));
    let claude = |app: &App| {
        let picker = app.inline_interactive_state.as_ref().unwrap();
        let entry = picker
            .entries
            .iter()
            .find(|e| e.name == "claude-opus-4-8")
            .unwrap();
        entry.active_option().unwrap().api_method.clone()
    };
    assert_eq!(claude(&app), "claude-oauth");

    type_into(&mut app, "@openrouter");
    assert_eq!(claude(&app), "openrouter", "scope selects its route");

    app.handle_key(KeyCode::Esc, KeyModifiers::empty()).unwrap();
    assert!(app.inline_interactive_state.as_ref().unwrap().filter.is_empty());
    assert_eq!(claude(&app), "claude-oauth", "Esc restores the route");

    // Backspacing the scope away restores it too.
    type_into(&mut app, "@openrouter");
    for _ in 0.."@openrouter".len() {
        app.handle_key(KeyCode::Backspace, KeyModifiers::empty())
            .unwrap();
    }
    assert_eq!(claude(&app), "claude-oauth", "Backspace restores the route");

    // So does cycling Ctrl+P back to all providers.
    for _ in 0..5 {
        app.handle_key(KeyCode::Char('p'), KeyModifiers::CONTROL)
            .unwrap();
    }
    assert_eq!(
        model_picker_active_provider(app.inline_interactive_state.as_ref().unwrap()),
        None
    );
    assert_eq!(claude(&app), "claude-oauth", "Ctrl+P to all restores the route");
}

/// Review #1567: under `@openrouter` the Claude row has only one route in
/// scope, so Up/Down in the route column cannot move. That must not count as
/// picking the OpenRouter route: clearing the scope still restores the
/// user's Anthropic route.
#[test]
fn blocked_route_navigation_keeps_the_saved_route() {
    let mut app = create_test_app();
    app.inline_interactive_state = Some(model_browser_state(false));
    let claude = |app: &App| {
        let picker = app.inline_interactive_state.as_ref().unwrap();
        let entry = picker
            .entries
            .iter()
            .find(|e| e.name == "claude-opus-4-8")
            .unwrap();
        entry.active_option().unwrap().api_method.clone()
    };
    type_into(&mut app, "@openrouter");
    {
        let picker = app.inline_interactive_state.as_mut().unwrap();
        picker.selected = picker
            .filtered
            .iter()
            .position(|&i| picker.entries[i].name == "claude-opus-4-8")
            .unwrap();
        picker.column = 1;
    }
    assert_eq!(claude(&app), "openrouter");
    for code in [KeyCode::Up, KeyCode::Down, KeyCode::Down, KeyCode::Up] {
        app.handle_key(code, KeyModifiers::empty()).unwrap();
        assert_eq!(claude(&app), "openrouter", "no other route in scope");
    }
    app.handle_key(KeyCode::Esc, KeyModifiers::empty()).unwrap();
    assert_eq!(
        claude(&app),
        "claude-oauth",
        "a key press that moved nothing must not drop the saved route"
    );
}

/// Review #1567: with a scope active the route column only moves between
/// routes of that provider, so the header's `Provider:` stays true. A route
/// the user picks by hand while scoped is kept after the scope is cleared.
#[test]
fn route_column_stays_inside_the_provider_scope() {
    let mut app = create_test_app();
    app.inline_interactive_state = Some(model_browser_state(false));
    type_into(&mut app, "@anthropic");
    {
        let picker = app.inline_interactive_state.as_mut().unwrap();
        assert_eq!(picker.entries[picker.filtered[0]].name, "claude-opus-4-8");
        picker.column = 1;
    }
    let route = |app: &App| {
        let picker = app.inline_interactive_state.as_ref().unwrap();
        let entry = &picker.entries[picker.filtered[picker.selected]];
        entry.active_option().unwrap().api_method.clone()
    };
    assert_eq!(route(&app), "claude-oauth");
    app.handle_key(KeyCode::Down, KeyModifiers::empty()).unwrap();
    assert_eq!(route(&app), "claude-oauth", "Down cannot leave @anthropic");
    app.handle_key(KeyCode::Up, KeyModifiers::empty()).unwrap();
    assert_eq!(route(&app), "claude-oauth");

    // Unscoped, the route column reaches every route, and a route picked by
    // hand under a later scope survives clearing that scope.
    app.handle_key(KeyCode::Esc, KeyModifiers::empty()).unwrap();
    let picker = app.inline_interactive_state.as_mut().unwrap();
    picker.selected = picker
        .filtered
        .iter()
        .position(|&i| picker.entries[i].name == "claude-opus-4-8")
        .unwrap();
    picker.column = 1;
    app.handle_key(KeyCode::Down, KeyModifiers::empty()).unwrap();
    assert_eq!(route(&app), "openrouter", "unscoped Down reaches OpenRouter");
}

#[test]
fn ctrl_p_cycles_providers_then_back_to_all() {
    let mut app = create_test_app();
    app.inline_interactive_state = Some(model_browser_state(false));

    let ctrl_p = |app: &mut App| {
        app.handle_key(KeyCode::Char('p'), KeyModifiers::CONTROL)
            .unwrap();
        model_picker_active_provider(app.inline_interactive_state.as_ref().unwrap())
    };

    assert_eq!(ctrl_p(&mut app).as_deref(), Some("OpenAI"));
    assert_eq!(visible_model_names(&app), vec!["gpt-5.5"]);
    assert_eq!(ctrl_p(&mut app).as_deref(), Some("Anthropic"));
    assert_eq!(ctrl_p(&mut app).as_deref(), Some("OpenRouter"));
    assert_eq!(
        visible_model_names(&app),
        vec!["claude-opus-4-8", "deepseek-v4"]
    );
    assert_eq!(ctrl_p(&mut app).as_deref(), Some("Gemini"));
    assert_eq!(ctrl_p(&mut app), None, "wraps to all providers");
    assert_eq!(visible_model_names(&app).len(), 4);

    // Search text survives provider cycling.
    type_into(&mut app, "claude");
    assert_eq!(ctrl_p(&mut app).as_deref(), Some("OpenAI"));
    assert!(visible_model_names(&app).is_empty());
    assert_eq!(ctrl_p(&mut app).as_deref(), Some("Anthropic"));
    assert_eq!(visible_model_names(&app), vec!["claude-opus-4-8"]);
    let picker = app.inline_interactive_state.as_ref().unwrap();
    assert_eq!(split_model_picker_filter(&picker.filter).1, "claude");
}

#[test]
fn ctrl_p_from_preview_focuses_browser_with_provider_scope() {
    let mut app = create_test_app();
    app.inline_interactive_state = Some(model_browser_state(true));
    app.input = "/model ".to_string();
    app.cursor_pos = app.input.len();

    app.handle_key(KeyCode::Char('p'), KeyModifiers::CONTROL)
        .unwrap();

    let picker = app.inline_interactive_state.as_ref().unwrap();
    assert!(!picker.preview, "Ctrl+P moves into the focused browser");
    assert_eq!(model_picker_active_provider(picker).as_deref(), Some("OpenAI"));
    assert_eq!(app.input(), "");
}

#[test]
fn provider_only_scope_enter_keeps_browser_open() {
    let mut app = create_test_app();
    let mut picker = model_browser_state(true);
    picker.filter = "@openrouter".to_string();
    App::apply_inline_interactive_filter(&mut picker);
    app.inline_interactive_state = Some(picker);
    app.input = "/model @openrouter".to_string();
    app.cursor_pos = app.input.len();

    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("provider-only scope + Enter should browse, not switch");
    assert!(!picker.preview);
    assert_eq!(
        visible_model_names(&app),
        vec!["claude-opus-4-8", "deepseek-v4"]
    );
}

#[test]
fn focused_browser_page_keys_scroll() {
    let mut app = create_test_app();
    app.inline_interactive_state = Some(model_browser_state(false));

    app.handle_key(KeyCode::End, KeyModifiers::empty()).unwrap();
    assert_eq!(app.inline_interactive_state.as_ref().unwrap().selected, 3);
    app.handle_key(KeyCode::Home, KeyModifiers::empty()).unwrap();
    assert_eq!(app.inline_interactive_state.as_ref().unwrap().selected, 0);
    app.handle_key(KeyCode::PageDown, KeyModifiers::empty())
        .unwrap();
    assert_eq!(app.inline_interactive_state.as_ref().unwrap().selected, 3);
}
