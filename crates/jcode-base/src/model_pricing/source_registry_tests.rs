//! `[[pricing.sources]]` registry tests.
//!
//! Everything here runs in an isolated `JCODE_HOME`, uses `file://` sheets or a
//! primed cache for the network case, and never lets a background fetch start
//! (`background_refresh_allowed` is false in test builds), so the merge order,
//! the TTL and the failure paths are all exercised deterministically.

use super::ModelCost;
use super::entry::ModelPricingEntry;
use super::source_registry::{
    cached_fetched_at_for_tests, clear_sources_cache_for_tests, forget_failures_for_tests,
    in_backoff_for_tests, source_card,
};
use crate::model_pricing::{clear_memory_cache_for_tests, save_test_cache, save_test_source};
use jcode_provider_core::Currency;
use std::ffi::OsString;
use std::time::SystemTime;

/// The models.dev snapshot every test starts from: DeepSeek at $0.66/$1.98 per
/// Mtok, so a sheet's own numbers are unmistakable.
const MODELS_DEV: &[(&str, &str, ModelCost)] = &[(
    "deepseek",
    "deepseek-v4-pro",
    ModelCost {
        input_usd_per_mtok: 0.66,
        output_usd_per_mtok: 1.98,
        cache_read_usd_per_mtok: Some(0.022),
        cache_write_usd_per_mtok: None,
    },
)];

/// models.dev's price for the canonical 25k-in/5k-out reference request.
const MODELS_DEV_REFERENCE: f64 = (0.66 * 25_000.0 + 1.98 * 5_000.0) / 1_000_000.0;

/// An isolated `JCODE_HOME`, with the test-env lock held for its lifetime.
struct Env {
    _lock: std::sync::MutexGuard<'static, ()>,
    dir: tempfile::TempDir,
    previous_home: Option<OsString>,
}

impl Env {
    fn new() -> Self {
        let lock = crate::storage::lock_test_env();
        let dir = tempfile::TempDir::new().expect("tempdir");
        let previous_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", dir.path());
        clear_memory_cache_for_tests();
        clear_sources_cache_for_tests();
        crate::config::invalidate_config_cache();
        Self {
            _lock: lock,
            dir,
            previous_home,
        }
    }

    fn write_config(&self, body: &str) {
        std::fs::write(self.dir.path().join("config.toml"), body).expect("write config.toml");
        crate::config::invalidate_config_cache();
    }

    /// Write a sheet next to the config and return its `file://` URL.
    fn sheet_url(&self, name: &str, body: &str) -> String {
        let path = self.dir.path().join(name);
        std::fs::write(&path, body).expect("write sheet");
        format!("file://{}", path.display())
    }

    fn save_models_dev(&self) {
        save_test_cache(MODELS_DEV);
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        clear_memory_cache_for_tests();
        clear_sources_cache_for_tests();
        crate::config::invalidate_config_cache();
        match self.previous_home.take() {
            Some(previous) => crate::env::set_var("JCODE_HOME", previous),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }
}

/// A complete models.dev-shaped sheet body for one model.
fn sheet_body(provider: &str, model: &str, input: f64, output: f64) -> String {
    r#"{"PROVIDER":{"models":{"MODEL":{"cost":{"input":INPUT,"output":OUTPUT}}}}}"#
        .replace("PROVIDER", provider)
        .replace("MODEL", model)
        .replace("INPUT", &input.to_string())
        .replace("OUTPUT", &output.to_string())
}

fn cost(input: f64, output: f64) -> ModelPricingEntry {
    ModelPricingEntry::from_model_cost(ModelCost {
        input_usd_per_mtok: input,
        output_usd_per_mtok: output,
        cache_read_usd_per_mtok: None,
        cache_write_usd_per_mtok: None,
    })
}

fn reference_amount(input: f64, output: f64) -> f64 {
    (input * 25_000.0 + output * 5_000.0) / 1_000_000.0
}

/// The sheet that prices the model, or `None` when the layer has nothing to
/// say. This is the layer-level probe the ordering tests use.
fn priced_by(source_key: &str, model: &str) -> Option<String> {
    source_card(source_key, model, SystemTime::now()).map(|hit| hit.source_id)
}

#[test]
fn a_sheet_prices_between_the_config_card_and_models_dev() {
    let env = Env::new();
    env.save_models_dev();
    let url = env.sheet_url(
        "mirror.json",
        &sheet_body("deepseek", "deepseek-v4-pro", 9.0, 18.0),
    );
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "mirror"
url = "{url}"
"#
    ));

    // Sheet only: the sheet's rates, not models.dev's.
    let sheet_only =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("priced");
    assert!(
        (sheet_only.amount - reference_amount(9.0, 18.0)).abs() < 1e-12,
        "the sheet must price the call, got {sheet_only:?}"
    );

    // Config card added: the card is authoritative and it wins.
    let url = env.sheet_url(
        "mirror.json",
        &sheet_body("deepseek", "deepseek-v4-pro", 9.0, 18.0),
    );
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "mirror"
url = "{url}"

[pricing.providers.deepseek.models.deepseek-v4-pro.cost]
input = 1.0
output = 2.0
"#
    ));
    let with_card =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("priced");
    assert!(
        (with_card.amount - reference_amount(1.0, 2.0)).abs() < 1e-12,
        "a hand-written card must outrank a sheet, got {with_card:?}"
    );

    // Sheet removed: models.dev prices it, i.e. the sheet sat in between.
    env.write_config("");
    let catalog_only =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("priced");
    assert!(
        (catalog_only.amount - MODELS_DEV_REFERENCE).abs() < 1e-12,
        "models.dev must still be the floor, got {catalog_only:?}"
    );
}

#[test]
fn scope_form_one_matches_the_exact_source_key() {
    let env = Env::new();
    env.save_models_dev();
    let url = env.sheet_url(
        "openrouter.json",
        &sheet_body("openrouter", "some-model", 3.0, 4.0),
    );
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "openrouter-only"
url = "{url}"
scope = ["openrouter"]
"#
    ));

    assert_eq!(
        priced_by("openrouter", "some-model").as_deref(),
        Some("openrouter-only")
    );
    assert_eq!(
        priced_by("deepseek", "deepseek-v4-pro"),
        None,
        "a scope that does not name the provider must not price it"
    );
}

#[test]
fn scope_form_two_matches_the_models_dev_provider_id_on_both_sides() {
    let env = Env::new();
    env.save_models_dev();
    let url = env.sheet_url(
        "deepseek.json",
        &sheet_body("deepseek", "deepseek-v4-pro", 9.0, 18.0),
    );
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "ds"
url = "{url}"
scope = ["deepseek"]
"#
    ));

    // The fallback slug and the compatible-profile route are the same provider.
    for source_key in ["deepseek", "openai-compatible:deepseek"] {
        assert_eq!(
            priced_by(source_key, "deepseek-v4-pro").as_deref(),
            Some("ds"),
            "scope `deepseek` must cover {source_key}"
        );
    }
    assert_eq!(
        priced_by("openrouter", "some-model"),
        None,
        "scope `deepseek` must not cover a different provider"
    );
}

#[test]
fn scope_form_three_matches_the_compatible_profile_id() {
    let env = Env::new();
    env.save_models_dev();
    let url = env.sheet_url(
        "gateway.json",
        &sheet_body("my-gateway", "gw-model", 5.0, 6.0),
    );
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "gateway"
url = "{url}"
scope = ["openai-compatible:my-gateway"]
"#
    ));

    for source_key in ["openai-compatible:my-gateway", "my-gateway"] {
        assert_eq!(
            priced_by(source_key, "gw-model").as_deref(),
            Some("gateway"),
            "the profile id and its prefixed form are the same route ({source_key})"
        );
    }
    assert_eq!(
        priced_by("openai-compatible:other-gateway", "gw-model"),
        None,
        "a different profile must not be covered"
    );
}

#[test]
fn a_models_glob_scopes_a_sheet() {
    let env = Env::new();
    env.save_models_dev();
    // Two models in one sheet: only the globbed one is covered.
    let url = env.sheet_url(
        "glob.json",
        r#"{"deepseek":{"models":{
            "deepseek-v4-pro":{"cost":{"input":9.0,"output":18.0}},
            "deepseek-v5-pro":{"cost":{"input":4.0,"output":8.0}}
        }}}"#,
    );
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "v4-only"
url = "{url}"
scope = ["deepseek"]
models = ["deepseek-v4-*"]
"#
    ));

    assert_eq!(
        priced_by("deepseek", "deepseek-v4-pro").as_deref(),
        Some("v4-only")
    );
    assert_eq!(
        priced_by("deepseek", "deepseek-v5-pro"),
        None,
        "a model outside the glob falls through"
    );
}

#[test]
fn priority_decides_between_sheets_and_the_id_breaks_ties() {
    let env = Env::new();
    env.save_models_dev();
    // The ids are deliberately in the opposite order to the priorities, so an
    // id-only ordering cannot pass this test by accident.
    let losing = env.sheet_url(
        "losing.json",
        &sheet_body("deepseek", "deepseek-v4-pro", 9.0, 18.0),
    );
    let winning = env.sheet_url(
        "winning.json",
        &sheet_body("deepseek", "deepseek-v4-pro", 3.0, 6.0),
    );

    // Lower priority number wins, whichever order the file lists them in.
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "alpha"
url = "{losing}"
priority = 10

[[pricing.sources]]
id = "zulu"
url = "{winning}"
priority = 5
"#
    ));
    let winner = source_card("deepseek", "deepseek-v4-pro", SystemTime::now()).expect("hit");
    assert_eq!(winner.source_id, "zulu", "the lower priority number wins");
    assert_eq!(
        winner.entry.cost.input,
        Some(3.0),
        "the winner's own rates, not the loser's"
    );

    // Equal priority: the id decides, so the merge cannot depend on map order.
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "zulu"
url = "{losing}"

[[pricing.sources]]
id = "alpha"
url = "{winning}"
"#
    ));
    let winner = source_card("deepseek", "deepseek-v4-pro", SystemTime::now()).expect("hit");
    assert_eq!(
        winner.source_id, "alpha",
        "ties break on id lexicographically"
    );
    assert_eq!(winner.entry.cost.input, Some(3.0));
}

#[test]
fn a_lower_priority_sheet_fills_fields_the_winner_leaves_unset() {
    let env = Env::new();
    env.save_models_dev();
    // The winner's entry is primed rather than parsed: the JSON parser
    // (correctly) refuses a model with no output rate, and this test is about
    // what the merge does with an entry that has one.
    let complete = env.sheet_url(
        "complete.json",
        &sheet_body("deepseek", "deepseek-v4-pro", 9.0, 18.0),
    );
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "winner"
url = "file:///nonexistent-winner.json"
priority = 0

[[pricing.sources]]
id = "filler"
url = "{complete}"
priority = 10
"#
    ));
    save_test_source(
        "winner",
        "file:///nonexistent-winner.json",
        super::catalog::now_unix_secs(),
        &[(
            "deepseek",
            "deepseek-v4-pro",
            ModelPricingEntry {
                cost: crate::config::CostFields {
                    input: Some(1.0),
                    ..Default::default()
                },
                ..Default::default()
            },
        )],
    );

    let hit = source_card("deepseek", "deepseek-v4-pro", SystemTime::now()).expect("hit");
    assert_eq!(hit.source_id, "winner");
    assert_eq!(
        hit.entry.cost.input,
        Some(1.0),
        "the winner's own field stands"
    );
    assert_eq!(
        hit.entry.cost.output,
        Some(18.0),
        "the field the winner leaves unset comes from the next sheet"
    );
}

#[test]
fn a_fresh_sheet_is_used_and_a_stale_one_is_not() {
    let env = Env::new();
    env.save_models_dev();
    let url = "https://pricing.example.invalid/mirror.json";
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "mirror"
url = "{url}"
refresh_secs = 3600
"#
    ));

    let now = super::catalog::now_unix_secs();
    save_test_source(
        "mirror",
        url,
        now,
        &[("deepseek", "deepseek-v4-pro", cost(9.0, 18.0))],
    );
    let fresh =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("priced");
    assert!(
        (fresh.amount - reference_amount(9.0, 18.0)).abs() < 1e-12,
        "a copy within its TTL is used, got {fresh:?}"
    );

    // Same copy, fetched long before the 3600s TTL: not used, and the call is
    // still priced by models.dev (never left unpriced).
    save_test_source(
        "mirror",
        url,
        0,
        &[("deepseek", "deepseek-v4-pro", cost(9.0, 18.0))],
    );
    let stale =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("still priced");
    assert!(
        (stale.amount - MODELS_DEV_REFERENCE).abs() < 1e-12,
        "a stale copy must not be used, got {stale:?}"
    );
    assert_eq!(
        cached_fetched_at_for_tests("mirror"),
        Some(0),
        "the last good copy stays on disk for the refresh to replace"
    );
}

#[test]
fn an_unreachable_remote_sheet_falls_through_to_models_dev() {
    let env = Env::new();
    env.save_models_dev();
    env.write_config(
        r#"
[[pricing.sources]]
id = "gone"
url = "https://pricing.example.invalid/gone.json"
"#,
    );

    assert_eq!(priced_by("deepseek", "deepseek-v4-pro"), None);
    let priced =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("models.dev still prices the call");
    assert!((priced.amount - MODELS_DEV_REFERENCE).abs() < 1e-12);
    assert!(priced.currency.is_usd());
}

#[test]
fn a_missing_local_sheet_falls_through_to_models_dev() {
    let env = Env::new();
    env.save_models_dev();
    let missing = env.dir.path().join("not-there.json");
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "missing"
url = "file://{}"
"#,
        missing.display()
    ));

    let priced =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("models.dev still prices the call");
    assert!((priced.amount - MODELS_DEV_REFERENCE).abs() < 1e-12);
}

#[test]
fn a_malformed_sheet_falls_through_to_models_dev() {
    let env = Env::new();
    env.save_models_dev();
    let url = env.sheet_url("broken.json", "{ this is not json ");
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "broken"
url = "{url}"
"#
    ));

    let priced =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("models.dev still prices the call");
    assert!((priced.amount - MODELS_DEV_REFERENCE).abs() < 1e-12);
    assert_eq!(priced_by("deepseek", "deepseek-v4-pro"), None);
}

#[test]
fn a_sheet_whose_rule_is_out_of_effect_is_skipped() {
    let env = Env::new();
    env.save_models_dev();
    let url = env.sheet_url(
        "expired.json",
        r#"{"deepseek":{"models":{"deepseek-v4-pro":{
            "cost":{"input":9.0,"output":18.0},
            "effective_until":"2020-01-01T00:00:00Z"
        }}}}"#,
    );
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "expired"
url = "{url}"
"#
    ));

    assert_eq!(priced_by("deepseek", "deepseek-v4-pro"), None);
    let priced =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("models.dev still prices the call");
    assert!((priced.amount - MODELS_DEV_REFERENCE).abs() < 1e-12);

    // F8/F20: falling through silently is exactly the silent-wrong-price class
    // this feature removes, so the expiry is reported *by name* and the widget
    // and `/pricing` can label the models.dev price with it.
    let notice = crate::model_pricing::sheet_rule_out_of_effect(
        "deepseek",
        "deepseek-v4-pro",
        SystemTime::now(),
    )
    .expect("the out-of-effect sheet is reported");
    assert_eq!(
        notice.label(),
        "rule expired (pricing source `expired`)",
        "the marker names the sheet whose rule stopped applying"
    );
}

/// The counterpart of the test above: a sheet that *is* in effect must not be
/// reported as out of effect, or every price from a sheet would carry a
/// misleading marker.
#[test]
fn an_in_effect_sheet_rule_is_not_reported_out_of_effect() {
    let env = Env::new();
    env.save_models_dev();
    let url = env.sheet_url(
        "live.json",
        &sheet_body("deepseek", "deepseek-v4-pro", 9.0, 18.0),
    );
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "live"
url = "{url}"
"#
    ));

    assert_eq!(
        priced_by("deepseek", "deepseek-v4-pro").as_deref(),
        Some("live")
    );
    assert_eq!(
        crate::model_pricing::sheet_rule_out_of_effect(
            "deepseek",
            "deepseek-v4-pro",
            SystemTime::now()
        ),
        None,
        "an in-effect sheet is not out of effect"
    );
}

#[test]
fn a_foreign_currency_sheet_prices_in_its_own_currency_never_in_another() {
    let env = Env::new();
    env.save_models_dev();
    let url = env.sheet_url(
        "cny.json",
        &sheet_body("deepseek", "deepseek-v4-pro", 12.0, 24.0),
    );
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "cny"
url = "{url}"
currency = "CNY"
"#
    ));

    let cny =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("priced");
    assert_eq!(cny.currency.as_str(), "CNY");
    assert!(
        (cny.amount - reference_amount(12.0, 24.0)).abs() < 1e-12,
        "the numbers stay the sheet's, got {cny:?}"
    );

    // A model the sheet does not carry keeps models.dev's USD label: a currency
    // stated for one sheet must not leak into the layer below.
    let usd =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("priced");
    assert_eq!(usd.currency.as_str(), "CNY");

    env.write_config("");
    let catalog =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("priced");
    assert!(
        catalog.currency.is_usd(),
        "models.dev values are USD, got {:?}",
        catalog.currency
    );
}

/// A sheet that cannot price a call on its own (foreign currency, no output
/// rate) must not have models.dev's USD numbers relabelled as its currency, and
/// must not block models.dev from pricing the call either (F1).
#[test]
fn an_incomplete_foreign_currency_sheet_does_not_relabel_models_dev() {
    let env = Env::new();
    env.save_models_dev();
    env.write_config(
        r#"
[[pricing.sources]]
id = "incomplete"
url = "file:///nonexistent-incomplete.json"
currency = "CNY"
"#,
    );
    save_test_source(
        "incomplete",
        "file:///nonexistent-incomplete.json",
        super::catalog::now_unix_secs(),
        &[(
            "deepseek",
            "deepseek-v4-pro",
            ModelPricingEntry {
                cost: crate::config::CostFields {
                    input: Some(1.0),
                    ..Default::default()
                },
                ..Default::default()
            },
        )],
    );

    let priced =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("models.dev prices it");
    assert!(
        priced.currency.is_usd(),
        "a half card in CNY must not relabel USD numbers, got {priced:?}"
    );
    assert!((priced.amount - MODELS_DEV_REFERENCE).abs() < 1e-12);
}

#[test]
fn the_derived_billing_layer_labels_and_uses_the_sheet() {
    let env = Env::new();
    env.save_models_dev();
    let url = env.sheet_url(
        "mirror.json",
        &sheet_body("deepseek", "deepseek-v4-pro", 9.0, 18.0),
    );
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "mirror"
url = "{url}"
"#
    ));

    // This is the path billing takes when no hand-written card claims the call.
    let estimate = crate::provider::pricing::derived_pricing_for_source_at_size(
        "deepseek",
        "deepseek-v4-pro",
        None,
        None,
    )
    .expect("priced");
    assert_eq!(
        estimate.source,
        jcode_provider_core::RouteCostSource::ExtraPriceSource
    );
    assert_eq!(estimate.input_price_per_mtok_micros, Some(9_000_000));
    assert_eq!(estimate.output_price_per_mtok_micros, Some(18_000_000));
    assert!(
        estimate
            .note
            .as_deref()
            .unwrap_or_default()
            .contains("mirror"),
        "the sheet that priced the call is named: {estimate:?}"
    );

    env.write_config("");
    let catalog = crate::provider::pricing::derived_pricing_for_source_at_size(
        "deepseek",
        "deepseek-v4-pro",
        None,
        None,
    )
    .expect("priced");
    assert_eq!(
        catalog.source,
        jcode_provider_core::RouteCostSource::ModelsDevCatalog,
        "with no sheet the derived chain is unchanged"
    );
}

#[test]
fn the_config_card_still_outranks_a_sheet_in_the_route_catalog() {
    let env = Env::new();
    env.save_models_dev();
    let url = env.sheet_url(
        "mirror.json",
        &sheet_body("deepseek", "deepseek-v4-pro", 9.0, 18.0),
    );
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "mirror"
url = "{url}"

[pricing.providers.deepseek.models.deepseek-v4-pro.cost]
input = 1.0
output = 2.0
"#
    ));

    let estimate = crate::provider::pricing::metered_pricing_for_source_at(
        "deepseek",
        "deepseek-v4-pro",
        None,
        SystemTime::now(),
    )
    .expect("priced");
    assert_eq!(
        estimate.source,
        jcode_provider_core::RouteCostSource::ConfigPriceSheet,
        "the hand-written card is still the authority"
    );
    assert_eq!(estimate.input_price_per_mtok_micros, Some(1_000_000));
}

#[test]
fn glob_matching_is_case_insensitive_and_supports_star_and_question_mark() {
    use super::glob_matches;
    assert!(glob_matches("deepseek-v4-*", "deepseek-v4-pro"));
    assert!(glob_matches("deepseek-v4-*", "DEEPSEEK-V4-PRO"));
    assert!(glob_matches("*v4*", "deepseek-v4-pro"));
    assert!(glob_matches("*", "anything"));
    assert!(glob_matches("deepseek-v4-pro", "deepseek-v4-pro"));
    assert!(glob_matches("deepseek-v?-pro", "deepseek-v4-pro"));
    assert!(!glob_matches("deepseek-v5-*", "deepseek-v4-pro"));
    assert!(!glob_matches("deepseek-v4", "deepseek-v4-pro"));
    assert!(!glob_matches("", "deepseek-v4-pro"));
    assert!(!glob_matches("deepseek-*", ""));
}

#[test]
fn a_currency_stated_by_a_sheet_is_normalized_like_any_other_code() {
    let env = Env::new();
    env.save_models_dev();
    let url = env.sheet_url(
        "cny.json",
        &sheet_body("deepseek", "deepseek-v4-pro", 12.0, 24.0),
    );
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "cny"
url = "{url}"
currency = "cny"
"#
    ));

    let hit = source_card("deepseek", "deepseek-v4-pro", SystemTime::now()).expect("hit");
    assert_eq!(hit.currency, Currency::new("CNY"));
}

/// An invalid `[[pricing.sources]]` entry must be visible where the user can
/// see it, not only in the log: the display renders
/// `invalid [pricing]: <field_path>` next to the amount it changed
/// (`money_display::note_pricing_problems`).
#[test]
fn an_invalid_source_is_reported_to_the_display_and_ignored() {
    let env = Env::new();
    env.save_models_dev();
    env.write_config(
        r#"
[[pricing.sources]]
id = "bad"
url = "http://insecure.example/pricing.json"
"#,
    );

    let error = crate::model_pricing::pricing_config_error().expect("reported");
    assert_eq!(error.field_path, "pricing.sources[0].url");

    // And the section is ignored whole, exactly like any other invalid
    // `[pricing]`: models.dev still prices the call.
    let priced =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("priced");
    assert!((priced.amount - MODELS_DEV_REFERENCE).abs() < 1e-12);
}

/// A sheet states its extension fields in the config vocabulary, so a sheet can
/// carry peak/off-peak rates just like a hand-written card (spec 4.3).
#[test]
fn a_sheet_can_state_peak_hours_like_a_hand_written_card() {
    let env = Env::new();
    env.save_models_dev();
    let url = env.sheet_url(
        "peak.json",
        r#"{"deepseek":{"models":{"deepseek-v4-pro":{
            "cost":{"input":1.0,"output":2.0},
            "tariffs":{"peak":{"multiplier":2.0}},
            "schedule":[{
                "tariff":"peak",
                "utc_offset_minutes":0,
                "weekdays":["Mon","Tue","Wed","Thu","Fri"],
                "windows":[["01:00","04:00"]]
            }]
        }}}}"#,
    );
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "peak"
url = "{url}"
"#
    ));

    let instant = |secs: u64| std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
    // 2030-06-22T02:00:00Z (Saturday) and 2030-06-24T02:00:00Z (Monday, inside
    // the 01:00-04:00 UTC weekday window).
    let off_peak =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", instant(1_908_324_000))
            .expect("priced");
    let peak =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", instant(1_908_496_800))
            .expect("priced");
    assert!(
        (off_peak.amount - reference_amount(1.0, 2.0)).abs() < 1e-12,
        "off peak uses the sheet's base cost, got {off_peak:?}"
    );
    assert!(
        (peak.amount - reference_amount(2.0, 4.0)).abs() < 1e-12,
        "the sheet's peak tariff doubles it, got {peak:?}"
    );
}

/// A sheet's long-context tier has to be visible to the caller that memoizes a
/// derived price, or a long call's higher rates would stay cached for the next
/// short one (the trap models.dev's `context_over_200k` tier has).
#[test]
fn a_sheets_long_context_tier_is_reported_and_billed() {
    let env = Env::new();
    env.save_models_dev();
    let url = env.sheet_url(
        "tiers.json",
        r#"{"deepseek":{"models":{"deepseek-v4-pro":{
            "cost":{"input":1.0,"output":2.0},
            "context_tiers":[{"min_input_tokens":200000,"multiplier":2.0}]
        }}}}"#,
    );
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "tiers"
url = "{url}"
"#
    ));

    let now = SystemTime::now();
    assert_eq!(
        crate::model_pricing::derived_context_tier_in_force(
            "deepseek",
            "deepseek-v4-pro",
            now,
            Some(200_000)
        ),
        None,
        "exactly the threshold is still the base tier"
    );
    assert_eq!(
        crate::model_pricing::derived_context_tier_in_force(
            "deepseek",
            "deepseek-v4-pro",
            now,
            Some(200_001)
        ),
        Some(200_000)
    );

    let base = crate::model_pricing::effective_entry_at_size(
        "deepseek",
        "deepseek-v4-pro",
        now,
        Some(100_000),
    )
    .expect("priced");
    let long = crate::model_pricing::effective_entry_at_size(
        "deepseek",
        "deepseek-v4-pro",
        now,
        Some(300_000),
    )
    .expect("priced");
    assert_eq!(base.0.cost.input, Some(1.0));
    assert_eq!(base.0.cost.output, Some(2.0));
    assert_eq!(long.0.cost.input, Some(2.0), "the sheet's tier doubles it");
    assert_eq!(long.0.cost.output, Some(4.0));

    // With no sheet, the models.dev tier answers instead (here: none).
    env.write_config("");
    assert_eq!(
        crate::model_pricing::derived_context_tier_in_force(
            "deepseek",
            "deepseek-v4-pro",
            now,
            Some(300_000)
        ),
        None
    );
}

/// A source that keeps failing must not be re-read (and re-logged) on every
/// price lookup: the route catalog asks once per route. The cost of that is a
/// short deferral, spelled out here.
#[test]
fn a_failing_source_is_not_retried_on_every_lookup() {
    let env = Env::new();
    env.save_models_dev();
    let missing = env.dir.path().join("not-there.json");
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "missing"
url = "file://{}"
"#,
        missing.display()
    ));

    assert_eq!(priced_by("deepseek", "deepseek-v4-pro"), None);
    assert!(
        in_backoff_for_tests("missing"),
        "the failure buys a backoff window"
    );

    // The user fixes the file. The retry is deferred by the backoff window
    // (holding a lookup off for a minute beats re-reading and re-logging on
    // every route), and the lookup still degrades in the meantime.
    std::fs::write(
        &missing,
        sheet_body("deepseek", "deepseek-v4-pro", 9.0, 18.0),
    )
    .expect("write sheet");
    assert_eq!(
        priced_by("deepseek", "deepseek-v4-pro"),
        None,
        "a freshly failed source is left alone for the backoff window"
    );
    let priced =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("models.dev still prices the call");
    assert!((priced.amount - MODELS_DEV_REFERENCE).abs() < 1e-12);

    // Once the window is over the fixed sheet is picked up.
    forget_failures_for_tests();
    assert_eq!(
        priced_by("deepseek", "deepseek-v4-pro").as_deref(),
        Some("missing")
    );
}
