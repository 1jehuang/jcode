//! The billing-facing rate lookup: a complete card is priced in its own
//! currency, and a card that cannot price the call is reported as such instead
//! of being topped up from the next layer (spec 4.4).
//!
//! Kept out of `call_rates.rs` and the already-large `mod.rs` test module: the
//! size ratchet tracks production files only, and this file is excluded.

use super::call_rates::{
    ConfigCallRates, call_cost, config_call_rates, config_call_rates_with_tier,
};
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
            config_call_rates("deepseek", "deepseek-v4-pro", SystemTime::now(), None)
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
            None,
        ) else {
            panic!("off-peak instant is a config hit");
        };
        assert_eq!(off_peak.input_per_mtok, 1.0);

        let ConfigCallRates::Priced(peak) =
            config_call_rates("deepseek", "deepseek-v4-pro", instant(INSIDE_PEAK), None)
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
            config_call_rates("deepseek", "deepseek-v4-pro", SystemTime::now(), None),
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
            config_call_rates("deepseek", "deepseek-v4-pro", SystemTime::now(), None),
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
            config_call_rates("deepseek", "deepseek-v4-pro", SystemTime::now(), None),
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
            config_call_rates("deepseek", "deepseek-v4-pro", SystemTime::now(), None),
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
            config_call_rates("deepseek", "deepseek-v4-pro", SystemTime::now(), None),
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
            config_call_rates("deepseek", "deepseek-v4-pro", SystemTime::now(), None),
            ConfigCallRates::OutOfEffect(RuleOutOfEffect::NotYetEffective)
        );
    });
}

/// A catalog entry that states a `cache_write` rate, which is what models.dev
/// publishes for Anthropic models (its own 1.25x figure).
const CACHE_WRITE_CATALOG: &[(&str, &str, ModelCost)] = &[(
    "anthropic",
    "claude-sonnet-4-6",
    ModelCost {
        input_usd_per_mtok: 3.0,
        output_usd_per_mtok: 15.0,
        cache_read_usd_per_mtok: Some(0.3),
        cache_write_usd_per_mtok: Some(3.75),
    },
)];

#[test]
fn a_configured_cache_write_rate_is_reported_by_the_billing_card() {
    // I-1: the rate the user wrote must reach the billing path. Otherwise
    // `cache_write` is validated, carried into the card, and silently ignored:
    // the cost site applies Anthropic's `input x 1.25/2.0` premium instead and
    // the user is billed a number they never configured.
    let config = r#"
[pricing.providers."claude:api-key".models."claude-sonnet-4-6".cost]
input = 1.0
output = 2.0
cache_write = 0.9
"#;
    with_pricing_env(config, &[], || {
        let ConfigCallRates::Priced(card) = config_call_rates(
            "claude:api-key",
            "claude-sonnet-4-6",
            SystemTime::now(),
            None,
        ) else {
            panic!("a complete card must price the call");
        };
        assert_eq!(card.input_per_mtok, 1.0);
        assert_eq!(card.cache_write_per_mtok, Some(0.9));
    });
}

#[test]
fn a_cache_write_rate_that_only_the_fallback_layer_states_is_not_honoured() {
    // Parity constraint for I-1: only a rate the user's own card states may
    // replace the billing premium. A `cache_write` that arrived by merging
    // models.dev into a USD card belongs to the *fallback* layer, and the
    // pre-feature heuristic has to keep owning that case: otherwise writing an
    // unrelated card (input/output only) would quietly change what every
    // Anthropic cache write costs.
    let config = r#"
[pricing.providers."claude:api-key".models."claude-sonnet-4-6".cost]
input = 1.0
output = 2.0
"#;
    with_pricing_env(config, CACHE_WRITE_CATALOG, || {
        let ConfigCallRates::Priced(card) = config_call_rates(
            "claude:api-key",
            "claude-sonnet-4-6",
            SystemTime::now(),
            None,
        ) else {
            panic!("a complete card must price the call");
        };
        assert_eq!(
            card.cache_read_per_mtok,
            Some(0.3),
            "the USD card still merges the fallback field by field"
        );
        assert_eq!(
            card.cache_write_per_mtok, None,
            "models.dev's cache-write figure must not read as a configured rate"
        );
    });
}

#[test]
fn an_invalid_pricing_section_is_ignored_and_the_reason_is_reported() {
    // Task 8 F-3 / I-2: `validate` stops at the first error, so one bad line
    // drops the *whole* section and every hand-written rule in it silently
    // stops applying. The rejection has to be reportable next to the number the
    // user reads, not only in a log file.
    let config = r#"
[pricing.fx_rates]
CNY = -7.2

[pricing.providers.deepseek.models."deepseek-v4-pro".cost]
input = 4.5
output = 13.5
"#;
    with_pricing_env(config, DEEPSEEK_CATALOG, || {
        assert!(
            matches!(
                config_call_rates("deepseek", "deepseek-v4-pro", SystemTime::now(), None),
                ConfigCallRates::Absent
            ),
            "the whole section is dropped, including the providers that were fine"
        );

        let error = crate::model_pricing::pricing_config_error()
            .expect("a rejected [pricing] section must be reportable, not log-only");
        assert_eq!(error.field_path, "pricing.fx_rates.CNY");
        assert!(
            error.message.contains("positive"),
            "the message says what was wrong: {error}"
        );

        // Memoized against the loaded config: the render path reads this every
        // frame, so it must not re-validate (or re-log) per frame.
        let again = crate::model_pricing::pricing_config_error().expect("still reported");
        assert!(
            std::sync::Arc::ptr_eq(&error, &again),
            "the rejected section is parsed once per loaded config, not once per read"
        );
    });
}

/// Provenance: a cost view has to be able to name the tariff that is in force,
/// from either branch of the selection (a schedule window, or `default_tariff`).
#[test]
fn selected_config_tariff_names_the_window_in_force() {
    let config = r#"
[pricing.providers.deepseek]
currency = "CNY"

[pricing.providers.deepseek.models."deepseek-v4-pro"]
default_tariff = "off_peak"

[pricing.providers.deepseek.models."deepseek-v4-pro".cost]
input = 4.5
output = 13.5

[pricing.providers.deepseek.models."deepseek-v4-pro".tariffs.off_peak]
input = 4.5
output = 13.5

[pricing.providers.deepseek.models."deepseek-v4-pro".tariffs.peak]
multiplier = 2.0

[[pricing.providers.deepseek.models."deepseek-v4-pro".schedule]]
tariff = "peak"
utc_offset_minutes = 0
weekdays = ["Mon", "Tue", "Wed", "Thu", "Fri"]
windows = [["01:00", "04:00"]]
"#;
    with_pricing_env(config, &[], || {
        assert_eq!(
            super::selected_config_tariff("deepseek", "deepseek-v4-pro", instant(INSIDE_PEAK)),
            Some("peak".to_string()),
            "the schedule window selects the peak tariff"
        );
        assert_eq!(
            super::selected_config_tariff(
                "deepseek",
                "deepseek-v4-pro",
                instant(ONE_SECOND_BEFORE_PEAK)
            ),
            Some("off_peak".to_string()),
            "outside every window the card's default tariff applies"
        );
        assert_eq!(
            super::selected_config_tariff("openai", "gpt-5.5", instant(INSIDE_PEAK)),
            None,
            "a model no hand-written card prices has no config tariff"
        );
    });
}

#[test]
fn call_cost_prices_a_configured_card_with_the_client_math() {
    // The server resolves a remote call's cost with the same math the client
    // uses, so the reported figure matches what the client would have billed.
    let config = r#"
[pricing.providers.deepseek.models."deepseek-v4-pro".cost]
input = 1.0
output = 2.0
"#;
    with_pricing_env(config, &[], || {
        let (amount, currency) = super::call_cost(
            "deepseek",
            "deepseek-v4-pro",
            SystemTime::now(),
            None,
            1_000_000,
            1_000_000,
            0,
            0,
            false,
            false,
        )
        .expect("a configured card prices the call");
        assert!(
            (amount - 3.0).abs() < 1e-9,
            "1M input + 1M output at $1/$2 per Mtok is $3.00, got {amount}"
        );
        assert_eq!(currency, Currency::new("USD"));
    });
}

#[test]
fn call_cost_reads_the_tariff_in_effect_at_the_call_instant() {
    // A peak/off-peak card is resolved at the call's own instant (F15): the
    // same call priced one second apart bills the tariff in force at `at`.
    with_pricing_env(PEAK_CARD_CONFIG, &[], || {
        let (off_peak, _) = super::call_cost(
            "deepseek",
            "deepseek-v4-pro",
            instant(ONE_SECOND_BEFORE_PEAK),
            None,
            1_000_000,
            0,
            0,
            0,
            false,
            false,
        )
        .expect("off-peak instant is priced");
        assert!(
            (off_peak - 1.0).abs() < 1e-9,
            "off-peak input is $1.00/Mtok, got {off_peak}"
        );

        let (peak, _) = super::call_cost(
            "deepseek",
            "deepseek-v4-pro",
            instant(INSIDE_PEAK),
            None,
            1_000_000,
            0,
            0,
            0,
            false,
            false,
        )
        .expect("peak instant is priced");
        assert!(
            (peak - 10.0).abs() < 1e-9,
            "the peak tariff is 10x off-peak, so input is $10.00/Mtok, got {peak}"
        );
    });
}

#[test]
fn call_cost_is_none_when_no_layer_can_price_the_call() {
    // A card that claims the model but cannot price the call must yield no
    // server figure at all: the client then keeps its own fallback instead of
    // reporting a number the server invented (spec 4.4).
    with_pricing_env(HALF_WRITTEN_CNY_CARD_CONFIG, &[], || {
        assert!(
            super::call_cost(
                "deepseek",
                "deepseek-v4-pro",
                SystemTime::now(),
                None,
                1,
                1,
                0,
                0,
                false,
                false,
            )
            .is_none(),
            "an incomplete foreign-currency card must not be priced"
        );
    });
}

/// An isolated home with one vendor file, for the billing-path regressions.
fn with_vendor_file<T>(config: &str, file_body: &str, f: impl FnOnce() -> T) -> T {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());
    clear_memory_cache_for_tests();
    super::vendor_files::clear_vendor_files_cache_for_tests();
    let path = temp.path().join("prices.json");
    std::fs::write(&path, file_body).expect("write vendor file");
    let config = config.replace("{path}", &path.display().to_string());
    std::fs::write(temp.path().join("config.toml"), config).expect("write config.toml");
    crate::config::invalidate_config_cache();

    let out = f();

    clear_memory_cache_for_tests();
    super::vendor_files::clear_vendor_files_cache_for_tests();
    crate::config::invalidate_config_cache();
    if let Some(prev) = prev_home {
        crate::env::set_var("JCODE_HOME", prev);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
    out
}

/// A vendor file's explicit cache-write rate must price cache-creation tokens,
/// not the Anthropic-style premium over the input rate.
#[test]
fn a_vendor_file_cache_write_rate_reaches_call_cost() {
    let file =
        r#"{"models":{"deepseek-v4-pro":{"cost":{"input":3.0,"output":6.0,"cache_write":0.9}}}}"#;
    let config = "[pricing.providers.deepseek]\nfile = \"{path}\"\n";
    with_vendor_file(config, file, || {
        let (cost, currency) = call_cost(
            "deepseek",
            "deepseek-v4-pro",
            SystemTime::now(),
            None,
            0,
            0,
            0,
            1_000_000,
            // Anthropic-style split accounting still applies the explicit rate.
            true,
            false,
        )
        .expect("the file prices the call");
        assert_eq!(currency, Currency::usd());
        assert!(
            (cost - 0.9).abs() < 1e-9,
            "1M cache-write tokens at 0.9/Mtok must cost 0.9, got {cost}"
        );
    });
}

/// A partial USD inline card takes the rate it omits from the eligible vendor
/// file before the derived catalogs (the file is the next configured layer).
#[test]
fn a_partial_usd_card_takes_its_missing_rate_from_the_vendor_file() {
    let file = r#"{"models":{"deepseek-v4-pro":{"cost":{"input":1.0,"output":4.0}}}}"#;
    let config = r#"
[pricing.providers.deepseek]
file = "{path}"

[pricing.providers.deepseek.models."deepseek-v4-pro".cost]
input = 2.0
"#;
    with_vendor_file(config, file, || {
        let ConfigCallRates::Priced(card) =
            config_call_rates("deepseek", "deepseek-v4-pro", SystemTime::now(), None)
        else {
            panic!("the partial inline card plus the file must price the call");
        };
        assert_eq!(card.input_per_mtok, 2.0, "the inline input override wins");
        assert_eq!(
            card.output_per_mtok, 4.0,
            "the file supplies the output rate"
        );
    });
}

/// A foreign-currency card whose own absolute tariff supplies the missing rate
/// is complete; it must not be discarded in favour of a derived USD card.
#[test]
fn a_foreign_tariff_that_completes_the_card_is_not_discarded() {
    let config = r#"
[pricing.providers.deepseek]
currency = "CNY"

[pricing.providers.deepseek.models."deepseek-v4-pro"]
default_tariff = "active"

[pricing.providers.deepseek.models."deepseek-v4-pro".cost]
input = 4.5

[pricing.providers.deepseek.models."deepseek-v4-pro".tariffs.active]
input = 4.5
output = 13.5
"#;
    with_pricing_env(config, DEEPSEEK_CATALOG, || {
        let ConfigCallRates::Priced(card) =
            config_call_rates("deepseek", "deepseek-v4-pro", SystemTime::now(), None)
        else {
            panic!("the active absolute tariff completes the CNY card");
        };
        assert_eq!(card.currency, Currency::new("CNY"));
        assert_eq!(card.input_per_mtok, 4.5);
        assert_eq!(card.output_per_mtok, 13.5);
    });
}

/// A partial card completed from the curated tables must use the tier the call
/// runs in; standard-tier output would under-bill an Anthropic fast call.
#[test]
fn a_partial_card_completes_at_the_active_service_tier() {
    let config = r#"
[pricing.providers."claude:api-key".models."claude-opus-4-6".cost]
input = 7.0
"#;
    with_pricing_env(config, &[], || {
        let standard =
            config_call_rates("claude:api-key", "claude-opus-4-6", SystemTime::now(), None);
        let ConfigCallRates::Priced(standard) = standard else {
            panic!("the curated table completes the partial card");
        };
        assert_eq!(standard.output_per_mtok, 25.0);

        let fast = config_call_rates_with_tier(
            "claude:api-key",
            "claude-opus-4-6",
            SystemTime::now(),
            None,
            Some("priority"),
        );
        let ConfigCallRates::Priced(fast) = fast else {
            panic!("the curated fast table completes the partial card");
        };
        assert_eq!(fast.input_per_mtok, 7.0, "the card's own rate stays");
        assert_eq!(
            fast.output_per_mtok, 150.0,
            "the fast tier's output rate must fill the missing field"
        );
    });
}
