//! Model-id resolution for `AnthropicProvider::set_model`.
//!
//! Kept pure (allowlists are passed in) so it can be unit-tested without
//! auth or catalog state.

use jcode_provider_core::model_id::claude_dashed_version;
use jcode_provider_core::{
    AnthropicContextMode, anthropic_context_mode, anthropic_is_1m_model, anthropic_strip_1m_suffix,
};

/// Resolve a requested model to the id that should be stored and sent.
///
/// 1. A configured profile id matching exactly wins unchanged, so named
///    Anthropic-compatible profiles keep their own spelling
///    (`claude-private-5.5`).
/// 2. Otherwise dotted Claude versions are dashed (`claude-opus-5.5` ->
///    `claude-opus-5-5`) and a redundant `[1m]` suffix on native-1M models is
///    dropped, then the result must be in the profile list or `known`.
///
/// Returns `Err(candidate)` with the normalized id when nothing allows it.
pub(crate) fn resolve(
    requested: &str,
    profile_models: Option<&[String]>,
    known: impl FnOnce() -> Vec<String>,
) -> Result<String, String> {
    let profile = profile_models.unwrap_or_default();
    let trimmed = requested.trim();
    if profile.iter().any(|configured| configured == trimmed) {
        return Ok(trimmed.to_string());
    }
    let dashed = claude_dashed_version(trimmed);
    let candidate = if anthropic_is_1m_model(&dashed)
        && matches!(
            anthropic_context_mode(&dashed),
            AnthropicContextMode::Native1M
        ) {
        anthropic_strip_1m_suffix(&dashed).to_string()
    } else {
        dashed
    };
    if profile.contains(&candidate) || known().contains(&candidate) {
        Ok(candidate)
    } else {
        Err(candidate)
    }
}

#[cfg(test)]
mod tests {
    use super::resolve;

    fn known() -> Vec<String> {
        vec!["claude-opus-5-5".into(), "claude-sonnet-4-6".into()]
    }

    #[test]
    fn dotted_official_id_resolves_to_dashed_catalog_id() {
        assert_eq!(
            resolve("claude-opus-5.5", None, known).as_deref(),
            Ok("claude-opus-5-5")
        );
        assert_eq!(
            resolve(" claude-opus-5-5 ", None, known).as_deref(),
            Ok("claude-opus-5-5")
        );
    }

    #[test]
    fn native_1m_suffix_is_dropped() {
        assert_eq!(
            resolve("claude-opus-5.5[1m]", None, known).as_deref(),
            Ok("claude-opus-5-5")
        );
    }

    #[test]
    fn configured_profile_id_keeps_its_exact_spelling() {
        let profile = vec!["claude-private-5.5".to_string()];
        assert_eq!(
            resolve("claude-private-5.5", Some(&profile), Vec::new).as_deref(),
            Ok("claude-private-5.5")
        );
    }

    #[test]
    fn non_ascii_ids_do_not_panic() {
        let profile = vec!["abcdefé".to_string()];
        assert_eq!(
            resolve("abcdefé", Some(&profile), Vec::new).as_deref(),
            Ok("abcdefé")
        );
        assert_eq!(resolve("éééé", None, known), Err("éééé".to_string()));
    }

    #[test]
    fn unknown_model_is_rejected_with_normalized_id() {
        assert_eq!(
            resolve("claude-opus-6.1", None, known),
            Err("claude-opus-6-1".to_string())
        );
    }
}
