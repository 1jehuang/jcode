//! Read the active provider's base URL from `~/.codex/config.toml`.
//!
//! Issue #84 (upstream PR #95): when a user has Codex configured to talk to a
//! self-hosted OpenAI Responses-compatible gateway via:
//!
//! ```toml
//! model_provider = "my-gateway"
//!
//! [model_providers.my-gateway]
//! wire_api               = "responses"
//! requires_openai_auth   = true
//! base_url               = "https://gateway.example.com/v1"
//! ```
//!
//! ...jcode could trust the Codex auth.json (refresh token, ID token) but
//! still send requests to the hard-coded `https://api.openai.com/v1`. The
//! result was a confusing 401 "invalid API key" against the real OpenAI
//! endpoint while the gateway never saw the call.
//!
//! This module exposes a single helper, `active_responses_base_url()`, which
//! returns `Some(url)` only when ALL three conditions hold:
//!
//!   1. `~/.codex/config.toml` exists and is parseable.
//!   2. `model_provider = "<name>"` is set at the top level.
//!   3. `[model_providers.<name>]` has `wire_api = "responses"` AND
//!      `requires_openai_auth = true` AND a non-empty `base_url`.
//!
//! The helper returns `None` (silently) for any other shape so jcode falls
//! back to the existing `OPENAI_API_BASE` default. We do not treat
//! arbitrary chat-completions endpoints as Responses endpoints — that would
//! produce mismatched wire protocol errors.

use anyhow::Result;
use std::path::PathBuf;

/// Parse `~/.codex/config.toml` and return the active provider's `base_url`
/// when it requires OpenAI auth and uses the `responses` wire API.
///
/// Returns `None` (without error) when:
///   - the file does not exist,
///   - the file does not parse as TOML,
///   - the active provider is missing or does not match the criteria above.
///
/// Errors are only surfaced when an explicit `Err(_)` would help debugging;
/// callers should generally accept `None` as "use default OpenAI base URL".
pub fn active_responses_base_url() -> Option<String> {
    let path = codex_config_path().ok()?;
    let content = std::fs::read_to_string(&path).ok()?;
    base_url_from_toml(&content)
}

/// Resolve the on-disk path to the user's Codex config (`~/.codex/config.toml`).
///
/// Honours `JCODE_HOME`-sandboxed test runs via `crate::storage::user_home_path`.
pub fn codex_config_path() -> Result<PathBuf> {
    crate::storage::user_home_path(".codex/config.toml")
}

/// Pure parse function exposed for testing — no filesystem access.
pub fn base_url_from_toml(content: &str) -> Option<String> {
    let table: toml::Table = content.parse().ok()?;
    let active = table.get("model_provider")?.as_str()?.trim().to_string();
    if active.is_empty() {
        return None;
    }

    let providers = table.get("model_providers")?.as_table()?;
    let provider = providers.get(&active)?.as_table()?;

    let wire_api = provider.get("wire_api")?.as_str()?;
    if wire_api != "responses" {
        return None;
    }

    let requires_openai_auth = provider
        .get("requires_openai_auth")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !requires_openai_auth {
        return None;
    }

    let base_url = provider.get("base_url")?.as_str()?.trim();
    if base_url.is_empty() {
        return None;
    }
    Some(base_url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_some_for_valid_responses_provider() {
        let toml = r#"
model_provider = "internal"

[model_providers.internal]
wire_api = "responses"
requires_openai_auth = true
base_url = "https://gateway.example.com/v1"
"#;
        assert_eq!(
            base_url_from_toml(toml),
            Some("https://gateway.example.com/v1".to_string())
        );
    }

    #[test]
    fn returns_none_when_wire_api_is_not_responses() {
        let toml = r#"
model_provider = "internal"

[model_providers.internal]
wire_api = "chat"
requires_openai_auth = true
base_url = "https://gateway.example.com/v1"
"#;
        assert!(base_url_from_toml(toml).is_none());
    }

    #[test]
    fn returns_none_when_auth_not_required() {
        let toml = r#"
model_provider = "internal"

[model_providers.internal]
wire_api = "responses"
requires_openai_auth = false
base_url = "https://gateway.example.com/v1"
"#;
        assert!(base_url_from_toml(toml).is_none());
    }

    #[test]
    fn returns_none_when_active_provider_missing() {
        let toml = r#"
[model_providers.internal]
wire_api = "responses"
requires_openai_auth = true
base_url = "https://gateway.example.com/v1"
"#;
        assert!(base_url_from_toml(toml).is_none());
    }

    #[test]
    fn returns_none_when_provider_not_defined() {
        let toml = r#"
model_provider = "missing"

[model_providers.internal]
wire_api = "responses"
requires_openai_auth = true
base_url = "https://gateway.example.com/v1"
"#;
        assert!(base_url_from_toml(toml).is_none());
    }

    #[test]
    fn returns_none_when_base_url_blank_or_missing() {
        let blank = r#"
model_provider = "x"

[model_providers.x]
wire_api = "responses"
requires_openai_auth = true
base_url = "  "
"#;
        assert!(base_url_from_toml(blank).is_none());

        let missing = r#"
model_provider = "x"

[model_providers.x]
wire_api = "responses"
requires_openai_auth = true
"#;
        assert!(base_url_from_toml(missing).is_none());
    }

    #[test]
    fn returns_none_for_garbage_toml() {
        assert!(base_url_from_toml("not =, valid] toml").is_none());
        assert!(base_url_from_toml("").is_none());
    }

    #[test]
    fn defaults_requires_openai_auth_false_when_unspecified() {
        // Codex defaults requires_openai_auth=false, so the absence of the
        // field should NOT trigger an override.
        let toml = r#"
model_provider = "x"

[model_providers.x]
wire_api = "responses"
base_url = "https://gateway.example.com/v1"
"#;
        assert!(base_url_from_toml(toml).is_none());
    }
}
