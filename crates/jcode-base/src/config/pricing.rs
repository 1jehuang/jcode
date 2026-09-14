//! Validation and conversion for the `[pricing]` config section.
//!
//! The serde shape lives in `jcode-config-types::pricing` (a `*-types` crate
//! that must not depend on `jcode-provider-core`, where `Currency` lives).
//! Everything that needs real types — currency normalization, time-window
//! parsing, cross-field checks — happens here, and every error carries the
//! config path that produced it.

use chrono::{DateTime, NaiveTime, Utc, Weekday};
use jcode_config_types::{
    CostFile, ModelPricingRuleFile, PricingConfigFile, ScheduleRuleFile, TariffFile,
};
use jcode_provider_core::Currency;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

pub use jcode_config_types::OnRuleExpiry;

/// A validation failure, carrying the config path that caused it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PricingConfigError {
    pub field_path: String,
    pub message: String,
}

impl PricingConfigError {
    fn new(field_path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field_path: field_path.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for PricingConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field_path, self.message)
    }
}

impl std::error::Error for PricingConfigError {}

/// Price fields for one rate card. All optional so layers can merge per field.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CostFields {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
}

/// A named rate card: absolute prices, or a multiplier on the base cost.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tariff {
    Absolute(CostFields),
    Multiplier(f64),
}

/// One window in local (offset-shifted) time. `[start, end)`; `start > end`
/// means the window wraps past midnight and belongs to the start day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeWindow {
    pub start: NaiveTime,
    pub end: NaiveTime,
}

impl TimeWindow {
    /// Whether the window wraps past midnight.
    pub fn wraps_midnight(&self) -> bool {
        self.start > self.end
    }
}

/// A validated `schedule` entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScheduleRule {
    pub tariff: String,
    pub utc_offset_minutes: i32,
    pub weekdays: Vec<Weekday>,
    pub windows: Vec<TimeWindow>,
}

/// A validated per-model rate rule.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelPricingRule {
    pub cost: Option<CostFields>,
    pub tariffs: BTreeMap<String, Tariff>,
    pub schedule: Vec<ScheduleRule>,
    pub default_tariff: Option<String>,
    pub effective_from: Option<DateTime<Utc>>,
    pub effective_until: Option<DateTime<Utc>>,
    pub on_rule_expiry: OnRuleExpiry,
}

/// A validated per-provider rate rule set.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProviderPricing {
    pub currency: Option<Currency>,
    pub models: BTreeMap<String, ModelPricingRule>,
}

/// The validated runtime view of `[pricing]`.
#[derive(Debug, Clone, PartialEq)]
pub struct PricingConfig {
    pub fx_base: Currency,
    pub fx_rates: BTreeMap<Currency, f64>,
    pub providers: BTreeMap<String, ProviderPricing>,
}

impl Default for PricingConfig {
    fn default() -> Self {
        Self {
            fx_base: Currency::usd(),
            fx_rates: BTreeMap::new(),
            providers: BTreeMap::new(),
        }
    }
}

impl PricingConfig {
    /// Whether the user configured anything at all. When empty, the pricing
    /// path must behave exactly as it did before this feature existed.
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty() && self.fx_rates.is_empty()
    }
}

/// Convert the raw `[pricing]` section into validated runtime types.
///
/// Returns the config plus non-fatal warnings (e.g. an unrecognized currency
/// code, which is accepted by design).
pub fn validate(
    file: &PricingConfigFile,
) -> Result<(PricingConfig, Vec<String>), PricingConfigError> {
    let mut warnings = Vec::new();

    let fx_base = parse_currency(
        file.fx_base.as_deref().unwrap_or("USD"),
        "pricing.fx_base",
        &mut warnings,
    );

    let mut fx_rates = BTreeMap::new();
    for (code, rate) in &file.fx_rates {
        let path = format!("pricing.fx_rates.{code}");
        let currency = parse_currency(code, &path, &mut warnings);
        if !rate.is_finite() || *rate <= 0.0 {
            return Err(PricingConfigError::new(
                path,
                format!("fx rate must be a positive, finite number, got {rate}"),
            ));
        }
        fx_rates.insert(currency, *rate);
    }

    let mut providers = BTreeMap::new();
    for (name, provider) in &file.providers {
        let base = format!("pricing.providers.{name}");
        let currency = provider
            .currency
            .as_deref()
            .map(|code| parse_currency(code, &format!("{base}.currency"), &mut warnings));
        let mut models = BTreeMap::new();
        for (model, rule) in &provider.models {
            let path = format!("{base}.models.{model}");
            models.insert(model.clone(), convert_rule(rule, &path)?);
        }
        providers.insert(name.clone(), ProviderPricing { currency, models });
    }

    let config = PricingConfig {
        fx_base,
        fx_rates,
        providers,
    };
    Ok((config, warnings))
}

/// Normalize a currency code, warning (not failing) on unknown codes.
fn parse_currency(code: &str, path: &str, warnings: &mut Vec<String>) -> Currency {
    let currency = Currency::new(code);
    if !currency.is_known_iso() {
        warnings.push(format!(
            "{path}: unrecognized currency code `{}`; accepted as-is",
            currency
        ));
    }
    currency
}

fn convert_rule(
    rule: &ModelPricingRuleFile,
    path: &str,
) -> Result<ModelPricingRule, PricingConfigError> {
    let cost = match &rule.cost {
        Some(cost) => Some(convert_cost(cost, &format!("{path}.cost"))?),
        None => None,
    };

    let mut tariffs = BTreeMap::new();
    for (name, tariff) in &rule.tariffs {
        let tariff_path = format!("{path}.tariffs.{name}");
        tariffs.insert(name.clone(), convert_tariff(tariff, &tariff_path)?);
    }

    let mut schedule = Vec::new();
    for (index, entry) in rule.schedule.iter().enumerate() {
        let entry_path = format!("{path}.schedule[{index}]");
        schedule.push(convert_schedule(entry, &entry_path, &tariffs)?);
    }

    if let Some(default) = &rule.default_tariff
        && !tariffs.contains_key(default)
    {
        return Err(PricingConfigError::new(
            format!("{path}.default_tariff"),
            format!("references unknown tariff `{default}`"),
        ));
    }

    let effective_from = parse_timestamp(
        rule.effective_from.as_deref(),
        &format!("{path}.effective_from"),
    )?;
    let effective_until = parse_timestamp(
        rule.effective_until.as_deref(),
        &format!("{path}.effective_until"),
    )?;

    Ok(ModelPricingRule {
        cost,
        tariffs,
        schedule,
        default_tariff: rule.default_tariff.clone(),
        effective_from,
        effective_until,
        on_rule_expiry: rule.on_rule_expiry.unwrap_or_default(),
    })
}

fn convert_cost(cost: &CostFile, path: &str) -> Result<CostFields, PricingConfigError> {
    if cost.input.is_none()
        && cost.output.is_none()
        && cost.cache_read.is_none()
        && cost.cache_write.is_none()
    {
        return Err(PricingConfigError::new(
            path,
            "a `cost` table needs at least one of input/output/cache_read/cache_write",
        ));
    }
    Ok(CostFields {
        input: cost.input,
        output: cost.output,
        cache_read: cost.cache_read,
        cache_write: cost.cache_write,
    })
}

fn convert_tariff(tariff: &TariffFile, path: &str) -> Result<Tariff, PricingConfigError> {
    let has_absolute = tariff.input.is_some()
        || tariff.output.is_some()
        || tariff.cache_read.is_some()
        || tariff.cache_write.is_some();
    match (tariff.multiplier, has_absolute) {
        (Some(multiplier), false) => {
            if !multiplier.is_finite() || multiplier <= 0.0 {
                return Err(PricingConfigError::new(
                    path,
                    format!("multiplier must be positive and finite, got {multiplier}"),
                ));
            }
            Ok(Tariff::Multiplier(multiplier))
        }
        (Some(_), true) => Err(PricingConfigError::new(
            path,
            "a tariff is either a `multiplier` or absolute prices, not both",
        )),
        (None, true) => Ok(Tariff::Absolute(CostFields {
            input: tariff.input,
            output: tariff.output,
            cache_read: tariff.cache_read,
            cache_write: tariff.cache_write,
        })),
        (None, false) => Err(PricingConfigError::new(
            path,
            "a tariff needs either a `multiplier` or at least one absolute price",
        )),
    }
}

fn convert_schedule(
    entry: &ScheduleRuleFile,
    path: &str,
    tariffs: &BTreeMap<String, Tariff>,
) -> Result<ScheduleRule, PricingConfigError> {
    if entry.tariff.trim().is_empty() {
        return Err(PricingConfigError::new(
            format!("{path}.tariff"),
            "a schedule entry must name a tariff",
        ));
    }
    if !tariffs.contains_key(&entry.tariff) {
        return Err(PricingConfigError::new(
            format!("{path}.tariff"),
            format!("references unknown tariff `{}`", entry.tariff),
        ));
    }
    if entry.utc_offset_minutes.abs() > 24 * 60 {
        return Err(PricingConfigError::new(
            format!("{path}.utc_offset_minutes"),
            format!(
                "UTC offset must be within ±1440 minutes, got {}",
                entry.utc_offset_minutes
            ),
        ));
    }

    let mut weekdays = Vec::with_capacity(entry.weekdays.len());
    for (index, day) in entry.weekdays.iter().enumerate() {
        let day_path = format!("{path}.weekdays[{index}]");
        weekdays.push(parse_weekday(day, &day_path)?);
    }

    let mut windows = Vec::with_capacity(entry.windows.len());
    for (index, (start, end)) in entry.windows.iter().enumerate() {
        let window_path = format!("{path}.windows[{index}]");
        let start = parse_time(start, &format!("{window_path}.start"))?;
        let end = parse_time(end, &format!("{window_path}.end"))?;
        if start == end {
            return Err(PricingConfigError::new(
                window_path,
                "window start equals end; use `start > end` for a window that wraps past midnight",
            ));
        }
        windows.push(TimeWindow { start, end });
    }

    Ok(ScheduleRule {
        tariff: entry.tariff.clone(),
        utc_offset_minutes: entry.utc_offset_minutes,
        weekdays,
        windows,
    })
}

fn parse_time(raw: &str, path: &str) -> Result<NaiveTime, PricingConfigError> {
    let trimmed = raw.trim();
    NaiveTime::parse_from_str(trimmed, "%H:%M")
        .or_else(|_| NaiveTime::parse_from_str(trimmed, "%H:%M:%S"))
        .map_err(|_| {
            PricingConfigError::new(
                path,
                format!("expected `HH:MM` (or `HH:MM:SS`), got `{raw}`"),
            )
        })
}

fn parse_weekday(raw: &str, path: &str) -> Result<Weekday, PricingConfigError> {
    let normalized = raw.trim().to_ascii_lowercase();
    let weekday = match normalized.as_str() {
        "mon" | "monday" => Weekday::Mon,
        "tue" | "tues" | "tuesday" => Weekday::Tue,
        "wed" | "weds" | "wednesday" => Weekday::Wed,
        "thu" | "thur" | "thurs" | "thursday" => Weekday::Thu,
        "fri" | "friday" => Weekday::Fri,
        "sat" | "saturday" => Weekday::Sat,
        "sun" | "sunday" => Weekday::Sun,
        _ => {
            return Err(PricingConfigError::new(
                path,
                format!(
                    "unknown weekday `{raw}`; expected one of Mon/Tue/Wed/Thu/Fri/Sat/Sun \
                     (full names accepted)"
                ),
            ));
        }
    };
    Ok(weekday)
}

fn parse_timestamp(
    raw: Option<&str>,
    path: &str,
) -> Result<Option<DateTime<Utc>>, PricingConfigError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    DateTime::parse_from_rfc3339(raw.trim())
        .map(|parsed| Some(parsed.with_timezone(&Utc)))
        .map_err(|_| {
            PricingConfigError::new(path, format!("expected an RFC 3339 timestamp, got `{raw}`"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_toml(section: &str) -> PricingConfigFile {
        toml::from_str(section).expect("toml parses into the DTO")
    }

    #[test]
    fn empty_config_defaults() {
        let (config, warnings) = validate(&parse_toml("")).expect("validates");
        assert!(config.is_empty());
        assert!(warnings.is_empty(), "no warnings for an absent section");
        assert!(config.fx_base.is_usd());
    }

    #[test]
    fn currency_normalizes_case() {
        let file = parse_toml(
            r#"
            [providers.deepseek]
            currency = "cny"
            "#,
        );
        let (config, _) = validate(&file).expect("validates");
        let provider = config.providers.get("deepseek").expect("provider");
        assert_eq!(
            provider.currency.as_ref().map(Currency::as_str),
            Some("CNY")
        );
    }

    #[test]
    fn unknown_currency_accepted_with_warning() {
        let file = parse_toml(
            r#"
            [providers.custom]
            currency = "xyz"
            "#,
        );
        let (config, warnings) = validate(&file).expect("unknown codes are accepted");
        assert_eq!(
            config
                .providers
                .get("custom")
                .and_then(|p| p.currency.as_ref())
                .map(Currency::as_str),
            Some("XYZ")
        );
        assert_eq!(warnings.len(), 1, "exactly one warning: {warnings:?}");
        assert!(
            warnings[0].contains("xyz") || warnings[0].contains("XYZ"),
            "warning should name the code: {}",
            warnings[0]
        );
    }

    #[test]
    fn fx_rates_parses_mixed_currencies() {
        let file = parse_toml(
            r#"
            fx_base = "usd"
            [fx_rates]
            CNY = 7.2
            EUR = 0.92
            JPY = 150.0
            "#,
        );
        let (config, warnings) = validate(&file).expect("validates");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(config.fx_base.is_usd());
        assert_eq!(config.fx_rates.len(), 3);
        let cny = config
            .fx_rates
            .get(&Currency::new("CNY"))
            .copied()
            .expect("CNY rate");
        assert!((cny - 7.2).abs() < 1e-12);
    }

    #[test]
    fn non_positive_fx_rate_is_rejected() {
        let file = parse_toml(
            r#"
            [fx_rates]
            CNY = 0.0
            "#,
        );
        let err = validate(&file).expect_err("zero rate is invalid");
        assert!(
            err.field_path.contains("fx_rates") && err.field_path.contains("CNY"),
            "path should locate the rate: {err}"
        );
    }

    #[test]
    fn partial_cost_is_allowed_for_field_level_merge() {
        // Spec 5.3: "only input written, output falls back to the next layer".
        let file = parse_toml(
            r#"
            [providers.p.models.m.cost]
            input = 1.0
            "#,
        );
        let (config, _) = validate(&file).expect("partial cost is legal");
        let cost = config.providers["p"].models["m"]
            .cost
            .clone()
            .expect("cost present");
        assert_eq!(cost.input, Some(1.0));
        assert_eq!(cost.output, None);
    }

    #[test]
    fn empty_cost_table_is_rejected() {
        let file = parse_toml(
            r#"
            [providers.p.models.m.cost]
            "#,
        );
        let err = validate(&file).expect_err("a cost table with no fields is meaningless");
        assert!(
            err.field_path.contains("providers.p.models.m.cost"),
            "path: {err}"
        );
    }

    #[test]
    fn tariff_needs_multiplier_or_absolute_but_not_both() {
        let both = parse_toml(
            r#"
            [providers.p.models.m.tariffs.t]
            multiplier = 2.0
            input = 1.0
            "#,
        );
        assert!(
            validate(&both).is_err(),
            "multiplier + absolute is ambiguous"
        );

        let neither = parse_toml(
            r#"
            [providers.p.models.m.tariffs.t]
            input = 1.0
            output = 2.0
            "#,
        );
        assert!(validate(&neither).is_ok(), "absolute prices are fine");

        let empty = parse_toml(
            r#"
            [providers.p.models.m.tariffs.t]
            "#,
        );
        assert!(validate(&empty).is_err(), "an empty tariff is meaningless");
    }

    #[test]
    fn schedule_referencing_unknown_tariff_is_rejected() {
        let file = parse_toml(
            r#"
            [[providers.p.models.m.schedule]]
            tariff = "peak"
            windows = [["01:00", "04:00"]]
            "#,
        );
        let err = validate(&file).expect_err("tariff must exist");
        assert!(err.field_path.contains("schedule"), "path: {err}");
    }

    #[test]
    fn window_start_equals_end_rejected() {
        let file = parse_toml(
            r#"
            [providers.p.models.m.tariffs.peak]
            multiplier = 2.0

            [[providers.p.models.m.schedule]]
            tariff = "peak"
            windows = [["01:00", "01:00"]]
            "#,
        );
        let err = validate(&file).expect_err("zero-length window is invalid");
        assert!(err.field_path.contains("windows"), "path: {err}");
    }

    #[test]
    fn wrap_around_window_is_accepted() {
        let file = parse_toml(
            r#"
            [providers.p.models.m.tariffs.peak]
            multiplier = 2.0

            [[providers.p.models.m.schedule]]
            tariff = "peak"
            windows = [["22:00", "02:00"]]
            "#,
        );
        let (config, _) = validate(&file).expect("wrap-around is legal");
        let rule = &config.providers["p"].models["m"];
        let window = rule.schedule[0].windows[0];
        assert!(window.wraps_midnight());
        assert_eq!(
            window.start,
            NaiveTime::from_hms_opt(22, 0, 0).expect("22:00")
        );
        assert_eq!(window.end, NaiveTime::from_hms_opt(2, 0, 0).expect("02:00"));
    }

    #[test]
    fn invalid_time_string_reports_field_path() {
        let file = parse_toml(
            r#"
            [providers.p.models.m.tariffs.peak]
            multiplier = 2.0

            [[providers.p.models.m.schedule]]
            tariff = "peak"
            windows = [["01:00", "25:99"]]
            "#,
        );
        let err = validate(&file).expect_err("bad time is rejected");
        assert!(
            err.field_path.contains("schedule") && err.field_path.contains("windows"),
            "path should point at the window: {err}"
        );
    }

    #[test]
    fn unknown_weekday_is_rejected() {
        let file = parse_toml(
            r#"
            [providers.p.models.m.tariffs.peak]
            multiplier = 2.0

            [[providers.p.models.m.schedule]]
            tariff = "peak"
            weekdays = ["Funday"]
            windows = [["01:00", "04:00"]]
            "#,
        );
        let err = validate(&file).expect_err("bad weekday is rejected");
        assert!(err.field_path.contains("weekdays"), "path: {err}");
    }

    #[test]
    fn weekdays_parse_case_insensitively_and_expand_names() {
        let file = parse_toml(
            r#"
            [providers.p.models.m.tariffs.peak]
            multiplier = 2.0

            [[providers.p.models.m.schedule]]
            tariff = "peak"
            weekdays = ["mon", "Tuesday", "FRI"]
            windows = [["01:00", "04:00"]]
            "#,
        );
        let (config, _) = validate(&file).expect("validates");
        assert_eq!(
            config.providers["p"].models["m"].schedule[0].weekdays,
            vec![Weekday::Mon, Weekday::Tue, Weekday::Fri]
        );
    }

    #[test]
    fn on_rule_expiry_defaults_to_fallback() {
        let file = parse_toml(
            r#"
            [providers.p.models.m.cost]
            input = 1.0
            output = 2.0
            "#,
        );
        let (config, _) = validate(&file).expect("validates");
        assert_eq!(
            config.providers["p"].models["m"].on_rule_expiry,
            OnRuleExpiry::Fallback
        );
    }

    #[test]
    fn effective_window_parses_rfc3339() {
        let file = parse_toml(
            r#"
            [providers.p.models.m]
            effective_until = "2026-12-31T23:59:59Z"
            "#,
        );
        let (config, _) = validate(&file).expect("validates");
        let until = config.providers["p"].models["m"]
            .effective_until
            .expect("parsed");
        assert_eq!(until.timestamp(), 1_798_761_599);
    }

    #[test]
    fn invalid_effective_until_reports_field_path() {
        let file = parse_toml(
            r#"
            [providers.p.models.m]
            effective_until = "soon"
            "#,
        );
        let err = validate(&file).expect_err("bad timestamp is rejected");
        assert!(err.field_path.contains("effective_until"), "path: {err}");
    }

    #[test]
    fn default_tariff_must_exist() {
        let file = parse_toml(
            r#"
            [providers.p.models.m]
            default_tariff = "off_peak"
            "#,
        );
        let err = validate(&file).expect_err("dangling default_tariff is rejected");
        assert!(err.field_path.contains("default_tariff"), "path: {err}");
    }
}
