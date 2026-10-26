//! Lenient serde helpers for string-valued configuration enums.
//!
//! A single unrecognized enum value used to abort parsing of the *entire*
//! `config.toml`, so `Config::load` silently fell back to `Config::default()`
//! and every unrelated setting looked like it was ignored (issue #689: one
//! `diagram_mode = "inline"` line also disabled `centered`, `idle_animation`,
//! `show_thinking`, and `reasoning_display`). Degrading the one field that is
//! actually wrong is far better than discarding the user's configuration.

use serde::Deserialize;

/// Deserialize a string-valued enum, falling back to its `Default` when the
/// value is not recognized.
pub(crate) fn lenient_enum<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    let raw = String::deserialize(deserializer)?;
    let value = serde::de::value::StrDeserializer::<serde::de::value::Error>::new(raw.as_str());
    Ok(T::deserialize(value).unwrap_or_default())
}

/// `Option` variant of [`lenient_enum`]: an unrecognized value becomes `None`
/// ("not configured") rather than a config-wide parse failure.
pub(crate) fn lenient_optional_enum<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    let Some(raw) = Option::<String>::deserialize(deserializer)? else {
        return Ok(None);
    };
    let value = serde::de::value::StrDeserializer::<serde::de::value::Error>::new(raw.as_str());
    Ok(T::deserialize(value).ok())
}

#[derive(Deserialize)]
#[serde(untagged)]
pub(crate) enum LenientBoolInput {
    Bool(bool),
    Int(i64),
    Str(String),
    Other(serde::de::IgnoredAny),
}

pub(crate) fn parse_lenient_bool(default: bool, input: LenientBoolInput) -> bool {
    match input {
        LenientBoolInput::Bool(b) => b,
        LenientBoolInput::Int(i) => i != 0,
        LenientBoolInput::Str(s) => match s.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => default,
        },
        LenientBoolInput::Other(_) => default,
    }
}

pub(crate) fn lenient_bool_false<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(LenientBoolInput::deserialize(deserializer)
        .map(|input| parse_lenient_bool(false, input))
        .unwrap_or(false))
}

pub(crate) fn lenient_bool_true<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(LenientBoolInput::deserialize(deserializer)
        .map(|input| parse_lenient_bool(true, input))
        .unwrap_or(true))
}

#[derive(Deserialize)]
#[serde(untagged)]
enum LenientNumberInput {
    Int(i64),
    Float(f64),
    Str(String),
    Other(serde::de::IgnoredAny),
}

#[allow(clippy::manual_ok_err)]
pub(crate) fn lenient_max_secondary_suggestions<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let parsed = match LenientNumberInput::deserialize(deserializer) {
        Ok(LenientNumberInput::Int(v)) => Some(v),
        Ok(LenientNumberInput::Float(v)) if v.is_finite() => Some(v.round() as i64),
        Ok(LenientNumberInput::Str(s)) => match s.trim().parse::<i64>() {
            Ok(v) => Some(v),
            Err(_) => None,
        },
        _ => None,
    };
    Ok(match parsed {
        Some(v) => v.clamp(1, 20) as usize,
        None => 3,
    })
}

#[derive(Deserialize)]
#[serde(untagged)]
enum LenientStringListInput {
    List(Vec<String>),
    Single(String),
    Other(serde::de::IgnoredAny),
}

pub(crate) fn lenient_string_list<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw_items = match LenientStringListInput::deserialize(deserializer) {
        Ok(LenientStringListInput::List(items)) => items,
        Ok(LenientStringListInput::Single(s)) => {
            s.split(['|', ';', '\n']).map(ToString::to_string).collect()
        }
        _ => Vec::new(),
    };
    let mut out = Vec::new();
    for item in raw_items {
        let trimmed = item.trim();
        if !trimmed.is_empty()
            && !out
                .iter()
                .any(|existing: &String| existing.eq_ignore_ascii_case(trimmed))
        {
            out.push(trimmed.to_string());
        }
    }
    Ok(out)
}

pub(crate) fn lenient_confidence_threshold<'de, D>(deserializer: D) -> Result<f32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let parsed = match LenientNumberInput::deserialize(deserializer) {
        Ok(LenientNumberInput::Float(v)) if v.is_finite() => Some(v as f32),
        Ok(LenientNumberInput::Int(v)) => Some(v as f32),
        Ok(LenientNumberInput::Str(s)) => match s.trim().parse::<f32>() {
            Ok(v) if v.is_finite() => Some(v),
            _ => None,
        },
        _ => None,
    };
    Ok(match parsed {
        Some(v) => v.clamp(0.0, 1.0),
        None => 0.55,
    })
}

/// Regression tests for issue #689: an unrecognized value in one `[display]`
/// enum field must not discard the rest of the user's configuration.
#[cfg(test)]
mod tests {
    use crate::{
        DiagramDisplayMode, DiffDisplayMode, DisplayConfig, LatexRenderingMode,
        MarkdownSpacingMode, ReasoningDisplayMode,
    };

    fn parse(json: &str) -> DisplayConfig {
        serde_json::from_str(json).expect("display config must still parse")
    }

    #[test]
    fn unknown_diagram_mode_keeps_the_rest_of_the_display_config() {
        let display = parse(
            r#"{
                "centered": true,
                "idle_animation": true,
                "show_thinking": true,
                "reasoning_display": "current",
                "diagram_mode": "totally-bogus"
            }"#,
        );

        assert!(display.centered, "unrelated settings must survive");
        assert!(display.idle_animation);
        assert!(display.show_thinking);
        assert_eq!(display.reasoning_display(), ReasoningDisplayMode::Current);
        assert_eq!(display.diagram_mode, DiagramDisplayMode::default());
    }

    /// The exact config from the issue report: `diagram_mode = "inline"` is now
    /// accepted as the inline-only (default) mode.
    #[test]
    fn diagram_mode_inline_is_accepted_as_the_default_mode() {
        let display = parse(r#"{"centered": true, "diagram_mode": "inline"}"#);
        assert!(display.centered);
        assert_eq!(display.diagram_mode, DiagramDisplayMode::None);
    }

    #[test]
    fn unknown_reasoning_display_falls_back_without_losing_other_fields() {
        let display = parse(r#"{"centered": true, "reasoning_display": "nope"}"#);
        assert!(display.centered);
        assert!(
            !display.has_explicit_reasoning_display(),
            "an unparseable value means 'not configured'"
        );
    }

    #[test]
    fn unknown_diff_and_latex_modes_fall_back_to_defaults() {
        let display = parse(
            r#"{"centered": true, "diff_mode": "nope", "latex_rendering": "nope",
                "markdown_spacing": "nope"}"#,
        );
        assert!(display.centered);
        assert_eq!(display.diff_mode, DiffDisplayMode::default());
        assert_eq!(display.latex_rendering, LatexRenderingMode::default());
        assert_eq!(display.markdown_spacing, MarkdownSpacingMode::default());
    }

    /// The removed `overscroll_status` option must not break existing configs.
    #[test]
    fn legacy_overscroll_status_key_is_ignored() {
        let display = parse(r#"{"centered": true, "overscroll_status": "on"}"#);
        assert!(display.centered);
    }

    /// Valid values must still round-trip (leniency must not swallow them).
    #[test]
    fn valid_values_are_unaffected() {
        let display = parse(r#"{"diagram_mode": "pinned", "diff_mode": "full-inline"}"#);
        assert_eq!(display.diagram_mode, DiagramDisplayMode::Pinned);
        assert_eq!(display.diff_mode, DiffDisplayMode::FullInline);
    }

    #[test]
    fn suggested_replies_config_lenient_clamping_and_defaults() {
        let default_cfg = crate::SuggestedRepliesConfig::default();
        assert_eq!(default_cfg.max_secondary_suggestions, 3);
        assert!(default_cfg.show_confidence);
        assert!(default_cfg.custom_prompts.is_empty());

        let cfg: crate::SuggestedRepliesConfig = serde_json::from_str(
            r#"{
                "useOpenGf": "yes",
                "maxSecondarySuggestions": 99,
                "confidenceThreshold": 1.8,
                "showConfidence": "no",
                "custom_questions": ["Проверь тесты", "Запусти clippy", ""],
                "enableFullAutoExplorer": 1,
                "sixLensAutoReview": "on"
            }"#,
        )
        .expect("must parse leniently");
        assert!(cfg.enabled);
        assert!(cfg.use_open_gf);
        assert_eq!(cfg.max_secondary_suggestions, 20);
        assert_eq!(cfg.confidence_threshold, 1.0);
        assert!(!cfg.show_confidence);
        assert_eq!(cfg.custom_prompts, vec!["Проверь тесты", "Запусти clippy"]);
        assert!(cfg.enable_full_auto_explorer);
        assert!(cfg.six_lens_auto_review);

        let low_cfg: crate::SuggestedRepliesConfig = serde_json::from_str(
            r#"{
                "max_secondary_suggestions": -3,
                "confidence_threshold": -0.5,
                "use_open_gf": "invalid-bool"
            }"#,
        )
        .expect("must parse out-of-range values leniently");
        assert_eq!(low_cfg.max_secondary_suggestions, 1);
        assert_eq!(low_cfg.confidence_threshold, 0.0);
        assert!(!low_cfg.use_open_gf);
        assert!(low_cfg.show_confidence);
    }
}
