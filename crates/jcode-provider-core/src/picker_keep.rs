//! `provider.model_picker_keep` keep-list rules for the model picker.
//!
//! A keep-list is the inverse of a prune list: when configured, the model
//! picker renders only rows matching these entries — one model, one lane,
//! one reasoning effort per entry — instead of every catalog model expanded
//! across every lane and every effort rung. Parsing lives here so the TUI
//! and any future picker surface share one grammar; the caller supplies the
//! known-lane vocabulary (fixed lanes plus configured profiles and provider
//! metadata) so this crate stays metadata-free.

/// One parsed `model_picker_keep` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerKeepRule {
    /// Normalized lane selector (`None` = match every lane). Built from the
    /// entry prefix, e.g. `openai-oauth` in `openai-oauth:gpt-6-luna:med`.
    pub lane: Option<String>,
    /// Model id, lowercased for case-insensitive matching.
    pub model: String,
    /// Pinned canonical effort (`None` = collapse matched routes to their
    /// family's default effort row instead of pinning one explicitly).
    pub effort: Option<String>,
}

/// Lane tokens recognized without any caller-supplied configuration.
const FIXED_KEEP_LANES: &[&str] = &[
    "openai-api",
    "openai-oauth",
    "openai-api-key",
    "anthropic-api",
    "claude-api",
    "claude-oauth",
    "openrouter",
    "openai-compatible",
    "ollama",
    "llama.cpp",
];

/// Normalize a lane selector the same way route provider labels normalize:
/// lowercase with non-alphanumerics dropped, so `openai-api`,
/// `OpenAI API`, and `openai api` all compare equal.
pub fn normalize_keep_lane(value: &str) -> String {
    crate::normalize_model_route_provider_label(value)
}

/// True when `segment` names a known lane: a fixed token, or one of the
/// caller-supplied known lanes (configured provider ids, openai-compatible
/// profile ids, provider metadata ids/aliases — all pre-normalized).
fn keep_segment_is_lane(segment: &str, known_lanes: &[String]) -> bool {
    let normalized = normalize_keep_lane(segment);
    if normalized.is_empty() {
        return false;
    }
    FIXED_KEEP_LANES
        .iter()
        .any(|lane| normalize_keep_lane(lane) == normalized)
        || known_lanes.iter().any(|lane| *lane == normalized)
}

/// Parse the trailing `:effort` segment, accepting the colloquial `med`
/// alias for `medium`. Returns `None` when the segment is not an effort so
/// model ids containing colons (`hf.co/org/model:tag`) stay intact.
fn parse_keep_effort_segment(segment: &str) -> Option<String> {
    let canonical = if segment.trim().eq_ignore_ascii_case("med") {
        Some("medium")
    } else {
        crate::reasoning::canonical_reasoning_effort(segment)
    };
    canonical.map(str::to_string)
}

/// Parse all raw `model_picker_keep` entries, dropping empty and malformed
/// ones. `known_lanes` must be pre-normalized (see [`normalize_keep_lane`]).
pub fn parse_picker_keep_rules(
    entries: impl IntoIterator<Item = impl AsRef<str>>,
    known_lanes: &[String],
) -> Vec<PickerKeepRule> {
    entries
        .into_iter()
        .filter_map(|entry| parse_picker_keep_rule(entry.as_ref(), known_lanes))
        .collect()
}

/// Parse one raw entry: `[lane:][lane2:]model[:effort]`. The lane prefix is
/// only split off when it names a known lane; otherwise the whole head is
/// treated as the model id.
fn parse_picker_keep_rule(entry: &str, known_lanes: &[String]) -> Option<PickerKeepRule> {
    let entry = entry.trim();
    if entry.is_empty() {
        return None;
    }
    let lower = entry.to_ascii_lowercase();
    let mut segments: Vec<&str> = lower.split(':').collect();

    let mut effort = None;
    if let Some(last) = segments.last() {
        if let Some(canonical) = parse_keep_effort_segment(last) {
            effort = Some(canonical);
            segments.pop();
        }
    }

    let (lane, model) = if segments.len() >= 2 {
        // Longest known lane prefix wins: `openai-compatible:myprofile:model`
        // first tries the profile-qualified lane, then the bare
        // `openai-compatible` family lane. Unknown prefixes stay part of the
        // model id, so `hf.co/org/model:tag` parses as a bare model.
        let mut split: Option<(Option<String>, String)> = None;
        for k in (1..segments.len()).rev() {
            let candidate_lane = segments[..k].join(":");
            if keep_segment_is_lane(&candidate_lane, known_lanes) {
                let model = segments[k..].join(":");
                split = Some((Some(normalize_keep_lane(&candidate_lane)), model));
                break;
            }
        }
        split.unwrap_or_else(|| (None, segments.join(":")))
    } else {
        (None, segments.join(":"))
    };

    let model = model.trim().to_string();
    if model.is_empty() {
        return None;
    }
    Some(PickerKeepRule {
        lane,
        model,
        effort,
    })
}

/// True when a keep rule selects this route (lane + model; the effort
/// dimension is applied by the picker when it expands effort rows).
pub fn picker_keep_rule_matches_route(
    rule: &PickerKeepRule,
    model: &str,
    route_provider: &str,
    api_method: &str,
) -> bool {
    if rule.model != model.trim().to_ascii_lowercase() {
        return false;
    }
    let Some(lane) = &rule.lane else {
        return true;
    };
    let provider = normalize_keep_lane(route_provider);
    let method = normalize_keep_lane(api_method);
    // The bare `openai-compatible` family lane matches every profile route.
    if lane == "openaicompatible" && method.starts_with("openaicompatible") {
        return true;
    }
    // `openai-compatible:myprofile` normalizes to `openaicompatiblemyprofile`;
    // also expose the bare profile id for convenience.
    let profile_id = api_method
        .split_once(':')
        .map(|(_, profile)| normalize_keep_lane(profile))
        .unwrap_or_default();
    *lane == provider
        || *lane == method
        || (!profile_id.is_empty() && *lane == profile_id)
        || crate::model_route_provider_labels_match(route_provider, lane)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> Vec<String> {
        ["kimi", "glm", "deepseek-vision", "mimo", "qwencloud"]
            .iter()
            .map(|lane| normalize_keep_lane(lane))
            .collect()
    }

    #[test]
    fn parses_bare_model() {
        let rules = parse_picker_keep_rules(["gpt-6-luna"], &[]);
        assert_eq!(
            rules,
            vec![PickerKeepRule {
                lane: None,
                model: "gpt-6-luna".to_string(),
                effort: None,
            }]
        );
    }

    #[test]
    fn parses_lane_model_and_effort() {
        let rules = parse_picker_keep_rules(["openai-oauth:gpt-6-luna:med"], &[]);
        assert_eq!(
            rules,
            vec![PickerKeepRule {
                lane: Some("openaioauth".to_string()),
                model: "gpt-6-luna".to_string(),
                effort: Some("medium".to_string()),
            }]
        );
    }

    #[test]
    fn parses_configured_lane_without_effort() {
        let rules = parse_picker_keep_rules(["kimi:kimi-for-coding"], &known());
        assert_eq!(
            rules,
            vec![PickerKeepRule {
                lane: Some("kimi".to_string()),
                model: "kimi-for-coding".to_string(),
                effort: None,
            }]
        );
    }

    #[test]
    fn unknown_prefix_stays_part_of_model_id() {
        // `hf.co` is not a known lane, so nothing is split off.
        let rules = parse_picker_keep_rules(["hf.co/org/qwen3:q4_k_m"], &known());
        assert_eq!(
            rules,
            vec![PickerKeepRule {
                lane: None,
                model: "hf.co/org/qwen3:q4_k_m".to_string(),
                effort: None,
            }]
        );
    }

    #[test]
    fn colon_model_with_known_lane_profile() {
        let lanes = vec![normalize_keep_lane("openai-compatible")];
        let rules = parse_picker_keep_rules(["openai-compatible:myprofile:my-model:high"], &lanes);
        assert_eq!(
            rules,
            vec![PickerKeepRule {
                lane: Some("openaicompatible".to_string()),
                model: "myprofile:my-model".to_string(),
                effort: Some("high".to_string()),
            }]
        );
    }

    #[test]
    fn drops_empty_and_model_only_entries() {
        assert!(parse_picker_keep_rules(["", "   "], &known()).is_empty());
        // A lone effort-looking segment is not a model.
        assert!(parse_picker_keep_rules([":high"], &known()).is_empty());
    }

    #[test]
    fn matches_route_by_api_method_lane() {
        let rules = parse_picker_keep_rules(["openai-api:gpt-6-astra:high"], &[]);
        let rule = &rules[0];
        assert!(picker_keep_rule_matches_route(
            rule,
            "gpt-6-astra",
            "OpenAI",
            "openai-api",
        ));
        // The sibling lane must not match.
        assert!(!picker_keep_rule_matches_route(
            rule,
            "gpt-6-astra",
            "OpenAI",
            "openai-oauth",
        ));
        // A different model must not match.
        assert!(!picker_keep_rule_matches_route(
            rule,
            "gpt-6-luna",
            "OpenAI",
            "openai-api"
        ));
    }

    #[test]
    fn bare_model_matches_every_lane() {
        let rules = parse_picker_keep_rules(["gpt-6-luna:low"], &[]);
        let rule = &rules[0];
        assert!(picker_keep_rule_matches_route(
            rule,
            "gpt-6-luna",
            "OpenAI",
            "openai-oauth"
        ));
        assert!(picker_keep_rule_matches_route(
            rule,
            "gpt-6-luna",
            "OpenAI",
            "openai-api"
        ));
        // Lane-less rules match any lane, including ones with no keep entry.
        assert!(picker_keep_rule_matches_route(
            rule,
            "gpt-6-luna",
            "OpenAI",
            "openai-oauth-x"
        ));
        assert!(!picker_keep_rule_matches_route(
            rule,
            "gpt-6-astra",
            "OpenAI",
            "openai-oauth"
        ));
    }

    #[test]
    fn configured_lane_matches_provider_label() {
        let rules = parse_picker_keep_rules(["kimi:kimi-for-coding"], &known());
        let rule = &rules[0];
        assert!(picker_keep_rule_matches_route(
            rule,
            "kimi-for-coding",
            "Kimi",
            "openai-compatible:kimi",
        ));
        assert!(!picker_keep_rule_matches_route(
            rule,
            "kimi-for-coding",
            "GLM",
            "openai-compatible:glm",
        ));
    }
}
