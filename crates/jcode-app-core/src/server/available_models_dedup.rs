//! Dedup key for `AvailableModelsUpdated` catalog broadcasts.
//!
//! Split out of `client_lifecycle.rs` to keep that file off the code-size
//! ratchet's growth list.

use crate::protocol::ServerEvent;

/// Dedup key for `AvailableModelsUpdated` that ignores cosmetic relative-age
/// text in route details.
///
/// Route details embed human-readable cache ages ("12m ago", "3h ago"). Those
/// strings tick forward on their own, so a byte-comparison of encoded events
/// reports a "change" every time an age bucket rolls over even though no model
/// or route actually changed. That made otherwise-identical catalogs fan out to
/// every connected client, and each client then invalidated its picker cache,
/// rewrote its on-disk catalog cache, and repainted a full frame, which starved
/// the input line.
///
/// Normalizing those substrings out means a catalog whose only difference is
/// elapsed time is treated as unchanged and never broadcast. Real changes
/// (models, providers, api methods, availability, non-age detail text) still
/// differ and still propagate.
pub(super) fn available_models_dedup_key(event: &ServerEvent) -> String {
    let ServerEvent::AvailableModelsUpdated {
        provider_name,
        provider_model,
        available_models,
        available_model_routes,
    } = event
    else {
        return crate::protocol::encode_event(event);
    };
    // Only route details contain cosmetic ages. Scanning the encoded catalog
    // wastes work and can hide real changes to model IDs or provider names.
    let routes: Vec<_> = available_model_routes
        .iter()
        .map(|route| {
            // Exhaustive destructuring makes additions to ModelRoute require a
            // deliberate dedup decision rather than silently hiding changes.
            let crate::provider::ModelRoute {
                model,
                provider,
                api_method,
                available,
                detail,
                catalog_metadata,
                usage,
                cheapness,
            } = route;
            (
                model,
                provider,
                api_method,
                available,
                strip_relative_age_text(detail),
                catalog_metadata,
                usage,
                cheapness,
            )
        })
        .collect();
    serde_json::to_string(&(provider_name, provider_model, available_models, routes))
        .unwrap_or_else(|_| crate::protocol::encode_event(event))
}

/// Replace `<digits><unit> ago` runs (e.g. `12m ago`, `3h ago`, `5d ago`) with a
/// fixed placeholder so relative-age drift does not register as a change.
pub(super) fn strip_relative_age_text(text: &str) -> String {
    const AGE_SUFFIX: &[u8] = b" ago";
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while let Some(offset) = bytes[i..].iter().position(u8::is_ascii_digit) {
        let start = i + offset;
        let mut end = start + 1;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
        let unit_is_age = matches!(bytes.get(end), Some(b's' | b'm' | b'h' | b'd'));
        if unit_is_age && bytes[end + 1..].starts_with(AGE_SUFFIX) {
            // ASCII digits are always UTF-8 boundaries. Copy unchanged spans
            // in bulk, not one Unicode character at a time across the catalog.
            out.push_str(&text[copied..start]);
            out.push_str("<age>");
            copied = end + 1 + AGE_SUFFIX.len();
            i = copied;
        } else {
            // Consume each digit run once, avoiding quadratic suffix scans.
            i = end;
        }
    }
    out.push_str(&text[copied..]);
    out
}

#[cfg(test)]
#[path = "client_lifecycle_catalog_dedup_tests.rs"]
mod tests;
