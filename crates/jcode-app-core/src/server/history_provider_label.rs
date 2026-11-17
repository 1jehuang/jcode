//! Profile-aware provider labels for history payloads.

use super::*;

pub(super) fn history_provider_name_from_session(
    session: &crate::session::Session,
) -> Option<String> {
    let key = session.provider_key.as_deref()?.trim();
    if key.is_empty() {
        return None;
    }

    let label = match key.to_ascii_lowercase().as_str() {
        "openai" => "OpenAI".to_string(),
        "claude" | "anthropic" => "Anthropic".to_string(),
        "openrouter" => "OpenRouter".to_string(),
        "copilot" => "GitHub Copilot".to_string(),
        "cursor" => "Cursor".to_string(),
        "gemini" => "Gemini".to_string(),
        "bedrock" => "Bedrock".to_string(),
        "antigravity" => "Antigravity".to_string(),
        "jcode" => "Jcode".to_string(),
        // A direct OpenAI-compatible profile is persisted either bare
        // (`deepseek`, the session vocabulary) or as its source key
        // (`openai-compatible:deepseek`). Both name the same profile, so report
        // its display name like every other arm here; a raw key would otherwise
        // reach the header, and `source_key_for_provider_label` maps the
        // prefixed shape away from `openai:api-key` only by accident (#1286).
        other => {
            // Import-source codes are stored in the same field but are not
            // profile routes; `opencode` in particular collides with the
            // `OpenCode Zen` profile id, so it must stay verbatim.
            const IMPORT_SOURCE_CODES: &[&str] = &["opencode", "claude-code", "openai-codex"];
            if IMPORT_SOURCE_CODES.contains(&other) {
                other.to_string()
            } else {
                let profile_id = other.strip_prefix("openai-compatible:").unwrap_or(other);
                crate::provider_catalog::openai_compatible_profile_by_id(profile_id)
                    .map(|profile| profile.display_name.to_string())
                    .unwrap_or_else(|| other.to_string())
            }
        }
    };

    Some(label)
}

/// The provider name a `History` payload reports.
///
/// The persisted session key wins when there is one. Otherwise ask for the
/// profile-aware label: `Provider::name()` is the stable machine id for the
/// provider *class* (`OpenRouter` for the multiplexing slot, `openrouter` for
/// a concrete runtime instance), and that slot also serves every direct
/// OpenAI-compatible profile, so it must not be shown as this session's
/// provider (issue #1286).
pub(super) fn history_provider_name(
    session: &crate::session::Session,
    provider: &dyn Provider,
) -> Option<String> {
    history_provider_name_from_session(session).or_else(|| Some(provider.display_name()))
}
