//! Anthropic provider shared helpers (compatibility shim).
//!
//! The direct Anthropic Messages API *runtime* (`AnthropicProvider`) now lives
//! in the downstream `jcode-provider-anthropic-runtime` crate so provider
//! edits do not rebuild the base -> app-core -> tui spine. The binary's
//! composition root registers it via [`crate::provider::external`].
//!
//! Base keeps the pieces its own auth/usage/sidecar code (and the runtime
//! crate) share:
//! - the OAuth attribution headers + Claude CLI user agent used for
//!   subscription API calls,
//! - API-key resolution (`load_anthropic_api_key`, `has_anthropic_api_key`),
//! - the process-wide cache-TTL toggle, and
//! - the static model list.

use anyhow::{Context, Result};
use std::sync::atomic::{AtomicU8, Ordering};
use uuid::Uuid;

pub use jcode_provider_core::CredentialMode as AnthropicCredentialMode;
use jcode_provider_core::{ANTHROPIC_OAUTH_BETA_HEADERS, anthropic_effectively_1m};

// 0 follows persisted configuration, 1/2 are explicit process-local overrides.
static CACHE_TTL_1H: AtomicU8 = AtomicU8::new(0);

/// Override cache TTL for this process. UI preferences should use Config instead.
pub fn set_cache_ttl_1h(enabled: bool) {
    CACHE_TTL_1H.store(if enabled { 2 } else { 1 }, Ordering::Relaxed);
}

/// Check if 1-hour cache TTL is enabled
pub fn is_cache_ttl_1h() -> bool {
    match CACHE_TTL_1H.load(Ordering::Relaxed) {
        1 => false,
        2 => true,
        _ => crate::config::config().provider.anthropic_cache_ttl_1h,
    }
}

/// User-Agent for Claude subscription (OAuth) requests.
///
/// This identifies Jcode honestly. It used to mimic the official Claude Code
/// CLI (`claude-cli/x.y.z (external, sdk-cli)`) along with `x-app: cli`,
/// `X-Stainless-*`, `X-Claude-Code-Session-Id` and
/// `anthropic-dangerous-direct-browser-access`. Live probes on 2026-11-05
/// showed none of those are needed: Opus and Sonnet accept OAuth requests with
/// a plain Jcode User-Agent, bill them to the same subscription bucket
/// (identical `anthropic-ratelimit-unified-*` headers), and cache prompts
/// normally. The only required piece is the billing attribution system block,
/// see `jcode_provider_anthropic::OAUTH_BILLING_HEADER` and
/// docs/ANTHROPIC_OAUTH_IDENTITY_PROBE_2026_11_05.md.
///
/// The name is kept so existing call sites don't churn.
pub const CLAUDE_CLI_USER_AGENT: &str = concat!("jcode/", env!("CARGO_PKG_VERSION"));

pub const OAUTH_BETA_HEADERS: &str = ANTHROPIC_OAUTH_BETA_HEADERS;

/// Whether a model id effectively runs with the 1M-token context beta.
pub fn effectively_1m(model: &str) -> bool {
    anthropic_effectively_1m(model)
}

pub fn new_oauth_request_id() -> String {
    Uuid::new_v4().to_string()
}

/// Attach per-request headers for Claude subscription (OAuth) calls.
///
/// Only a request id is sent. The Claude Code CLI impersonation headers that
/// used to live here were not needed, see [`CLAUDE_CLI_USER_AGENT`].
/// `_session_id` is kept for call-site compatibility.
pub fn apply_oauth_attribution_headers(
    req: reqwest::RequestBuilder,
    _session_id: &str,
) -> reqwest::RequestBuilder {
    req.header("x-client-request-id", new_oauth_request_id())
}

/// Available models
pub const AVAILABLE_MODELS: &[&str] = jcode_provider_core::ALL_CLAUDE_MODELS;

pub fn load_anthropic_api_key() -> Result<String> {
    if std::env::var("JCODE_ANTHROPIC_AUTH")
        .ok()
        .is_some_and(|value| value.eq_ignore_ascii_case("none"))
    {
        return Ok(String::new());
    }
    if let Ok(env_name) = std::env::var("JCODE_ANTHROPIC_API_KEY_NAME") {
        let env_name = env_name.trim();
        if !env_name.is_empty() {
            if let Ok(value) = std::env::var(env_name)
                && !value.trim().is_empty()
            {
                return Ok(value);
            }
            if let Ok(env_file) = std::env::var("JCODE_ANTHROPIC_ENV_FILE")
                && let Some(value) = crate::provider_catalog::load_env_value_from_config_file(
                    env_name,
                    env_file.trim(),
                )
                && !value.trim().is_empty()
            {
                return Ok(value);
            }
            anyhow::bail!(
                "Anthropic-compatible profile credential '{}' is not configured",
                env_name
            );
        }
    }
    if let Ok(value) = std::env::var("ANTHROPIC_AUTH_TOKEN")
        && !value.trim().is_empty()
    {
        return Ok(value);
    }
    let key = crate::provider_catalog::load_api_key_from_env_or_config(
        "ANTHROPIC_API_KEY",
        "anthropic.env",
    )
    .context("No Anthropic API key found")?;
    if std::env::var("JCODE_LOG_SERVICE_TIER").is_ok() {
        let prefix: String = key.chars().take(14).collect();
        eprintln!(
            "[anthropic] resolved API key prefix={prefix}... (len={})",
            key.len()
        );
    }
    Ok(key)
}

pub fn has_anthropic_api_key() -> bool {
    load_anthropic_api_key().is_ok()
}
