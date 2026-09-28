//! Currency-aware rendering of cost figures for the TUI widget and `/usage`.
//!
//! `[display].currency = "native"` (the default) converts nothing: each amount
//! keeps the currency it was priced in. Naming a concrete code converts through
//! [`FxTable`], and a missing rate falls back to the native currency with a
//! visible note — never an invented rate, and never a `$` on a non-USD amount.

use crate::model_pricing::{FxTable, convert};
use jcode_provider_core::{Currency, Money};
use std::collections::BTreeMap;

/// Which currency costs are shown in, plus the rates needed to get there.
///
/// `target == None` is `[display].currency = "native"`: no conversion at all.
#[derive(Debug, Clone)]
pub struct DisplayTarget {
    target: Option<Currency>,
    fx: FxTable,
}

/// One displayed amount: the number, the currency it is really in, and — when
/// the requested conversion was impossible — why it stayed native.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayAmount {
    pub amount: f64,
    pub currency: Currency,
    pub note: Option<String>,
}

impl DisplayAmount {
    /// An amount already in USD, the currency the built-in pricing path uses;
    /// native mode shows it as-is.
    pub fn usd(amount: f64) -> Self {
        Self {
            amount,
            currency: Currency::usd(),
            note: None,
        }
    }
}

impl DisplayTarget {
    /// Show every amount in its own currency.
    pub fn native() -> Self {
        Self {
            target: None,
            fx: FxTable::new(Currency::usd(), BTreeMap::new()),
        }
    }

    pub fn new(target: Option<Currency>, fx: FxTable) -> Self {
        Self { target, fx }
    }

    /// The live `[display].currency` + `[pricing]` FX table.
    pub fn from_config() -> Self {
        let config = crate::config::config();
        let target = if config.display.currency_is_native() {
            None
        } else {
            Some(Currency::new(&config.display.currency))
        };
        let fx = FxTable::from_config(&crate::model_pricing::pricing_config());
        Self::new(target, fx)
    }

    pub fn target(&self) -> Option<&Currency> {
        self.target.as_ref()
    }

    pub fn fx(&self) -> &FxTable {
        &self.fx
    }

    /// The currency to label a zero amount with: the display target when one is
    /// configured, otherwise USD (the currency jcode prices in by default).
    pub fn zero_currency(&self) -> Currency {
        self.target.clone().unwrap_or_else(Currency::usd)
    }

    /// Resolve one amount for display.
    ///
    /// When the requested conversion is impossible the amount is returned in the
    /// currency it was already in, with a `note` naming the missing rate. The
    /// note is the warning: it is shown next to the amount, so a user who asked
    /// for EUR and sees CNY is told why.
    pub fn resolve(&self, amount: f64, currency: &Currency) -> DisplayAmount {
        let Some(target) = self.target.as_ref() else {
            return DisplayAmount {
                amount,
                currency: currency.clone(),
                note: None,
            };
        };
        if target == currency {
            return DisplayAmount {
                amount,
                currency: currency.clone(),
                note: None,
            };
        }
        match convert(&Money::new(amount, currency.clone()), target, &self.fx) {
            Some(money) => DisplayAmount {
                amount: money.amount,
                currency: money.currency,
                note: None,
            },
            None => DisplayAmount {
                amount,
                currency: currency.clone(),
                note: Some(format!("no {target} rate")),
            },
        }
    }

    /// Resolve a per-currency bucket map into display rows, largest first.
    ///
    /// Buckets that resolve to the same currency (a conversion target) are
    /// summed; buckets that cannot be converted stay in their own currency.
    pub fn resolve_buckets<V: Into<f64> + Copy>(
        &self,
        buckets: &BTreeMap<Currency, V>,
    ) -> Vec<DisplayAmount> {
        let mut merged: BTreeMap<Currency, DisplayAmount> = BTreeMap::new();
        for (currency, amount) in buckets {
            let resolved = self.resolve((*amount).into(), currency);
            match merged.get_mut(&resolved.currency) {
                Some(existing) => {
                    existing.amount += resolved.amount;
                    existing.note = existing.note.take().or(resolved.note);
                }
                None => {
                    merged.insert(resolved.currency.clone(), resolved);
                }
            }
        }
        let mut rows: Vec<DisplayAmount> = merged.into_values().collect();
        rows.sort_by(|a, b| {
            b.amount
                .total_cmp(&a.amount)
                .then_with(|| a.currency.cmp(&b.currency))
        });
        rows
    }

    /// [`Self::resolve_buckets`] for a session total that has not billed
    /// anything yet: an empty total still renders as a zero rather than
    /// collapsing the widget line.
    ///
    /// A configured display currency names itself, so `[display] currency =
    /// "CNY"` keeps rendering `CNY 0.0000`. In native mode there is no currency
    /// to name - nothing was spent in *any* currency - and labelling the zero
    /// `$` would read as "zero dollars" rather than "no spend recorded", so the
    /// row is left unlabelled.
    pub fn resolve_totals<V: Into<f64> + Copy>(
        &self,
        buckets: &BTreeMap<Currency, V>,
    ) -> Vec<DisplayAmount> {
        if buckets.is_empty() {
            return vec![match self.target.as_ref() {
                Some(target) => DisplayAmount {
                    amount: 0.0,
                    currency: target.clone(),
                    note: None,
                },
                None => DisplayAmount {
                    amount: 0.0,
                    currency: Currency::new(""),
                    note: None,
                },
            }];
        }
        self.resolve_buckets(buckets)
    }
}

/// Attach a display note to the primary (largest) row of a resolved total,
/// keeping any note the currency resolution already put there.
///
/// This is how the F8/F20 marker rides along with the figure it explains: the
/// note belongs on the amount the user reads, not in a log line — the pricing
/// resolver runs per call and the note is rendered every frame, so the two must
/// not be the same event.
pub fn note_primary(rows: &mut [DisplayAmount], note: &str) {
    let Some(primary) = rows.first_mut() else {
        return;
    };
    match primary.note.as_mut() {
        Some(existing) => {
            existing.push_str(", ");
            existing.push_str(note);
        }
        None => primary.note = Some(note.to_string()),
    }
}

/// Say, on the amount itself, why the session cost is not what the user's own
/// `[pricing]` config asked for.
///
/// Three pricing problems silently change what the displayed figure *means*,
/// and all of them used to be invisible in the UI:
///
/// * a rule that is out of effect, where the figure is the next layer's
///   (F8/F20) — an inline card *or* a `[pricing.providers.<vendor>].file`, whose
///   notice names the vendor,
/// * a `[pricing]` section that failed validation, where the resolver dropped
///   every rule in it and the figure may be models.dev's (I-2), and
/// * a card that claims the pair but cannot price the call, where nothing was
///   accrued and the figure stays at zero (spec 4.4): without the note that
///   zero reads as "this call was free".
///
/// The note belongs on the amount the user reads. The validation case is read
/// here rather than stored per call: the resolver memoizes the rejection against
/// the loaded config, so this costs one lock (the same read `from_config`
/// already does on this path) and the rejection is logged once per config
/// instance, not once per frame.
pub fn note_pricing_problems(
    rows: &mut [DisplayAmount],
    notice: Option<&crate::model_pricing::PricingNotice>,
) {
    if let Some(notice) = notice {
        note_primary(rows, &notice.label());
    }
    if let Some(error) = crate::model_pricing::pricing_config_error() {
        note_primary(rows, &format!("invalid [pricing]: {}", error.field_path));
    }
}

/// Format one amount with the currency it is actually in.
///
/// USD keeps the familiar `$` (and the pre-existing rendering); every other
/// currency is prefixed with its ISO code, because no symbol is unambiguous
/// across currencies (`¥` is both CNY and JPY).
pub fn format_amount(amount: f64, currency: &Currency, decimals: usize) -> String {
    if currency.as_str().is_empty() {
        // No currency to name (native mode before anything was billed): the
        // bare number, never a `$` that would read as dollars.
        format!("{:.*}", decimals, amount)
    } else if currency.is_usd() {
        format!("${:.*}", decimals, amount)
    } else {
        format!("{} {:.*}", currency.as_str(), decimals, amount)
    }
}

/// One-line summary of display rows: the primary (largest) amount, followed by
/// a count when the session spans more than one currency.
///
/// Rows must be ordered largest first, as [`DisplayTarget::resolve_buckets`]
/// returns them.
pub fn summarize(rows: &[DisplayAmount], decimals: usize) -> String {
    let Some(primary) = rows.first() else {
        return String::new();
    };
    let mut text = format_amount(primary.amount, &primary.currency, decimals);
    if let Some(note) = primary.note.as_deref() {
        text.push_str(&format!(" ({note})"));
    }
    if rows.len() > 1 {
        text.push_str(&format!(" +{} more", rows.len() - 1));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usd_table() -> FxTable {
        FxTable::new(
            Currency::usd(),
            BTreeMap::from([(Currency::new("CNY"), 7.2), (Currency::new("EUR"), 0.92)]),
        )
    }

    #[test]
    fn native_shows_original_currency_and_never_a_dollar_sign() {
        let target = DisplayTarget::native();
        let rows = target.resolve_buckets(&BTreeMap::from([
            (Currency::new("CNY"), 8.0),
            (Currency::usd(), 1.5),
        ]));

        assert_eq!(rows.len(), 2, "one row per currency: {rows:?}");
        assert_eq!(rows[0].currency.as_str(), "CNY");
        assert!((rows[0].amount - 8.0).abs() < 1e-9);
        assert!(rows[0].note.is_none(), "native mode needs no rate");
        assert_eq!(rows[1].currency.as_str(), "USD");

        assert_eq!(format_amount(8.0, &Currency::new("CNY"), 2), "CNY 8.00");
        assert_eq!(format_amount(1.5, &Currency::usd(), 2), "$1.50");
        assert!(
            !format_amount(8.0, &Currency::new("CNY"), 2).contains('$'),
            "a CNY amount must not be labelled with a dollar sign"
        );
    }

    #[test]
    fn display_conversion_uses_the_target_currency() {
        let target = DisplayTarget::new(Some(Currency::new("CNY")), usd_table());
        let rows = target.resolve_buckets(&BTreeMap::from([(Currency::usd(), 10.0)]));

        assert_eq!(
            rows.len(),
            1,
            "converted amounts merge into one row: {rows:?}"
        );
        assert_eq!(rows[0].currency.as_str(), "CNY");
        assert!((rows[0].amount - 72.0).abs() < 1e-9, "got {rows:?}");
        assert!(rows[0].note.is_none());
    }

    #[test]
    fn missing_rate_falls_back_to_native_with_warning() {
        // `fx_rates` has no EUR rate, so a USD-priced amount cannot be shown in
        // EUR. The amount must stay in its own currency, and the row must say
        // why rather than quietly printing a wrong number.
        let fx = FxTable::new(
            Currency::usd(),
            BTreeMap::from([(Currency::new("CNY"), 7.2)]),
        );
        let target = DisplayTarget::new(Some(Currency::new("EUR")), fx);

        let row = target.resolve(8.0, &Currency::new("CNY"));
        assert_eq!(row.currency.as_str(), "CNY", "falls back to native");
        assert!(
            (row.amount - 8.0).abs() < 1e-9,
            "native amount is untouched"
        );
        let note = row
            .note
            .as_deref()
            .expect("a warning about the missing rate");
        assert!(note.contains("EUR"), "the note names the target: {note}");

        let text = summarize(std::slice::from_ref(&row), 2);
        assert_eq!(text, "CNY 8.00 (no EUR rate)");
    }

    #[test]
    fn unpriceable_bucket_does_not_poison_the_convertible_ones() {
        let fx = FxTable::new(
            Currency::usd(),
            BTreeMap::from([(Currency::new("CNY"), 7.2)]),
        );
        let target = DisplayTarget::new(Some(Currency::new("CNY")), fx);
        let rows = target.resolve_buckets(&BTreeMap::from([
            (Currency::usd(), 1.0),
            (Currency::new("JPY"), 500.0),
        ]));

        // USD converts (7.20 CNY) and JPY keeps its own currency with a note.
        assert_eq!(rows.len(), 2, "{rows:?}");
        let jpy = rows
            .iter()
            .find(|row| row.currency.as_str() == "JPY")
            .unwrap();
        assert!((jpy.amount - 500.0).abs() < 1e-9);
        assert_eq!(jpy.note.as_deref(), Some("no CNY rate"));
        let cny = rows
            .iter()
            .find(|row| row.currency.as_str() == "CNY")
            .unwrap();
        assert!((cny.amount - 7.2).abs() < 1e-9);
    }

    #[test]
    fn summarize_shows_the_primary_amount_and_counts_the_rest() {
        let target = DisplayTarget::native();
        let rows = target.resolve_buckets(&BTreeMap::from([
            (Currency::usd(), 0.42),
            (Currency::new("CNY"), 8.0),
            (Currency::new("EUR"), 1.0),
        ]));

        assert_eq!(summarize(&rows, 4), "CNY 8.0000 +2 more");
        assert_eq!(summarize(&[], 4), "");
    }

    #[test]
    fn note_primary_appends_without_losing_the_rate_note() {
        // The expired-rule marker (F8/F20) rides on the amount as a note. A row
        // that already explains a missing FX rate keeps that explanation: both
        // facts are true and the user needs both.
        let mut rows =
            DisplayTarget::native().resolve_totals(&BTreeMap::from([(Currency::usd(), 0.42)]));
        note_primary(&mut rows, "rule expired");
        assert_eq!(summarize(&rows, 2), "$0.42 (rule expired)");

        let target = DisplayTarget::new(
            Some(Currency::new("EUR")),
            FxTable::new(Currency::usd(), BTreeMap::new()),
        );
        let mut rows = target.resolve_totals(&BTreeMap::from([(Currency::usd(), 8.0)]));
        note_primary(&mut rows, "rule expired");
        assert_eq!(summarize(&rows, 2), "$8.00 (no EUR rate, rule expired)");

        // No rows to label is a no-op, not a panic.
        let mut empty: Vec<DisplayAmount> = Vec::new();
        note_primary(&mut empty, "rule expired");
        assert!(empty.is_empty());
    }

    /// Spec 5.3 (currency/display): in native mode *any* currency is shown as
    /// it was recorded, with its own code and no conversion — not just USD/CNY.
    #[test]
    fn native_mode_leaves_every_currency_alone() {
        let target = DisplayTarget::native();
        let rows = target.resolve_buckets(&BTreeMap::from([
            (Currency::new("eur"), 3.5),
            (Currency::new("JPY"), 1500.0),
        ]));

        assert_eq!(rows.len(), 2, "one row per currency: {rows:?}");
        assert_eq!(rows[0].currency.as_str(), "JPY", "largest first");
        assert!((rows[0].amount - 1500.0).abs() < 1e-9);
        assert_eq!(
            rows[1].currency.as_str(),
            "EUR",
            "codes normalize to upper case"
        );
        assert!((rows[1].amount - 3.5).abs() < 1e-9);
        assert!(
            rows.iter().all(|row| row.note.is_none()),
            "native mode has no rate to be missing: {rows:?}"
        );

        assert_eq!(
            format_amount(1500.0, &Currency::new("JPY"), 2),
            "JPY 1500.00"
        );
        assert_eq!(format_amount(3.5, &Currency::new("EUR"), 2), "EUR 3.50");
        assert_eq!(summarize(&rows, 2), "JPY 1500.00 +1 more");
    }

    #[test]
    fn empty_session_still_renders_zero_in_a_named_currency() {
        let native = DisplayTarget::native();
        assert_eq!(native.zero_currency().as_str(), "USD");
        let native_zero = summarize(&native.resolve_totals(&BTreeMap::<Currency, f64>::new()), 4);
        assert_eq!(native_zero, "0.0000");
        assert!(
            !native_zero.contains('$'),
            "native mode must not invent a dollar sign for an empty total: {native_zero}"
        );

        let target = DisplayTarget::new(Some(Currency::new("CNY")), usd_table());
        assert_eq!(target.zero_currency().as_str(), "CNY");
        assert_eq!(
            summarize(&target.resolve_totals(&BTreeMap::<Currency, f64>::new()), 4),
            "CNY 0.0000"
        );
    }
}
