//! Currency defaults for route price estimates.
//!
//! Split from the inline `tests` module in `lib.rs` (rather than kept there)
//! because `lib.rs` is already above the repository's code-size budget: the
//! size ratchet tracks production files and excludes `*_tests.rs`.

use crate::{Currency, RouteCheapnessEstimate, RouteCostConfidence, RouteCostSource};

#[test]
fn unconfigured_provider_currency_is_usd() {
    // Every construction path that does not state a currency is USD, and a
    // payload written before the field existed still deserializes.
    let estimate = RouteCheapnessEstimate::metered(
        RouteCostSource::ModelsDevCatalog,
        RouteCostConfidence::High,
        660_000,
        1_980_000,
        None,
        None,
    );
    assert!(estimate.currency.is_usd(), "default currency is USD");

    let legacy: RouteCheapnessEstimate = serde_json::from_value(serde_json::json!({
        "billing_kind": "metered",
        "source": "models_dev_catalog",
        "confidence": "high",
        "input_price_per_mtok_micros": 660_000,
        "output_price_per_mtok_micros": 1_980_000,
        "reference_input_tokens": 25_000,
        "reference_output_tokens": 5_000
    }))
    .expect("legacy payload without a currency");
    assert!(legacy.currency.is_usd(), "absent field defaults to USD");

    let priced = RouteCheapnessEstimate::metered(
        RouteCostSource::ConfigPriceSheet,
        RouteCostConfidence::Exact,
        4_500_000,
        13_500_000,
        None,
        None,
    )
    .with_currency(Currency::new("cny"));
    assert_eq!(priced.currency.as_str(), "CNY");
}
