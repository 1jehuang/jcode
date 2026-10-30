//! Canonical model-id normalization.
//!
//! Model ids arrive in many shapes: mixed case, with the Anthropic `[1m]`
//! long-context suffix, with dated releases (`claude-haiku-4-5-20251001`),
//! or provider-qualified (`openrouter/anthropic/claude-...`). Historically
//! each subsystem (catalog, pricing, capability lookup, auth preferences,
//! subscription matching) hand-rolled its own partial normalization, which
//! made the same model compare unequal across layers.
//!
//! This module is the single home for those operations. Subsystems may
//! still compose them differently (pricing keeps case, subscription
//! matching splits `@`), but the primitive transforms live here.

/// Anthropic long-context opt-in suffix.
pub const LONG_CONTEXT_SUFFIX: &str = "[1m]";

/// Strip the `[1m]` long-context suffix, if present.
pub fn strip_long_context_suffix(model: &str) -> &str {
    model.strip_suffix(LONG_CONTEXT_SUFFIX).unwrap_or(model)
}

/// Split a model id into its base id and whether it carried the `[1m]`
/// long-context suffix.
pub fn split_long_context(model: &str) -> (&str, bool) {
    match model.strip_suffix(LONG_CONTEXT_SUFFIX) {
        Some(base) => (base, true),
        None => (model, false),
    }
}

/// Canonical lowercase key: trimmed, ASCII-lowercased, `[1m]` stripped.
///
/// Use this whenever model ids are compared or used as map keys across
/// subsystem boundaries.
pub fn canonical(model: &str) -> String {
    let normalized = model.trim().to_ascii_lowercase();
    strip_long_context_suffix(&normalized).to_string()
}

/// Strip a trailing 8-digit `-YYYYMMDD` release date so dated ids
/// (`claude-haiku-4-5-20251001`) match bare canonical ids
/// (`claude-haiku-4-5`).
pub fn strip_date_suffix(model: &str) -> &str {
    match model.rsplit_once('-') {
        Some((head, tail)) if tail.len() == 8 && tail.bytes().all(|byte| byte.is_ascii_digit()) => {
            head
        }
        _ => model,
    }
}

/// Rewrite a dotted Claude version (`claude-opus-5.5`) to the dashed form
/// Anthropic's own catalog uses (`claude-opus-5-5`).
///
/// Only a `.` between two ASCII digits is rewritten, so other punctuation in
/// custom ids is left alone. Ids that are not `claude-*` are returned trimmed
/// but otherwise unchanged, so dotted ids from other providers (`gpt-5.5`,
/// `qwen3.6`) are never rewritten. The `[1m]` suffix and case are preserved.
pub fn claude_dashed_version(model: &str) -> String {
    let trimmed = model.trim();
    // `get` returns None on a non-char boundary instead of panicking.
    let is_claude = trimmed
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("claude-"));
    if !is_claude {
        return trimmed.to_string();
    }
    let bytes = trimmed.as_bytes();
    trimmed
        .char_indices()
        .map(|(i, c)| {
            let between_digits = c == '.'
                && i > 0
                && bytes[i - 1].is_ascii_digit()
                && bytes.get(i + 1).is_some_and(u8::is_ascii_digit);
            if between_digits { '-' } else { c }
        })
        .collect()
}

/// Final path segment of a slash-qualified id
/// (`openrouter/anthropic/claude-x` -> `claude-x`). Ids without `/` are
/// returned unchanged.
pub fn slash_base(model: &str) -> &str {
    model.rsplit('/').next().unwrap_or(model)
}

/// Case-insensitive membership test against a static known-model list,
/// ignoring the `[1m]` suffix on the probe side only (the list may contain
/// explicit `[1m]` entries which are matched exactly first).
pub fn matches_known_model(model: &str, known: &[&str]) -> bool {
    let trimmed = model.trim();
    if known
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(trimmed))
    {
        return true;
    }
    let base = strip_long_context_suffix(trimmed);
    known
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(base))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_lowercases_trims_and_strips_long_context() {
        assert_eq!(canonical("  Claude-Sonnet-4-6[1m] "), "claude-sonnet-4-6");
        assert_eq!(canonical("gpt-5.3-codex"), "gpt-5.3-codex");
    }

    #[test]
    fn split_long_context_reports_suffix() {
        assert_eq!(
            split_long_context("claude-opus-4-6[1m]"),
            ("claude-opus-4-6", true)
        );
        assert_eq!(
            split_long_context("claude-opus-4-6"),
            ("claude-opus-4-6", false)
        );
    }

    #[test]
    fn strip_date_suffix_only_strips_8_digit_dates() {
        assert_eq!(
            strip_date_suffix("claude-haiku-4-5-20251001"),
            "claude-haiku-4-5"
        );
        assert_eq!(strip_date_suffix("claude-haiku-4-5"), "claude-haiku-4-5");
        assert_eq!(strip_date_suffix("gpt-4-1106"), "gpt-4-1106");
    }

    #[test]
    fn claude_dashed_version_only_rewrites_claude_ids() {
        assert_eq!(claude_dashed_version("claude-opus-5.5"), "claude-opus-5-5");
        assert_eq!(
            claude_dashed_version(" Claude-Sonnet-5.5[1m] "),
            "Claude-Sonnet-5-5[1m]"
        );
        assert_eq!(claude_dashed_version("claude-opus-5-5"), "claude-opus-5-5");
        assert_eq!(claude_dashed_version("gpt-5.5"), "gpt-5.5");
        assert_eq!(claude_dashed_version("qwen3.6-27b"), "qwen3.6-27b");
        // Only dots between digits are version separators.
        assert_eq!(claude_dashed_version("claude-x.y-5.5"), "claude-x.y-5-5");
        assert_eq!(claude_dashed_version("claude-opus-5."), "claude-opus-5.");
    }

    #[test]
    fn claude_dashed_version_handles_non_ascii_without_panicking() {
        // Byte 7 falls inside a multi-byte character in each of these.
        assert_eq!(claude_dashed_version("éééé"), "éééé");
        assert_eq!(claude_dashed_version("abcdefé"), "abcdefé");
        assert_eq!(claude_dashed_version("claude-é-5.5"), "claude-é-5-5");
    }

    #[test]
    fn slash_base_takes_last_segment() {
        assert_eq!(
            slash_base("anthropic/claude-sonnet-4-6"),
            "claude-sonnet-4-6"
        );
        assert_eq!(slash_base("claude-sonnet-4-6"), "claude-sonnet-4-6");
    }

    #[test]
    fn matches_known_model_is_case_insensitive_and_1m_tolerant() {
        let known = ["gpt-5.3-codex", "claude-opus-4-6[1m]"];
        assert!(matches_known_model("GPT-5.3-Codex", &known));
        assert!(matches_known_model("claude-opus-4-6[1m]", &known));
        // probe with [1m] matches the bare known id too
        assert!(matches_known_model("gpt-5.3-codex[1m]", &known));
        assert!(!matches_known_model("gpt-4o", &known));
    }
}
