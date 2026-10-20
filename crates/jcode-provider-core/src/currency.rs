//! Currency codes and money amounts shared across pricing code paths.
//!
//! `Currency` is a thin, normalized wrapper around an ISO 4217 code. It is
//! deliberately permissive: vendors do ship non-standard codes, so unknown
//! codes are accepted and only flagged by [`Currency::is_known_iso`] as a
//! heuristic (used for a warning, never for a hard failure).
//!
//! Nothing in the pricing path may hardcode a specific currency or currency
//! pair; USD is only the default when no price source states otherwise.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Common ISO 4217 codes, used only to decide whether to warn about an
/// unrecognized code. This list is intentionally not exhaustive: a code
/// missing here is still a valid `Currency`.
const KNOWN_ISO: &[&str] = &[
    "AED", "ARS", "AUD", "BRL", "CAD", "CHF", "CLP", "CNY", "COP", "CZK", "DKK", "EGP", "EUR",
    "GBP", "HKD", "HUF", "IDR", "ILS", "INR", "ISK", "JPY", "KRW", "MXN", "MYR", "NGN", "NOK",
    "NZD", "PEN", "PHP", "PKR", "PLN", "RON", "RUB", "SAR", "SEK", "SGD", "THB", "TRY", "TWD",
    "UAH", "USD", "VND", "ZAR",
];

/// A currency code, normalized to uppercase.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Currency(String);

impl Currency {
    /// Normalize `code` (trim + uppercase) into a `Currency`.
    pub fn new(code: &str) -> Self {
        Self(code.trim().to_ascii_uppercase())
    }

    /// The normalized code.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this is a code we recognize as a common ISO 4217 currency.
    ///
    /// Unknown codes are still valid; this only gates a warning.
    pub fn is_known_iso(&self) -> bool {
        KNOWN_ISO.contains(&self.0.as_str())
    }

    /// The default currency used when no price source states otherwise.
    pub fn usd() -> Self {
        Self::new("USD")
    }

    /// Whether this is USD.
    pub fn is_usd(&self) -> bool {
        self.0 == "USD"
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Currency {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::new(s))
    }
}

/// An amount in a specific currency.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Money {
    pub amount: f64,
    pub currency: Currency,
}

impl Money {
    pub fn new(amount: f64, currency: Currency) -> Self {
        Self { amount, currency }
    }

    pub fn usd(amount: f64) -> Self {
        Self::new(amount, Currency::usd())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_normalizes_case_and_whitespace() {
        assert_eq!(Currency::new(" cny ").as_str(), "CNY");
        assert_eq!(Currency::new("usd").as_str(), "USD");
        assert_eq!(Currency::new("Eur").as_str(), "EUR");
    }

    #[test]
    fn distinct_codes_normalize_to_equal_values() {
        assert_eq!(Currency::new("cny"), Currency::new("CNY"));
    }

    #[test]
    fn unknown_code_is_accepted_but_not_known_iso() {
        let custom = Currency::new("xyz");
        assert_eq!(custom.as_str(), "XYZ");
        assert!(!custom.is_known_iso());
    }

    #[test]
    fn common_codes_are_known_iso() {
        for code in ["USD", "CNY", "EUR", "JPY", "GBP", "HKD", "KRW"] {
            assert!(Currency::new(code).is_known_iso(), "{code} should be known");
        }
    }

    #[test]
    fn usd_helper_and_predicate() {
        assert!(Currency::usd().is_usd());
        assert!(Currency::new("usd").is_usd());
        assert!(!Currency::new("CNY").is_usd());
    }

    #[test]
    fn serde_is_the_raw_code() {
        let json = serde_json::to_string(&Currency::new("cny")).expect("serialize");
        assert_eq!(json, "\"CNY\"");
        let back: Currency = serde_json::from_str("\"CNY\"").expect("deserialize");
        assert_eq!(back, Currency::new("CNY"));
    }

    #[test]
    fn money_keeps_amount_and_currency_together() {
        let money = Money::usd(1.25);
        assert!((money.amount - 1.25).abs() < 1e-12);
        assert!(money.currency.is_usd());
    }

    #[test]
    fn ordering_is_by_code() {
        let mut codes = vec![Currency::new("USD"), Currency::new("CNY")];
        codes.sort();
        assert_eq!(codes[0].as_str(), "CNY");
        assert_eq!(codes[1].as_str(), "USD");
    }
}
