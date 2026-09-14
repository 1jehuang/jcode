//! The billing-facing rate lookup: a complete card is priced in its own
//! currency, and a card that cannot price the call is reported as such instead
//! of being topped up from the next layer (spec 4.4).
//!
//! Kept out of `call_rates.rs` and the already-large `mod.rs` test module: the
//! size ratchet tracks production files only, and this file is excluded.

use super::call_rates::{ConfigCallRates, config_call_rates};
use super::{ModelCost, RuleOutOfEffect, clear_memory_cache_for_tests, save_test_cache};
use jcode_provider_core::Currency;
use std::time::SystemTime;

/// DeepSeek's peak windows: peak is UTC 01:00-04:00 on weekdays, 10x off-peak.
const PEAK_CARD_CONFIG: &str = r#"
[pricing.providers.deepseek.models."deepseek-v4-pro"]
default_tariff = "off_peak"

[pricing.providers.deepseek.models."deepseek-v4-pro".cost]
input = 1.0
output = 2.0

[pricing.providers.deepseek.models."deepseek-v4-pro".tariffs.off_peak]
input = 1.0
output = 2.0

[pricing.providers.deepseek.models."deepseek-v4-pro".tariffs.peak]
multiplier = 10.0

[[pricing.providers.deepseek.models."deepseek-v4-pro".schedule]]
tariff = "peak"
utc_offset_minutes = 0
weekdays = ["Mon", "Tue", "Wed", "Thu", "Fri"]
windows = [["01:00", "04:00"]]
"#;

/// A CNY card with only an `input` rate, and nothing under it: no field-level
/// merge is possible without relabelling USD numbers as CNY (F1).
const HALF_WRITTEN_CNY_CARD_CONFIG: &str = r#"
[pricing.providers.deepseek]
currency = "CNY"

[pricing.providers.deepseek.models."deepseek-v4-pro".cost]
input = 4.5
"#;

const DEEPSEEK_CATALOG: &[(&str, &str, ModelCost)] = &[(
    "deepseek",
    "deepseek-v4-pro",
    ModelCost {
        input_usd_per_mtok: 0.66,
        output_usd_per_mtok: 1.98,
        cache_read_usd_per_mtok: None,
        cache_write_usd_per_mtok: None,
    },
)];

/// A fixed instant, so nothing here depends on the wall clock.
fn instant(epoch_secs: u64) -> SystemTime {
    std::time::UNIX_EPOCH + std::time::Duration::from_secs(epoch_secs)
}

/// 2026-09-14T00:59:59Z (Monday): one second before the peak window opens.
const ONE_SECOND_BEFORE_PEAK: u64 = 1_789_347_599;
/// 2026-09-14T01:30:00Z (Monday): inside the peak window.
const INSIDE_PEAK: u64 = 1_789_349_400;

/// Run `f` with `toml` as the user's `config.toml` in an isolated home, and an
/// optional models.dev catalog behind it.
fn with_pricing_env<T>(
    config_toml: &str,
    catalog: &[(&str, &str, ModelCost)],
    f: impl FnOnce() -> T,
) -> T {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());
    clear_memory_cache_for_tests();
    if !catalog.is_empty() {
        save_test_cache(catalog);
    }
    std::fs::write(temp.path().join("config.toml"), config_toml).expect("write config.toml");
    crate::config::invalidate_config_cache();

    let out = f();

    clear_memory_cache_for_tests();
    crate::config::invalidate_config_cache();
    if let Some(prev) = prev_home {
        crate::env::set_var("JCODE_HOME", prev);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
    out
}

#[test]
fn complete_config_card_is_priced_in_its_own_currency() {
    let config = r#"
[pricing.providers.deepseek]
currency = "CNY"

[pricing.providers.deepseek.models."deepseek-v4-pro".cost]
input = 4.5
output = 13.5
cache_read = 0.15
"#;
    with_pricing_env(config, &[], || {
        let ConfigCallRates::Priced(card) =
            config_call_rates("deepseek", "deepseek-v4-pro", SystemTime::now())
        else {
            panic!("a complete card must price the call");
        };
        assert_eq!(card.currency, Currency::new("CNY"));
        assert_eq!(card.input_per_mtok, 4.5);
        assert_eq!(card.output_per_mtok, 13.5);
        assert_eq!(card.cache_read_per_mtok, Some(0.15));
    });
}

#[test]
fn tariff_in_effect_at_the_call_instant_is_the_one_priced() {
    with_pricing_env(PEAK_CARD_CONFIG, &[], || {
        let ConfigCallRates::Priced(off_peak) = config_call_rates(
            "deepseek",
            "deepseek-v4-pro",
            instant(ONE_SECOND_BEFORE_PEAK),
        ) else {
            panic!("off-peak instant is a config hit");
        };
        assert_eq!(off_peak.input_per_mtok, 1.0);

        let ConfigCallRates::Priced(peak) =
            config_call_rates("deepseek", "deepseek-v4-pro", instant(INSIDE_PEAK))
        else {
            panic!("peak instant is a config hit");
        };
        assert_eq!(peak.input_per_mtok, 10.0);
        assert_eq!(peak.output_per_mtok, 20.0);
    });
}

#[test]
fn without_a_configured_rule_the_answer_is_absent() {
    with_pricing_env("[display]\ncurrency = \"native\"\n", &[], || {
        assert!(matches!(
            config_call_rates("deepseek", "deepseek-v4-pro", SystemTime::now()),
            ConfigCallRates::Absent
        ));
    });
}

#[test]
fn incomplete_foreign_currency_card_is_reported_instead_of_priced() {
    // No catalog behind it, so the CNY card cannot be completed. Billing must
    // hear "configured, but not priceable" rather than get partial rates it
    // would top up with generic defaults.
    with_pricing_env(HALF_WRITTEN_CNY_CARD_CONFIG, &[], || {
        assert!(matches!(
            config_call_rates("deepseek", "deepseek-v4-pro", SystemTime::now()),
            ConfigCallRates::ConfiguredWithoutPrice
        ));
    });
}

#[test]
fn foreign_currency_card_that_loses_to_the_catalog_is_absent() {
    // With a catalog entry the USD layer wins outright (F1); the billing path
    // must then keep going to the derived layers instead of reporting a
    // non-price for the model.
    with_pricing_env(HALF_WRITTEN_CNY_CARD_CONFIG, DEEPSEEK_CATALOG, || {
        assert!(matches!(
            config_call_rates("deepseek", "deepseek-v4-pro", SystemTime::now()),
            ConfigCallRates::Absent
        ));
    });
}

#[test]
fn expired_rule_with_no_price_is_reported_instead_of_priced() {
    let config = r#"
[pricing.providers.deepseek]
currency = "CNY"

[pricing.providers.deepseek.models."deepseek-v4-pro"]
effective_until = "2020-01-01T00:00:00Z"
on_rule_expiry = "no_price"

[pricing.providers.deepseek.models."deepseek-v4-pro".cost]
input = 4.5
output = 13.5
"#;
    with_pricing_env(config, DEEPSEEK_CATALOG, || {
        assert!(matches!(
            config_call_rates("deepseek", "deepseek-v4-pro", SystemTime::now()),
            ConfigCallRates::ConfiguredWithoutPrice
        ));
    });
}

#[test]
fn expired_rule_that_falls_back_reports_the_reason() {
    // The default `on_rule_expiry` is `fallback` (F20): the next layer prices
    // the call, and the caller is told *why* it was not the config card, so the
    // display can label the fallback price (F8). Reporting plain `Absent` here
    // is what leaves the user reading a models.dev number without ever learning
    // their own rule stopped applying.
    let config = r#"
[pricing.providers.deepseek]
currency = "CNY"

[pricing.providers.deepseek.models."deepseek-v4-pro"]
effective_until = "2020-01-01T00:00:00Z"

[pricing.providers.deepseek.models."deepseek-v4-pro".cost]
input = 4.5
output = 13.5
"#;
    with_pricing_env(config, DEEPSEEK_CATALOG, || {
        assert_eq!(
            config_call_rates("deepseek", "deepseek-v4-pro", SystemTime::now()),
            ConfigCallRates::OutOfEffect(RuleOutOfEffect::Expired)
        );
    });
}

#[test]
fn rule_that_starts_later_reports_not_yet_effective() {
    // The other direction of the same window: a rule that has not opened yet is
    // not "expired", and saying so would misdescribe the user's own config.
    let config = r#"
[pricing.providers.deepseek.models."deepseek-v4-pro"]
effective_from = "2100-01-01T00:00:00Z"

[pricing.providers.deepseek.models."deepseek-v4-pro".cost]
input = 4.5
output = 13.5
"#;
    with_pricing_env(config, DEEPSEEK_CATALOG, || {
        assert_eq!(
            config_call_rates("deepseek", "deepseek-v4-pro", SystemTime::now()),
            ConfigCallRates::OutOfEffect(RuleOutOfEffect::NotYetEffective)
        );
    });
}
