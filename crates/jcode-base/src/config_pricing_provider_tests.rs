//! Validation and serialization of `[pricing.providers.<vendor>].file`.
//!
//! Split from `config::pricing` so the production file stays inside the
//! code-size budget; everything here goes through the public `validate` entry
//! point the config loader uses.

use crate::config::pricing::validate;
use jcode_config_types::PricingConfigFile;

fn parse_toml(section: &str) -> PricingConfigFile {
    toml::from_str(section).expect("toml parses into the DTO")
}

/// The one resolved vendor file for a single-provider section.
fn file_of(section: &str) -> std::path::PathBuf {
    let (config, _) = validate(&parse_toml(section)).expect("validates");
    config
        .providers
        .values()
        .next()
        .and_then(|provider| provider.file.clone())
        .expect("the provider declares a file")
}

/// An isolated `JCODE_HOME` (and, optionally, `HOME`), with the test-env lock
/// held for its lifetime. Only the resolution tests that read the environment
/// need this; the rest name a path.
struct Home {
    _lock: std::sync::MutexGuard<'static, ()>,
    previous_jcode_home: Option<std::ffi::OsString>,
    previous_home: Option<std::ffi::OsString>,
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Self {
        let lock = crate::storage::lock_test_env();
        let dir = tempfile::TempDir::new().expect("tempdir");
        let previous_jcode_home = std::env::var_os("JCODE_HOME");
        let previous_home = std::env::var_os("HOME");
        crate::env::set_var("JCODE_HOME", dir.path());
        crate::env::set_var("HOME", dir.path());
        Self {
            _lock: lock,
            previous_jcode_home,
            previous_home,
            dir,
        }
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        restore("JCODE_HOME", self.previous_jcode_home.take());
        restore("HOME", self.previous_home.take());
    }
}

fn restore(key: &str, value: Option<std::ffi::OsString>) {
    match value {
        Some(value) => crate::env::set_var(key, value),
        None => crate::env::remove_var(key),
    }
}

/// A bare name is the convenience form: `file = "deepseek.json"` resolves under
/// `~/.jcode/cache/` so the user does not have to spell out the cache path.
#[test]
fn a_bare_vendor_file_name_resolves_under_the_jcode_cache_dir() {
    let home = Home::new();
    let path = file_of("[providers.deepseek]\nfile = \"deepseek.json\"\n");
    assert_eq!(path, home.dir.path().join("cache").join("deepseek.json"));
}

/// Anything that is not a bare name is a path, used as written. `~` is the one
/// expansion: `~/…` becomes the home directory. A `file://` value is just such
/// a path-like string, with no special meaning: it is not resolved under the
/// cache dir and simply names a file that does not exist.
#[test]
fn a_non_bare_value_is_a_path_used_as_written() {
    let home = Home::new();
    // Absolute path: verbatim.
    assert_eq!(
        file_of("[providers.deepseek]\nfile = \"/opt/jcode/prices.json\"\n"),
        std::path::PathBuf::from("/opt/jcode/prices.json")
    );
    // Relative with a separator: verbatim (relative to the process cwd).
    assert_eq!(
        file_of("[providers.deepseek]\nfile = \"pricing/prices.json\"\n"),
        std::path::PathBuf::from("pricing/prices.json")
    );
    // `~` expands to the home directory.
    assert_eq!(
        file_of("[providers.deepseek]\nfile = \"~/prices.json\"\n"),
        home.dir.path().join("prices.json")
    );
    // A `file://` string is a path, not a synonym for a bare name: it is not
    // resolved under the cache dir.
    let path = file_of("[providers.deepseek]\nfile = \"file:///opt/jcode/prices.json\"\n");
    assert_eq!(
        path,
        std::path::PathBuf::from("file:///opt/jcode/prices.json")
    );
    assert!(
        !path.starts_with(home.dir.path()),
        "a file:// value must not be cache-resolved: {path:?}"
    );
}

/// An empty `file` string is a half-deleted line, not a request to write rules
/// inline (that is an absent `file`), so it is rejected with its config path.
#[test]
fn an_empty_vendor_file_is_rejected() {
    let err = validate(&parse_toml("[providers.deepseek]\nfile = \"\"\n"))
        .expect_err("an empty file string is rejected");
    assert_eq!(err.field_path, "pricing.providers.deepseek.file");
    assert!(
        err.message.contains("empty"),
        "the error must say what is wrong: {err}"
    );
}

/// Every fixed name jcode writes in `~/.jcode/cache/` must be refused as a bare
/// name. The error is factual and short: it names the clash.
#[test]
fn a_bare_name_may_not_shadow_a_jcode_cache_file() {
    let _home = Home::new();
    for name in crate::config::pricing::guarded_cache_names_for_tests() {
        let err = validate(&parse_toml(&format!(
            "[providers.deepseek]\nfile = \"{name}\"\n"
        )))
        .expect_err("a bare name that shadows jcode's cache is refused");
        assert_eq!(err.field_path, "pricing.providers.deepseek.file");
        assert!(err.message.contains(name), "the clash must be named: {err}");
    }
}

/// The guarded names also include families jcode builds from a runtime namespace
/// (`<namespace>_models.json`, `<ns>_endpoints_<model>.json`,
/// `session_search_<source>_index_v2.bin`), so representative members must be
/// refused too.
#[test]
fn a_namespaced_jcode_cache_name_is_refused_as_a_bare_vendor_file() {
    let _home = Home::new();
    for name in [
        "openrouter_models.json",
        "deepseek_models.json",
        "openai-compatible_models.json",
        "openrouter_endpoints_gpt-4o.json",
        "session_search_claude_index_v2.bin",
    ] {
        let err = validate(&parse_toml(&format!(
            "[providers.deepseek]\nfile = \"{name}\"\n"
        )))
        .expect_err("a bare name that shadows jcode's cache is refused");
        assert_eq!(err.field_path, "pricing.providers.deepseek.file");
        assert!(err.message.contains(name), "the clash must be named: {err}");
    }
}

/// The guard applies to bare names only: the same name written as a non-bare
/// path is the user's own file and must be accepted exactly as written.
#[test]
fn a_guarded_cache_name_as_a_non_bare_path_is_accepted() {
    let _home = Home::new();
    let guarded = crate::config::pricing::guarded_cache_names_for_tests();
    let names = guarded.iter().copied().chain([
        "openrouter_models.json",
        "deepseek_models.json",
        "openrouter_endpoints_gpt-4o.json",
        "session_search_claude_index_v2.bin",
    ]);
    for name in names {
        let raw = format!("./{name}");
        assert_eq!(
            file_of(&format!("[providers.deepseek]\nfile = \"{raw}\"\n")),
            std::path::PathBuf::from(&raw),
            "a non-bare path is used as written, never cache-resolved"
        );
    }
}

#[test]
fn a_vendor_currency_is_normalized_and_defaults_to_usd() {
    let (config, _) = validate(&parse_toml("[providers.deepseek]\n")).expect("validates");
    assert!(
        config.providers["deepseek"].currency.is_none(),
        "an absent currency stays absent (the resolver defaults to USD)"
    );

    let (config, _) =
        validate(&parse_toml("[providers.deepseek]\ncurrency = \"cny\"\n")).expect("validates");
    assert_eq!(
        config.providers["deepseek"].currency,
        Some(jcode_provider_core::Currency::new("CNY"))
    );

    let (_, warnings) = validate(&parse_toml("[providers.deepseek]\ncurrency = \"zzz\"\n"))
        .expect("an unknown code is accepted");
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("pricing.providers.deepseek.currency")),
        "an unknown currency warns with its path: {warnings:?}"
    );
}

#[test]
fn a_vendor_file_alone_is_not_empty() {
    let (config, _) =
        validate(&parse_toml("[providers.deepseek]\nfile = \"/a.json\"\n")).expect("validates");
    assert!(
        !config.is_empty(),
        "a configured vendor file means the section is configured"
    );
}

/// `Config::save()` serializes the whole struct, so a default-valued field is
/// baked into the user's `config.toml` unless it says `skip_serializing_if`
/// (the lesson of commit `9da6f9831`).
#[test]
fn a_default_valued_vendor_file_is_never_written_back() {
    let toml = toml::to_string_pretty(&crate::config::Config::default()).expect("serialize");
    assert!(
        !toml.contains("[pricing"),
        "an unconfigured [pricing] must stay absent:\n{toml}"
    );

    // A provider that only sets a currency must not gain a `file` key.
    let mut config = crate::config::Config::default();
    config.pricing.providers.insert(
        "deepseek".to_string(),
        jcode_config_types::ProviderPricingFile {
            currency: Some("CNY".to_string()),
            ..Default::default()
        },
    );
    let toml = toml::to_string_pretty(&config).expect("serialize");
    assert!(toml.contains("[pricing.providers.deepseek]"), "{toml}");
    let section = &toml[toml
        .find("[pricing.providers.deepseek]")
        .expect("the deepseek section is serialized")..];
    assert!(
        !section.contains("file ="),
        "an absent file must not be written:\n{section}"
    );
}

#[test]
fn a_configured_vendor_file_is_written_back() {
    let mut config = crate::config::Config::default();
    config.pricing.providers.insert(
        "deepseek".to_string(),
        jcode_config_types::ProviderPricingFile {
            file: Some("/prices/a.json".to_string()),
            currency: Some("CNY".to_string()),
            ..Default::default()
        },
    );
    let toml = toml::to_string_pretty(&config).expect("serialize");
    assert!(
        toml.contains("[pricing.providers.deepseek]"),
        "a configured vendor must survive a save:\n{toml}"
    );
    assert!(toml.contains("file = \"/prices/a.json\""), "{toml}");
    assert!(toml.contains("currency = \"CNY\""), "{toml}");
}
