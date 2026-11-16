/// Grok Build routing spec: `grok-4.6` and `grok-build:grok-4.6` both become
/// `grok-build:grok-4.6` so `MultiProvider::set_model` dispatches to the Grok
/// Build runtime instead of treating the bare id as the active provider's model.
pub fn grok_build_model_spec(model: &str) -> String {
    let bare = model.strip_prefix("grok-build:").unwrap_or(model).trim();
    format!("grok-build:{bare}")
}

/// OpenRouter catalog id for a bare model: claude models gain an `anthropic/`
/// prefix, OpenAI models an `openai/` prefix, already-qualified ids pass
/// through. Mirrors `jcode_base::provider::openrouter_catalog_model_id` but
/// lives here so `RouteSelection::routed_model_spec` has no upward dep.
pub(crate) fn openrouter_catalog_model_id(model: &str) -> String {
    let trimmed = model.trim();
    match crate::models::provider_for_model(trimmed) {
        Some("claude") => format!("anthropic/{trimmed}"),
        Some("openai") => format!("openai/{trimmed}"),
        _ => trimmed.to_string(),
    }
}

/// Account-qualified OAuth method, preserving the case-sensitive saved label.
pub fn oauth_account_label(method: &str) -> Option<&str> {
    let (base, label) = method.trim().split_once('@')?;
    if !matches!(base, "claude-oauth" | "openai-oauth")
        || label.is_empty()
        || label.contains([':', '@'])
        || label.chars().any(char::is_control)
    {
        return None;
    }
    Some(label)
}
