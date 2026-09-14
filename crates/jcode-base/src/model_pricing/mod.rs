//! Live model pricing catalog backed by <https://models.dev>.
//!
//! jcode's static pricing tables (`jcode_provider_core::pricing`) only cover
//! first-party Anthropic/OpenAI models and go stale whenever a provider ships
//! new models or changes prices. models.dev publishes a free, no-auth JSON
//! catalog (`https://models.dev/api.json`) with per-model `input`/`output`/
//! `cache_read`/`cache_write` USD prices per million tokens across 140+
//! providers, including every OpenAI-compatible profile jcode ships.
//!
//! This module mirrors the OpenRouter catalog pattern:
//!   - a 24h disk cache under `~/.jcode/cache/models_dev_pricing.json`,
//!   - synchronous lookups that never block on the network,
//!   - a background refresh scheduled on cache miss/staleness.
//!
//! Lookup order for callers is curated static table first (exact, reviewed),
//! then this catalog, then provider-specific sources (OpenRouter endpoints),
//! and only then a generic fallback.
//!
//! Layout: `catalog` owns the models.dev fetch/parse/cache machinery, `rules`
//! owns tariff selection (peak/off-peak windows, multipliers, validity), and
//! this module owns provider-id mapping, model-id normalization, the lookup
//! entry points, the refresh scheduler, and the resolver that puts hand-written
//! `[pricing]` rules ahead of every derived source.

mod call_rates;
mod catalog;
mod entry;
mod fx;
mod generation;
mod rules;
mod sources;

/// The per-model rate fields an entry carries, re-exported here so the billing
/// call sites can read a card without reaching into `config`.
pub use crate::config::CostFields;
pub use call_rates::{CallRateCard, ConfigCallRates, config_call_rates};
pub use catalog::ModelCost;
pub use entry::ModelPricingEntry;
pub use fx::{FxTable, convert};
pub use generation::pricing_generation;
pub use sources::pricing_config;

use catalog::PricingCache;
#[cfg(test)]
use catalog::parse_api_response;
use catalog::{CACHE_TTL_SECS, REFRESH_IN_FLIGHT, load_cache, now_unix_secs, refresh_now};
#[cfg(test)]
pub(crate) use catalog::{clear_memory_cache_for_tests, save_test_cache};
#[cfg(test)]
#[path = "call_rates_tests.rs"]
mod call_rates_tests;

use jcode_provider_core::{
    CHEAPNESS_REFERENCE_INPUT_TOKENS, CHEAPNESS_REFERENCE_OUTPUT_TOKENS, Currency, Money,
};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::SystemTime;

/// Translate a jcode provider key (runtime key, activity source key, or
/// compatible-profile id) to the models.dev provider id.
pub fn models_dev_provider_id(jcode_provider: &str) -> Option<&'static str> {
    let key = jcode_provider
        .trim()
        .strip_prefix("openai-compatible:")
        .unwrap_or_else(|| jcode_provider.trim());
    Some(match key {
        "anthropic" | "claude" | "claude:api-key" | "anthropic-api" => "anthropic",
        "openai" | "openai:api-key" | "openai-api" => "openai",
        "openrouter" => "openrouter",
        "opencode" => "opencode",
        "opencode-go" => "opencode-go",
        "deepseek" => "deepseek",
        "moonshotai" => "moonshotai",
        "kimi" => "kimi-for-coding",
        "zai" => "zai",
        "cerebras" => "cerebras",
        "groq" => "groq",
        "mistral" => "mistral",
        "xai" => "xai",
        "minimax" => "minimax",
        "togetherai" => "togetherai",
        "fireworks" => "fireworks-ai",
        "deepinfra" => "deepinfra",
        "perplexity" => "perplexity",
        "nebius" => "nebius",
        "scaleway" => "scaleway",
        "stackit" => "stackit",
        "huggingface" => "huggingface",
        "baseten" => "baseten",
        "chutes" => "chutes",
        "nvidia-nim" => "nvidia",
        "302ai" => "302ai",
        "cortecs" => "cortecs",
        "alibaba-coding-plan" => "alibaba",
        "bedrock" => "amazon-bedrock",
        "azure-openai" | "azure" => "azure",
        "gemini" | "gemini-api" => "google",
        _ => return None,
    })
}

/// Strip jcode-local suffixes/prefixes a model id may carry before catalog
/// lookup (`[1m]` long-context alias, `provider/` prefixes for OpenRouter ids).
fn normalize_model_id(model: &str) -> &str {
    let model = jcode_provider_core::model_id::strip_long_context_suffix(model).trim();
    model
        .rsplit_once('@')
        .map_or(model, |(bare, _)| bare.trim())
}

/// Look up live pricing for `model` under a jcode provider key. Returns `None`
/// when the catalog has no entry; never blocks on the network. Schedules a
/// background refresh when the disk cache is missing or stale.
pub fn lookup(jcode_provider: &str, model: &str) -> Option<ModelCost> {
    let provider_id = models_dev_provider_id(jcode_provider)?;
    let cache = ensure_cache_fresh()?;
    let models = cache.providers.get(provider_id)?;
    let model = normalize_model_id(model);
    if let Some(entry) = models.get(model) {
        return entry.to_model_cost();
    }
    // OpenRouter-style ids (`anthropic/claude-...`) may reach here with the
    // provider prefix still attached; retry on the bare model name.
    if let Some((_, bare)) = model.rsplit_once('/') {
        return models.get(bare).and_then(ModelPricingEntry::to_model_cost);
    }
    None
}

/// Return the freshest cache available, scheduling a refresh if needed.
fn ensure_cache_fresh() -> Option<Arc<PricingCache>> {
    let cache = load_cache();
    let stale = cache
        .as_ref()
        .map(|c| now_unix_secs().saturating_sub(c.cached_at_unix_secs) >= CACHE_TTL_SECS)
        .unwrap_or(true);
    if stale {
        schedule_refresh();
    }
    cache
}

/// The effective rate card for `(provider, model)` at `at`, together with the
/// currency of the layer that produced it.
///
/// Priority (spec 4.4): hand-written `[pricing.providers]` rules first, then
/// models.dev. Optional `[[pricing.sources]]` extra sources land between the
/// two later.
///
/// A card that is out of effect at `at` (`effective_from` / `effective_until`)
/// is dropped and the next layer prices the model instead; peak/off-peak
/// selection happens in `rules`, driven by `sources::config_price`, so the
/// `cost` returned here is already the tariff in effect at `at`.
///
/// This is the rate-level entry point: unlike [`effective_cost`] it hands back
/// input/output/cache rates instead of one scalar, which is what a billing call
/// site needs. Config rules are resolved at `at`, so tariff selection (peak vs
/// off-peak) follows the instant the caller passes, not the wall clock.
pub fn effective_entry(
    provider: &str,
    model: &str,
    at: SystemTime,
) -> Option<(ModelPricingEntry, Currency)> {
    match sources::config_price(provider, model, at) {
        sources::ConfigPrice::NoPrice => None,
        sources::ConfigPrice::Hit { entry, currency } => {
            let resolved = sources::resolve_card(*entry, currency, provider, model, at);
            Some((resolved.entry, resolved.currency))
        }
        sources::ConfigPrice::Absent => models_dev_entry(provider, model),
    }
}

/// The models.dev layer's card, always USD.
fn models_dev_entry(provider: &str, model: &str) -> Option<(ModelPricingEntry, Currency)> {
    lookup(provider, model).map(|cost| (ModelPricingEntry::from_model_cost(cost), Currency::usd()))
}

/// The hand-written `[pricing.providers]` card for `(provider, model)`, if the
/// user configured one that is in effect at `at`.
///
/// Unlike [`effective_cost`] this never falls back to models.dev, so the route
/// pricing chain can keep labelling each layer separately.
pub(crate) fn configured_entry(
    provider: &str,
    model: &str,
    at: SystemTime,
) -> Option<(ModelPricingEntry, Currency)> {
    match sources::config_price(provider, model, at) {
        sources::ConfigPrice::Hit { entry, currency } => {
            let resolved = sources::resolve_card(*entry, currency, provider, model, at);
            // A card that could not price the model is not this layer's win;
            // let the chain reach models.dev and label it as such.
            resolved
                .from_config
                .then_some((resolved.entry, resolved.currency))
        }
        sources::ConfigPrice::Absent | sources::ConfigPrice::NoPrice => None,
    }
}

/// What one canonical request costs on this route at `at`, in the currency the
/// winning layer uses.
///
/// The canonical request is the same one the route catalog's cheapness
/// estimates use (25k input / 5k output tokens), so the number is directly
/// comparable with `RouteCheapnessEstimate::estimated_reference_cost_micros`.
/// Per-token rates live on [`ModelPricingEntry::cost`]; this is the single
/// scalar a caller wants when it only needs "how expensive is this model".
///
/// `None` means "we cannot price this": no config rule and no catalog entry, or
/// a rule that expired with `on_rule_expiry = "no_price"`.
pub fn effective_cost(provider: &str, model: &str, at: SystemTime) -> Option<Money> {
    let (entry, currency) = effective_entry(provider, model, at)?;
    let input = entry.cost.input?;
    let output = entry.cost.output?;
    let amount = (input * CHEAPNESS_REFERENCE_INPUT_TOKENS as f64
        + output * CHEAPNESS_REFERENCE_OUTPUT_TOKENS as f64)
        / 1_000_000.0;
    Some(Money::new(amount, currency))
}

/// Spawn one background refresh at a time. Safe to call from sync contexts;
/// uses a thread + ad-hoc runtime when no Tokio runtime is active.
pub fn schedule_refresh() {
    // Keep tests hermetic: never hit the network from test builds (the
    // `test-support` feature also covers downstream crates' test targets via
    // feature unification), and let users opt out entirely.
    // JCODE_FORCE_PRICING_REFRESH=1 re-enables the fetch for manual e2e checks
    // (e.g. `cargo run --example pricing_e2e_check`, which builds with
    // test-support unified in).
    let forced = std::env::var_os("JCODE_FORCE_PRICING_REFRESH").is_some();
    if !forced
        && (cfg!(any(test, feature = "test-support"))
            || std::env::var_os("JCODE_DISABLE_PRICING_REFRESH").is_some())
    {
        return;
    }
    if REFRESH_IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return;
    }
    let work = || async {
        if let Err(e) = refresh_now().await {
            crate::logging::warn(&format!("models.dev pricing refresh failed: {e:#}"));
        }
        REFRESH_IN_FLIGHT.store(false, Ordering::SeqCst);
    };
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        handle.spawn(work());
    } else {
        std::thread::spawn(move || {
            if let Ok(runtime) = tokio::runtime::Runtime::new() {
                runtime.block_on(work());
            } else {
                REFRESH_IN_FLIGHT.store(false, Ordering::SeqCst);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_provider_core::{
        CHEAPNESS_REFERENCE_INPUT_TOKENS, CHEAPNESS_REFERENCE_OUTPUT_TOKENS,
    };
    use std::time::SystemTime;

    /// A models.dev snapshot holding `deepseek/deepseek-v4-pro`.
    const DEEPSEEK_CACHE: &[(&str, &str, ModelCost)] = &[(
        "deepseek",
        "deepseek-v4-pro",
        ModelCost {
            input_usd_per_mtok: 0.66,
            output_usd_per_mtok: 1.98,
            cache_read_usd_per_mtok: Some(0.022),
            cache_write_usd_per_mtok: None,
        },
    )];

    /// Run `f` in an isolated `JCODE_HOME` with an optional `config.toml` and
    /// an optional models.dev cache, then restore the environment.
    fn with_pricing_env<T>(
        config_toml: Option<&str>,
        cache: &[(&str, &str, ModelCost)],
        f: impl FnOnce() -> T,
    ) -> T {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let prev_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());
        clear_memory_cache_for_tests();
        if !cache.is_empty() {
            save_test_cache(cache);
        }
        if let Some(toml) = config_toml {
            std::fs::write(temp.path().join("config.toml"), toml).expect("write config.toml");
        }
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

    /// The canonical request the route catalog prices: 25k in / 5k out.
    fn reference_cost(input_per_mtok: f64, output_per_mtok: f64) -> f64 {
        (input_per_mtok * CHEAPNESS_REFERENCE_INPUT_TOKENS as f64
            + output_per_mtok * CHEAPNESS_REFERENCE_OUTPUT_TOKENS as f64)
            / 1_000_000.0
    }

    #[test]
    fn parses_models_dev_shape() {
        let body = r#"{
            "deepseek": {
                "id": "deepseek",
                "models": {
                    "deepseek-v4-flash": {
                        "cost": {"input": 0.14, "output": 0.28, "cache_read": 0.0028}
                    },
                    "free-model": {"cost": {"input": 0, "output": 0}},
                    "no-cost-model": {}
                }
            },
            "anthropic": {
                "models": {
                    "claude-fable-5": {
                        "cost": {"input": 10, "output": 50, "cache_read": 1, "cache_write": 12.5}
                    }
                }
            }
        }"#;
        let cache = parse_api_response(body).expect("parsed");
        assert_eq!(cache.schema_version, catalog::SCHEMA_VERSION);
        let deepseek = cache.providers.get("deepseek").expect("deepseek");
        assert_eq!(deepseek.len(), 2, "model without cost is skipped");
        let flash = deepseek.get("deepseek-v4-flash").expect("flash");
        assert_eq!(flash.cost.input, Some(0.14));
        assert_eq!(flash.cost.output, Some(0.28));
        assert_eq!(flash.cost.cache_read, Some(0.0028));
        assert_eq!(flash.cost.cache_write, None);

        let fable = cache
            .providers
            .get("anthropic")
            .and_then(|m| m.get("claude-fable-5"))
            .expect("fable");
        assert_eq!(fable.cost.input, Some(10.0));
        assert_eq!(fable.cost.output, Some(50.0));
        assert_eq!(fable.cost.cache_write, Some(12.5));
        assert!(
            fable.tariffs.is_empty(),
            "models.dev has no extension fields today"
        );
    }

    #[test]
    fn rejects_empty_response() {
        assert!(parse_api_response("{}").is_err());
        assert!(parse_api_response("[]").is_err());
    }

    #[test]
    fn provider_key_mapping_covers_jcode_providers() {
        assert_eq!(models_dev_provider_id("claude:api-key"), Some("anthropic"));
        assert_eq!(models_dev_provider_id("openai:api-key"), Some("openai"));
        assert_eq!(
            models_dev_provider_id("openai-compatible:deepseek"),
            Some("deepseek")
        );
        assert_eq!(
            models_dev_provider_id("openai-compatible:nvidia-nim"),
            Some("nvidia")
        );
        assert_eq!(models_dev_provider_id("bedrock"), Some("amazon-bedrock"));
        assert_eq!(models_dev_provider_id("unknown-thing"), None);
    }

    #[test]
    fn lookup_normalizes_model_ids() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let prev_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());
        clear_memory_cache_for_tests();

        save_test_cache(&[
            (
                "anthropic",
                "claude-opus-4-6",
                ModelCost {
                    input_usd_per_mtok: 5.0,
                    output_usd_per_mtok: 25.0,
                    cache_read_usd_per_mtok: Some(0.5),
                    cache_write_usd_per_mtok: Some(6.25),
                },
            ),
            (
                "openrouter",
                "kimi-k2",
                ModelCost {
                    input_usd_per_mtok: 0.5,
                    output_usd_per_mtok: 2.0,
                    cache_read_usd_per_mtok: None,
                    cache_write_usd_per_mtok: None,
                },
            ),
        ]);

        // [1m] suffix strips before lookup.
        let opus = lookup("claude:api-key", "claude-opus-4-6[1m]").expect("priced");
        assert!((opus.input_usd_per_mtok - 5.0).abs() < 1e-9);

        // provider/model ids fall back to the bare model name.
        let kimi = lookup("openrouter", "moonshotai/kimi-k2").expect("priced");
        assert!((kimi.output_usd_per_mtok - 2.0).abs() < 1e-9);
        let pinned = lookup("openrouter", "moonshotai/kimi-k2@Sail Research").expect("priced");
        assert!((pinned.output_usd_per_mtok - 2.0).abs() < 1e-9);

        assert!(lookup("claude:api-key", "claude-unknown").is_none());

        clear_memory_cache_for_tests();
        if let Some(prev) = prev_home {
            crate::env::set_var("JCODE_HOME", prev);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    /// A non-USD card that cannot be completed and has no layer underneath it
    /// must never be rendered as a price: there is no scalar to show, only
    /// "unknown" (spec 4.4, and the warning the resolver emits).
    #[test]
    fn incomplete_foreign_currency_card_is_never_rendered_as_a_price() {
        let config = r#"
[pricing.providers.deepseek]
currency = "CNY"

[pricing.providers.deepseek.models."deepseek-v4-pro".cost]
input = 4.5
"#;
        with_pricing_env(Some(config), &[], || {
            let at = SystemTime::now();
            assert!(
                effective_cost("deepseek", "deepseek-v4-pro", at).is_none(),
                "a one-sided CNY card has no displayable price"
            );
            // The rate-level entry still exists, but it is missing the output
            // direction, so nothing downstream can render it as a price.
            let (entry, currency) =
                effective_entry("deepseek", "deepseek-v4-pro", at).expect("config card");
            assert_eq!(currency, Currency::new("CNY"));
            assert_eq!(entry.cost.input, Some(4.5));
            assert_eq!(entry.cost.output, None, "no direction was invented");
            assert!(matches!(
                config_call_rates("deepseek", "deepseek-v4-pro", at),
                ConfigCallRates::ConfiguredWithoutPrice
            ));
        });
    }

    /// No `[pricing]` section at all: the resolver must hand back exactly the
    /// models.dev numbers, in USD, that `lookup` returned before this feature
    /// existed.
    #[test]
    fn empty_config_is_parity() {
        with_pricing_env(None, DEEPSEEK_CACHE, || {
            let at = SystemTime::now();
            let raw =
                lookup("openai-compatible:deepseek", "deepseek-v4-pro").expect("catalog entry");
            let (entry, currency) =
                effective_entry("openai-compatible:deepseek", "deepseek-v4-pro", at)
                    .expect("resolved");

            assert_eq!(entry.cost.input, Some(raw.input_usd_per_mtok));
            assert_eq!(entry.cost.output, Some(raw.output_usd_per_mtok));
            assert_eq!(entry.cost.cache_read, raw.cache_read_usd_per_mtok);
            assert!(entry.tariffs.is_empty(), "models.dev has no tariff tiers");
            assert!(currency.is_usd(), "models.dev values are USD");

            let money = effective_cost("openai-compatible:deepseek", "deepseek-v4-pro", at)
                .expect("priced");
            assert!(money.currency.is_usd());
            assert!(
                (money.amount - reference_cost(raw.input_usd_per_mtok, raw.output_usd_per_mtok))
                    .abs()
                    <= 1e-9
            );
        });
    }

    /// A hand-written `[pricing.providers]` card outranks models.dev, including
    /// its currency.
    #[test]
    fn config_overrides_models_dev() {
        let config = r#"
[pricing.providers.deepseek]
currency = "CNY"

[pricing.providers.deepseek.models."deepseek-v4-pro".cost]
input = 4.5
output = 13.5
cache_read = 0.15
"#;
        with_pricing_env(Some(config), DEEPSEEK_CACHE, || {
            let at = SystemTime::now();
            let money = effective_cost("deepseek", "deepseek-v4-pro", at).expect("config card");
            assert_eq!(money.currency.as_str(), "CNY");
            assert!((money.amount - reference_cost(4.5, 13.5)).abs() <= 1e-9);

            let (entry, currency) =
                effective_entry("deepseek", "deepseek-v4-pro", at).expect("card");
            assert_eq!(entry.cost.input, Some(4.5));
            assert_eq!(entry.cost.output, Some(13.5));
            assert_eq!(entry.cost.cache_read, Some(0.15));
            assert_eq!(
                entry.cost.cache_write, None,
                "a non-USD card must not inherit USD rates from models.dev"
            );
            assert_eq!(currency.as_str(), "CNY");
        });
    }

    /// F1: `currency = "CNY"` describes the prices the user writes, not the
    /// prices models.dev publishes. With no card for the model, the fallback
    /// must be labelled USD instead of relabelling USD numbers as CNY.
    #[test]
    fn currency_without_price_falls_back_as_usd() {
        let config = r#"
[pricing.providers.deepseek]
currency = "CNY"
"#;
        with_pricing_env(Some(config), DEEPSEEK_CACHE, || {
            let money = effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
                .expect("models.dev fallback");
            assert!(
                money.currency.is_usd(),
                "USD numbers must not be relabelled as CNY"
            );
            assert!((money.amount - reference_cost(0.66, 1.98)).abs() <= 1e-9);
        });
    }

    /// The same F1 rule for a half-written foreign-currency card: no
    /// cross-currency field merge, fall back to the next layer instead.
    #[test]
    fn partial_foreign_currency_card_never_mixes_usd_rates() {
        let config = r#"
[pricing.providers.deepseek]
currency = "CNY"

[pricing.providers.deepseek.models."deepseek-v4-pro".cost]
input = 4.5
"#;
        with_pricing_env(Some(config), DEEPSEEK_CACHE, || {
            let money = effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
                .expect("falls back to models.dev");
            assert!(money.currency.is_usd());
            assert!((money.amount - reference_cost(0.66, 1.98)).abs() <= 1e-9);
        });
    }

    /// A USD card may merge field by field with models.dev, per spec 4.4.
    #[test]
    fn partial_usd_card_merges_models_dev_fields() {
        let config = r#"
[pricing.providers.deepseek.models."deepseek-v4-pro".cost]
input = 1.5
"#;
        with_pricing_env(Some(config), DEEPSEEK_CACHE, || {
            let (entry, currency) =
                effective_entry("deepseek", "deepseek-v4-pro", SystemTime::now())
                    .expect("merged card");
            assert_eq!(entry.cost.input, Some(1.5), "the written field wins");
            assert_eq!(entry.cost.output, Some(1.98), "missing fields fall back");
            assert_eq!(entry.cost.cache_read, Some(0.022));
            assert!(currency.is_usd());
        });
    }

    /// Config provider keys follow the same identity rules as `scope`
    /// (spec 4.2.1): a bare models.dev id also matches compatible profiles.
    #[test]
    fn config_provider_key_matches_compatible_profile() {
        let config = r#"
[pricing.providers.deepseek]
currency = "CNY"

[pricing.providers.deepseek.models."deepseek-v4-pro".cost]
input = 4.5
output = 13.5
"#;
        with_pricing_env(Some(config), DEEPSEEK_CACHE, || {
            let money = effective_cost(
                "openai-compatible:deepseek",
                "deepseek-v4-pro",
                SystemTime::now(),
            )
            .expect("profile resolves to the deepseek card");
            assert_eq!(money.currency.as_str(), "CNY");
        });
    }

    /// A cache written by the previous binary has no `schema_version` and stores
    /// bare cost objects; it must still load.
    #[test]
    fn v1_cache_still_reads() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let prev_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());
        clear_memory_cache_for_tests();

        let path = catalog::cache_path();
        let v1 = serde_json::json!({
            "cached_at_unix_secs": 1_700_000_000u64,
            "providers": {
                "deepseek": {
                    "deepseek-v4-pro": {
                        "input_usd_per_mtok": 0.66,
                        "output_usd_per_mtok": 1.98,
                        "cache_read_usd_per_mtok": 0.022
                    }
                }
            }
        });
        crate::storage::write_json(&path, &v1).expect("write v1 cache");

        let raw = lookup("deepseek", "deepseek-v4-pro").expect("v1 entry still reads");
        assert!((raw.input_usd_per_mtok - 0.66).abs() < 1e-9);
        assert!((raw.output_usd_per_mtok - 1.98).abs() < 1e-9);
        assert_eq!(raw.cache_read_usd_per_mtok, Some(0.022));

        clear_memory_cache_for_tests();
        let loaded = catalog::load_cache().expect("cache loads");
        assert_eq!(loaded.schema_version, 1, "a file without a version is v1");
        assert!(
            loaded.providers["deepseek"]["deepseek-v4-pro"]
                .tariffs
                .is_empty(),
            "v1 entries carry no extension fields"
        );

        // The pure-v1 file is also readable through the resolver.
        let money =
            effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now()).expect("priced");
        assert!(money.currency.is_usd());

        // Writing back upgrades the file to the current schema.
        save_test_cache(DEEPSEEK_CACHE);
        clear_memory_cache_for_tests();
        let rewritten = catalog::load_cache().expect("cache loads");
        assert_eq!(rewritten.schema_version, catalog::SCHEMA_VERSION);

        clear_memory_cache_for_tests();
        if let Some(prev) = prev_home {
            crate::env::set_var("JCODE_HOME", prev);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    /// An unconfigured provider keeps the pre-feature behaviour: models.dev
    /// numbers, labelled USD.
    #[test]
    fn unconfigured_provider_currency_is_usd() {
        with_pricing_env(
            Some("[display]\ncurrency = \"native\"\n"),
            DEEPSEEK_CACHE,
            || {
                let money = effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
                    .expect("priced");
                assert!(money.currency.is_usd());
                assert!(
                    effective_cost("unknown-provider", "whatever", SystemTime::now()).is_none(),
                    "no config rule and no catalog mapping means no price"
                );
            },
        );
    }

    /// `effective_until` in the past with the default `on_rule_expiry =
    /// "fallback"` drops the card and prices from the next layer.
    #[test]
    fn expired_rule_falls_back() {
        let config = r#"
[pricing.providers.deepseek]
currency = "CNY"

[pricing.providers.deepseek.models."deepseek-v4-pro"]
effective_until = "2020-01-01T00:00:00Z"

[pricing.providers.deepseek.models."deepseek-v4-pro".cost]
input = 4.5
output = 13.5
"#;
        with_pricing_env(Some(config), DEEPSEEK_CACHE, || {
            let money = effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
                .expect("expired cards fall back to models.dev");
            assert!(
                money.currency.is_usd(),
                "the CNY card must not leak its currency"
            );
            assert!((money.amount - reference_cost(0.66, 1.98)).abs() <= 1e-9);
        });
    }

    /// `on_rule_expiry = "no_price"` refuses to price rather than falling back.
    #[test]
    fn expired_rule_no_price_suppresses_price() {
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
        with_pricing_env(Some(config), DEEPSEEK_CACHE, || {
            assert!(
                effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now()).is_none(),
                "no_price must suppress the models.dev fallback too"
            );
        });
    }

    /// A card that starts in the future is not in effect yet.
    #[test]
    fn not_yet_effective_rule_falls_back() {
        let config = r#"
[pricing.providers.deepseek]
currency = "CNY"

[pricing.providers.deepseek.models."deepseek-v4-pro"]
effective_from = "2999-01-01T00:00:00Z"

[pricing.providers.deepseek.models."deepseek-v4-pro".cost]
input = 4.5
output = 13.5
"#;
        with_pricing_env(Some(config), DEEPSEEK_CACHE, || {
            let money = effective_cost("deepseek", "deepseek-v4-pro", SystemTime::now())
                .expect("not-yet-effective cards fall back");
            assert!(money.currency.is_usd());
            assert!((money.amount - reference_cost(0.66, 1.98)).abs() <= 1e-9);
        });
    }
}
