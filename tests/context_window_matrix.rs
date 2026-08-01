//! Context-window resolution matrix.
//!
//! Companion to `provider_matrix.rs`. Where that suite sweeps *auth and
//! endpoint* state, this one sweeps the state space of **how a context window
//! gets resolved**, because that resolution is where a specific, recurring
//! class of bug lives.
//!
//! Why this deserves its own suite: an over-reported window is a silent
//! correctness bug, not a crash. jcode budgets prompts and triggers compaction
//! from `Provider::context_window()`. If that number is larger than what the
//! endpoint actually serves, the request is built too big, the server truncates
//! it, and the user sees a model that has "forgotten" the conversation while
//! the context gauge still looks healthy. Nothing fails loudly. The repo has
//! already paid for this several times:
//!
//! - #403: a session-routing `<profile>:<model>` prefix defeated the per-model
//!   lookup, so a 128K endpoint was budgeted at the provider default.
//! - #447: llama.cpp reports its serving window only under `meta.n_ctx`, so
//!   local models fell back to the generic 200K default.
//! - #541: a user-configured window for a GPT-named model lost to a
//!   model-family fallback.
//! - #577/#578: Claude ids fell through to 200K, and the live catalog
//!   over-advertised 1M for models that are actually 200K-capped.
//! - Ollama: `/v1/models` omits `context_length` entirely while the server caps
//!   at `OLLAMA_CONTEXT_LENGTH` (default 4096) and truncates silently.
//!
//! Every one of those is the same shape: one resolution source disagreed with
//! another, and the wrong one won. Per-provider unit tests each covered their
//! own case after the fact. This suite instead states the cross-cutting
//! invariants once, so the *next* provider added inherits the guardrail.
//!
//! Everything here is offline and hermetic: no network, no live catalog.

use jcode_provider_core::{
    DEFAULT_CONTEXT_LIMIT, context_limit_for_model_with_provider,
    context_limit_for_model_with_provider_and_cache,
};

/// A window big enough to matter but not plausibly a real model limit, used to
/// prove a configured/cached value actually wins rather than coincidentally
/// matching a fallback.
const SENTINEL_WINDOW: usize = 123_456;

/// Models whose windows are resolved by static/catalog knowledge across the
/// provider families jcode ships. Kept deliberately broad rather than exact:
/// the invariant under test is "resolution produces a sane, positive window",
/// not any single vendor's current number, so this does not churn on releases.
fn representative_models() -> Vec<(&'static str, Option<&'static str>)> {
    vec![
        ("claude-opus-4-5", Some("anthropic")),
        ("claude-sonnet-4-5", Some("anthropic")),
        ("gpt-5.3-codex-spark", Some("openai")),
        ("deepseek-v4-flash", Some("openrouter")),
        ("qwen3:0.6b", Some("openrouter")),
        ("some-model-nobody-has-heard-of", None),
    ]
}

/// Invariant: resolution never yields a nonsensical window.
///
/// A zero or absurd window is worse than a wrong one: it makes budgeting and
/// compaction arithmetic meaningless. This is the floor every other assertion
/// in this file stands on.
#[test]
fn resolved_windows_are_always_plausible() {
    for (model, provider) in representative_models() {
        let resolved =
            context_limit_for_model_with_provider(model, provider).unwrap_or(DEFAULT_CONTEXT_LIMIT);
        assert!(
            resolved >= 4096,
            "{model} via {provider:?} resolved an implausibly small window: {resolved}"
        );
        assert!(
            resolved <= 20_000_000,
            "{model} via {provider:?} resolved an implausibly large window: {resolved}"
        );
    }
}

/// Invariant: an explicitly configured or cached window beats every generic
/// fallback (regression shape of #541).
///
/// This is the single most important rule in the file. Configured/cached data
/// is endpoint-specific ground truth; family heuristics are guesses. When a
/// guess outranks ground truth, jcode over-budgets a real endpoint. Note the
/// model ids below deliberately look like well-known models, because that name
/// collision is exactly what caused #541.
#[test]
fn configured_window_wins_over_family_fallbacks() {
    let deceptive_ids = [
        "gpt-4o",
        "gpt-5.3-codex-spark",
        "deepseek-v4-flash",
        "qwen3.6-35b-a2000-128k",
        "llama3.2",
    ];

    for model in deceptive_ids {
        let resolved = context_limit_for_model_with_provider_and_cache(model, None, |candidate| {
            (candidate == model).then_some(SENTINEL_WINDOW)
        });
        assert_eq!(
            resolved,
            Some(SENTINEL_WINDOW),
            "configured window for {model} lost to a generic fallback, which is how #541 \
             over-budgeted a custom endpoint serving a GPT-named model"
        );
    }
}

/// Invariant: resolution is deterministic.
///
/// Context windows feed both the displayed gauge and the compaction trigger. If
/// those two reads disagree, the meter and the budget drift apart (the class of
/// confusion tracked in #441).
#[test]
fn resolution_is_deterministic() {
    for (model, provider) in representative_models() {
        let first = context_limit_for_model_with_provider(model, provider);
        for _ in 0..3 {
            assert_eq!(
                context_limit_for_model_with_provider(model, provider),
                first,
                "{model} via {provider:?} resolved a different window on a repeat read"
            );
        }
    }
}

/// Invariant: a provider hint never *silently* widens a model's window.
///
/// Routing the same model through a different provider hint may legitimately
/// narrow the window (a gateway can serve less than the trained maximum, which
/// is precisely the Ollama and llama.cpp case). Widening is the dangerous
/// direction, so require that any widening be a deliberate, known case rather
/// than an accident of hint parsing.
#[test]
fn provider_hints_do_not_silently_widen_windows() {
    // Copilot deliberately re-declares windows for models it re-serves, so it
    // is an explicit, reviewed exception rather than an accidental widening.
    let hints = [Some("openrouter"), Some("anthropic"), Some("openai"), None];

    for (model, _) in representative_models() {
        let baseline = context_limit_for_model_with_provider(model, None);
        let Some(baseline) = baseline else {
            continue;
        };
        for hint in hints {
            let Some(resolved) = context_limit_for_model_with_provider(model, hint) else {
                continue;
            };
            assert!(
                resolved <= baseline.max(DEFAULT_CONTEXT_LIMIT),
                "{model} via {hint:?} widened its window to {resolved} above the \
                 unhinted {baseline}; widening risks over-budgeting a real endpoint"
            );
        }
    }
}

/// Invariant: an unknown model falls back to a defined default rather than
/// producing an unbounded or zero budget.
///
/// New model ids appear constantly. The failure mode to prevent is a brand-new
/// id silently inheriting something enormous.
#[test]
fn unknown_models_fall_back_to_the_declared_default() {
    let unknown = "totally-unreleased-model-2099-ultra";
    let resolved =
        context_limit_for_model_with_provider(unknown, None).unwrap_or(DEFAULT_CONTEXT_LIMIT);
    assert_eq!(
        resolved, DEFAULT_CONTEXT_LIMIT,
        "unknown models must land on the declared default so budgeting stays bounded"
    );
}

/// Invariant: the cache callback is consulted for the id actually being
/// resolved.
///
/// #403 was exactly this: the runtime model carried a `<profile>:<model>`
/// routing prefix, the per-model lookup missed, and the provider default won.
/// A cache probe keyed on an unrelated id must not leak into the result.
#[test]
fn cache_lookups_do_not_match_unrelated_model_ids() {
    let resolved =
        context_limit_for_model_with_provider_and_cache("qwen3:0.6b", None, |candidate| {
            (candidate == "a-completely-different-model").then_some(SENTINEL_WINDOW)
        });
    assert_ne!(
        resolved,
        Some(SENTINEL_WINDOW),
        "a cache entry for an unrelated model id leaked into resolution"
    );
}
