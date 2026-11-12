//! OpenAI provider shared helpers (compatibility shim).
//!
//! The OpenAI provider *runtime* (`OpenAIProvider`: Codex OAuth + API key,
//! Responses API over SSE and persistent WebSocket) now lives in the
//! downstream `jcode-provider-openai-runtime` crate so provider edits do not
//! rebuild the base -> app-core -> tui spine. The binary's composition root
//! registers it via [`crate::provider::external`].
//!
//! Base keeps the pieces its own catalog/routing code shares with the runtime:
//! - the API base-URL resolution used by catalog fetches, and
//! - the extended prompt-cache-retention model predicate used by
//!   cache-TTL routing.

pub use jcode_provider_core::CredentialMode as OpenAICredentialMode;

const OPENAI_API_BASE: &str = "https://api.openai.com/v1";

/// Resolve the OpenAI Responses API base URL for **API-key** mode.
///
/// Defaults to `https://api.openai.com/v1`, but honors a user override so
/// the native `openai-api` provider can target a local/proxied Responses
/// API endpoint (issue #343). Checked in order:
/// `JCODE_OPENAI_API_BASE`, `OPENAI_BASE_URL`, `OPENAI_API_BASE`.
///
/// The override must be an absolute `http(s)://` URL; anything else is
/// logged and ignored so a malformed value never silently breaks requests.
/// A `/responses` suffix is not expected here (it is appended by callers),
/// so a trailing `/responses` is trimmed to avoid `.../responses/responses`.
pub fn resolve_api_base() -> String {
    const OVERRIDE_VARS: [&str; 3] = [
        "JCODE_OPENAI_API_BASE",
        "OPENAI_BASE_URL",
        "OPENAI_API_BASE",
    ];
    for var in OVERRIDE_VARS {
        let Ok(raw) = std::env::var(var) else {
            continue;
        };
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
            crate::logging::warn(&format!(
                "Ignoring invalid {} '{}'; expected an absolute http(s):// URL",
                var, trimmed
            ));
            continue;
        }
        let normalized = trimmed
            .trim_end_matches('/')
            .trim_end_matches("/responses")
            .trim_end_matches('/');
        if normalized.is_empty() {
            crate::logging::warn(&format!(
                "Ignoring invalid {} '{}'; URL has no host/path",
                var, trimmed
            ));
            continue;
        }
        crate::logging::info(&format!(
            "OpenAI Responses API base overridden to '{}' via {}",
            normalized, var
        ));
        return normalized.to_string();
    }
    // Fall back to the active Codex Responses provider base URL from
    // `~/.codex/config.toml` so OpenAI-compatible API-key traffic honors a
    // gateway configured there, before defaulting to api.openai.com (#374).
    if let Some(base) = codex_config_responses_base() {
        crate::logging::info(&format!(
            "OpenAI Responses API base resolved to '{}' from ~/.codex/config.toml",
            base
        ));
        return base;
    }
    OPENAI_API_BASE.to_string()
}

/// Read the active Codex model_provider's `base_url` from
/// `~/.codex/config.toml` when it serves the Responses wire API.
///
/// Codex config shape:
/// ```toml
/// model_provider = "mygw"
/// [model_providers.mygw]
/// base_url = "https://gateway.example/v1"
/// wire_api = "responses"   # only "responses" is honored here
/// ```
/// Returns `None` when the file/keys are missing, the URL is not absolute
/// http(s), or the provider's `wire_api` is not `responses`.
fn codex_config_responses_base() -> Option<String> {
    let path = crate::storage::user_home_path(".codex/config.toml").ok()?;
    let contents = std::fs::read_to_string(&path).ok()?;
    let value: toml::Value = contents.parse().ok()?;

    let provider_name = value.get("model_provider")?.as_str()?.trim();
    if provider_name.is_empty() {
        return None;
    }
    let provider = value
        .get("model_providers")?
        .as_table()?
        .get(provider_name)?
        .as_table()?;

    // Only honor providers that speak the Responses wire API. When the key
    // is absent, Codex defaults to the Responses API for OpenAI-style
    // providers, so treat "missing" as eligible.
    if let Some(wire_api) = provider.get("wire_api").and_then(|v| v.as_str())
        && !wire_api.trim().eq_ignore_ascii_case("responses")
    {
        return None;
    }

    let base = provider.get("base_url")?.as_str()?.trim();
    if !(base.starts_with("http://") || base.starts_with("https://")) {
        crate::logging::warn(&format!(
            "Ignoring ~/.codex/config.toml base_url '{}' for provider '{}'; expected an absolute http(s):// URL",
            base, provider_name
        ));
        return None;
    }
    let normalized = base
        .trim_end_matches('/')
        .trim_end_matches("/responses")
        .trim_end_matches('/');
    if normalized.is_empty() {
        return None;
    }
    Some(normalized.to_string())
}

/// Whether `model_id` supports the legacy `prompt_cache_retention: "24h"`.
///
/// Keep this list tied to the documented extended-retention models, not a
/// broad prefix match (for example, GPT-4.1 mini is not GPT-4.1).
/// https://developers.openai.com/api/docs/guides/prompt-caching#summary-of-model-differences
pub fn supports_extended_prompt_cache_retention(model_id: &str) -> bool {
    let model = model_id.trim().to_ascii_lowercase();
    let model = match model.as_str() {
        "gpt-5.4-1m" => "gpt-5.4",
        model => model,
    };
    [
        "gpt-5.5",
        "gpt-5.5-pro",
        "gpt-5.4",
        "gpt-5.2",
        "gpt-5.1-codex-max",
        "gpt-5.1",
        "gpt-5.1-codex",
        "gpt-5.1-codex-mini",
        "gpt-5.1-chat-latest",
        "gpt-5",
        "gpt-5-codex",
        "gpt-4.1",
    ]
    .iter()
    .any(|base| {
        model == *base
            || model.strip_prefix(base).is_some_and(|suffix| {
                // Dated API snapshots inherit the base model's policy.
                let bytes = suffix.as_bytes();
                bytes.len() == 11
                    && bytes[0] == b'-'
                    && bytes[5] == b'-'
                    && bytes[8] == b'-'
                    && bytes
                        .iter()
                        .enumerate()
                        .all(|(i, c)| matches!(i, 0 | 5 | 8) || c.is_ascii_digit())
            })
    })
}

/// GPT-5.6 and later use `prompt_cache_options.ttl` (default and only supported
/// value: `30m`), not the older `prompt_cache_retention` request field.
pub fn uses_prompt_cache_options(model_id: &str) -> bool {
    let model = model_id.trim().to_ascii_lowercase();
    let Some(version) = model.strip_prefix("gpt-").and_then(|v| v.split('-').next()) else {
        return false;
    };
    let (major, minor) = version.split_once('.').unwrap_or((version, "0"));
    match (major.parse::<u32>(), minor.parse::<u32>()) {
        (Ok(major), Ok(minor)) => major > 5 || (major == 5 && minor >= 6),
        _ => false,
    }
}

/// Normalize the supported legacy override values, shared by TTL reporting and
/// the request builder. Model-specific override compatibility is checked by the API.
pub fn normalize_prompt_cache_retention(value: &str) -> Option<&str> {
    match value.trim() {
        "in_memory" => Some("in_memory"),
        "24h" => Some("24h"),
        _ => None,
    }
}

pub fn prompt_cache_retention_from_env() -> Option<String> {
    let raw = std::env::var("JCODE_OPENAI_PROMPT_CACHE_RETENTION").ok()?;
    normalize_prompt_cache_retention(&raw).map(str::to_owned)
}

/// Retention actually sent on API-key requests. OAuth does not use this policy.
pub fn effective_prompt_cache_retention<'a>(
    model_id: &str,
    configured: Option<&'a str>,
) -> Option<&'a str> {
    if uses_prompt_cache_options(model_id) {
        // Omit the obsolete field and rely on the documented 30m default.
        return None;
    }
    configured
        .and_then(normalize_prompt_cache_retention)
        .or_else(|| supports_extended_prompt_cache_retention(model_id).then_some("24h"))
}

/// API cache retention estimate, not a guaranteed cache hit or expiry time.
/// Legacy extended retention is typically 30m (24h is only its upper bound),
/// in-memory is typically 5-10m, and 30m is the minimum lifetime for GPT-5.6+.
/// Routing/eviction can still prevent cache hits.
pub fn prompt_cache_ttl_for_model(model_id: Option<&str>, configured: Option<&str>) -> u64 {
    let model = model_id.unwrap_or_default();
    if uses_prompt_cache_options(model)
        || effective_prompt_cache_retention(model, configured) == Some("24h")
    {
        30 * 60
    } else {
        300
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    #[test]
    fn extended_cache_retention_matches_documented_models_and_snapshots() {
        for model in [
            "gpt-5.5",
            "gpt-5.5-pro",
            "gpt-5.4",
            "gpt-5.4-1m",
            "gpt-5.2",
            "gpt-5.1-codex-max",
            "gpt-5.1",
            "gpt-5.1-codex",
            "gpt-5.1-codex-mini",
            "gpt-5.1-chat-latest",
            "gpt-5",
            "gpt-5-codex",
            "gpt-4.1",
            "gpt-4.1-2025-04-14",
            " GPT-5.4 ",
        ] {
            assert!(supports_extended_prompt_cache_retention(model), "{model}");
        }
        for model in [
            "gpt-5.6",
            "gpt-6-astra",
            "gpt-4o",
            "gpt-4.1-mini",
            "gpt-4.1-nano",
            "gpt-5-mini",
            "gpt-5.4-mini",
            "gpt-5.50",
            "gpt-5.1-unknown",
            "unknown",
        ] {
            assert!(!supports_extended_prompt_cache_retention(model), "{model}");
        }
    }

    #[test]
    fn cache_retention_policy_and_ttl_share_override_semantics() {
        for (model, configured, retention, ttl) in [
            ("gpt-5.4", None, Some("24h"), 1800),
            ("gpt-5.4", Some(" in_memory "), Some("in_memory"), 300),
            ("gpt-5.4", Some("invalid"), Some("24h"), 1800),
            ("gpt-4o", None, None, 300),
            ("gpt-4o", Some("24h"), Some("24h"), 1800),
            ("gpt-5.6-sol", None, None, 1800),
            ("gpt-5.6-sol", Some("24h"), None, 1800),
            ("gpt-5.6-sol", Some("in_memory"), None, 1800),
            ("gpt-6-astra", None, None, 1800),
        ] {
            assert_eq!(
                effective_prompt_cache_retention(model, configured),
                retention,
                "{model}"
            );
            assert_eq!(
                prompt_cache_ttl_for_model(Some(model), configured),
                ttl,
                "{model}"
            );
        }
        for invalid in ["", " ", "24H", "1h"] {
            assert_eq!(normalize_prompt_cache_retention(invalid), None);
        }
    }

    #[test]
    fn cache_ttl_provider_routes_and_environment_override() {
        let _guard = crate::storage::lock_test_env();
        let key = "JCODE_OPENAI_PROMPT_CACHE_RETENTION";
        let saved = std::env::var_os(key);
        // Restore even on assertion failure so other provider tests are isolated.
        struct RestoreEnv(&'static str, Option<std::ffi::OsString>);
        impl Drop for RestoreEnv {
            fn drop(&mut self) {
                match &self.1 {
                    Some(value) => crate::env::set_var(self.0, value),
                    None => crate::env::remove_var(self.0),
                }
            }
        }
        let _restore = RestoreEnv(key, saved);
        for (value, expected) in [
            (None, 1800),
            (Some(" in_memory "), 300),
            (Some("24h"), 1800),
            (Some("bogus"), 1800),
        ] {
            match value {
                Some(value) => crate::env::set_var(key, value),
                None => crate::env::remove_var(key),
            }
            for provider in [
                "openai-api",
                "openai-api-key",
                " OpenAI-API ",
                "openai-key",
                "openai-apikey",
                "openai-platform",
                "platform-openai",
                "openai-api:",
            ] {
                assert_eq!(
                    crate::provider::cache_ttl_for_provider_model(provider, Some("gpt-5.4")),
                    Some(expected)
                );
                assert!(crate::provider::cache_ttl_is_estimate(provider));
            }
            for provider in ["openai", "OpenAI", "openai-oauth"] {
                assert_eq!(
                    crate::provider::cache_ttl_for_provider_model(provider, Some("gpt-5.4")),
                    None
                );
            }
        }
        assert!(!crate::provider::cache_ttl_is_estimate("anthropic"));
        for provider in ["openrouter", "jcode subscription", " Gemini "] {
            assert!(crate::provider::cache_ttl_is_estimate(provider));
        }
    }
}
