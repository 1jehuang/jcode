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

/// Ordered Antigravity effort suffixes encoded on sibling model IDs
/// (for example `gemini-4-neon-low`, `gemini-4-neon-medium`, `gemini-4-neon-high`,
/// `gemini-4-neon-xhigh`).
pub const ANTIGRAVITY_EFFORT_SUFFIXES: &[&str] = &["low", "medium", "high", "xhigh"];

/// Split an Antigravity effort-suffixed model ID into `(base_model, effort)`.
///
/// Strips an optional `antigravity:` prefix before matching `-low`, `-medium`,
/// `-high`, or `-xhigh`. Returns `None` for models without a recognized effort
/// suffix (such as `gemini-3-flash` or `gemini-4-carbon-disabled`).
pub fn split_antigravity_model_effort(model: &str) -> Option<(&str, &'static str)> {
    let trimmed = crate::strip_own_model_prefix(model, "antigravity:");
    for &effort in ANTIGRAVITY_EFFORT_SUFFIXES {
        let suffix_len = 1 + effort.len();
        if trimmed.len() > suffix_len {
            let split_at = trimmed.len() - suffix_len;
            if trimmed.is_char_boundary(split_at)
                && trimmed.as_bytes()[split_at] == b'-'
                && trimmed[split_at + 1..].eq_ignore_ascii_case(effort)
            {
                let base = &trimmed[..split_at];
                if !base.is_empty() {
                    return Some((base, effort));
                }
            }
        }
    }
    None
}

/// Discover the ordered selectable reasoning efforts for an Antigravity model
/// from a catalog of available Antigravity model IDs.
///
/// Returns matching sibling effort suffixes in canonical order (`low`,
/// `medium`, `high`, `xhigh`) followed by `"swarm"`, or an empty list when the
/// model does not have multiple sibling effort variants.
pub fn antigravity_efforts_from_catalog<'a, I>(
    model: &str,
    available_model_ids: I,
) -> Vec<&'static str>
where
    I: IntoIterator<Item = &'a str>,
{
    let Some((base, current_effort)) = split_antigravity_model_effort(model) else {
        return Vec::new();
    };
    let mut found = [false; 4];
    for id in available_model_ids {
        if let Some((candidate_base, effort)) = split_antigravity_model_effort(id)
            && candidate_base.eq_ignore_ascii_case(base)
            && let Some(pos) = ANTIGRAVITY_EFFORT_SUFFIXES
                .iter()
                .position(|e| *e == effort)
        {
            found[pos] = true;
        }
    }
    if let Some(pos) = ANTIGRAVITY_EFFORT_SUFFIXES
        .iter()
        .position(|e| *e == current_effort)
    {
        found[pos] = true;
    }
    let mut efforts: Vec<&'static str> = ANTIGRAVITY_EFFORT_SUFFIXES
        .iter()
        .zip(found)
        .filter_map(|(&effort, present)| present.then_some(effort))
        .collect();
    if efforts.len() < 2 {
        return Vec::new();
    }
    efforts.push("swarm");
    efforts
}

/// Infer the selectable effort ladder for an Antigravity model when the live
/// catalog is not yet available (for example during remote TUI bootstrap).
fn inferred_antigravity_efforts(model: &str) -> Vec<&'static str> {
    let trimmed = crate::strip_own_model_prefix(model, "antigravity:");
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("default") {
        // Default Antigravity fallback model is `gemini-4-neon-low`.
        return vec!["low", "medium", "high", "xhigh", "swarm"];
    }
    let Some((base, effort)) = split_antigravity_model_effort(trimmed) else {
        return Vec::new();
    };
    let base_lower = base.to_ascii_lowercase();
    if base_lower.starts_with("gpt-oss-") {
        // Antigravity only ships a single `-medium` tier for `gpt-oss-*`.
        return Vec::new();
    }
    if base_lower == "gemini-3.1-pro" {
        return vec!["low", "high", "swarm"];
    }
    if base_lower.ends_with("-tiered") {
        return vec!["high", "xhigh", "swarm"];
    }
    if base_lower == "gemini-4-neon" || base_lower == "gemini-4-hydrogen" || effort == "xhigh" {
        return vec!["low", "medium", "high", "xhigh", "swarm"];
    }
    vec!["low", "medium", "high", "swarm"]
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

    if provider.contains("antigravity") {
        return inferred_antigravity_efforts(&model);
    }

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
    fn antigravity_splits_and_discovers_sibling_effort_ladders() {
        assert_eq!(
            split_antigravity_model_effort("gemini-4-neon-low"),
            Some(("gemini-4-neon", "low"))
        );
        assert_eq!(
            split_antigravity_model_effort("antigravity:gemini-4-neon-xhigh"),
            Some(("gemini-4-neon", "xhigh"))
        );
        assert_eq!(
            split_antigravity_model_effort("gemini-4-hydrogen-tiered-xhigh"),
            Some(("gemini-4-hydrogen-tiered", "xhigh"))
        );
        assert_eq!(
            split_antigravity_model_effort("claude-opus-5-5-medium"),
            Some(("claude-opus-5-5", "medium"))
        );
        assert_eq!(split_antigravity_model_effort("gemini-3-flash"), None);
        assert_eq!(
            split_antigravity_model_effort("gemini-4-carbon-disabled"),
            None
        );

        let catalog = [
            "gemini-4-neon-low",
            "gemini-4-neon-medium",
            "gemini-4-neon-high",
            "gemini-4-neon-xhigh",
            "gemini-4-hydrogen-tiered-high",
            "gemini-4-hydrogen-tiered-xhigh",
            "claude-opus-5-5-low",
            "claude-opus-5-5-medium",
            "claude-opus-5-5-high",
            "gpt-oss-120b-medium",
            "gemini-3-flash",
            "gemini-5-future-low",
            "gemini-5-future-xhigh",
        ];
        assert_eq!(
            antigravity_efforts_from_catalog("gemini-4-neon-low", catalog),
            vec!["low", "medium", "high", "xhigh", "swarm"]
        );
        assert_eq!(
            antigravity_efforts_from_catalog("gemini-4-hydrogen-tiered-high", catalog),
            vec!["high", "xhigh", "swarm"]
        );
        assert_eq!(
            antigravity_efforts_from_catalog("claude-opus-5-5-low", catalog),
            vec!["low", "medium", "high", "swarm"]
        );
        assert_eq!(
            antigravity_efforts_from_catalog("gemini-5-future-low", catalog),
            vec!["low", "xhigh", "swarm"]
        );
        assert!(antigravity_efforts_from_catalog("gpt-oss-120b-medium", catalog).is_empty());
        assert!(antigravity_efforts_from_catalog("gemini-3-flash", catalog).is_empty());

        // Provider inference checks Antigravity before Anthropic/OpenAI model prefixes.
        assert_eq!(
            inferred_reasoning_efforts(Some("antigravity"), Some("gemini-4-neon-low")),
            vec!["low", "medium", "high", "xhigh", "swarm"]
        );
        assert_eq!(
            inferred_reasoning_efforts(Some("antigravity"), Some("claude-opus-5-5-low")),
            vec!["low", "medium", "high", "swarm"]
        );
        assert_eq!(
            inferred_reasoning_efforts(Some("antigravity"), Some("gemini-3.1-pro-low")),
            vec!["low", "high", "swarm"]
        );
        assert!(
            inferred_reasoning_efforts(Some("antigravity"), Some("gpt-oss-120b-medium")).is_empty()
        );
        assert!(inferred_reasoning_efforts(Some("antigravity"), Some("gemini-3-flash")).is_empty());
    }
}
