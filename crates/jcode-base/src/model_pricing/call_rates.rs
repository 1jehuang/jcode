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
use crate::model_pricing::entry::ModelPricingEntry;
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
    /// Cache-write rate when the card states one.
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
    pub fn from_entry(entry: &ModelPricingEntry, currency: Currency) -> Option<Self> {
        let cost: &CostFields = &entry.cost;
        Some(Self {
            input_per_mtok: cost.input?,
            output_per_mtok: cost.output?,
            cache_read_per_mtok: cost.cache_read,
            cache_write_per_mtok: cost.cache_write,
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
    /// No card claims this pair. Callers keep their pre-feature fallback.
    Absent,
}

/// The config-authoritative answer for one call.
///
/// `provider` is the activity source key the billing path uses (`claude:api-key`,
/// `openai-compatible:deepseek`, ...); `at` is the call's own instant, so the
/// peak/off-peak tariff this returns is the one in effect for that call (F15).
pub fn config_call_rates(provider: &str, model: &str, at: SystemTime) -> ConfigCallRates {
    match sources::config_price(provider, model, at) {
        ConfigPrice::NoPrice => ConfigCallRates::ConfiguredWithoutPrice,
        ConfigPrice::Absent => ConfigCallRates::Absent,
        ConfigPrice::Hit { entry, currency } => {
            let resolved = sources::resolve_card(*entry, currency, provider, model, at);
            if !resolved.from_config {
                // The card lost to the next layer (a foreign-currency card that
                // cannot be completed, per F1). That is not this layer's answer:
                // let the derived layers price the call and label it.
                return ConfigCallRates::Absent;
            }
            match CallRateCard::from_entry(&resolved.entry, resolved.currency) {
                Some(card) => ConfigCallRates::Priced(card),
                None => ConfigCallRates::ConfiguredWithoutPrice,
            }
        }
    }
}
