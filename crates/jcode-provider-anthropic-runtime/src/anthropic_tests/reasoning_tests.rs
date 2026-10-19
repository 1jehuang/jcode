use super::*;

#[test]
fn test_anthropic_reasoning_effort_request_parts() {
    let provider = AnthropicProvider::new();
    provider.set_model("claude-sonnet-4-6").unwrap();
    provider.set_reasoning_effort("none").unwrap();
    assert!(
        provider.set_reasoning_effort("minimal").is_err(),
        "Anthropic must reject rather than silently promote minimal to max"
    );

    assert_eq!(
        provider.available_efforts(),
        vec!["none", "low", "medium", "high", "max", "swarm"]
    );
    assert_eq!(provider.reasoning_effort().as_deref(), Some("none"));

    // Sonnet 4.6 supports the real `max` API level (but not `xhigh`).
    provider.set_reasoning_effort("max").unwrap();
    assert_eq!(provider.reasoning_effort().as_deref(), Some("max"));

    // `xhigh` is rejected on models that do not support it.
    assert!(provider.set_reasoning_effort("xhigh").is_err());

    provider.set_reasoning_effort("medium").unwrap();
    let (thinking, output_config, temperature) =
        provider.build_reasoning_request_parts("claude-sonnet-4-6", true);

    match thinking.expect("adaptive thinking should be enabled") {
        ApiThinking::Adaptive { display, .. } => assert_eq!(display, Some("summarized")),
        ApiThinking::Enabled { .. } => panic!("Claude 4.6 should use adaptive thinking"),
    }
    assert_eq!(
        output_config.expect("output_config should be set").effort,
        "medium"
    );
    assert_eq!(
        temperature, None,
        "thinking requests must omit OAuth temperature"
    );
}

#[test]
fn test_anthropic_preserves_swarm_sentinel_for_cycling() {
    // Storing the swarm effort must preserve the sentinel so the UI keeps
    // showing swarm mode, and the removed `swarm-deep` is rejected.
    let provider = AnthropicProvider::new();
    provider.set_model("claude-sonnet-4-6").unwrap();

    provider.set_reasoning_effort("swarm").unwrap();
    assert_eq!(provider.reasoning_effort().as_deref(), Some("swarm"));

    assert!(provider.set_reasoning_effort("swarm-deep").is_err());
    provider.set_reasoning_effort("high").unwrap();
    provider.set_reasoning_effort("swarm").unwrap();
    assert_eq!(provider.reasoning_effort().as_deref(), Some("swarm"));
}

#[test]
fn test_anthropic_show_thinking_enables_adaptive_thinking_without_effort() {
    // With no explicit reasoning effort, an adaptive-thinking model should still
    // request summarized thinking when the user has opted into the display.
    // Crucially, `output_config` must stay None so we do not force a stronger
    // (more expensive) reasoning level than the model's default.
    //
    // We use a non-Opus model here because Opus now carries an implicit `xhigh`
    // default (see `test_anthropic_opus_defaults_to_xhigh_effort`); Sonnet keeps
    // the model's own default so this invariant stays meaningful.
    //
    // `build_reasoning_request_parts_inner` takes the model directly, so we do
    // not depend on `set_model` accepting a particular catalog entry. With no
    // effort configured, `self.reasoning_effort()` resolves to None regardless
    // of the default model.
    let provider = AnthropicProvider::new();
    // Make the test independent of the ambient config's anthropic_reasoning_effort
    // by clearing the field directly; we only exercise the show_thinking path.
    *provider.reasoning_effort.write().unwrap() = None;

    // show_thinking = false: nothing requested.
    let (thinking, output_config, _temp) =
        provider.build_reasoning_request_parts_inner("claude-sonnet-4-6", true, false);
    assert!(
        thinking.is_none(),
        "no thinking should be requested when both effort and show_thinking are off"
    );
    assert!(output_config.is_none());

    // show_thinking = true: adaptive thinking requested, no output_config.
    let (thinking, output_config, temperature) =
        provider.build_reasoning_request_parts_inner("claude-sonnet-4-6", true, true);
    match thinking.expect("show_thinking should enable adaptive thinking") {
        ApiThinking::Adaptive { display, .. } => assert_eq!(display, Some("summarized")),
        ApiThinking::Enabled { .. } => panic!("Sonnet 4.6 should use adaptive thinking"),
    }
    assert!(
        output_config.is_none(),
        "show_thinking alone must not force an output reasoning effort"
    );
    assert_eq!(
        temperature, None,
        "thinking requests must omit OAuth temperature"
    );
}

#[test]
fn test_anthropic_explicit_none_effort_disables_thinking_even_with_show_thinking() {
    // Regression: with `display.show_thinking = true` (the default), setting
    // effort to `none` still requested adaptive thinking, so the user kept
    // seeing reasoning on Fable/Sonnet. An explicit `none` must suppress the
    // thinking request entirely, on both adaptive and manual thinking models.
    let provider = AnthropicProvider::new();
    *provider.reasoning_effort.write().unwrap() = Some("none".to_string());

    // Adaptive-thinking model (Fable 5 / Sonnet 4.6 family).
    let (thinking, output_config, temperature) =
        provider.build_reasoning_request_parts_inner("claude-fable-5", true, true);
    assert!(
        thinking.is_none(),
        "explicit effort=none must suppress thinking even when show_thinking is on"
    );
    assert!(output_config.is_none());
    assert_eq!(
        temperature,
        Some(1.0),
        "no thinking means the OAuth path restores temperature"
    );

    // Manual-thinking model.
    let (thinking, output_config, _temp) =
        provider.build_reasoning_request_parts_inner("claude-3-7-sonnet", false, true);
    assert!(
        thinking.is_none(),
        "explicit effort=none must suppress manual thinking budgets too"
    );
    assert!(output_config.is_none());
}

#[test]
fn test_anthropic_fable_defaults_to_high_effort() {
    // Fable 5 defaults to `high` reasoning when no explicit user effort is
    // configured. An explicit override still wins.
    let provider = AnthropicProvider::new();
    *provider.reasoning_effort.write().unwrap() = None;

    assert_eq!(
        AnthropicProvider::default_reasoning_effort_for_model("claude-fable-5").as_deref(),
        Some("high"),
    );

    // The default drives the request: output_config high + adaptive thinking.
    let (thinking, output_config, _temp) =
        provider.build_reasoning_request_parts_inner("claude-fable-5", true, false);
    assert_eq!(
        output_config
            .expect("Fable should default to a forced output effort")
            .effort,
        "high",
    );
    match thinking.expect("Fable default effort should enable adaptive thinking") {
        ApiThinking::Adaptive { display, .. } => assert_eq!(display, Some("summarized")),
        ApiThinking::Enabled { .. } => panic!("Fable 5 should use adaptive thinking"),
    }

    // The surfaced status mirrors the effective default for the active model.
    *provider
        .model
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = "claude-fable-5".to_string();
    assert_eq!(provider.reasoning_effort().as_deref(), Some("high"));

    // An explicit user override still wins over the Fable default.
    provider.set_reasoning_effort("low").unwrap();
    assert_eq!(provider.reasoning_effort().as_deref(), Some("low"));
    provider.set_reasoning_effort("none").unwrap();
    let (thinking, output_config, _temp) =
        provider.build_reasoning_request_parts_inner("claude-fable-5", true, true);
    assert!(
        thinking.is_none(),
        "explicit none must beat the high default and show_thinking"
    );
    assert!(output_config.is_none());
}

#[test]
fn test_anthropic_sonnet_5_supports_full_effort_ladder() {
    // `claude-sonnet-5` accepts `output_config` effort low..xhigh/max and
    // adaptive thinking (verified live 2026-07-07).
    assert!(AnthropicProvider::model_supports_output_effort(
        "claude-sonnet-5"
    ));
    assert!(AnthropicProvider::model_supports_adaptive_thinking(
        "claude-sonnet-5"
    ));
    assert!(AnthropicProvider::model_supports_xhigh_effort(
        "claude-sonnet-5"
    ));
    assert!(AnthropicProvider::model_supports_max_effort(
        "claude-sonnet-5"
    ));

    let provider = AnthropicProvider::new();
    *provider.reasoning_effort.write().unwrap() = None;
    *provider
        .model
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = "claude-sonnet-5".to_string();

    // No forced default: Sonnet keeps the model's own reasoning behavior.
    assert_eq!(
        AnthropicProvider::default_reasoning_effort_for_model("claude-sonnet-5"),
        None,
    );

    // Explicit efforts are accepted and drive the request.
    for effort in ["low", "medium", "high", "xhigh", "max"] {
        provider.set_reasoning_effort(effort).unwrap();
        assert_eq!(provider.reasoning_effort().as_deref(), Some(effort));
        let (thinking, output_config, _temp) =
            provider.build_reasoning_request_parts_inner("claude-sonnet-5", true, false);
        assert_eq!(
            output_config
                .expect("explicit effort should set output_config")
                .effort,
            effort,
        );
        assert!(matches!(thinking, Some(ApiThinking::Adaptive { .. })));
    }

    assert_eq!(
        provider.available_efforts(),
        vec!["none", "low", "medium", "high", "xhigh", "max", "swarm"],
    );
}

#[test]
fn test_anthropic_opus_defaults_to_xhigh_effort() {
    // Opus is a reasoning-heavy flagship, so when the user has *not* configured
    // an explicit effort it should default to its strongest supported level
    // (`xhigh` on Opus 4.7/4.8). This drives both the request `output_config`
    // and the surfaced `reasoning_effort()` status.
    let provider = AnthropicProvider::new();
    // Clear any ambient config-provided effort so we exercise the model default.
    *provider.reasoning_effort.write().unwrap() = None;

    assert_eq!(
        AnthropicProvider::default_reasoning_effort_for_model("claude-opus-4-8").as_deref(),
        Some("xhigh"),
    );
    assert_eq!(
        AnthropicProvider::default_reasoning_effort_for_model("claude-opus-4-7").as_deref(),
        Some("xhigh"),
    );
    // Older Opus does not support xhigh, so it clamps to high.
    assert_eq!(
        AnthropicProvider::default_reasoning_effort_for_model("claude-opus-4-5").as_deref(),
        Some("high"),
    );
    // Non-Opus models keep the model's own default (no forced effort).
    assert_eq!(
        AnthropicProvider::default_reasoning_effort_for_model("claude-sonnet-4-6"),
        None,
    );

    // Even without show_thinking, Opus forces its strongest output effort.
    let (thinking, output_config, _temp) =
        provider.build_reasoning_request_parts_inner("claude-opus-4-8", true, false);
    assert_eq!(
        output_config
            .expect("Opus should default to a forced output effort")
            .effort,
        "xhigh",
    );
    match thinking.expect("Opus default effort should enable adaptive thinking") {
        ApiThinking::Adaptive { display, .. } => assert_eq!(display, Some("summarized")),
        ApiThinking::Enabled { .. } => panic!("Opus 4.8 should use adaptive thinking"),
    }

    // The surfaced status mirrors the effective default for the active model.
    *provider
        .model
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = "claude-opus-4-8".to_string();
    assert_eq!(provider.reasoning_effort().as_deref(), Some("xhigh"));

    // An explicit user override still wins over the Opus default.
    provider.set_reasoning_effort("low").unwrap();
    assert_eq!(provider.reasoning_effort().as_deref(), Some("low"));
}

#[test]
fn test_anthropic_show_thinking_enables_manual_thinking_without_effort() {
    // Manual-thinking models (e.g. Claude 3.7 Sonnet) need a concrete budget;
    // with only the display toggle on we fall back to the minimal budget. We use
    // a non-Opus model here because Opus now carries an implicit strongest-effort
    // default (see `test_anthropic_opus_defaults_to_xhigh_effort`). The model is
    // passed directly so this does not depend on `set_model` validation.
    let provider = AnthropicProvider::new();
    // Independent of ambient config: clear any configured effort.
    *provider.reasoning_effort.write().unwrap() = None;

    let (thinking, _output_config, _temp) =
        provider.build_reasoning_request_parts_inner("claude-3-7-sonnet", false, false);
    assert!(thinking.is_none());

    let (thinking, _output_config, _temperature) =
        provider.build_reasoning_request_parts_inner("claude-3-7-sonnet", false, true);
    match thinking.expect("show_thinking should enable manual thinking") {
        ApiThinking::Enabled { budget_tokens } => assert_eq!(budget_tokens, 1_024),
        ApiThinking::Adaptive { .. } => panic!("Claude 3.7 Sonnet should use manual thinking"),
    }
}

#[test]
fn test_anthropic_max_alias_uses_strongest_real_effort() {
    // `max` is a real API level on output_config effort models.
    assert_eq!(
        AnthropicProvider::actual_effort_for_model("claude-sonnet-4-6", "max"),
        "max"
    );
    assert_eq!(
        AnthropicProvider::actual_effort_for_model("claude-opus-4-7", "max"),
        "max"
    );
    assert_eq!(
        AnthropicProvider::actual_effort_for_model("claude-opus-4-8", "max"),
        "max"
    );
    // Manual-thinking models (no output_config) clamp max to high.
    assert_eq!(
        AnthropicProvider::actual_effort_for_model("claude-opus-4-5", "max"),
        "high"
    );
    // xhigh still clamps to high where unsupported.
    assert_eq!(
        AnthropicProvider::actual_effort_for_model("claude-sonnet-4-6", "xhigh"),
        "high"
    );
    // The default resolved swarm effort preserves strongest-supported mapping.
    assert_eq!(
        AnthropicProvider::resolved_effort_for_model("claude-opus-4-8", "max"),
        "max"
    );
    assert_eq!(
        AnthropicProvider::resolved_effort_for_model("claude-sonnet-4-6", "max"),
        "max"
    );
    assert_eq!(
        AnthropicProvider::resolved_effort_for_model("claude-opus-4-5", "max"),
        "high"
    );
}

#[test]
fn test_anthropic_opus_48_fast_mode_service_tier_serializes_priority() {
    let provider = AnthropicProvider::new();
    provider.set_model("claude-opus-4-8").unwrap();

    assert_eq!(provider.available_service_tiers(), vec!["off", "priority"]);
    assert_eq!(provider.service_tier(), None);

    provider.set_service_tier("priority").unwrap();
    assert_eq!(provider.service_tier().as_deref(), Some("priority"));

    let request = ApiRequest {
        model: strip_1m_suffix(&provider.model()).to_string(),
        max_tokens: 1024,
        system: None,
        messages: vec![],
        tools: None,
        metadata: None,
        thinking: None,
        output_config: None,
        temperature: None,
        service_tier: provider.current_service_tier_for_model(&provider.model()),
        stream: true,
    };
    let value = serde_json::to_value(&request).unwrap();

    assert_eq!(value["model"], "claude-opus-4-8");
    assert_eq!(value["service_tier"], "auto");
}

#[test]
fn test_anthropic_fast_mode_is_limited_to_opus_48() {
    let provider = AnthropicProvider::new();
    provider.set_model("claude-opus-4-6").unwrap();

    assert!(provider.available_service_tiers().is_empty());
    assert!(provider.set_service_tier("priority").is_err());
    assert_eq!(provider.service_tier(), None);

    // A stale `[1m]` alias for a native-1M model is migrated to canonical form.
    provider.set_model("claude-opus-4-8[1m]").unwrap();
    assert_eq!(provider.model(), "claude-opus-4-8");
    provider.set_service_tier("priority").unwrap();
    assert_eq!(provider.service_tier().as_deref(), Some("priority"));

    provider.set_service_tier("off").unwrap();
    assert_eq!(provider.service_tier(), None);
}

#[test]
fn test_anthropic_manual_thinking_budget_for_opus_45() {
    let provider = AnthropicProvider::new();
    // Keep this request-builder test independent of the live/persisted Anthropic
    // model catalog, which may legitimately omit older Opus 4.5 models.
    *provider
        .model
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = "claude-opus-4-5".to_string();
    provider.set_reasoning_effort("high").unwrap();

    let (thinking, output_config, temperature) =
        provider.build_reasoning_request_parts("claude-opus-4-5", false);

    match thinking.expect("manual thinking should be enabled") {
        ApiThinking::Enabled { budget_tokens } => assert_eq!(budget_tokens, 8_192),
        ApiThinking::Adaptive { .. } => panic!("Claude Opus 4.5 should use manual thinking"),
    }
    assert_eq!(output_config.unwrap().effort, "high");
    assert_eq!(temperature, None);
}
