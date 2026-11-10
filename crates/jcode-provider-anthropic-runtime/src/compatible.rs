//! Explicit direct-connection construction for Anthropic-wire endpoints.
//!
//! Used by provider orchestrators that need a Messages transport at a
//! non-Anthropic base URL without mutating process-global environment.

use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::sync::Arc;

use super::{AnthropicCredentialMode, AnthropicProvider, DirectTransportConfig, RwLock};

pub(super) fn direct_api_url() -> String {
    let base = std::env::var("JCODE_ANTHROPIC_API_BASE")
        .ok()
        .or_else(|| std::env::var("ANTHROPIC_BASE_URL").ok())
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty());
    match base {
        Some(base) if base.ends_with("/messages") => base,
        Some(base) => format!("{base}/messages"),
        None => super::API_URL.to_string(),
    }
}

pub(super) fn configured_direct_headers() -> Result<HeaderMap> {
    let Some(raw) = std::env::var("JCODE_ANTHROPIC_HEADERS")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(HeaderMap::new());
    };
    let headers: std::collections::BTreeMap<String, String> = serde_json::from_str(&raw)
        .context("JCODE_ANTHROPIC_HEADERS must be a JSON object of string values")?;
    let mut result = HeaderMap::new();
    for (name, value) in headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .with_context(|| format!("invalid Anthropic-compatible header name '{name}'"))?;
        let value = HeaderValue::from_str(&value)
            .with_context(|| format!("invalid value for Anthropic-compatible header '{name}'"))?;
        result.insert(name, value);
    }
    Ok(result)
}

impl AnthropicProvider {
    /// Construct an Anthropic Messages runtime for a compatible gateway that
    /// accepts `Authorization: Bearer <key>`.
    ///
    /// The caller owns the key lifecycle. No OAuth credentials, Claude Code
    /// identity blocks, or process-global env are consulted.
    pub fn new_command_code(api_base: &str, api_key: &str, model: &str) -> Result<Self> {
        let base = api_base.trim().trim_end_matches('/');
        if base.is_empty() {
            anyhow::bail!("Command Code Messages base URL is empty");
        }
        let mut provider = Self::new();
        provider.direct_transport = DirectTransportConfig {
            api_url: format!("{base}/messages"),
            #[cfg(test)]
            oauth_api_url: None,
            headers: Ok(HeaderMap::new()),
            auth_mode: "bearer".to_string(),
            auth_header: "authorization".to_string(),
        };
        provider.profile_api_key = Some(Ok(api_key.to_string()));
        provider.profile_models = Some(vec![model.to_string()]);
        provider.credential_mode = Arc::new(RwLock::new(AnthropicCredentialMode::ApiKey));
        provider.model = Arc::new(std::sync::RwLock::new(model.to_string()));
        provider.max_tokens_override = Some(32_000);
        Ok(provider)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_code_constructor_uses_bearer_messages_route() {
        let provider = AnthropicProvider::new_command_code(
            "https://api.commandcode.ai/provider/v1",
            "test-key",
            "claude-sonnet-5",
        )
        .expect("constructor");
        assert_eq!(
            provider.direct_transport.api_url,
            "https://api.commandcode.ai/provider/v1/messages"
        );
        assert_eq!(provider.direct_transport.auth_mode, "bearer");
        assert_eq!(
            provider.profile_models.as_deref(),
            Some(&["claude-sonnet-5".to_string()][..])
        );
        assert_eq!(
            provider
                .profile_api_key
                .as_ref()
                .and_then(|key| key.as_deref().ok()),
            Some("test-key")
        );
        assert_eq!(provider.max_tokens_override, Some(32_000));
    }
}
