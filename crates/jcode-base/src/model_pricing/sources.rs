//! Hand-written price sources: the `[pricing.providers]` layer and the rules
//! that decide which layer owns a `(provider, model)` pair.
//!
//! This is the highest-priority price source (spec 4.4): it outranks the
//! curated static tables, OpenRouter's own caches, and models.dev. The optional
//! `[[pricing.sources]]` registry lands later and plugs in between this layer
//! and models.dev.
//!
//! Two rules from the spec are enforced here and nowhere else:
//!
//! * provider keys follow the `scope` identity rules (spec 4.2.1) so
//!   `[pricing.providers."deepseek"]` also covers `openai-compatible:deepseek`;
//! * **currency follows the price** (F1): a card in a currency other than the
//!   next layer's never inherits that layer's numbers.

use crate::config::PricingConfig;
use crate::config::pricing::{ProviderPricing, validate};
use crate::model_pricing::entry::{ModelPricingEntry, RuleOutOfEffect};
use crate::model_pricing::rules;
use crate::model_pricing::{ModelCost, models_dev_provider_id, normalize_model_id};
use jcode_provider_core::Currency;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// The config layer's answer for one `(provider, model)` pair.
pub(super) enum ConfigPrice {
    /// No `[pricing.providers]` rule claims this pair: ask the next layer.
    Absent,
    /// A rule claims it and these are its rates.
    Hit {
        entry: Box<ModelPricingEntry>,
        currency: Currency,
    },
    /// A rule claims it but is out of effect with `on_rule_expiry = "no_price"`:
    /// refuse to price rather than fall back to a worse estimate (spec 4.4).
    NoPrice,
    /// A rule claims it but its validity window does not cover `at`, and
    /// `on_rule_expiry = "fallback"` sends the call to the next layer. That
    /// layer prices it, and callers must label *that* price as the fallback
    /// (spec F8/F20): the user wrote the rule and has to learn it stopped
    /// applying, otherwise a models.dev number silently replaces their own.
    OutOfEffect(RuleOutOfEffect),
}

/// Validated `[pricing]` snapshot, memoized against the loaded config instance.
///
/// Validation is pure but allocation-heavy, and the route catalog asks for a
/// price once per route, so the parsed view is cached and invalidated whenever
/// `config()` reloads (a reload leaks a brand-new `Config`, so its address is a
/// stable identity for "the config the user currently has").
static PRICING_CONFIG: Mutex<Option<(usize, Arc<PricingConfig>)>> = Mutex::new(None);

/// The validated `[pricing]` view of the loaded config.
///
/// This is the one price-related view of the config that callers outside this
/// module (currency display, for instance) should read: it is validated, memoized
/// against the loaded config instance, and invalidated on reload.
pub fn pricing_config() -> Arc<PricingConfig> {
    let config = crate::config::config();
    let identity = std::ptr::from_ref(config) as usize;
    if let Ok(memo) = PRICING_CONFIG.lock()
        && let Some((cached, parsed)) = memo.as_ref()
        && *cached == identity
    {
        return Arc::clone(parsed);
    }

    let parsed = match validate(&config.pricing) {
        Ok((parsed, warnings)) => {
            for warning in warnings {
                crate::logging::warn(&format!("pricing config: {warning}"));
            }
            parsed
        }
        Err(error) => {
            // An invalid `[pricing]` section must never take pricing down with
            // it: ignore it, keep the pre-feature behaviour, and say why.
            crate::logging::warn(&format!(
                "ignoring invalid [pricing] config ({error}); falling back to models.dev"
            ));
            PricingConfig::default()
        }
    };
    let parsed = Arc::new(parsed);
    if let Ok(mut memo) = PRICING_CONFIG.lock() {
        // Only a *new* identity counts as a change, and the comparison is made
        // against the entry being replaced: when two threads race the same
        // reload the loser sees the winner's identity already stored and does
        // not bump a second time.
        let changed = memo.as_ref().map(|(cached, _)| *cached) != Some(identity);
        *memo = Some((identity, Arc::clone(&parsed)));
        drop(memo);
        if changed {
            // This is the one place that observes "the config in force is not
            // the one the priced view was built from", so it is the one place
            // that can tell the price memos outside this module to re-derive.
            super::generation::bump_pricing_generation();
        }
    }
    parsed
}

/// The config-authoritative card for `(source_key, model)`, if the user wrote
/// one that is in effect at `at`.
pub(super) fn config_price(source_key: &str, model: &str, at: SystemTime) -> ConfigPrice {
    let config = pricing_config();
    if config.providers.is_empty() {
        return ConfigPrice::Absent;
    }
    let Some(provider) = find_provider(&config, source_key) else {
        return ConfigPrice::Absent;
    };
    let Some(rule) = find_rule(provider, model) else {
        return ConfigPrice::Absent;
    };

    let entry = ModelPricingEntry::from_rule(rule);
    // Only the validity window is decided here. Peak/off-peak selection runs in
    // `resolve_card`, *after* the field-level merge: a tariff scales the
    // effective base card, so scaling before models.dev fills in the fields a
    // partial card leaves out would scale the written fields and leave the
    // merged ones behind.
    if let Some(reason) = entry.out_of_effect_reason(at) {
        return match entry.on_rule_expiry {
            crate::config::OnRuleExpiry::Fallback => {
                crate::logging::warn(&format!(
                    "pricing rule for {source_key}/{model} is out of effect; \
                     falling back to the next price source"
                ));
                ConfigPrice::OutOfEffect(reason)
            }
            crate::config::OnRuleExpiry::NoPrice => ConfigPrice::NoPrice,
        };
    }

    ConfigPrice::Hit {
        entry: Box::new(entry),
        currency: provider.currency.clone().unwrap_or_else(Currency::usd),
    }
}

/// Merge what the config card leaves out with the next layer, honoring F1.
///
/// * A card already in USD (the models.dev layer's currency) merges field by
///   field, so "only `input` written" keeps models.dev's output price.
/// * A card in any other currency never inherits models.dev's numbers, because
///   a USD figure would be relabelled as, say, CNY. If such a card cannot price
///   the model on its own, the next layer wins outright.
pub(super) fn resolve_card(
    mut entry: ModelPricingEntry,
    mut currency: Currency,
    provider: &str,
    model: &str,
    at: SystemTime,
) -> ResolvedCard {
    let fallback = crate::model_pricing::lookup(provider, model);
    let mut from_config = true;

    if currency.is_usd() {
        // Same currency as the next layer, so missing fields merge per field.
        if let Some(fallback) = fallback {
            merge_same_currency(&mut entry, &fallback);
        }
    } else if entry.cost.input.is_some() && entry.cost.output.is_some() {
        // A complete foreign-currency card stands on its own.
    } else if let Some(fallback) = fallback {
        crate::logging::warn(&format!(
            "pricing rule for {provider}/{model} is incomplete and denominated in {currency}; \
             using models.dev values (USD) instead of relabelling them"
        ));
        entry = ModelPricingEntry::from_model_cost(fallback);
        currency = Currency::usd();
        from_config = false;
    } else {
        // Non-USD, incomplete, and nothing underneath it: neither direction may
        // borrow the other layer's numbers (F1), so this card can never price
        // the model. Warn rather than let a partial card read as a configured
        // price (spec 4.4): billing refuses it and display must render
        // "unknown" instead of a one-sided figure.
        crate::logging::warn(&format!(
            "pricing rule for {provider}/{model} is incomplete and denominated in {currency}, \
             and no fallback price exists; this model cannot be priced until the rule supplies \
             both input and output"
        ));
    }

    // Peak/off-peak selection runs on the *effective* card, i.e. after any
    // field-level merge above: a tariff scales the whole base, and running it
    // first would scale only the fields the card happened to write.
    if let Some(selected) = rules::resolve_tier(&entry, at) {
        crate::logging::debug(&format!(
            "pricing: {provider}/{model} uses tariff `{}`",
            selected.tariff.as_deref().unwrap_or("base")
        ));
        entry.cost = selected.cost;
    }

    ResolvedCard {
        entry,
        currency,
        from_config,
    }
}

/// A resolved card, plus which layer actually supplied the rates.
pub(super) struct ResolvedCard {
    pub(super) entry: ModelPricingEntry,
    pub(super) currency: Currency,
    /// `false` when the config card could not price the model and the rates are
    /// the next layer's, so callers can keep labelling sources truthfully.
    pub(super) from_config: bool,
}

/// Fill unset fields from a fallback layer that is known to use the same
/// currency.
fn merge_same_currency(entry: &mut ModelPricingEntry, fallback: &ModelCost) {
    entry.cost.input = entry.cost.input.or(Some(fallback.input_usd_per_mtok));
    entry.cost.output = entry.cost.output.or(Some(fallback.output_usd_per_mtok));
    entry.cost.cache_read = entry.cost.cache_read.or(fallback.cache_read_usd_per_mtok);
    entry.cost.cache_write = entry.cost.cache_write.or(fallback.cache_write_usd_per_mtok);
}

/// Find the `[pricing.providers]` section that owns `source_key`.
///
/// Exact key wins; otherwise any key that resolves to the same models.dev
/// provider id or compatible-profile id matches (spec 4.2.1).
fn find_provider<'a>(config: &'a PricingConfig, source_key: &str) -> Option<&'a ProviderPricing> {
    if let Some(exact) = config.providers.get(source_key) {
        return Some(exact);
    }
    config
        .providers
        .iter()
        .find(|(key, _)| provider_key_matches(key, source_key))
        .map(|(_, provider)| provider)
}

/// Whether a `[pricing.providers]` key refers to the same provider as
/// `source_key`, using the three normalized identity forms of spec 4.2.1.
fn provider_key_matches(config_key: &str, source_key: &str) -> bool {
    let key = config_key.trim();
    if key == source_key {
        return true;
    }
    // `openai-compatible:foo` and `foo` are the same compatible profile.
    let key_profile = key.strip_prefix("openai-compatible:");
    if key_profile == Some(source_key) {
        return true;
    }
    if source_key.strip_prefix("openai-compatible:") == Some(key) {
        return true;
    }
    // Anything that maps to the same models.dev provider id is the same
    // provider (covers the jcode slugs, api-key routes, and profiles).
    match (
        models_dev_provider_id(key),
        models_dev_provider_id(source_key),
    ) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

/// Find the model rule the provider section declares for `model`.
fn find_rule<'a>(
    provider: &'a ProviderPricing,
    model: &str,
) -> Option<&'a crate::config::ModelPricingRule> {
    if let Some(rule) = provider.models.get(model) {
        return Some(rule);
    }
    // Callers may hand over ids carrying jcode-local decorations (`[1m]`,
    // `@pin`); the catalog strips them, so config lookup does too.
    let normalized = normalize_model_id(model);
    if normalized != model {
        return provider.models.get(normalized);
    }
    None
}
