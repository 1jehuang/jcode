//! Lenient serde deserializers for tool-input fields.
//!
//! Some providers (notably Claude's tool calling) emit numeric and boolean
//! tool arguments as JSON *strings* — e.g. `{"compactions": "0"}` instead of
//! `{"compactions": 0}` — even when the tool's JSON schema declares the field
//! as `integer`/`boolean`. `serde_json` is strict by default and rejects these
//! with errors like `invalid type: string "0", expected u32`, which causes the
//! whole tool call to fail (see issue #106 for `end_ambient_cycle`).
//!
//! These helpers accept either the native JSON type or a string representation
//! and coerce to the target type, so tool inputs survive that provider quirk.
//! Apply them per-field with `#[serde(deserialize_with = ...)]` on fields whose
//! schema declares a numeric or boolean type.

use serde::{Deserialize, Deserializer, de};
use std::fmt;

struct U32OrString;

impl<'de> de::Visitor<'de> for U32OrString {
    type Value = u32;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a u32 or a string representing a u32")
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<u32, E> {
        u32::try_from(v).map_err(|_| E::custom(format!("number {v} out of range for u32")))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<u32, E> {
        u32::try_from(v).map_err(|_| E::custom(format!("number {v} out of range for u32")))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<u32, E> {
        if v.fract() == 0.0 && v >= 0.0 && v <= f64::from(u32::MAX) {
            Ok(v as u32)
        } else {
            Err(E::custom(format!("number {v} is not a valid u32")))
        }
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<u32, E> {
        let trimmed = v.trim();
        trimmed
            .parse::<u32>()
            .map_err(|_| E::custom(format!("string {trimmed:?} is not a valid u32")))
    }
}

/// Deserialize a `u32` from either a JSON number or a numeric string.
pub fn u32_from_string_or_number<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_any(U32OrString)
}

/// Deserialize an `Option<u32>` from either a JSON number, a numeric string,
/// or null/missing. Empty strings deserialize to `None`.
pub fn opt_u32_from_string_or_number<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: Deserializer<'de>,
{
    // Accept null, missing, number, or string.
    let value: Option<serde_json::Value> = Option::deserialize(deserializer)?;
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(serde_json::Value::String(s)) => s
            .trim()
            .parse::<u32>()
            .map(Some)
            .map_err(|_| de::Error::custom(format!("string {:?} is not a valid u32", s.trim()))),
        Some(serde_json::Value::Number(n)) => {
            if let Some(u) = n.as_u64() {
                u32::try_from(u)
                    .map(Some)
                    .map_err(|_| de::Error::custom(format!("number {u} out of range for u32")))
            } else if let Some(f) = n.as_f64() {
                if f.fract() == 0.0 && f >= 0.0 && f <= f64::from(u32::MAX) {
                    Ok(Some(f as u32))
                } else {
                    Err(de::Error::custom(format!("number {f} is not a valid u32")))
                }
            } else {
                Err(de::Error::custom("number is not a valid u32"))
            }
        }
        Some(other) => Err(de::Error::custom(format!(
            "expected u32 or numeric string, got {other}"
        ))),
    }
}

struct BoolOrString;

impl<'de> de::Visitor<'de> for BoolOrString {
    type Value = bool;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a bool or a string representing a bool")
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<bool, E> {
        Ok(v)
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<bool, E> {
        match v.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "y" => Ok(true),
            "false" | "0" | "no" | "n" | "" => Ok(false),
            other => Err(E::custom(format!("string {other:?} is not a valid bool"))),
        }
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<bool, E> {
        Ok(v != 0)
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<bool, E> {
        Ok(v != 0)
    }
}

/// Deserialize a `bool` from either a JSON bool or a string/number representation.
pub fn bool_from_string_or_bool<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_any(BoolOrString)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Demo {
        #[serde(deserialize_with = "u32_from_string_or_number")]
        n: u32,
        #[serde(default, deserialize_with = "opt_u32_from_string_or_number")]
        maybe: Option<u32>,
        #[serde(default, deserialize_with = "bool_from_string_or_bool")]
        flag: bool,
    }

    #[test]
    fn accepts_native_number() {
        let d: Demo = serde_json::from_value(serde_json::json!({"n": 5})).unwrap();
        assert_eq!(d.n, 5);
    }

    #[test]
    fn accepts_string_number() {
        // The #106 case: Claude sends {"compactions": "0"}.
        let d: Demo = serde_json::from_value(serde_json::json!({"n": "0"})).unwrap();
        assert_eq!(d.n, 0);
        let d: Demo = serde_json::from_value(serde_json::json!({"n": "42"})).unwrap();
        assert_eq!(d.n, 42);
    }

    #[test]
    fn rejects_garbage_string() {
        let r: Result<Demo, _> = serde_json::from_value(serde_json::json!({"n": "abc"}));
        assert!(r.is_err());
    }

    #[test]
    fn optional_handles_null_empty_string_and_values() {
        let d: Demo = serde_json::from_value(serde_json::json!({"n": 1})).unwrap();
        assert_eq!(d.maybe, None);
        let d: Demo = serde_json::from_value(serde_json::json!({"n": 1, "maybe": null})).unwrap();
        assert_eq!(d.maybe, None);
        let d: Demo = serde_json::from_value(serde_json::json!({"n": 1, "maybe": ""})).unwrap();
        assert_eq!(d.maybe, None);
        let d: Demo = serde_json::from_value(serde_json::json!({"n": 1, "maybe": "7"})).unwrap();
        assert_eq!(d.maybe, Some(7));
        let d: Demo = serde_json::from_value(serde_json::json!({"n": 1, "maybe": 9})).unwrap();
        assert_eq!(d.maybe, Some(9));
    }

    #[test]
    fn bool_accepts_string_and_native() {
        let d: Demo = serde_json::from_value(serde_json::json!({"n": 1, "flag": true})).unwrap();
        assert!(d.flag);
        let d: Demo = serde_json::from_value(serde_json::json!({"n": 1, "flag": "true"})).unwrap();
        assert!(d.flag);
        let d: Demo = serde_json::from_value(serde_json::json!({"n": 1, "flag": "false"})).unwrap();
        assert!(!d.flag);
    }
}
