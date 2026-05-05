/// Available Claude models used by model lists and provider routing.
pub const ALL_CLAUDE_MODELS: &[&str] = &[
    "claude-opus-4-6",
    "claude-opus-4-6[1m]",
    "claude-sonnet-4-6",
    "claude-sonnet-4-6[1m]",
    "claude-haiku-4-5",
    "claude-opus-4-5",
    "claude-sonnet-4-5",
    "claude-sonnet-4-20250514",
];

/// Available OpenAI models used by model lists and provider routing.
pub const ALL_OPENAI_MODELS: &[&str] = &[
    "gpt-5.5",
    "gpt-5.4",
    "gpt-5.4-pro",
    "gpt-5.3-codex",
    "gpt-5.3-codex-spark",
    "gpt-5.2-chat-latest",
    "gpt-5.2-codex",
    "gpt-5.2-pro",
    "gpt-5.1-codex-mini",
    "gpt-5.1-codex-max",
    "gpt-5.2",
    "gpt-5.1-chat-latest",
    "gpt-5.1",
    "gpt-5.1-codex",
    "gpt-5-chat-latest",
    "gpt-5-codex",
    "gpt-5-codex-mini",
    "gpt-5-pro",
    "gpt-5-mini",
    "gpt-5-nano",
    "gpt-5",
];

/// Default context window size when model-specific data isn't known.
pub const DEFAULT_CONTEXT_LIMIT: usize = 200_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCapabilities {
    pub provider: Option<String>,
    pub context_window: Option<usize>,
}

fn normalize_provider_id(provider: &str) -> String {
    provider.trim().to_ascii_lowercase()
}

pub fn provider_key_from_hint(provider_hint: Option<&str>) -> Option<&'static str> {
    let normalized = normalize_provider_id(provider_hint?);
    match normalized.as_str() {
        "anthropic" | "claude" => Some("claude"),
        "openai" => Some("openai"),
        "openrouter" => Some("openrouter"),
        "copilot" | "github copilot" => Some("copilot"),
        "antigravity" => Some("antigravity"),
        "gemini" | "google gemini" => Some("gemini"),
        "cursor" => Some("cursor"),
        _ => None,
    }
}

fn model_id_for_capability_lookup(model: &str, provider: Option<&str>) -> (String, bool) {
    let normalized = model.trim().to_ascii_lowercase();
    let (base, is_1m) = if let Some(base) = normalized.strip_suffix("[1m]") {
        (base.to_string(), true)
    } else {
        (normalized, false)
    };

    let lookup = if matches!(provider, Some("openrouter")) || base.contains('/') {
        base.rsplit('/').next().unwrap_or(&base).to_string()
    } else {
        base
    };

    (lookup, is_1m)
}

fn copilot_context_limit_for_model(model: &str) -> usize {
    match model {
        "claude-sonnet-4" | "claude-sonnet-4-6" | "claude-sonnet-4.6" => 128_000,
        "claude-opus-4-6" | "claude-opus-4.6" | "claude-opus-4.6-fast" => 200_000,
        "claude-opus-4.5" | "claude-opus-4-5" => 200_000,
        "claude-sonnet-4.5" | "claude-sonnet-4-5" => 200_000,
        "claude-haiku-4.5" | "claude-haiku-4-5" => 200_000,
        "gpt-4o" | "gpt-4o-mini" => 128_000,
        m if m.starts_with("gpt-4o") => 128_000,
        m if m.starts_with("gpt-4.1") => 128_000,
        m if m.starts_with("gpt-5") => 128_000,
        "o3-mini" | "o4-mini" => 128_000,
        m if m.starts_with("gemini-2.0-flash") => 1_000_000,
        m if m.starts_with("gemini-2.5") => 1_000_000,
        m if m.starts_with("gemini-3") => 1_000_000,
        _ => 128_000,
    }
}

/// Return the static provider class for a built-in model name.
///
/// Root providers may layer runtime-only provider catalogs on top of this.
pub fn provider_for_model_with_hint(
    model: &str,
    provider_hint: Option<&str>,
) -> Option<&'static str> {
    if let Some(provider) = provider_key_from_hint(provider_hint) {
        return Some(provider);
    }

    let model = model.trim();
    if model.contains('@') {
        Some("openrouter")
    } else if ALL_CLAUDE_MODELS.contains(&model) {
        Some("claude")
    } else if ALL_OPENAI_MODELS.contains(&model) {
        Some("openai")
    } else if model.contains('/') {
        Some("openrouter")
    } else if model.starts_with("claude-") {
        Some("claude")
    } else if model.starts_with("gpt-") {
        Some("openai")
    } else if model.starts_with("gemini-") {
        Some("gemini")
    } else {
        None
    }
}

pub fn provider_for_model(model: &str) -> Option<&'static str> {
    provider_for_model_with_hint(model, None)
}

pub fn context_limit_for_model_with_provider_and_cache(
    model: &str,
    provider_hint: Option<&str>,
    cached_context_limit: impl Fn(&str) -> Option<usize>,
) -> Option<usize> {
    let provider = provider_key_from_hint(provider_hint).or_else(|| provider_for_model(model));
    let (model, is_1m) = model_id_for_capability_lookup(model, provider);
    let model = model.as_str();

    if matches!(provider, Some("copilot")) {
        return Some(copilot_context_limit_for_model(model));
    }

    // Spark variant has a smaller context window than the full codex model.
    if model.starts_with("gpt-5.3-codex-spark") {
        return Some(128_000);
    }

    if model.starts_with("gpt-5.2-chat")
        || model.starts_with("gpt-5.1-chat")
        || model.starts_with("gpt-5-chat")
    {
        return Some(128_000);
    }

    // GPT-5.4-family models should default to the long-context window.
    // The live Codex OAuth catalog can still override this via the dynamic cache above.
    if model.starts_with("gpt-5.4") {
        return Some(1_000_000);
    }

    // Most GPT-5.x codex/reasoning models: 272k per Codex backend API.
    if model.starts_with("gpt-5") {
        return Some(272_000);
    }

    if model.starts_with("claude-opus-4-6") || model.starts_with("claude-opus-4.6") {
        return Some(if is_1m { 1_048_576 } else { 200_000 });
    }

    if model.starts_with("claude-sonnet-4-6") || model.starts_with("claude-sonnet-4.6") {
        return Some(if is_1m { 1_048_576 } else { 200_000 });
    }

    if model.starts_with("claude-opus-4-5") || model.starts_with("claude-opus-4.5") {
        return Some(200_000);
    }

    if let Some(limit) = cached_context_limit(model) {
        return Some(limit);
    }

    if model.starts_with("gemini-2.0-flash")
        || model.starts_with("gemini-2.5")
        || model.starts_with("gemini-3")
    {
        return Some(1_000_000);
    }

    None
}

pub fn context_limit_for_model_with_provider(
    model: &str,
    provider_hint: Option<&str>,
) -> Option<usize> {
    context_limit_for_model_with_provider_and_cache(model, provider_hint, |_| None)
}

pub fn context_limit_for_model(model: &str) -> Option<usize> {
    context_limit_for_model_with_provider(model, None)
}

/// Normalize a Copilot-style model name to the canonical form used by our
/// provider model lists. Copilot uses dots in version numbers (e.g.
/// `claude-opus-4.6`) while canonical lists use hyphens (`claude-opus-4-6`).
/// Returns None if no normalization is needed (model already canonical or unknown).
pub fn normalize_copilot_model_name(model: &str) -> Option<&'static str> {
    for canonical in ALL_CLAUDE_MODELS.iter().chain(ALL_OPENAI_MODELS.iter()) {
        if *canonical == model {
            return None;
        }
    }
    let normalized = model.replace('.', "-");
    ALL_CLAUDE_MODELS
        .iter()
        .chain(ALL_OPENAI_MODELS.iter())
        .find(|canonical| **canonical == normalized)
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_limit_handles_claude_1m_aliases() {
        assert_eq!(
            context_limit_for_model_with_provider("claude-opus-4-6[1m]", Some("claude")),
            Some(1_048_576)
        );
        assert_eq!(
            context_limit_for_model_with_provider("claude-sonnet-4.6", Some("claude")),
            Some(200_000)
        );
    }

    #[test]
    fn context_limit_handles_copilot_hint() {
        assert_eq!(
            context_limit_for_model_with_provider("gpt-5.4", Some("copilot")),
            Some(128_000)
        );
        assert_eq!(
            context_limit_for_model_with_provider("gemini-2.5-pro", Some("copilot")),
            Some(1_000_000)
        );
    }

    #[test]
    fn context_limit_uses_cache_for_unknown_models() {
        assert_eq!(
            context_limit_for_model_with_provider_and_cache("custom-model", None, |model| {
                (model == "custom-model").then_some(42_000)
            }),
            Some(42_000)
        );
    }

    #[test]
    fn normalizes_copilot_model_names() {
        assert_eq!(
            normalize_copilot_model_name("claude-opus-4.6"),
            Some("claude-opus-4-6")
        );
        assert_eq!(normalize_copilot_model_name("claude-opus-4-6"), None);
    }
}
