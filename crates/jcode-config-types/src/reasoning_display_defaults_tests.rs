use super::*;

#[test]
fn anthropic_effort_defaults_to_low() {
    let config = ProviderConfig::default();
    assert_eq!(config.anthropic_reasoning_effort.as_deref(), Some("low"));
    let parsed: ProviderConfig = serde_json::from_str("{}").unwrap();
    assert_eq!(parsed.anthropic_reasoning_effort.as_deref(), Some("low"));
}

#[test]
fn proactive_floor_defaults_and_legacy_config() {
    let config = CompactionConfig::default();
    assert_eq!(config.reactive_threshold, 0.80);
    assert_eq!(config.reactive_threshold_tokens, 400_000);
    assert_eq!(config.proactive_floor, 0.40);
    assert_eq!(config.proactive_floor_tokens, 150_000);
    let parsed: CompactionConfig = serde_json::from_str(r#"{"proactive_floor":0.15}"#).unwrap();
    assert_eq!(parsed.proactive_floor, 0.15);
    assert_eq!(parsed.proactive_floor_tokens, 150_000);
    assert_eq!(parsed.reactive_threshold, 0.80);
    assert_eq!(parsed.reactive_threshold_tokens, 400_000);
}

#[test]
fn explicit_reasoning_display_is_distinguishable_from_the_legacy_fallback() {
    // Front-ends (the desktop) apply their own default only when the user
    // has not chosen one, so this flag must not be true just because
    // `show_thinking` happens to be set.
    let mut display = DisplayConfig {
        reasoning_display: None,
        show_thinking: true,
        ..DisplayConfig::default()
    };
    assert!(!display.has_explicit_reasoning_display());
    assert_eq!(display.reasoning_display(), ReasoningDisplayMode::Full);

    display.set_reasoning_display(ReasoningDisplayMode::Current);
    assert!(display.has_explicit_reasoning_display());
    assert_eq!(display.reasoning_display(), ReasoningDisplayMode::Current);
    assert!(
        display.show_thinking,
        "any active display mode must keep reasoning requested from the provider"
    );

    display.set_reasoning_display(ReasoningDisplayMode::Off);
    assert!(display.has_explicit_reasoning_display());
    assert!(!display.show_thinking);
}
