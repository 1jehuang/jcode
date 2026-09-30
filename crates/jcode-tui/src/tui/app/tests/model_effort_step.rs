// `/model` lists one row per model and route. Picking a model whose route has
// a reasoning ladder opens a short second step listing that route's levels
// (opencode style) instead of listing "model (low)", "model (high)", ...
// rows side by side.

/// Local provider that records the route and effort the picker applies.
#[derive(Clone, Default)]
struct EffortRecordingProvider {
    route: StdArc<StdMutex<Option<String>>>,
    effort: StdArc<StdMutex<Option<String>>>,
}

#[async_trait::async_trait]
impl Provider for EffortRecordingProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        unimplemented!("EffortRecordingProvider")
    }

    fn name(&self) -> &str {
        "effort-recording"
    }

    fn model(&self) -> String {
        "claude-opus-4-8".to_string()
    }

    fn reasoning_effort(&self) -> Option<String> {
        self.effort.lock().unwrap().clone()
    }

    fn set_reasoning_effort(&self, effort: &str) -> Result<()> {
        *self.effort.lock().unwrap() = Some(effort.to_string());
        Ok(())
    }

    fn set_route_selection(&self, selection: &crate::provider::RouteSelection) -> Result<()> {
        *self.route.lock().unwrap() =
            Some(format!("{}:{}", selection.api_method, selection.model));
        Ok(())
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

fn effort_step_names(app: &App) -> Vec<String> {
    let picker = app.inline_interactive_state.as_ref().expect("picker open");
    picker
        .filtered
        .iter()
        .map(|&i| picker.entries[i].name.clone())
        .collect()
}

fn selected_row_name(app: &App) -> String {
    let picker = app.inline_interactive_state.as_ref().expect("picker open");
    picker.entries[picker.filtered[picker.selected]].name.clone()
}

/// Select the row for `model` reached through `api_method`.
fn select_model_row(app: &mut App, model: &str, api_method: &str) {
    let picker = app.inline_interactive_state.as_mut().expect("picker open");
    let position = picker
        .filtered
        .iter()
        .position(|&i| {
            let entry = &picker.entries[i];
            entry.name == model
                && entry
                    .active_option()
                    .is_some_and(|route| route.api_method == api_method)
        })
        .unwrap_or_else(|| {
            panic!(
                "no `{model}` row via {api_method}: {:?}",
                picker
                    .filtered
                    .iter()
                    .map(|&i| picker.entries[i].name.clone())
                    .collect::<Vec<_>>()
            )
        });
    picker.selected = position;
    // Past the route column: Enter picks this exact route.
    picker.column = picker.max_navigable_column();
}

fn select_step_row(app: &mut App, name: &str) {
    let picker = app.inline_interactive_state.as_mut().expect("picker open");
    picker.selected = picker
        .filtered
        .iter()
        .position(|&i| picker.entries[i].name == name)
        .unwrap_or_else(|| panic!("no `{name}` level row"));
}

/// Enter on a model row, then Enter again if the row has a reasoning ladder
/// and the level step opened: selects the model at its preselected level.
fn enter_and_confirm_level(app: &mut App) {
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();
    if app
        .inline_interactive_state
        .as_ref()
        .is_some_and(|picker| picker.effort_step.is_some())
    {
        app.handle_key(KeyCode::Enter, KeyModifiers::empty())
            .unwrap();
    }
}

fn press(app: &mut App, code: KeyCode) {
    app.handle_key(code, KeyModifiers::empty()).unwrap();
}

#[test]
fn effort_step_model_list_has_one_row_per_model_and_route() {
    let mut app = create_test_app();
    configure_test_remote_models_with_openai_recommendations(&mut app);
    app.open_model_picker();
    let picker = app.inline_interactive_state.as_ref().unwrap();

    for entry in &picker.entries {
        assert!(
            entry.effort.is_none() && !entry.name.ends_with(')'),
            "model rows carry no effort suffix: {:?}",
            entry.name
        );
    }
    let rows = |model: &str, api_method: &str| {
        picker
            .entries
            .iter()
            .filter(|entry| {
                entry.name == model
                    && entry
                        .active_option()
                        .is_some_and(|route| route.api_method == api_method)
            })
            .count()
    };
    assert_eq!(rows("gpt-5.5", "openai-oauth"), 1);
    assert_eq!(rows("claude-opus-4-8", "claude-oauth"), 1);
    assert_eq!(rows("claude-opus-4-8", "claude-api"), 1);
}

#[test]
fn effort_step_enter_on_effort_model_opens_levels_without_switching() {
    let mut app = create_test_app();
    configure_test_remote_models_with_openai_recommendations(&mut app);
    app.open_model_picker();
    select_model_row(&mut app, "gpt-5.5", "openai-oauth");

    press(&mut app, KeyCode::Enter);

    assert!(app.pending_route_selection.is_none(), "no switch yet");
    assert!(app.pending_model_switch.is_none(), "no switch yet");
    assert_eq!(
        effort_step_names(&app),
        vec!["none", "minimal", "low", "med", "high", "xhigh", "max"],
        "the OpenAI ladder, without swarm modes"
    );
    let picker = app.inline_interactive_state.as_ref().unwrap();
    assert!(
        picker
            .entries
            .iter()
            .all(|entry| entry.effort.is_some()),
        "every level row carries its effort"
    );
}

#[test]
fn effort_step_choosing_a_level_stages_model_and_effort_in_remote_mode() {
    let mut app = create_test_app();
    configure_test_remote_models_with_openai_recommendations(&mut app);
    app.open_model_picker();
    select_model_row(&mut app, "gpt-5.5", "openai-oauth");
    press(&mut app, KeyCode::Enter);
    select_step_row(&mut app, "max");

    press(&mut app, KeyCode::Enter);

    assert!(app.inline_interactive_state.is_none(), "picker closes");
    let route = app
        .pending_route_selection
        .as_ref()
        .expect("model switch staged");
    assert_eq!(route.model, "gpt-5.5");
    assert_eq!(route.api_method, "openai-oauth");
    assert_eq!(app.pending_reasoning_effort.as_deref(), Some("max"));
}

#[test]
fn effort_step_choosing_a_level_applies_model_and_effort_locally() {
    with_temp_jcode_home(|| {
        let provider = EffortRecordingProvider::default();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let shared: Arc<dyn Provider> = Arc::new(provider.clone());
        let registry = rt.block_on(crate::tool::Registry::new(shared.clone()));
        let mut app = App::new_for_test_harness(shared, registry);
        let mut picker = model_browser_state(false);
        picker.column = picker.max_navigable_column();
        app.inline_interactive_state = Some(picker);
        select_model_row(&mut app, "claude-opus-4-8", "claude-oauth");

        press(&mut app, KeyCode::Enter);
        assert!(provider.route.lock().unwrap().is_none(), "no switch yet");
        select_step_row(&mut app, "low");
        press(&mut app, KeyCode::Enter);

        assert!(app.inline_interactive_state.is_none());
        assert_eq!(
            provider.route.lock().unwrap().as_deref(),
            Some("claude-oauth:claude-opus-4-8")
        );
        assert_eq!(provider.effort.lock().unwrap().as_deref(), Some("low"));
    });
}

#[test]
fn effort_step_esc_returns_to_the_model_list_with_filter_kept() {
    let mut app = create_test_app();
    app.inline_interactive_state = Some(model_browser_state(false));
    type_into(&mut app, "opus");
    assert_eq!(selected_row_name(&app), "claude-opus-4-8");

    press(&mut app, KeyCode::Enter); // route column (two routes)
    press(&mut app, KeyCode::Enter); // method column
    press(&mut app, KeyCode::Enter); // level step
    assert!(
        effort_step_names(&app).contains(&"high".to_string()),
        "{:?}",
        effort_step_names(&app)
    );

    press(&mut app, KeyCode::Esc);

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("Esc goes back, it does not close");
    assert_eq!(picker.filter, "opus");
    assert_eq!(picker.entries.len(), 4, "the full model list is back");
    assert_eq!(selected_row_name(&app), "claude-opus-4-8");
    assert!(app.pending_route_selection.is_none());
}

#[test]
fn effort_step_route_without_ladder_switches_at_once() {
    let mut app = create_test_app();
    configure_test_remote_models_with_openai_recommendations(&mut app);
    app.remote_model_options.push(crate::provider::ModelRoute {
        model: "claude-opus-4-8".to_string(),
        provider: "Copilot".to_string(),
        api_method: "copilot".to_string(),
        available: true,
        detail: String::new(),
        usage: None,
        cheapness: None,
    });
    app.open_model_picker();
    select_model_row(&mut app, "claude-opus-4-8", "copilot");

    press(&mut app, KeyCode::Enter);

    assert!(app.inline_interactive_state.is_none(), "switched at once");
    assert!(app.pending_route_selection.is_some());
    assert!(app.pending_reasoning_effort.is_none());
}

#[test]
fn effort_step_preselects_current_effort_for_the_current_model() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        configure_test_remote_models_with_openai_recommendations(&mut app);
        app.remote_provider_name = Some("OpenAI".to_string());
        app.remote_reasoning_effort = Some("low".to_string());
        app.open_model_picker();
        select_model_row(&mut app, "gpt-5.2", "openai-oauth");
        press(&mut app, KeyCode::Enter);
        assert_eq!(selected_row_name(&app), "low", "current model keeps its level");

        press(&mut app, KeyCode::Esc);
        select_model_row(&mut app, "claude-opus-4-8", "claude-oauth");
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            selected_row_name(&app),
            "high",
            "a family without a saved default level starts on high"
        );

        // Another OpenAI model starts on the family's saved level.
        press(&mut app, KeyCode::Esc);
        crate::config::Config::set_openai_reasoning_effort(Some("xhigh")).unwrap();
        crate::config::invalidate_config_cache();
        select_model_row(&mut app, "gpt-5.5", "openai-oauth");
        press(&mut app, KeyCode::Enter);
        assert_eq!(selected_row_name(&app), "xhigh", "saved default level");
    });
}

/// Favorites saved for an old "model (high)" row still mark the model's row,
/// and recorded picks for any level still rank it.
#[test]
fn effort_step_old_effort_keyed_favorites_and_usage_match_the_model_row() {
    with_temp_jcode_home(|| {
        let dir = crate::storage::app_config_dir().expect("config dir");
        std::fs::create_dir_all(&dir).unwrap();
        let old_key = "gpt-5.5\u{1f}OpenAI\u{1f}openai-oauth\u{1f}high";
        std::fs::write(
            dir.join("model_picker_favorites.json"),
            serde_json::json!({ "version": 1, "favorites": [old_key] }).to_string(),
        )
        .unwrap();
        std::fs::write(
            dir.join("model_picker_usage.json"),
            serde_json::json!({
                "version": 1,
                "selections": {
                    old_key: { "count": 2, "last_selected_unix_secs": 1 },
                    "gpt-5.5\u{1f}OpenAI\u{1f}openai-oauth\u{1f}low":
                        { "count": 1, "last_selected_unix_secs": 2 },
                },
            })
            .to_string(),
        )
        .unwrap();

        let mut app = create_test_app();
        configure_test_remote_models_with_openai_recommendations(&mut app);
        app.open_model_picker();
        let picker = app.inline_interactive_state.as_ref().unwrap();
        let row = picker
            .entries
            .iter()
            .find(|entry| entry.name == "gpt-5.5")
            .expect("gpt-5.5 row");
        assert!(row.is_favorite, "the old effort-keyed favorite marks the row");
        assert_eq!(row.usage_score, 350, "picks at every level count");
    });
}

/// Save-default (Ctrl+O) on a model with a reasoning ladder opens the same
/// level step and saves model + chosen level. Esc there saves nothing.
#[test]
fn effort_step_save_default_saves_model_and_chosen_level() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        configure_test_remote_models_with_openai_recommendations(&mut app);
        app.open_model_picker();
        select_model_row(&mut app, "claude-opus-4-8", "claude-oauth");

        app.handle_key(KeyCode::Char('o'), KeyModifiers::CONTROL)
            .unwrap();
        assert!(
            effort_step_names(&app).contains(&"xhigh".to_string()),
            "Ctrl+O opens the level step"
        );
        press(&mut app, KeyCode::Esc);
        assert!(
            crate::config::Config::load().provider.default_model.is_none(),
            "Esc saves nothing"
        );

        app.handle_key(KeyCode::Char('o'), KeyModifiers::CONTROL)
            .unwrap();
        select_step_row(&mut app, "max");
        press(&mut app, KeyCode::Enter);

        let config = crate::config::Config::load();
        assert_eq!(
            config.provider.default_model.as_deref(),
            Some("claude-oauth:claude-opus-4-8")
        );
        assert_eq!(
            config.provider.anthropic_reasoning_effort.as_deref(),
            Some("max")
        );
        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("back in the model list");
        assert!(picker.effort_step.is_none());
        assert!(picker.entries[picker.filtered[picker.selected]].is_default);
        assert!(app.pending_route_selection.is_none(), "saving does not switch");
    });
}
