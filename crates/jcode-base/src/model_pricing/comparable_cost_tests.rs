//! Ordering routes by price across currencies.
//!
//! `estimated_reference_cost_micros` is only meaningful inside one currency: a
//! hand-written `[pricing.providers]` card stamps its own currency onto the
//! estimate while the models.dev catalog behind it publishes USD, and the model
//! picker orders both with a single number. The decisive case is a pair whose
//! raw order and converted order disagree, which is what these tests pin.

use super::*;
use jcode_provider_core::{RouteCostConfidence, RouteCostSource};

/// Run `f` with `toml` as the user's `config.toml` in an isolated home.
fn with_config<T>(config_toml: &str, f: impl FnOnce() -> T) -> T {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());
    std::fs::write(temp.path().join("config.toml"), config_toml).expect("write config.toml");
    crate::config::invalidate_config_cache();

    let out = f();

    crate::config::invalidate_config_cache();
    if let Some(prev) = prev_home {
        crate::env::set_var("JCODE_HOME", prev);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
    out
}

/// An estimate whose reference cost is `reference_micros` in `currency`.
///
/// The reference request is `CHEAPNESS_REFERENCE_INPUT_TOKENS` input tokens, so
/// the per-Mtok rate yielding a given reference cost is
/// `reference_micros * 1_000_000 / tokens`.
fn estimate_with_reference_cost(
    reference_micros: u64,
    currency: Currency,
) -> RouteCheapnessEstimate {
    let input_per_mtok_micros = reference_micros * 1_000_000 / CHEAPNESS_REFERENCE_INPUT_TOKENS;
    RouteCheapnessEstimate::metered(
        RouteCostSource::ConfigPriceSheet,
        RouteCostConfidence::Exact,
        input_per_mtok_micros,
        0,
        None,
        Option::<String>::None,
    )
    .with_currency(currency)
}

const CNY_PER_USD: &str = r#"
[pricing]
fx_base = "USD"

[pricing.fx_rates]
CNY = 7.2
"#;

/// The case that makes this necessary: 7_200 CNY micros against 1_500 USD micros.
/// The raw numbers put the USD route first, but 7_200 CNY is 1_000 USD micros, so
/// the CNY card is actually cheaper. Ordering on the raw estimate gets this
/// backwards, and a CNY card sitting next to USD catalog entries is exactly what
/// a hand-written `[pricing.providers]` card produces.
#[test]
fn conversion_can_reverse_the_raw_order() {
    with_config(CNY_PER_USD, || {
        let cny = estimate_with_reference_cost(7_200, Currency::new("CNY"));
        let usd = estimate_with_reference_cost(1_500, Currency::usd());

        assert!(
            cny.estimated_reference_cost_micros > usd.estimated_reference_cost_micros,
            "the raw estimates must disagree with the converted ones for this test to mean anything"
        );

        assert_eq!(comparable_reference_cost_micros(&cny), Some(1_000));
        assert_eq!(comparable_reference_cost_micros(&usd), Some(1_500));
        assert!(
            comparable_reference_cost_micros(&cny) < comparable_reference_cost_micros(&usd),
            "the CNY card is the cheaper route once both are in one currency"
        );
    });
}

/// A currency the table cannot convert must report "cannot say" rather than be
/// compared as if the units matched; the picker sorts those below every priced
/// route.
#[test]
fn a_currency_with_no_rate_is_not_converted() {
    with_config(CNY_PER_USD, || {
        let jpy = estimate_with_reference_cost(7_200, Currency::new("JPY"));
        assert_eq!(
            comparable_reference_cost_micros(&jpy),
            None,
            "JPY has no configured rate, so it must not be ordered against USD"
        );

        // The base currency needs no entry of its own.
        let usd = estimate_with_reference_cost(1_500, Currency::usd());
        assert_eq!(comparable_reference_cost_micros(&usd), Some(1_500));
    });
}

/// An unpriced route has nothing to order by.
#[test]
fn an_estimate_without_a_reference_cost_has_no_order() {
    with_config(CNY_PER_USD, || {
        let mut estimate = estimate_with_reference_cost(7_200, Currency::new("CNY"));
        estimate.estimated_reference_cost_micros = None;
        assert_eq!(comparable_reference_cost_micros(&estimate), None);
    });
}
