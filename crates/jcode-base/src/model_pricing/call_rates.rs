//! The billing-facing rate lookup: which rate card prices one API call, at the
//! call's own instant.
//!
//! The route catalog wants one scalar ("how expensive is this model"), so
//! `effective_cost` collapses a card into a reference request. Billing is a
//! different question: it needs the per-token rates, labelled with the currency
//! of the layer that produced them, and it needs an honest "cannot price this"
//! when the user configured a rule that does not cover the call.
//!
//! That last case is why this entry point is not an `Option`. Spec 4.4 is
//! explicit that a configured-but-unavailable price must not be replaced by a
//! generic estimate (`$15/$60` in the TUI): the caller has to be able to tell
//! "nothing knows this model, keep your old fallback" apart from "the user's own
//! rule cannot price it, so show nothing".

use crate::config::CostFields;
use crate::model_pricing::entry::{ModelPricingEntry, RuleOutOfEffect};
use crate::model_pricing::sources::{self, ConfigPrice};
use jcode_provider_core::Currency;
use std::time::SystemTime;

/// Per-million-token rates exactly as the layer that produced them states them.
#[derive(Debug, Clone, PartialEq)]
pub struct CallRateCard {
    /// Fresh (uncached) input rate.
    pub input_per_mtok: f64,
    /// Output/completion rate.
    pub output_per_mtok: f64,
    /// Cache-read rate when the card states one.
    pub cache_read_per_mtok: Option<f64>,
    /// Cache-write rate when the *user's own* card states one, directly or
    /// through the tariff it selects.
    ///
    /// A `cache_write` that only arrived by merging the fallback layer is
    /// deliberately absent here, even though the merged rate card in `entry`
    /// does carry it: the cost site's cache-write premium (Anthropic's
    /// `input x 1.25/2.0`) owns that case, exactly as it did before `[pricing]`
    /// existed. Honouring a models.dev figure here would change what a cache
    /// write costs for a card that never mentioned one.
    pub cache_write_per_mtok: Option<f64>,
    /// Currency the rates above are denominated in. Never inherited across
    /// layers (F1).
    pub currency: Currency,
}

impl CallRateCard {
    /// The card for a resolved entry, or `None` when it cannot price a call.
    ///
    /// Input and output are both required: a call's cost is dominated by the
    /// output rate, so pricing without it would silently bill the missing half
    /// at whatever the caller's fallback is.
    ///
    /// `cache_write_per_mtok` is passed in rather than read off `entry`: only a
    /// rate the config layer states may replace the billing premium, and `entry`
    /// is the card *after* the field-level merge with the next layer. The caller
    /// reads the distinction from `sources::ResolvedCard::config_cache_write`.
    pub fn from_entry(
        entry: &ModelPricingEntry,
        cache_write_per_mtok: Option<f64>,
        currency: Currency,
    ) -> Option<Self> {
        let cost: &CostFields = &entry.cost;
        Some(Self {
            input_per_mtok: cost.input?,
            output_per_mtok: cost.output?,
            cache_read_per_mtok: cost.cache_read,
            cache_write_per_mtok,
            currency,
        })
    }
}

/// What the `[pricing.providers]` layer says about one `(provider, model)` pair
/// at one instant.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigCallRates {
    /// A hand-written card prices this call, with the tariff in effect at `at`
    /// already applied.
    Priced(CallRateCard),
    /// A hand-written card claims this pair but cannot price the call: it is
    /// incomplete in a currency that cannot merge with the next layer, or its
    /// rule expired with `on_rule_expiry = "no_price"`. Callers must show
    /// "unknown" rather than substitute an estimate (spec 4.4).
    ConfiguredWithoutPrice,
    /// A rule claims this pair but is out of effect with `on_rule_expiry =
    /// "fallback"` (spec F20): the *next* layer prices the call, and callers
    /// must label that price with [`RuleOutOfEffect::label`] so the user learns
    /// their own rule stopped applying (spec F8). This is deliberately not
    /// [`Self::ConfiguredWithoutPrice`]: here the call *is* priced, just not by
    /// the config layer.
    OutOfEffect(RuleOutOfEffect),
    /// No card claims this pair. Callers keep their pre-feature fallback.
    Absent,
}

/// The config-authoritative answer for one call.
///
/// `provider` is the activity source key the billing path uses (`claude:api-key`,
/// `openai-compatible:deepseek`, ...); `at` is the call's own instant, so the
/// peak/off-peak tariff this returns is the one in effect for that call (F15).
///
/// `input_tokens` is the call's reported input token count from its first usage
/// snapshot; it selects the call's long-context tier when the card declares one.
/// `None` means the caller has no count (a cheapness estimate, `/pricing`'s
/// reference value), and the call is then priced at the base tier: the rates
/// returned are the ones below the first `min_input_tokens`.
pub fn config_call_rates(
    provider: &str,
    model: &str,
    at: SystemTime,
    input_tokens: Option<u64>,
) -> ConfigCallRates {
    match sources::config_price(provider, model, at) {
        ConfigPrice::NoPrice => ConfigCallRates::ConfiguredWithoutPrice,
        ConfigPrice::OutOfEffect(reason) => ConfigCallRates::OutOfEffect(reason),
        ConfigPrice::Absent => ConfigCallRates::Absent,
        ConfigPrice::Hit { entry, currency } => {
            let resolved =
                sources::resolve_card(*entry, currency, provider, model, at, input_tokens);
            if !resolved.owns_price {
                // The card lost to the next layer (a foreign-currency card that
                // cannot be completed, per F1). That is not this layer's answer:
                // let the derived layers price the call and label it.
                return ConfigCallRates::Absent;
            }
            match CallRateCard::from_entry(
                &resolved.entry,
                resolved.config_cache_write,
                resolved.currency,
            ) {
                Some(card) => ConfigCallRates::Priced(card),
                None => ConfigCallRates::ConfiguredWithoutPrice,
            }
        }
    }
}

/// A problem with the user's own `[pricing]` configuration that changes what
/// the displayed figure *means*, in the form the display labels next to the
/// amount that figure belongs to.
///
/// Two shapes reach here, and both are shown on the amount rather than in a log
/// line because the resolver runs per call and the label is rendered every
/// frame:
///
/// * a rule that stopped applying (F8/F20), where the figure is a lower layer's,
///   and
/// * a card that claims the pair but cannot price the call (spec 4.4), where no
///   cost was accrued at all and the figure is deliberately left at zero rather
///   than replaced by an estimate.
///
/// Both of the user's own layers are covered, and the vendor file names itself:
/// a user with several `[pricing.providers.<vendor>].file` entries has to be
/// able to tell which of them stopped applying, so the marker carries the
/// vendor key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PricingNotice {
    /// A hand-written `[pricing.providers]` card. Label: `rule expired` /
    /// `rule not in effect yet`.
    ConfigCard(RuleOutOfEffect),
    /// A rule inside a `[pricing.providers.<vendor>].file`, named by its vendor.
    ///
    /// A vendor file rule is out of effect exactly like a card's `fallback`
    /// behaviour: the next layer prices the call (see `vendor_files`). The label
    /// is the same class as the card's, with the vendor appended.
    VendorFile {
        vendor: String,
        reason: RuleOutOfEffect,
    },
    /// A hand-written card claims this pair but cannot price the call (its
    /// `on_rule_expiry = "no_price"` rule expired, or the card is incomplete in
    /// a currency that cannot merge with the next layer). Nothing is billed, so
    /// the session figure stays at zero; the label is what stops that zero from
    /// reading as "this call was free".
    ConfiguredWithoutPrice,
}

impl PricingNotice {
    /// The short marker shown next to the price, in the same style as the
    /// `(no EUR rate)` note.
    pub fn label(&self) -> String {
        match self {
            Self::ConfigCard(reason) => reason.label().to_string(),
            Self::VendorFile { vendor, reason } => {
                format!("{} (pricing.providers `{vendor}` file)", reason.label())
            }
            Self::ConfiguredWithoutPrice => "rule cannot price this call".to_string(),
        }
    }
}

/// The vendor file rule that covers this call but is out of effect at `at`, if
/// the file layer is what sends the price below it.
///
/// This is the file's half of the F8/F20 marker. The call is still priced by the
/// next layer (that is the existing fall-through); the notice is what tells the
/// user their file stopped applying instead of a models.dev number taking over
/// silently.
pub fn vendor_file_rule_out_of_effect(
    provider: &str,
    model: &str,
    at: SystemTime,
) -> Option<PricingNotice> {
    let (vendor, reason) = super::vendor_file_out_of_effect(provider, model, at)?;
    Some(PricingNotice::VendorFile { vendor, reason })
}

/// The dollar cost of one call's reported usage, resolved at the call's own
/// instant with the same two-layer path the client's billing uses.
///
/// A remote client normally prices each call itself, from its own
/// configuration. That makes the displayed and accrued figure depend on the
/// *client's* cards/currency/vendor files/schedules, so the same call can be
/// billed two different ways by two clients (Greptile P1/P2). A server that
/// resolves the cost itself and reports it on the usage event removes that
/// ambiguity: every client shows and accrues the server's answer.
///
/// `provider` is the activity source key the billing path uses
/// (`claude:api-key`, `openai-compatible:deepseek`, ...); `model` is the model
/// id the request was issued with; `at` is the call's own start instant (F15),
/// so the peak/off-peak tariff is the one in effect for that call;
/// `service_tier` is the active tier (`/fast on`, OpenAI flex), which only the
/// derived layers honour.
///
/// The resolution order is exactly the client's: the hand-written
/// `[pricing.providers]` layer first (it may price the call, or refuse to), then
/// the derived layers (vendor file, curated tables, OpenRouter, models.dev) via
/// the same [`crate::provider::pricing::derived_pricing_for_source_at_size`] the
/// client calls. Returns `None` when no layer can price the call (a `no_price`
/// rule, an incomplete foreign-currency card, or a model unknown to every
/// source): the caller must then leave the call unpriced rather than substitute
/// an estimate (spec 4.4).
#[allow(clippy::too_many_arguments)]
pub fn call_cost(
    provider: &str,
    model: &str,
    at: SystemTime,
    service_tier: Option<&str>,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_creation_tokens: u64,
    is_anthropic: bool,
    is_openai: bool,
) -> Option<(f64, Currency)> {
    // Config layer first: a hand-written card states its rates (and, crucially,
    // its own cache-write rate) exactly, and can refuse to price the call.
    match config_call_rates(provider, model, at, Some(input_tokens)) {
        ConfigCallRates::Priced(card) => {
            let amount = usage_cost(
                card.input_per_mtok,
                card.output_per_mtok,
                card.cache_read_per_mtok,
                card.cache_write_per_mtok,
                is_anthropic,
                is_openai,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens,
            );
            return Some((amount, card.currency));
        }
        ConfigCallRates::ConfiguredWithoutPrice => return None,
        ConfigCallRates::OutOfEffect(_) | ConfigCallRates::Absent => {}
    }

    // Derived layers: the same entry point the client's `refresh_cached_pricing`
    // falls through to. Those layers state no cache-write rate, so cache writes
    // keep the premium heuristic below.
    let estimate = crate::provider::pricing::derived_pricing_for_source_at_size(
        provider,
        model,
        service_tier,
        at,
        Some(input_tokens),
    )?;
    let micros_to_rate = |micros: u64| micros as f64 / 1_000_000.0;
    let amount = usage_cost(
        micros_to_rate(estimate.input_price_per_mtok_micros?),
        micros_to_rate(estimate.output_price_per_mtok_micros?),
        estimate
            .cache_read_price_per_mtok_micros
            .map(micros_to_rate),
        None,
        is_anthropic,
        is_openai,
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_creation_tokens,
    );
    Some((amount, estimate.currency))
}

/// Turn per-million-token rates into one call's dollar cost, mirroring the
/// client's split-accounting math (Anthropic excludes cache counts from input;
/// OpenAI-style reports them as a subset of input).
#[allow(clippy::too_many_arguments)]
fn usage_cost(
    input_per_mtok: f64,
    output_per_mtok: f64,
    cache_read_per_mtok: Option<f64>,
    cache_write_per_mtok: Option<f64>,
    is_anthropic: bool,
    is_openai: bool,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_creation_tokens: u64,
) -> f64 {
    let split_accounting = is_anthropic
        || (!is_openai && (cache_creation_tokens > 0 || cache_read_tokens > input_tokens));

    let fresh_input_tokens = if split_accounting {
        input_tokens
    } else {
        input_tokens
            .saturating_sub(cache_read_tokens)
            .saturating_sub(cache_creation_tokens)
    };

    let prompt_cost = fresh_input_tokens as f64 * input_per_mtok / 1_000_000.0;
    let completion_cost = output_tokens as f64 * output_per_mtok / 1_000_000.0;
    let cache_read_cost = match cache_read_per_mtok {
        Some(price) => cache_read_tokens as f64 * price / 1_000_000.0,
        None => cache_read_tokens as f64 * input_per_mtok / 1_000_000.0,
    };
    let cache_write_cost = if cache_creation_tokens > 0 {
        let price = match cache_write_per_mtok {
            Some(price) => price,
            None => {
                let multiplier = if is_anthropic {
                    if crate::provider::anthropic::is_cache_ttl_1h() {
                        2.0
                    } else {
                        1.25
                    }
                } else if is_openai {
                    1.25
                } else {
                    1.0
                };
                input_per_mtok * multiplier
            }
        };
        cache_creation_tokens as f64 * price / 1_000_000.0
    } else {
        0.0
    };

    prompt_cost + completion_cost + cache_read_cost + cache_write_cost
}
