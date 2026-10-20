//! One model's price entry: the base rate card plus the extension fields the
//! old flat models.dev cache used to drop (`tariffs`, `schedule`, validity).
//!
//! The rule shape is not re-invented here: an entry reuses the validated types
//! from [`crate::config::pricing`] (`CostFields` / `Tariff` / `ScheduleRule` /
//! `OnRuleExpiry`), so a hand-written config rule and a catalog entry are the
//! same shape and can be merged field by field.
//!
//! Currency is deliberately **not** a field. Prices are plain numbers; the
//! layer that produced them states their currency (models.dev is always USD,
//! `[pricing.providers.<name>].currency` states it for config rules). Keeping
//! the number and its currency apart is what makes the "no cross-currency
//! inheritance" rule in the resolver enforceable.

use crate::config::{
    ContextTier, CostFields, ModelPricingRule, OnRuleExpiry, ScheduleRule, Tariff,
};
use crate::model_pricing::ModelCost;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;
use std::time::SystemTime;

/// A per-model rate card with room for the extension fields.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ModelPricingEntry {
    /// Base rates per million tokens, in the currency of the layer that
    /// produced them. Optional so a layer can fill in what an earlier layer
    /// left out (spec 4.4 field-level merge).
    pub cost: CostFields,
    /// Billing identities this entry is restricted to, trimmed and
    /// case-sensitive.
    ///
    /// Empty means "applies to every route", which is the behaviour before
    /// `route` existed and is byte-compatible with it. A non-empty list is
    /// matched by [`Self::route_applies`]; a route it does not name is skipped
    /// (the next layer prices the call), never mispriced by this entry.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub route: Vec<String>,
    /// Named rate cards: absolute prices or a multiplier on `cost`.
    ///
    /// Selected by `model_pricing::rules`, so cache entries, config rules, and
    /// extra sources all share one shape.
    pub tariffs: BTreeMap<String, Tariff>,
    /// Time windows selecting a tariff, first match wins. Windows are the
    /// half-open local interval `[start, end)` of the rule's fixed UTC offset;
    /// `model_pricing::rules` does the matching.
    pub schedule: Vec<ScheduleRule>,
    /// Long-context overlays, applied *after* the window tariff is selected and
    /// only when the call's reported input token count exceeds the threshold.
    ///
    /// Declaration order, first match wins. Both hand-written config rules and
    /// models.dev's native `context_over_200k` rates land here, so the two
    /// sources converge on one representation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_tiers: Vec<ContextTier>,
    pub default_tariff: Option<String>,
    /// Inclusive lower bound; a card is not in effect before it.
    pub effective_from: Option<DateTime<Utc>>,
    /// Exclusive upper bound: at this instant the card stops being in effect.
    pub effective_until: Option<DateTime<Utc>>,
    /// What to do once the card is out of effect.
    pub on_rule_expiry: OnRuleExpiry,
}

/// Why a rule's validity window does not cover an instant (spec F8/F20).
///
/// The two directions are different user situations — a promotion that ran out
/// versus a rule that has not opened yet — so the resolver reports which one
/// applies and the display can name it instead of a generic "not applicable".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleOutOfEffect {
    /// `effective_until` has passed.
    Expired,
    /// `effective_from` has not been reached yet.
    NotYetEffective,
}

impl RuleOutOfEffect {
    /// The short marker shown next to a price that a *lower* layer produced
    /// because this rule was out of effect, in the same style as the
    /// `(no EUR rate)` note. Kept here so the wording lives next to the reason
    /// it describes.
    pub fn label(self) -> &'static str {
        match self {
            Self::Expired => "rule expired",
            Self::NotYetEffective => "rule not in effect yet",
        }
    }
}

impl ModelPricingEntry {
    /// A bare models.dev entry: rates only, every extension field empty.
    pub fn from_model_cost(cost: ModelCost) -> Self {
        Self {
            cost: CostFields {
                input: Some(cost.input_usd_per_mtok),
                output: Some(cost.output_usd_per_mtok),
                cache_read: cost.cache_read_usd_per_mtok,
                cache_write: cost.cache_write_usd_per_mtok,
            },
            ..Self::default()
        }
    }

    /// A validated `[pricing.providers]` rule.
    pub fn from_rule(rule: &ModelPricingRule) -> Self {
        Self {
            cost: rule.cost.clone().unwrap_or_default(),
            route: rule.route.clone(),
            tariffs: rule.tariffs.clone(),
            schedule: rule.schedule.clone(),
            context_tiers: rule.context_tiers.clone(),
            default_tariff: rule.default_tariff.clone(),
            effective_from: rule.effective_from,
            effective_until: rule.effective_until,
            on_rule_expiry: rule.on_rule_expiry,
        }
    }

    /// Whether this entry applies to a call with the billing identity
    /// `source_key` (e.g. `openrouter`, `deepseek`, `openai-compatible:deepseek`).
    ///
    /// An empty [`Self::route`] applies everywhere, which is what keeps a
    /// route-less rule byte-compatible with the behaviour before the field
    /// existed. A non-empty list matches when it names the call's identity
    /// exactly, or when the call is a compatible profile
    /// (`openai-compatible:<x>`) and the list names its short form `<x>`: a
    /// profile may be addressed either way (`/pricing` prints the raw key as
    /// "looked up as ...").
    ///
    /// This is the *only* matching implementation; every scan site calls it so
    /// the layers cannot disagree about what a route is.
    pub fn route_applies(&self, source_key: &str) -> bool {
        route_matches(&self.route, source_key)
    }

    /// The complete rate card as the flat catalog type, if both base rates are
    /// known.
    ///
    /// `ModelCost`'s field names say USD because every value that reaches the
    /// catalog is a models.dev value; config cards are resolved live and never
    /// travel through this conversion.
    pub fn to_model_cost(&self) -> Option<ModelCost> {
        Some(ModelCost {
            input_usd_per_mtok: self.cost.input?,
            output_usd_per_mtok: self.cost.output?,
            cache_read_usd_per_mtok: self.cost.cache_read,
            cache_write_usd_per_mtok: self.cost.cache_write,
        })
    }

    /// Why this card is not in effect at `at`, or `None` when it is.
    ///
    /// Only the validity bounds are checked here: tariff/schedule selection
    /// (which tariff applies *within* the valid window) is
    /// `model_pricing::rules`' job.
    pub fn out_of_effect_reason(&self, at: SystemTime) -> Option<RuleOutOfEffect> {
        let at: DateTime<Utc> = at.into();
        if let Some(from) = self.effective_from
            && at < from
        {
            return Some(RuleOutOfEffect::NotYetEffective);
        }
        if let Some(until) = self.effective_until
            && at >= until
        {
            return Some(RuleOutOfEffect::Expired);
        }
        None
    }

    /// Whether this card's validity window covers `at`.
    pub fn is_active_at(&self, at: SystemTime) -> bool {
        self.out_of_effect_reason(at).is_none()
    }
}

/// Whether a `route` filter applies to a call with the billing identity
/// `source_key`.
///
/// The one matching rule shared by every scan site (inline cards, vendor files,
/// the estimate path), so no layer can disagree about what a route is. An empty
/// filter applies everywhere; a non-empty one matches the identity exactly, or a
/// compatible profile's short form (`deepseek` matches
/// `openai-compatible:deepseek`).
pub(crate) fn route_matches(route: &[String], source_key: &str) -> bool {
    if route.is_empty() {
        return true;
    }
    if route.iter().any(|entry| entry == source_key) {
        return true;
    }
    match source_key.strip_prefix("openai-compatible:") {
        Some(short) => route.iter().any(|entry| entry == short),
        None => false,
    }
}

impl<'de> Deserialize<'de> for ModelPricingEntry {
    /// Accept both cache shapes:
    ///
    /// * v1 (previous binary): a bare [`ModelCost`] object, e.g.
    ///   `{"input_usd_per_mtok": 0.66, ...}`.
    /// * v2 (current): `{"cost": {...}, "tariffs": ..., ...}`.
    ///
    /// Order matters for `untagged`: the v2 shape is tried first because its
    /// `cost` key is required, so a v1 object cannot accidentally match it.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct FullEntry {
            cost: CostFields,
            #[serde(default)]
            route: Vec<String>,
            #[serde(default)]
            tariffs: BTreeMap<String, Tariff>,
            #[serde(default)]
            schedule: Vec<ScheduleRule>,
            #[serde(default)]
            context_tiers: Vec<ContextTier>,
            #[serde(default)]
            default_tariff: Option<String>,
            #[serde(default)]
            effective_from: Option<DateTime<Utc>>,
            #[serde(default)]
            effective_until: Option<DateTime<Utc>>,
            #[serde(default)]
            on_rule_expiry: OnRuleExpiry,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            V2(FullEntry),
            V1(ModelCost),
        }

        match Repr::deserialize(deserializer)? {
            Repr::V2(full) => Ok(Self {
                cost: full.cost,
                route: full.route,
                tariffs: full.tariffs,
                schedule: full.schedule,
                context_tiers: full.context_tiers,
                default_tariff: full.default_tariff,
                effective_from: full.effective_from,
                effective_until: full.effective_until,
                on_rule_expiry: full.on_rule_expiry,
            }),
            Repr::V1(cost) => Ok(Self::from_model_cost(cost)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use jcode_config_types::OnRuleExpiry;

    fn deepseek_cost() -> ModelCost {
        ModelCost {
            input_usd_per_mtok: 0.66,
            output_usd_per_mtok: 1.98,
            cache_read_usd_per_mtok: Some(0.022),
            cache_write_usd_per_mtok: None,
        }
    }

    #[test]
    fn bare_model_cost_has_no_extensions() {
        let entry = ModelPricingEntry::from_model_cost(deepseek_cost());
        assert_eq!(entry.cost.input, Some(0.66));
        assert_eq!(entry.cost.output, Some(1.98));
        assert_eq!(entry.cost.cache_read, Some(0.022));
        assert!(entry.tariffs.is_empty());
        assert!(entry.schedule.is_empty());
        assert_eq!(entry.default_tariff, None);
        assert_eq!(entry.on_rule_expiry, OnRuleExpiry::Fallback);
        assert!(entry.is_active_at(SystemTime::now()));
        assert_eq!(entry.to_model_cost(), Some(deepseek_cost()));
    }

    #[test]
    fn v1_shape_deserializes_as_a_bare_cost() {
        let entry: ModelPricingEntry =
            serde_json::from_str(r#"{"input_usd_per_mtok": 0.66, "output_usd_per_mtok": 1.98}"#)
                .expect("v1 entry");
        assert_eq!(entry.cost.input, Some(0.66));
        assert_eq!(entry.cost.output, Some(1.98));
        assert_eq!(entry.cost.cache_read, None);
    }

    #[test]
    fn v2_shape_round_trips() {
        let mut entry = ModelPricingEntry::from_model_cost(deepseek_cost());
        entry.default_tariff = Some("off_peak".to_string());
        entry
            .tariffs
            .insert("peak".to_string(), Tariff::Multiplier(2.0));
        entry.context_tiers.push(crate::config::ContextTier {
            min_input_tokens: 200_000,
            tariff: Tariff::Multiplier(2.0),
        });
        entry.context_tiers.push(crate::config::ContextTier {
            min_input_tokens: 500_000,
            tariff: Tariff::Absolute(CostFields {
                input: Some(9.0),
                output: Some(27.0),
                cache_read: None,
                cache_write: None,
            }),
        });
        let json = serde_json::to_string(&entry).expect("serialize");
        let back: ModelPricingEntry = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, entry);
        assert_eq!(back.context_tiers.len(), 2);
    }

    /// An entry with no tiers must not write an empty `context_tiers` key: the
    /// cache is read by older binaries too, and a default-valued field is noise.
    #[test]
    fn an_entry_without_tiers_does_not_serialize_the_key() {
        let entry = ModelPricingEntry::from_model_cost(deepseek_cost());
        let json = serde_json::to_string(&entry).expect("serialize");
        assert!(!json.contains("context_tiers"), "{json}");
    }

    #[test]
    fn validity_bounds_are_inclusive_and_exclusive() {
        let mut entry = ModelPricingEntry::from_model_cost(deepseek_cost());
        entry.effective_from = Some(
            Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                .single()
                .expect("valid timestamp"),
        );
        entry.effective_until = Some(
            Utc.with_ymd_and_hms(2026, 12, 31, 23, 59, 59)
                .single()
                .expect("valid timestamp"),
        );
        let before = SystemTime::from(
            Utc.with_ymd_and_hms(2025, 12, 31, 23, 59, 59)
                .single()
                .expect("valid timestamp"),
        );
        let at_start = SystemTime::from(
            Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                .single()
                .expect("valid timestamp"),
        );
        let at_end = SystemTime::from(
            Utc.with_ymd_and_hms(2026, 12, 31, 23, 59, 59)
                .single()
                .expect("valid timestamp"),
        );
        assert!(!entry.is_active_at(before), "not in effect before `from`");
        assert!(entry.is_active_at(at_start), "`from` is inclusive");
        assert!(!entry.is_active_at(at_end), "`until` is exclusive");

        // F8/F20: the display has to name *which* way the rule is out of
        // effect, so the reason is not collapsed into "not active".
        assert_eq!(
            entry.out_of_effect_reason(before),
            Some(RuleOutOfEffect::NotYetEffective)
        );
        assert_eq!(entry.out_of_effect_reason(at_start), None);
        assert_eq!(
            entry.out_of_effect_reason(at_end),
            Some(RuleOutOfEffect::Expired)
        );
        assert_eq!(
            RuleOutOfEffect::Expired.label(),
            "rule expired",
            "the marker the widget shows"
        );
    }
}
