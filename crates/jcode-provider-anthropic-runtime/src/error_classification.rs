//! Classify Anthropic failures and choose safe model fallbacks.

use super::*;

/// Account-local limits can be escaped by selecting another OAuth pool member.
/// Do not classify model/transport overload or arbitrary server failures here.
pub(super) fn is_account_limit_error(error: &str) -> bool {
    [
        "429 too many requests",
        "rate limit",
        "rate_limit",
        "quota",
        "usage exhausted",
        "usage limit",
        "limit reached",
        "billing",
        "credit balance",
        "payment required",
    ]
    .iter()
    .any(|needle| error.contains(needle))
}

/// Check if an error is transient and should be retried
pub(super) fn is_retryable_error(error_str: &str) -> bool {
    jcode_provider_core::is_transient_transport_error(error_str)
        // Server errors (5xx)
        || error_str.contains("500 internal server error")
        || error_str.contains("502 bad gateway")
        || error_str.contains("503 service unavailable")
        || error_str.contains("504 gateway timeout")
        || error_str.contains("overloaded")
        // Rate limiting (429)
        || error_str.contains("429 too many requests")
        || error_str.contains("rate limit")
        || error_str.contains("rate_limit")
        // API-level server errors (SSE error events)
        || error_str.contains("api_error")
        || error_str.contains("internal server error")
}

pub(super) fn is_fable_scoped_limit_error(model: &str, error: &str) -> bool {
    let model = strip_1m_suffix(model).to_ascii_lowercase();
    if !model.contains("fable") {
        return false;
    }
    let error = error.to_ascii_lowercase();
    let is_limit = error.contains("rate_limit")
        || error.contains("rate limit")
        || error.contains("usage_limit")
        || error.contains("usage limit");
    let is_scoped = error.contains("fable")
        || error.contains("weekly")
        || error.contains("week limit")
        || error.contains("7-day")
        || error.contains("7 day");
    is_limit && is_scoped
}

/// Detect an Anthropic "model not found" rejection.
///
/// Anthropic returns HTTP 404 with `"type":"not_found_error"` when a model id
/// has been retired or renamed (e.g. `claude-fable-5` after it was folded into
/// Opus 4.8). The message text varies, so match on the stable structural
/// markers. `error_str` is expected to already be lowercased.
pub(super) fn is_model_not_found_error(error_str: &str) -> bool {
    let mentions_model = error_str.contains("model");
    let is_not_found = error_str.contains("not_found_error")
        || error_str.contains("404 not found")
        || error_str.contains("(404 ");
    is_not_found
        && (mentions_model
            || error_str.contains("is not available")
            || error_str.contains("please use"))
}

/// Detect an Anthropic rejection of an explicit reasoning request.
///
/// Some models are listed with reasoning capabilities in `GET /v1/models` but
/// the live Messages API still rejects them with a 400 `invalid_request_error`:
///
/// - an adaptive `thinking` block -> "adaptive thinking is not supported on
///   this model"
/// - an `output_config` effort -> "This model does not support the effort
///   parameter."
///
/// (e.g. `claude-fable-5`). When we hit either we can self-heal by dropping the
/// offending reasoning fields and retrying, rather than hard-failing the turn.
/// `error_str` is expected to already be lowercased.
pub(super) fn is_reasoning_unsupported_error(error_str: &str) -> bool {
    let is_bad_request =
        error_str.contains("invalid_request_error") || error_str.contains("400 bad request");
    let mentions_reasoning_field = error_str.contains("thinking")
        || error_str.contains("effort")
        || error_str.contains("output_config");
    let mentions_unsupported =
        error_str.contains("not supported") || error_str.contains("does not support");
    is_bad_request && mentions_reasoning_field && mentions_unsupported
}

/// Models that have been retired and must never be chosen as a fallback target
/// (the server 404s them, so picking one just loops). Matched as a substring of
/// the normalized id so dated variants are covered too. `claude-fable` was
/// briefly on this list while retired, but the model is live again.
const RETIRED_ANTHROPIC_MODEL_MARKERS: &[&str] = &["claude-mythos"];

pub(super) fn anthropic_model_is_retired(model: &str) -> bool {
    let normalized = AnthropicProvider::normalized_model_key(model);
    RETIRED_ANTHROPIC_MODEL_MARKERS
        .iter()
        .any(|marker| normalized.contains(marker))
}

/// Quality rank for an Anthropic model id: lower is better. Uses the curated
/// flagship-first `ALL_CLAUDE_MODELS` order (Opus > Sonnet > Haiku > older), so
/// fallback never silently downgrades to a cheaper tier when a stronger model is
/// available. Unknown/uncurated ids sort after every curated one but before
/// retired models, which sort last.
pub(super) fn anthropic_model_quality_rank(model: &str) -> usize {
    if anthropic_model_is_retired(model) {
        return usize::MAX;
    }
    let normalized = jcode_provider_core::model_id::strip_date_suffix(
        &jcode_provider_core::model_id::canonical(model),
    )
    .to_string();
    jcode_provider_core::ALL_CLAUDE_MODELS
        .iter()
        .position(|candidate| {
            jcode_provider_core::model_id::strip_date_suffix(
                &jcode_provider_core::model_id::canonical(candidate),
            ) == normalized
        })
        // Curated models keep their position; unknown-but-not-retired models sort
        // just after the curated list so they only win when nothing curated is
        // available.
        .unwrap_or(jcode_provider_core::ALL_CLAUDE_MODELS.len())
}

/// Parse a server-recommended replacement model from a 404 body, e.g.
/// "Claude Fable 5 is not available. Please use Opus 4.8." -> the catalog id
/// `claude-opus-4-8`. Returns the best matching known catalog id, if any.
/// `error_str` is expected to already be lowercased.
pub(super) fn anthropic_recommended_model_from_error(error_str: &str) -> Option<String> {
    // Look for the phrase after "please use" / "use " and try to match it against
    // the known catalog by collapsing it to a comparable token form. The server
    // phrases the recommendation in prose ("Opus 4.8"), so compare on the digits
    // and family word rather than exact ids.
    let hint = error_str
        .split("please use")
        .nth(1)
        .or_else(|| error_str.split("use ").nth(1))?;
    // A decimal point inside a model version is not a sentence boundary.
    // Truncating "Opus 4.8" to "Opus 4" selected an arbitrary older release
    // whenever catalog ordering changed.
    let bytes = hint.as_bytes();
    let end = hint.char_indices().find_map(|(i, c)| {
        let version_dot = c == '.'
            && i > 0
            && bytes[i - 1].is_ascii_digit()
            && bytes.get(i + 1).is_some_and(u8::is_ascii_digit);
        (matches!(c, '.' | '!' | '\n') && !version_dot).then_some(i)
    });
    let hint = hint[..end.unwrap_or(hint.len())].trim();
    if hint.is_empty() {
        return None;
    }
    // Reduce the hint to alphanumeric tokens (e.g. "opus", "4", "8").
    let hint_tokens: Vec<String> = hint
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_ascii_lowercase())
        .collect();
    if hint_tokens.is_empty() {
        return None;
    }
    // Score each known catalog model by how many hint tokens it contains.
    jcode_base::provider::known_anthropic_model_ids()
        .into_iter()
        .filter(|candidate| !anthropic_model_is_retired(candidate))
        .map(|candidate| {
            let key = AnthropicProvider::normalized_model_key(&candidate);
            // The catalog id uses hyphenated digits ("claude-opus-4-8"), so the
            // hint tokens ["opus","4","8"] should all appear.
            let mut candidate_tokens: Vec<&str> = key.split('-').collect();
            let score = hint_tokens
                .iter()
                .filter(|token| {
                    if let Some(index) = candidate_tokens
                        .iter()
                        .position(|part| *part == token.as_str())
                    {
                        candidate_tokens.remove(index);
                        true
                    } else {
                        false
                    }
                })
                .count();
            (candidate, score)
        })
        // Require at least the family word plus one version digit to match so we
        // do not pick an arbitrary model from a single shared token.
        .filter(|(_, score)| *score >= 2)
        .max_by_key(|(_, score)| *score)
        .map(|(candidate, _)| candidate)
}

/// Pick the next Anthropic model to try after a "model not found" failure.
///
/// Strategy (most authoritative first):
///   1. Honor any server "Please use X" recommendation parsed from the error.
///   2. Otherwise pick the highest-quality untried model from the curated
///      flagship-first catalog, skipping retired families so we never downgrade
///      to a cheaper tier (e.g. Haiku) while a stronger model is available.
///
/// Returns `None` once every viable candidate is exhausted so the caller can
/// surface the original error.
pub(super) fn anthropic_fallback_model(tried: &[String], error_str: &str) -> Option<String> {
    let already_tried = |candidate: &str| {
        tried.iter().any(|model| {
            AnthropicProvider::normalized_model_key(model)
                == AnthropicProvider::normalized_model_key(candidate)
        })
    };

    // 1. Server recommendation wins when it points at an untried, non-retired
    //    model.
    if let Some(recommended) = anthropic_recommended_model_from_error(error_str)
        && !already_tried(&recommended)
        && !anthropic_model_is_retired(&recommended)
    {
        return Some(recommended);
    }

    // 2. Best available by curated quality order, skipping retired and tried.
    jcode_base::provider::known_anthropic_model_ids()
        .into_iter()
        .filter(|candidate| !already_tried(candidate) && !anthropic_model_is_retired(candidate))
        .min_by_key(|candidate| anthropic_model_quality_rank(candidate))
}

pub(super) fn is_oauth_auth_error(error_str: &str) -> bool {
    error_str.contains("oauth token has expired")
        || error_str.contains("token has expired")
        || error_str.contains("authentication_error")
        || error_str.contains("invalid token")
        || error_str.contains("invalid_grant")
        || error_str.contains("does not meet scope requirement")
        || ((error_str.contains("401 unauthorized") || error_str.contains("403 forbidden"))
            && (error_str.contains("oauth") || error_str.contains("token")))
}

pub(super) fn is_oauth_catalog_auth_error(error_str: &str) -> bool {
    let lower = error_str.to_ascii_lowercase();
    lower.contains("401 unauthorized")
        || lower.contains("403 forbidden")
        || is_oauth_auth_error(&lower)
}
