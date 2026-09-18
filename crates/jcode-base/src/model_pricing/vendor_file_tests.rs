//! `[pricing.providers.<vendor>].file` vendor price file tests.
//!
//! Everything here runs in an isolated `JCODE_HOME` and uses local vendor files
//! or a primed cache, so the model-id matching, the mtime freshness and the
//! failure paths are all exercised deterministically.

use super::entry::ModelPricingEntry;
use super::vendor_files::{
    MAX_VENDOR_FILE_BYTES_TEST as MAX_VENDOR_FILE_BYTES, cached_fingerprint_for_tests,
    cached_vendor_ids_for_tests, clear_vendor_files_cache_for_tests, forget_failures_for_tests,
    in_backoff_for_tests, local_file_reads_for_tests, read_local_file,
    reset_local_file_reads_for_tests,
};
use super::{ModelCost, clear_memory_cache_for_tests, save_test_cache};
use std::ffi::OsString;
use std::time::SystemTime;

/// The models.dev snapshot every test starts from: DeepSeek at $0.66/$1.98 per
/// Mtok, so a vendor file's own numbers are unmistakable.
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
        clear_vendor_files_cache_for_tests();
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

    /// Write a vendor file next to the config and return its path.
    fn vendor_path(&self, name: &str, body: &str) -> String {
        let path = self.dir.path().join(name);
        std::fs::write(&path, body).expect("write vendor file");
        path.display().to_string()
    }

    fn save_models_dev(&self) {
        save_test_cache(MODELS_DEV);
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        clear_memory_cache_for_tests();
        clear_vendor_files_cache_for_tests();
        crate::config::invalidate_config_cache();
        match self.previous_home.take() {
            Some(previous) => crate::env::set_var("JCODE_HOME", previous),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }
}

/// A vendor file body naming one model, in the only accepted shape.
fn vendor_body(model: &str, input: f64, output: f64) -> String {
    r#"{"models":{"MODEL":{"cost":{"input":INPUT,"output":OUTPUT}}}}"#
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

/// The vendor that prices the model, or `None` when the file layer has nothing
/// to say. This is the layer-level probe the ordering tests use.
fn priced_by(provider: &str, model: &str) -> Option<String> {
    super::vendor_file_card_at_size(provider, model, SystemTime::now(), None)
        .map(|card| card.vendor)
}

/// The whole precedence chain in one test: an inline card outranks a vendor
/// file, which outranks models.dev.
#[test]
fn a_card_outranks_a_file_which_outranks_models_dev() {
    let env = Env::new();
    env.save_models_dev();
    let path = env.vendor_path("prices.json", &vendor_body("deepseek-v4-pro", 5.0, 10.0));

    // 1. Only the file is configured: it prices the model.
    env.write_config(&format!(
        "[pricing.providers.deepseek]\nfile = \"{path}\"\n"
    ));
    let file_price =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("the file prices the model");
    assert!(
        (file_price.amount - reference_amount(5.0, 10.0)).abs() < 1e-12,
        "the file's own rates must win over models.dev, got {file_price:?}"
    );

    // 2. Add an inline card for the same model: it outranks the file.
    env.write_config(&format!(
        "[pricing.providers.deepseek]\nfile = \"{path}\"\n\n\
         [pricing.providers.deepseek.models.\"deepseek-v4-pro\".cost]\ninput = 7.0\noutput = 14.0\n"
    ));
    let card_price =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("the card prices the model");
    assert!(
        (card_price.amount - reference_amount(7.0, 14.0)).abs() < 1e-12,
        "the inline card must outrank the file, got {card_price:?}"
    );

    // 3. Remove every rule: models.dev is the last layer.
    env.write_config("");
    let chain_price =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("models.dev prices the model");
    assert!(
        (chain_price.amount - reference_amount(0.66, 1.98)).abs() < 1e-12,
        "without a rule the derived chain answers, got {chain_price:?}"
    );
}

/// The feature the whole increment exists for: a DeepSeek file prices a
/// `deepseek-flash` call even when the route is OpenRouter.
#[test]
fn a_file_rule_applies_by_model_id_regardless_of_route() {
    let env = Env::new();
    env.save_models_dev();
    let path = env.vendor_path("deepseek.json", &vendor_body("deepseek-flash", 1.0, 4.0));
    env.write_config(&format!(
        "[pricing.providers.deepseek]\nfile = \"{path}\"\ncurrency = \"CNY\"\n"
    ));

    let price =
        crate::model_pricing::effective_cost("openrouter", "deepseek-flash", SystemTime::now())
            .expect("the DeepSeek file prices the model reached through OpenRouter");
    assert_eq!(
        price.currency.as_str(),
        "CNY",
        "the file's currency follows the price, got {price:?}"
    );
    assert!(
        (price.amount - reference_amount(1.0, 4.0)).abs() < 1e-12,
        "the OpenRouter route must reach the DeepSeek file, got {price:?}"
    );

    // The route key does not gate the file: the same model id under a vendor
    // the user named something else is still found.
    assert_eq!(
        priced_by("openrouter", "deepseek-flash").as_deref(),
        Some("deepseek")
    );
}

/// Two vendors naming the same model must resolve deterministically, and a
/// later vendor's file must not merge fields into the winner.
#[test]
fn two_vendors_naming_one_model_resolve_in_lexicographic_order() {
    let env = Env::new();
    env.save_models_dev();
    let alpha = env.vendor_path("alpha.json", &vendor_body("deepseek-v4-pro", 3.0, 6.0));
    let zeta = env.vendor_path("zeta.json", &vendor_body("deepseek-v4-pro", 9.0, 18.0));
    env.write_config(&format!(
        "[pricing.providers.zeta]\nfile = \"{zeta}\"\n\n\
         [pricing.providers.alpha]\nfile = \"{alpha}\"\n"
    ));

    assert_eq!(
        priced_by("deepseek", "deepseek-v4-pro").as_deref(),
        Some("alpha"),
        "the lexicographically first vendor wins"
    );
    let price =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("a vendor prices the model");
    assert!(
        (price.amount - reference_amount(3.0, 6.0)).abs() < 1e-12,
        "no merging across vendors, got {price:?}"
    );
}

#[test]
fn the_legacy_vendor_keyed_file_is_rejected_with_a_clear_message() {
    let env = Env::new();
    let legacy = env.vendor_path(
        "legacy.json",
        r#"{"deepseek":{"models":{"deepseek-v4-pro":{"cost":{"input":1.0,"output":2.0}}}}}"#,
    );
    let error = read_local_file(std::path::Path::new(&legacy))
        .expect_err("the old vendor-keyed shape is rejected");
    let message = format!("{error:#}");
    assert!(
        message.contains("deepseek") && message.contains("models"),
        "the error must name the offending outer key: {message}"
    );
    assert!(
        message.contains("no outer provider key"),
        "the error must say what to do: {message}"
    );
}

#[test]
fn a_file_without_a_top_level_models_map_is_rejected() {
    let env = Env::new();
    let path = env.vendor_path("empty.json", "{}");
    let error = read_local_file(std::path::Path::new(&path))
        .expect_err("a file without `models` is invalid");
    assert!(
        format!("{error:#}").contains("models"),
        "the error must name the missing field: {error:#}"
    );

    // An explicitly empty map is valid and contributes nothing.
    let empty = env.vendor_path("models-empty.json", r#"{"models":{}}"#);
    assert!(
        read_local_file(std::path::Path::new(&empty))
            .expect("an empty models map is valid")
            .is_empty()
    );
}

#[test]
fn a_file_rule_is_validated_like_a_card() {
    let env = Env::new();
    let path = env.vendor_path(
        "bad.json",
        r#"{"models":{"deepseek-v4-pro":{"cost":{"input":-5.0,"output":1.0}}}}"#,
    );
    let error = read_local_file(std::path::Path::new(&path))
        .expect_err("a negative rate is rejected exactly as in a card");
    assert!(
        format!("{error:#}").contains("input"),
        "the error must name the field: {error:#}"
    );
}

#[test]
fn a_missing_vendor_file_falls_through_to_models_dev() {
    let env = Env::new();
    env.save_models_dev();
    let missing = env.dir.path().join("not-there.json");
    env.write_config(&format!(
        "[pricing.providers.deepseek]\nfile = \"{}\"\n",
        missing.display()
    ));

    let price =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("models.dev prices the model when the file is missing");
    assert!(
        price.currency.is_usd() && (price.amount - reference_amount(0.66, 1.98)).abs() < 1e-12,
        "a missing file must fall through, got {price:?}"
    );
}

#[test]
fn a_malformed_vendor_file_falls_through_to_models_dev() {
    let env = Env::new();
    env.save_models_dev();
    let path = env.vendor_path("broken.json", "{ this is not json");
    env.write_config(&format!(
        "[pricing.providers.deepseek]\nfile = \"{path}\"\n"
    ));

    let price =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("models.dev prices the model when the file is malformed");
    assert!(
        (price.amount - reference_amount(0.66, 1.98)).abs() < 1e-12,
        "a malformed file must fall through, got {price:?}"
    );
}

#[test]
fn an_oversized_vendor_file_is_refused_before_it_is_buffered() {
    let env = Env::new();
    let path = env.dir.path().join("huge.json");
    std::fs::write(&path, vec![b' '; MAX_VENDOR_FILE_BYTES + 1]).expect("write oversized file");
    let error = read_local_file(&path).expect_err("an oversized file is refused");
    assert!(
        format!("{error:#}").contains("larger than"),
        "the size error must be explicit: {error:#}"
    );
}

#[test]
fn a_non_regular_vendor_file_path_is_refused_without_being_opened() {
    let env = Env::new();
    let dir = env.dir.path().join("a-directory");
    std::fs::create_dir_all(&dir).expect("create directory");
    let error = read_local_file(&dir).expect_err("a directory is not a price file");
    assert!(
        format!("{error:#}").contains("not a regular file"),
        "the error must say why: {error:#}"
    );
}

#[test]
fn a_bare_name_vendor_file_is_read_from_the_jcode_cache_dir() {
    let env = Env::new();
    env.save_models_dev();
    let cache_dir = env.dir.path().join("cache");
    std::fs::create_dir_all(&cache_dir).expect("create cache dir");
    std::fs::write(
        cache_dir.join("deepseek.json"),
        vendor_body("deepseek-v4-pro", 2.0, 4.0),
    )
    .expect("write cache vendor file");
    env.write_config("[pricing.providers.deepseek]\nfile = \"deepseek.json\"\n");

    let price =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("the bare-name file under the cache dir prices the model");
    assert!(
        (price.amount - reference_amount(2.0, 4.0)).abs() < 1e-12,
        "a bare name must resolve under ~/.jcode/cache/, got {price:?}"
    );
}

#[test]
fn an_edited_vendor_file_is_picked_up_at_the_next_lookup() {
    let env = Env::new();
    env.save_models_dev();
    let path = env.vendor_path("prices.json", &vendor_body("deepseek-v4-pro", 1.0, 2.0));
    env.write_config(&format!(
        "[pricing.providers.deepseek]\nfile = \"{path}\"\n"
    ));

    let before =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("the file prices the model");
    assert!((before.amount - reference_amount(1.0, 2.0)).abs() < 1e-12);

    let path = std::path::PathBuf::from(&path);
    std::fs::write(&path, vendor_body("deepseek-v4-pro", 3.0, 6.0)).expect("rewrite the file");
    set_modified(&path, 1_900_000_000);

    let after =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("the edited file still prices the model");
    assert!(
        (after.amount - reference_amount(3.0, 6.0)).abs() < 1e-12,
        "an edited local file must be re-read at the next lookup, got {after:?}"
    );
}

/// Force a file's mtime so freshness tests do not depend on clock granularity.
fn set_modified(path: &std::path::Path, secs: u64) {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open the file to set its mtime");
    file.set_times(
        std::fs::FileTimes::new()
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs)),
    )
    .expect("set the file's mtime");
}

#[test]
fn an_unchanged_vendor_file_is_not_reread_on_every_lookup() {
    let env = Env::new();
    env.save_models_dev();
    let path = env.vendor_path("prices.json", &vendor_body("deepseek-v4-pro", 9.0, 18.0));
    env.write_config(&format!(
        "[pricing.providers.deepseek]\nfile = \"{path}\"\n"
    ));

    reset_local_file_reads_for_tests();
    assert_eq!(
        priced_by("deepseek", "deepseek-v4-pro").as_deref(),
        Some("deepseek")
    );
    let reads_after_first = local_file_reads_for_tests();
    assert!(
        reads_after_first >= 1,
        "the first lookup reads the file, got {reads_after_first}"
    );

    for _ in 0..8 {
        assert_eq!(
            priced_by("deepseek", "deepseek-v4-pro").as_deref(),
            Some("deepseek")
        );
    }
    assert_eq!(
        local_file_reads_for_tests(),
        reads_after_first,
        "an unchanged local file must be stat'ed, not re-read"
    );
    let (mtime, size) =
        cached_fingerprint_for_tests("deepseek").expect("the copy is fingerprinted");
    assert!(mtime > 0 && size > 0, "mtime {mtime}, size {size}");
}

#[test]
fn a_missing_vendor_file_costs_one_attempt_per_backoff_not_one_per_lookup() {
    let env = Env::new();
    env.save_models_dev();
    let missing = env.dir.path().join("not-there.json");
    env.write_config(&format!(
        "[pricing.providers.deepseek]\nfile = \"{}\"\n",
        missing.display()
    ));

    reset_local_file_reads_for_tests();
    assert_eq!(priced_by("deepseek", "deepseek-v4-pro"), None);
    let attempts = local_file_reads_for_tests();
    assert_eq!(attempts, 1, "the first lookup attempts the file once");
    assert!(
        in_backoff_for_tests("deepseek"),
        "the failure buys a backoff"
    );

    for _ in 0..8 {
        assert_eq!(priced_by("deepseek", "deepseek-v4-pro"), None);
    }
    assert_eq!(
        local_file_reads_for_tests(),
        attempts,
        "the backoff window prevents re-reading a missing file"
    );

    // Once the window is over, the attempt is made again rather than never.
    forget_failures_for_tests();
    assert_eq!(priced_by("deepseek", "deepseek-v4-pro"), None);
    assert_eq!(local_file_reads_for_tests(), attempts + 1);
}

#[test]
fn a_file_rule_that_is_out_of_effect_is_skipped_and_labelled_with_the_vendor() {
    let env = Env::new();
    env.save_models_dev();
    let path = env.vendor_path(
        "expired.json",
        r#"{"models":{"deepseek-v4-pro":{"cost":{"input":9.0,"output":18.0},"effective_until":"2020-01-01T00:00:00Z"}}}"#,
    );
    env.write_config(&format!(
        "[pricing.providers.deepseek]\nfile = \"{path}\"\n"
    ));

    assert_eq!(
        priced_by("deepseek", "deepseek-v4-pro"),
        None,
        "an out-of-effect file must not price the call"
    );
    let notice =
        crate::model_pricing::vendor_file_rule_out_of_effect("deepseek-v4-pro", SystemTime::now())
            .expect("the out-of-effect file rule is reported");
    let label = notice.label();
    assert!(
        label.contains("expired") && label.contains("deepseek"),
        "the marker must name the vendor: {label}"
    );

    // The next layer prices the call anyway.
    let price =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("models.dev prices the call");
    assert!((price.amount - reference_amount(0.66, 1.98)).abs() < 1e-12);
}

#[test]
fn a_foreign_currency_file_prices_in_its_own_currency_never_in_another() {
    let env = Env::new();
    env.save_models_dev();
    let path = env.vendor_path(
        "cny.json",
        r#"{"models":{"deepseek-v4-pro":{"cost":{"input":4.5,"output":13.5}}}}"#,
    );
    env.write_config(&format!(
        "[pricing.providers.deepseek]\nfile = \"{path}\"\ncurrency = \"CNY\"\n"
    ));

    let price =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("the complete CNY file prices the call alone");
    assert_eq!(price.currency.as_str(), "CNY");
    assert!((price.amount - reference_amount(4.5, 13.5)).abs() < 1e-12);
}

#[test]
fn an_incomplete_foreign_currency_file_does_not_relabel_models_dev() {
    let env = Env::new();
    env.save_models_dev();
    let path = env.vendor_path(
        "partial-cny.json",
        r#"{"models":{"deepseek-v4-pro":{"cost":{"input":4.5}}}}"#,
    );
    env.write_config(&format!(
        "[pricing.providers.deepseek]\nfile = \"{path}\"\ncurrency = \"CNY\"\n"
    ));

    assert_eq!(
        priced_by("deepseek", "deepseek-v4-pro"),
        None,
        "an incomplete foreign-currency file must not price the call"
    );
    let price =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("models.dev prices the call instead");
    assert!(
        price.currency.is_usd() && (price.amount - reference_amount(0.66, 1.98)).abs() < 1e-12,
        "a USD fallback must not be relabelled CNY, got {price:?}"
    );
}

#[test]
fn a_vendor_file_can_state_peak_hours_like_a_hand_written_card() {
    let env = Env::new();
    env.save_models_dev();
    let path = env.vendor_path(
        "peak.json",
        r#"{"models":{"deepseek-v4-pro":{
           "cost":{"input":1.0,"output":2.0},
           "tariffs":{"peak":{"multiplier":10.0}},
           "schedule":[{"tariff":"peak","utc_offset_minutes":0,"weekdays":["Mon","Tue","Wed","Thu","Fri"],"windows":[["01:00","04:00"]]}]
        }}}"#,
    );
    env.write_config(&format!(
        "[pricing.providers.deepseek]\nfile = \"{path}\"\n"
    ));

    // 2030-06-22T02:00:00Z (Saturday) and 2030-06-24T02:00:00Z (Monday peak).
    let instant = |secs: u64| std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
    let off_peak =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", instant(1_908_324_000))
            .expect("the file prices an off-peak call");
    let peak =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", instant(1_908_496_800))
            .expect("the file prices a peak call");
    assert!((off_peak.amount - reference_amount(1.0, 2.0)).abs() < 1e-12);
    assert!(
        (peak.amount - reference_amount(10.0, 20.0)).abs() < 1e-12,
        "the file's schedule must select the peak tariff, got {peak:?}"
    );
}

#[test]
fn the_derived_price_identity_carries_the_vendor_file_and_tariff() {
    let env = Env::new();
    env.save_models_dev();
    let path = env.vendor_path(
        "peak.json",
        r#"{"models":{"deepseek-v4-pro":{
           "cost":{"input":1.0,"output":2.0},
           "tariffs":{"peak":{"multiplier":10.0}},
           "schedule":[{"tariff":"peak","utc_offset_minutes":0,"weekdays":["Mon","Tue","Wed","Thu","Fri"],"windows":[["01:00","04:00"]]}]
        }}}"#,
    );
    env.write_config(&format!(
        "[pricing.providers.deepseek]\nfile = \"{path}\"\n"
    ));

    let instant = |secs: u64| std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
    let off_peak = crate::model_pricing::derived_price_identity(
        "deepseek",
        "deepseek-v4-pro",
        instant(1_908_324_000),
        None,
    );
    let peak = crate::model_pricing::derived_price_identity(
        "deepseek",
        "deepseek-v4-pro",
        instant(1_908_496_800),
        None,
    );
    assert_ne!(
        off_peak.vendor_file, peak.vendor_file,
        "the tariff in force must be part of the derived price identity"
    );
    assert!(
        peak.vendor_file
            .as_deref()
            .is_some_and(|id| id.starts_with("deepseek")),
        "the identity must name the vendor: {peak:?}"
    );
}

#[test]
fn a_vendors_long_context_tier_is_billed_for_a_large_call() {
    let env = Env::new();
    env.save_models_dev();
    let path = env.vendor_path(
        "tiers.json",
        r#"{"models":{"deepseek-v4-pro":{
           "cost":{"input":1.0,"output":2.0},
           "context_tiers":[{"min_input_tokens":200000,"multiplier":10.0}]
        }}}"#,
    );
    env.write_config(&format!(
        "[pricing.providers.deepseek]\nfile = \"{path}\"\n"
    ));

    let at = SystemTime::now();
    let small = crate::model_pricing::effective_entry_at_size(
        "deepseek",
        "deepseek-v4-pro",
        at,
        Some(100_000),
    )
    .expect("the base tier prices a small call");
    let large = crate::model_pricing::effective_entry_at_size(
        "deepseek",
        "deepseek-v4-pro",
        at,
        Some(300_000),
    )
    .expect("the long-context tier prices a large call");
    assert_eq!(small.0.cost.input, Some(1.0));
    assert_eq!(
        large.0.cost.input,
        Some(10.0),
        "a call above the threshold must use the higher tier"
    );
    assert_eq!(
        crate::model_pricing::derived_price_identity(
            "deepseek",
            "deepseek-v4-pro",
            at,
            Some(300_000)
        )
        .context_tier,
        Some(200_000),
    );
}

#[test]
fn the_vendor_file_cache_is_keyed_by_vendor() {
    let env = Env::new();
    env.save_models_dev();
    let path = env.vendor_path("prices.json", &vendor_body("deepseek-v4-pro", 1.0, 2.0));
    env.write_config(&format!(
        "[pricing.providers.deepseek]\nfile = \"{path}\"\n"
    ));
    assert_eq!(
        priced_by("deepseek", "deepseek-v4-pro").as_deref(),
        Some("deepseek")
    );
    assert_eq!(cached_vendor_ids_for_tests(), vec!["deepseek".to_string()]);

    // A primed vendor with no file is still a valid rule (the file is optional).
    super::save_test_vendor(
        "primed",
        std::path::Path::new("/nope.json"),
        &[("deepseek-v4-pro", cost(4.0, 8.0))],
    );
    assert_eq!(
        priced_by("deepseek", "deepseek-v4-pro").as_deref(),
        Some("deepseek")
    );
    assert_eq!(
        cached_vendor_ids_for_tests(),
        vec!["deepseek".to_string(), "primed".to_string()]
    );
}

#[test]
fn a_vendor_file_prices_from_its_own_rates_until_the_config_points_elsewhere() {
    let env = Env::new();
    env.save_models_dev();
    let first = env.vendor_path("first.json", &vendor_body("deepseek-v4-pro", 1.0, 2.0));
    let second = env.vendor_path("second.json", &vendor_body("deepseek-v4-pro", 3.0, 6.0));
    env.write_config(&format!(
        "[pricing.providers.deepseek]\nfile = \"{first}\"\n"
    ));
    let one =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("the first file prices the model");
    assert!((one.amount - reference_amount(1.0, 2.0)).abs() < 1e-12);

    // Pointing the same vendor at another file must not keep serving the old
    // one: the cached copy records the path it came from.
    env.write_config(&format!(
        "[pricing.providers.deepseek]\nfile = \"{second}\"\n"
    ));
    let two =
        crate::model_pricing::effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
            .expect("the second file prices the model");
    assert!(
        (two.amount - reference_amount(3.0, 6.0)).abs() < 1e-12,
        "a path change must invalidate the cached copy, got {two:?}"
    );
}
