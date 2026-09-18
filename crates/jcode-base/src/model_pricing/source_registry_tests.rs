//! `[[pricing.sources]]` registry tests.
//!
//! Everything here runs in an isolated `JCODE_HOME` and uses local sheet files
//! or a primed cache, so the merge order, the mtime freshness and the failure
//! paths are all exercised deterministically.

use super::ModelCost;
use super::entry::ModelPricingEntry;
use super::source_registry::{
    MAX_SHEET_BYTES_TEST as MAX_SHEET_BYTES, cached_fingerprint_for_tests,
    cached_source_ids_for_tests, clear_sources_cache_for_tests, forget_failures_for_tests,
    in_backoff_for_tests, local_sheet_reads_for_tests, read_local_sheet,
    reset_local_sheet_reads_for_tests, save_catalog, source_card,
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

    /// Write a sheet next to the config and return its path.
    fn sheet_path(&self, name: &str, body: &str) -> String {
        let path = self.dir.path().join(name);
        std::fs::write(&path, body).expect("write sheet");
        path.display().to_string()
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
    let url = env.sheet_path(
        "mirror.json",
        &sheet_body("deepseek", "deepseek-v4-pro", 9.0, 18.0),
    );
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "mirror"
file = "{url}"
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
    let url = env.sheet_path(
        "mirror.json",
        &sheet_body("deepseek", "deepseek-v4-pro", 9.0, 18.0),
    );
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "mirror"
file = "{url}"

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
    let url = env.sheet_path(
        "openrouter.json",
        &sheet_body("openrouter", "some-model", 3.0, 4.0),
    );
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "openrouter-only"
file = "{url}"
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
    let url = env.sheet_path(
        "deepseek.json",
        &sheet_body("deepseek", "deepseek-v4-pro", 9.0, 18.0),
    );
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "ds"
file = "{url}"
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
    let url = env.sheet_path(
        "gateway.json",
        &sheet_body("my-gateway", "gw-model", 5.0, 6.0),
    );
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "gateway"
file = "{url}"
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
    let url = env.sheet_path(
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
file = "{url}"
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
    let losing = env.sheet_path(
        "losing.json",
        &sheet_body("deepseek", "deepseek-v4-pro", 9.0, 18.0),
    );
    let winning = env.sheet_path(
        "winning.json",
        &sheet_body("deepseek", "deepseek-v4-pro", 3.0, 6.0),
    );

    // Lower priority number wins, whichever order the file lists them in.
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "alpha"
file = "{losing}"
priority = 10

[[pricing.sources]]
id = "zulu"
file = "{winning}"
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
file = "{losing}"

[[pricing.sources]]
id = "alpha"
file = "{winning}"
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
    let complete = env.sheet_path(
        "complete.json",
        &sheet_body("deepseek", "deepseek-v4-pro", 9.0, 18.0),
    );
    // The winner's card is primed (the parser refuses a card with no output
    // rate), so its file has to exist for the primed copy to be fresh.
    let winner_path = env.dir.path().join("winner.json");
    std::fs::write(&winner_path, "{}").expect("write the winner's file");
    let winner_url = winner_path.display().to_string();
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "winner"
file = "{winner_url}"
priority = 0

[[pricing.sources]]
id = "filler"
file = "{complete}"
priority = 10
"#
    ));
    save_test_source(
        "winner",
        &winner_path,
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
fn a_missing_local_sheet_falls_through_to_models_dev() {
    let env = Env::new();
    env.save_models_dev();
    let missing = env.dir.path().join("not-there.json");
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "missing"
file = "{}"
"#,
        missing.display()
    ));

    let priced =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("models.dev still prices the call");
    assert!((priced.amount - MODELS_DEV_REFERENCE).abs() < 1e-12);
}

/// A `file://` value is not a synonym for anything: it is simply a path-like
/// string that does not exist on disk, so the source contributes no rules and
/// the call falls through to the next layer.
#[test]
fn a_file_url_value_is_just_a_path_that_does_not_exist() {
    let env = Env::new();
    env.save_models_dev();
    env.write_config(
        r#"
[[pricing.sources]]
id = "scheme"
file = "file:///nowhere/prices.json"
"#,
    );

    assert_eq!(priced_by("deepseek", "deepseek-v4-pro"), None);
    let priced =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("models.dev still prices the call");
    assert!((priced.amount - MODELS_DEV_REFERENCE).abs() < 1e-12);
}

#[test]
fn a_malformed_sheet_falls_through_to_models_dev() {
    let env = Env::new();
    env.save_models_dev();
    let url = env.sheet_path("broken.json", "{ this is not json ");
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "broken"
file = "{url}"
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
    let url = env.sheet_path(
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
file = "{url}"
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
    let url = env.sheet_path(
        "live.json",
        &sheet_body("deepseek", "deepseek-v4-pro", 9.0, 18.0),
    );
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "live"
file = "{url}"
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
    let url = env.sheet_path(
        "cny.json",
        &sheet_body("deepseek", "deepseek-v4-pro", 12.0, 24.0),
    );
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "cny"
file = "{url}"
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
    // The entry is primed rather than parsed (the parser refuses a card with no
    // output rate), so the file it is primed against has to exist for the cache
    // to look like a copy this process just read.
    let path = env.dir.path().join("incomplete.json");
    std::fs::write(&path, "{}").expect("write the primed sheet's file");
    let location = path.display().to_string();
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "incomplete"
file = "{location}"
currency = "CNY"
"#
    ));
    save_test_source(
        "incomplete",
        &path,
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
    let url = env.sheet_path(
        "mirror.json",
        &sheet_body("deepseek", "deepseek-v4-pro", 9.0, 18.0),
    );
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "mirror"
file = "{url}"
"#
    ));

    // This is the path billing takes when no hand-written card claims the call.
    let estimate = crate::provider::pricing::derived_pricing_for_source_at_size(
        "deepseek",
        "deepseek-v4-pro",
        None,
        SystemTime::now(),
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
        SystemTime::now(),
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
    let url = env.sheet_path(
        "mirror.json",
        &sheet_body("deepseek", "deepseek-v4-pro", 9.0, 18.0),
    );
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "mirror"
file = "{url}"

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
    let url = env.sheet_path(
        "cny.json",
        &sheet_body("deepseek", "deepseek-v4-pro", 12.0, 24.0),
    );
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "cny"
file = "{url}"
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
file = ""
"#,
    );

    let error = crate::model_pricing::pricing_config_error().expect("reported");
    assert_eq!(error.field_path, "pricing.sources[0].file");

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
    let url = env.sheet_path(
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
file = "{url}"
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

/// F-A at the base layer: the derived billing layer (curated tables, sheets,
/// models.dev) prices a sheet-priced call at the *call's* instant, not the wall
/// clock, so an ongoing session follows the schedule across a window boundary.
#[test]
fn the_derived_billing_layer_reads_a_sheet_schedule_at_the_call_instant() {
    let env = Env::new();
    env.save_models_dev();
    let url = env.sheet_path(
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
file = "{url}"
"#
    ));

    let instant = |secs: u64| std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
    // 2030-06-22T02:00:00Z (Saturday) and 2030-06-24T02:00:00Z (Monday, inside
    // the 01:00-04:00 UTC weekday window). Both are years away from the wall
    // clock, so a "read the clock" implementation cannot satisfy both.
    let off_peak = crate::provider::pricing::derived_pricing_for_source_at_size(
        "deepseek",
        "deepseek-v4-pro",
        None,
        instant(1_908_324_000),
        None,
    )
    .expect("priced");
    let peak = crate::provider::pricing::derived_pricing_for_source_at_size(
        "deepseek",
        "deepseek-v4-pro",
        None,
        instant(1_908_496_800),
        None,
    )
    .expect("priced");
    assert_eq!(
        off_peak.input_price_per_mtok_micros,
        Some(1_000_000),
        "off peak prices the sheet's base rate"
    );
    assert_eq!(
        peak.input_price_per_mtok_micros,
        Some(2_000_000),
        "the peak window doubles the sheet's input rate"
    );
}

/// A sheet's long-context tier has to be visible to the caller that memoizes a
/// derived price, or a long call's higher rates would stay cached for the next
/// short one (the trap models.dev's `context_over_200k` tier has).
#[test]
fn a_sheets_long_context_tier_is_reported_and_billed() {
    let env = Env::new();
    env.save_models_dev();
    let url = env.sheet_path(
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
file = "{url}"
"#
    ));

    let now = SystemTime::now();
    assert_eq!(
        crate::model_pricing::derived_price_identity(
            "deepseek",
            "deepseek-v4-pro",
            now,
            Some(200_000)
        )
        .context_tier,
        None,
        "exactly the threshold is still the base tier"
    );
    assert_eq!(
        crate::model_pricing::derived_price_identity(
            "deepseek",
            "deepseek-v4-pro",
            now,
            Some(200_001)
        )
        .context_tier,
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
        crate::model_pricing::derived_price_identity(
            "deepseek",
            "deepseek-v4-pro",
            now,
            Some(300_000)
        )
        .context_tier,
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
file = "{}"
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

/// Two provider keys in one sheet can both match the caller's identity
/// (`claude` and `anthropic-api` both map to models.dev `anthropic`, which the
/// sheet does not key), so the last-resort match has to be deterministic. A
/// fresh `Env` per iteration gives the sheet's provider map a fresh `HashMap`
/// seed, so hash-order dependence would show up as disagreement between
/// iterations; the documented choice is the lexicographically smallest key
/// (`anthropic-api`).
#[test]
fn two_matching_sheet_provider_keys_always_pick_the_same_section() {
    for iteration in 0..16 {
        let env = Env::new();
        let url = env.sheet_path(
            "mirror.json",
            r#"{"claude":{"models":{"claude-fable-5":{"cost":{"input":9.0,"output":9.0}}}},
                "anthropic-api":{"models":{"claude-fable-5":{"cost":{"input":1.0,"output":2.0}}}}}"#,
        );
        env.write_config(&format!(
            r#"
[[pricing.sources]]
id = "mirror"
file = "{url}"
"#
        ));
        let hit = source_card("claude:api-key", "claude-fable-5", SystemTime::now())
            .expect("a sheet prices the pair");
        assert_eq!(
            hit.entry.cost.input,
            Some(1.0),
            "iteration {iteration}: the lexicographically smallest matching key (`anthropic-api`) must win"
        );
    }
}

/// F1's field-level fallback, one layer down: a USD `[pricing.providers]` card
/// that leaves a field unwritten takes the `[[pricing.sources]]` sheet's value
/// before models.dev's, because a sheet outranks models.dev.
#[test]
fn a_partial_usd_config_card_is_filled_from_a_sheet_before_models_dev() {
    let env = Env::new();
    env.save_models_dev();
    let url = env.sheet_path(
        "mirror.json",
        &sheet_body("deepseek", "deepseek-v4-pro", 3.0, 7.0),
    );
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "mirror"
file = "{url}"

[pricing.providers.deepseek.models."deepseek-v4-pro".cost]
input = 1.5
"#
    ));

    let (entry, currency) =
        crate::model_pricing::effective_entry("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("the card prices the model");

    assert_eq!(entry.cost.input, Some(1.5), "the written field still wins");
    assert_eq!(
        entry.cost.output,
        Some(7.0),
        "the sheet fills the blank before models.dev's 1.98"
    );
    assert!(currency.is_usd());
}

/// The sheet layer must not re-enter itself through the new field-level
/// fallback: a sheet's own card takes models.dev's numbers, never another
/// sheet's.
#[test]
fn a_sheet_card_fills_from_models_dev_and_not_another_sheet() {
    let env = Env::new();
    env.save_models_dev();
    let url = env.sheet_path(
        "mirror.json",
        r#"{"deepseek":{"models":{"deepseek-v4-pro":{"cost":{"input":3.0,"output":7.0}}}}}"#,
    );
    env.write_config(&format!(
        r#"
[[pricing.sources]]
id = "mirror"
file = "{url}"
"#
    ));

    let card = crate::model_pricing::source_card_at_size(
        "deepseek",
        "deepseek-v4-pro",
        SystemTime::now(),
        None,
    )
    .expect("the sheet prices the model");
    assert_eq!(card.source_id, "mirror");
    assert_eq!(card.entry.cost.input, Some(3.0));
    assert_eq!(card.entry.cost.output, Some(7.0));
}

/// A one-model sheet body for the cache tests below.
fn providers_for(
    model: &str,
) -> std::collections::HashMap<String, std::collections::HashMap<String, ModelPricingEntry>> {
    let mut models = std::collections::HashMap::new();
    models.insert(model.to_string(), cost(1.0, 2.0));
    std::collections::HashMap::from([("deepseek".to_string(), models)])
}

fn cache_file(env: &Env) -> std::path::PathBuf {
    env.dir.path().join("cache").join("pricing_sources.json")
}

/// Two different sources' saves must not lose each other. The in-memory `Arc`
/// is the base when it matches the path, so a save that finds the disk file
/// cleared (a second process rewriting it, say) still keeps the sheet this
/// process already holds instead of dropping it.
#[test]
fn a_save_bases_on_memory_and_never_drops_the_other_source() {
    let env = Env::new();
    let location = env.dir.path().join("sheet.json");
    save_catalog("a", &location, None, providers_for("m-a"));
    // The disk file disappears under us; the process still holds `a`.
    std::fs::remove_file(cache_file(&env)).expect("remove cache file");
    save_catalog("b", &location, None, providers_for("m-b"));

    assert_eq!(
        cached_source_ids_for_tests(),
        vec!["a".to_string(), "b".to_string()],
        "the in-memory view must keep both sources"
    );
    let raw = std::fs::read_to_string(cache_file(&env)).expect("cache file written again");
    assert!(raw.contains("\"a\"") && raw.contains("\"b\""), "{raw}");
}

/// The threaded shape of the same guarantee: a burst of saves of different
/// sources all survive in memory and on disk.
#[test]
fn concurrent_saves_of_different_sources_all_survive() {
    let env = Env::new();
    let location = env.dir.path().join("sheet.json");
    let handles: Vec<_> = (0..12)
        .map(|index| {
            let id = format!("s{index}");
            let location = location.clone();
            std::thread::spawn(move || save_catalog(&id, &location, None, providers_for("m")))
        })
        .collect();
    for handle in handles {
        handle.join().expect("save thread");
    }

    let ids = cached_source_ids_for_tests();
    let raw = std::fs::read_to_string(cache_file(&env)).expect("cache file");
    for index in 0..12 {
        let id = format!("s{index}");
        assert!(ids.contains(&id), "memory lost {id}: {ids:?}");
        assert!(raw.contains(&format!("\"{id}\"")), "disk lost {id}: {raw}");
    }
}

/// An oversized local sheet is refused from its metadata, before `open` and
/// before any bytes are buffered. The trap byte past the ceiling means a
/// full read would fail as invalid UTF-8 instead: the size error proves the
/// stat precheck ran.
#[test]
fn an_oversized_local_sheet_is_refused_before_it_is_buffered() {
    let env = Env::new();
    let path = env.dir.path().join("huge.json");
    let mut bytes = vec![b' '; MAX_SHEET_BYTES + 1];
    bytes[MAX_SHEET_BYTES] = 0xFF;
    std::fs::write(&path, &bytes).expect("write oversized sheet");

    let error = read_local_sheet(&path).expect_err("oversized sheet is refused");
    assert!(
        error.to_string().contains("larger than"),
        "the size precheck must fire, got: {error}"
    );
}

/// A `file://` path that names something other than a regular file is refused
/// without being opened: opening a FIFO with no writer would block a lookup
/// forever. A directory is the easily-created stand-in.
#[test]
fn a_non_regular_sheet_path_is_refused_without_being_opened() {
    let env = Env::new();
    let path = env.dir.path().join("sheet-dir");
    std::fs::create_dir(&path).expect("create dir");

    let error = read_local_sheet(&path).expect_err("a directory is not a sheet");
    assert!(
        error.to_string().contains("not a regular file"),
        "the stat precheck must reject it, got: {error}"
    );
}

/// The single-source case: `[[pricing.sources]]` with a `file` and nothing else.
/// The derived id (`prices`, from `prices.json`) is the cache key and the label
/// `/pricing` and the cost line print, so it must be the same on every load.
#[test]
fn an_id_less_local_source_prices_the_call_under_a_stable_derived_id() {
    let env = Env::new();
    env.save_models_dev();
    let url = env.sheet_path(
        "prices.json",
        &sheet_body("deepseek", "deepseek-v4-pro", 9.0, 18.0),
    );
    let config = format!("[[pricing.sources]]\nfile = \"{url}\"\n");
    env.write_config(&config);

    let priced =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("the sheet prices the model");
    assert!(
        (priced.amount - reference_amount(9.0, 18.0)).abs() < 1e-12,
        "an id-less source must still price the call, got {priced:?}"
    );
    assert_eq!(
        priced_by("deepseek", "deepseek-v4-pro").as_deref(),
        Some("prices")
    );

    // Loading the same config again derives the same id and reads the same
    // cache entry instead of creating a second one.
    crate::config::invalidate_config_cache();
    assert_eq!(
        priced_by("deepseek", "deepseek-v4-pro").as_deref(),
        Some("prices")
    );
    assert_eq!(
        cached_source_ids_for_tests(),
        vec!["prices".to_string()],
        "one id-less source must produce exactly one cache key"
    );
}

/// A bare `file` name resolves under `~/.jcode/cache/`, so the one-line form
/// names the price file the user dropped in the cache dir rather than spelling
/// out a path.
#[test]
fn a_bare_name_source_is_read_from_the_jcode_cache_dir() {
    let env = Env::new();
    env.save_models_dev();
    std::fs::create_dir_all(env.dir.path().join("cache")).expect("create cache dir");
    std::fs::write(
        env.dir.path().join("cache").join("deepseek.json"),
        sheet_body("deepseek", "deepseek-v4-pro", 9.0, 18.0),
    )
    .expect("write the bare-name sheet");
    env.write_config("[[pricing.sources]]\nfile = \"deepseek.json\"\n");

    let priced =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("the bare-name sheet prices the model");
    assert!(
        (priced.amount - reference_amount(9.0, 18.0)).abs() < 1e-12,
        "a bare name must resolve under the cache dir, got {priced:?}"
    );
    assert_eq!(
        priced_by("deepseek", "deepseek-v4-pro").as_deref(),
        Some("deepseek")
    );
}

/// A local price file is the user's own file, so editing it has to take effect
/// at the next lookup.
#[test]
fn an_edited_local_sheet_is_picked_up_at_the_next_lookup() {
    let env = Env::new();
    env.save_models_dev();
    let path = env.dir.path().join("prices.json");
    std::fs::write(&path, sheet_body("deepseek", "deepseek-v4-pro", 9.0, 18.0))
        .expect("write the price file");
    env.write_config(&format!(
        r#"
[[pricing.sources]]
file = "{}"
"#,
        path.display()
    ));

    let before =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("the sheet prices the model");
    assert!(
        (before.amount - reference_amount(9.0, 18.0)).abs() < 1e-12,
        "the first read uses the file, got {before:?}"
    );

    // The user edits their price file and saves. The mtime is forced forward so
    // the test does not depend on the filesystem's timestamp granularity.
    std::fs::write(&path, sheet_body("deepseek", "deepseek-v4-pro", 3.0, 6.0))
        .expect("rewrite the price file");
    set_modified(&path, 1_900_000_000);

    let after =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("the sheet still prices the model");
    assert!(
        (after.amount - reference_amount(3.0, 6.0)).abs() < 1e-12,
        "an edited local sheet must be re-read at the next lookup, got {after:?}"
    );
}

/// Force a file's mtime so freshness tests do not depend on clock granularity.
fn set_modified(path: &std::path::Path, secs: u64) {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open the sheet to set its mtime");
    file.set_times(
        std::fs::FileTimes::new()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs)),
    )
    .expect("set the sheet's mtime");
}

/// The freshness check costs a `stat`, never a re-read: an unchanged local file
/// must not be opened again on every price lookup, because the route catalog
/// asks for a price once per route.
#[test]
fn an_unchanged_local_sheet_is_not_reread_on_every_lookup() {
    let env = Env::new();
    env.save_models_dev();
    let url = env.sheet_path(
        "prices.json",
        &sheet_body("deepseek", "deepseek-v4-pro", 9.0, 18.0),
    );
    env.write_config(&format!("[[pricing.sources]]\nfile = \"{url}\"\n"));

    reset_local_sheet_reads_for_tests();
    assert_eq!(
        priced_by("deepseek", "deepseek-v4-pro").as_deref(),
        Some("prices")
    );
    let reads_after_first = local_sheet_reads_for_tests();
    assert!(
        reads_after_first >= 1,
        "the first lookup reads the file, got {reads_after_first}"
    );

    for _ in 0..8 {
        assert_eq!(
            priced_by("deepseek", "deepseek-v4-pro").as_deref(),
            Some("prices")
        );
    }
    assert_eq!(
        local_sheet_reads_for_tests(),
        reads_after_first,
        "an unchanged local sheet must be stat'ed, not re-read"
    );
    let (mtime, size) = cached_fingerprint_for_tests("prices").expect("the copy is fingerprinted");
    assert!(mtime > 0 && size > 0, "mtime {mtime}, size {size}");
}

/// A source that cannot be read at all must not cost a syscall per lookup: the
/// failure backoff is what bounds a missing file, exactly as it bounds a failing
/// fetch.
#[test]
fn a_missing_local_sheet_costs_one_attempt_per_backoff_not_one_per_lookup() {
    let env = Env::new();
    env.save_models_dev();
    let missing = env.dir.path().join("not-there.json");
    env.write_config(&format!(
        "[[pricing.sources]]\nfile = \"{}\"\n",
        missing.display()
    ));

    reset_local_sheet_reads_for_tests();
    assert_eq!(priced_by("deepseek", "deepseek-v4-pro"), None);
    let attempts = local_sheet_reads_for_tests();
    assert_eq!(attempts, 1, "the first lookup attempts the file once");
    assert!(
        in_backoff_for_tests("not-there"),
        "the failure buys a backoff"
    );

    for _ in 0..8 {
        assert_eq!(priced_by("deepseek", "deepseek-v4-pro"), None);
    }
    assert_eq!(
        local_sheet_reads_for_tests(),
        attempts,
        "the backoff window prevents re-reading a missing file"
    );

    // Once the window is over, the attempt is made again rather than never.
    forget_failures_for_tests();
    assert_eq!(priced_by("deepseek", "deepseek-v4-pro"), None);
    assert_eq!(local_sheet_reads_for_tests(), attempts + 1);
}
