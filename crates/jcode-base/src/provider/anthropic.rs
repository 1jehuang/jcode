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
use std::sync::atomic::{AtomicBool, Ordering};
use uuid::Uuid;

pub use jcode_provider_core::CredentialMode as AnthropicCredentialMode;
use jcode_provider_core::{
    ANTHROPIC_OAUTH_BETA_HEADERS, anthropic_effectively_1m,
    anthropic_stainless_arch as stainless_arch, anthropic_stainless_os as stainless_os,
};

static CACHE_TTL_1H: AtomicBool = AtomicBool::new(true);

/// Enable or disable the 1-hour cache TTL (default: 1-hour)
pub fn set_cache_ttl_1h(enabled: bool) {
    CACHE_TTL_1H.store(enabled, Ordering::Relaxed);
}

/// Check if 1-hour cache TTL is enabled
pub fn is_cache_ttl_1h() -> bool {
    CACHE_TTL_1H.load(Ordering::Relaxed)
}

/// User-Agent for OAuth requests, matching the official Claude Code CLI.
pub const CLAUDE_CLI_USER_AGENT: &str = "claude-cli/2.1.123 (external, sdk-cli)";

pub const OAUTH_BETA_HEADERS: &str = ANTHROPIC_OAUTH_BETA_HEADERS;

/// Whether a model id effectively runs with the 1M-token context beta.
pub fn effectively_1m(model: &str) -> bool {
    anthropic_effectively_1m(model)
}

pub fn new_oauth_request_id() -> String {
    Uuid::new_v4().to_string()
}

/// Attach the OAuth attribution headers the official Claude CLI sends.
/// Shared by the runtime crate's request path and base's usage probes.
pub fn apply_oauth_attribution_headers(
    req: reqwest::RequestBuilder,
    session_id: &str,
) -> reqwest::RequestBuilder {
    req.header("x-client-request-id", new_oauth_request_id())
        .header("x-app", "cli")
        .header("X-Claude-Code-Session-Id", session_id)
        .header("X-Stainless-Arch", stainless_arch())
        .header("X-Stainless-Lang", "js")
        .header("X-Stainless-OS", stainless_os())
        .header("X-Stainless-Package-Version", "0.81.0")
        .header("X-Stainless-Retry-Count", "0")
        .header("X-Stainless-Runtime", "node")
        .header("X-Stainless-Runtime-Version", "v24.3.0")
        .header("X-Stainless-Timeout", "600")
        .header("anthropic-dangerous-direct-browser-access", "true")
}

/// Available models
pub const AVAILABLE_MODELS: &[&str] = &[
    "claude-fable-5",
    "claude-opus-5",
    "claude-opus-4-8",
    "claude-opus-4-6",
    "claude-opus-4-6[1m]",
    "claude-sonnet-5",
    "claude-sonnet-4-6",
    "claude-sonnet-4-6[1m]",
    "claude-haiku-4-5",
    "claude-opus-4-5",
    "claude-sonnet-4-5",
    "claude-sonnet-4-20250514",
];

pub fn load_anthropic_api_key() -> Result<String> {
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
