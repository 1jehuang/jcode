//! Tests for the `AvailableModelsUpdated` dedup key.
//!
//! Split out of `client_lifecycle_tests.rs` to keep that file under the
//! test-size ratchet.

use super::{available_models_dedup_key, strip_relative_age_text};
use crate::protocol::ServerEvent;

/// Relative cache ages ("12m ago") drift on their own. If they reach the
/// catalog dedup key, every age rollover looks like a real catalog change and
/// fans a full repaint out to every connected client, which starves the TUI
/// input line.
#[test]
fn relative_age_text_is_normalized_out_of_the_catalog_dedup_key() {
    assert_eq!(
        strip_relative_age_text("openrouter · cached 12m ago · $3/M"),
        "openrouter · cached <age> · $3/M"
    );
    assert_eq!(
        strip_relative_age_text("a 5s ago b 3h ago c 2d ago"),
        "a <age> b <age> c <age>"
    );
}

#[test]
fn catalog_dedup_key_ignores_age_drift_but_keeps_real_changes() {
    let route = |detail: &str| crate::provider::ModelRoute {
        model: "claude-opus-4.6".to_string(),
        provider: "OpenRouter".to_string(),
        api_method: "openrouter".to_string(),
        available: true,
        detail: detail.to_string(),
        cheapness: None,
    };
    let event = |detail: &str| ServerEvent::AvailableModelsUpdated {
        provider_name: Some("OpenRouter".to_string()),
        provider_model: Some("claude-opus-4.6".to_string()),
        available_models: vec!["claude-opus-4.6".to_string()],
        available_model_routes: vec![route(detail)],
    };

    // Only the cosmetic age moved: same catalog, must dedup.
    assert_eq!(
        available_models_dedup_key(&event("cached 12m ago")),
        available_models_dedup_key(&event("cached 41m ago")),
    );
    // A genuine detail change must still be visible.
    assert_ne!(
        available_models_dedup_key(&event("cached 12m ago")),
        available_models_dedup_key(&event("rate limited, cached 12m ago")),
    );
}

/// A trailing digit run with no unit/suffix must not index past the string.
#[test]
fn age_normalization_handles_trailing_digits_and_unicode() {
    assert_eq!(strip_relative_age_text("tokens 12345"), "tokens 12345");
    assert_eq!(strip_relative_age_text("9"), "9");
    assert_eq!(strip_relative_age_text("9m"), "9m");
    assert_eq!(
        strip_relative_age_text("→ · émoji 7h ago"),
        "→ · émoji <age>"
    );
}

/// Real route detail strings captured from a production OpenRouter catalog
/// cache. These carry both self-ticking cache ages and endpoint stats
/// (`p50`, `tps`) that only move when the endpoint cache genuinely refreshes.
/// Ages must normalize away; stats must not, since a stats change means the
/// upstream data really did change and clients should see it.
#[test]
fn production_route_details_normalize_ages_but_preserve_endpoint_stats() {
    let with_age = "in $0.30/M, out $2.50/M, cache write $0.08/M, cache read $0.03/M, 100%, \
                    493ms p50, 143tps, cache on, 17m ago";
    let later_age = "in $0.30/M, out $2.50/M, cache write $0.08/M, cache read $0.03/M, 100%, \
                     493ms p50, 143tps, cache on, 56m ago";
    let changed_stats = "in $0.30/M, out $2.50/M, cache write $0.08/M, cache read $0.03/M, 100%, \
                         900ms p50, 143tps, cache on, 17m ago";

    // Age drift alone must collapse to the same key.
    assert_eq!(
        strip_relative_age_text(with_age),
        strip_relative_age_text(later_age),
        "cache-age drift must not look like a catalog change"
    );
    // A real endpoint-stat change must survive normalization.
    assert_ne!(
        strip_relative_age_text(with_age),
        strip_relative_age_text(changed_stats),
        "endpoint stat changes are real and must still propagate"
    );
    // `143tps` has no " ago" suffix, so it must be left intact.
    assert!(
        strip_relative_age_text(with_age).contains("143tps"),
        "bare unit-suffixed numbers must not be mistaken for ages"
    );
    assert!(strip_relative_age_text(with_age).ends_with("cache on, <age>"));
}
