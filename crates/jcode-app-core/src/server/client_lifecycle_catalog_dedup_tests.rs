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
        catalog_metadata: None,
        model: "claude-opus-4.6".to_string(),
        provider: "OpenRouter".to_string(),
        api_method: "openrouter".to_string(),
        available: true,
        detail: detail.to_string(),
        usage: None,
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

#[test]
fn age_normalization_handles_long_non_age_digit_runs() {
    let digits = "1234567890".repeat(10_000);
    assert_eq!(strip_relative_age_text(&digits), digits);
    let text = format!("{digits}ms p50 · 2m ago");
    assert_eq!(
        strip_relative_age_text(&text),
        format!("{digits}ms p50 · <age>")
    );
}

#[test]
fn catalog_dedup_keeps_model_and_auth_route_changes() {
    let baseline = ServerEvent::AvailableModelsUpdated {
        provider_name: Some("test 12m ago".into()),
        provider_model: Some("model 12m ago".into()),
        available_models: vec!["model 12m ago".into()],
        available_model_routes: vec![crate::provider::ModelRoute {
            catalog_metadata: None,
            model: "model 12m ago".into(),
            provider: "test 12m ago".into(),
            api_method: "oauth".into(),
            available: true,
            detail: "cached 12m ago".into(),
            usage: None,
            cheapness: None,
        }],
    };
    let key = available_models_dedup_key(&baseline);
    // Every semantic catalog field must invalidate dedup immediately, with no
    // time-based caching that could mask login/logout or a model switch.
    for field in 0..9 {
        let mut changed = baseline.clone();
        let ServerEvent::AvailableModelsUpdated {
            provider_name,
            provider_model,
            available_models,
            available_model_routes,
        } = &mut changed
        else {
            unreachable!()
        };
        let route = &mut available_model_routes[0];
        match field {
            0 => *provider_name = Some("test 41m ago".into()),
            1 => *provider_model = Some("model 41m ago".into()),
            2 => available_models[0] = "model 41m ago".into(),
            3 => route.model = "model 41m ago".into(),
            4 => route.provider = "test 41m ago".into(),
            5 => route.api_method = "api-key".into(),
            6 => route.available = false,
            7 => route.usage = Some(Default::default()),
            8 => {
                route.cheapness = Some(crate::provider::RouteCheapnessEstimate::metered(
                    crate::provider::RouteCostSource::PublicApiPricing,
                    crate::provider::RouteCostConfidence::Exact,
                    300_000,
                    2_500_000,
                    None,
                    None,
                ));
            }
            _ => unreachable!(),
        }
        assert_ne!(key, available_models_dedup_key(&changed), "field {field}");
    }
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
