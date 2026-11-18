use super::*;

pub(super) fn rank_newest_released_model(
    profile_id: &str,
    resolved: &ResolvedOpenAiCompatibleProfile,
    cache: jcode_provider_openrouter::DiskCache,
) -> Option<String> {
    let source_matches = cache
        .source_api_base
        .as_deref()
        .and_then(normalize_api_base)
        == normalize_api_base(&resolved.api_base);
    if !source_matches {
        return None;
    }

    // Proxies with a curated flagship order (OpenRouter carries Claude/GPT
    // under vendor prefixes) rank by quality first. Plain recency picks
    // whatever shipped last; live, that put OpenRouter on
    // `mistralai/mistral-large-4-0` over `anthropic/claude-opus-5.5`.
    let chat_ids: Vec<&str> = cache
        .models
        .iter()
        .map(|model| model.id.trim())
        .filter(|id| !id.is_empty() && crate::provider::is_listable_model_name(id))
        .collect();
    if let Some(flagship) =
        crate::auth::lifecycle::ranked_flagship_for_provider(profile_id, &chat_ids)
    {
        return Some(flagship);
    }

    cache
        .models
        .into_iter()
        .enumerate()
        .filter_map(|(index, model)| {
            let id = model.id.trim().to_string();
            let created = model.created?;
            // Never auto-select an obviously non-chat model (TTS/speech/embeddings/
            // rerankers/image/etc.) as a profile's default. Catalogs like Groq,
            // NVIDIA NIM, and Chutes expose their entire model list, and the
            // newest-released entry is frequently a non-chat model (e.g. Groq's
            // `canopylabs/orpheus-*` TTS), which must not become the chat default.
            if id.is_empty() || !crate::provider::is_listable_model_name(&id) {
                return None;
            }
            let tier = openai_compatible_model_quality_tier(&id);
            Some((tier, created, std::cmp::Reverse(index), id))
        })
        // Pick the strongest tier first, then the newest within that tier, then
        // catalog order. Ranking by quality *before* recency stops a brand-new
        // cheap/small model (e.g. a freshly released `*-flash`/`*-mini`) from
        // becoming the default of a heterogeneous proxy catalog (OpenCode Zen,
        // Groq, ...) when a stronger sibling is available.
        .max_by_key(|(tier, created, reverse_index, _)| (*tier, *created, *reverse_index))
        .map(|(_, _, _, id)| id)
}

/// Cheap/small/fast tier markers. A model id whose tokens include one of these
/// is a cheaper or smaller variant that must not become a profile's default
/// while a stronger sibling exists. Matched on whole tokens (split on
/// non-alphanumeric) so brand names like `minimax` are not mistaken for
/// `mini`/`max`.
const OPENAI_COMPAT_CHEAP_TIER_MARKERS: &[&str] = &[
    "mini",
    "nano",
    "lite",
    "small",
    "tiny",
    "flash",
    "instant",
    "air",
    "micro",
    "lightning",
    "haiku",
    "scout",
    "edge",
    "lowcost",
];

/// Flagship/strong tier markers. A model id whose tokens include one of these
/// advertises itself as a provider's top-tier or specialized-strong model.
const OPENAI_COMPAT_FLAGSHIP_TIER_MARKERS: &[&str] = &[
    "opus", "max", "ultra", "pro", "plus", "large", "coder", "code", "reasoner", "405b", "480b",
    "235b", "671b", "256b",
];

/// Coarse cross-vendor quality tier for an openai-compatible catalog model id
/// (higher is stronger): `2` = flagship-marked, `0` = cheap/small-marked, `1`
/// otherwise (a bare frontier id like `gpt-5.5` or `minimax-m2.7`).
///
/// This relies on the near-universal naming convention that cheaper variants
/// carry a size/speed marker (`mini`, `flash`, `air`, ...) and flagship variants
/// carry a top-tier marker (`max`, `pro`, `opus`, `coder`, ...). It is a coarse
/// heuristic only used to break the "newest model" tie sensibly; the user can
/// always override the selected model.
pub(super) fn openai_compatible_model_quality_tier(model_id: &str) -> u8 {
    let lower = model_id.to_ascii_lowercase();
    let tokens: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    let has = |markers: &[&str]| markers.iter().any(|marker| tokens.contains(marker));
    // Flagship markers win when both are present (e.g. `qwen3-coder-30b`): a
    // strong-specialization signal outweighs a size token.
    if has(OPENAI_COMPAT_FLAGSHIP_TIER_MARKERS) {
        return 2;
    }
    if has(OPENAI_COMPAT_CHEAP_TIER_MARKERS) {
        return 0;
    }
    1
}
