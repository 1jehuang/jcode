//! Conifer route-specific context observations.
//!
//! These are exact observations, not model-family guarantees. Live or persisted
//! catalog metadata takes precedence at runtime. See `docs/CONIFER_PROVIDER.md`.

pub(crate) fn context_limit(model: &str) -> Option<usize> {
    Some(match model {
        "grok-4.6" | "grok-4.5" => 500_000,
        "grok-4.3" => 1_000_000,
        "seed-2.0-pro" | "seed-2.0-code" | "seed-2.0-mini" => 256_000,
        "step-3.7-flash" | "step-3.7-flash-novita" => 262_144,
        "hy3" | "hy3-tencent" | "hy3-novita" => 262_144,
        "ling-3.0-flash" => 131_072,
        "inkling" | "inkling-small" => 524_288,
        "nemotron-3-ultra" | "nemotron-3-super-120b" | "nemotron-3.5-lightning" => 262_144,
        "mistral-large-latest" | "mistral-medium-latest" | "mistral-small-latest" => 256_000,
        "command-a-cohere" => 256_000,
        "llama-4-maverick" => 1_048_576,
        "llama-4-scout" => 327_680,
        "gemma-4-31b" => 128_000,
        _ => return None,
    })
}
