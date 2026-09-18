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
mod vendor_files;

/// The per-model rate fields an entry carries, re-exported here so the billing
/// call sites can read a card without reaching into `config`.
pub use crate::config::CostFields;
pub use call_rates::{
    CallRateCard, ConfigCallRates, PricingNotice, config_call_rates, vendor_file_rule_out_of_effect,
};
pub use catalog::ModelCost;
pub use entry::{ModelPricingEntry, RuleOutOfEffect};
pub use fx::{FxTable, convert};
pub use generation::pricing_generation;
pub use sources::{pricing_config, pricing_config_error};
#[cfg(test)]
pub(crate) use vendor_files::save_test_vendor;

/// The file name of jcode's own models.dev cache, so the config layer can refuse
/// a bare `[pricing.providers.<vendor>].file` name that would shadow it.
pub(crate) fn catalog_cache_file_name() -> &'static str {
    catalog::CACHE_FILE
}

/// The file name of jcode's own vendor files cache, for the same collision
/// guard.
pub(crate) fn vendor_files_cache_file_name() -> &'static str {
    vendor_files::VENDOR_FILES_CACHE_FILE
}

use catalog::PricingCache;
#[cfg(test)]
use catalog::parse_api_response;
use catalog::{CACHE_TTL_SECS, REFRESH_IN_FLIGHT, load_cache, now_unix_secs, refresh_now};
#[cfg(test)]
pub(crate) use catalog::{clear_memory_cache_for_tests, save_test_cache};
#[cfg(test)]
#[path = "call_rates_tests.rs"]
mod call_rates_tests;
#[cfg(test)]
#[path = "comparable_cost_tests.rs"]
mod comparable_cost_tests;
#[cfg(test)]
#[path = "vendor_file_tests.rs"]
mod vendor_file_tests;

use jcode_provider_core::{
    CHEAPNESS_REFERENCE_INPUT_TOKENS, CHEAPNESS_REFERENCE_OUTPUT_TOKENS, Currency, Money,
    RouteCheapnessEstimate,
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
    lookup_entry(jcode_provider, model).and_then(|entry| entry.to_model_cost())
}

/// The models.dev entry itself, extension fields included.
///
/// [`lookup`] flattens to [`ModelCost`], which cannot carry the long-context
/// tiers models.dev states as `cost.context_over_200k`. The billing path needs
/// them, so it reads this instead.
pub(crate) fn lookup_entry(jcode_provider: &str, model: &str) -> Option<ModelPricingEntry> {
    let provider_id = models_dev_provider_id(jcode_provider)?;
    let cache = ensure_cache_fresh()?;
    let models = cache.providers.get(provider_id)?;
    let model = normalize_model_id(model);
    if let Some(entry) = models.get(model) {
        return Some(entry.clone());
    }
    // OpenRouter-style ids (`anthropic/claude-...`) may reach here with the
    // provider prefix still attached; retry on the bare model name.
    if let Some((_, bare)) = model.rsplit_once('/') {
        return models.get(bare).cloned();
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
/// Priority (spec 4.4): hand-written inline `[pricing.providers]` rules first,
/// then `[pricing.providers.<vendor>].file` rules, then models.dev.
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
///
/// A long-context tier needs the call's input token count, which this entry
/// point does not take: it prices the **base tier** (below the card's first
/// `min_input_tokens`). Use [`effective_entry_at_size`] when the count is known.
pub fn effective_entry(
    provider: &str,
    model: &str,
    at: SystemTime,
) -> Option<(ModelPricingEntry, Currency)> {
    effective_entry_at_size(provider, model, at, None)
}

/// [`effective_entry`] with the call's reported input token count, so a card's
/// long-context tier is selected. `None` is the base tier.
pub fn effective_entry_at_size(
    provider: &str,
    model: &str,
    at: SystemTime,
    input_tokens: Option<u64>,
) -> Option<(ModelPricingEntry, Currency)> {
    match sources::config_price(provider, model, at) {
        sources::ConfigPrice::NoPrice => None,
        sources::ConfigPrice::Hit { entry, currency } => {
            let resolved =
                sources::resolve_card(*entry, currency, provider, model, at, input_tokens);
            Some((resolved.entry, resolved.currency))
        }
        // A rule that is out of effect is not this layer's answer either: the
        // next layer prices the model, exactly as if the rule had not been
        // written for this instant (the expiry itself is reported by
        // `config_call_rates`, which is the side that has to label it).
        sources::ConfigPrice::OutOfEffect(_) | sources::ConfigPrice::Absent => {
            // A `[pricing.providers.<vendor>].file` rule sits strictly between
            // the hand-written cards and models.dev (spec 4.4).
            match vendor_file_card_at_size(provider, model, at, input_tokens) {
                Some(card) => Some((card.entry, card.currency)),
                None => models_dev_card_at_size(provider, model, at, input_tokens),
            }
        }
    }
}

/// The vendor-file layer's card for `(provider, model)` at `at`, with the
/// vendor and file that supplied it, or `None` when no vendor file prices the
/// model.
///
/// A vendor file rule is a price card like any other: the merge and the currency
/// rules are the config card's, not a second implementation. [`sources::resolve_card`]
/// fills what the file leaves out from models.dev when the currencies agree and
/// refuses to relabel a foreign-currency file's numbers when they do not (F1).
/// `None` in that second case is what sends the call on to models.dev instead of
/// pricing it from half a card.
///
/// Vendor files are matched by **model id, not route**, so a
/// `provider = OpenRouter, model = deepseek-flash` call reaches DeepSeek's file.
/// Vendors are consulted in `BTreeMap` (lexicographic) order; the first file
/// that states a rule for the model decides. (A reserved future `route = [...]`
/// field would narrow this; it is not implemented.)
pub(crate) fn vendor_file_card_at_size(
    provider: &str,
    model: &str,
    at: SystemTime,
    input_tokens: Option<u64>,
) -> Option<VendorFileCard> {
    let config = sources::pricing_config();
    for (vendor, pricing) in &config.providers {
        let Some(path) = pricing.file.as_deref() else {
            continue;
        };
        let Some(entry) = vendor_files::vendor_rule(vendor, path, model) else {
            continue;
        };
        // The first file that states a rule for this model decides: either it
        // prices the call, or its validity window is why it did not. The same
        // first-file rule is what `vendor_file_out_of_effect` mirrors.
        if entry.out_of_effect_reason(at).is_some() {
            return None;
        }
        // Asked before the merge below, and before the card is consumed: the
        // schedule that picks the tariff is the file's own, and the selection
        // (and the tier) is what the billing memo has to key on.
        let selected = rules::resolve_tier(&entry, at, input_tokens);
        let context_tier = selected.as_ref().and_then(|tier| tier.context_tier);
        let tariff = selected.and_then(|tier| tier.tariff);
        let currency = pricing.currency.clone().unwrap_or_else(Currency::usd);
        let resolved = sources::resolve_card(entry, currency, provider, model, at, input_tokens);
        return resolved.owns_price.then_some(VendorFileCard {
            entry: resolved.entry,
            currency: resolved.currency,
            vendor: vendor.clone(),
            path: path.to_path_buf(),
            tariff,
            context_tier,
        });
    }
    None
}

/// The vendor-file layer's card, with what a caller needs to label it.
pub(crate) struct VendorFileCard {
    pub(crate) entry: ModelPricingEntry,
    pub(crate) currency: Currency,
    /// The `[pricing.providers.<vendor>]` key the file hangs under.
    pub(crate) vendor: String,
    /// The resolved local file the rules were read from.
    pub(crate) path: std::path::PathBuf,
    /// The name of the tariff the file's schedule selected at the call's
    /// instant, or `None` when the file's own base `cost` applies.
    ///
    /// A file's schedule is time-dependent, and a derived price is memoized, so
    /// the tariff in force is part of what identifies the price the memo holds:
    /// without it a memo filled off-peak would keep billing off-peak rates after
    /// the window closed (the schedule bug Task 10 introduced).
    pub(crate) tariff: Option<String>,
    /// The file's long-context tier in force for the call's input token count,
    /// if it declares one.
    ///
    /// A derived price is memoized per `(model, tariff, generation)`, so without
    /// this a call billed at a file's higher long-context rates would leave
    /// those rates cached for the next short call (the same trap the models.dev
    /// `context_over_200k` tier has).
    pub(crate) context_tier: Option<u64>,
}

/// The models.dev layer's card, always USD, with the long-context tier for
/// `input_tokens` applied.
///
/// Reads the full entry rather than the flattened [`ModelCost`], so the
/// `context_over_200k` rates the catalog parsed into `context_tiers` survive to
/// the billing path instead of being dropped here. The derived billing layer
/// (curated tables, OpenRouter caches, models.dev) uses this too, which is what
/// makes a >200k models.dev call cost more than the base rate.
pub(crate) fn models_dev_card_at_size(
    provider: &str,
    model: &str,
    at: SystemTime,
    input_tokens: Option<u64>,
) -> Option<(ModelPricingEntry, Currency)> {
    let mut entry = lookup_entry(provider, model)?;
    if let Some(selected) = rules::resolve_tier(&entry, at, input_tokens) {
        entry.cost = selected.cost;
    }
    Some((entry, Currency::usd()))
}

/// What identifies the derived-layer rate card a call would be billed from at
/// one instant, for the caller that memoizes a derived price.
///
/// The derived layers are the only ones memoized - a hand-written
/// `[pricing.providers]` card is resolved at each call's own instant - so this
/// is exactly the state of the derived chain that can change what a memoized
/// price *means*:
///
/// * `vendor_file` names the `[pricing.providers.<vendor>].file` and the tariff
///   its schedule selected at the call's instant, or `None` when no file prices
///   the call and the catalogs jcode ships answer. A memo keyed on this identity
///   re-resolves when the file's window changes, so a call in the peak window
///   never reuses the price a call in the off-peak window cached.
/// * `context_tier` is the long-context tier in force (`min_input_tokens`), or
///   `None` for the base tier. Without it a long call's higher rates would stay
///   cached for the next short one.
///
/// Both are read with the *call's* instant, never the wall clock, which is what
/// makes peak/off-peak files behave like the hand-written rules they mirror.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedPriceIdentity {
    /// `"{vendor}#{tariff}"` for the file in force, else `None`.
    pub vendor_file: Option<String>,
    /// The long-context tier in force, else `None`.
    pub context_tier: Option<u64>,
}

/// The derived-layer price identity for `(provider, model)` at `at`.
///
/// The vendor-file layer outranks models.dev, so a file's own tariff and tier
/// are asked for first; only when no file prices the call does models.dev's
/// `context_over_200k` tier answer.
pub fn derived_price_identity(
    provider: &str,
    model: &str,
    at: SystemTime,
    input_tokens: Option<u64>,
) -> DerivedPriceIdentity {
    if let Some(card) = vendor_file_card_at_size(provider, model, at, input_tokens) {
        return DerivedPriceIdentity {
            vendor_file: Some(format!(
                "{}#{}",
                card.vendor,
                card.tariff.as_deref().unwrap_or("base")
            )),
            context_tier: card.context_tier,
        };
    }
    DerivedPriceIdentity {
        vendor_file: None,
        context_tier: models_dev_context_tier_in_force(provider, model, at, input_tokens),
    }
}

pub fn models_dev_context_tier_in_force(
    provider: &str,
    model: &str,
    at: SystemTime,
    input_tokens: Option<u64>,
) -> Option<u64> {
    let entry = lookup_entry(provider, model)?;
    rules::resolve_tier(&entry, at, input_tokens)?.context_tier
}

/// The vendor file that states a rule for `(provider, model)` but is out of
/// effect at `at`, as `(vendor, reason)`, if the file layer is what sends the
/// price below it.
///
/// This is the vendor file's half of the F8/F20 marker. The call is still priced
/// by the next layer (that is the existing fall-through); the notice is what
/// tells the user their file stopped applying instead of a models.dev number
/// taking over silently.
///
/// Only the *first* file that states an entry for the model can be that reason,
/// mirroring [`vendor_file_card_at_size`]'s iteration order: an earlier file
/// that states nothing for this model would not have priced the call even while
/// in effect, and an earlier file that *is* in effect wins outright, so neither
/// is reported.
pub(super) fn vendor_file_out_of_effect(
    model: &str,
    at: SystemTime,
) -> Option<(String, RuleOutOfEffect)> {
    let config = sources::pricing_config();
    for (vendor, pricing) in &config.providers {
        let Some(path) = pricing.file.as_deref() else {
            continue;
        };
        let Some(entry) = vendor_files::vendor_rule(vendor, path, model) else {
            continue;
        };
        return entry
            .out_of_effect_reason(at)
            .map(|reason| (vendor.clone(), reason));
    }
    None
}

/// The vendor file that prices `(provider, model)` at `at`, as
/// `(vendor, file, currency)`, or `None` when no file can price it.
///
/// This exists so `/pricing` can name the file layer in exactly the case its own
/// "no `[pricing.providers]` rule prices this model" text is about: without it
/// the report listed only "models.dev, a provider cache, or the fallback
/// estimate" while the `reference request` figure beside it came from a file.
pub fn vendor_file_for(
    provider: &str,
    model: &str,
    at: SystemTime,
) -> Option<(String, std::path::PathBuf, Currency)> {
    vendor_file_card_at_size(provider, model, at, None)
        .map(|card| (card.vendor, card.path, card.currency))
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
///
/// The reference request has no size of its own, so this prices the **base
/// tier** and deliberately ignores any long-context overlay: the number is a
/// cheapness comparison, not a prediction of one call's bill.
pub fn effective_cost(provider: &str, model: &str, at: SystemTime) -> Option<Money> {
    let (entry, currency) = effective_entry(provider, model, at)?;
    let input = entry.cost.input?;
    let output = entry.cost.output?;
    let amount = (input * CHEAPNESS_REFERENCE_INPUT_TOKENS as f64
        + output * CHEAPNESS_REFERENCE_OUTPUT_TOKENS as f64)
        / 1_000_000.0;
    Some(Money::new(amount, currency))
}

/// The `[pricing]` tariff in effect for `(provider, model)` at `at`.
///
/// Provenance for a caller that has to *explain* a price rather than compute
/// one: the name is the `tariffs.<name>` key that the hand-written card's
/// schedule (or its `default_tariff`) selected. `None` means no hand-written rule
/// prices this model at `at`, or the card applies its own `cost` with no tariff
/// in force - the two cases a cost view should not have to guess between.
///
/// The long-context overlay is *not* reflected here: it changes the rates, not
/// the tariff name. A caller with a token count reports it separately (see
/// [`context_tier_thresholds`]); a caller without one is on the base
/// tier.
pub fn selected_config_tariff(provider: &str, model: &str, at: SystemTime) -> Option<String> {
    let sources::ConfigPrice::Hit { entry, .. } = sources::config_price(provider, model, at) else {
        return None;
    };
    rules::resolve_tier(&entry, at, None)?.tariff
}

/// The long-context thresholds of the card that prices `(provider, model)` at
/// `at`, in declaration order.
///
/// A caller that has to *explain* a price (`/pricing`) has no token count and
/// therefore prices the base tier. Listing the declared thresholds is how it
/// stays honest: the user learns which tiers a real call can cross instead of
/// reading the base figure as the only one. The hand-written card wins when one
/// is in effect; otherwise the models.dev card's own `context_over_200k` tier is
/// what a real call crosses, so it is the one named.
pub fn context_tier_thresholds(provider: &str, model: &str, at: SystemTime) -> Vec<u64> {
    let entry = match sources::config_price(provider, model, at) {
        sources::ConfigPrice::Hit { entry, .. } => entry,
        // A `no_price` rule refuses to price the call at all, so no tier of it
        // can ever apply.
        sources::ConfigPrice::NoPrice => return Vec::new(),
        sources::ConfigPrice::Absent | sources::ConfigPrice::OutOfEffect(_) => {
            match lookup_entry(provider, model) {
                Some(entry) => Box::new(entry),
                None => return Vec::new(),
            }
        }
    };
    entry
        .context_tiers
        .iter()
        .map(|tier| tier.min_input_tokens)
        .collect()
}

/// [`RouteCheapnessEstimate::estimated_reference_cost_micros`] expressed in the
/// FX base currency, so routes priced in different currencies can be ordered.
///
/// The raw estimate is only meaningful inside one currency: 7 CNY/Mtok is
/// cheaper than 2 USD/Mtok at any plausible rate, yet the raw numbers order them
/// the other way round. That is not hypothetical - a hand-written
/// `[pricing.providers]` card stamps its own currency onto the estimate (see
/// `config_price_estimate`), so a CNY card and a USD catalog sit side by side in
/// the model picker.
///
/// The rate table is the live `[pricing].fx_rates`, and `None` means it cannot
/// convert this estimate's currency: a caller must then keep the route out of
/// cross-currency ordering instead of comparing unlike units.
pub fn comparable_reference_cost_micros(estimate: &RouteCheapnessEstimate) -> Option<u64> {
    let micros = estimate.estimated_reference_cost_micros?;
    let fx = FxTable::from_config(&pricing_config());
    if estimate.currency == fx.base {
        return Some(micros);
    }
    let amount = Money::new(micros as f64 / 1_000_000.0, estimate.currency.clone());
    let converted = convert(&amount, &fx.base, &fx)?;
    let micros = (converted.amount * 1_000_000.0).round();
    (micros.is_finite() && micros >= 0.0 && micros <= u64::MAX as f64).then_some(micros as u64)
}

/// Whether a background pricing refresh may start at all.
///
/// Tests must never reach the network (the `test-support` feature also covers
/// downstream crates' test targets via feature unification), and users can opt
/// out entirely. JCODE_FORCE_PRICING_REFRESH=1 re-enables the fetch for manual
/// e2e checks (e.g. `cargo run --example pricing_e2e_check`, which builds with
/// the `test-support` feature unified in). Only the models.dev catalog fetches;
/// user vendor files are always local files.
pub(crate) fn background_refresh_allowed() -> bool {
    if std::env::var_os("JCODE_FORCE_PRICING_REFRESH").is_some() {
        return true;
    }
    !cfg!(any(test, feature = "test-support"))
        && std::env::var_os("JCODE_DISABLE_PRICING_REFRESH").is_none()
}

/// Spawn one background refresh at a time. Safe to call from sync contexts;
/// uses a thread + ad-hoc runtime when no Tokio runtime is active.
pub fn schedule_refresh() {
    if !background_refresh_allowed() {
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
                        "cost": {
                            "input": 0.14,
                            "output": 0.28,
                            "cache_read": 0.0028,
                            "context_over_200k": {"input": 0.28, "output": 0.56}
                        }
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
        assert_eq!(
            flash.context_tiers.len(),
            1,
            "models.dev's `context_over_200k` rates must be consumed, not dropped"
        );
        assert_eq!(flash.context_tiers[0].min_input_tokens, 200_000);
        assert!(matches!(
            flash.context_tiers[0].tariff,
            crate::config::Tariff::Absolute(_)
        ));

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

    /// models.dev's native `context_over_200k` rates survive the whole path: the
    /// response is parsed into the cache, the lookup applies the tier, and a
    /// call above the threshold is priced above the base rate. This is the
    /// end-to-end consumption check for the field that used to be dropped.
    #[test]
    fn models_dev_context_over_200k_is_consumed_end_to_end() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let prev_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());
        clear_memory_cache_for_tests();

        // The upstream shape: cost.context_over_200k = { input, output }.
        let body = r#"{
            "deepseek": {"models": {"deepseek-v4-pro": {
                "cost": {
                    "input": 0.28, "output": 0.42,
                    "context_over_200k": {"input": 0.56, "output": 0.84}
                }
            }}}
        }"#;
        let cache = parse_api_response(body).expect("parse the models.dev response");
        catalog::save_cache(&cache);
        clear_memory_cache_for_tests();

        let at = SystemTime::now();
        let base = effective_entry_at_size("deepseek", "deepseek-v4-pro", at, Some(200_000))
            .expect("models.dev card");
        assert_eq!(
            base.0.cost.input,
            Some(0.28),
            "exactly 200k is the base rate"
        );
        let long = effective_entry_at_size("deepseek", "deepseek-v4-pro", at, Some(200_001))
            .expect("models.dev card");
        assert_eq!(
            long.0.cost.input,
            Some(0.56),
            "the >200k input rate must be billed, not the base rate"
        );
        assert_eq!(long.0.cost.output, Some(0.84));
        let unknown = effective_entry_at_size("deepseek", "deepseek-v4-pro", at, None)
            .expect("models.dev card");
        assert_eq!(
            unknown.0.cost.input,
            Some(0.28),
            "no count prices the base tier"
        );

        // The derived billing layer (what the TUI prices a models.dev model
        // with) sees the tier too, instead of the flat base rates.
        let estimate = crate::provider::pricing::derived_pricing_for_source_at_size(
            "deepseek",
            "deepseek-v4-pro",
            None,
            at,
            Some(200_001),
        )
        .expect("derived estimate");
        assert_eq!(estimate.input_price_per_mtok_micros, Some(560_000));
        assert_eq!(
            models_dev_context_tier_in_force("deepseek", "deepseek-v4-pro", at, Some(200_001)),
            Some(200_000)
        );
        assert_eq!(
            models_dev_context_tier_in_force("deepseek", "deepseek-v4-pro", at, Some(200_000)),
            None,
            "the memo key marker is only set when a tier really applies"
        );

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
                config_call_rates("deepseek", "deepseek-v4-pro", at, None),
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

    /// F10/F18, the third cache point: the resolver's own memoized `[pricing]`
    /// view has to follow a config reload. A hand edit plus a reload must change
    /// the price on the very next read, with no pricing cache cleared by the
    /// caller, and it must advance `pricing_generation` — the counter the TUI
    /// price memo and the route catalog fold into their keys.
    #[test]
    fn a_config_edit_reprices_without_clearing_the_pricing_memo() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let prev_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());
        clear_memory_cache_for_tests();
        crate::config::invalidate_config_cache();

        let card = |input: f64, output: f64| {
            format!(
                "[pricing.providers.deepseek]\ncurrency = \"CNY\"\n\n\
                 [pricing.providers.deepseek.models.\"deepseek-v4-pro\".cost]\n\
                 input = {input}\noutput = {output}\n"
            )
        };
        let path = temp.path().join("config.toml");
        let at = SystemTime::now();

        std::fs::write(&path, card(4.5, 13.5)).expect("write config.toml");
        crate::config::invalidate_config_cache();
        let before = effective_cost("deepseek", "deepseek-v4-pro", at).expect("first card prices");
        assert_eq!(before.currency.as_str(), "CNY");
        assert!((before.amount - reference_cost(4.5, 13.5)).abs() <= 1e-9);
        let generation_before = pricing_generation();

        // Hand edit + reload. Nothing clears a pricing cache in between: the
        // resolver has to notice the new config on its own.
        std::fs::write(&path, card(9.0, 27.0)).expect("rewrite config.toml");
        crate::config::invalidate_config_cache();

        let after = effective_cost("deepseek", "deepseek-v4-pro", at).expect("second card prices");
        assert!(
            (after.amount - reference_cost(9.0, 27.0)).abs() <= 1e-9,
            "the edited card must be in force, got {}",
            after.amount
        );
        assert!(
            pricing_generation() > generation_before,
            "a config reload must advance the pricing generation"
        );

        clear_memory_cache_for_tests();
        crate::config::invalidate_config_cache();
        if let Some(prev) = prev_home {
            crate::env::set_var("JCODE_HOME", prev);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
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

    /// A models.dev entry states its rates in the same vocabulary a
    /// hand-written card does, so it must be held to the same finite,
    /// non-negative rule and fail with the field path that caused it.
    #[test]
    fn a_sheet_stating_a_negative_rate_is_rejected_with_its_path() {
        let body = r#"{"deepseek":{"models":{"deepseek-v4-pro":{
            "cost":{"input":-5.0,"output":1.0}
        }}}}"#;
        let error = parse_api_response(body).expect_err("a negative sheet rate is rejected");
        let message = format!("{error:#}");
        assert!(
            message.contains("deepseek.deepseek-v4-pro.cost.input"),
            "the sheet error must name the field: {message}"
        );
    }

    /// A `no_price` rule refuses the call, and the route/picker path must refuse
    /// it too: a models.dev estimate there would contradict `/usage` and the cost
    /// widget, which correctly show "unknown" (spec 4.4).
    #[test]
    fn a_no_price_rule_yields_no_route_estimate() {
        let config = r#"
[pricing.providers.deepseek.models."deepseek-v4-pro"]
effective_until = "2020-01-01T00:00:00Z"
on_rule_expiry = "no_price"
"#;
        with_pricing_env(Some(config), DEEPSEEK_CACHE, || {
            let at = SystemTime::now();
            assert!(matches!(
                config_call_rates("deepseek", "deepseek-v4-pro", at, None),
                ConfigCallRates::ConfiguredWithoutPrice
            ));
            assert!(
                crate::provider::pricing::metered_pricing_for_source_at(
                    "deepseek",
                    "deepseek-v4-pro",
                    None,
                    at,
                )
                .is_none(),
                "the picker must not show a models.dev estimate for a no_price rule"
            );
        });
    }

    /// The route/picker path refuses a card that cannot price the call even when
    /// a *derived* layer (here the curated tables, since there is no models.dev
    /// cache) could; the `!owns_price`/incomplete-card case is the same refusal
    /// as `no_price`.
    #[test]
    fn an_incomplete_card_suppresses_the_route_estimate_even_when_a_catalog_could_price_it() {
        let config = r#"
[pricing.providers.claude]
currency = "CNY"

[pricing.providers.claude.models."claude-sonnet-4-6".cost]
input = 4.5
"#;
        with_pricing_env(Some(config), &[], || {
            let at = SystemTime::now();
            assert!(matches!(
                config_call_rates("claude:api-key", "claude-sonnet-4-6", at, None),
                ConfigCallRates::ConfiguredWithoutPrice
            ));
            assert!(
                crate::provider::pricing::metered_pricing_for_source_at(
                    "claude:api-key",
                    "claude-sonnet-4-6",
                    None,
                    at,
                )
                .is_none(),
                "no layer may price a route the user's own incomplete card claims"
            );
        });
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
