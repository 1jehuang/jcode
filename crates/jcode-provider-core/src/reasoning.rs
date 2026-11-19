//! Shared reasoning-effort ladders.
//!
//! Keep these in provider-core so provider runtimes and UI clients expose the
//! same ordered values. `swarm` is a Jcode UI sentinel rather than a
//! wire-level provider value, but it belongs in the selectable ladder.

/// OpenAI Responses API effort levels, followed by Jcode's swarm mode.
pub const OPENAI_SELECTABLE_EFFORTS: &[&str] = &[
    "none", "minimal", "low", "medium", "high", "xhigh", "max", "swarm",
];

/// OpenRouter's unified reasoning effort levels.
///
/// OpenRouter currently treats `max` as an alias for `xhigh`, so it is not a
/// separate rung in this ladder.
pub const OPENROUTER_SELECTABLE_EFFORTS: &[&str] =
    &["none", "minimal", "low", "medium", "high", "xhigh", "swarm"];

/// Direct DeepSeek effort levels, followed by Jcode's swarm mode.
pub const DEEPSEEK_SELECTABLE_EFFORTS: &[&str] = &["none", "low", "medium", "high", "max", "swarm"];

/// Effective effort a provider applies when the caller has not selected one.
///
/// ACP requires a concrete `currentValue` for a select option, so an unset
/// effort still has to be displayed. Reporting a middle rung that the provider
/// does not actually use misleads clients (DeepSeek runs at `high` by default,
/// not `medium`). Only providers with a documented default are covered here;
/// `None` means unknown and callers may keep their own presentation fallback.
pub fn default_reasoning_effort(
    provider_name: Option<&str>,
    model_name: Option<&str>,
) -> Option<&'static str> {
    let provider = provider_name.unwrap_or_default().to_ascii_lowercase();
    let model = model_name.unwrap_or_default().to_ascii_lowercase();

    // OpenRouter routes each model to an upstream whose own default applies.
    if provider.contains("openrouter") {
        return None;
    }

    // DeepSeek thinking mode is enabled by default at effort `high`
    // (https://api-docs.deepseek.com/guides/thinking_mode). A thinking-mode
    // override that disables it is owned by the daemon's provider and reported
    // separately via `Provider::default_reasoning_effort`, so clients never
    // infer it from their own environment.
    if provider.contains("deepseek") || model.contains("deepseek") {
        return Some("high");
    }

    // Platform-API-only GPT Pro models default to `high`; `gpt-5-pro` does not
    // even accept a lower effort.
    if crate::is_openai_api_only_pro_model(&model) {
        return Some("high");
    }

    // OpenAI reasoning models default to `medium` when no effort is passed.
    let is_openai_model = model.starts_with("gpt-")
        || model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
        || model.starts_with("o5");
    if provider.contains("openai") || provider.contains("codex") || is_openai_model {
        return Some("medium");
    }

    None
}

/// Convert a provider-advertised OpenAI/OpenRouter effort into the canonical
/// static value used by the provider trait.
pub fn canonical_reasoning_effort(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "none" => Some("none"),
        "minimal" => Some("minimal"),
        "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "xhigh" => Some("xhigh"),
        "max" => Some("max"),
        _ => None,
    }
}

/// Infer the selectable effort ladder when only provider/model identity is
/// available, such as in a remote TUI session.
pub fn inferred_reasoning_efforts(
    provider_name: Option<&str>,
    model_name: Option<&str>,
) -> Vec<&'static str> {
    let provider = provider_name.unwrap_or_default().to_ascii_lowercase();
    let model = model_name.unwrap_or_default().to_ascii_lowercase();

    if provider.contains("openrouter") {
        return OPENROUTER_SELECTABLE_EFFORTS.to_vec();
    }

    if provider.contains("deepseek") || model.contains("deepseek") {
        return DEEPSEEK_SELECTABLE_EFFORTS.to_vec();
    }

    if provider.contains("z.ai") || provider == "zai" || model.starts_with("glm-") {
        return OPENAI_SELECTABLE_EFFORTS.to_vec();
    }

    let is_openai_model = model.starts_with("gpt-")
        || model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
        || model.starts_with("o5");
    if provider.contains("openai-compatible") {
        return if is_openai_model {
            OPENAI_SELECTABLE_EFFORTS.to_vec()
        } else {
            Vec::new()
        };
    }

    let is_anthropic = provider.contains("anthropic")
        || provider.contains("claude")
        || model.starts_with("claude-");
    if is_anthropic {
        let caps = crate::anthropic_reasoning_caps(&model);
        if !caps.supports_reasoning_effort() {
            return Vec::new();
        }
        let mut efforts = vec!["none", "low", "medium", "high"];
        if caps.xhigh_effort {
            efforts.push("xhigh");
        }
        if caps.max_effort {
            efforts.push("max");
        }
        efforts.push("swarm");
        return efforts;
    }

    let is_openai = provider.contains("openai") || provider.contains("codex") || is_openai_model;
    if is_openai {
        return OPENAI_SELECTABLE_EFFORTS.to_vec();
    }

    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_ladders_preserve_distinct_max_semantics() {
        assert_eq!(
            inferred_reasoning_efforts(Some("openai"), Some("gpt-5.4")),
            OPENAI_SELECTABLE_EFFORTS
        );
        assert!(OPENAI_SELECTABLE_EFFORTS.contains(&"max"));
        assert!(OPENAI_SELECTABLE_EFFORTS.contains(&"minimal"));
        assert!(OPENROUTER_SELECTABLE_EFFORTS.contains(&"minimal"));
        assert!(!OPENROUTER_SELECTABLE_EFFORTS.contains(&"max"));
        assert!(DEEPSEEK_SELECTABLE_EFFORTS.contains(&"max"));
        assert_eq!(
            inferred_reasoning_efforts(Some("openai-compatible:custom"), Some("gpt-5.6")),
            OPENAI_SELECTABLE_EFFORTS,
            "direct OpenAI-compatible runtimes use the OpenAI reasoning_effort vocabulary"
        );
    }

    #[test]
    fn zai_and_glm_identities_expose_openai_efforts() {
        assert_eq!(
            inferred_reasoning_efforts(Some("Z.AI"), Some("glm-5.3")),
            OPENAI_SELECTABLE_EFFORTS
        );
        assert_eq!(
            inferred_reasoning_efforts(Some("openai-compatible:zai"), Some("glm-5.3-flash")),
            OPENAI_SELECTABLE_EFFORTS
        );
    }

    #[test]
    fn anthropic_ladder_comes_from_model_capabilities() {
        assert_eq!(
            inferred_reasoning_efforts(Some("anthropic"), Some("claude-sonnet-4-6")),
            vec!["none", "low", "medium", "high", "max", "swarm"]
        );
        assert_eq!(
            inferred_reasoning_efforts(Some("anthropic"), Some("claude-opus-4-7")),
            vec!["none", "low", "medium", "high", "xhigh", "max", "swarm"]
        );
    }

    #[test]
    fn documented_provider_defaults_are_not_middle_rungs() {
        // DeepSeek thinking mode is on at `high` unless the caller overrides it.
        assert_eq!(
            default_reasoning_effort(Some("deepseek"), Some("deepseek-flash")),
            Some("high")
        );
        assert_eq!(
            default_reasoning_effort(Some("OpenCode Go"), Some("opencode-go:deepseek-v4.1-flash")),
            Some("high")
        );
        assert_eq!(
            default_reasoning_effort(Some("openai"), Some("gpt-5.4")),
            Some("medium")
        );
        // gpt-5-pro accepts only `high`, so a `medium` default would be a value
        // the model cannot run.
        assert_eq!(
            default_reasoning_effort(Some("openai"), Some("gpt-5-pro")),
            Some("high")
        );
    }

    #[test]
    fn unknown_provider_defaults_stay_unknown() {
        // OpenRouter routes to an upstream whose default jcode cannot know.
        assert_eq!(
            default_reasoning_effort(Some("openrouter"), Some("deepseek/deepseek-v4-pro")),
            None
        );
        assert_eq!(
            default_reasoning_effort(Some("anthropic"), Some("claude-sonnet-4-6")),
            None
        );
        assert_eq!(default_reasoning_effort(None, None), None);
    }
}
