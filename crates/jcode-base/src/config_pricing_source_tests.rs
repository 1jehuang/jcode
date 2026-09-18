//! Validation and serialization of `[[pricing.sources]]`.
//!
//! Split from `config::pricing` so the production file stays inside the
//! code-size budget; everything here goes through the public `validate` entry
//! point the config loader uses.

use crate::config::pricing::{DEFAULT_SOURCE_REFRESH_SECS, validate};
use crate::config::{PricingSource, SourceLocation};
use jcode_config_types::PricingConfigFile;
use jcode_provider_core::Currency;

fn parse_toml(section: &str) -> PricingConfigFile {
    toml::from_str(section).expect("toml parses into the DTO")
}

fn sources_of(section: &str) -> Vec<PricingSource> {
    validate(&parse_toml(section)).expect("validates").0.sources
}

#[test]
fn sources_resolve_by_priority_then_id_whatever_the_file_order_is() {
    let sources = sources_of(
        r#"
        [[sources]]
        id = "zeta"
        url = "https://example.test/zeta.json"
        priority = 5

        [[sources]]
        id = "alpha"
        url = "https://example.test/alpha.json"
        priority = 5

        [[sources]]
        id = "first"
        url = "https://example.test/first.json"
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
    // The single-source case: `url` alone. Deriving the id is what the next
    // tests pin down.
    let sources = sources_of("[[sources]]\nurl = \"file:///home/me/prices.json\"\n");
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].id, "prices");

    let err = validate(&parse_toml("[[sources]]\nid = \"a\"\n")).expect_err("a url-less source");
    assert_eq!(err.field_path, "pricing.sources[0].url");
    let err =
        validate(&parse_toml("[[sources]]\nid = \"a\"\nurl = \"\"\n")).expect_err("an empty url");
    assert_eq!(err.field_path, "pricing.sources[0].url");

    // An explicit id is still allowed to be written, and an explicitly empty
    // one is a mistake rather than a request to derive: deriving silently there
    // would hide a half-deleted line.
    let sources = sources_of("[[sources]]\nid = \"corp\"\nurl = \"file:///home/me/prices.json\"\n");
    assert_eq!(sources[0].id, "corp");
    let err = validate(&parse_toml(
        "[[sources]]\nid = \"\"\nurl = \"https://x.test/a.json\"\n",
    ))
    .expect_err("an explicitly empty id is rejected");
    assert_eq!(err.field_path, "pricing.sources[0].id");
}

/// A derived id is the cache key and the label users see, so it must be a
/// property of the location, not of the file's declaration order or of anything
/// ambient. Loading the same config twice must produce the same id, and two
/// different id-less entries must each get their own.
#[test]
fn an_id_is_derived_from_the_location_and_is_stable_across_loads() {
    // The URL keeps its last path segment, without the extension or the query.
    let section = "[[sources]]\nurl = \"https://pricing.test/models_dev.mirror.json?token=x\"\n";
    assert_eq!(sources_of(section)[0].id, "models_dev.mirror");
    assert_eq!(sources_of(section)[0].id, "models_dev.mirror");

    // A local file uses its file stem, scheme or not.
    assert_eq!(
        sources_of("[[sources]]\nurl = \"file:///opt/jcode/prices.json\"\n")[0].id,
        "prices"
    );
    assert_eq!(
        sources_of("[[sources]]\nurl = \"/opt/jcode/prices.json\"\n")[0].id,
        "prices"
    );

    // A location that names no file at all still gets a recognisable,
    // deterministic id rather than an empty cache key.
    assert_eq!(
        sources_of("[[sources]]\nurl = \"https://pricing.test/\"\n")[0].id,
        "source1"
    );

    // Two id-less entries keep two distinct ids: the second is suffixed in
    // declaration order, so neither sheet is silently merged under one key.
    let sources = sources_of(
        r#"
        [[sources]]
        url = "file:///one/prices.json"

        [[sources]]
        url = "file:///two/prices.json"
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
        url = "file:///one/prices.json"

        [[sources]]
        id = "prices"
        url = "https://other.test/prices.json"
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
        derived.location,
        SourceLocation::LocalFile(std::path::PathBuf::from("/one/prices.json")),
        "the replaced name lands on the derived entry"
    );
}

#[test]
fn duplicate_explicit_source_ids_are_rejected() {
    let err = validate(&parse_toml(
        r#"
        [[sources]]
        id = "mirror"
        url = "https://x.test/a.json"

        [[sources]]
        id = "mirror"
        url = "https://x.test/b.json"
        "#,
    ))
    .expect_err("explicit ids must be unique");
    assert_eq!(err.field_path, "pricing.sources[1].id");
    assert!(err.message.contains("duplicate"), "{err}");
}

#[test]
fn a_source_must_be_https_or_a_local_file() {
    for url in ["http://x.test/a.json", "ftp://x.test/a.json"] {
        let err = validate(&parse_toml(&format!(
            "[[sources]]\nid = \"a\"\nurl = \"{url}\"\n"
        )))
        .expect_err("only https and local files are accepted");
        assert_eq!(err.field_path, "pricing.sources[0].url");
        assert!(
            err.message.contains("https://"),
            "the message says what is allowed: {err}"
        );
    }

    let sources = sources_of("[[sources]]\nid = \"a\"\nurl = \"file:///opt/jcode/p.json\"\n");
    assert_eq!(
        sources[0].location,
        SourceLocation::LocalFile(std::path::PathBuf::from("/opt/jcode/p.json"))
    );

    // A bare path is the same thing spelled without a scheme.
    let sources = sources_of("[[sources]]\nid = \"a\"\nurl = \"/opt/jcode/p.json\"\n");
    assert_eq!(
        sources[0].location,
        SourceLocation::LocalFile(std::path::PathBuf::from("/opt/jcode/p.json"))
    );

    let sources = sources_of("[[sources]]\nid = \"a\"\nurl = \"https://x.test/p.json\"\n");
    assert_eq!(
        sources[0].location,
        SourceLocation::Remote("https://x.test/p.json".to_string())
    );
}

#[test]
fn the_format_defaults_to_models_dev_v1_and_anything_else_is_rejected() {
    let sources = sources_of("[[sources]]\nid = \"a\"\nurl = \"https://x.test/a.json\"\n");
    assert_eq!(sources[0].refresh_secs, DEFAULT_SOURCE_REFRESH_SECS);

    let sources = sources_of(
        "[[sources]]\nid = \"a\"\nurl = \"https://x.test/a.json\"\nformat = \"models_dev_v1\"\n",
    );
    assert_eq!(sources[0].id, "a");

    let err = validate(&parse_toml(
        "[[sources]]\nid = \"a\"\nurl = \"https://x.test/a.json\"\nformat = \"csv\"\n",
    ))
    .expect_err("an unknown format is rejected, not ignored");
    assert_eq!(err.field_path, "pricing.sources[0].format");
}

#[test]
fn a_ttl_must_be_at_least_one_second() {
    let err = validate(&parse_toml(
        "[[sources]]\nid = \"a\"\nurl = \"https://x.test/a.json\"\nrefresh_secs = 0\n",
    ))
    .expect_err("a zero TTL is rejected");
    assert_eq!(err.field_path, "pricing.sources[0].refresh_secs");

    let sources =
        sources_of("[[sources]]\nid = \"a\"\nurl = \"https://x.test/a.json\"\nrefresh_secs = 60\n");
    assert_eq!(sources[0].refresh_secs, 60);
}

#[test]
fn an_empty_scope_or_model_entry_is_rejected() {
    let err = validate(&parse_toml(
        "[[sources]]\nid = \"a\"\nurl = \"https://x.test/a.json\"\nscope = [\"deepseek\", \" \"]\n",
    ))
    .expect_err("a blank scope entry is rejected");
    assert_eq!(err.field_path, "pricing.sources[0].scope[1]");

    let err = validate(&parse_toml(
        "[[sources]]\nid = \"a\"\nurl = \"https://x.test/a.json\"\nmodels = [\"\"]\n",
    ))
    .expect_err("a blank glob is rejected");
    assert_eq!(err.field_path, "pricing.sources[0].models[0]");
}

#[test]
fn a_source_currency_is_normalized_and_defaults_to_usd() {
    let sources = sources_of("[[sources]]\nid = \"a\"\nurl = \"https://x.test/a.json\"\n");
    assert!(sources[0].currency.is_usd());

    let sources = sources_of(
        "[[sources]]\nid = \"a\"\nurl = \"https://x.test/a.json\"\ncurrency = \"cny\"\n",
    );
    assert_eq!(sources[0].currency, Currency::new("CNY"));

    let (_, warnings) = validate(&parse_toml(
        "[[sources]]\nid = \"a\"\nurl = \"https://x.test/a.json\"\ncurrency = \"zzz\"\n",
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
    let (config, _) = validate(&parse_toml(
        "[[sources]]\nid = \"a\"\nurl = \"https://x.test/a.json\"\n",
    ))
    .expect("validates");
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
            url: "https://x.test/a.json".to_string(),
            ..Default::default()
        });
    let toml = toml::to_string_pretty(&config).expect("serialize");
    assert!(
        toml.contains("[[pricing.sources]]"),
        "a configured source must survive a save:\n{toml}"
    );
    assert!(toml.contains("id = \"mirror\""), "{toml}");
}
