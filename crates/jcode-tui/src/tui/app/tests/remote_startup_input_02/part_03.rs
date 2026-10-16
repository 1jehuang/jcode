#[test]
fn test_model_picker_alt_s_opens_swarm_model_picker() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.is_remote = true;
        app.remote_available_entries = vec!["gpt-5.5".to_string()];
        app.remote_model_options = vec![crate::provider::ModelRoute {
            model: "gpt-5.5".to_string(),
            provider: "OpenAI".to_string(),
            api_method: "openai".to_string(),
            available: true,
            detail: String::new(),
            cheapness: None,
            usage: None,
        }];

        app.open_model_picker();

        // Verify the model picker is open
        assert!(
            app.inline_interactive_state.is_some(),
            "model picker should be open"
        );

        // Alt+S should open the swarm model sub-picker (Ctrl+S stays the
        // global input-stash chord)
        app.handle_key(KeyCode::Char('s'), KeyModifiers::ALT)
            .unwrap();

        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("picker should still be open after Alt+S");

        // After Alt+S, entries should be tagged as AgentModelChoice for Swarm target
        assert!(
            picker.entries.iter().any(|e| matches!(
                e.action,
                crate::tui::PickerAction::AgentModelChoice {
                    target: crate::tui::AgentModelTarget::Swarm,
                    ..
                }
            )),
            "Alt+S should open swarm model picker with AgentModelChoice entries"
        );
    });
}

#[test]
fn test_swarm_picker_from_model_preview_stays_searchable() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.is_remote = true;
        // >64 names-only entries and no hydrated routes: the typed /model
        // preview opens on lightweight routes without caching them, so the
        // Alt+S rebuild cannot be served from a fresh picker cache. That
        // rebuild path is the one that restores preview=true from the /model
        // preview picker, which used to leave the swarm sub-picker glued to
        // the composer: the next typed character landed in the composer and
        // the preview sync closed the picker instead of filtering it.
        app.remote_available_entries = (0..80).map(|i| format!("gpt-test-model-{}", i)).collect();
        if let Ok(config_dir) = crate::storage::app_config_dir() {
            let _ = std::fs::remove_file(config_dir.join("remote_model_catalog_cache.json"));
        }

        for c in "/model".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
                .unwrap();
        }
        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("model picker preview should be open");
        assert!(picker.preview);

        // Alt+S opens the swarm sub-picker from the preview.
        app.handle_key(KeyCode::Char('s'), KeyModifiers::ALT)
            .unwrap();
        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("swarm picker should be open after Alt+S");
        assert!(
            picker.entries.iter().any(|e| matches!(
                e.action,
                crate::tui::PickerAction::AgentModelChoice {
                    target: crate::tui::AgentModelTarget::Swarm,
                    ..
                }
            )),
            "Alt+S from preview should open swarm model picker"
        );
        assert!(
            !picker.preview,
            "swarm sub-picker must be a standalone picker, not a composer-glued preview"
        );

        // Typing must filter the sub-picker. With preview=true restored the
        // character fell through to the composer and the preview sync closed
        // the picker.
        app.handle_key(KeyCode::Char('g'), KeyModifiers::empty())
            .unwrap();
        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("swarm picker must stay open while searching");
        assert_eq!(picker.filter, "g");
        assert!(
            !picker.filtered.is_empty(),
            "typed search must keep matching rows in the swarm picker"
        );
        assert_eq!(
            app.input(),
            "",
            "search characters must not land in the composer"
        );
    });
}

#[test]
fn test_model_picker_alt_m_opens_memory_model_picker_with_routed_models() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.is_remote = true;
        // A routed custom-provider model (slash form) plus a plain OpenAI one:
        // the memory sub-picker used to filter entries down to openai/claude
        // only, hiding the routed model and making the picker look broken.
        // The sidecar's Provider backend routes extraction through any live
        // provider, so both models must be selectable.
        app.remote_available_entries = vec![
            "gpt-5.5".to_string(),
            "together_ai/revolut-ltd/glm-5-3-responses".to_string(),
        ];
        app.remote_model_options = vec![
            crate::provider::ModelRoute {
                model: "gpt-5.5".to_string(),
                provider: "OpenAI".to_string(),
                api_method: "openai".to_string(),
                available: true,
                detail: String::new(),
                cheapness: None,
                usage: None,
            },
            crate::provider::ModelRoute {
                model: "together_ai/revolut-ltd/glm-5-3-responses".to_string(),
                provider: "Together AI".to_string(),
                api_method: "openai".to_string(),
                available: true,
                detail: String::new(),
                cheapness: None,
                usage: None,
            },
        ];

        app.open_model_picker();
        assert!(
            app.inline_interactive_state.is_some(),
            "model picker should be open"
        );

        app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT)
            .unwrap();

        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("memory picker should be open after Alt+M");
        assert!(
            picker.entries.iter().any(|e| matches!(
                e.action,
                crate::tui::PickerAction::AgentModelChoice {
                    target: crate::tui::AgentModelTarget::Memory,
                    ..
                }
            )),
            "Alt+M should open the memory model picker"
        );
        assert!(
            picker.entries.iter().any(|e| {
                e.name.contains("together_ai/revolut-ltd/glm-5-3-responses")
            }),
            "routed custom-provider models must appear in the memory picker"
        );
        assert!(
            picker.entries.iter().any(|e| e.name.contains("gpt-5.5")),
            "plain OpenAI models must still appear in the memory picker"
        );
    });
}

#[test]
fn test_memory_model_selection_persists_routed_override_end_to_end() {
    use crate::message::Message;
    use crate::provider::Provider;
    use std::sync::{Arc as StdArc, Mutex as StdMutex};

    with_temp_jcode_home(|| {
        // Local mode: Enter runs the save inline, so the test can observe the
        // session override directly. Remote mode would only queue a server
        // request (set_agent_model), which is covered by the pending-request
        // tests elsewhere.
        //
        // The mock provider serves the picker catalog through
        // available_models_display, exactly like a live routed custom
        // provider would expose `together_ai/...` models.
        #[derive(Clone)]
        struct RoutedCatalogMock(StdArc<StdMutex<String>>);

        #[async_trait::async_trait]
        impl Provider for RoutedCatalogMock {
            async fn complete(
                &self,
                _messages: &[Message],
                _tools: &[crate::message::ToolDefinition],
                _system: &str,
                _resume_session_id: Option<&str>,
            ) -> Result<crate::provider::EventStream> {
                unimplemented!("RoutedCatalogMock")
            }
            fn name(&self) -> &str {
                "openrouter"
            }
            fn model(&self) -> String {
                self.0.lock().unwrap().clone()
            }
            fn set_model(&self, model: &str) -> anyhow::Result<()> {
                *self.0.lock().unwrap() = model.to_string();
                Ok(())
            }
            fn available_models_display(&self) -> Vec<String> {
                vec![
                    "gpt-5.5".to_string(),
                    "together_ai/revolut-ltd/glm-5-3-responses".to_string(),
                ]
            }
            fn model_routes(&self) -> Vec<crate::provider::ModelRoute> {
                vec![
                    crate::provider::ModelRoute {
                        model: "gpt-5.5".to_string(),
                        provider: "OpenAI".to_string(),
                        api_method: "openai".to_string(),
                        available: true,
                        detail: String::new(),
                        cheapness: None,
                        usage: None,
                    },
                    crate::provider::ModelRoute {
                        model: "together_ai/revolut-ltd/glm-5-3-responses".to_string(),
                        provider: "Together AI".to_string(),
                        api_method: "openai".to_string(),
                        available: true,
                        detail: String::new(),
                        cheapness: None,
                        usage: None,
                    },
                ]
            }
            fn fork(&self) -> Arc<dyn Provider> {
                Arc::new(self.clone())
            }
        }

        let routed_model = "together_ai/revolut-ltd/glm-5-3-responses";
        let provider: Arc<dyn Provider> = Arc::new(RoutedCatalogMock(StdArc::new(StdMutex::new(
            routed_model.to_string(),
        ))));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
        let mut app = App::new_for_test_harness(provider.clone(), registry);

        app.open_model_picker();
        app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT)
            .unwrap();

        // Narrow to the routed model so the selection is unambiguous.
        for ch in "together_ai".chars() {
            app.handle_key(KeyCode::Char(ch), KeyModifiers::NONE)
                .unwrap();
        }
        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("memory picker must stay open while searching");
        assert!(
            !picker.filtered.is_empty(),
            "memory picker must list the routed custom-provider model"
        );
        let filtered_names: Vec<String> = picker
            .filtered
            .iter()
            .map(|&i| picker.entries[i].name.clone())
            .collect::<Vec<_>>();
        assert!(
            filtered_names
                .iter()
                .all(|name| name.contains("together_ai/revolut-ltd/glm-5-3-responses")),
            "filter should leave only routed models, got {:?}",
            filtered_names
        );
        // The selected entry's saved spec is what Enter persists: the routed
        // model id possibly prefixed with the transport route (e.g.
        // `openai-oauth:`), never the display name with its effort suffix.
        let entry = &picker.entries[picker.filtered[picker.selected]];
        let expected_spec =
            crate::tui::app::inline_interactive::subagent_picker_model_spec(entry);
        assert!(
            expected_spec.contains("together_ai/revolut-ltd/glm-5-3-responses"),
            "selected entry should be the routed model, got {}",
            expected_spec
        );

        // Enter saves the override: local mode writes it to the session file
        // synchronously.
        app.handle_key(KeyCode::Enter, KeyModifiers::NONE)
            .unwrap();

        assert!(
            app.inline_interactive_state.is_none(),
            "picker should close after selection"
        );
        let saved = app
            .session
            .effective_agent_model("memory", None)
            .expect("session memory override must be saved after Enter");
        assert_eq!(
            saved, expected_spec,
            "saved memory override must keep the full routed spec"
        );

        // The sidecar consumption path must accept the routed spec.
        // Sidecar::for_session routes session memory overrides through the
        // live provider, exactly like the end-of-turn memory extraction does
        // in both local TUI and server sessions.
        crate::provider::set_active_provider(provider.clone());
        let sidecar = crate::sidecar::Sidecar::for_session(&app.session);
        assert_eq!(
            sidecar.model_name(),
            saved,
            "sidecar must resolve the routed memory override through the provider backend"
        );
    });
}

/// Build a remote test app with a big names-only catalog and open the model
/// picker, returning the app with the async picker load pending.
///
/// A sibling test that does not hold the env lock can persist its own catalog
/// into this temp home while it is live, and a hydrated catalog opens the
/// picker synchronously instead of leaving the async load pending. Clear the
/// stray cache file before opening, and rebuild once if a write still
/// sneaked between the guard and the open.
fn open_app_with_pending_big_remote_catalog() -> App {
    let build = || {
        let mut app = create_test_app();
        app.is_remote = true;
        // >64 remote entries with no hydrated routes take the async catalog
        // path: the picker opens with lightweight routes and a pending
        // background refresh.
        app.remote_available_entries = (0..80).map(|i| format!("gpt-test-model-{}", i)).collect();
        if let Ok(config_dir) = crate::storage::app_config_dir() {
            let _ = std::fs::remove_file(config_dir.join("remote_model_catalog_cache.json"));
        }
        app.open_model_picker();
        app
    };
    let app = build();
    if app.pending_model_picker_load.is_none() {
        build()
    } else {
        app
    }
}

#[test]
fn test_agent_model_picker_late_catalog_refresh_keeps_agent_entries() {
    with_temp_jcode_home(|| {
        let mut app = open_app_with_pending_big_remote_catalog();
        assert!(
            app.pending_model_picker_load.is_some(),
            "large remote catalog should leave a pending background refresh"
        );

        // Alt+S opens the swarm sub-picker. If the refresh completes within
        // the two-second wait window the open path retags inline; if it is
        // still in flight the pending load carries the agent target so the
        // poll completion path retags instead. Simulate the in-flight case:
        // tag the still-pending load, then let the refresh land.
        app.handle_key(KeyCode::Char('s'), KeyModifiers::ALT)
            .unwrap();
        if let Some(pending) = app.pending_model_picker_load.as_mut() {
            pending.agent_target = Some(crate::tui::AgentModelTarget::Swarm);
        }

        let mut waited = 0;
        while app.pending_model_picker_load.is_some() && waited < 500 {
            app.poll_model_picker_load();
            std::thread::sleep(std::time::Duration::from_millis(5));
            waited += 5;
        }

        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("picker should still be open after the refresh");
        assert!(
            picker.entries.iter().any(|e| matches!(
                e.action,
                crate::tui::PickerAction::AgentModelChoice {
                    target: crate::tui::AgentModelTarget::Swarm,
                    ..
                }
            )),
            "late catalog refresh must keep AgentModelChoice entries, got {:?}",
            picker
                .entries
                .iter()
                .map(|e| e.action.clone())
                .collect::<Vec<_>>()
        );
    });
}

#[test]
fn test_agent_model_picker_late_refresh_keeps_inherit_override_search_and_selection() {
    with_temp_jcode_home(|| {
        // A saved swarm override that the refreshed catalog will NOT contain:
        // the retag path must keep a row for it or the override can never be
        // cleared from the picker. The picker opens in session scope by
        // default, so seed the session override, not the global config.

        let mut app = open_app_with_pending_big_remote_catalog();
        assert!(
            app.pending_model_picker_load.is_some(),
            "large remote catalog should leave a pending background refresh"
        );
        app.session
            .set_agent_model_override("swarm", Some("claude-legacy-model".to_string()))
            .unwrap();

        app.handle_key(KeyCode::Char('s'), KeyModifiers::ALT)
            .unwrap();
        if let Some(pending) = app.pending_model_picker_load.as_mut() {
            pending.agent_target = Some(crate::tui::AgentModelTarget::Swarm);
        }

        // User narrows the picker with a search filter and moves down to a
        // model row while the catalog refresh is still in flight.
        for ch in "gpt-test-model-7".chars() {
            app.handle_key(KeyCode::Char(ch), KeyModifiers::NONE)
                .unwrap();
        }
        app.handle_key(KeyCode::Down, KeyModifiers::NONE).unwrap();
        let filter_before = {
            let picker = app.inline_interactive_state.as_ref().unwrap();
            assert!(
                !picker.filter.is_empty(),
                "search text should be active before the refresh lands"
            );
            picker.filter.clone()
        };
        let selected_before = {
            let picker = app.inline_interactive_state.as_ref().unwrap();
            picker.entries[picker.filtered[picker.selected]]
                .name
                .clone()
        };

        // Let the late catalog refresh land while the search is active.
        let mut waited = 0;
        while app.pending_model_picker_load.is_some() && waited < 500 {
            app.poll_model_picker_load();
            std::thread::sleep(std::time::Duration::from_millis(5));
            waited += 5;
        }
        assert!(
            app.pending_model_picker_load.is_none(),
            "refresh should have landed within the wait window"
        );

        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("picker should still be open after the refresh");
        // The inherit (clear-override) row must survive the retag.
        assert!(
            picker.entries.iter().any(|e| matches!(
                e.action,
                crate::tui::PickerAction::AgentModelChoice {
                    target: crate::tui::AgentModelTarget::Swarm,
                    clear_override: true,
                }
            )),
            "late refresh must keep the inherit (clear override) row"
        );
        // The saved override missing from the catalog must keep its row.
        assert!(
            picker
                .entries
                .iter()
                .any(|e| e.name == "claude-legacy-model"),
            "late refresh must keep a row for the saved override"
        );
        // The user's search filter must still be applied, not silently reset.
        assert_eq!(
            picker.filter, filter_before,
            "late refresh must keep the active search filter"
        );
        // And the entry they had selected must still be the selected row, so
        // Enter saves the model they actually chose.
        let selected_after = picker.entries[picker.filtered[picker.selected]]
            .name
            .clone();
        assert_eq!(
            selected_after, selected_before,
            "late refresh must keep the user's selected entry"
        );
    });
}

#[test]
fn test_model_picker_agent_chords_do_not_fire_when_not_in_model_picker() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_available_entries = vec!["gpt-5.5".to_string()];
    app.remote_model_options = vec![crate::provider::ModelRoute {
        model: "gpt-5.5".to_string(),
        provider: "OpenAI".to_string(),
        api_method: "openai".to_string(),
        available: true,
        detail: String::new(),
        cheapness: None,
        usage: None,
    }];

    // Don't open the model picker. Alt+S / Alt+M should be no-ops, and
    // Ctrl+S must not open anything either: it stays the input-stash chord.
    assert!(app.inline_interactive_state.is_none());

    app.handle_key(KeyCode::Char('s'), KeyModifiers::ALT)
        .unwrap();
    app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT)
        .unwrap();
    app.handle_key(KeyCode::Char('s'), KeyModifiers::CONTROL)
        .unwrap();

    // Still no picker should be open
    assert!(
        app.inline_interactive_state.is_none(),
        "agent chords should not open anything when no picker is active"
    );
}

#[test]
fn test_ctrl_s_with_no_picker_stashes_input() {
    let mut app = create_test_app();
    app.set_input_for_test("draft message");
    assert!(app.inline_interactive_state.is_none());

    // With no picker open, Ctrl+S is the input-stash chord: it must stash the
    // draft and never open a picker or sub-picker.
    app.handle_key(KeyCode::Char('s'), KeyModifiers::CONTROL)
        .unwrap();

    assert!(
        app.inline_interactive_state.is_none(),
        "Ctrl+S with no picker must not open the swarm sub-picker"
    );
    assert_eq!(app.input(), "");
    assert!(
        app.stashed_input.is_some(),
        "Ctrl+S with no picker must stash the draft"
    );

    // A second press restores the draft.
    app.handle_key(KeyCode::Char('s'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.input(), "draft message");
    assert!(app.inline_interactive_state.is_none());
}
