//! Tariff tiers: which named rate card one entry uses at one instant.
//!
//! Spec 4.3 semantics, implemented literally and with the clock injected:
//!
//! * `schedule` is matched in declaration order, **first match wins**;
//! * a window is the half-open local interval `[start, end)`, where "local"
//!   means the UTC instant shifted by the rule's fixed `utc_offset_minutes`
//!   (no IANA timezone library anywhere in v1);
//! * `weekdays` is decided on the *local* date, and an empty list means every
//!   day;
//! * `start > end` is a window that wraps past midnight (F23) and — together
//!   with its `weekdays` gate — belongs to the day it *starts* on, so
//!   `["22:00", "02:00"]` with `weekdays = ["Fri"]` covers Friday 22:00
//!   through Saturday 01:59:59;
//! * no match falls back to `default_tariff`, then to the entry's own `cost`;
//! * `Tariff::Multiplier(m)` scales every known base field, `Tariff::Absolute`
//!   overrides the fields it writes and leaves the rest to `cost`;
//! * `effective_from` (inclusive) / `effective_until` (exclusive) bound the
//!   whole entry, tariffs included (F8/F20).
//!
//! Nothing here reads the wall clock: every entry point takes the instant it
//! should evaluate, which is what makes peak/off-peak boundary behaviour
//! testable at all.

use crate::config::{CostFields, ScheduleRule, Tariff, TimeWindow};
use crate::model_pricing::entry::ModelPricingEntry;
use chrono::{DateTime, Datelike, Duration, NaiveTime, Utc, Weekday};
use std::time::SystemTime;

/// The tariff one entry selects at one instant, with its complete rates.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct SelectedTier {
    /// Name of the tariff that supplied the rates. `None` when the entry's own
    /// `cost` applies: no schedule matched and no usable `default_tariff`.
    pub(super) tariff: Option<String>,
    /// The base `cost` with the selected tariff applied field by field.
    pub(super) cost: CostFields,
}

/// The tariff in effect at `at`, with the complete rates it produces.
///
/// `None` means the *entry* is out of effect at `at` (`effective_from` /
/// `effective_until`), which tariff selection cannot answer around; the caller
/// applies `entry.on_rule_expiry` (F8/F20): fall back to the next layer, or
/// refuse to price.
pub(super) fn resolve_tier(entry: &ModelPricingEntry, at: SystemTime) -> Option<SelectedTier> {
    if !entry.is_active_at(at) {
        return None;
    }
    let at: DateTime<Utc> = at.into();

    if let Some(rule) = entry.schedule.iter().find(|rule| rule_matches(rule, at))
        && let Some(cost) = tariff_cost(entry, &rule.tariff)
    {
        return Some(SelectedTier {
            tariff: Some(rule.tariff.clone()),
            cost,
        });
    }

    if let Some(name) = entry.default_tariff.as_deref()
        && let Some(cost) = tariff_cost(entry, name)
    {
        return Some(SelectedTier {
            tariff: Some(name.to_string()),
            cost,
        });
    }

    // Nothing selected: the entry's own `cost` is the rate card.
    Some(SelectedTier {
        tariff: None,
        cost: entry.cost.clone(),
    })
}

/// The rates a named tariff produces from `entry.cost`.
///
/// `None` when the name is not defined. Config validation rejects dangling
/// names, so this is for hand-edited cache files: warn and let the caller
/// degrade to the base cost rather than silently dropping a price.
fn tariff_cost(entry: &ModelPricingEntry, name: &str) -> Option<CostFields> {
    match entry.tariffs.get(name) {
        Some(tariff) => Some(apply_tariff(&entry.cost, tariff)),
        None => {
            crate::logging::warn(&format!(
                "pricing rule references unknown tariff `{name}`; using the base cost"
            ));
            None
        }
    }
}

/// Apply one tariff to a base rate card.
fn apply_tariff(cost: &CostFields, tariff: &Tariff) -> CostFields {
    match tariff {
        // A multiplier scales the fields the base card states and leaves the
        // absent ones absent: scaling "unknown" would invent a rate.
        Tariff::Multiplier(factor) => CostFields {
            input: cost.input.map(|rate| rate * factor),
            output: cost.output.map(|rate| rate * factor),
            cache_read: cost.cache_read.map(|rate| rate * factor),
            cache_write: cost.cache_write.map(|rate| rate * factor),
        },
        // An absolute card overrides the fields it writes, field by field.
        Tariff::Absolute(fields) => CostFields {
            input: fields.input.or(cost.input),
            output: fields.output.or(cost.output),
            cache_read: fields.cache_read.or(cost.cache_read),
            cache_write: fields.cache_write.or(cost.cache_write),
        },
    }
}

/// Whether one schedule rule covers `at`.
fn rule_matches(rule: &ScheduleRule, at: DateTime<Utc>) -> bool {
    // v1 deliberately has no IANA timezone support: the rule states a fixed
    // offset, so shifting the instant is the whole conversion, and the shifted
    // instant's clock reading and date are what "local" means here.
    let local = at + Duration::minutes(i64::from(rule.utc_offset_minutes));
    let time = local.time();
    let day = local.weekday();

    if rule.windows.is_empty() {
        // An empty window list means "every time of day", mirroring "empty
        // weekdays means every day", so `weekdays = ["Sat", "Sun"]` alone reads
        // as "the whole weekend".
        return weekday_enabled(&rule.weekdays, day);
    }

    rule.windows
        .iter()
        .any(|window| window_matches(window, time, day, &rule.weekdays))
}

/// Whether a local clock reading falls inside one window.
///
/// `time` and `day` are the local reading and its local weekday (already
/// shifted by the rule's fixed offset); `day` is only used to attribute the
/// after-midnight half of a wrapping window to the day it started on.
fn window_matches(
    window: &TimeWindow,
    time: NaiveTime,
    day: Weekday,
    weekdays: &[Weekday],
) -> bool {
    window_start_day(window, time, day)
        .is_some_and(|start_day| weekday_enabled(weekdays, start_day))
}

/// The day a window is attributed to when it covers a local reading, or `None`
/// when it does not cover it.
///
/// Windows are half-open (`[start, end)`). `start > end` wraps past midnight
/// (F23): the after-midnight half still belongs to the day the window started,
/// so Friday 23:00 and Saturday 01:00 of `["22:00", "02:00"]` both come back as
/// `Fri`.
fn window_start_day(window: &TimeWindow, time: NaiveTime, day: Weekday) -> Option<Weekday> {
    if window.wraps_midnight() {
        if time >= window.start {
            Some(day)
        } else if time < window.end {
            Some(previous_weekday(day))
        } else {
            None
        }
    } else if time >= window.start && time < window.end {
        Some(day)
    } else {
        None
    }
}

/// Whether `day` is one of the rule's start days; an empty list is every day.
fn weekday_enabled(weekdays: &[Weekday], day: Weekday) -> bool {
    weekdays.is_empty() || weekdays.contains(&day)
}

fn previous_weekday(day: Weekday) -> Weekday {
    match day {
        Weekday::Mon => Weekday::Sun,
        Weekday::Tue => Weekday::Mon,
        Weekday::Wed => Weekday::Tue,
        Weekday::Thu => Weekday::Wed,
        Weekday::Fri => Weekday::Thu,
        Weekday::Sat => Weekday::Fri,
        Weekday::Sun => Weekday::Sat,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_pricing::sources::{self, ConfigPrice};
    use chrono::TimeZone;
    use jcode_provider_core::Currency;
    use std::collections::BTreeMap;

    /// A fixed instant, always UTC and always explicit (never the wall clock).
    fn at(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> SystemTime {
        SystemTime::from(
            Utc.with_ymd_and_hms(year, month, day, hour, minute, second)
                .single()
                .expect("valid instant"),
        )
    }

    fn window(start: (u32, u32, u32), end: (u32, u32, u32)) -> TimeWindow {
        TimeWindow {
            start: NaiveTime::from_hms_opt(start.0, start.1, start.2).expect("valid start"),
            end: NaiveTime::from_hms_opt(end.0, end.1, end.2).expect("valid end"),
        }
    }

    fn weekdays(days: &[Weekday]) -> Vec<Weekday> {
        days.to_vec()
    }

    fn rule(
        tariff: &str,
        utc_offset_minutes: i32,
        days: &[Weekday],
        windows: Vec<TimeWindow>,
    ) -> ScheduleRule {
        ScheduleRule {
            tariff: tariff.to_string(),
            utc_offset_minutes,
            weekdays: weekdays(days),
            windows,
        }
    }

    /// The window matcher itself, with no clock involved: local reading +
    /// local weekday + window in, attribution out.
    #[test]
    fn window_start_day_is_half_open_and_attributes_wrapping_hits() {
        let clock = |hour: u32, minute: u32| {
            NaiveTime::from_hms_opt(hour, minute, 0).expect("valid local time")
        };

        let daytime = window((1, 0, 0), (4, 0, 0));
        assert_eq!(window_start_day(&daytime, clock(0, 59), Weekday::Mon), None);
        assert_eq!(
            window_start_day(&daytime, clock(1, 0), Weekday::Mon),
            Some(Weekday::Mon),
            "`start` is inclusive"
        );
        assert_eq!(
            window_start_day(&daytime, clock(3, 59), Weekday::Mon),
            Some(Weekday::Mon)
        );
        assert_eq!(
            window_start_day(&daytime, clock(4, 0), Weekday::Mon),
            None,
            "`end` is exclusive"
        );

        let wrapping = window((22, 0, 0), (2, 0, 0));
        assert_eq!(
            window_start_day(&wrapping, clock(22, 0), Weekday::Fri),
            Some(Weekday::Fri)
        );
        assert_eq!(
            window_start_day(&wrapping, clock(0, 0), Weekday::Sat),
            Some(Weekday::Fri),
            "after midnight the window still belongs to its start day"
        );
        assert_eq!(
            window_start_day(&wrapping, clock(1, 59), Weekday::Sat),
            Some(Weekday::Fri)
        );
        assert_eq!(window_start_day(&wrapping, clock(2, 0), Weekday::Sat), None);
        assert_eq!(
            window_start_day(&wrapping, clock(12, 0), Weekday::Sat),
            None
        );

        // The weekday gate is applied to the attributed day, not to `day`.
        assert!(window_matches(
            &wrapping,
            clock(1, 0),
            Weekday::Sat,
            &[Weekday::Fri]
        ));
        assert!(!window_matches(
            &wrapping,
            clock(1, 0),
            Weekday::Sat,
            &[Weekday::Sat]
        ));
    }

    /// The DeepSeek shape from the spec: a base card, `off_peak` = the base
    /// rates, `peak` = 2x, peak windows = UTC 01:00-04:00 and 06:00-10:00 on
    /// weekdays only.
    fn deepseek_entry() -> ModelPricingEntry {
        let mut tariffs = BTreeMap::new();
        tariffs.insert("peak".to_string(), Tariff::Multiplier(2.0));
        tariffs.insert(
            "off_peak".to_string(),
            Tariff::Absolute(CostFields {
                input: Some(4.5),
                output: Some(13.5),
                cache_read: Some(0.15),
                cache_write: None,
            }),
        );
        ModelPricingEntry {
            cost: CostFields {
                input: Some(4.5),
                output: Some(13.5),
                cache_read: Some(0.15),
                cache_write: Some(0.6),
            },
            tariffs,
            schedule: vec![rule(
                "peak",
                0,
                &[
                    Weekday::Mon,
                    Weekday::Tue,
                    Weekday::Wed,
                    Weekday::Thu,
                    Weekday::Fri,
                ],
                vec![window((1, 0, 0), (4, 0, 0)), window((6, 0, 0), (10, 0, 0))],
            )],
            default_tariff: Some("off_peak".to_string()),
            effective_from: None,
            effective_until: None,
            on_rule_expiry: crate::config::OnRuleExpiry::Fallback,
        }
    }

    fn selected(entry: &ModelPricingEntry, at: SystemTime) -> SelectedTier {
        resolve_tier(entry, at).expect("entry is in effect at this instant")
    }

    fn tier(entry: &ModelPricingEntry, at: SystemTime) -> String {
        selected(entry, at)
            .tariff
            .unwrap_or_else(|| "base".to_string())
    }

    fn input_rate(entry: &ModelPricingEntry, at: SystemTime) -> f64 {
        selected(entry, at).cost.input.expect("input rate")
    }

    #[test]
    fn peak_window_boundaries_are_half_open() {
        let entry = deepseek_entry();
        // Monday 2026-09-14, UTC. DeepSeek's peak hours are 01:00-04:00 and
        // 06:00-10:00, so every boundary needs its own assertion on both sides.
        let cases: [((u32, u32, u32), &str, f64); 8] = [
            ((0, 59, 59), "off_peak", 4.5),
            ((1, 0, 0), "peak", 9.0),
            ((3, 59, 59), "peak", 9.0),
            ((4, 0, 0), "off_peak", 4.5),
            ((5, 59, 59), "off_peak", 4.5),
            ((6, 0, 0), "peak", 9.0),
            ((9, 59, 59), "peak", 9.0),
            ((10, 0, 0), "off_peak", 4.5),
        ];
        for ((hour, minute, second), expected_tier, expected_input) in cases {
            let instant = at(2026, 9, 14, hour, minute, second);
            assert_eq!(
                tier(&entry, instant),
                expected_tier,
                "tariff at {hour:02}:{minute:02}:{second:02} UTC"
            );
            assert_eq!(
                input_rate(&entry, instant),
                expected_input,
                "input rate at {hour:02}:{minute:02}:{second:02} UTC"
            );
        }

        // The whole rate card follows the selected tier, not just `input`.
        let peak = selected(&entry, at(2026, 9, 14, 2, 0, 0));
        assert_eq!(peak.cost.output, Some(27.0));
        assert_eq!(peak.cost.cache_read, Some(0.3));
        assert_eq!(peak.cost.cache_write, Some(1.2));
    }

    #[test]
    fn weekday_gating_uses_the_local_date() {
        let entry = deepseek_entry();
        // Friday 2026-09-11 09:59:59 is peak; one second later the window closes.
        assert_eq!(tier(&entry, at(2026, 9, 11, 9, 59, 59)), "peak");
        assert_eq!(tier(&entry, at(2026, 9, 11, 10, 0, 0)), "off_peak");
        // Saturday 02:00 and Sunday 01:00 are inside the UTC peak hours but the
        // weekdays gate keeps them off-peak all day.
        assert_eq!(tier(&entry, at(2026, 9, 12, 2, 0, 0)), "off_peak");
        assert_eq!(tier(&entry, at(2026, 9, 13, 1, 30, 0)), "off_peak");
        // Monday 00:00 is before the first window; Sunday 01:00 is a weekend.
        assert_eq!(tier(&entry, at(2026, 9, 14, 0, 0, 0)), "off_peak");
        assert_eq!(tier(&entry, at(2026, 9, 13, 1, 0, 0)), "off_peak");
        assert_eq!(tier(&entry, at(2026, 9, 14, 1, 0, 0)), "peak");
    }

    /// `["22:00", "02:00"]` on Fridays runs Friday 22:00 through Saturday
    /// 01:59:59, and the after-midnight half is attributed to Friday (F23).
    #[test]
    fn wrapping_window_belongs_to_its_start_day() {
        let mut entry = deepseek_entry();
        entry.schedule = vec![rule(
            "peak",
            0,
            &[Weekday::Fri],
            vec![window((22, 0, 0), (2, 0, 0))],
        )];

        assert_eq!(tier(&entry, at(2026, 9, 11, 21, 59, 59)), "off_peak");
        assert_eq!(tier(&entry, at(2026, 9, 11, 22, 0, 0)), "peak");
        assert_eq!(tier(&entry, at(2026, 9, 11, 23, 59, 59)), "peak");
        assert_eq!(tier(&entry, at(2026, 9, 12, 0, 0, 0)), "peak");
        assert_eq!(tier(&entry, at(2026, 9, 12, 1, 59, 59)), "peak");
        assert_eq!(tier(&entry, at(2026, 9, 12, 2, 0, 0)), "off_peak");
        // Thursday 23:00 starts on Thursday, which is not enabled.
        assert_eq!(tier(&entry, at(2026, 9, 10, 23, 0, 0)), "off_peak");
        // Friday 01:00 would be the after-midnight half of Thursday's window.
        assert_eq!(tier(&entry, at(2026, 9, 11, 1, 0, 0)), "off_peak");
    }

    #[test]
    fn first_matching_schedule_rule_wins() {
        let mut entry = deepseek_entry();
        // Declared first, and matching at the same instant as the peak rule.
        entry
            .schedule
            .insert(0, rule("promo", 0, &[], vec![window((1, 0, 0), (2, 0, 0))]));
        entry
            .tariffs
            .insert("promo".to_string(), Tariff::Multiplier(0.5));

        assert_eq!(tier(&entry, at(2026, 9, 14, 1, 30, 0)), "promo");
        assert_eq!(input_rate(&entry, at(2026, 9, 14, 1, 30, 0)), 2.25);
        // Outside the promo window the next declared rule still matches.
        assert_eq!(tier(&entry, at(2026, 9, 14, 3, 0, 0)), "peak");
        // Nothing matches: the default tariff applies.
        assert_eq!(tier(&entry, at(2026, 9, 14, 5, 0, 0)), "off_peak");
    }

    #[test]
    fn multiplier_scales_every_known_field_and_keeps_absent_ones_absent() {
        let entry = deepseek_entry();
        let peak = selected(&entry, at(2026, 9, 14, 2, 0, 0));
        assert_eq!(peak.tariff.as_deref(), Some("peak"));
        assert_eq!(peak.cost.input, Some(9.0));
        assert_eq!(peak.cost.output, Some(27.0));
        assert_eq!(peak.cost.cache_read, Some(0.3));
        assert_eq!(peak.cost.cache_write, Some(1.2));

        let mut partial = deepseek_entry();
        partial.cost.cache_write = None;
        let peak = selected(&partial, at(2026, 9, 14, 2, 0, 0));
        assert_eq!(
            peak.cost.cache_write, None,
            "scaling an absent rate must not invent one"
        );
    }

    #[test]
    fn absolute_tariff_overrides_written_fields_only() {
        let mut entry = deepseek_entry();
        entry.tariffs.insert(
            "promo".to_string(),
            Tariff::Absolute(CostFields {
                input: Some(1.0),
                output: None,
                cache_read: Some(0.01),
                cache_write: None,
            }),
        );
        entry.schedule = vec![rule("promo", 0, &[], vec![window((3, 0, 0), (4, 0, 0))])];

        let promo = selected(&entry, at(2026, 9, 14, 3, 30, 0));
        assert_eq!(promo.cost.input, Some(1.0), "written field wins");
        assert_eq!(promo.cost.output, Some(13.5), "absent field falls back");
        assert_eq!(promo.cost.cache_read, Some(0.01));
        assert_eq!(promo.cost.cache_write, Some(0.6), "absent field falls back");

        // `off_peak` is also an absolute card and leaves `cache_write` unset.
        let off_peak = selected(&entry, at(2026, 9, 14, 5, 0, 0));
        assert_eq!(off_peak.cost.cache_write, Some(0.6));
        assert_eq!(off_peak.cost.input, Some(4.5));
    }

    #[test]
    fn default_tariff_applies_when_nothing_matches() {
        let mut entry = deepseek_entry();
        entry.schedule.clear();
        let resolved = selected(&entry, at(2026, 9, 14, 2, 0, 0));
        assert_eq!(resolved.tariff.as_deref(), Some("off_peak"));
        assert_eq!(resolved.cost.input, Some(4.5));
    }

    #[test]
    fn missing_or_unknown_tariff_falls_back_to_base_cost() {
        let mut no_default = deepseek_entry();
        no_default.schedule.clear();
        no_default.default_tariff = None;
        let resolved = selected(&no_default, at(2026, 9, 14, 2, 0, 0));
        assert_eq!(resolved.tariff, None);
        assert_eq!(resolved.cost, no_default.cost);

        let mut dangling_default = deepseek_entry();
        dangling_default.schedule.clear();
        dangling_default.default_tariff = Some("gone".to_string());
        let resolved = selected(&dangling_default, at(2026, 9, 14, 2, 0, 0));
        assert_eq!(resolved.tariff, None);
        assert_eq!(resolved.cost, dangling_default.cost);

        // A schedule entry naming a tariff the entry does not define (possible
        // for hand-edited cache files) must not panic either: it degrades to
        // the next tier the entry can actually resolve.
        let mut dangling_schedule = deepseek_entry();
        dangling_schedule.schedule = vec![rule("gone", 0, &[], vec![window((1, 0, 0), (4, 0, 0))])];
        let resolved = selected(&dangling_schedule, at(2026, 9, 14, 2, 0, 0));
        assert_eq!(resolved.tariff.as_deref(), Some("off_peak"));

        let mut dangling_only = dangling_schedule.clone();
        dangling_only.default_tariff = None;
        let resolved = selected(&dangling_only, at(2026, 9, 14, 2, 0, 0));
        assert_eq!(resolved.tariff, None);
        assert_eq!(resolved.cost, dangling_only.cost);
    }

    #[test]
    fn empty_weekdays_and_windows_mean_every_day_and_every_time() {
        let mut entry = deepseek_entry();
        entry.schedule = vec![rule("peak", 0, &[], vec![window((1, 0, 0), (4, 0, 0))])];
        assert_eq!(
            tier(&entry, at(2026, 9, 12, 2, 0, 0)),
            "peak",
            "empty weekdays means every day of the week"
        );
        assert_eq!(tier(&entry, at(2026, 9, 13, 8, 0, 0)), "off_peak");

        entry.schedule = vec![rule("peak", 0, &[Weekday::Sat, Weekday::Sun], Vec::new())];
        assert_eq!(
            tier(&entry, at(2026, 9, 12, 13, 0, 0)),
            "peak",
            "empty windows means every time of day"
        );
        assert_eq!(tier(&entry, at(2026, 9, 14, 13, 0, 0)), "off_peak");
    }

    #[test]
    fn windows_are_evaluated_in_offset_local_time() {
        let mut entry = deepseek_entry();
        // A +08:00 rule written in Beijing hours: 09:00-12:00 local, Mondays.
        entry.schedule = vec![rule(
            "peak",
            480,
            &[Weekday::Mon],
            vec![window((9, 0, 0), (12, 0, 0))],
        )];

        // 2026-09-14T01:00:00Z == Monday 09:00 Beijing.
        assert_eq!(tier(&entry, at(2026, 9, 14, 1, 0, 0)), "peak");
        assert_eq!(tier(&entry, at(2026, 9, 14, 0, 59, 59)), "off_peak");
        assert_eq!(tier(&entry, at(2026, 9, 14, 4, 0, 0)), "off_peak");
        // Sunday 01:00 UTC is Sunday 09:00 Beijing: the local weekday decides.
        assert_eq!(tier(&entry, at(2026, 9, 13, 1, 0, 0)), "off_peak");
    }

    #[test]
    fn validity_bounds_expire_at_the_instant() {
        let mut entry = deepseek_entry();
        let from = Utc
            .with_ymd_and_hms(2026, 9, 14, 1, 0, 0)
            .single()
            .expect("valid instant");
        let until = Utc
            .with_ymd_and_hms(2026, 9, 14, 3, 0, 0)
            .single()
            .expect("valid instant");
        entry.effective_from = Some(from);
        entry.effective_until = Some(until);

        assert_eq!(
            tier(&entry, SystemTime::from(until - Duration::seconds(1))),
            "peak",
            "one second before the upper bound is still in effect"
        );
        assert_eq!(
            resolve_tier(&entry, SystemTime::from(until)),
            None,
            "`effective_until` is exclusive"
        );
        assert_eq!(
            tier(&entry, SystemTime::from(from)),
            "peak",
            "`effective_from` is inclusive"
        );
        assert_eq!(
            resolve_tier(&entry, SystemTime::from(from - Duration::seconds(1))),
            None,
            "nothing applies before `effective_from`"
        );
    }

    // ---- config-level wiring: the card handed downstream is tier-selected ----

    /// The DeepSeek peak/off-peak card as a user would write it. `#EXTRA#` is
    /// replaced so validity keys can be added to the model table without
    /// re-opening it after its sub-tables.
    const PEAK_CONFIG: &str = r#"
[pricing.providers.deepseek]
currency = "CNY"

[pricing.providers.deepseek.models."deepseek-v4-pro"]
default_tariff = "off_peak"
#EXTRA#
[pricing.providers.deepseek.models."deepseek-v4-pro".cost]
input = 4.5
output = 13.5
cache_read = 0.15

[pricing.providers.deepseek.models."deepseek-v4-pro".tariffs.off_peak]
input = 4.5
output = 13.5
cache_read = 0.15

[pricing.providers.deepseek.models."deepseek-v4-pro".tariffs.peak]
multiplier = 2.0

[[pricing.providers.deepseek.models."deepseek-v4-pro".schedule]]
tariff = "peak"
utc_offset_minutes = 0
weekdays = ["Mon", "Tue", "Wed", "Thu", "Fri"]
windows = [["01:00", "04:00"], ["06:00", "10:00"]]
"#;

    fn peak_config(extra_keys: &str) -> String {
        PEAK_CONFIG.replace("#EXTRA#", extra_keys)
    }

    /// Run `f` with `toml` as the user's `config.toml` in an isolated home.
    fn with_config<T>(toml: &str, f: impl FnOnce() -> T) -> T {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let prev_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());
        std::fs::write(temp.path().join("config.toml"), toml).expect("write config.toml");
        crate::config::invalidate_config_cache();

        let out = f();

        crate::config::invalidate_config_cache();
        if let Some(prev) = prev_home {
            crate::env::set_var("JCODE_HOME", prev);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
        out
    }

    fn hit(price: ConfigPrice) -> Option<(ModelPricingEntry, Currency)> {
        match price {
            ConfigPrice::Hit { entry, currency } => Some((*entry, currency)),
            ConfigPrice::Absent | ConfigPrice::NoPrice => None,
        }
    }

    /// The card the resolver hands downstream: config lookup, field-level merge
    /// with the next layer, then tariff selection on the merged base.
    fn resolved(
        provider: &str,
        model: &str,
        at: SystemTime,
    ) -> Option<(ModelPricingEntry, Currency)> {
        let (entry, currency) = hit(sources::config_price(provider, model, at))?;
        let card = sources::resolve_card(entry, currency, provider, model, at);
        Some((card.entry, card.currency))
    }

    /// Deliverable 2: the resolved card carries the rates *as selected at the
    /// call instant*, so the billing path only has to read `cost`.
    #[test]
    fn resolved_card_carries_the_tier_selected_at_the_call_time() {
        with_config(&peak_config(""), || {
            let (peak, currency) =
                resolved("deepseek", "deepseek-v4-pro", at(2026, 9, 14, 2, 0, 0))
                    .expect("peak instant is a config hit");
            assert_eq!(currency.as_str(), "CNY");
            assert_eq!(peak.cost.input, Some(9.0), "peak = 2x the base card");
            assert_eq!(peak.cost.output, Some(27.0));
            assert_eq!(peak.cost.cache_read, Some(0.3));

            let (off_peak, _) = resolved("deepseek", "deepseek-v4-pro", at(2026, 9, 14, 0, 30, 0))
                .expect("off-peak instant is a config hit");
            assert_eq!(off_peak.cost.input, Some(4.5));

            let (weekend, _) = resolved("deepseek", "deepseek-v4-pro", at(2026, 9, 12, 2, 0, 0))
                .expect("weekends are off-peak");
            assert_eq!(weekend.cost.input, Some(4.5));

            // The scalar entry point sees the same tier.
            let money = crate::model_pricing::effective_cost(
                "deepseek",
                "deepseek-v4-pro",
                at(2026, 9, 14, 2, 0, 0),
            )
            .expect("priced");
            assert_eq!(money.currency.as_str(), "CNY");
            let expected = (9.0 * 25_000.0 + 27.0 * 5_000.0) / 1_000_000.0;
            assert!((money.amount - expected).abs() <= 1e-9);
        });
    }

    /// F8/F20 at the exact boundary instant, for both `on_rule_expiry` values.
    #[test]
    fn expiry_at_the_instant_follows_on_rule_expiry() {
        let fallback = peak_config("effective_until = \"2026-09-14T03:00:00Z\"");
        with_config(&fallback, || {
            let (before, _) = resolved("deepseek", "deepseek-v4-pro", at(2026, 9, 14, 2, 59, 59))
                .expect("one second before the bound the card is still in effect");
            assert_eq!(before.cost.input, Some(9.0), "and still on the peak tier");

            assert!(
                matches!(
                    sources::config_price("deepseek", "deepseek-v4-pro", at(2026, 9, 14, 3, 0, 0)),
                    ConfigPrice::Absent
                ),
                "`fallback` drops the expired card so the next layer prices it"
            );
        });

        let no_price = peak_config(
            "effective_until = \"2026-09-14T03:00:00Z\"\non_rule_expiry = \"no_price\"",
        );
        with_config(&no_price, || {
            assert!(
                matches!(
                    sources::config_price("deepseek", "deepseek-v4-pro", at(2026, 9, 14, 3, 0, 0)),
                    ConfigPrice::NoPrice
                ),
                "`no_price` refuses to price instead of falling back"
            );
            assert!(
                matches!(
                    sources::config_price(
                        "deepseek",
                        "deepseek-v4-pro",
                        at(2026, 9, 14, 2, 59, 59)
                    ),
                    ConfigPrice::Hit { .. }
                ),
                "the card still prices one second earlier"
            );
        });
    }

    /// Regression: a tariff scales the **effective** base card, i.e. after the
    /// field-level merge. Scaling before the merge used to scale only the fields
    /// a partial USD card had written, leaving the merged-in fields at their
    /// unpremultiplied value (peak `input` doubled, `output` not).
    #[test]
    fn usd_partial_card_multiplier_also_scales_merged_fields() {
        let config = r#"
[pricing.providers.deepseek.models."deepseek-v4-pro".cost]
input = 1.0

[pricing.providers.deepseek.models."deepseek-v4-pro".tariffs.peak]
multiplier = 2.0

[[pricing.providers.deepseek.models."deepseek-v4-pro".schedule]]
tariff = "peak"
utc_offset_minutes = 0
weekdays = ["Mon", "Tue", "Wed", "Thu", "Fri"]
windows = [["01:00", "04:00"]]
"#;
        with_config(config, || {
            crate::model_pricing::clear_memory_cache_for_tests();
            crate::model_pricing::save_test_cache(&[(
                "deepseek",
                "deepseek-v4-pro",
                crate::model_pricing::ModelCost {
                    input_usd_per_mtok: 1.0,
                    output_usd_per_mtok: 8.0,
                    cache_read_usd_per_mtok: None,
                    cache_write_usd_per_mtok: None,
                },
            )]);

            let (card, currency) =
                resolved("deepseek", "deepseek-v4-pro", at(2026, 9, 14, 2, 0, 0))
                    .expect("peak instant is a config hit");
            assert!(currency.is_usd(), "an absent `currency` key means USD");
            assert_eq!(card.cost.input, Some(2.0), "the written field is scaled");
            assert_eq!(
                card.cost.output,
                Some(16.0),
                "the merged-in field must be scaled by the same tariff"
            );
        });
    }
}
