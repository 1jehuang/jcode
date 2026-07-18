/// Quality-first default for Claude-capable routes.
pub const DEFAULT_CLAUDE_MODEL: &str = "claude-fable-5";

/// Quality-first default for OpenAI-capable routes.
pub const DEFAULT_OPENAI_MODEL: &str = "gpt-5.6-sol";

/// Available Claude models used by model lists and provider routing.
///
/// NOTE: The Mythos preview family was retired by Anthropic and 404s, so it is
/// intentionally NOT listed here. `claude-fable-5` was briefly retired but is
/// live again. The list is curated best-first; position 0 is the flagship
/// used for post-login default selection.
pub const ALL_CLAUDE_MODELS: &[&str] = &[
    DEFAULT_CLAUDE_MODEL,
    "claude-opus-4-8",
    "claude-opus-4-6",
    "claude-opus-4-6[1m]",
    "claude-sonnet-5",
    "claude-sonnet-4-6",
    "claude-sonnet-4-6[1m]",
    "claude-haiku-4-5",
    "claude-opus-4-5",
    "claude-sonnet-4-5",
    "claude-sonnet-4-20250514",
];

/// Available OpenAI models used by model lists and provider routing.
/// The list is curated best-first; position 0 is the quality-first default.
pub const CHATGPT_WEB_MODEL: &str = "gpt-5.6-pro[web]";

/// GPT Pro reasoning models. These are exposed only on the OpenAI platform
/// API (`api.openai.com` with an `OPENAI_API_KEY`); the ChatGPT/Codex OAuth
/// backend rejects them ("not supported when using Codex with a ChatGPT
/// account"). Keep them in their own list so the OAuth-scoped Codex catalog
/// can never hide them from the picker and so route building can mark them
/// API-key-only.
pub const OPENAI_API_ONLY_PRO_MODELS: &[&str] =
    &["gpt-5.5-pro", "gpt-5.4-pro", "gpt-5.2-pro", "gpt-5-pro"];

/// True when `model` is a GPT Pro model that only works with an OpenAI
/// platform API key (never ChatGPT/Codex OAuth).
pub fn is_openai_api_only_pro_model(model: &str) -> bool {
    let trimmed = model.trim();
    OPENAI_API_ONLY_PRO_MODELS
        .iter()
        .any(|pro| trimmed.eq_ignore_ascii_case(pro))
        || (trimmed.len() > 4
            && OPENAI_API_ONLY_PRO_MODELS
                .iter()
                .any(|pro| trimmed.to_ascii_lowercase().starts_with(&format!("{pro}-"))))
}

pub const ALL_OPENAI_MODELS: &[&str] = &[
    DEFAULT_OPENAI_MODEL,
    // ChatGPT web-only route. The `[web]` suffix is intentionally part of the
    // jcode model id so it can never be mistaken for an API/Codex model with
    // the same upstream slug.
    CHATGPT_WEB_MODEL,
    "gpt-5.5-pro",
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

pub fn is_listable_model_name(model: &str) -> bool {
    let trimmed = model.trim();
    !trimmed.is_empty() && !matches!(trimmed, "copilot models" | "openrouter models")
}

fn model_id_for_capability_lookup(model: &str, provider: Option<&str>) -> (String, bool) {
    let normalized = model.trim().to_ascii_lowercase();
    let (base, is_1m) = crate::model_id::split_long_context(&normalized);

    let lookup = if matches!(provider, Some("openrouter")) || base.contains('/') {
        crate::model_id::slash_base(base).to_string()
    } else {
        base.to_string()
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

/// Whether `model` resolves to a Claude family jcode classifies statically
/// (i.e. one whose long-context behavior we have verified). Matches by family
/// prefix so dated aliases (`claude-opus-4-5-20251101`) and `[1m]` suffixes are
/// covered, while unknown/future Claude ids fall through to the dynamic cache.
fn base_is_known_claude_model(base: &str) -> bool {
    const KNOWN_CLAUDE_PREFIXES: &[&str] = &[
        "claude-opus-4-8",
        "claude-opus-4.8",
        "claude-opus-4-7",
        "claude-opus-4.7",
        "claude-opus-4-6",
        "claude-opus-4.6",
        "claude-opus-4-5",
        "claude-opus-4.5",
        "claude-sonnet-5",
        "claude-sonnet-4-6",
        "claude-sonnet-4.6",
        "claude-sonnet-4-5",
        "claude-sonnet-4.5",
        "claude-haiku-4-5",
        "claude-haiku-4.5",
        // Fable 5 is a native-1M flagship (see `anthropic_context_mode`). It is
        // not in `ALL_CLAUDE_MODELS` because Anthropic retired its public id (it
        // 404s and is served as Opus 4.8), but sessions can still be pinned to
        // it, so it must classify as 1M instead of falling through to 200K.
        "claude-fable-5",
        "claude-fable",
    ];
    KNOWN_CLAUDE_PREFIXES
        .iter()
        .any(|prefix| base.starts_with(prefix))
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

    // Claude models: classify long-context behavior centrally. This is the
    // authoritative source for known Claude models because the live catalog's
    // `max_input_tokens` field over-advertises 1M for models that are actually
    // 200K-capped (verified against the live API). Unknown/future Claude models
    // fall through to the dynamic cache below.
    if base_is_known_claude_model(model) {
        let mode = crate::anthropic::anthropic_context_mode(model);
        return Some(if is_1m {
            mode.long_context_window()
        } else {
            mode.default_context_window()
        });
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

    // Open-weight model families served by many OpenAI-compatible gateways
    // (Z.AI, Moonshot, MiniMax, Alibaba, etc.). Their `/v1/models` endpoints
    // frequently omit `context_length`, so without this classifier these models
    // fall back to the generic 200K default even when their real window is
    // larger (e.g. GLM-5.2's 1M). This is checked AFTER the dynamic cache so a
    // live catalog or user `context_window` config always wins.
    if let Some(limit) = open_weight_family_context_limit(model) {
        return Some(limit);
    }

    None
}

/// Best-effort context window for well-known open-weight model families.
///
/// Keyed on the canonical (lowercased, slash-stripped) model id so the same
/// family resolves consistently regardless of which gateway serves it and how
/// it spells version numbers (`glm-4.7`, `glm-47`, `glm-4p7`). Values reflect
/// each family's published context window; a live `/v1/models` catalog or an
/// explicit user `context_window` config overrides these upstream.
pub fn open_weight_family_context_limit(model: &str) -> Option<usize> {
    let m = model;

    // --- Z.AI GLM family ---
    if m.contains("glm") {
        // GLM-5.2: first GLM with a truly usable 1M-token context window.
        if m.contains("glm-5.2") || m.contains("glm-52") || m.contains("glm-5p2") {
            return Some(1_000_000);
        }
        // GLM-5 / GLM-5.1 and GLM-4.6 / GLM-4.7: 200K context.
        if m.contains("glm-5")
            || m.contains("glm-4.7")
            || m.contains("glm-47")
            || m.contains("glm-4p7")
            || m.contains("glm-4-7")
            || m.contains("glm-4.6")
            || m.contains("glm-46")
            || m.contains("glm-4p6")
        {
            return Some(200_000);
        }
        // GLM-4.5 and earlier GLM-4: 128K context.
        if m.contains("glm-4") {
            return Some(128_000);
        }
    }

    // --- DeepSeek (check V4 before V3 so the more specific match wins) ---
    if m.contains("deepseek-v4") {
        return Some(1_000_000);
    }
    if m.contains("deepseek-v3.2") || m.contains("deepseek-v3p2") || m.contains("deepseek-v3-2") {
        return Some(163_840);
    }
    if m.contains("deepseek-v3") {
        return Some(131_072);
    }

    // --- Moonshot Kimi K2 family: 256K context ---
    if m.contains("kimi") {
        return Some(262_144);
    }

    // --- MiniMax M2 family: 204,800 context ---
    if m.contains("minimax") {
        return Some(204_800);
    }

    // --- Xiaomi MiMo V2 family: 256K context ---
    if m.contains("mimo") {
        return Some(262_144);
    }

    // --- Alibaba GTE-Qwen2 retrieval models: 32K context ---
    if m.contains("gte-qwen") {
        return Some(32_768);
    }
    // --- Alibaba Qwen3 / Qwen3.5 family: 256K context ---
    if m.contains("qwen3") || m.contains("qwen-3") {
        return Some(262_144);
    }

    // --- OpenAI gpt-oss open weights: 131K context ---
    if m.contains("gpt-oss") {
        return Some(131_072);
    }

    // --- Meta Llama 3.x: 128K context ---
    if m.contains("llama-3") {
        return Some(131_072);
    }

    // --- Nous Hermes 4 (Llama-based): 128K context ---
    if m.contains("hermes-4") {
        return Some(131_072);
    }

    // --- Google Gemma 3: 128K context ---
    if m.contains("gemma-3") {
        return Some(131_072);
    }

    // --- Mistral small 3.x: 128K context ---
    if m.contains("mistral-small-3") {
        return Some(131_072);
    }

    // --- xAI grok-code-fast: 256K context ---
    if m.contains("grok-code-fast") {
        return Some(256_000);
    }

    // --- Perplexity Sonar: 128K context ---
    if m.contains("sonar") {
        return Some(128_000);
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
    fn quality_first_defaults_are_first_in_curated_model_orders() {
        assert_eq!(
            ALL_CLAUDE_MODELS.first().copied(),
            Some(DEFAULT_CLAUDE_MODEL)
        );
        assert_eq!(
            ALL_OPENAI_MODELS.first().copied(),
            Some(DEFAULT_OPENAI_MODEL)
        );
    }

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
    fn context_limit_classifies_claude_by_context_mode() {
        // Native-1M: 1M by default, suffix is a no-op.
        assert_eq!(
            context_limit_for_model_with_provider("claude-opus-4-8", Some("claude")),
            Some(1_000_000)
        );
        assert_eq!(
            context_limit_for_model_with_provider("claude-opus-4-8[1m]", Some("claude")),
            Some(1_000_000)
        );
        assert_eq!(
            context_limit_for_model_with_provider("claude-opus-4-7", Some("claude")),
            Some(1_000_000)
        );
        // Opt-in 1M: 200K by default, 1M only via the [1m] suffix.
        assert_eq!(
            context_limit_for_model_with_provider("claude-opus-4-6", Some("claude")),
            Some(200_000)
        );
        // Standard: 200K, even though the live catalog over-advertises 1M for it.
        assert_eq!(
            context_limit_for_model_with_provider("claude-sonnet-4-5", Some("claude")),
            Some(200_000)
        );
        assert_eq!(
            context_limit_for_model_with_provider("claude-opus-4-5", Some("claude")),
            Some(200_000)
        );
        assert_eq!(
            context_limit_for_model_with_provider("claude-haiku-4-5", Some("claude")),
            Some(200_000)
        );
    }

    #[test]
    fn context_limit_classifies_retired_fable_as_native_1m() {
        // `claude-fable-5` is a native-1M flagship. Even though Anthropic retired
        // its public id, sessions pinned to it must report 1M, not the 200K
        // default that would result from falling through the known-model gate.
        assert_eq!(
            context_limit_for_model_with_provider("claude-fable-5", Some("claude")),
            Some(1_000_000)
        );
        assert_eq!(
            context_limit_for_model_with_provider("claude-fable-5[1m]", Some("claude")),
            Some(1_000_000)
        );
    }

    #[test]
    fn anthropic_context_mode_classifications() {
        use crate::anthropic::{AnthropicContextMode, anthropic_context_mode};
        assert_eq!(
            anthropic_context_mode("claude-opus-4-8"),
            AnthropicContextMode::Native1M
        );
        assert_eq!(
            anthropic_context_mode("claude-opus-4-8[1m]"),
            AnthropicContextMode::Native1M
        );
        assert_eq!(
            anthropic_context_mode("claude-opus-4-7"),
            AnthropicContextMode::Native1M
        );
        assert_eq!(
            anthropic_context_mode("claude-opus-4-6"),
            AnthropicContextMode::OptIn1M
        );
        // Sonnet 5 is native 1M: 1M is both the default and the maximum
        // (issue #450).
        assert_eq!(
            anthropic_context_mode("claude-sonnet-5"),
            AnthropicContextMode::Native1M
        );
        assert_eq!(
            anthropic_context_mode("claude-sonnet-5-20260701"),
            AnthropicContextMode::Native1M
        );
        assert_eq!(
            anthropic_context_mode("claude-sonnet-4-6"),
            AnthropicContextMode::OptIn1M
        );
        assert_eq!(
            anthropic_context_mode("claude-sonnet-4-5"),
            AnthropicContextMode::Standard
        );
        assert_eq!(
            anthropic_context_mode("claude-opus-4-5"),
            AnthropicContextMode::Standard
        );

        // Only opt-in models surface a [1m] picker alias.
        assert!(!anthropic_context_mode("claude-opus-4-8").exposes_1m_alias());
        assert!(anthropic_context_mode("claude-opus-4-6").exposes_1m_alias());
        assert!(!anthropic_context_mode("claude-sonnet-4-5").exposes_1m_alias());
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

    #[test]
    fn classifies_api_only_pro_models() {
        assert!(is_openai_api_only_pro_model("gpt-5.5-pro"));
        assert!(is_openai_api_only_pro_model("gpt-5-pro"));
        assert!(is_openai_api_only_pro_model(" GPT-5.4-PRO "));
        // Dated snapshots of a pro model count too.
        assert!(is_openai_api_only_pro_model("gpt-5.5-pro-2026-04-23"));
        // Non-pro and near-miss ids do not.
        assert!(!is_openai_api_only_pro_model("gpt-5.5"));
        assert!(!is_openai_api_only_pro_model("gpt-5.6-sol"));
        assert!(!is_openai_api_only_pro_model(CHATGPT_WEB_MODEL));
        assert!(!is_openai_api_only_pro_model("gemini-2.5-pro"));
        // Every listed pro model classifies as pro.
        for pro in OPENAI_API_ONLY_PRO_MODELS {
            assert!(is_openai_api_only_pro_model(pro));
        }
    }
}
