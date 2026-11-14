use super::*;

#[test]
fn test_provider_for_model_unknown() {
    assert_eq!(provider_for_model("unknown-model"), None);
}

#[test]
fn test_provider_for_model_cursor() {
    assert_eq!(provider_for_model("composer-2-fast"), Some("cursor"));
    assert_eq!(provider_for_model("composer-2"), Some("cursor"));
    assert_eq!(provider_for_model("sonnet-4.6"), Some("cursor"));
    assert_eq!(provider_for_model("gpt-5"), Some("openai"));
}

#[test]
fn test_context_limit_spark_vs_codex() {
    assert_eq!(
        context_limit_for_model("gpt-5.3-codex-spark"),
        Some(128_000)
    );
    assert_eq!(context_limit_for_model("gpt-5.5"), Some(272_000));
    assert_eq!(context_limit_for_model("gpt-5.3-codex"), Some(272_000));
    assert_eq!(context_limit_for_model("gpt-5.2-codex"), Some(272_000));
    assert_eq!(context_limit_for_model("gpt-5-codex"), Some(272_000));
}

#[test]
fn test_context_limit_gpt_5_4() {
    assert_eq!(context_limit_for_model("gpt-5.4"), Some(1_000_000));
    assert_eq!(context_limit_for_model("gpt-5.4-pro"), Some(1_000_000));
    assert_eq!(context_limit_for_model("gpt-5.4[1m]"), Some(1_000_000));
}

#[test]
fn test_context_limit_respects_provider_hint() {
    assert_eq!(
        context_limit_for_model_with_provider("gpt-5.4", Some("openai")),
        Some(1_000_000)
    );
    assert_eq!(
        context_limit_for_model_with_provider("gpt-5.4", Some("copilot")),
        Some(128_000)
    );
    assert_eq!(
        context_limit_for_model_with_provider("claude-sonnet-4-6[1m]", Some("claude")),
        Some(1_048_576)
    );
}

#[test]
fn test_resolve_model_capabilities_uses_provider_hint() {
    let openai = resolve_model_capabilities("gpt-5.4", Some("openai"));
    assert_eq!(openai.provider.as_deref(), Some("openai"));
    assert_eq!(openai.context_window, Some(1_000_000));

    let copilot = resolve_model_capabilities("gpt-5.4", Some("copilot"));
    assert_eq!(copilot.provider.as_deref(), Some("copilot"));
    assert_eq!(copilot.context_window, Some(128_000));

    let gemini = resolve_model_capabilities("gemini-2.5-pro", Some("gemini"));
    assert_eq!(gemini.provider.as_deref(), Some("gemini"));
    assert_eq!(gemini.context_window, Some(1_000_000));
}

#[test]
fn test_normalize_model_id_strips_1m_suffix() {
    assert_eq!(models::normalize_model_id("gpt-5.4[1m]"), "gpt-5.4");
    assert_eq!(models::normalize_model_id(" GPT-5.4[1M] "), "gpt-5.4");
}

#[test]
fn test_merge_openai_model_ids_appends_dynamic_oauth_models() {
    let models = models::merge_openai_model_ids(vec![
        "gpt-5.4".to_string(),
        "gpt-5.4-fast-preview".to_string(),
        "gpt-5.4-fast-preview".to_string(),
        " gpt-5.5-experimental ".to_string(),
    ]);

    assert!(models.iter().any(|model| model == "gpt-5.4"));
    assert!(models.iter().any(|model| model == "gpt-5.4-fast-preview"));
    assert!(models.iter().any(|model| model == "gpt-5.5-experimental"));
    assert_eq!(
        models
            .iter()
            .filter(|model| model.as_str() == "gpt-5.4-fast-preview")
            .count(),
        1
    );
}

#[test]
fn test_merge_anthropic_model_ids_appends_dynamic_models() {
    let models = models::merge_anthropic_model_ids(vec![
        "claude-opus-4-6".to_string(),
        "claude-sonnet-5-preview".to_string(),
        "claude-sonnet-5-preview".to_string(),
        " claude-haiku-5-beta ".to_string(),
    ]);

    assert!(models.iter().any(|model| model == "claude-opus-4-6"));
    assert!(models.iter().any(|model| model == "claude-opus-4-6[1m]"));
    assert!(
        models
            .iter()
            .any(|model| model == "claude-sonnet-5-preview")
    );
    assert!(models.iter().any(|model| model == "claude-haiku-5-beta"));
    assert_eq!(
        models
            .iter()
            .filter(|model| model.as_str() == "claude-sonnet-5-preview")
            .count(),
        1
    );
}

#[test]
fn test_parse_anthropic_model_catalog_reads_context_limits() {
    let data = serde_json::json!({
        "data": [
            {
                "id": "claude-opus-4-6",
                "max_input_tokens": 1_048_576
            },
            {
                "id": "claude-sonnet-5-preview",
                "max_input_tokens": 333_000
            }
        ]
    });

    let catalog = models::parse_anthropic_model_catalog(&data);
    assert!(
        catalog
            .available_models
            .contains(&"claude-opus-4-6".to_string())
    );
    assert!(
        catalog
            .available_models
            .contains(&"claude-sonnet-5-preview".to_string())
    );
    assert_eq!(
        catalog.context_limits.get("claude-opus-4-6"),
        Some(&1_048_576)
    );
    assert_eq!(
        catalog.context_limits.get("claude-sonnet-5-preview"),
        Some(&333_000)
    );
}

#[test]
fn test_context_limit_claude() {
    with_clean_provider_test_env(|| {
        assert_eq!(context_limit_for_model("claude-opus-4-6"), Some(200_000));
        assert_eq!(context_limit_for_model("claude-sonnet-4-6"), Some(200_000));
        assert_eq!(
            context_limit_for_model("claude-opus-4-6[1m]"),
            Some(1_048_576)
        );
        assert_eq!(
            context_limit_for_model("claude-sonnet-4-6[1m]"),
            Some(1_048_576)
        );
        // Opus 4.8 / 4.7 expose a 1M window natively (no `[1m]` opt-in needed),
        // matching the live Anthropic catalog's `max_input_tokens: 1000000`.
        assert_eq!(context_limit_for_model("claude-opus-4-8"), Some(1_000_000));
        assert_eq!(
            context_limit_for_model("claude-opus-4-8[1m]"),
            Some(1_000_000)
        );
        assert_eq!(context_limit_for_model("claude-opus-4-7"), Some(1_000_000));
    });
}

#[test]
fn test_context_limit_dynamic_cache() {
    populate_context_limits(
        [("test-model-xyz".to_string(), 64_000)]
            .into_iter()
            .collect(),
    );
    assert_eq!(context_limit_for_model("test-model-xyz"), Some(64_000));
}

// --- Migrated from the OpenRouter runtime tests: these exercise MultiProvider
// --- routing/identity with a real OpenRouter runtime via the registry.

fn strata_config(model: &str, context_window: Option<usize>) -> crate::config::Config {
    let mut cfg = crate::config::Config::default();
    cfg.providers.insert(
        "strata".into(),
        crate::config::NamedProviderConfig {
            models: vec![crate::config::NamedProviderModelConfig {
                id: model.into(),
                context_window,
                ..Default::default()
            }],
            ..Default::default()
        },
    );
    cfg
}

#[test]
fn strata_configured_context_survives_catalog_publication() {
    let model = "qwen3.8-flash-next-ud-q4_k_xl-abliterated";
    let _seeding = crate::provider::models::CONFIG_SEED_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    crate::provider::models::seed_context_limits_from_config(&strata_config(model, Some(196_608)));
    populate_context_limits(std::collections::HashMap::from([
        (model.into(), 262_144),
        (format!("strata:{model}"), 262_144),
    ]));
    for id in [model.to_string(), format!("strata:{model}")] {
        for hint in [None, Some("strata")] {
            assert_eq!(
                context_limit_for_model_with_provider(&id, hint),
                Some(196_608),
                "{id} via {hint:?}"
            );
        }
    }
}

#[test]
fn removed_context_window_override_stops_applying_after_reload() {
    let model = "qwen3.8-flash-issue2115-removed";
    let _seeding = crate::provider::models::CONFIG_SEED_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    crate::provider::models::seed_context_limits_from_config(&strata_config(model, Some(196_608)));
    assert_eq!(
        context_limit_for_model_with_provider(model, Some("strata")),
        Some(196_608)
    );
    // The override is removed from config, then the endpoint publishes its own limit.
    crate::provider::models::seed_context_limits_from_config(&strata_config(model, None));
    populate_context_limits(std::collections::HashMap::from([(
        format!("strata:{model}"),
        131_072,
    )]));
    assert_eq!(
        crate::provider::models::configured_context_limit_for_model(&format!("strata:{model}")),
        None
    );
    assert_eq!(
        context_limit_for_model_with_provider(model, Some("strata")),
        Some(131_072)
    );
}

#[test]
fn named_provider_hint_finds_scoped_live_context_before_family() {
    let model = "qwen3.8-flash-issue2115-live";
    populate_context_limits(std::collections::HashMap::from([
        (format!("strata:{model}"), 196_608),
        (format!("other:{model}"), 131_072),
    ]));
    assert_eq!(
        context_limit_for_model_with_provider(model, Some("strata")),
        Some(196_608)
    );
    assert_eq!(
        context_limit_for_model_with_provider(model, Some("other")),
        Some(131_072)
    );
}
