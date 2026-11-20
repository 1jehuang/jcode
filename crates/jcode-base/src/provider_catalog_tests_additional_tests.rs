use super::*;

#[test]
fn conifer_static_fallback_preserves_catalog_except_unverified_together_alias() {
    let profile = openai_compatible_profile_by_id("conifer").expect("Conifer profile");
    let models = openai_compatible_profile_static_models(profile);

    assert_eq!(models.len(), 95);
    assert_eq!(models.first().map(String::as_str), Some("claude-fable-5"));
    assert_eq!(models.last().map(String::as_str), Some("gemma-3-27b"));
    assert!(models.iter().any(|model| model == "gpt-5.6-sol"));
    assert!(models.iter().any(|model| model == "nemotron-3-ultra"));
    assert!(
        !models
            .iter()
            .any(|model| model == "nemotron-3-ultra-together")
    );
    assert_eq!(
        openai_compatible_profile_context_limit("conifer", "nemotron-3-ultra-together"),
        None,
        "an undocumented Together route must not inherit DeepInfra's window"
    );
}

#[test]
fn conifer_context_limits_match_public_catalog_snapshot() {
    // Exact observations from /v1/catalog, 2026-09-16. Source/hash and refresh
    // policy are documented in docs/CONIFER_PROVIDER.md (issue #1274).
    let expected = [
        ("grok-4.6", 500_000),
        ("grok-4.5", 500_000),
        ("grok-4.3", 1_000_000),
        ("seed-2.0-pro", 256_000),
        ("seed-2.0-code", 256_000),
        ("seed-2.0-mini", 256_000),
        ("step-3.7-flash", 262_144),
        ("step-3.7-flash-novita", 262_144),
        ("hy3", 262_144),
        ("hy3-tencent", 262_144),
        ("hy3-novita", 262_144),
        ("ling-3.0-flash", 131_072),
        ("inkling", 524_288),
        ("inkling-small", 524_288),
        ("nemotron-3-ultra", 262_144),
        ("nemotron-3-super-120b", 262_144),
        ("nemotron-3.5-lightning", 262_144),
        ("mistral-large-latest", 256_000),
        ("mistral-medium-latest", 256_000),
        ("mistral-small-latest", 256_000),
        ("command-a-cohere", 256_000),
        ("llama-4-maverick", 1_048_576),
        ("llama-4-scout", 327_680),
        ("gemma-4-31b", 128_000),
    ];
    let limits = openai_compatible_profile_static_context_limits(CONIFER_PROFILE);
    for (model, limit) in expected {
        assert_eq!(limits.get(model), Some(&limit), "{model}");
        assert_eq!(
            openai_compatible_profile_context_limit(" CONIFER ", &model.to_uppercase()),
            Some(limit),
            "{model}"
        );
        assert_eq!(
            openai_compatible_profile_context_limit("openrouter", model),
            jcode_provider_core::models::open_weight_family_context_limit(model),
            "Conifer metadata must not leak into other profiles: {model}"
        );
    }
    assert_eq!(
        openai_compatible_profile_context_limit("conifer", "grok-4.6-unknown"),
        None
    );
    assert_eq!(
        openai_compatible_profile_context_limit("conifer", "kimi-k3"),
        jcode_provider_core::models::open_weight_family_context_limit("kimi-k3"),
        "existing shared family fallback must remain intact"
    );
}
