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
    picker.filter = "@fireworks".to_string();
    App::apply_inline_interactive_filter(&mut picker);

    let names: Vec<&str> = picker
        .filtered
        .iter()
        .map(|&i| picker.entries[i].name.as_str())
        .collect();
    assert_eq!(names, vec!["claude-opus-4-8"]);
    let entry = &picker.entries[picker.filtered[0]];
    assert_eq!(entry.active_option().unwrap().provider, "Fireworks");

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
