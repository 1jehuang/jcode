use super::*;

/// Regression: when a model has `display_name` configured, the picker shows the
/// alias but `set_model` must convert it back to the raw upstream id so the API
/// request body uses the identifier the endpoint expects.
#[test]
fn named_provider_set_model_resolves_display_name_to_raw_id() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let mut profile = make_per_model_test_profile();
    profile.model_catalog = false;
    let provider =
        OpenRouterProvider::new_named_openai_compatible("custom", &profile).expect("provider");

    // Simulate the picker sending the display-name alias through set_model.
    provider.set_model("Friendly Name").expect("set_model");

    // The internal model field must hold the raw id, not the display alias.
    assert_eq!(
        provider.model(),
        "raw-model-id",
        "set_model should resolve display name back to raw model id"
    );
}

/// Regression: `reasoning = false` on a DeepSeek-family model should
/// force-disable the effort ladder, not fall through to auto-detection.
#[test]
fn named_provider_per_model_reasoning_false_force_disables_deepseek() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let mut profile = jcode_base::config::NamedProviderConfig {
        base_url: "http://localhost:8080/v1".to_string(),
        auth: jcode_base::config::NamedProviderAuth::None,
        default_model: Some("deepseek-chat".to_string()),
        models: vec![jcode_base::config::NamedProviderModelConfig {
            id: "deepseek-chat".to_string(),
            reasoning: Some(false),
            ..Default::default()
        }],
        ..Default::default()
    };
    profile.model_catalog = false;
    let provider =
        OpenRouterProvider::new_named_openai_compatible("custom", &profile).expect("provider");

    assert!(
        !provider.supports_deepseek_reasoning_effort(),
        "reasoning=false should force-disable DeepSeek reasoning even on a DeepSeek model"
    );
    assert!(
        !provider.supports_any_reasoning_effort(),
        "reasoning=false should make supports_any_reasoning_effort false"
    );
    assert!(
        provider.available_efforts().is_empty(),
        "reasoning=false on DeepSeek model should produce empty effort ladder; got: {:?}",
        provider.available_efforts()
    );
}

/// Regression: `reasoning = false` on a GPT-family reasoning model should
/// force-disable the OpenAI reasoning effort ladder.
#[test]
fn named_provider_per_model_reasoning_false_force_disables_openai() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let mut profile = jcode_base::config::NamedProviderConfig {
        base_url: "http://localhost:8080/v1".to_string(),
        auth: jcode_base::config::NamedProviderAuth::None,
        default_model: Some("gpt-5.3-codex".to_string()),
        models: vec![jcode_base::config::NamedProviderModelConfig {
            id: "gpt-5.3-codex".to_string(),
            reasoning: Some(false),
            ..Default::default()
        }],
        ..Default::default()
    };
    profile.model_catalog = false;
    let provider =
        OpenRouterProvider::new_named_openai_compatible("custom", &profile).expect("provider");

    assert!(
        !provider.supports_openai_reasoning_effort(),
        "reasoning=false should force-disable OpenAI reasoning even on a GPT-family model"
    );
    assert!(
        !provider.supports_any_reasoning_effort(),
        "reasoning=false should make supports_any_reasoning_effort false"
    );
}

/// Regression for the core bug: a GPT-family model with `reasoning: true` on a
/// generic OpenAI-compatible profile must use the OpenAI effort ladder, not the
/// DeepSeek ladder. Before the fix, `supports_deepseek_reasoning_effort`
/// returned `true` first (because the per-model override was checked there
/// before `supports_openai_reasoning_effort`), so the picker showed the
/// DeepSeek ladder (no `minimal`/`xhigh`) and the request body used the
/// DeepSeek `reasoning_effort` wire format.
#[test]
fn named_provider_per_model_reasoning_true_on_gpt_model_uses_openai_ladder() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let mut profile = jcode_base::config::NamedProviderConfig {
        base_url: "http://localhost:8080/v1".to_string(),
        auth: jcode_base::config::NamedProviderAuth::None,
        default_model: Some("gpt-5.3-codex".to_string()),
        models: vec![jcode_base::config::NamedProviderModelConfig {
            id: "gpt-5.3-codex".to_string(),
            reasoning: Some(true),
            ..Default::default()
        }],
        ..Default::default()
    };
    profile.model_catalog = false;
    let provider =
        OpenRouterProvider::new_named_openai_compatible("custom", &profile).expect("provider");

    let efforts = provider.available_efforts();
    assert!(
        efforts.contains(&"minimal"),
        "GPT-family model with reasoning=true must use OpenAI ladder (has minimal); got: {efforts:?}"
    );
    assert!(
        efforts.contains(&"xhigh"),
        "GPT-family model with reasoning=true must use OpenAI ladder (has xhigh); got: {efforts:?}"
    );
    assert!(
        provider.supports_openai_reasoning_effort(),
        "GPT-family model with reasoning=true should support OpenAI reasoning effort"
    );
    assert!(
        !provider.supports_deepseek_reasoning_effort(),
        "GPT-family model with reasoning=true must NOT use DeepSeek reasoning effort"
    );
}

/// A DeepSeek-family model with `reasoning: true` must get the DeepSeek ladder
/// (which has `max` but not `minimal`/`xhigh`), not the OpenAI ladder.
#[test]
fn named_provider_per_model_reasoning_true_on_deepseek_model_uses_deepseek_ladder() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let mut profile = jcode_base::config::NamedProviderConfig {
        base_url: "http://localhost:8080/v1".to_string(),
        auth: jcode_base::config::NamedProviderAuth::None,
        default_model: Some("deepseek-chat".to_string()),
        models: vec![jcode_base::config::NamedProviderModelConfig {
            id: "deepseek-chat".to_string(),
            reasoning: Some(true),
            ..Default::default()
        }],
        ..Default::default()
    };
    profile.model_catalog = false;
    let provider =
        OpenRouterProvider::new_named_openai_compatible("custom", &profile).expect("provider");

    let efforts = provider.available_efforts();
    assert!(
        efforts.contains(&"max"),
        "DeepSeek model with reasoning=true must use DeepSeek ladder (has max); got: {efforts:?}"
    );
    assert!(
        !efforts.contains(&"minimal"),
        "DeepSeek ladder should not have minimal; got: {efforts:?}"
    );
    assert!(
        !efforts.contains(&"xhigh"),
        "DeepSeek ladder should not have xhigh; got: {efforts:?}"
    );
    assert!(
        provider.supports_deepseek_reasoning_effort(),
        "DeepSeek model with reasoning=true should support DeepSeek reasoning effort"
    );
    assert!(
        !provider.supports_openai_reasoning_effort(),
        "DeepSeek model with reasoning=true must NOT use OpenAI reasoning effort"
    );
}

/// `reasoning: true` on a generic (non-DeepSeek, non-GPT) model must enable the
/// OpenAI `reasoning_effort` ladder. This is the common custom-provider case:
/// the user points at an OpenAI-compatible gateway with a model id that does
/// not match auto-detection and explicitly declares `reasoning: true`.
#[test]
fn named_provider_per_model_reasoning_true_sends_effort_in_request_body() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let (api_base, request_rx) = spawn_single_response_chat_server();
    let mut profile = jcode_base::config::NamedProviderConfig {
        base_url: api_base,
        auth: jcode_base::config::NamedProviderAuth::None,
        default_model: Some("custom-gateway-model".to_string()),
        models: vec![jcode_base::config::NamedProviderModelConfig {
            id: "custom-gateway-model".to_string(),
            reasoning: Some(true),
            ..Default::default()
        }],
        ..Default::default()
    };
    profile.model_catalog = false;
    let provider =
        OpenRouterProvider::new_named_openai_compatible("custom", &profile).expect("provider");
    provider
        .set_reasoning_effort("high")
        .expect("reasoning=true should make high effort accepted");

    let messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "hello".to_string(),
            cache_control: None,
        }]
        .into(),
        timestamp: None,
        tool_duration_ms: None,
    }];
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(async {
        let stream = provider
            .complete(&messages, &[], "", None)
            .await
            .expect("stream starts");
        futures::pin_mut!(stream);
        while let Some(event) = stream.next().await {
            if event.is_err() {
                break;
            }
        }
    });

    let request = request_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("capture request");
    assert!(
        request.contains(r#""reasoning_effort":"high""#),
        "reasoning=true should cause reasoning_effort=high in request body; got: {request}"
    );
}

/// `set_reasoning_effort` must accept the `minimal` rung (OpenAI-only) for a
/// non-DeepSeek model with `reasoning: true`. Before the fix, the DeepSeek
/// ladder (which lacks `minimal`) was used, so `set_reasoning_effort("minimal")`
/// would bail.
#[test]
fn named_provider_per_model_reasoning_true_accepts_minimal_effort() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let mut profile = jcode_base::config::NamedProviderConfig {
        base_url: "http://localhost:8080/v1".to_string(),
        auth: jcode_base::config::NamedProviderAuth::None,
        default_model: Some("custom-gateway-model".to_string()),
        models: vec![jcode_base::config::NamedProviderModelConfig {
            id: "custom-gateway-model".to_string(),
            reasoning: Some(true),
            ..Default::default()
        }],
        ..Default::default()
    };
    profile.model_catalog = false;
    let provider =
        OpenRouterProvider::new_named_openai_compatible("custom", &profile).expect("provider");
    provider
        .set_reasoning_effort("minimal")
        .expect("minimal should be accepted on the OpenAI ladder for a non-DeepSeek model with reasoning=true");
    assert_eq!(
        provider.reasoning_effort().as_deref(),
        Some("minimal"),
        "reasoning_effort getter should return the accepted minimal effort"
    );
}

/// Switching the active model via `set_model` must update the per-model effort
/// detection. A profile with a reasoning model and a plain model: effort is
/// available on the reasoning model, then disappears after switching to the
/// plain model.
#[test]
fn named_provider_set_model_updates_per_model_effort_detection() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let mut profile = make_per_model_test_profile();
    profile.model_catalog = false;
    let provider =
        OpenRouterProvider::new_named_openai_compatible("custom", &profile).expect("provider");

    // raw-model-id has reasoning=true -> effort ladder available.
    assert!(
        provider.supports_any_reasoning_effort(),
        "raw-model-id should have effort support via reasoning=true"
    );

    // Switch to plain-model which has no reasoning override.
    provider.set_model("plain-model").expect("set_model");
    assert!(
        !provider.supports_any_reasoning_effort(),
        "plain-model has no reasoning override and is not auto-detected; effort should be gone"
    );
    assert!(
        provider.available_efforts().is_empty(),
        "plain-model should have empty effort ladder after set_model"
    );

    // Switch back to the reasoning model -> effort returns.
    provider
        .set_model("raw-model-id")
        .expect("set_model back to reasoning model");
    assert!(
        provider.supports_any_reasoning_effort(),
        "switching back to reasoning model should re-enable effort"
    );
}

/// A stored reasoning effort must be cleared when switching from a model that
/// supports reasoning to one that does not. Otherwise the stale effort leaks
/// into the next request as an unsupported value.
#[test]
fn named_provider_switching_to_non_reasoning_model_clears_stored_effort() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let mut profile = make_per_model_test_profile();
    profile.model_catalog = false;
    let provider =
        OpenRouterProvider::new_named_openai_compatible("custom", &profile).expect("provider");

    provider
        .set_reasoning_effort("high")
        .expect("set high on reasoning model");
    assert_eq!(
        provider.reasoning_effort().as_deref(),
        Some("high"),
        "effort should be stored before switching"
    );

    provider.set_model("plain-model").expect("set_model");
    assert!(
        provider.reasoning_effort().is_none(),
        "stored effort should be cleared after switching to a non-reasoning model"
    );
}

/// `set_reasoning_effort` must reject an effort rung that is valid on the OpenAI
/// ladder but not on the DeepSeek ladder when the active model is DeepSeek with
/// `reasoning: true`. This confirms the DeepSeek ladder is actually selected.
#[test]
fn named_provider_deepseek_with_reasoning_true_rejects_minimal_effort() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let mut profile = jcode_base::config::NamedProviderConfig {
        base_url: "http://localhost:8080/v1".to_string(),
        auth: jcode_base::config::NamedProviderAuth::None,
        default_model: Some("deepseek-chat".to_string()),
        models: vec![jcode_base::config::NamedProviderModelConfig {
            id: "deepseek-chat".to_string(),
            reasoning: Some(true),
            ..Default::default()
        }],
        ..Default::default()
    };
    profile.model_catalog = false;
    let provider =
        OpenRouterProvider::new_named_openai_compatible("custom", &profile).expect("provider");

    let result = provider.set_reasoning_effort("minimal");
    assert!(
        result.is_err(),
        "minimal is not on the DeepSeek ladder; set_reasoning_effort should reject it; got: {result:?}"
    );
}

/// The per-model `reasoning: true` override must cause the configured
/// `openai_reasoning_effort` to be loaded at construction time even when the
/// provider-level `supports_reasoning_effort` is unset. Without the fix,
/// `initial_reasoning_effort` only checked the provider-level flag and ignored
/// the per-model override, so the user's configured default effort was lost.
#[test]
fn named_provider_per_model_reasoning_loads_initial_effort_from_config() {
    let _lock = ENV_LOCK.lock();
    let _effort = EnvVarGuard::set("JCODE_OPENAI_REASONING_EFFORT", "high");
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    jcode_base::config::invalidate_config_cache();

    let mut profile = jcode_base::config::NamedProviderConfig {
        base_url: "http://localhost:8080/v1".to_string(),
        auth: jcode_base::config::NamedProviderAuth::None,
        default_model: Some("custom-model".to_string()),
        models: vec![jcode_base::config::NamedProviderModelConfig {
            id: "custom-model".to_string(),
            reasoning: Some(true),
            ..Default::default()
        }],
        ..Default::default()
    };
    profile.model_catalog = false;
    let provider =
        OpenRouterProvider::new_named_openai_compatible("custom", &profile).expect("provider");

    assert_eq!(
        provider.reasoning_effort().as_deref(),
        Some("high"),
        "per-model reasoning=true should load configured openai_reasoning_effort at startup"
    );

    jcode_base::config::invalidate_config_cache();
}

/// When the per-model override enables reasoning on a non-DeepSeek model, the
/// initial effort must be normalized with the OpenAI ladder so OpenAI-only
/// rungs like `minimal` survive. Before the fix, `initial_reasoning_effort`
/// always used the DeepSeek normalizer, which rejects `minimal`.
#[test]
fn named_provider_per_model_reasoning_loads_minimal_initial_effort() {
    let _lock = ENV_LOCK.lock();
    let _effort = EnvVarGuard::set("JCODE_OPENAI_REASONING_EFFORT", "minimal");
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    jcode_base::config::invalidate_config_cache();

    let mut profile = jcode_base::config::NamedProviderConfig {
        base_url: "http://localhost:8080/v1".to_string(),
        auth: jcode_base::config::NamedProviderAuth::None,
        default_model: Some("custom-model".to_string()),
        models: vec![jcode_base::config::NamedProviderModelConfig {
            id: "custom-model".to_string(),
            reasoning: Some(true),
            ..Default::default()
        }],
        ..Default::default()
    };
    profile.model_catalog = false;
    let provider =
        OpenRouterProvider::new_named_openai_compatible("custom", &profile).expect("provider");

    assert_eq!(
        provider.reasoning_effort().as_deref(),
        Some("minimal"),
        "minimal must survive normalization with the OpenAI ladder for a non-DeepSeek model with reasoning=true"
    );

    jcode_base::config::invalidate_config_cache();
}

/// When the per-model override enables reasoning on a DeepSeek model, the
/// initial effort must be normalized with the DeepSeek ladder so DeepSeek-only
/// rungs like `max` survive and OpenAI-only rungs like `minimal` are rejected.
#[test]
fn named_provider_per_model_reasoning_deepseek_normalizes_with_deepseek_ladder() {
    let _lock = ENV_LOCK.lock();
    let _effort = EnvVarGuard::set("JCODE_OPENAI_REASONING_EFFORT", "max");
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    jcode_base::config::invalidate_config_cache();

    let mut profile = jcode_base::config::NamedProviderConfig {
        base_url: "http://localhost:8080/v1".to_string(),
        auth: jcode_base::config::NamedProviderAuth::None,
        default_model: Some("deepseek-chat".to_string()),
        models: vec![jcode_base::config::NamedProviderModelConfig {
            id: "deepseek-chat".to_string(),
            reasoning: Some(true),
            ..Default::default()
        }],
        ..Default::default()
    };
    profile.model_catalog = false;
    let provider =
        OpenRouterProvider::new_named_openai_compatible("custom", &profile).expect("provider");

    assert_eq!(
        provider.reasoning_effort().as_deref(),
        Some("max"),
        "max must survive normalization with the DeepSeek ladder for a DeepSeek model with reasoning=true"
    );

    jcode_base::config::invalidate_config_cache();
}

/// `reasoning: false` per-model must prevent loading the initial effort even
/// when `openai_reasoning_effort` is configured and the model would otherwise
/// auto-detect as a reasoning model (e.g. DeepSeek-family).
#[test]
fn named_provider_per_model_reasoning_false_blocks_initial_effort() {
    let _lock = ENV_LOCK.lock();
    let _effort = EnvVarGuard::set("JCODE_OPENAI_REASONING_EFFORT", "high");
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    jcode_base::config::invalidate_config_cache();

    let mut profile = jcode_base::config::NamedProviderConfig {
        base_url: "http://localhost:8080/v1".to_string(),
        auth: jcode_base::config::NamedProviderAuth::None,
        default_model: Some("deepseek-chat".to_string()),
        models: vec![jcode_base::config::NamedProviderModelConfig {
            id: "deepseek-chat".to_string(),
            reasoning: Some(false),
            ..Default::default()
        }],
        ..Default::default()
    };
    profile.model_catalog = false;
    let provider =
        OpenRouterProvider::new_named_openai_compatible("custom", &profile).expect("provider");

    assert!(
        provider.reasoning_effort().is_none(),
        "reasoning=false should block initial effort even with openai_reasoning_effort configured"
    );
    assert!(
        !provider.supports_any_reasoning_effort(),
        "reasoning=false should disable effort support entirely"
    );

    jcode_base::config::invalidate_config_cache();
}

/// Switching models via the display-name alias must update the per-model effort
/// detection, the same as switching by raw id. The picker sends display names
/// back through `set_model`, so `resolve_display_name_to_id` + `model_key`
/// update must work together to pick up the new model's reasoning override.
#[test]
fn named_provider_set_model_via_display_name_updates_effort_detection() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let mut profile = make_per_model_test_profile();
    profile.model_catalog = false;
    let provider =
        OpenRouterProvider::new_named_openai_compatible("custom", &profile).expect("provider");

    // raw-model-id (alias "Friendly Name") has reasoning=true.
    assert!(
        provider.supports_any_reasoning_effort(),
        "initial model should have effort support via reasoning=true"
    );

    // Switch to plain-model via its raw id (no display name configured).
    provider.set_model("plain-model").expect("set_model");
    assert!(
        !provider.supports_any_reasoning_effort(),
        "plain-model should have no effort after switching by raw id"
    );

    // Switch back via the display-name alias, not the raw id. This must still
    // update model_key and re-enable effort detection.
    provider
        .set_model("Friendly Name")
        .expect("set_model via display name");
    assert_eq!(
        provider.model(),
        "raw-model-id",
        "display name should resolve to raw id"
    );
    assert!(
        provider.supports_any_reasoning_effort(),
        "switching back via display name should re-enable effort detection"
    );
}

/// Switching from an OpenAI-ladder model to a DeepSeek-ladder model must clear
/// a stored OpenAI-only effort (e.g. `minimal`) since it is not on the DeepSeek
/// ladder. The stored-effort cleanup in `set_model` uses `available_efforts()`
/// after the model_key is updated, so the new ladder determines validity.
#[test]
fn named_provider_switching_to_deepseek_clears_openai_only_effort() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    // Pin the configured effort explicitly: the F4 fix re-resolves the
    // configured default when switching to a per-model `reasoning = true`
    // model, and the global config default must not leak into this test.
    let _effort = EnvVarGuard::set("JCODE_OPENAI_REASONING_EFFORT", "minimal");
    jcode_base::config::invalidate_config_cache();
    let mut profile = jcode_base::config::NamedProviderConfig {
        base_url: "http://localhost:8080/v1".to_string(),
        auth: jcode_base::config::NamedProviderAuth::None,
        default_model: Some("gpt-5.3-codex".to_string()),
        models: vec![
            jcode_base::config::NamedProviderModelConfig {
                id: "gpt-5.3-codex".to_string(),
                reasoning: Some(true),
                ..Default::default()
            },
            jcode_base::config::NamedProviderModelConfig {
                id: "deepseek-chat".to_string(),
                reasoning: Some(true),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    profile.model_catalog = false;
    let provider =
        OpenRouterProvider::new_named_openai_compatible("custom", &profile).expect("provider");

    // Set minimal, which is valid on the OpenAI ladder.
    provider
        .set_reasoning_effort("minimal")
        .expect("minimal is valid for GPT model with OpenAI ladder");
    assert_eq!(
        provider.reasoning_effort().as_deref(),
        Some("minimal"),
        "minimal should be stored"
    );

    // Switch to deepseek-chat which has the DeepSeek ladder (no minimal).
    provider.set_model("deepseek-chat").expect("set_model");

    // The stored minimal must be cleared because it is not on the DeepSeek ladder.
    assert!(
        provider.reasoning_effort().is_none(),
        "minimal should be cleared after switching to a DeepSeek model that lacks it"
    );
    assert!(
        provider.supports_deepseek_reasoning_effort(),
        "deepseek model should use DeepSeek reasoning effort"
    );
    assert!(
        !provider.available_efforts().contains(&"minimal"),
        "DeepSeek ladder should not have minimal"
    );
}

/// Regression test: Mistral accepts the top-level `reasoning_effort` field for
/// `mistral-small-*` models. The `strict_openai_schema_endpoint` check (which
/// blocks `reasoning_content` and `thinking`) must NOT block
/// `reasoning_effort`. A previous attempt wrongly suppressed it for all
/// Mistral models, breaking the feature. This test verifies that
/// `reasoning_effort` is present in the request body when a user sets
/// `reasoning: true` on a `mistral-small-latest` model on a Mistral-profiled
/// provider (issue #708).
#[test]
fn named_provider_mistral_small_with_reasoning_sends_reasoning_effort() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let (api_base, request_rx) = spawn_single_response_chat_server();
    let mut profile = jcode_base::config::NamedProviderConfig {
        base_url: api_base,
        auth: jcode_base::config::NamedProviderAuth::None,
        default_model: Some("mistral-small-latest".to_string()),
        models: vec![jcode_base::config::NamedProviderModelConfig {
            id: "mistral-small-latest".to_string(),
            reasoning: Some(true),
            ..Default::default()
        }],
        ..Default::default()
    };
    profile.model_catalog = false;
    // Profile name "mistral" triggers strict_openai_schema_endpoint, which
    // blocks reasoning_content and thinking but must NOT block reasoning_effort.
    let provider =
        OpenRouterProvider::new_named_openai_compatible("mistral", &profile).expect("provider");
    provider
        .set_reasoning_effort("high")
        .expect("reasoning=true should accept high effort");

    // Verify the effort ladder is available despite strict_openai_schema.
    assert!(
        provider.supports_openai_reasoning_effort(),
        "per-model reasoning=true should enable the OpenAI effort ladder for mistral-small"
    );
    assert!(
        provider.reasoning_effort().is_some(),
        "effort should be stored internally"
    );

    let messages = vec![Message {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: "hello".to_string(),
            cache_control: None,
        }]
        .into(),
        timestamp: None,
        tool_duration_ms: None,
    }];
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    rt.block_on(async {
        let stream = provider
            .complete(&messages, &[], "", None)
            .await
            .expect("stream starts");
        futures::pin_mut!(stream);
        while let Some(event) = stream.next().await {
            if event.is_err() {
                break;
            }
        }
    });

    let request = request_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("capture request");
    assert!(
        request.contains(r#""reasoning_effort":"high""#),
        "Mistral with reasoning=true must receive reasoning_effort=high in the request body; got: {request}"
    );
    assert!(
        !request.contains(r#""thinking""#),
        "Mistral strict-schema endpoint must NOT receive the thinking field; got: {request}"
    );
}

// --- Per-model reasoning_effort map (opencode-style per-level mapping) ---

/// Build the opencode-style map from the user's issue #571 example: xhigh/high/
/// none map to wire values, medium/low are disabled.
fn opencode_style_reasoning_map() -> jcode_base::config::ReasoningEffortMapConfig {
    let rung =
        |effort: Option<&str>, disabled: bool| jcode_base::config::ReasoningEffortRungConfig {
            reasoning_effort: effort.map(ToString::to_string),
            disabled,
        };
    jcode_base::config::ReasoningEffortMapConfig {
        none: Some(rung(Some("none"), false)),
        high: Some(rung(Some("high"), false)),
        xhigh: Some(rung(Some("xhigh"), false)),
        medium: Some(rung(None, true)),
        low: Some(rung(None, true)),
        ..Default::default()
    }
}

fn make_mapped_compatible_provider(
    map: jcode_base::config::ReasoningEffortMapConfig,
) -> OpenRouterProvider {
    let mut provider = make_custom_compatible_provider();
    provider
        .static_model_reasoning_maps
        .insert(DEFAULT_MODEL.to_ascii_lowercase(), map);
    provider
}

#[test]
fn reasoning_map_defines_the_selectable_ladder() {
    let provider = make_mapped_compatible_provider(opencode_style_reasoning_map());
    assert_eq!(
        provider.available_efforts(),
        vec!["none", "high", "xhigh", "swarm"]
    );
    assert!(provider.supports_any_reasoning_effort());
}

#[test]
fn reasoning_map_rejects_disabled_and_unknown_efforts() {
    let provider = make_mapped_compatible_provider(opencode_style_reasoning_map());
    assert!(provider.set_reasoning_effort("medium").is_err());
    assert!(provider.set_reasoning_effort("low").is_err());
    assert!(provider.set_reasoning_effort("minimal").is_err());
    assert!(provider.set_reasoning_effort("max").is_err());
    assert!(provider.set_reasoning_effort("turbo").is_err());
    provider.set_reasoning_effort("xhigh").unwrap();
    assert_eq!(provider.reasoning_effort().as_deref(), Some("xhigh"));
}

#[test]
fn reasoning_map_enables_effort_without_provider_flag_or_family_detection() {
    // A generic model id that matches neither the DeepSeek nor GPT family
    // detection, and the provider-level flag is unset: the map alone must
    // enable the effort feature.
    let provider = make_mapped_compatible_provider(opencode_style_reasoning_map());
    assert!(!crate::OpenRouterProvider::model_key_is_deepseek_family(
        &provider.model_key_snapshot()
    ));
    provider.set_reasoning_effort("high").unwrap();
    assert_eq!(provider.reasoning_effort().as_deref(), Some("high"));
}

#[test]
fn reasoning_map_sends_mapped_wire_value_in_request_body() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let (api_base, request_rx) = spawn_single_response_chat_server();
    let mut provider = make_mapped_compatible_provider(opencode_style_reasoning_map());
    provider.api_base = api_base;
    provider.supports_model_catalog = false;
    provider.set_reasoning_effort("xhigh").unwrap();

    let messages = vec![Message::user("hi")];
    rt.block_on(async {
        let mut stream = provider
            .complete(&messages, &[], "", None)
            .await
            .expect("complete");
        while let Some(event) = stream.next().await {
            event.expect("fixture stream should succeed");
        }
    });
    let request = request_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let body = parse_captured_request_body(&request);
    assert_eq!(body["reasoning_effort"], "xhigh");
}

#[test]
fn reasoning_map_sends_custom_wire_value_in_request_body() {
    // The wire value can differ from the rung name (opencode parity: e.g.
    // `xhigh = { reasoningEffort = "very_high" }`).
    let map = jcode_base::config::ReasoningEffortMapConfig {
        high: Some(jcode_base::config::ReasoningEffortRungConfig {
            reasoning_effort: Some("very_high".to_string()),
            disabled: false,
        }),
        ..Default::default()
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let (api_base, request_rx) = spawn_single_response_chat_server();
    let mut provider = make_mapped_compatible_provider(map);
    provider.api_base = api_base;
    provider.supports_model_catalog = false;
    provider.set_reasoning_effort("high").unwrap();

    let messages = vec![Message::user("hi")];
    rt.block_on(async {
        let mut stream = provider
            .complete(&messages, &[], "", None)
            .await
            .expect("complete");
        while let Some(event) = stream.next().await {
            event.expect("fixture stream should succeed");
        }
    });
    let request = request_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let body = parse_captured_request_body(&request);
    assert_eq!(body["reasoning_effort"], "very_high");
}

#[test]
fn reasoning_map_dynamic_switch_changes_wire_value_per_request() {
    // Dynamic mid-session switching (/effort or keybinding): each request must
    // reflect the currently selected mapped rung.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let map = opencode_style_reasoning_map();
    let messages = vec![Message::user("hi")];

    for rung in ["high", "xhigh", "none"] {
        let (api_base, request_rx) = spawn_single_response_chat_server();
        let mut provider = make_mapped_compatible_provider(map.clone());
        provider.api_base = api_base;
        provider.supports_model_catalog = false;
        provider.set_reasoning_effort(rung).unwrap();
        assert_eq!(provider.reasoning_effort().as_deref(), Some(rung));

        rt.block_on(async {
            let mut stream = provider
                .complete(&messages, &[], "", None)
                .await
                .expect("complete");
            while let Some(event) = stream.next().await {
                event.expect("fixture stream should succeed");
            }
        });
        let request = request_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let body = parse_captured_request_body(&request);
        assert_eq!(body["reasoning_effort"], rung, "rung {rung}: {body}");
    }
}

#[test]
fn reasoning_map_clones_through_fork() {
    let provider = make_mapped_compatible_provider(opencode_style_reasoning_map());
    let forked = provider.fork();
    assert_eq!(
        forked.available_efforts(),
        vec!["none", "high", "xhigh", "swarm"]
    );
}

#[test]
fn selecting_swarm_effort_keeps_the_sentinel_stored() {
    // The ladder advertises the swarm sentinel, so selecting it must store it
    // as-is. Letting it fall through to the OpenAI family normalizer would
    // silently clear the stored effort. The retired `swarm-deep` mode is
    // neither advertised nor accepted.
    let provider = make_mapped_compatible_provider(opencode_style_reasoning_map());
    provider.set_reasoning_effort("high").unwrap();
    provider.set_reasoning_effort("swarm").unwrap();
    assert_eq!(provider.reasoning_effort().as_deref(), Some("swarm"));
    assert!(provider.set_reasoning_effort("swarm-deep").is_err());

    let mut bare = make_custom_compatible_provider();
    bare.reasoning_effort_support = Some(true);
    bare.set_reasoning_effort("swarm").unwrap();
    assert_eq!(bare.reasoning_effort().as_deref(), Some("swarm"));
}

#[test]
fn swarm_root_clamps_to_the_highest_enabled_map_rung() {
    // The map enables none+high+xhigh (high maps to "very_high" on the
    // wire). The configured swarm root defaults to `low`, which the endpoint
    // never accepts; the request must clamp to the highest enabled rung
    // instead of sending the unlisted default (greptile F4).
    let provider = make_mapped_compatible_provider(opencode_style_reasoning_map());
    provider.set_reasoning_effort("swarm").unwrap();
    assert_eq!(
        provider.resolve_swarm_root_effort_for_request("swarm"),
        "xhigh",
        "the default low root is not an enabled map rung, so clamp to the highest rung"
    );
    // The full Responses resolution then translates the clamped rung through
    // the map (xhigh has no explicit wire value: identity).
    assert_eq!(
        provider.resolve_request_reasoning_effort("swarm"),
        Some("xhigh".to_string())
    );
    // The retired deep sentinel is not a swarm mode anymore: it must resolve
    // to itself (rejected by set_reasoning_effort) rather than a root rung.
    assert_eq!(
        provider.resolve_swarm_root_effort_for_request("swarm-deep"),
        "swarm-deep"
    );
}

#[test]
fn swarm_root_passes_through_when_the_map_enables_it() {
    // When the configured root IS an enabled rung, it must survive
    // unchanged; clamping only applies to unlisted roots.
    let mut provider = make_mapped_compatible_provider(opencode_style_reasoning_map());
    provider
        .static_model_reasoning_maps
        .insert(DEFAULT_MODEL.to_ascii_lowercase(), {
            let mut map = opencode_style_reasoning_map();
            map.low = Some(jcode_base::config::ReasoningEffortRungConfig {
                reasoning_effort: Some("low".to_string()),
                disabled: false,
            });
            map
        });
    assert_eq!(
        provider.resolve_swarm_root_effort_for_request("swarm"),
        "low"
    );
}

#[test]
fn responses_resolution_translates_mapped_rungs_and_suppresses_none() {
    // Ordinary mapped rungs translate to their wire values; the `none` rung
    // maps to the wire value "none", which build_request then drops since
    // the Responses API treats effort "none" as no reasoning field.
    let provider = make_mapped_compatible_provider(opencode_style_reasoning_map());
    assert_eq!(
        provider.resolve_request_reasoning_effort("high"),
        Some("high".to_string())
    );
    // xhigh has no explicit wire value: identity mapping.
    assert_eq!(
        provider.resolve_request_reasoning_effort("xhigh"),
        Some("xhigh".to_string())
    );
    // `none` resolves to its mapped wire value "none"; build_request
    // filters that value so the request omits the reasoning field.
    assert_eq!(
        provider.resolve_request_reasoning_effort("none"),
        Some("none".to_string())
    );
    let mut request = super::opencode_go_responses::build_request(
        &[],
        &[],
        "",
        "gpt-5.6-luna",
        provider.resolve_request_reasoning_effort("none").as_deref(),
        None,
        |value| value.clone(),
    );
    assert!(request.get("reasoning").is_none());
    request = super::opencode_go_responses::build_request(
        &[],
        &[],
        "",
        "gpt-5.6-luna",
        provider.resolve_request_reasoning_effort("high").as_deref(),
        None,
        |value| value.clone(),
    );
    assert_eq!(request["reasoning"]["effort"].as_str(), Some("high"));
}

#[test]
fn no_map_ladders_accept_the_advertised_swarm_sentinel() {
    // The client ladder (configured_reasoning_efforts_in) appends the swarm
    // sentinel for reasoning=true models; the runtime ladder must accept it or
    // selecting the advertised rung bails. The retired deep sentinel must be
    // absent from every ladder and rejected by the setter.
    let mut provider = make_custom_compatible_provider();
    provider.reasoning_effort_support = Some(true);
    provider.set_model("gpt-5.5").unwrap();
    let ladder = provider.available_efforts();
    assert!(ladder.contains(&"swarm"), "ladder: {ladder:?}");
    assert!(!ladder.contains(&"swarm-deep"), "ladder: {ladder:?}");
    provider.set_reasoning_effort("swarm").unwrap();
    assert_eq!(provider.reasoning_effort().as_deref(), Some("swarm"));

    let deepseek = make_custom_compatible_provider();
    deepseek.set_model("deepseek-v4-flash").unwrap();
    assert!(deepseek.available_efforts().contains(&"swarm"));
    assert!(!deepseek.available_efforts().contains(&"swarm-deep"));
    deepseek.set_reasoning_effort("swarm").unwrap();
    assert_eq!(deepseek.reasoning_effort().as_deref(), Some("swarm"));
}

#[test]
fn reasoning_map_overrides_deepseek_family_detection() {
    // A DeepSeek-family model id with a configured map must use the map's
    // ladder and wire values, not the built-in DeepSeek ladder.
    let mut provider = make_mapped_compatible_provider(opencode_style_reasoning_map());
    provider.model = Arc::new(RwLock::new("deepseek-v4-flash".to_string()));
    provider.static_model_reasoning_maps.insert(
        "deepseek-v4-flash".to_string(),
        opencode_style_reasoning_map(),
    );

    assert_eq!(
        provider.available_efforts(),
        vec!["none", "high", "xhigh", "swarm"]
    );
    assert!(provider.set_reasoning_effort("max").is_err());
    provider.set_reasoning_effort("xhigh").unwrap();
    assert_eq!(provider.reasoning_effort().as_deref(), Some("xhigh"));
}

#[test]
fn reasoning_map_per_model_false_still_disables_effort() {
    // `reasoning = false` remains the force-off switch even when a map is
    // present: an empty/disabled configuration must never resurrect effort.
    let mut provider = make_mapped_compatible_provider(opencode_style_reasoning_map());
    provider
        .static_model_reasoning
        .insert(DEFAULT_MODEL.to_ascii_lowercase(), false);
    // supports_any_reasoning_effort follows the per-model override: false wins.
    assert!(!provider.supports_any_reasoning_effort());
    assert!(provider.set_reasoning_effort("high").is_err());
}

#[test]
fn reasoning_map_survives_model_switch_to_mapped_and_unmapped_models() {
    // Switching models dynamically: mapped models expose the map ladder,
    // unmapped models fall back to the built-in detection, and switching back
    // restores the map.
    let provider = make_mapped_compatible_provider(opencode_style_reasoning_map());
    provider.set_model("other-model").unwrap();
    // other-model is unmapped and matches no built-in family detection, so the
    // built-in OpenAI ladder applies (no xhigh/max rungs).
    assert!(!provider.available_efforts().contains(&"xhigh"));
    provider.set_model(DEFAULT_MODEL).unwrap();
    assert_eq!(
        provider.available_efforts(),
        vec!["none", "high", "xhigh", "swarm"]
    );
}

// extra_body effort fallback: providers that do not expose switchable effort
// can still send a fixed one via OpenAI-compatible `extra_body`. The display
// path must surface it, but live `/effort` changes must always win.
fn make_extra_body_provider(extra_body: serde_json::Value) -> OpenRouterProvider {
    let mut provider = make_custom_compatible_provider();
    provider.extra_body =
        Some(serde_json::from_value(extra_body).expect("valid extra_body object"));
    provider
}

#[test]
fn test_extra_body_reasoning_effort_top_level_shape() {
    let provider = make_extra_body_provider(serde_json::json!({
        "reasoning_effort": "high"
    }));
    assert_eq!(
        provider.effective_reasoning_effort(),
        Some("high".to_string())
    );
    // Switchable effort stays off: the fallback is display-only.
    assert_eq!(provider.reasoning_effort(), None);
}

#[test]
fn test_extra_body_reasoning_effort_reasoning_object_shape() {
    let provider = make_extra_body_provider(serde_json::json!({
        "reasoning": { "effort": "medium" }
    }));
    assert_eq!(
        provider.effective_reasoning_effort(),
        Some("medium".to_string())
    );
}

#[test]
fn test_extra_body_reasoning_effort_chat_template_kwargs_shape() {
    let provider = make_extra_body_provider(serde_json::json!({
        "chat_template_kwargs": { "reasoning_effort": "low" }
    }));
    assert_eq!(
        provider.effective_reasoning_effort(),
        Some("low".to_string())
    );
}

#[test]
fn test_extra_body_reasoning_effort_normalizes_and_trims() {
    let provider = make_extra_body_provider(serde_json::json!({
        "reasoning_effort": "  High  "
    }));
    assert_eq!(
        provider.effective_reasoning_effort(),
        Some("high".to_string())
    );
}

#[test]
fn test_extra_body_reasoning_effort_switchable_effort_wins() {
    let mut provider = make_extra_body_provider(serde_json::json!({
        "reasoning_effort": "high"
    }));
    // Mark effort switchable so the stored value is honored; without it
    // `reasoning_effort()` gates on support and always returns None.
    provider.reasoning_effort_support = Some(true);
    futures::executor::block_on(async {
        *provider.reasoning_effort.write().await = Some("low".to_string());
    });
    // Live /effort selection wins over the extra_body fallback.
    assert_eq!(provider.reasoning_effort(), Some("low".to_string()));
    assert_eq!(
        provider.effective_reasoning_effort(),
        Some("low".to_string())
    );
}

#[test]
fn test_extra_body_reasoning_effort_absent_when_unset() {
    let provider = make_custom_compatible_provider();
    assert_eq!(provider.effective_reasoning_effort(), None);
    let provider = make_extra_body_provider(serde_json::json!({
        "reasoning_effort": "   "
    }));
    assert_eq!(provider.effective_reasoning_effort(), None);
    let provider = make_extra_body_provider(serde_json::json!({
        "reasoning_effort": 42
    }));
    assert_eq!(provider.effective_reasoning_effort(), None);
}

// Integration: the real acceptance path is config.toml `[providers.llmg-coding]`
// with `extra_body` carrying a fixed reasoning effort and
// `supports_reasoning_effort = false` (strict schema, no switchable /effort).
// These tests go through the real named-profile constructor so the effort
// surfaces exactly as it would for a user config.
fn llmg_like_profile(extra_body: serde_json::Value) -> jcode_base::config::NamedProviderConfig {
    jcode_base::config::NamedProviderConfig {
        base_url: "https://llm.example.com/v1".to_string(),
        api_key: Some("test-key".to_string()),
        default_model: Some("together_ai/revolut-ltd/glm-5-2-nvfp4".to_string()),
        supports_reasoning_effort: Some(false),
        extra_body: Some(extra_body),
        ..Default::default()
    }
}

#[test]
fn named_profile_extra_body_effort_surfaces_through_real_constructor() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");

    let profile = llmg_like_profile(serde_json::json!({
        "reasoning_effort": "high"
    }));
    let provider = OpenRouterProvider::new_named_openai_compatible("llmg-coding", &profile)
        .expect("named profile should initialize");

    // Strict-schema endpoint: switching is off, reasoning_effort() is None...
    assert_eq!(provider.reasoning_effort(), None);
    // ...but the effort that will actually be sent must surface.
    assert_eq!(
        provider.effective_reasoning_effort(),
        Some("high".to_string())
    );
}

#[test]
fn named_profile_nim_chat_template_kwargs_effort_surfaces() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");

    // NVIDIA NIM style from issue #341: effort nested under
    // chat_template_kwargs rather than top-level.
    let profile = llmg_like_profile(serde_json::json!({
        "chat_template_kwargs": { "thinking": true, "reasoning_effort": "high" }
    }));
    let provider = OpenRouterProvider::new_named_openai_compatible("llmg-coding", &profile)
        .expect("named profile should initialize");

    assert_eq!(
        provider.effective_reasoning_effort(),
        Some("high".to_string())
    );
}

#[test]
fn named_profile_extra_body_env_var_path_surfaces_effort() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let _extra = EnvVarGuard::set(
        "JCODE_OPENAI_EXTRA_BODY",
        r#"{"reasoning":{"effort":"low"}}"#,
    );

    // No profile-level extra_body: the env-var source alone must surface.
    let profile = jcode_base::config::NamedProviderConfig {
        base_url: "https://llm.example.com/v1".to_string(),
        api_key: Some("test-key".to_string()),
        default_model: Some("glm-5-2-nvfp4".to_string()),
        supports_reasoning_effort: Some(false),
        ..Default::default()
    };
    let provider = OpenRouterProvider::new_named_openai_compatible("llmg-coding", &profile)
        .expect("named profile should initialize");

    assert_eq!(
        provider.effective_reasoning_effort(),
        Some("low".to_string())
    );
}

// Full acceptance chain in one test: user config.toml on disk -> strict
// config load -> real named-profile provider constructor -> display effort.
// This is the exact path a user with `extra_body` effort sees in the UI.
#[test]
fn user_toml_extra_body_effort_reaches_provider_display() {
    let _lock = ENV_LOCK.lock();
    let home = tempfile::tempdir().expect("temp home");
    let _home = EnvVarGuard::set("JCODE_HOME", home.path().to_str().expect("utf8 path"));
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let config_path = home.path().join("config.toml");
    std::fs::write(
        &config_path,
        r#"
[providers.llmg-coding]
type = "openai-compatible"
base_url = "https://llm.example.com/v1"
auth = "Bearer"
api_key = "test-key"
supports_reasoning_effort = false

[providers.llmg-coding.extra_body]
reasoning_effort = "high"

[[providers.llmg-coding.models]]
id = "together_ai/revolut-ltd/glm-5-2-nvfp4"
"#,
    )
    .expect("write config");

    let config = jcode_base::config::Config::load_strict().expect("strict config load");
    let profile = &config.providers["llmg-coding"];
    let provider = OpenRouterProvider::new_named_openai_compatible("llmg-coding", profile)
        .expect("provider from real config");

    // Display path must surface the effort the backend will actually get.
    assert_eq!(
        provider.effective_reasoning_effort(),
        Some("high".to_string()),
        "extra_body effort from user TOML must surface for display"
    );
    // Control path stays off: no switchable effort for this profile.
    assert_eq!(provider.reasoning_effort(), None);
    assert!(provider.available_efforts().is_empty());
}

// Request-body precedence, empirically verified against a fake HTTP server:
// when the user config sets BOTH switchable /effort support AND an extra_body
// reasoning_effort, the extra_body merge (issue #341, merged last) overrides
// the switchable field in the actual request. The display path intentionally
// shows the switchable value (live /effort wins). This documents the
// ambiguity warned about in the Provider trait docs: config that sets both
// is a user error and should be avoided.
#[test]
fn request_body_extra_body_effort_overrides_switchable_when_both_configured() {
    let _lock = ENV_LOCK.lock();
    let _namespace = EnvVarGuard::remove("JCODE_OPENROUTER_CACHE_NAMESPACE");
    let (base, request_rx) = spawn_single_response_chat_server();

    let profile = jcode_base::config::NamedProviderConfig {
        base_url: base,
        api_key: Some("test-key".to_string()),
        default_model: Some("glm-5-2-nvfp4".to_string()),
        // Switchable effort enabled AND extra_body carries a different one:
        // the ambiguous config the trait docs warn about.
        supports_reasoning_effort: Some(true),
        extra_body: Some(serde_json::json!({ "reasoning_effort": "high" })),
        ..Default::default()
    };
    let mut provider = OpenRouterProvider::new_named_openai_compatible("llmg-coding", &profile)
        .expect("named profile should initialize");
    provider.reasoning_effort_support = Some(true);

    // Simulate the user cycling /effort to "low" via the switchable state.
    futures::executor::block_on(async {
        *provider.reasoning_effort.write().await = Some("low".to_string());
    });

    // Display shows the switchable (live) value, per the agreed semantics.
    assert_eq!(provider.reasoning_effort(), Some("low".to_string()));
    assert_eq!(
        provider.effective_reasoning_effort(),
        Some("low".to_string()),
        "live /effort must win in the UI even when extra_body also sets one"
    );

    // But the wire request carries the extra_body value, because the merge
    // intentionally lets extra_body override jcode-generated fields (#341).
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let raw = rt.block_on(async {
        use futures::StreamExt;
        let stream = provider
            .complete(&[jcode_base::message::Message::user("hi")], &[], "", None)
            .await
            .expect("stream starts");
        let _ = stream.collect::<Vec<_>>().await;
        request_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("server captured request")
    });
    let body: serde_json::Value =
        serde_json::from_str(raw.split("\r\n\r\n").nth(1).expect("body")).expect("json body");
    assert_eq!(
        body["reasoning_effort"], "high",
        "extra_body merge order means the config value wins on the wire"
    );
}
