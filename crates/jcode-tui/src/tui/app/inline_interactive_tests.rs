//! Tests for `#[path]`-attributed module `inline_interactive_tests` of `inline_interactive.rs`.
//!
//! Extracted from the parent file: an inline `#[cfg(test)]` mod counts toward
//! the code-size budget, which only exempts `*_tests.rs` files.

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
    filter_routes_by_provider_allowlist, key_char_eq_ignore_ascii_case,
    model_picker_effort_matches_default, model_picker_route_is_current,
    model_picker_route_is_default, model_picker_route_is_recommended,
    next_model_favorite_after_current, picker_is_runtime_model_picker,
    remote_model_catalog_cache_is_fresh, remote_model_catalog_cache_origin,
    remote_model_catalog_snapshot_is_safe, route_supports_reasoning_effort,
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
    };

    assert!(picker_is_runtime_model_picker(&runtime));
    assert!(!picker_is_runtime_model_picker(&agent));
}

#[test]
fn favorite_cycle_reaches_every_favorite_across_fresh_picker_sorts() {
    let favorite_specs = [
        ("claude-opus-5", "high", 2550),
        ("claude-opus-4-8", "high", 950),
        ("claude-opus-4-8", "medium", 0),
        ("claude-opus-4-8", "low", 0),
    ];
    let mut current_name = "claude-opus-5 (high)".to_string();
    let mut visited = Vec::new();

    for _ in 0..favorite_specs.len() {
        // Rebuild and re-sort the picker exactly as each hotkey press does:
        // the newly current model moves to row zero, while the other
        // favorites retain their usage-based ranking.
        let mut entries: Vec<_> = favorite_specs
            .iter()
            .map(|(model, effort, usage_score)| {
                let effort_label = if *effort == "medium" { "med" } else { effort };
                let mut entry = picker_entry(
                    &format!("{model} ({effort_label})"),
                    "Anthropic",
                    *usage_score,
                );
                entry.effort = Some((*effort).to_string());
                entry.is_favorite = true;
                entry.is_current = entry.name == current_name;
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
        };

        let next = next_model_favorite_after_current(&picker)
            .expect("a favorited model should be selectable");
        current_name = picker.entries[picker.filtered[next]].name.clone();
        visited.push(current_name.clone());
    }

    assert_eq!(
        visited,
        [
            "claude-opus-4-8 (high)",
            "claude-opus-4-8 (low)",
            "claude-opus-4-8 (med)",
            "claude-opus-5 (high)",
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
fn model_picker_effort_default_matches_only_stored_variant() {
    // Anthropic: stored effort selects exactly one variant.
    assert!(model_picker_effort_matches_default(
        Some("claude-oauth"),
        Some("high"),
        Some("high"),
        None,
    ));
    assert!(!model_picker_effort_matches_default(
        Some("claude-oauth"),
        Some("low"),
        Some("high"),
        None,
    ));
    // OpenAI uses its own stored effort.
    assert!(model_picker_effort_matches_default(
        Some("openai-oauth"),
        Some("medium"),
        None,
        Some("medium"),
    ));
    assert!(!model_picker_effort_matches_default(
        Some("openai-api"),
        Some("high"),
        None,
        Some("medium"),
    ));
    // No stored effort: every variant keeps the legacy default marker.
    assert!(model_picker_effort_matches_default(
        Some("claude-oauth"),
        Some("xhigh"),
        None,
        None,
    ));
    // Entries without an effort always match.
    assert!(model_picker_effort_matches_default(
        Some("claude-oauth"),
        None,
        Some("high"),
        None,
    ));
    // Unknown provider families ignore stored efforts.
    assert!(model_picker_effort_matches_default(
        Some("openrouter"),
        Some("high"),
        Some("low"),
        Some("low"),
    ));
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
        "gpt-5.5",
        &openai_oauth_route
    ));
    assert!(!model_picker_route_is_recommended(
        "gpt-5.5",
        &openai_api_key_route
    ));
    assert!(!model_picker_route_is_recommended(
        "gpt-5.5",
        &copilot_route
    ));
    assert!(!model_picker_route_is_recommended(
        "gpt-5.5",
        &unavailable_openai_oauth_route,
    ));

    // Current policy (see jcode-provider-core): claude-opus-4-8 is the
    // recommended Anthropic flagship; older Opus and OpenRouter/Copilot
    // routes are not recommended.
    assert!(model_picker_route_is_recommended(
        "claude-opus-4-8",
        &claude_oauth_route,
    ));
    assert!(!model_picker_route_is_recommended(
        "claude-opus-4-7",
        &claude_oauth_route,
    ));
    assert!(!model_picker_route_is_recommended(
        "claude-opus-4-8",
        &claude_openrouter_route,
    ));
    assert!(!model_picker_route_is_recommended(
        "claude-opus-4-8",
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
