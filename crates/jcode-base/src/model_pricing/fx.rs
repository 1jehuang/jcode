//! Reference FX conversion: one base currency plus a ratio table.
//!
//! `[pricing].fx_rates` is the *only* rate source in v1 — there is no automatic
//! fetch — so a missing rate must degrade rather than guess. Every rate is
//! `1 {base} = N {target}`, and any pair is derived through the base:
//! `a -> b = amount / ratio(a) * ratio(b)`, where the base's own ratio is 1.

use jcode_provider_core::{Currency, Money};
use std::collections::BTreeMap;

/// `fx_base` plus the hand-written `1 base = N target` table.
#[derive(Debug, Clone, PartialEq)]
pub struct FxTable {
    pub base: Currency,
    pub rates: BTreeMap<Currency, f64>,
}

impl FxTable {
    pub fn new(base: Currency, rates: BTreeMap<Currency, f64>) -> Self {
        Self { base, rates }
    }

    /// The table the live `[pricing]` config declares.
    pub fn from_config(pricing: &crate::config::PricingConfig) -> Self {
        Self::new(pricing.fx_base.clone(), pricing.fx_rates.clone())
    }

    /// Units of `currency` per 1 `base`; the base itself is always 1.
    ///
    /// A rate that is missing, zero, negative, or non-finite is `None`: a
    /// nonsensical ratio must not be used as if it were a real one.
    pub fn ratio(&self, currency: &Currency) -> Option<f64> {
        if currency == &self.base {
            return Some(1.0);
        }
        self.rates
            .get(currency)
            .copied()
            .filter(|ratio| ratio.is_finite() && *ratio > 0.0)
    }

    /// Whether this table can derive `from -> to`.
    pub fn can_convert(&self, from: &Currency, to: &Currency) -> bool {
        self.ratio(from).is_some() && self.ratio(to).is_some()
    }
}

/// Convert `money` into `target`, or `None` when a rate is missing.
///
/// `None` means "this table cannot say", never "assume 1:1": callers must fall
/// back to the native currency instead of inventing a number.
pub fn convert(money: &Money, target: &Currency, fx: &FxTable) -> Option<Money> {
    if &money.currency == target {
        return Some(money.clone());
    }
    let from = fx.ratio(&money.currency)?;
    let to = fx.ratio(target)?;
    let amount = money.amount / from * to;
    amount
        .is_finite()
        .then(|| Money::new(amount, target.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jcode_provider_core::Money;

    fn table() -> FxTable {
        FxTable::new(
            Currency::usd(),
            BTreeMap::from([(Currency::new("CNY"), 7.2), (Currency::new("EUR"), 0.9)]),
        )
    }

    #[test]
    fn convert_uses_base_rates() {
        let fx = table();

        let cny = convert(&Money::usd(10.0), &Currency::new("CNY"), &fx).expect("USD -> CNY");
        assert!((cny.amount - 72.0).abs() < 1e-9, "got {cny:?}");
        assert_eq!(cny.currency.as_str(), "CNY");

        let usd = convert(
            &Money::new(72.0, Currency::new("CNY")),
            &Currency::usd(),
            &fx,
        )
        .expect("CNY -> USD");
        assert!((usd.amount - 10.0).abs() < 1e-9, "got {usd:?}");

        // Through the base, never a hardcoded pair: 7.2 CNY = 1 USD = 0.9 EUR.
        let eur = convert(
            &Money::new(7.2, Currency::new("CNY")),
            &Currency::new("EUR"),
            &fx,
        )
        .expect("CNY -> EUR");
        assert!((eur.amount - 0.9).abs() < 1e-9, "got {eur:?}");
    }

    #[test]
    fn base_currency_ratio_is_one() {
        let fx = table();
        assert_eq!(fx.ratio(&Currency::usd()), Some(1.0));
        assert_eq!(fx.ratio(&Currency::new("usd")), Some(1.0));
        assert_eq!(fx.ratio(&Currency::new("CNY")), Some(7.2));
    }

    #[test]
    fn missing_rate_is_not_guessed() {
        let fx = table();
        assert_eq!(fx.ratio(&Currency::new("JPY")), None);
        assert!(!fx.can_convert(&Currency::new("JPY"), &Currency::usd()));
        assert!(
            convert(
                &Money::new(100.0, Currency::new("JPY")),
                &Currency::usd(),
                &fx
            )
            .is_none(),
            "a currency with no rate must not be converted"
        );
        assert!(
            convert(&Money::usd(1.0), &Currency::new("JPY"), &fx).is_none(),
            "a target with no rate must not be invented"
        );
    }

    #[test]
    fn identity_conversion_needs_no_rate_entry() {
        // The same currency is always 1:1, rate table or not.
        let fx = FxTable::new(Currency::usd(), BTreeMap::new());
        let money = Money::new(3.5, Currency::new("XYZ"));
        let same = convert(&money, &Currency::new("XYZ"), &fx).expect("identity");
        assert_eq!(same, money);
    }
}
