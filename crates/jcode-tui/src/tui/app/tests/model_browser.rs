// `/model` + Enter opens a browsable, searchable model picker instead of
// switching to the first listed model. `@provider` tokens and Ctrl+P scope
// the list to one provider. `/model <name>` still switches directly.

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
    configure_test_remote_models(&mut app);
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
    configure_test_remote_models(&mut app);

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

    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();
    assert!(
        app.inline_interactive_state.is_none(),
        "Enter on a searched row selects it and closes the browser"
    );
}

#[test]
fn model_with_name_still_previews_and_enter_selects() {
    let mut app = create_test_app();
    configure_test_remote_models(&mut app);

    type_into(&mut app, "/model g52c");
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    assert!(app.inline_interactive_state.is_none());
    assert!(app.input().is_empty());
}

#[test]
fn preview_enter_after_arrow_navigation_selects_that_row() {
    let mut app = create_test_app();
    configure_test_remote_models(&mut app);

    type_into(&mut app, "/model");
    app.handle_key(KeyCode::Down, KeyModifiers::empty()).unwrap();
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

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
