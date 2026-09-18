//! Validation and serialization of `[[pricing.sources]]`.
//!
//! Split from `config::pricing` so the production file stays inside the
//! code-size budget; everything here goes through the public `validate` entry
//! point the config loader uses.

use crate::config::PricingSource;
use crate::config::pricing::validate;
use jcode_config_types::PricingConfigFile;

fn parse_toml(section: &str) -> PricingConfigFile {
    toml::from_str(section).expect("toml parses into the DTO")
}

fn sources_of(section: &str) -> Vec<PricingSource> {
    validate(&parse_toml(section)).expect("validates").0.sources
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

#[test]
fn sources_resolve_by_priority_then_id_whatever_the_file_order_is() {
    let sources = sources_of(
        r#"
        [[sources]]
        id = "zeta"
        file = "/prices/zeta.json"
        priority = 5

        [[sources]]
        id = "alpha"
        file = "/prices/alpha.json"
        priority = 5

        [[sources]]
        id = "first"
        file = "/prices/first.json"
        priority = -3
        "#,
    );
    let ids: Vec<&str> = sources.iter().map(|source| source.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["first", "alpha", "zeta"],
        "priority ascending, ties by id: {ids:?}"
    );
    assert_eq!(sources[0].priority, -3);
}

#[test]
fn a_source_needs_a_location_but_an_id_is_optional() {
    // The single-source case: `file` alone. Deriving the id is what the next
    // tests pin down.
    let sources = sources_of("[[sources]]\nfile = \"/home/me/prices.json\"\n");
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].id, "prices");

    let err = validate(&parse_toml("[[sources]]\nid = \"a\"\n")).expect_err("a file-less source");
    assert_eq!(err.field_path, "pricing.sources[0].file");
    let err =
        validate(&parse_toml("[[sources]]\nid = \"a\"\nfile = \"\"\n")).expect_err("an empty file");
    assert_eq!(err.field_path, "pricing.sources[0].file");

    // An explicit id is still allowed to be written, and an explicitly empty
    // one is a mistake rather than a request to derive: deriving silently there
    // would hide a half-deleted line.
    let sources = sources_of("[[sources]]\nid = \"corp\"\nfile = \"/home/me/prices.json\"\n");
    assert_eq!(sources[0].id, "corp");
    let err = validate(&parse_toml(
        "[[sources]]\nid = \"\"\nfile = \"/home/me/prices.json\"\n",
    ))
    .expect_err("an explicitly empty id is rejected");
    assert_eq!(err.field_path, "pricing.sources[0].id");
}

/// A derived id is the cache key and the label users see, so it must be a
/// property of the file, not of the declaration order or of anything ambient.
/// Loading the same config twice must produce the same id, and two different
/// id-less entries must each get their own.
#[test]
fn an_id_is_derived_from_the_location_and_is_stable_across_loads() {
    let section = "[[sources]]\nfile = \"/opt/jcode/models_dev.mirror.json\"\n";
    assert_eq!(sources_of(section)[0].id, "models_dev.mirror");
    assert_eq!(sources_of(section)[0].id, "models_dev.mirror");

    // A relative path uses its file stem too.
    assert_eq!(
        sources_of("[[sources]]\nfile = \"prices.json\"\n")[0].id,
        "prices"
    );
    assert_eq!(
        sources_of("[[sources]]\nfile = \"sub/dir/prices.json\"\n")[0].id,
        "prices"
    );

    // A location that names no file at all still gets a recognisable,
    // deterministic id rather than an empty cache key.
    assert_eq!(sources_of("[[sources]]\nfile = \"/\"\n")[0].id, "source1");

    // Two id-less entries keep two distinct ids: the second is suffixed in
    // declaration order, so neither sheet is silently merged under one key.
    let sources = sources_of(
        r#"
        [[sources]]
        file = "/one/prices.json"

        [[sources]]
        file = "/two/prices.json"
        "#,
    );
    let ids: Vec<&str> = sources.iter().map(|source| source.id.as_str()).collect();
    assert_eq!(ids, vec!["prices", "prices-2"], "{ids:?}");
}

/// A derived id must yield to an explicit one, wherever the explicit entry is
/// declared: the explicit form is what the user wrote down, so it wins the name.
#[test]
fn a_derived_id_never_collides_with_an_explicit_id() {
    let sources = sources_of(
        r#"
        [[sources]]
        file = "/one/prices.json"

        [[sources]]
        id = "prices"
        file = "/other/prices.json"
        "#,
    );
    // Ordered by priority (both 0), then id: `prices` before `prices-2`.
    let ids: Vec<&str> = sources.iter().map(|source| source.id.as_str()).collect();
    assert_eq!(ids, vec!["prices", "prices-2"], "{ids:?}");
    let derived = sources
        .iter()
        .find(|source| source.id == "prices-2")
        .expect("the derived one is disambiguated");
    assert_eq!(
        derived.path,
        std::path::PathBuf::from("/one/prices.json"),
        "the replaced name lands on the derived entry"
    );
}

#[test]
fn duplicate_explicit_source_ids_are_rejected() {
    let err = validate(&parse_toml(
        r#"
        [[sources]]
        id = "mirror"
        file = "/a.json"

        [[sources]]
        id = "mirror"
        file = "/b.json"
        "#,
    ))
    .expect_err("explicit ids must be unique");
    assert_eq!(err.field_path, "pricing.sources[1].id");
    assert!(err.message.contains("duplicate"), "{err}");
}

/// A bare name is the single-source convenience: `file = "deepseek.json"`
/// resolves under `~/.jcode/cache/` so the user does not have to spell out the
/// cache path.
#[test]
fn a_bare_name_resolves_under_the_jcode_cache_dir() {
    let home = Home::new();
    let sources = sources_of("[[sources]]\nfile = \"deepseek.json\"\n");
    assert_eq!(
        sources[0].path,
        home.dir.path().join("cache").join("deepseek.json")
    );
    assert_eq!(sources[0].id, "deepseek");
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
        sources_of("[[sources]]\nfile = \"/opt/jcode/prices.json\"\n")[0].path,
        std::path::PathBuf::from("/opt/jcode/prices.json")
    );
    // Relative with a separator: verbatim (relative to the process cwd).
    assert_eq!(
        sources_of("[[sources]]\nfile = \"pricing/prices.json\"\n")[0].path,
        std::path::PathBuf::from("pricing/prices.json")
    );
    // `~` expands to the home directory.
    assert_eq!(
        sources_of("[[sources]]\nfile = \"~/prices.json\"\n")[0].path,
        home.dir.path().join("prices.json")
    );
    // A `file://` string is a path, not a synonym for a bare name: it is not
    // resolved under the cache dir.
    let path = sources_of("[[sources]]\nfile = \"file:///opt/jcode/prices.json\"\n")[0]
        .path
        .clone();
    assert_eq!(
        path,
        std::path::PathBuf::from("file:///opt/jcode/prices.json")
    );
    assert!(
        !path.starts_with(home.dir.path()),
        "a file:// value must not be cache-resolved: {path:?}"
    );
}

/// Every fixed name jcode writes in `~/.jcode/cache/` must be refused as a bare
/// name. The error is factual and short: it names the clash.
#[test]
fn a_bare_name_may_not_shadow_a_jcode_cache_file() {
    let _home = Home::new();
    for name in crate::config::pricing::guarded_cache_names_for_tests() {
        let err = validate(&parse_toml(&format!(
            "[[sources]]\nid = \"a\"\nfile = \"{name}\"\n"
        )))
        .expect_err("a bare name that shadows jcode's cache is refused");
        assert_eq!(err.field_path, "pricing.sources[0].file");
        assert!(err.message.contains(name), "the clash must be named: {err}");
    }
}

/// The guarded names also include families jcode builds from a runtime namespace
/// (`<namespace>_models.json`, `<ns>_endpoints_<model>.json`,
/// `session_search_<source>_index_v2.bin`), so representative members must be
/// refused too.
#[test]
fn a_namespaced_jcode_cache_name_is_refused_as_a_bare_source() {
    let _home = Home::new();
    for name in [
        "openrouter_models.json",
        "deepseek_models.json",
        "openai-compatible_models.json",
        "openrouter_endpoints_gpt-4o.json",
        "session_search_claude_index_v2.bin",
    ] {
        let err = validate(&parse_toml(&format!(
            "[[sources]]\nid = \"a\"\nfile = \"{name}\"\n"
        )))
        .expect_err("a bare name that shadows jcode's cache is refused");
        assert_eq!(err.field_path, "pricing.sources[0].file");
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
        let sources = sources_of(&format!("[[sources]]\nid = \"a\"\nfile = \"{raw}\"\n"));
        assert_eq!(
            sources[0].path,
            std::path::PathBuf::from(&raw),
            "a non-bare path is used as written, never cache-resolved"
        );
    }
}

#[test]
fn the_format_defaults_to_models_dev_v1_and_anything_else_is_rejected() {
    let sources = sources_of("[[sources]]\nid = \"a\"\nfile = \"/a.json\"\n");
    assert_eq!(sources[0].path, std::path::PathBuf::from("/a.json"));

    let sources =
        sources_of("[[sources]]\nid = \"a\"\nfile = \"/a.json\"\nformat = \"models_dev_v1\"\n");
    assert_eq!(sources[0].id, "a");

    let err = validate(&parse_toml(
        "[[sources]]\nid = \"a\"\nfile = \"/a.json\"\nformat = \"csv\"\n",
    ))
    .expect_err("an unknown format is rejected, not ignored");
    assert_eq!(err.field_path, "pricing.sources[0].format");
}

#[test]
fn an_empty_scope_or_model_entry_is_rejected() {
    let err = validate(&parse_toml(
        "[[sources]]\nid = \"a\"\nfile = \"/a.json\"\nscope = [\"deepseek\", \" \"]\n",
    ))
    .expect_err("a blank scope entry is rejected");
    assert_eq!(err.field_path, "pricing.sources[0].scope[1]");

    let err = validate(&parse_toml(
        "[[sources]]\nid = \"a\"\nfile = \"/a.json\"\nmodels = [\"\"]\n",
    ))
    .expect_err("a blank glob is rejected");
    assert_eq!(err.field_path, "pricing.sources[0].models[0]");
}

#[test]
fn a_source_currency_is_normalized_and_defaults_to_usd() {
    let sources = sources_of("[[sources]]\nid = \"a\"\nfile = \"/a.json\"\n");
    assert!(sources[0].currency.is_usd());

    let sources = sources_of("[[sources]]\nid = \"a\"\nfile = \"/a.json\"\ncurrency = \"cny\"\n");
    assert_eq!(
        sources[0].currency,
        jcode_provider_core::Currency::new("CNY")
    );

    let (_, warnings) = validate(&parse_toml(
        "[[sources]]\nid = \"a\"\nfile = \"/a.json\"\ncurrency = \"zzz\"\n",
    ))
    .expect("an unknown code is accepted");
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("pricing.sources[0].currency")),
        "an unknown currency warns with its path: {warnings:?}"
    );
}

#[test]
fn a_sources_only_section_is_not_empty() {
    let (config, _) =
        validate(&parse_toml("[[sources]]\nid = \"a\"\nfile = \"/a.json\"\n")).expect("validates");
    assert!(
        !config.is_empty(),
        "a configured source means the section is configured"
    );
}

/// `Config::save()` serializes the whole struct, so a default-valued field is
/// baked into the user's `config.toml` unless it says `skip_serializing_if`
/// (the lesson of commit `9da6f9831`).
#[test]
fn a_default_valued_sources_list_is_never_written_back() {
    let toml = toml::to_string_pretty(&crate::config::Config::default()).expect("serialize");
    assert!(
        !toml.contains("[pricing"),
        "an unconfigured [pricing] must stay absent:\n{toml}"
    );

    // And a section that is configured for something else must not gain an
    // empty `sources` array either.
    let mut config = crate::config::Config::default();
    config.pricing.fx_rates.insert("CNY".to_string(), 7.2);
    let toml = toml::to_string_pretty(&config).expect("serialize");
    assert!(toml.contains("[pricing.fx_rates]"), "{toml}");
    assert!(
        !toml.contains("[[pricing.sources]]"),
        "an empty sources list must not be written:\n{toml}"
    );

    let section = toml::to_string_pretty(&PricingConfigFile::default()).expect("serialize");
    assert!(
        !section.contains("sources"),
        "the field itself must be skipped when empty: {section:?}"
    );
}

#[test]
fn a_configured_source_is_written_back() {
    let mut config = crate::config::Config::default();
    config
        .pricing
        .sources
        .push(jcode_config_types::PricingSourceFile {
            id: Some("mirror".to_string()),
            file: "/prices/a.json".to_string(),
            ..Default::default()
        });
    let toml = toml::to_string_pretty(&config).expect("serialize");
    assert!(
        toml.contains("[[pricing.sources]]"),
        "a configured source must survive a save:\n{toml}"
    );
    assert!(toml.contains("id = \"mirror\""), "{toml}");
    assert!(toml.contains("file = \"/prices/a.json\""), "{toml}");
}
