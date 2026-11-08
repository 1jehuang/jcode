use super::*;

#[test]
fn placeholder_default_is_current_flagship() {
    assert_eq!(DEFAULT_MODEL, "anthropic/claude-opus-5.5");
}

#[test]
fn placeholder_default_promotes_to_newest_flagship() {
    let catalog = [
        "anthropic/claude-sonnet-4",
        "anthropic/claude-haiku-4.5",
        "anthropic/claude-opus-4.6",
        "anthropic/claude-opus-5.5",
        "anthropic/claude-opus-5.6",
        "openai/gpt-5.5",
        "deepseek/deepseek-v4-pro",
    ];
    assert_eq!(
        select_promoted_default(DEFAULT_MODEL, &catalog).as_deref(),
        Some("anthropic/claude-opus-5.6")
    );
}

#[test]
fn placeholder_default_never_overrides_a_chosen_model() {
    let catalog = ["anthropic/claude-sonnet-4", "anthropic/claude-opus-5.5"];
    assert_eq!(
        select_promoted_default("deepseek/deepseek-v4-pro", &catalog),
        None
    );
}

#[test]
fn placeholder_default_stays_when_catalog_has_nothing_better() {
    assert_eq!(
        select_promoted_default(DEFAULT_MODEL, &["deepseek/deepseek-v4-pro"]),
        None
    );
    assert_eq!(select_promoted_default(DEFAULT_MODEL, &[]), None);
    assert_eq!(
        select_promoted_default(DEFAULT_MODEL, &[DEFAULT_MODEL]),
        None
    );
}
