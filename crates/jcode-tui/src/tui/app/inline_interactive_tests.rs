#[test]
fn ssh_model_picker_uses_wire_catalog_without_local_cache() {
    if crate::tui::app::commands_dispatch::ssh_test_runs_in_child(
        "ssh_model_picker_uses_wire_catalog_without_local_cache",
    ) {
        return;
    }
    assert!(super::model_picker_usage_path().is_none());
    assert!(super::model_picker_favorites_path().is_none());
    assert!(super::remote_model_catalog_cache_path().is_none());
    let mut app = crate::tui::app::tests::create_test_app();
    app.is_remote = true;
    app.remote_startup_phase = None;
    app.remote_provider_name = Some("remote-provider".to_string());
    app.remote_provider_model = Some("remote-test-model".to_string());
    app.remote_available_entries = vec!["remote-test-model".to_string()];
    assert!(!app.hydrate_remote_model_catalog_cache());
    let routes = app.build_remote_model_routes_fallback();
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].model, "remote-test-model");
    assert_eq!(routes[0].api_method, "remote-catalog");
    app.open_model_picker();
    assert!(app.inline_interactive_state.is_some());
    assert!(app.pending_model_picker_load.is_none());
    app.persist_remote_model_catalog_cache();
    app.handle_inline_interactive_key(KeyCode::Char('o'), crossterm::event::KeyModifiers::CONTROL)
        .unwrap();
    assert!(
        app.display_messages
            .last()
            .unwrap()
            .content
            .contains("Saving a default model")
    );
    // Choosing a runtime route still stages a request for the remote daemon.
    for _ in 0..3 {
        if app.inline_interactive_state.is_some() {
            app.handle_inline_interactive_key(KeyCode::Enter, crossterm::event::KeyModifiers::NONE)
                .unwrap();
        }
    }
    assert!(app.pending_model_switch.is_some());
}

use super::{
    REMOTE_MODEL_CATALOG_CACHE_MAX_AGE_SECS, REMOTE_MODEL_CATALOG_CACHE_VERSION,
    REMOTE_MODEL_CATALOG_MAX_DETAIL_BYTES, RemoteModelCatalogCache,
    filter_routes_by_provider_allowlist, fold_model_picker_usage_efforts,
    key_char_eq_ignore_ascii_case, model_effort_step_preselection, model_picker_route_is_current,
    model_picker_route_is_default, model_picker_route_is_recommended,
    model_route_effort_levels_with_config, next_model_favorite_after_current,
    picker_is_runtime_model_picker, remote_model_catalog_cache_is_fresh,
    remote_model_catalog_cache_origin, remote_model_catalog_snapshot_is_safe,
    route_supports_reasoning_effort,
};
use crate::tui::{
    AgentModelTarget, App, InlineInteractiveState, PickerAction, PickerEntry, PickerKind,
    PickerOption,
};
use crossterm::event::KeyCode;

fn picker_entry(name: &str, provider: &str, usage_score: u32) -> PickerEntry {
    PickerEntry {
        name: name.to_string(),
        options: vec![picker_option(provider)],
        action: PickerAction::Model,
        selected_option: 0,
        is_current: false,
        is_default: false,
        is_favorite: false,
        recommended: false,
        recommendation_rank: usize::MAX,
        usage_score,
        old: false,
        created_date: None,
        effort: None,
    }
}

fn picker_option_with_method(provider: &str, api_method: &str) -> PickerOption {
    PickerOption {
        provider: provider.to_string(),
        api_method: api_method.to_string(),
        available: true,
        detail: String::new(),
        estimated_reference_cost_micros: None,
    }
}

fn picker_option(provider: &str) -> PickerOption {
    picker_option_with_method(provider, "test")
}

struct ConfigHomeGuard(Option<std::ffi::OsString>);

impl Drop for ConfigHomeGuard {
    fn drop(&mut self) {
        match self.0.take() {
            Some(home) => crate::env::set_var("JCODE_HOME", home),
            None => crate::env::remove_var("JCODE_HOME"),
        }
        crate::config::Config::invalidate_cache();
    }
}

#[test]
fn named_profile_default_level_is_persisted_per_model() {
    let _env = crate::storage::lock_test_env();
    let _home = ConfigHomeGuard(std::env::var_os("JCODE_HOME"));
    let dir = tempfile::TempDir::new().unwrap();
    crate::env::set_var("JCODE_HOME", dir.path());
    crate::config::Config::invalidate_cache();
    std::fs::write(dir.path().join("config.toml"),
        "[providers.custom]\ntype = \"openai-compatible\"\nbase_url = \"https://example.invalid/v1\"\n").unwrap();
    let mut app = crate::tui::app::tests::create_test_app();
    let mut entry = picker_entry("kimi-k3", "custom", 0);
    entry.options = vec![picker_option_with_method(
        "custom",
        "openai-compatible:custom",
    )];
    app.save_model_picker_default(0, &entry, Some("high"));
    crate::config::Config::invalidate_cache();
    let saved = crate::config::Config::load();
    let effort = saved.providers["custom"]
        .models
        .iter()
        .find(|model| model.id == "kimi-k3")
        .and_then(|model| model.reasoning_effort.as_deref());
    assert_eq!(effort, Some("high"));
    assert_eq!(
        super::saved_default_reasoning_effort("kimi-k3", &entry.options[0]).as_deref(),
        Some("high")
    );
}

#[test]
fn named_profile_effort_preserves_case_trim_fields_and_clear_semantics() {
    let _env = crate::storage::lock_test_env();
    let _home = ConfigHomeGuard(std::env::var_os("JCODE_HOME"));
    let dir = tempfile::tempdir().unwrap();
    crate::env::set_var("JCODE_HOME", dir.path());
    std::fs::write(
        dir.path().join("config.toml"),
        r#"
[providers.Custom]
type = "openai-compatible"
base_url = "https://example.invalid/v1"
[[providers.Custom.models]]
id = " Kimi-K3 "
context_window = 123456
reasoning = true
reasoning_effort = "low"
"#,
    )
    .unwrap();
    crate::config::invalidate_config_cache();
    crate::config::Config::set_named_provider_model_reasoning_effort(
        "custom",
        " kimi-k3 ",
        Some("high"),
    )
    .unwrap();
    let loaded = crate::config::Config::load();
    assert_eq!(loaded.providers["Custom"].models.len(), 1);
    assert_eq!(
        loaded.providers["Custom"].models[0].context_window,
        Some(123456)
    );
    assert_eq!(loaded.providers["Custom"].models[0].reasoning, Some(true));
    assert_eq!(
        super::named_model_default_effort(&loaded, "openai-compatible:CUSTOM", "KIMI-K3")
            .as_deref(),
        Some("high")
    );
    crate::config::Config::set_named_provider_model_reasoning_effort("CUSTOM", "kimi-k3", None)
        .unwrap();
    assert!(
        crate::config::Config::load().providers["Custom"].models[0]
            .reasoning_effort
            .is_none()
    );
    crate::config::Config::set_named_provider_model_reasoning_effort("custom", "unknown", None)
        .unwrap();
    assert_eq!(
        crate::config::Config::load().providers["Custom"]
            .models
            .len(),
        1
    );
    assert!(
        crate::config::Config::set_named_provider_model_reasoning_effort(
            "missing",
            "kimi-k3",
            Some("high")
        )
        .is_err()
    );
    assert!(
        crate::config::Config::set_named_provider_model_reasoning_effort(
            "custom",
            " ",
            Some("high")
        )
        .is_err()
    );
    std::fs::write(dir.path().join("config.toml"), "[broken").unwrap();
    assert!(
        crate::config::Config::set_named_provider_model_reasoning_effort(
            "custom",
            "kimi-k3",
            Some("high")
        )
        .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("config.toml")).unwrap(),
        "[broken"
    );
}

#[test]
fn unsupported_default_effort_does_not_claim_saved_level() {
    let _env = crate::storage::lock_test_env();
    let _home = ConfigHomeGuard(std::env::var_os("JCODE_HOME"));
    let dir = tempfile::tempdir().unwrap();
    crate::env::set_var("JCODE_HOME", dir.path());
    crate::config::invalidate_config_cache();
    let mut app = crate::tui::app::tests::create_test_app();
    let mut entry = picker_entry("example-model", "unknown", 0);
    entry.options = vec![picker_option_with_method("unknown", "unknown")];
    app.save_model_picker_default(0, &entry, Some("high"));
    assert!(
        app.status_notice.as_ref().is_some_and(
            |(notice, _)| notice.starts_with("Default →") && !notice.contains("(high)")
        )
    );
    assert!(
        !app.display_messages()
            .iter()
            .any(|m| m.content.contains("(high)"))
    );
}

struct LevelRejectingProvider;

#[async_trait::async_trait]
impl crate::provider::Provider for LevelRejectingProvider {
    async fn complete(
        &self,
        _messages: &[crate::message::Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> anyhow::Result<crate::provider::EventStream> {
        unimplemented!("no inference in picker regression")
    }
    fn name(&self) -> &str {
        "custom"
    }
    fn model(&self) -> String {
        "deepseek-v4".into()
    }
    fn set_route_selection(
        &self,
        _selection: &crate::provider::RouteSelection,
    ) -> anyhow::Result<()> {
        Ok(())
    }
    fn set_model(&self, _model: &str) -> anyhow::Result<()> {
        Ok(())
    }
    fn set_reasoning_effort(&self, effort: &str) -> anyhow::Result<()> {
        if effort == "high" {
            Ok(())
        } else {
            anyhow::bail!("unsupported effort {effort}")
        }
    }
    fn fork(&self) -> std::sync::Arc<dyn crate::provider::Provider> {
        std::sync::Arc::new(Self)
    }
}

#[test]
fn rejected_reasoning_level_is_reported_instead_of_claimed() {
    let _env = crate::storage::lock_test_env();
    let mut app = crate::tui::app::tests::create_test_app();
    app.provider = std::sync::Arc::new(LevelRejectingProvider);
    let mut entry = picker_entry("deepseek-v4", "custom", 0);
    entry.options = vec![picker_option_with_method(
        "custom",
        "openai-compatible:custom",
    )];
    app.apply_model_picker_choice(entry.clone(), Some("xhigh".into()));
    assert!(
        app.display_messages
            .last()
            .unwrap()
            .content
            .contains("`xhigh` was not applied")
    );
    assert!(
        app.status_notice
            .as_ref()
            .is_some_and(|(notice, _)| notice.contains("level unchanged"))
    );
    app.apply_model_picker_choice(entry, Some("high".into()));
    assert!(
        app.status_notice
            .as_ref()
            .is_some_and(|(notice, _)| !notice.contains("level unchanged"))
    );
}

#[test]
fn named_compatible_route_opens_reasoning_level_step_when_enabled() {
    let mut config = crate::config::Config::default();
    config.providers.insert(
        "custom".into(),
        crate::config::NamedProviderConfig {
            supports_reasoning_effort: Some(true),
            ..Default::default()
        },
    );
    let route = picker_option_with_method("custom", "openai-compatible:custom");
    assert_eq!(
        model_route_effort_levels_with_config("kimi-k3", &route, &config),
        vec!["none", "low", "medium", "high", "max"]
    );
}

#[test]
fn named_compatible_route_skips_level_step_when_disabled() {
    let mut config = crate::config::Config::default();
    config.providers.insert(
        "custom".into(),
        crate::config::NamedProviderConfig {
            supports_reasoning_effort: Some(false),
            ..Default::default()
        },
    );
    let route = picker_option_with_method("custom", "openai-compatible:custom");
    assert!(model_route_effort_levels_with_config("kimi-k3", &route, &config).is_empty());
}

#[test]
fn saved_account_model_picker_current_and_default_are_account_specific() {
    let work = picker_option_with_method("OpenAI", "openai-oauth@work");
    let personal = picker_option_with_method("OpenAI", "openai-oauth@personal");
    assert!(super::model_picker_route_is_current(
        "gpt-5.5",
        &work,
        "gpt-5.5",
        "OpenAI",
        Some("openai-oauth@work")
    ));
    assert!(!super::model_picker_route_is_current(
        "gpt-5.5",
        &personal,
        "gpt-5.5",
        "OpenAI",
        Some("openai-oauth@work")
    ));
    assert!(super::model_picker_route_is_default(
        "gpt-5.5",
        &work,
        Some("openai-oauth@work:gpt-5.5"),
        Some("openai-oauth")
    ));
    assert!(!super::model_picker_route_is_default(
        "gpt-5.5",
        &personal,
        Some("openai-oauth@work:gpt-5.5"),
        Some("openai-oauth")
    ));
    assert!(super::remote_catalog_api_method_is_safe(
        "openai-oauth@work"
    ));
    assert!(super::remote_catalog_api_method_is_safe(
        "claude-oauth@personal"
    ));
    assert!(!super::remote_catalog_api_method_is_safe(
        "claude-oauth@work:bad"
    ));
}

#[test]
fn model_picker_hotkey_char_matching_is_case_insensitive() {
    assert!(key_char_eq_ignore_ascii_case(KeyCode::Char('f'), 'f'));
    assert!(key_char_eq_ignore_ascii_case(KeyCode::Char('F'), 'f'));
    assert!(key_char_eq_ignore_ascii_case(KeyCode::Char('D'), 'd'));
    assert!(!key_char_eq_ignore_ascii_case(KeyCode::Char('x'), 'f'));
}

#[test]
fn runtime_model_picker_scope_excludes_agent_model_picker() {
    let runtime = InlineInteractiveState {
        kind: PickerKind::Model,
        filtered: vec![0],
        entries: vec![picker_entry("gpt-5.5", "OpenAI", 0)],
        selected: 0,
        column: 0,
        filter: String::new(),
        preview: false,
        scoped_route_restore: Vec::new(),
        effort_step: None,
    };
    let mut agent_entry = picker_entry("Swarm / subagent", "gpt-5 default", 0);
    agent_entry.action = PickerAction::AgentTarget(AgentModelTarget::Swarm);
    let agent = InlineInteractiveState {
        kind: PickerKind::Model,
        filtered: vec![0],
        entries: vec![agent_entry],
        selected: 0,
        column: 0,
        filter: String::new(),
        preview: false,
        scoped_route_restore: Vec::new(),
        effort_step: None,
    };

    assert!(picker_is_runtime_model_picker(&runtime));
    assert!(!picker_is_runtime_model_picker(&agent));
}

#[test]
fn favorite_cycle_reaches_every_favorite_across_fresh_picker_sorts() {
    // Favorites are model + route rows, with reasoning level chosen separately.
    let favorite_specs = [
        ("claude-opus-5", "Anthropic", 2550),
        ("claude-opus-4-8", "Anthropic", 950),
        ("claude-opus-4-8", "Bedrock", 0),
        ("claude-opus-4-8", "Copilot", 0),
    ];
    let mut current = ("claude-opus-5".to_string(), "Anthropic".to_string());
    let mut visited = Vec::new();
    for _ in 0..favorite_specs.len() {
        let mut entries: Vec<_> = favorite_specs
            .iter()
            .map(|(model, provider, usage_score)| {
                let mut entry = picker_entry(model, provider, *usage_score);
                entry.is_favorite = true;
                entry.is_current = (model.to_string(), provider.to_string()) == current;
                entry
            })
            .collect();
        entries.sort_by_key(|entry| {
            (
                !entry.is_current,
                std::cmp::Reverse(entry.usage_score),
                entry.name.clone(),
            )
        });
        let picker = InlineInteractiveState {
            kind: PickerKind::Model,
            filtered: (0..entries.len()).collect(),
            entries,
            selected: 0,
            column: 0,
            filter: String::new(),
            preview: false,
            scoped_route_restore: Vec::new(),
            effort_step: None,
        };
        let next = next_model_favorite_after_current(&picker)
            .expect("a favorited model should be selectable");
        let entry = &picker.entries[picker.filtered[next]];
        current = (
            entry.name.clone(),
            entry.active_option().unwrap().provider.clone(),
        );
        visited.push(format!("{} via {}", current.0, current.1));
    }
    assert_eq!(
        visited,
        [
            "claude-opus-4-8 via Anthropic",
            "claude-opus-4-8 via Bedrock",
            "claude-opus-4-8 via Copilot",
            "claude-opus-5 via Anthropic"
        ]
    );
}

#[test]
fn model_picker_fuzzy_filter_prefers_previously_selected_route() {
    let mut picker = InlineInteractiveState {
        kind: PickerKind::Model,
        filtered: vec![0, 1],
        entries: vec![
            picker_entry("claude-opus-4.6", "Cursor", 0),
            picker_entry("claude-opus-4.5", "Anthropic", 150),
        ],
        selected: 0,
        column: 0,
        filter: "opus".to_string(),
        preview: false,
        scoped_route_restore: Vec::new(),
        effort_step: None,
    };

    App::apply_inline_interactive_filter(&mut picker);

    assert_eq!(picker.filtered, vec![1, 0]);
}

#[test]
fn model_picker_fuzzy_filter_tolerates_common_typos() {
    let mut picker = InlineInteractiveState {
        kind: PickerKind::Model,
        filtered: vec![0, 1],
        entries: vec![
            picker_entry("gpt-5-codex", "OpenAI", 0),
            picker_entry("claude-opus-4.6", "Anthropic", 0),
        ],
        selected: 0,
        column: 0,
        filter: "codxe".to_string(),
        preview: false,
        scoped_route_restore: Vec::new(),
        effort_step: None,
    };

    App::apply_inline_interactive_filter(&mut picker);

    assert_eq!(picker.filtered, vec![0]);
}

#[test]
fn model_picker_exact_name_outranks_longer_frequently_used_prefix() {
    let mut picker = InlineInteractiveState {
        kind: PickerKind::Model,
        filtered: vec![0, 1],
        entries: vec![
            picker_entry("gpt-5.5", "OpenAI", 150),
            picker_entry("gpt-5", "OpenAI", 0),
        ],
        selected: 0,
        column: 0,
        filter: "gpt-5".to_string(),
        preview: false,
        scoped_route_restore: Vec::new(),
        effort_step: None,
    };

    App::apply_inline_interactive_filter(&mut picker);

    assert_eq!(picker.filtered, vec![1, 0]);
}

#[test]
fn model_picker_current_route_requires_matching_provider() {
    let openai_route = picker_option("OpenAI");
    let copilot_route = picker_option("Copilot");

    assert!(model_picker_route_is_current(
        "gpt-5.5",
        &openai_route,
        "gpt-5.5",
        "OpenAI",
        None,
    ));
    assert!(!model_picker_route_is_current(
        "gpt-5.5",
        &copilot_route,
        "gpt-5.5",
        "OpenAI",
        None,
    ));
}

#[test]
fn model_picker_current_route_requires_matching_api_method() {
    let oauth_route = picker_option_with_method("Anthropic", "claude-oauth");
    let api_key_route = picker_option_with_method("Anthropic", "claude-api");

    assert!(!model_picker_route_is_current(
        "claude-fable-5",
        &oauth_route,
        "claude-fable-5",
        "Claude",
        Some("claude-api"),
    ));
    assert!(model_picker_route_is_current(
        "claude-fable-5",
        &api_key_route,
        "claude-fable-5",
        "Claude",
        Some("claude-api"),
    ));
}

#[test]
fn model_picker_current_route_allows_provider_aliases() {
    assert!(jcode_provider_core::model_route_provider_labels_match(
        "Anthropic",
        "Claude"
    ));
    assert!(jcode_provider_core::model_route_provider_labels_match(
        "auto",
        "OpenRouter"
    ));
    assert!(jcode_provider_core::model_route_provider_labels_match(
        "GitHub Copilot",
        "Copilot"
    ));
    assert!(jcode_provider_core::model_route_provider_labels_match(
        "AWS Bedrock",
        "Bedrock"
    ));
}

#[test]
fn model_picker_provider_match_does_not_use_substring_false_positives() {
    assert!(!jcode_provider_core::model_route_provider_labels_match(
        "OpenRouter/OpenAI",
        "OpenAI"
    ));
    assert!(!jcode_provider_core::model_route_provider_labels_match(
        "OpenAI",
        "OpenRouter"
    ));
}

#[test]
fn model_picker_default_route_requires_matching_provider_when_config_has_provider() {
    let openai_route = picker_option_with_method("OpenAI", "openai-oauth");
    let copilot_route = picker_option_with_method("Copilot", "copilot");

    assert!(model_picker_route_is_default(
        "gpt-5.5",
        &openai_route,
        Some("gpt-5.5"),
        Some("openai"),
    ));
    assert!(!model_picker_route_is_default(
        "gpt-5.5",
        &copilot_route,
        Some("gpt-5.5"),
        Some("openai"),
    ));
}

#[test]
fn model_picker_default_route_marks_anthropic_api_config_provider() {
    // Regression: config `default_provider = "anthropic-api"` is the
    // dual-auth spelling of the route keyed `anthropic-api-key`. The picker
    // must still mark the Anthropic API-key route as the default ★ even
    // though the two spellings normalize differently, and must NOT mark the
    // OAuth route for the same model.
    let api_route = picker_option_with_method("Anthropic", "anthropic-api-key");
    let oauth_route = picker_option_with_method("Anthropic", "claude-oauth");

    assert!(model_picker_route_is_default(
        "claude-opus-4-8",
        &api_route,
        Some("claude-opus-4-8"),
        Some("anthropic-api"),
    ));
    assert!(!model_picker_route_is_default(
        "claude-opus-4-8",
        &oauth_route,
        Some("claude-opus-4-8"),
        Some("anthropic-api"),
    ));

    // The equivalent `claude-api` spelling behaves identically.
    assert!(model_picker_route_is_default(
        "claude-opus-4-8",
        &api_route,
        Some("claude-opus-4-8"),
        Some("claude-api"),
    ));
}

#[test]
fn model_effort_step_preselects_current_then_saved_then_high() {
    let levels = ["none", "low", "medium", "high", "xhigh", "max"];
    assert_eq!(
        model_effort_step_preselection(&levels, Some("low"), Some("max")),
        1
    );
    assert_eq!(
        model_effort_step_preselection(&levels, None, Some("max")),
        5
    );
    assert_eq!(model_effort_step_preselection(&levels, None, None), 3);
    assert_eq!(
        model_effort_step_preselection(&levels, Some("minimal"), Some("MAX")),
        5
    );
    assert_eq!(model_effort_step_preselection(&["low"], None, None), 0);
}

#[test]
fn model_picker_usage_folds_effort_keyed_history_into_model_rows() {
    let route = picker_option_with_method("OpenAI", "openai-oauth");
    let key = |effort: &str| format!("gpt-5.5\u{1f}OpenAI\u{1f}openai-oauth\u{1f}{effort}");
    let mut store = super::ModelPickerUsageStore::default();
    for (effort, count, secs) in [("high", 3, 10), ("low", 2, 30), ("", 1, 20)] {
        store.selections.insert(
            key(effort),
            super::ModelPickerUsageEntry {
                count,
                last_selected_unix_secs: secs,
            },
        );
    }
    fold_model_picker_usage_efforts(&mut store);
    assert_eq!(store.selections.len(), 1);
    let entry = &store.selections[&super::model_picker_usage_key("gpt-5.5", &route)];
    assert_eq!(entry.count, 6, "every level's picks count for the model");
    assert_eq!(entry.last_selected_unix_secs, 30);
    assert_eq!(
        super::model_picker_usage_score(&store, "gpt-5.5", &route),
        650
    );
}

#[test]
fn model_picker_default_route_honors_provider_prefixed_model_specs() {
    let openai_route = picker_option_with_method("OpenAI", "openai-oauth");
    let copilot_route = picker_option_with_method("Copilot", "copilot");

    assert!(model_picker_route_is_default(
        "gpt-5.5",
        &copilot_route,
        Some("copilot:gpt-5.5"),
        None,
    ));
    assert!(!model_picker_route_is_default(
        "gpt-5.5",
        &openai_route,
        Some("copilot:gpt-5.5"),
        None,
    ));
}

#[test]
fn model_picker_default_route_matches_openrouter_endpoint_specs() {
    let openrouter_openai_route = picker_option_with_method("OpenAI", "openrouter");

    assert!(model_picker_route_is_default(
        "gpt-5.5",
        &openrouter_openai_route,
        Some("openai/gpt-5.5@OpenAI"),
        Some("openrouter"),
    ));
    assert!(!model_picker_route_is_default(
        "gpt-5.5",
        &openrouter_openai_route,
        Some("anthropic/gpt-5.5@OpenAI"),
        Some("openrouter"),
    ));
}

#[test]
fn model_picker_recommended_route_is_provider_aware() {
    let openai_oauth_route = picker_option_with_method("OpenAI", "openai-oauth");
    let openai_api_key_route = picker_option_with_method("OpenAI", "openai-api-key");
    let copilot_route = picker_option_with_method("Copilot", "copilot");
    let claude_oauth_route = picker_option_with_method("Anthropic", "claude-oauth");
    let claude_openrouter_route = picker_option_with_method("Anthropic", "openrouter");
    let openrouter_auto_route = picker_option_with_method("auto", "openrouter");
    let openrouter_provider_route = picker_option_with_method("DeepSeek", "openrouter");
    let deepseek_direct_route = picker_option_with_method("DeepSeek", "openai-compatible:deepseek");
    let unavailable_openai_oauth_route = PickerOption {
        available: false,
        ..openai_oauth_route.clone()
    };

    assert!(model_picker_route_is_recommended(
        jcode_provider_core::DEFAULT_OPENAI_MODEL,
        &openai_oauth_route
    ));
    assert!(!model_picker_route_is_recommended(
        jcode_provider_core::DEFAULT_OPENAI_MODEL,
        &openai_api_key_route
    ));
    assert!(!model_picker_route_is_recommended(
        jcode_provider_core::DEFAULT_OPENAI_MODEL,
        &copilot_route
    ));
    assert!(!model_picker_route_is_recommended(
        jcode_provider_core::DEFAULT_OPENAI_MODEL,
        &unavailable_openai_oauth_route,
    ));

    // Recommendations follow the curated defaults (see jcode-provider-core):
    // the default Claude flagship is recommended; older Opus and
    // OpenRouter/Copilot routes are not.
    assert!(model_picker_route_is_recommended(
        jcode_provider_core::DEFAULT_CLAUDE_MODEL,
        &claude_oauth_route,
    ));
    assert!(!model_picker_route_is_recommended(
        "claude-opus-4-8",
        &claude_oauth_route,
    ));
    assert!(!model_picker_route_is_recommended(
        jcode_provider_core::DEFAULT_CLAUDE_MODEL,
        &claude_openrouter_route,
    ));
    assert!(!model_picker_route_is_recommended(
        jcode_provider_core::DEFAULT_CLAUDE_MODEL,
        &copilot_route,
    ));

    // DeepSeek routes are no longer in the recommended set at all.
    assert!(!model_picker_route_is_recommended(
        "deepseek/deepseek-v4-pro",
        &openrouter_auto_route,
    ));
    assert!(!model_picker_route_is_recommended(
        "deepseek/deepseek-v4-pro",
        &deepseek_direct_route,
    ));
    assert!(!model_picker_route_is_recommended(
        "deepseek/deepseek-v4-pro",
        &openrouter_provider_route,
    ));
}

#[test]
fn remote_model_catalog_cache_keeps_flattened_legacy_schema() {
    let cache: RemoteModelCatalogCache = serde_json::from_value(serde_json::json!({
        "version": 1,
        "provider_name": "OpenAI",
        "provider_model": "gpt-5.5",
        "available_models": ["gpt-5.5"],
        "model_routes": [{
            "model": "gpt-5.5",
            "provider": "OpenAI",
            "api_method": "openai-oauth",
            "available": true,
            "detail": "OAuth"
        }],
        "observed_at_unix_secs": 123,
    }))
    .expect("legacy flattened remote cache should deserialize");

    assert_eq!(cache.snapshot.provider_name.as_deref(), Some("OpenAI"));
    assert_eq!(cache.snapshot.provider_model.as_deref(), Some("gpt-5.5"));
    assert_eq!(cache.snapshot.available_models, ["gpt-5.5"]);
    assert_eq!(cache.snapshot.model_routes.len(), 1);
    assert!(cache.origin.is_empty());
    assert_eq!(
        cache.snapshot.model_routes[0].api_method_kind(),
        crate::provider::ModelRouteApiMethod::OpenAIOAuth
    );

    let serialized = serde_json::to_value(&cache).expect("cache should serialize");
    assert_eq!(serialized["provider_name"], "OpenAI");
    assert!(serialized.get("snapshot").is_none());
}

#[test]
fn remote_model_catalog_cache_rejects_stale_and_future_timestamps() {
    let snapshot = jcode_provider_core::ModelCatalogSnapshot::new(
        Some("OpenAI".to_string()),
        Some("gpt-5.5".to_string()),
        vec!["gpt-5.5".to_string()],
        vec![model_route("gpt-5.5", "OpenAI", "openai-oauth")],
    );
    let now = REMOTE_MODEL_CATALOG_CACHE_MAX_AGE_SECS + 10_000;
    let mut cache = RemoteModelCatalogCache {
        version: REMOTE_MODEL_CATALOG_CACHE_VERSION,
        origin: remote_model_catalog_cache_origin(),
        snapshot,
        observed_at_unix_secs: now,
    };

    assert!(remote_model_catalog_cache_is_fresh(&cache, now));
    cache.observed_at_unix_secs = now - REMOTE_MODEL_CATALOG_CACHE_MAX_AGE_SECS - 1;
    assert!(!remote_model_catalog_cache_is_fresh(&cache, now));
    cache.observed_at_unix_secs = now + 5 * 60 + 1;
    assert!(!remote_model_catalog_cache_is_fresh(&cache, now));
}

#[test]
fn remote_model_catalog_cache_rejects_forged_or_oversized_routes() {
    let safe_snapshot = jcode_provider_core::ModelCatalogSnapshot::new(
        Some("AWS Bedrock".to_string()),
        Some("us.anthropic.claude-sonnet-4-6".to_string()),
        vec!["us.anthropic.claude-sonnet-4-6".to_string()],
        vec![model_route(
            "us.anthropic.claude-sonnet-4-6",
            "AWS Bedrock",
            "bedrock",
        )],
    );
    assert!(remote_model_catalog_snapshot_is_safe(&safe_snapshot));

    let mut forged = safe_snapshot.clone();
    forged.model_routes[0].api_method = "shell:steal-credentials".to_string();
    assert!(!remote_model_catalog_snapshot_is_safe(&forged));

    let mut control = safe_snapshot.clone();
    control.model_routes[0].provider = "AWS Bedrock\nOpenAI".to_string();
    assert!(!remote_model_catalog_snapshot_is_safe(&control));

    let mut oversized = safe_snapshot;
    oversized.model_routes[0].detail = "x".repeat(REMOTE_MODEL_CATALOG_MAX_DETAIL_BYTES + 1);
    assert!(!remote_model_catalog_snapshot_is_safe(&oversized));
}

fn model_route(model: &str, provider: &str, api_method: &str) -> crate::provider::ModelRoute {
    crate::provider::ModelRoute {
        catalog_metadata: None,
        model: model.to_string(),
        provider: provider.to_string(),
        api_method: api_method.to_string(),
        available: true,
        detail: String::new(),
        usage: None,
        cheapness: None,
    }
}

#[test]
fn route_effort_support_covers_effort_capable_runtimes_only() {
    assert!(route_supports_reasoning_effort("claude-oauth"));
    assert!(route_supports_reasoning_effort("claude-api"));
    assert!(route_supports_reasoning_effort("openai-oauth"));
    assert!(route_supports_reasoning_effort("openai-api-key"));
    assert!(route_supports_reasoning_effort("openrouter"));
    assert!(!route_supports_reasoning_effort(
        "openai-compatible:llamacpp"
    ));
    assert!(!route_supports_reasoning_effort("openai-compatible:zai"));
    assert!(!route_supports_reasoning_effort("copilot"));
    assert!(!route_supports_reasoning_effort("bedrock"));
    assert!(!route_supports_reasoning_effort("https"));
    assert!(!route_supports_reasoning_effort("openai-compatible"));
    assert!(!route_supports_reasoning_effort("remote-catalog"));
    assert!(!route_supports_reasoning_effort("current"));
}

#[test]
fn provider_allowlist_filters_routes_by_label_method_and_profile() {
    let routes = vec![
        model_route("gpt-5.5", "OpenAI", "openai-oauth"),
        model_route("claude-fable-5", "Anthropic", "claude-oauth"),
        model_route("qwen3-coder", "llama.cpp", "openai-compatible:llamacpp"),
        model_route("deepseek/deepseek-v4-pro", "auto", "openrouter"),
    ];

    // Provider label match (normalized: case/dots/spaces insensitive).
    let filtered = filter_routes_by_provider_allowlist(
        routes.clone(),
        Some(&["Llama.CPP".to_string()]),
        "unrelated-current",
        "OpenAI",
        None,
    );
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].model, "qwen3-coder");

    // Bare openai-compatible profile id match.
    let filtered = filter_routes_by_provider_allowlist(
        routes.clone(),
        Some(&["llamacpp".to_string()]),
        "unrelated-current",
        "OpenAI",
        None,
    );
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].provider, "llama.cpp");

    // Api-method match plus alias-aware provider label match.
    let filtered = filter_routes_by_provider_allowlist(
        routes.clone(),
        Some(&["claude-oauth".to_string(), "openrouter".to_string()]),
        "unrelated-current",
        "OpenAI",
        None,
    );
    let models: Vec<&str> = filtered.iter().map(|r| r.model.as_str()).collect();
    assert_eq!(models, ["claude-fable-5", "deepseek/deepseek-v4-pro"]);
}

#[test]
fn provider_allowlist_keeps_current_model_and_never_empties_picker() {
    let routes = vec![
        model_route("gpt-5.5", "OpenAI", "openai-oauth"),
        model_route("qwen3-coder", "llama.cpp", "openai-compatible:llamacpp"),
    ];

    // Current model's route survives even when its provider is filtered out.
    let filtered = filter_routes_by_provider_allowlist(
        routes.clone(),
        Some(&["llamacpp".to_string()]),
        "gpt-5.5",
        "OpenAI",
        Some("openai-oauth"),
    );
    let models: Vec<&str> = filtered.iter().map(|r| r.model.as_str()).collect();
    assert_eq!(models, ["gpt-5.5", "qwen3-coder"]);

    // A filter matching nothing falls back to the full list.
    let filtered = filter_routes_by_provider_allowlist(
        routes.clone(),
        Some(&["nonexistent".to_string()]),
        "unrelated-current",
        "OpenAI",
        None,
    );
    assert_eq!(filtered.len(), routes.len());

    // None / empty / blank-entry allowlists are no-ops.
    assert_eq!(
        filter_routes_by_provider_allowlist(routes.clone(), None, "x", "OpenAI", None).len(),
        2
    );
    assert_eq!(
        filter_routes_by_provider_allowlist(routes.clone(), Some(&[]), "x", "OpenAI", None,).len(),
        2
    );
    assert_eq!(
        filter_routes_by_provider_allowlist(
            routes,
            Some(&["  ".to_string()]),
            "x",
            "OpenAI",
            None,
        )
        .len(),
        2
    );
}

#[test]
fn provider_allowlist_does_not_keep_disallowed_route_sharing_current_model() {
    let routes = vec![
        model_route(
            "moonshotai/Kimi-K3",
            "my-provider",
            "openai-compatible:my-provider",
        ),
        model_route("moonshotai/Kimi-K3", "Copilot", "copilot"),
    ];

    let filtered = filter_routes_by_provider_allowlist(
        routes,
        Some(&["my-provider".to_string()]),
        "moonshotai/Kimi-K3",
        "my-provider",
        Some("openai-compatible:my-provider"),
    );

    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].provider, "my-provider");
}

#[test]
fn provider_allowlist_keeps_only_exact_active_route_when_model_and_provider_overlap() {
    let routes = vec![
        model_route("gpt-5.5", "OpenAI", "openai-oauth"),
        model_route("gpt-5.5", "OpenAI", "openai-api-key"),
        model_route("gpt-5.5", "Copilot", "copilot"),
        model_route("qwen3-coder", "llama.cpp", "openai-compatible:llamacpp"),
    ];

    let filtered = filter_routes_by_provider_allowlist(
        routes,
        Some(&["llamacpp".to_string()]),
        "gpt-5.5",
        "OpenAI",
        Some("openai-oauth"),
    );

    let routes: Vec<(&str, &str, &str)> = filtered
        .iter()
        .map(|route| {
            (
                route.model.as_str(),
                route.provider.as_str(),
                route.api_method.as_str(),
            )
        })
        .collect();
    assert_eq!(
        routes,
        [
            ("gpt-5.5", "OpenAI", "openai-oauth"),
            ("qwen3-coder", "llama.cpp", "openai-compatible:llamacpp"),
        ]
    );
}
