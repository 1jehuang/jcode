#[test]
fn r24_model_rows_do_not_repeat_reasoning_levels() {
    let mut app = create_test_app();
    r23_configure_available_model_browser_routes(&mut app);
    app.open_model_picker();
    let picker = app.inline_interactive_state.as_ref().unwrap();
    assert!(picker.entries.iter().all(|e| e.effort.is_none()));
    let mut identities = std::collections::HashSet::new();
    for entry in &picker.entries {
        let route = entry.active_option().unwrap();
        assert!(identities.insert((&entry.name, &route.provider, &route.api_method)));
    }
}

#[test]
fn r24_model_enter_waits_for_reasoning_confirmation() {
    let mut app = create_test_app();
    r23_configure_available_model_browser_routes(&mut app);
    app.open_model_picker();
    let picker = app.inline_interactive_state.as_mut().unwrap();
    picker.selected = picker
        .filtered
        .iter()
        .position(|&i| picker.entries[i].name.starts_with("gpt-5.2-codex"))
        .unwrap();
    picker.column = picker.max_navigable_column();
    app.handle_key(KeyCode::Enter, KeyModifiers::NONE).unwrap();
    assert!(app.pending_route_selection.is_none());
    assert!(app.pending_reasoning_effort.is_none());
    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("reasoning choice stays open");
    assert!(
        picker
            .filtered
            .iter()
            .all(|&i| !picker.entries[i].name.starts_with("gpt-"))
    );
}

// `/model` lists one row per model and route. Picking a model whose route has
// a reasoning ladder opens a short second step listing that route's levels
// (opencode style) instead of listing "model (low)", "model (high)", ...
// rows side by side.

/// Local provider that records the route and effort the picker applies.
#[derive(Clone, Default)]
struct EffortRecordingProvider {
    route: StdArc<StdMutex<Option<String>>>,
    effort: StdArc<StdMutex<Option<String>>>,
    reject_effort: bool,
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

    fn available_efforts(&self) -> Vec<&'static str> {
        vec!["low", "medium", "high", "swarm", "swarm-deep"]
    }

    fn set_reasoning_effort(&self, effort: &str) -> Result<()> {
        if self.reject_effort {
            anyhow::bail!("mock rejected reasoning level");
        }
        *self.effort.lock().unwrap() = Some(effort.to_string());
        Ok(())
    }

    fn set_route_selection(&self, selection: &crate::provider::RouteSelection) -> Result<()> {
        *self.route.lock().unwrap() = Some(format!("{}:{}", selection.api_method, selection.model));
        Ok(())
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

fn r24_effort_step_names(app: &App) -> Vec<String> {
    let picker = app.inline_interactive_state.as_ref().expect("picker open");
    picker
        .filtered
        .iter()
        .map(|&i| picker.entries[i].name.clone())
        .collect()
}

fn r24_selected_row_name(app: &App) -> String {
    let picker = app.inline_interactive_state.as_ref().expect("picker open");
    picker.entries[picker.filtered[picker.selected]]
        .name
        .clone()
}

/// Select the row for `model` reached through `api_method`.
fn r24_select_model_row(app: &mut App, model: &str, api_method: &str) {
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

fn r24_select_step_row(app: &mut App, name: &str) {
    let picker = app.inline_interactive_state.as_mut().expect("picker open");
    picker.selected = picker
        .filtered
        .iter()
        .position(|&i| picker.entries[i].name == name)
        .unwrap_or_else(|| panic!("no `{name}` level row"));
}

/// Enter on a model row, then Enter again if the row has a reasoning ladder
/// and the level step opened: selects the model at its preselected level.
fn r24_enter_and_confirm_level(app: &mut App) {
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

fn r24_press(app: &mut App, code: KeyCode) {
    app.handle_key(code, KeyModifiers::empty()).unwrap();
}

#[test]
fn r24_effort_step_model_list_has_one_row_per_model_and_route() {
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
fn r24_effort_step_enter_on_effort_model_opens_levels_without_switching() {
    let mut app = create_test_app();
    configure_test_remote_models_with_openai_recommendations(&mut app);
    app.open_model_picker();
    r24_select_model_row(&mut app, "gpt-5.5", "openai-oauth");

    r24_press(&mut app, KeyCode::Enter);

    assert!(app.pending_route_selection.is_none(), "no switch yet");
    assert!(app.pending_model_switch.is_none(), "no switch yet");
    assert_eq!(
        r24_effort_step_names(&app),
        vec!["none", "minimal", "low", "med", "high", "xhigh", "max"],
        "the OpenAI ladder, without swarm modes"
    );
    let picker = app.inline_interactive_state.as_ref().unwrap();
    assert!(
        picker.entries.iter().all(|entry| entry.effort.is_some()),
        "every level row carries its effort"
    );
}

#[test]
fn r24_effort_step_choosing_a_level_stages_model_and_effort_in_remote_mode() {
    let mut app = create_test_app();
    configure_test_remote_models_with_openai_recommendations(&mut app);
    app.open_model_picker();
    r24_select_model_row(&mut app, "gpt-5.5", "openai-oauth");
    r24_press(&mut app, KeyCode::Enter);
    r24_select_step_row(&mut app, "max");

    r24_press(&mut app, KeyCode::Enter);

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
fn r24_effort_step_choosing_a_level_applies_model_and_effort_locally() {
    with_temp_jcode_home(|| {
        let provider = EffortRecordingProvider::default();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let shared: Arc<dyn Provider> = Arc::new(provider.clone());
        let registry = rt.block_on(crate::tool::Registry::new(shared.clone()));
        let mut app = App::new_for_test_harness(shared, registry);
        let mut picker = r23_model_browser_state(false);
        picker.column = picker.max_navigable_column();
        app.inline_interactive_state = Some(picker);
        r24_select_model_row(&mut app, "claude-opus-4-8", "claude-oauth");

        r24_press(&mut app, KeyCode::Enter);
        assert!(provider.route.lock().unwrap().is_none(), "no switch yet");
        r24_select_step_row(&mut app, "low");
        r24_press(&mut app, KeyCode::Enter);

        assert!(app.inline_interactive_state.is_none());
        assert_eq!(
            provider.route.lock().unwrap().as_deref(),
            Some("claude-oauth:claude-opus-4-8")
        );
        assert_eq!(provider.effort.lock().unwrap().as_deref(), Some("low"));
    });
}

#[test]
fn r24_effort_step_esc_returns_to_the_model_list_with_filter_kept() {
    let mut app = create_test_app();
    app.inline_interactive_state = Some(r23_model_browser_state(false));
    r23_type_into(&mut app, "opus");
    assert_eq!(r24_selected_row_name(&app), "claude-opus-4-8");

    r24_press(&mut app, KeyCode::Enter); // route column (two routes)
    r24_press(&mut app, KeyCode::Enter); // method column
    r24_press(&mut app, KeyCode::Enter); // level step
    assert!(
        r24_effort_step_names(&app).contains(&"high".to_string()),
        "{:?}",
        r24_effort_step_names(&app)
    );

    r24_press(&mut app, KeyCode::Esc);

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("Esc goes back, it does not close");
    assert_eq!(picker.filter, "opus");
    assert_eq!(picker.entries.len(), 4, "the full model list is back");
    assert_eq!(r24_selected_row_name(&app), "claude-opus-4-8");
    assert_eq!(
        picker.column,
        picker.max_navigable_column(),
        "return keeps the parent navigation column"
    );
    assert_eq!(
        picker
            .selected_entry()
            .unwrap()
            .active_option()
            .unwrap()
            .api_method,
        "claude-oauth"
    );
    assert!(app.pending_route_selection.is_none());
}

#[test]
fn r24_effort_step_route_without_ladder_switches_at_once() {
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
    r24_select_model_row(&mut app, "claude-opus-4-8", "copilot");

    r24_press(&mut app, KeyCode::Enter);

    assert!(app.inline_interactive_state.is_none(), "switched at once");
    assert!(app.pending_route_selection.is_some());
    assert!(app.pending_reasoning_effort.is_none());
}

#[test]
fn r24_effort_step_preselects_current_effort_for_the_current_model() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        configure_test_remote_models_with_openai_recommendations(&mut app);
        app.remote_provider_name = Some("OpenAI".to_string());
        app.remote_reasoning_effort = Some("low".to_string());
        app.open_model_picker();
        r24_select_model_row(&mut app, "gpt-5.2", "openai-oauth");
        r24_press(&mut app, KeyCode::Enter);
        assert_eq!(
            r24_selected_row_name(&app),
            "low",
            "current model keeps its level"
        );

        r24_press(&mut app, KeyCode::Esc);
        r24_select_model_row(&mut app, "claude-opus-4-8", "claude-oauth");
        r24_press(&mut app, KeyCode::Enter);
        assert_eq!(
            r24_selected_row_name(&app),
            crate::config::Config::load()
                .provider
                .anthropic_reasoning_effort
                .as_deref()
                .unwrap_or("high"),
            "another family starts on its configured default"
        );

        // Another OpenAI model starts on the family's saved level.
        r24_press(&mut app, KeyCode::Esc);
        crate::config::Config::set_openai_reasoning_effort(Some("xhigh")).unwrap();
        crate::config::invalidate_config_cache();
        r24_select_model_row(&mut app, "gpt-5.5", "openai-oauth");
        r24_press(&mut app, KeyCode::Enter);
        assert_eq!(r24_selected_row_name(&app), "xhigh", "saved default level");
    });
}

/// Favorites saved for an old "model (high)" row still mark the model's row,
/// and recorded picks for any level still rank it.
#[test]
fn r24_effort_step_old_effort_keyed_favorites_and_usage_match_the_model_row() {
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
        assert!(
            row.is_favorite,
            "the old effort-keyed favorite marks the row"
        );
        assert_eq!(row.usage_score, 350, "picks at every level count");
    });
}

/// Save-default (Ctrl+O) on a model with a reasoning ladder opens the same
/// level step and saves model + chosen level. Esc there saves nothing.
#[test]
fn r24_effort_step_save_default_saves_model_and_chosen_level() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        configure_test_remote_models_with_openai_recommendations(&mut app);
        app.open_model_picker();
        r24_select_model_row(&mut app, "claude-opus-4-8", "claude-oauth");

        app.handle_key(KeyCode::Char('o'), KeyModifiers::CONTROL)
            .unwrap();
        assert!(
            r24_effort_step_names(&app).contains(&"xhigh".to_string()),
            "Ctrl+O opens the level step"
        );
        r24_press(&mut app, KeyCode::Esc);
        assert!(
            crate::config::Config::load()
                .provider
                .default_model
                .is_none(),
            "Esc saves nothing"
        );

        app.handle_key(KeyCode::Char('o'), KeyModifiers::CONTROL)
            .unwrap();
        r24_select_step_row(&mut app, "max");
        r24_press(&mut app, KeyCode::Enter);

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
        assert!(
            app.pending_route_selection.is_none(),
            "saving does not switch"
        );
    });
}

/// The same model through a different route is not the current route: its
/// level step starts on the saved default for the family, not on the level
/// the active route happens to run at.
#[test]
fn r24_effort_step_other_route_of_the_current_model_uses_the_saved_level() {
    with_temp_jcode_home(|| {
        crate::config::Config::set_openai_reasoning_effort(Some("xhigh")).unwrap();
        crate::config::invalidate_config_cache();
        let mut app = create_test_app();
        configure_test_remote_models_with_openai_recommendations(&mut app);
        app.remote_model_options.push(crate::provider::ModelRoute {
            model: "gpt-5.2".to_string(),
            provider: "OpenAI".to_string(),
            api_method: "openai-api-key".to_string(),
            available: true,
            detail: String::new(),
            usage: None,
            cheapness: None,
        });
        app.remote_provider_name = Some("OpenAI".to_string());
        app.remote_reasoning_effort = Some("low".to_string());
        app.session.route_api_method = Some("openai-oauth".to_string());
        app.open_model_picker();

        r24_select_model_row(&mut app, "gpt-5.2", "openai-oauth");
        r24_press(&mut app, KeyCode::Enter);
        assert_eq!(
            r24_selected_row_name(&app),
            "low",
            "the active route keeps its level"
        );

        r24_press(&mut app, KeyCode::Esc);
        r24_select_model_row(&mut app, "gpt-5.2", "openai-api-key");
        r24_press(&mut app, KeyCode::Enter);
        assert_eq!(
            r24_selected_row_name(&app),
            "xhigh",
            "another route of the same model starts on the saved level"
        );
    });
}

/// A route load that finishes while the level step is open must not reopen
/// the model list after the user confirms a level.
#[test]
fn r24_effort_step_completed_choice_is_not_reopened_by_late_catalog() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        configure_test_remote_models_with_openai_recommendations(&mut app);
        app.open_model_picker();
        r24_select_model_row(&mut app, "claude-opus-4-8", "claude-oauth");
        r24_press(&mut app, KeyCode::Enter);
        assert!(
            app.inline_interactive_state
                .as_ref()
                .is_some_and(|picker| picker.effort_step.is_some()),
            "level step open"
        );

        // A background route load completes while the step is open.
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Ok(ModelPickerRoutesResult {
            outcome: ModelPickerBuildOutcome::Built {
                entries: Vec::new(),
                model_count: 0,
                routes: app.remote_model_options.clone(),
            },
        }))
        .unwrap();
        // Same signature the picker computes, so the load counts as current.
        let signature = app.model_picker_cache.as_ref().unwrap().signature.clone();
        app.pending_model_picker_load = Some(PendingModelPickerLoad {
            store_remote_routes: true,
            request_id: app.model_picker_load_request_id,
            signature,
            picker_started: std::time::Instant::now(),
            receiver: rx,
        });

        r24_press(&mut app, KeyCode::Enter);
        assert!(
            app.inline_interactive_state.is_none(),
            "confirming closes the picker"
        );
        app.poll_model_picker_load();
        assert!(
            app.inline_interactive_state.is_none(),
            "the finished route load must not reopen the model list"
        );
    });
}

#[test]
fn r24_effort_step_survives_a_catalog_load_before_confirmation() {
    let mut app = create_test_app();
    configure_test_remote_models_with_openai_recommendations(&mut app);
    app.open_model_picker();
    r24_select_model_row(&mut app, "claude-opus-4-8", "claude-oauth");
    r24_press(&mut app, KeyCode::Enter);
    let signature = app.model_picker_cache.as_ref().unwrap().signature.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(Ok(ModelPickerRoutesResult {
        outcome: ModelPickerBuildOutcome::Built {
            entries: Vec::new(),
            model_count: 0,
            routes: app.remote_model_options.clone(),
        },
    }))
    .unwrap();
    app.pending_model_picker_load = Some(PendingModelPickerLoad {
        store_remote_routes: true,
        request_id: app.model_picker_load_request_id,
        signature,
        picker_started: std::time::Instant::now(),
        receiver: rx,
    });
    assert!(app.poll_model_picker_load());
    assert!(
        app.inline_interactive_state
            .as_ref()
            .unwrap()
            .effort_step
            .is_some()
    );
    assert!(app.pending_route_selection.is_none());
    r24_press(&mut app, KeyCode::Enter);
    assert!(app.inline_interactive_state.is_none());
    assert_eq!(
        app.pending_route_selection.as_ref().unwrap().model,
        "claude-opus-4-8"
    );
}

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
        .position(|m| m.name == "gpt-5.5")
        .expect("gpt-5.5 should be in picker");
    assert!(
        picker.entries[entry_idx].effort.is_none(),
        "model rows carry no effort; it is picked in the level step"
    );

    let filtered_pos = picker
        .filtered
        .iter()
        .position(|&i| i == entry_idx)
        .expect("gpt-5.5 should be in filtered list");
    app.inline_interactive_state.as_mut().unwrap().selected = filtered_pos;

    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();
    {
        let picker = app.inline_interactive_state.as_mut().expect("level step");
        assert!(picker.effort_step.is_some(), "Enter opens the level step");
        picker.selected = picker
            .entries
            .iter()
            .position(|entry| entry.effort.as_deref() == Some("high"))
            .expect("high level row");
    }
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    assert!(
        app.inline_interactive_state.is_none(),
        "picker should close"
    );
    assert!(
        app.pending_route_selection.is_some(),
        "model switch should be staged for the remote dispatcher"
    );
    assert_eq!(
        app.pending_reasoning_effort.as_deref(),
        Some("high"),
        "the picked level must be staged so it reaches the server (issue #427)"
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
        usage: None,
        cheapness: None,
    });

    app.open_model_picker();
    // The level step lists the ladder of the picked route only.
    let levels_for = |app: &mut App, api_method: &str| -> Vec<String> {
        {
            let picker = app.inline_interactive_state.as_mut().expect("picker open");
            picker.selected = picker
                .filtered
                .iter()
                .position(|&i| {
                    picker.entries[i].name == "gpt-5.5"
                        && picker.entries[i]
                            .active_option()
                            .is_some_and(|route| route.api_method == api_method)
                })
                .expect("gpt-5.5 row for route");
            picker.column = picker.max_navigable_column();
        }
        app.handle_key(KeyCode::Enter, KeyModifiers::empty())
            .unwrap();
        let levels = app
            .inline_interactive_state
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .filter_map(|entry| entry.effort.clone())
            .collect();
        app.handle_key(KeyCode::Esc, KeyModifiers::empty()).unwrap();
        levels
    };

    let native = levels_for(&mut app, "openai-oauth");
    let openrouter = levels_for(&mut app, "openrouter");
    assert!(native.iter().any(|e| e == "max"));
    assert!(native.iter().any(|e| e == "minimal"));
    assert!(openrouter.iter().any(|e| e == "xhigh"));
    assert!(openrouter.iter().any(|e| e == "minimal"));
    assert!(
        !openrouter.iter().any(|e| e == "max"),
        "OpenRouter must not advertise max as a distinct rung because it aliases xhigh"
    );
    assert!(
        !native
            .iter()
            .chain(&openrouter)
            .any(|e| e.starts_with("swarm")),
        "swarm modes are not reasoning levels"
    );
}

#[test]
fn test_local_model_picker_openrouter_bare_openai_route_uses_openai_catalog_prefix() {
    let (mut app, set_model_calls) = create_openrouter_spec_capture_test_app();
    app.open_model_picker();
    wait_for_model_picker_load(&mut app);

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");
    let model_idx = picker
        .entries
        .iter()
        .position(|entry| entry.name == "gpt-5.4")
        .expect("openrouter-backed OpenAI entry should be in picker");
    let filtered_pos = picker
        .filtered
        .iter()
        .position(|&i| i == model_idx)
        .expect("entry should be in filtered list");

    app.inline_interactive_state.as_mut().unwrap().selected = filtered_pos;
    // Enter opens the level step. The model only switches once a level is
    // confirmed.
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .expect("model picker selection should succeed");
    assert!(set_model_calls.lock().unwrap().is_empty());
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .expect("level selection should succeed");

    assert_eq!(
        set_model_calls.lock().unwrap().as_slice(),
        ["openai/gpt-5.4@OpenAI"]
    );
}

#[test]
fn r24_effort_step_reports_local_rejection_after_the_model_switch() {
    with_temp_jcode_home(|| {
        let provider = EffortRecordingProvider {
            reject_effort: true,
            ..Default::default()
        };
        let mut app = create_test_app();
        app.is_remote = false;
        app.provider = Arc::new(provider.clone());
        app.inline_interactive_state = Some(r23_model_browser_state(false));
        r24_select_model_row(&mut app, "claude-opus-4-8", "claude-oauth");
        r24_press(&mut app, KeyCode::Enter);
        r24_select_step_row(&mut app, "high");
        r24_press(&mut app, KeyCode::Enter);
        assert!(app.inline_interactive_state.is_none());
        assert_eq!(
            provider.route.lock().unwrap().as_deref(),
            Some("claude-oauth:claude-opus-4-8")
        );
        assert!(provider.effort.lock().unwrap().is_none());
        assert!(app.status_notice().unwrap().contains("rejected"));
        assert!(
            app.display_messages()
                .iter()
                .any(|m| m.content.contains("mock rejected reasoning level"))
        );
    });
}

#[test]
fn review_r24_refresh_reaches_restored_parent_after_both_catalog_paths() {
    for refresh in ["catalog", "load", "auth"] {
        for save_default in [false, true] {
            with_temp_jcode_home(|| {
                let mut app = create_test_app();
                configure_test_remote_models_with_openai_recommendations(&mut app);
                app.open_model_picker();
                r23_type_into(&mut app, "@openai gpt");
                r24_select_model_row(&mut app, "gpt-5.5", "openai-oauth");
                let parent = app.inline_interactive_state.as_ref().unwrap().clone();
                let signature = app.model_picker_cache.as_ref().unwrap().signature.clone();
                let mut entries = parent.entries.clone();
                let selected = entries
                    .iter_mut()
                    .find(|entry| {
                        entry.name == "gpt-5.5"
                            && entry
                                .active_option()
                                .is_some_and(|route| route.api_method == "openai-oauth")
                    })
                    .unwrap();
                selected
                    .options
                    .iter_mut()
                    .for_each(|route| route.available = false);
                let mut future = selected.clone();
                future.name = "gpt-future-review".into();
                future
                    .options
                    .iter_mut()
                    .for_each(|route| route.available = true);
                entries.push(future);
                let mut routes = app.remote_model_options.clone();
                let selected = routes
                    .iter_mut()
                    .find(|route| route.model == "gpt-5.5" && route.api_method == "openai-oauth")
                    .unwrap();
                selected.available = false;
                let mut future = selected.clone();
                future.model = "gpt-future-review".into();
                future.available = true;
                routes.push(future);
                if save_default {
                    app.handle_key(KeyCode::Char('o'), KeyModifiers::CONTROL)
                        .unwrap();
                } else {
                    r24_press(&mut app, KeyCode::Enter);
                }
                let levels = r24_effort_step_names(&app);
                let selected_level = app.inline_interactive_state.as_ref().unwrap().selected;
                if refresh == "load" {
                    let (tx, rx) = std::sync::mpsc::channel();
                    tx.send(Ok(ModelPickerRoutesResult {
                        outcome: ModelPickerBuildOutcome::Built {
                            model_count: entries.len(),
                            entries,
                            routes,
                        },
                    }))
                    .unwrap();
                    app.pending_model_picker_load = Some(PendingModelPickerLoad {
                        store_remote_routes: true,
                        request_id: app.model_picker_load_request_id,
                        signature,
                        picker_started: Instant::now(),
                        receiver: rx,
                    });
                    assert!(app.poll_model_picker_load());
                } else {
                    app.remote_model_options = routes;
                    app.invalidate_model_picker_cache();
                    if refresh == "auth" {
                        app.auth_catalog_refresh_pending = true;
                        app.finish_auth_catalog_refresh();
                    } else {
                        app.refresh_open_model_picker_after_catalog_update();
                    }
                }
                assert_eq!(r24_effort_step_names(&app), levels);
                assert_eq!(
                    app.inline_interactive_state.as_ref().unwrap().selected,
                    selected_level
                );
                assert!(app.pending_route_selection.is_none());
                r24_press(
                    &mut app,
                    if save_default {
                        KeyCode::Enter
                    } else {
                        KeyCode::Esc
                    },
                );
                let restored = app
                    .inline_interactive_state
                    .as_ref()
                    .expect("back in model list");
                assert!(restored.effort_step.is_none());
                assert!(
                    restored
                        .entries
                        .iter()
                        .any(|entry| entry.name == "gpt-future-review"),
                    "new models must appear after returning; refresh={refresh}, default={save_default}"
                );
                assert_eq!(restored.filter, parent.filter);
                assert_eq!(restored.column, parent.column);
                let selected = restored.selected_entry().unwrap();
                assert_eq!(selected.name, "gpt-5.5");
                assert_eq!(selected.active_option().unwrap().api_method, "openai-oauth");
                assert!(
                    !selected.active_option().unwrap().available,
                    "updated route availability must replace the held parent"
                );
            });
        }
    }
}

#[test]
fn r24_local_saved_default_keeps_upstream_selection_until_full_catalog() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.provider = Arc::new(EffortRecordingProvider::default());
        app.is_remote = false;
        let model = "claude-opus-4-8";
        let mut parent = r23_model_browser_state(false);
        parent.entries = vec![
            r23_model_browser_entry(model, &[("Anthropic", "claude-oauth")]),
            r23_model_browser_entry(model, &[("Fireworks", "openrouter")]),
        ];
        parent.filtered = vec![0, 1];
        parent.selected = 1;
        parent.column = parent.max_navigable_column();
        let full_entries = parent.entries.clone();
        app.inline_interactive_state = Some(parent);

        app.handle_key(KeyCode::Char('o'), KeyModifiers::CONTROL)
            .unwrap();
        assert!(
            app.inline_interactive_state
                .as_ref()
                .unwrap()
                .effort_step
                .is_some()
        );
        app.refresh_open_model_picker_after_catalog_update();
        r24_press(&mut app, KeyCode::Enter);

        let held = app.inline_interactive_state.as_ref().unwrap();
        assert_eq!(held.selected_entry().unwrap().name, model);
        assert_eq!(
            held.selected_entry()
                .unwrap()
                .active_option()
                .unwrap()
                .provider,
            "Fireworks"
        );
        assert!(
            crate::config::Config::load()
                .provider
                .default_model
                .as_deref()
                .is_some_and(|saved| saved.contains("Fireworks"))
        );

        let pending = app
            .pending_model_picker_load
            .as_mut()
            .expect("full local load");
        let (tx, rx) = std::sync::mpsc::channel();
        pending.receiver = rx;
        let routes = [("Anthropic", "claude-oauth"), ("Fireworks", "openrouter")]
            .into_iter()
            .map(|(provider, api_method)| crate::provider::ModelRoute {
                model: model.into(),
                provider: provider.into(),
                api_method: api_method.into(),
                available: true,
                detail: String::new(),
                usage: None,
                cheapness: None,
            })
            .collect();
        tx.send(Ok(ModelPickerRoutesResult {
            outcome: ModelPickerBuildOutcome::Built {
                entries: full_entries,
                model_count: 1,
                routes,
            },
        }))
        .unwrap();
        assert!(app.poll_model_picker_load());
        let selected = app
            .inline_interactive_state
            .as_ref()
            .unwrap()
            .selected_entry()
            .unwrap();
        assert_eq!(selected.name, model);
        assert_eq!(selected.active_option().unwrap().provider, "Fireworks");
        assert_eq!(selected.active_option().unwrap().api_method, "openrouter");
    });
}
