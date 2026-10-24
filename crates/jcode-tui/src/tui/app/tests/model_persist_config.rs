// End-to-end test: `persist_model_switch_to_config` writes the model to
// `config.toml` so it survives relaunch/new sessions.
//
// Before the fix, `finalize_model_switch` only saved to the *session* file.
// A resumed session restored the model, but a NEW session or relaunch read
// `config.toml`'s `[provider].default_model` and reverted to the old default.
// This test proves the fix: after `cycle_model`, the config file on disk
// reflects the new model.

use std::sync::RwLock;

/// Mock provider with a mutable model and multiple available models for
/// `cycle_model` to cycle through.
#[derive(Clone)]
struct CycleSwitchProvider {
    model: StdArc<RwLock<String>>,
    models: Vec<&'static str>,
}

#[async_trait::async_trait]
impl Provider for CycleSwitchProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        unimplemented!("CycleSwitchProvider")
    }

    fn name(&self) -> &str {
        "cycle-test"
    }

    fn model(&self) -> String {
        self.model.read().unwrap().clone()
    }

    fn available_models(&self) -> Vec<&'static str> {
        self.models.clone()
    }

    fn set_model(&self, model: &str) -> Result<()> {
        *self.model.write().unwrap() = model.to_string();
        Ok(())
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

fn create_cycle_switch_test_app(
    initial_model: &str,
    models: Vec<&'static str>,
) -> App {
    ensure_test_jcode_home_if_unset();
    clear_persisted_test_ui_state();
    crate::tui::ui::clear_test_render_state_for_tests();

    let provider: Arc<dyn Provider> = Arc::new(CycleSwitchProvider {
        model: StdArc::new(RwLock::new(initial_model.to_string())),
        models,
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new_for_test_harness(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app
}

/// Read `[provider].default_model` from the config file on disk.
fn read_config_default_model() -> Option<String> {
    crate::config::Config::load().provider.default_model
}

fn flush_debounced_model_persist(app: &mut App) {
    std::thread::sleep(Duration::from_millis(550));
    assert!(
        app.maybe_flush_pending_model_config_persist(),
        "debounced model persist should flush after the debounce window"
    );
}

#[test]
fn cycle_model_persists_switch_to_config_toml() {
    with_temp_jcode_home(|| {
        // Write a config with a known default model.
        write_test_config(
            "[provider]\ndefault_model = \"model-a\"\ndefault_provider = \"cycle-test\"\n",
        );
        crate::config::invalidate_config_cache();
        assert_eq!(
            read_config_default_model().as_deref(),
            Some("model-a"),
            "precondition: config should have model-a as default"
        );

        // Create an app whose provider starts on model-a and can cycle to model-b.
        let mut app = create_cycle_switch_test_app("model-a", vec!["model-a", "model-b"]);
        app.session.provider_key = Some("cycle-test".to_string());

        // Cycle forward: model-a -> model-b
        app.cycle_model(1);

        // The session should reflect model-b
        assert_eq!(
            app.session.model.as_deref(),
            Some("model-b"),
            "session model should be model-b after cycling"
        );

        // The config file on disk should not be rewritten until the debounce fires.
        crate::config::invalidate_config_cache();
        assert_eq!(
            read_config_default_model().as_deref(),
            Some("model-a"),
            "config.toml default_model should stay model-a before debounced persist flush"
        );

        flush_debounced_model_persist(&mut app);
        crate::config::invalidate_config_cache();
        assert_eq!(
            read_config_default_model().as_deref(),
            Some("model-b"),
            "config.toml default_model should be model-b after cycle_model (persists across relaunch)"
        );

        // Cycle backward: model-b -> model-a
        app.cycle_model(-1);
        flush_debounced_model_persist(&mut app);
        crate::config::invalidate_config_cache();
        assert_eq!(
            read_config_default_model().as_deref(),
            Some("model-a"),
            "config.toml default_model should be model-a after cycling back"
        );
    });
}

#[test]
fn model_command_persists_switch_to_config_toml() {
    with_temp_jcode_home(|| {
        write_test_config(
            "[provider]\ndefault_model = \"model-a\"\ndefault_provider = \"cycle-test\"\n",
        );
        crate::config::invalidate_config_cache();

        let mut app = create_cycle_switch_test_app("model-a", vec!["model-a", "model-b"]);
        app.session.provider_key = Some("cycle-test".to_string());

        // Use /model model-b command (the handle_model_command path)
        let result = super::model_context::handle_model_command(&mut app, "/model model-b");
        assert!(result, "handle_model_command should return true for a valid model");

        flush_debounced_model_persist(&mut app);
        crate::config::invalidate_config_cache();
        assert_eq!(
            read_config_default_model().as_deref(),
            Some("model-b"),
            "config.toml default_model should be model-b after /model model-b command"
        );
        assert_eq!(
            app.session.model.as_deref(),
            Some("model-b"),
            "session model should be model-b after /model command"
        );
    });
}

/// Test that the model picker path (which calls `persist_model_switch_to_config`
/// with a route-prefixed spec and an explicit provider_key) persists the
/// selection to config.toml.
///
/// Before the fix, the model picker only saved to the session file. A new
/// session or relaunch read config.toml and reverted to the old default. This
/// test exercises the same call the picker makes: `persist_model_switch_to_config`
/// with a route spec like "copilot:gpt-5" and an explicit provider_key.
#[test]
fn model_picker_selection_persists_switch_to_config_toml() {
    with_temp_jcode_home(|| {
        write_test_config(
            "[provider]\ndefault_model = \"model-a\"\ndefault_provider = \"cycle-test\"\n",
        );
        crate::config::invalidate_config_cache();
        assert_eq!(
            read_config_default_model().as_deref(),
            Some("model-a"),
            "precondition: config should have model-a as default"
        );

        let mut app = create_cycle_switch_test_app("model-a", vec!["model-a", "model-b"]);
        app.session.provider_key = Some("cycle-test".to_string());

        // Simulate the model picker path: the picker computes a route spec
        // (e.g. "copilot:model-b") and a provider_key from the route, then
        // calls persist_model_switch_to_config with both. This is the exact
        // call added in inline_interactive.rs after set_route_selection.
        let picker_spec = "copilot:model-b";
        let picker_provider_key = Some("copilot");
        app.persist_model_switch_to_config(picker_spec, picker_provider_key);

        flush_debounced_model_persist(&mut app);
        // The config file on disk should reflect the picker's choice.
        crate::config::invalidate_config_cache();
        assert_eq!(
            read_config_default_model().as_deref(),
            Some("copilot:model-b"),
            "config.toml default_model should be copilot:model-b after model picker selection"
        );
    });
}

/// Test that `persist_model_switch_to_config` with `None` provider_key falls
/// back to the session's current provider_key (the path used by cycle_model
/// and /model <name>).
#[test]
fn persist_model_switch_uses_session_provider_key_when_none() {
    with_temp_jcode_home(|| {
        write_test_config(
            "[provider]\ndefault_model = \"model-a\"\ndefault_provider = \"cycle-test\"\n",
        );
        crate::config::invalidate_config_cache();

        let mut app = create_cycle_switch_test_app("model-a", vec!["model-a", "model-b"]);
        app.session.provider_key = Some("my-provider".to_string());

        // Passing None for provider_key should use session.provider_key.
        app.persist_model_switch_to_config("model-b", None);

        flush_debounced_model_persist(&mut app);
        crate::config::invalidate_config_cache();
        assert_eq!(
            read_config_default_model().as_deref(),
            Some("model-b"),
            "config.toml default_model should be model-b"
        );
        let cfg = crate::config::Config::load();
        assert_eq!(
            cfg.provider.default_provider.as_deref(),
            Some("my-provider"),
            "config.toml default_provider should match session provider_key"
        );
    });
}

#[test]
fn persist_model_switch_toggle_disables_config_write() {
    with_temp_jcode_home(|| {
        write_test_config(
            "[provider]\ndefault_model = \"model-a\"\ndefault_provider = \"cycle-test\"\npersist_model_switch = false\n",
        );
        crate::config::invalidate_config_cache();

        let mut app = create_cycle_switch_test_app("model-a", vec!["model-a", "model-b"]);
        app.session.provider_key = Some("cycle-test".to_string());

        app.cycle_model(1);
        std::thread::sleep(Duration::from_millis(550));
        assert!(
            !app.maybe_flush_pending_model_config_persist(),
            "disabled model persistence should not schedule a debounced write"
        );

        crate::config::invalidate_config_cache();
        assert_eq!(
            read_config_default_model().as_deref(),
            Some("model-a"),
            "config.toml default_model should not change when persist_model_switch=false"
        );
        assert_eq!(
            app.session.model.as_deref(),
            Some("model-b"),
            "session model should still change when config persistence is disabled"
        );
    });
}
