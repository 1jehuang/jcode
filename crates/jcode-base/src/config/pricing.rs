//! Validation and conversion for the `[pricing]` config section.
//!
//! The serde shape lives in `jcode-config-types::pricing` (a `*-types` crate
//! that must not depend on `jcode-provider-core`, where `Currency` lives).
//! Everything that needs real types — currency normalization, time-window
//! parsing, cross-field checks — happens here, and every error carries the
//! config path that produced it.

use chrono::{DateTime, NaiveTime, Utc, Weekday};
use jcode_config_types::{
    ContextTierFile, CostFile, ModelPricingRuleFile, PricingConfigFile, PricingSourceFile,
    ScheduleRuleFile, TariffFile,
};
use jcode_provider_core::Currency;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

pub use jcode_config_types::OnRuleExpiry;

/// The only `[[pricing.sources]].format` v1 understands: models.dev's own
/// `{provider: {models: {id: {cost, ...}}}}` shape.
pub const SOURCE_FORMAT_MODELS_DEV_V1: &str = "models_dev_v1";

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

/// A long-context overlay: above `min_input_tokens`, apply `tariff`.
///
/// Same vocabulary as a named tariff ([`Tariff`]): a multiplier scales the rate
/// card the schedule already selected, an absolute card overrides it field by
/// field. Matched in declaration order, first match wins, and only for a call
/// whose reported input token count is *strictly greater* than the threshold.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextTier {
    pub min_input_tokens: u64,
    pub tariff: Tariff,
}

/// A validated per-model rate rule.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelPricingRule {
    pub cost: Option<CostFields>,
    pub tariffs: BTreeMap<String, Tariff>,
    pub schedule: Vec<ScheduleRule>,
    pub context_tiers: Vec<ContextTier>,
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

/// A validated `[[pricing.sources]]` entry.
///
/// The validation that matters to the merge is done once, here: the list is
/// kept in **resolution order** (priority ascending, ties broken by `id`), so
/// lookups do not have to re-sort and the tie-break rule is visible in one
/// place. `currency` is the currency every number the sheet states is
/// denominated in.
#[derive(Debug, Clone, PartialEq)]
pub struct PricingSource {
    pub id: String,
    /// The resolved local file the sheet is read from. A bare name has already
    /// been resolved under `~/.jcode/cache/`; `~` has been expanded. jcode never
    /// fetches a price sheet, so this is always a path on disk.
    pub path: PathBuf,
    /// Provider identities this sheet may price; empty means every provider.
    pub scope: Vec<String>,
    /// Model globs this sheet may price; empty means every model.
    pub models: Vec<String>,
    pub priority: i64,
    pub currency: Currency,
}

/// The validated runtime view of `[pricing]`.
#[derive(Debug, Clone, PartialEq)]
pub struct PricingConfig {
    pub fx_base: Currency,
    pub fx_rates: BTreeMap<Currency, f64>,
    pub providers: BTreeMap<String, ProviderPricing>,
    /// Extra price sheets in resolution order (priority ascending, then `id`).
    pub sources: Vec<PricingSource>,
}

impl Default for PricingConfig {
    fn default() -> Self {
        Self {
            fx_base: Currency::usd(),
            fx_rates: BTreeMap::new(),
            providers: BTreeMap::new(),
            sources: Vec::new(),
        }
    }
}

impl PricingConfig {
    /// Whether the user configured anything at all. When empty, the pricing
    /// path must behave exactly as it did before this feature existed.
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty() && self.fx_rates.is_empty() && self.sources.is_empty()
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

    // Cross-currency ordering converts every estimate into `fx_base`. Every
    // route jcode ships is USD, so a non-USD `fx_base` with no USD rate leaves
    // those routes unconvertible: they keep their native currency and sort
    // below every comparable route, which is a silent-looking change in the
    // model picker. Say so once, without failing (the config is still usable).
    if !fx_base.is_usd() && !fx_rates.contains_key(&Currency::usd()) {
        warnings.push(format!(
            "pricing.fx_base: fx_base is `{fx_base}` but `pricing.fx_rates` has no `USD` entry, \
             so cross-currency ordering is disabled for every USD-priced route (they keep their \
             native currency and sort last); add `USD = <rate>` to compare them"
        ));
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
        sources: convert_sources(&file.sources, &mut warnings)?,
    };
    Ok((config, warnings))
}

/// Validate `[[pricing.sources]]` and put the entries in resolution order.
///
/// A malformed entry is a hard error carrying its config path, so the display
/// can name the line to fix (`invalid [pricing]: pricing.sources[0].file`); it
/// must not be a log-only warning, because a source that silently does not load
/// looks exactly like a source that has nothing to say.
///
/// The order is `priority` ascending with ties broken by `id` lexicographically
/// (spec 4.2.1's "同层确定性"): `HashMap` iteration order must never decide which
/// of two sheets prices a model.
///
/// `id` is **optional**: the single-source case is one line
/// (`file = "deepseek.json"`). An entry without one gets a
/// deterministic id derived from its location ([`derive_source_id`]), and a
/// derived id that would collide with another entry's explicit or derived id is
/// disambiguated deterministically (`prices`, `prices-2`, …) in declaration
/// order rather than panicking or silently merging two sheets under one cache
/// key. Explicit ids keep today's meaning: they must be non-empty and unique.
fn convert_sources(
    sources: &[PricingSourceFile],
    warnings: &mut Vec<String>,
) -> Result<Vec<PricingSource>, PricingConfigError> {
    // Reserve explicit ids first, so a derived id never takes a name an
    // explicit entry later in the file already claimed.
    let explicit: Vec<Option<String>> = {
        let mut reserved: Vec<String> = Vec::new();
        let mut explicit: Vec<Option<String>> = Vec::with_capacity(sources.len());
        for (index, source) in sources.iter().enumerate() {
            let path = format!("pricing.sources[{index}]");
            match source.id.as_deref().map(str::trim) {
                Some("") => {
                    return Err(PricingConfigError::new(
                        format!("{path}.id"),
                        "a pricing source `id` must not be empty; omit it to derive one from \
                         the location, or write a non-empty name",
                    ));
                }
                Some(id) => {
                    if reserved.iter().any(|existing| existing == id) {
                        return Err(PricingConfigError::new(
                            format!("{path}.id"),
                            format!("duplicate source id `{id}`; source ids must be unique"),
                        ));
                    }
                    reserved.push(id.to_string());
                    explicit.push(Some(id.to_string()));
                }
                None => explicit.push(None),
            }
        }
        explicit
    };

    let mut used: Vec<String> = explicit.iter().flatten().cloned().collect();
    let mut converted: Vec<PricingSource> = Vec::with_capacity(sources.len());
    for (index, source) in sources.iter().enumerate() {
        let path = format!("pricing.sources[{index}]");
        let resolved = parse_source_location(&source.file, &format!("{path}.file"))?;

        let id = match &explicit[index] {
            Some(id) => id.clone(),
            None => {
                let id = unique_derived_id(&resolved, index, &used);
                used.push(id.clone());
                id
            }
        };

        let mut scope = Vec::with_capacity(source.scope.len());
        for (scope_index, entry) in source.scope.iter().enumerate() {
            let entry = entry.trim();
            if entry.is_empty() {
                return Err(PricingConfigError::new(
                    format!("{path}.scope[{scope_index}]"),
                    "a scope entry must name a provider or profile; omit `scope` to \
                     cover every provider",
                ));
            }
            scope.push(entry.to_string());
        }

        let mut models = Vec::with_capacity(source.models.len());
        for (model_index, pattern) in source.models.iter().enumerate() {
            let pattern = pattern.trim();
            if pattern.is_empty() {
                return Err(PricingConfigError::new(
                    format!("{path}.models[{model_index}]"),
                    "a model glob must not be empty; omit `models` to cover every model",
                ));
            }
            models.push(pattern.to_string());
        }

        match source.format.as_deref().map(str::trim) {
            None | Some("") | Some(SOURCE_FORMAT_MODELS_DEV_V1) => {}
            Some(other) => {
                return Err(PricingConfigError::new(
                    format!("{path}.format"),
                    format!(
                        "unknown source format `{other}`; only `{SOURCE_FORMAT_MODELS_DEV_V1}` is \
                         supported"
                    ),
                ));
            }
        }

        let currency = parse_currency(
            source.currency.as_deref().unwrap_or("USD"),
            &format!("{path}.currency"),
            warnings,
        );

        converted.push(PricingSource {
            id,
            path: resolved,
            scope,
            models,
            priority: source.priority.unwrap_or(0),
            currency,
        });
    }

    // Stable and total: priority first, then the id, so the merge order is a
    // property of the config rather than of the writer's file layout.
    converted.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(converted)
}

/// The id for a `[[pricing.sources]]` entry that did not name one.
///
/// The id is a cache key (`~/.jcode/cache/pricing_sources.json`) and appears in
/// user-facing labels (`rule expired (pricing source \`prices\`)`), so it must
/// be deterministic and stable across edits, and recognisably derived from the
/// file the entry points at: its file stem, so `/opt/jcode/prices.json` and a
/// bare `prices.json` both yield `prices`. A path that yields nothing usable
/// falls back to `source{n}` (1-based), which is what makes an otherwise
/// anonymous sheet identifiable.
fn derive_source_id(path: &std::path::Path, index: usize) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::trim)
        .filter(|stem| !stem.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("source{}", index + 1))
}

/// [`derive_source_id`], disambiguated against everything already in use.
///
/// Two sheets may obviously point at files with the same name (`prices.json` in
/// two directories). Each id must be unique, and dropping one silently would
/// mean a sheet the user configured never prices anything, so the collision is
/// resolved by appending `-2`, `-3`, … in declaration order. The result depends
/// only on the file's declaration order, never on map iteration order.
fn unique_derived_id(path: &std::path::Path, index: usize, used: &[String]) -> String {
    let base = derive_source_id(path, index);
    if !used.contains(&base) {
        return base;
    }
    let mut suffix = 2;
    loop {
        let candidate = format!("{base}-{suffix}");
        if !used.contains(&candidate) {
            return candidate;
        }
        suffix += 1;
    }
}

/// `~/.jcode/cache/`, where a bare source name resolves and where the models.dev
/// catalog and the sources cache live.
fn cache_dir() -> PathBuf {
    crate::storage::jcode_dir()
        .unwrap_or_else(|_| PathBuf::from(".").join(".jcode"))
        .join("cache")
}

/// The names of jcode's own files in `~/.jcode/cache/`. A bare source name that
/// matches one of these is refused: the user's sheet must never shadow jcode's
/// cache.
fn jcode_cache_file_names() -> [&'static str; 2] {
    [
        crate::model_pricing::catalog_cache_file_name(),
        crate::model_pricing::sources_cache_file_name(),
    ]
}

/// Expand a leading `~` (alone or `~/…`) to the user's home directory. `~user`
/// is not expanded: it is left as a literal path, like every other shell would
/// need a passwd lookup for.
fn expand_tilde(path: PathBuf) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path;
    };
    if text == "~" {
        return dirs::home_dir().unwrap_or(path);
    }
    if let Some(rest) = text.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    path
}

/// Whether `raw` is a bare file name: no path separator and not `~`-anchored.
/// A bare name resolves under `~/.jcode/cache/`; everything else is a path.
fn is_bare_name(raw: &str) -> bool {
    !raw.is_empty() && !raw.contains('/') && !raw.contains('\\') && !raw.starts_with('~')
}

/// Turn a `file` value into the local path a source is read from.
///
/// A price sheet is always a local file: jcode never fetches it. The value is
/// either a **bare name** (`deepseek.json`), which resolves under
/// `~/.jcode/cache/` and is refused if it would shadow one of jcode's own files
/// there, or a **path** (absolute, or relative with a separator, `~` expanded),
/// which is used exactly as written. Anything that is not a readable file there
/// simply contributes no rules.
fn parse_source_location(raw: &str, path: &str) -> Result<PathBuf, PricingConfigError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(PricingConfigError::new(
            path,
            "a pricing source needs a `file` naming a local price sheet",
        ));
    }
    if is_bare_name(raw) {
        if jcode_cache_file_names().contains(&raw) {
            return Err(PricingConfigError::new(
                path,
                format!(
                    "`{raw}` is jcode's own file in {}; choose another name",
                    cache_dir().display()
                ),
            ));
        }
        return Ok(cache_dir().join(raw));
    }
    Ok(expand_tilde(PathBuf::from(raw)))
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

/// Shared with the `model_pricing` catalog parser: a custom `[[pricing.sources]]`
/// sheet states its extension fields in this vocabulary too, so a sheet and a
/// hand-written card validate identically.
pub(crate) fn convert_rule(
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

    let context_tiers =
        convert_context_tiers(&rule.context_tiers, &format!("{path}.context_tiers"))?;

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
        context_tiers,
        default_tariff: rule.default_tariff.clone(),
        effective_from,
        effective_until,
        on_rule_expiry: rule.on_rule_expiry.unwrap_or_default(),
    })
}

/// Reject a rate that is not a finite, non-negative number.
///
/// A rate can be written four ways (a `cost` table, a named tariff, a
/// `context_tiers` entry, or the same shape inside a `[[pricing.sources]]`
/// sheet), and every one of them funnels through here so the whole pricing
/// surface shares one rule. `NaN` is rejected along with the infinities and
/// negatives because `CostState::accrue` has no finite guard: a single `NaN`
/// would poison a session total permanently and a negative rate would truncate
/// to zero and read as "free".
fn validate_rate(value: Option<f64>, path: &str) -> Result<Option<f64>, PricingConfigError> {
    if let Some(value) = value
        && (!value.is_finite() || value < 0.0)
    {
        return Err(PricingConfigError::new(
            path,
            format!("a rate must be a finite, non-negative number, got {value}"),
        ));
    }
    Ok(value)
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
        input: validate_rate(cost.input, &format!("{path}.input"))?,
        output: validate_rate(cost.output, &format!("{path}.output"))?,
        cache_read: validate_rate(cost.cache_read, &format!("{path}.cache_read"))?,
        cache_write: validate_rate(cost.cache_write, &format!("{path}.cache_write"))?,
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
            input: validate_rate(tariff.input, &format!("{path}.input"))?,
            output: validate_rate(tariff.output, &format!("{path}.output"))?,
            cache_read: validate_rate(tariff.cache_read, &format!("{path}.cache_read"))?,
            cache_write: validate_rate(tariff.cache_write, &format!("{path}.cache_write"))?,
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

/// Validate `context_tiers`: threshold above zero, non-empty rates, no
/// duplicate thresholds.
///
/// Each tier reuses [`convert_tariff`], so an absolute tier and an absolute
/// named tariff accept exactly the same shape and produce the same error
/// messages. Duplicate thresholds are rejected because that is the one way the
/// "first match wins" rule could pick a tier the user did not mean: two entries
/// with the same threshold look like one rule written twice.
fn convert_context_tiers(
    tiers: &[ContextTierFile],
    path: &str,
) -> Result<Vec<ContextTier>, PricingConfigError> {
    let mut converted: Vec<ContextTier> = Vec::with_capacity(tiers.len());
    for (index, tier) in tiers.iter().enumerate() {
        let tier_path = format!("{path}[{index}]");
        let Some(min_input_tokens) = tier.min_input_tokens else {
            return Err(PricingConfigError::new(
                &tier_path,
                "a context tier must state `min_input_tokens`",
            ));
        };
        if min_input_tokens == 0 {
            return Err(PricingConfigError::new(
                format!("{tier_path}.min_input_tokens"),
                "a context tier threshold must be greater than zero",
            ));
        }
        if converted
            .iter()
            .any(|existing| existing.min_input_tokens == min_input_tokens)
        {
            return Err(PricingConfigError::new(
                format!("{tier_path}.min_input_tokens"),
                format!("duplicate context tier threshold {min_input_tokens}"),
            ));
        }
        let tariff_file = TariffFile {
            input: tier.input,
            output: tier.output,
            cache_read: tier.cache_read,
            cache_write: tier.cache_write,
            multiplier: tier.multiplier,
        };
        let tariff = convert_tariff(&tariff_file, &tier_path)?;
        converted.push(ContextTier {
            min_input_tokens,
            tariff,
        });
    }
    Ok(converted)
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

    /// A non-USD `fx_base` with no USD rate leaves every built-in (USD) route
    /// unconvertible, which silently disables cross-currency ordering. It is a
    /// warning, not an error: the section still works.
    #[test]
    fn non_usd_fx_base_without_a_usd_rate_warns() {
        let file = parse_toml(
            r#"
            fx_base = "CNY"
            [fx_rates]
            EUR = 0.92
            "#,
        );
        let (config, warnings) = validate(&file).expect("validates");
        assert_eq!(config.fx_base.as_str(), "CNY");
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("fx_base") && warning.contains("USD")),
            "the missing USD rate must be reported: {warnings:?}"
        );

        // With a USD rate the warning is gone: the ordering can convert.
        let file = parse_toml(
            r#"
            fx_base = "CNY"
            [fx_rates]
            USD = 0.14
            "#,
        );
        let (_, warnings) = validate(&file).expect("validates");
        assert!(
            !warnings.iter().any(|warning| warning.contains("fx_base")),
            "a USD rate makes the comparison possible: {warnings:?}"
        );
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
    fn context_tiers_convert_a_multiplier_and_an_absolute_card() {
        let file = parse_toml(
            r#"
            [providers.p.models.m.cost]
            input = 1.0
            output = 2.0

            [[providers.p.models.m.context_tiers]]
            min_input_tokens = 200_000
            multiplier = 2.0

            [[providers.p.models.m.context_tiers]]
            min_input_tokens = 500_000
            input = 9.0
            output = 27.0
            "#,
        );
        let (config, _) = validate(&file).expect("validates");
        let tiers = &config.providers["p"].models["m"].context_tiers;
        assert_eq!(tiers.len(), 2, "declaration order is kept");
        assert_eq!(tiers[0].min_input_tokens, 200_000);
        assert_eq!(tiers[0].tariff, Tariff::Multiplier(2.0));
        assert_eq!(
            tiers[1].tariff,
            Tariff::Absolute(CostFields {
                input: Some(9.0),
                output: Some(27.0),
                cache_read: None,
                cache_write: None,
            })
        );
    }

    #[test]
    fn context_tier_without_a_threshold_is_rejected() {
        let file = parse_toml(
            r#"
            [[providers.p.models.m.context_tiers]]
            multiplier = 2.0
            "#,
        );
        let err = validate(&file).expect_err("a tier must state its threshold");
        assert!(err.field_path.contains("context_tiers[0]"), "path: {err}");
    }

    #[test]
    fn zero_context_tier_threshold_is_rejected() {
        let file = parse_toml(
            r#"
            [[providers.p.models.m.context_tiers]]
            min_input_tokens = 0
            multiplier = 2.0
            "#,
        );
        let err = validate(&file).expect_err("a zero threshold would match every call");
        assert!(err.field_path.contains("min_input_tokens"), "path: {err}");
    }

    #[test]
    fn empty_context_tier_rates_are_rejected() {
        let file = parse_toml(
            r#"
            [[providers.p.models.m.context_tiers]]
            min_input_tokens = 200_000
            "#,
        );
        let err = validate(&file).expect_err("an empty tier is meaningless");
        assert!(err.field_path.contains("context_tiers[0]"), "path: {err}");
    }

    #[test]
    fn duplicate_context_tier_thresholds_are_rejected() {
        let file = parse_toml(
            r#"
            [[providers.p.models.m.context_tiers]]
            min_input_tokens = 200_000
            multiplier = 2.0

            [[providers.p.models.m.context_tiers]]
            min_input_tokens = 200_000
            multiplier = 3.0
            "#,
        );
        let err = validate(&file).expect_err("two tiers with one threshold are ambiguous");
        assert!(err.field_path.contains("context_tiers[1]"), "path: {err}");
    }

    /// A rule with no tiers must not bake `context_tiers = []` into the user's
    /// file the next time anything saves it (the lesson of `9da6f9831`).
    #[test]
    fn an_empty_context_tier_list_is_never_written_back_to_config() {
        let mut config = crate::config::Config::default();
        config.pricing.providers.insert(
            "deepseek".to_string(),
            jcode_config_types::ProviderPricingFile {
                currency: Some("CNY".to_string()),
                models: BTreeMap::from([(
                    "m".to_string(),
                    ModelPricingRuleFile {
                        cost: Some(CostFile {
                            input: Some(1.0),
                            output: Some(2.0),
                            cache_read: None,
                            cache_write: None,
                        }),
                        ..Default::default()
                    },
                )]),
            },
        );
        let toml = toml::to_string_pretty(&config).expect("serialize config");
        assert!(
            toml.contains("[pricing.providers.deepseek.models.m"),
            "the model table is serialized, so the assertion below is not vacuous:\n{toml}"
        );
        assert!(
            !toml.contains("context_tiers"),
            "an empty context_tiers list must not be baked into the user's config:\n{toml}"
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

    /// Every rate a hand-written card can state must be a finite,
    /// non-negative number, and the error must name the offending field:
    /// `nan`/`-inf` would otherwise poison a session total (`CostState::accrue`
    /// has no finite guard) and a negative rate would truncate to zero and read
    /// as "free".
    #[test]
    fn non_finite_or_negative_cost_rates_are_rejected_with_their_path() {
        for (spelling, expected) in [
            ("nan", "cost.input"),
            ("-inf", "cost.output"),
            ("-1.0", "cost.cache_read"),
        ] {
            let field = expected.rsplit('.').next().unwrap();
            let mut body = String::new();
            for name in ["input", "output", "cache_read"] {
                let value = if name == field { spelling } else { "1.0" };
                body.push_str(&format!("{name} = {value}\n"));
            }
            let file = parse_toml(&format!("[providers.p.models.m.cost]\n{body}"));
            let err = validate(&file).expect_err("a non-finite or negative rate is rejected");
            assert!(
                err.field_path.ends_with(expected),
                "expected path ending in {expected}, got {err}"
            );
        }
    }

    #[test]
    fn non_finite_or_negative_tariff_and_tier_rates_are_rejected() {
        for spelling in ["nan", "-inf", "-1.0"] {
            let file = parse_toml(&format!(
                r#"
                [providers.p.models.m.tariffs.peak]
                input = {spelling}
                output = 2.0
                "#,
            ));
            let err = validate(&file).expect_err("a bad tariff rate is rejected");
            assert!(err.field_path.contains("tariffs.peak.input"), "path: {err}");

            let file = parse_toml(&format!(
                r#"
                [[providers.p.models.m.context_tiers]]
                min_input_tokens = 200_000
                output = {spelling}
                "#,
            ));
            let err = validate(&file).expect_err("a bad tier rate is rejected");
            assert!(
                err.field_path.contains("context_tiers[0].output"),
                "path: {err}"
            );
        }
    }

    /// A zero rate is legitimate (`input = 0` is a free tier), and the check is
    /// `>= 0`, so this must stay accepted.
    #[test]
    fn a_zero_rate_is_accepted() {
        let file = parse_toml(
            r#"
            [providers.p.models.m.cost]
            input = 0.0
            output = 0
            "#,
        );
        validate(&file).expect("zero is a valid rate");
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

    /// `Config::save()` serializes the whole struct, so any field without
    /// `skip_serializing_if` gets baked into the user's `config.toml` the next
    /// time anything saves it. An unconfigured `[pricing]` must stay absent.
    #[test]
    fn an_empty_pricing_section_is_never_written_back_to_config() {
        let toml =
            toml::to_string_pretty(&crate::config::Config::default()).expect("serialize config");
        assert!(
            !toml.contains("[pricing"),
            "an empty [pricing] section must not be baked into the user's config:\n{toml}"
        );
    }

    /// The flip side: a section the user actually configured must still round-trip.
    #[test]
    fn a_configured_pricing_section_is_written_back() {
        let mut config = crate::config::Config::default();
        config.pricing.fx_rates.insert("CNY".to_string(), 7.2);
        let toml = toml::to_string_pretty(&config).expect("serialize config");
        assert!(
            toml.contains("[pricing.fx_rates]"),
            "a configured [pricing] must survive a save:\n{toml}"
        );
    }
}
