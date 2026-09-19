//! `[pricing]` section of the config: hand-written per-provider rate rules.
//!
//! This is the raw serde shape only. Every field is optional and
//! `#[serde(default)]`, so an absent `[pricing]` section behaves exactly like
//! today. Validation (currency normalization, time-window parsing, field-path
//! error messages) and conversion to runtime types live in
//! `jcode-base::config::pricing`, because a `*-types` crate must not depend on
//! `jcode-provider-core` where `Currency` lives.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What to do when a model's `effective_until` has passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnRuleExpiry {
    /// Fall back to the next layer and label the rule as expired (default).
    #[default]
    Fallback,
    /// Refuse to price at all rather than fall back to a worse estimate.
    NoPrice,
}

/// A named rate card: either absolute prices or a multiplier on the base cost.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TariffFile {
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
    /// Multiplier applied to the model's base `cost` (e.g. `2.0` for peak).
    pub multiplier: Option<f64>,
}

/// The base (off-peak / default) price of a model, in the provider currency.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CostFile {
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
}

/// One `schedule` entry: "when these windows hit, use this tariff".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScheduleRuleFile {
    pub tariff: String,
    /// Fixed UTC offset in minutes (`0` for DeepSeek). v1 has no IANA tz.
    pub utc_offset_minutes: i32,
    /// `["Mon", "Tue", ...]`; empty means every day.
    pub weekdays: Vec<String>,
    /// `[["01:00", "04:00"], ...]` in local (offset-shifted) time.
    pub windows: Vec<(String, String)>,
}

/// One `context_tiers` entry: "above this many input tokens, use these rates".
///
/// Reuses the `TariffFile` vocabulary (`multiplier` or absolute rates) rather
/// than inventing a second one, so an absolute tier and a named tariff mean the
/// same thing. Written as an array of tables (`[[...context_tiers]]`) for the
/// same reason `schedule` is: a multi-line inline table is invalid TOML.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextTierFile {
    /// The tier applies when a call's first usage snapshot reports **more** than
    /// this many input tokens. Exactly this many does not trigger it.
    pub min_input_tokens: Option<u64>,
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
    /// Multiplier applied to the rate card the schedule already selected.
    pub multiplier: Option<f64>,
}

/// Rate rules for a single model.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelPricingRuleFile {
    pub cost: Option<CostFile>,
    pub tariffs: BTreeMap<String, TariffFile>,
    pub schedule: Vec<ScheduleRuleFile>,
    /// Long-context overlays, matched in declaration order, first match wins.
    ///
    /// Never written back when empty: a default-valued field must not be baked
    /// into the user's file on a save (the lesson of commit `9da6f9831`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub context_tiers: Vec<ContextTierFile>,
    pub default_tariff: Option<String>,
    pub effective_from: Option<String>,
    pub effective_until: Option<String>,
    #[serde(
        default,
        deserialize_with = "crate::serde_lenient::lenient_optional_enum"
    )]
    pub on_rule_expiry: Option<OnRuleExpiry>,
}

/// Rate rules for a single provider ("vendor").
///
/// A vendor is the user's own label, not a route identity: its `models` rules
/// (and the model rules inside `file`) apply by **model id regardless of which
/// route a call uses**. That is deliberate - a user who runs
/// `provider = OpenRouter, model = deepseek-flash` still wants DeepSeek's
/// official CNY prices, and binding rules to a route would silently fall them
/// back to models.dev's USD numbers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderPricingFile {
    /// Optional local JSON file holding this vendor's model rules. A bare name
    /// (`deepseek.json`) resolves under `~/.jcode/cache/`; anything else
    /// (absolute, or relative with a separator, with a leading `~` expanded) is
    /// used as the path it is. The file holds only
    /// `{"models": {"<model-id>": {cost|tariffs|schedule|...}}}`, with no outer
    /// vendor key: the vendor is already the config key this file hangs under.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Currency the numbers in this vendor's rules are denominated in; defaults
    /// to USD when absent.
    pub currency: Option<String>,
    pub models: BTreeMap<String, ModelPricingRuleFile>,
}

/// `[pricing]`: hand-written rate rules, which outrank every other source.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PricingConfigFile {
    /// Base currency for `fx_rates` (defaults to USD).
    pub fx_base: Option<String>,
    /// `1 fx_base = N <code>`. v1 has no automatic fetch; hand-written wins.
    ///
    /// Skipped when empty (the lesson of `9da6f9831`): a default-valued field
    /// must not be baked into the user's file on a save.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub fx_rates: BTreeMap<String, f64>,
    /// Skipped when empty for the same reason: `Config::save()` serializes the
    /// whole struct, so without this an unconfigured `[pricing]` that only sets
    /// `fx_base` would still grow an empty `providers` table.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub providers: BTreeMap<String, ProviderPricingFile>,
}

impl PricingConfigFile {
    /// Whether the user configured any rate rules at all.
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty() && self.fx_rates.is_empty() && self.fx_base.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_section_parses_to_empty_default() {
        let parsed: PricingConfigFile = serde_json::from_str("{}").expect("parse");
        assert!(parsed.is_empty());
        assert!(parsed.providers.is_empty());
    }

    #[test]
    fn a_vendor_file_and_inline_models_parse_and_roundtrip() {
        let json = r#"{
            "providers": {
                "deepseek": {
                    "file": "deepseek.json",
                    "currency": "CNY",
                    "models": {
                        "deepseek-v4-pro": {"cost": {"input": 2.0, "output": 8.0}}
                    }
                },
                "inline-only": {"currency": "USD"}
            }
        }"#;
        let parsed: PricingConfigFile = serde_json::from_str(json).expect("parse");
        assert!(!parsed.is_empty());
        let deepseek = parsed.providers.get("deepseek").expect("deepseek");
        assert_eq!(deepseek.file.as_deref(), Some("deepseek.json"));
        assert_eq!(deepseek.currency.as_deref(), Some("CNY"));
        assert_eq!(deepseek.models.len(), 1);
        // Optional fields default rather than fail: a vendor with only inline
        // models, or only a file, is legal.
        let inline = parsed.providers.get("inline-only").expect("inline-only");
        assert_eq!(inline.file, None);
        assert!(inline.models.is_empty());

        let again = serde_json::to_string(&parsed).expect("serialize");
        assert!(again.contains("deepseek.json"));
        let reparsed: PricingConfigFile = serde_json::from_str(&again).expect("reparse");
        assert_eq!(reparsed.providers, parsed.providers);
    }

    /// The lesson of commit `9da6f9831`: a default-valued field must not be
    /// baked into the user's file the next time anything saves.
    #[test]
    fn a_vendor_without_a_file_does_not_serialize_the_key() {
        let parsed: PricingConfigFile =
            serde_json::from_str(r#"{"providers":{"deepseek":{"currency":"CNY"}}}"#)
                .expect("parse");
        let again = serde_json::to_string(&parsed).expect("serialize");
        assert!(!again.contains("\"file\""), "{again}");
    }

    #[test]
    fn an_empty_pricing_section_does_not_serialize_a_providers_key() {
        let parsed = PricingConfigFile::default();
        let json = serde_json::to_string(&parsed).expect("serialize");
        assert!(!json.contains("providers"), "{json}");
    }

    #[test]
    fn full_shape_roundtrips_through_json() {
        let json = r#"{
            "fx_base": "USD",
            "fx_rates": {"CNY": 7.2, "EUR": 0.92},
            "providers": {
                "deepseek": {
                    "currency": "CNY",
                    "models": {
                        "deepseek-v4-pro": {
                            "cost": {"input": 4.5, "output": 13.5, "cache_read": 0.15},
                            "tariffs": {"peak": {"multiplier": 2.0}},
                            "schedule": [{
                                "tariff": "peak",
                                "utc_offset_minutes": 0,
                                "weekdays": ["Mon", "Tue"],
                                "windows": [["01:00", "04:00"], ["06:00", "10:00"]]
                            }],
                            "context_tiers": [
                                {"min_input_tokens": 200000, "multiplier": 2.0},
                                {"min_input_tokens": 500000, "input": 9.0, "output": 27.0}
                            ],
                            "default_tariff": "off_peak",
                            "on_rule_expiry": "no_price"
                        }
                    }
                }
            }
        }"#;
        let parsed: PricingConfigFile = serde_json::from_str(json).expect("parse");
        assert!(!parsed.is_empty());
        let provider = parsed.providers.get("deepseek").expect("deepseek");
        assert_eq!(provider.currency.as_deref(), Some("CNY"));
        let rule = provider.models.get("deepseek-v4-pro").expect("model");
        assert_eq!(rule.schedule.len(), 1);
        assert_eq!(rule.schedule[0].utc_offset_minutes, 0);
        assert_eq!(rule.schedule[0].windows[0].0, "01:00");
        assert_eq!(rule.on_rule_expiry, Some(OnRuleExpiry::NoPrice));
        assert_eq!(rule.context_tiers.len(), 2);
        assert_eq!(rule.context_tiers[0].min_input_tokens, Some(200_000));
        assert_eq!(rule.context_tiers[0].multiplier, Some(2.0));
        assert_eq!(rule.context_tiers[1].input, Some(9.0));

        let again = serde_json::to_string(&parsed).expect("serialize");
        let reparsed: PricingConfigFile = serde_json::from_str(&again).expect("reparse");
        assert_eq!(reparsed.providers, parsed.providers);
    }

    #[test]
    fn unknown_on_rule_expiry_is_lenient_and_falls_back_to_none() {
        let json = r#"{"providers":{"p":{"models":{"m":{"on_rule_expiry":"wat"}}}}}"#;
        let parsed: PricingConfigFile = serde_json::from_str(json).expect("parse");
        let rule = parsed
            .providers
            .get("p")
            .and_then(|p| p.models.get("m"))
            .expect("model");
        assert_eq!(rule.on_rule_expiry, None);
        assert_eq!(OnRuleExpiry::default(), OnRuleExpiry::Fallback);
    }

    #[test]
    fn partial_cost_is_allowed_at_parse_time() {
        // Validation (e.g. "input is required when cost is present") belongs to
        // jcode-base; the DTO layer must not reject shapes.
        let json = r#"{"providers":{"p":{"models":{"m":{"cost":{"output":1.0}}}}}}"#;
        let parsed: PricingConfigFile = serde_json::from_str(json).expect("parse");
        let cost = parsed.providers["p"].models["m"]
            .cost
            .clone()
            .expect("cost");
        assert_eq!(cost.input, None);
        assert_eq!(cost.output, Some(1.0));
    }
}
