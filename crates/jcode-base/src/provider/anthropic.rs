//! Direct Anthropic API provider
//!
//! Uses the Anthropic Messages API directly without the Python SDK.
//! This provides better control and eliminates the Python dependency.

use super::{EventStream, NativeToolResultSender, Provider};
use crate::auth;
use crate::auth::oauth;
#[cfg(test)]
use crate::message::{ContentBlock, Role};
use crate::message::{Message, StreamEvent, ToolDefinition};
use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
#[cfg(test)]
use jcode_provider_anthropic::{ApiContentBlock, ToolResultContent, ToolResultContentBlock};
use jcode_provider_anthropic::{
    ApiMessage, ApiMetadata, ApiOutputConfig, ApiRequest, ApiSystem, ApiThinking, ApiTool,
};
use jcode_provider_core::{
    ANTHROPIC_OAUTH_BETA_HEADERS, anthropic_effectively_1m, anthropic_is_1m_model as is_1m_model,
    anthropic_map_tool_name_from_oauth as map_tool_name_from_oauth, anthropic_oauth_beta_headers,
    anthropic_stainless_arch as stainless_arch, anthropic_stainless_os as stainless_os,
    anthropic_strip_1m_suffix as strip_1m_suffix,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{RwLock, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

static CACHE_TTL_1H: AtomicBool = AtomicBool::new(true);

/// Enable or disable the 1-hour cache TTL (default: 1-hour)
pub fn set_cache_ttl_1h(enabled: bool) {
    CACHE_TTL_1H.store(enabled, Ordering::Relaxed);
}

/// Check if 1-hour cache TTL is enabled
pub fn is_cache_ttl_1h() -> bool {
    CACHE_TTL_1H.load(Ordering::Relaxed)
}

/// Anthropic Messages API endpoint
const API_URL: &str = "https://api.anthropic.com/v1/messages";

/// OAuth endpoint (with beta=true query param)
const API_URL_OAUTH: &str = "https://api.anthropic.com/v1/messages?beta=true";

/// User-Agent for OAuth requests, matching the official Claude Code CLI.
pub(crate) const CLAUDE_CLI_USER_AGENT: &str = "claude-cli/2.1.123 (external, sdk-cli)";

pub(crate) const OAUTH_BETA_HEADERS: &str = ANTHROPIC_OAUTH_BETA_HEADERS;
#[cfg(test)]
pub(crate) const OAUTH_BETA_HEADERS_1M: &str = jcode_provider_core::ANTHROPIC_OAUTH_BETA_HEADERS_1M;

pub fn effectively_1m(model: &str) -> bool {
    anthropic_effectively_1m(model)
}

fn oauth_beta_headers(model: &str) -> &'static str {
    anthropic_oauth_beta_headers(model)
}

pub(crate) fn new_oauth_request_id() -> String {
    Uuid::new_v4().to_string()
}

pub(crate) fn apply_oauth_attribution_headers(
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

#[derive(Debug, Clone, Default)]
struct OAuthClientMetadata {
    device_id: Option<String>,
    account_uuid: Option<String>,
    organization_uuid: Option<String>,
    email_address: Option<String>,
}

fn load_official_claude_client_metadata() -> OAuthClientMetadata {
    let path = match crate::storage::user_home_path(".claude.json") {
        Ok(path) => path,
        Err(_) => return OAuthClientMetadata::default(),
    };
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return OAuthClientMetadata::default(),
    };
    let parsed: Value = match serde_json::from_str(&content) {
        Ok(parsed) => parsed,
        Err(_) => return OAuthClientMetadata::default(),
    };
    let oauth = parsed.get("oauthAccount");
    OAuthClientMetadata {
        device_id: parsed
            .get("userID")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        account_uuid: oauth
            .and_then(|v| v.get("accountUuid"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        organization_uuid: oauth
            .and_then(|v| v.get("organizationUuid"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        email_address: oauth
            .and_then(|v| v.get("emailAddress"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
    }
}

fn oauth_request_metadata(session_id: &str) -> ApiMetadata {
    let official = load_official_claude_client_metadata();
    let device_id = official.device_id.unwrap_or_else(|| {
        Uuid::new_v5(&Uuid::NAMESPACE_DNS, session_id.as_bytes())
            .simple()
            .to_string()
    });
    let account_uuid = official
        .account_uuid
        .unwrap_or_else(|| "unknown-account".to_string());
    let user_id = json!({
        "device_id": device_id,
        "account_uuid": account_uuid,
        "session_id": session_id,
    })
    .to_string();
    ApiMetadata { user_id }
}

#[derive(Serialize)]
struct OAuthEvalRequest {
    attributes: OAuthEvalAttributes,
    #[serde(rename = "forcedVariations")]
    forced_variations: std::collections::BTreeMap<String, Value>,
    #[serde(rename = "forcedFeatures")]
    forced_features: Vec<String>,
    url: String,
}

#[derive(Serialize)]
struct OAuthEvalAttributes {
    id: String,
    #[serde(rename = "sessionId")]
    session_id: String,
    #[serde(rename = "deviceID")]
    device_id: String,
    platform: String,
    #[serde(rename = "organizationUUID")]
    organization_uuid: String,
    #[serde(rename = "accountUUID")]
    account_uuid: String,
    #[serde(rename = "userType")]
    user_type: String,
    #[serde(rename = "subscriptionType")]
    subscription_type: String,
    #[serde(rename = "rateLimitTier")]
    rate_limit_tier: String,
    #[serde(rename = "firstTokenTime")]
    first_token_time: i64,
    email: String,
    #[serde(rename = "appVersion")]
    app_version: String,
}

async fn oauth_preflight_get(
    client: &Client,
    headers: &reqwest::header::HeaderMap,
    label: &str,
    url: &str,
) -> Result<()> {
    let resp = client
        .get(url)
        .headers(headers.clone())
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = crate::util::http_error_body(resp, "HTTP error").await;
        anyhow::bail!("{} returned {}: {}", label, status, body);
    }

    Ok(())
}

async fn oauth_preflight_post_json<T: Serialize + ?Sized>(
    client: &Client,
    headers: &reqwest::header::HeaderMap,
    label: &str,
    url: &str,
    body: &T,
) -> Result<()> {
    let resp = client
        .post(url)
        .headers(headers.clone())
        .timeout(std::time::Duration::from_secs(5))
        .json(body)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = crate::util::http_error_body(resp, "HTTP error").await;
        anyhow::bail!("{} returned {}: {}", label, status, body);
    }

    Ok(())
}

fn record_oauth_preflight_result(label: &str, result: Result<()>) -> bool {
    match result {
        Ok(()) => true,
        Err(err) => {
            crate::logging::warn(&format!(
                "Claude OAuth preflight {} failed; continuing because Claude Code treats this bootstrap traffic as nonessential: {:#}",
                label, err
            ));
            false
        }
    }
}

async fn ensure_oauth_preflight(
    client: &Client,
    token: &str,
    session_id: &str,
    done_flag: &AtomicBool,
) -> Result<()> {
    if done_flag.load(Ordering::Relaxed) {
        return Ok(());
    }

    let official = load_official_claude_client_metadata();
    let Some(device_id) = official.device_id else {
        crate::logging::warn("Skipping Claude OAuth preflight: missing userID in ~/.claude.json");
        return Ok(());
    };
    let Some(account_uuid) = official.account_uuid else {
        crate::logging::warn(
            "Skipping Claude OAuth preflight: missing accountUuid in ~/.claude.json",
        );
        return Ok(());
    };
    let Some(organization_uuid) = official.organization_uuid else {
        crate::logging::warn(
            "Skipping Claude OAuth preflight: missing organizationUuid in ~/.claude.json",
        );
        return Ok(());
    };
    let Some(email_address) = official.email_address else {
        crate::logging::warn(
            "Skipping Claude OAuth preflight: missing emailAddress in ~/.claude.json",
        );
        return Ok(());
    };

    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        reqwest::header::HeaderValue::from_str(&format!("Bearer {}", token))?,
    );
    headers.insert(
        reqwest::header::USER_AGENT,
        reqwest::header::HeaderValue::from_static(CLAUDE_CLI_USER_AGENT),
    );
    headers.insert(
        reqwest::header::CONTENT_TYPE,
        reqwest::header::HeaderValue::from_static("application/json"),
    );
    headers.insert(
        reqwest::header::HeaderName::from_static("anthropic-beta"),
        reqwest::header::HeaderValue::from_static("oauth-2025-04-20"),
    );

    let mut all_ok = true;
    all_ok &= record_oauth_preflight_result(
        "bootstrap",
        oauth_preflight_get(
            client,
            &headers,
            "bootstrap",
            "https://api.anthropic.com/api/claude_cli/bootstrap",
        )
        .await,
    );
    all_ok &= record_oauth_preflight_result(
        "account settings",
        oauth_preflight_get(
            client,
            &headers,
            "account settings",
            "https://api.anthropic.com/api/oauth/account/settings",
        )
        .await,
    );
    all_ok &= record_oauth_preflight_result(
        "grove",
        oauth_preflight_get(
            client,
            &headers,
            "grove",
            "https://api.anthropic.com/api/claude_code_grove",
        )
        .await,
    );

    let eval = OAuthEvalRequest {
        attributes: OAuthEvalAttributes {
            id: device_id.clone(),
            session_id: session_id.to_string(),
            device_id: device_id.clone(),
            platform: std::env::consts::OS.to_string(),
            organization_uuid,
            account_uuid,
            user_type: "external".to_string(),
            subscription_type: crate::auth::claude::get_subscription_type()
                .unwrap_or_else(|| "pro".to_string()),
            rate_limit_tier: "default_claude_ai".to_string(),
            first_token_time: 1_740_976_801_491,
            email: email_address,
            app_version: "2.1.123".to_string(),
        },
        forced_variations: Default::default(),
        forced_features: Vec::new(),
        url: String::new(),
    };

    all_ok &= record_oauth_preflight_result(
        "eval",
        oauth_preflight_post_json(
            client,
            &headers,
            "eval",
            "https://api.anthropic.com/api/eval/sdk-zAZezfDKGoZuXXKe",
            &eval,
        )
        .await,
    );

    done_flag.store(true, Ordering::Relaxed);
    if all_ok {
        crate::logging::info("Claude OAuth preflight completed successfully");
    }
    Ok(())
}

/// Default model
const DEFAULT_MODEL: &str = "claude-opus-4-8";

/// API version header
const API_VERSION: &str = "2023-06-01";

/// Maximum number of retries for transient errors
const MAX_RETRIES: u32 = 3;

/// Base delay for exponential backoff (in milliseconds)
const RETRY_BASE_DELAY_MS: u64 = 1000;

/// Default max output tokens for Anthropic models.
/// Set to 32k to avoid truncating long tool calls (e.g. writing large files).
/// Override with JCODE_ANTHROPIC_MAX_TOKENS env var.
const DEFAULT_MAX_TOKENS: u32 = 32_768;

/// Available models
pub const AVAILABLE_MODELS: &[&str] = &[
    "claude-opus-4-8",
    "claude-opus-4-6",
    "claude-opus-4-6[1m]",
    "claude-sonnet-4-6",
    "claude-sonnet-4-6[1m]",
    "claude-haiku-4-5",
    "claude-opus-4-5",
    "claude-sonnet-4-5",
    "claude-sonnet-4-20250514",
];

/// Cached OAuth credentials
#[derive(Clone)]
struct CachedCredentials {
    access_token: String,
    refresh_token: String,
    expires_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AnthropicCredentialMode {
    Auto,
    OAuth,
    ApiKey,
}

impl AnthropicCredentialMode {
    fn from_runtime_env() -> Self {
        // Canonical parse: recognizes every runtime/route/CLI/prefix alias for
        // the Anthropic OAuth-vs-API decision in one place, so this can never
        // drift from the other vocabularies (see jcode_provider_core::auth_mode).
        match jcode_provider_core::runtime_env_pinned_mode(
            jcode_provider_core::DualAuthProvider::Anthropic,
        ) {
            Some(jcode_provider_core::AuthMode::ApiKey) => Self::ApiKey,
            Some(jcode_provider_core::AuthMode::Oauth) => Self::OAuth,
            None => Self::Auto,
        }
    }

    /// The canonical dual-auth route this explicit mode pins, if any.
    /// `Auto` has no explicit pin and returns `None`.
    pub(crate) fn auth_route(self) -> Option<jcode_provider_core::AuthRoute> {
        use jcode_provider_core::{AuthMode, AuthRoute};
        match self {
            Self::Auto => None,
            Self::OAuth => Some(AuthRoute::anthropic(AuthMode::Oauth)),
            Self::ApiKey => Some(AuthRoute::anthropic(AuthMode::ApiKey)),
        }
    }
}

pub(crate) fn load_anthropic_api_key() -> Result<String> {
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

pub(crate) fn has_anthropic_api_key() -> bool {
    load_anthropic_api_key().is_ok()
}

/// Direct Anthropic API provider
pub struct AnthropicProvider {
    client: Client,
    model: Arc<std::sync::RwLock<String>>,
    reasoning_effort: Arc<std::sync::RwLock<Option<String>>>,
    service_tier: Arc<std::sync::RwLock<Option<String>>>,
    /// Cached OAuth credentials (None if using API key)
    credentials: Arc<RwLock<Option<CachedCredentials>>>,
    credential_mode: Arc<RwLock<AnthropicCredentialMode>>,
    max_tokens: u32,
    oauth_session_id: String,
    oauth_preflight_done: Arc<AtomicBool>,
}

impl AnthropicProvider {
    fn is_usage_exhausted() -> bool {
        let usage = crate::usage::get_sync();
        usage.five_hour >= 0.99 && usage.seven_day >= 0.99
    }

    /// Resolve a usable access token (OAuth or API key) and whether it is OAuth.
    ///
    /// Exposed for the provider-doctor's native Claude driver so it can validate
    /// the credential and fetch the live model catalog through the exact same
    /// resolution path the runtime uses. Returns the bearer token and an
    /// `is_oauth` flag so callers can pick the matching catalog endpoint.
    pub async fn resolve_access_token_for_doctor(&self) -> Result<(String, bool)> {
        self.get_access_token().await
    }

    /// Pin the credential mode (OAuth vs API key) for a provider-doctor run.
    ///
    /// The `claude` login provider is specifically the OAuth/subscription path,
    /// while `claude-api` is the API-key path. The doctor must test the path
    /// implied by the provider id under test, regardless of what
    /// `JCODE_RUNTIME_PROVIDER` happens to be in the current process (e.g. a
    /// self-dev session may have it set to `claude-api`). This also updates
    /// `JCODE_RUNTIME_PROVIDER` so any provider instances the probes build
    /// afterwards inherit the same mode. Errors if the requested credential is
    /// not available, so the doctor can record a clear AUTH failure.
    pub fn pin_credential_mode_for_doctor(&self, oauth: bool) -> Result<()> {
        let mode = if oauth {
            AnthropicCredentialMode::OAuth
        } else {
            AnthropicCredentialMode::ApiKey
        };
        self.set_credential_mode(mode)
    }

    /// Fetch the live Anthropic model catalog using the resolved credential.
    ///
    /// Mirrors [`Provider::prefetch_models`] but returns the model ids to the
    /// caller (rather than only persisting them) so the doctor can assert the
    /// live `GET /v1/models` endpoint works and that the model under test is in
    /// the live catalog.
    pub async fn fetch_live_model_ids_for_doctor(&self) -> Result<Vec<String>> {
        let (token, is_oauth) = self.get_access_token().await?;
        if token.trim().is_empty() {
            anyhow::bail!("resolved an empty Anthropic access token");
        }
        let catalog = if is_oauth {
            crate::provider::fetch_anthropic_model_catalog_oauth(&token).await?
        } else {
            crate::provider::fetch_anthropic_model_catalog(&token).await?
        };
        // Persist so the rest of the process benefits from the warm catalog,
        // exactly like the runtime's own prefetch.
        crate::provider::persist_anthropic_model_catalog(&catalog);
        if !catalog.context_limits.is_empty() {
            crate::provider::populate_context_limits(catalog.context_limits.clone());
        }
        if !catalog.available_models.is_empty() {
            crate::provider::populate_anthropic_models(catalog.available_models.clone());
        }
        Ok(catalog.available_models)
    }

    pub fn new() -> Self {
        let model = std::env::var("JCODE_ANTHROPIC_MODEL").unwrap_or_else(|_| {
            if Self::is_usage_exhausted() {
                "claude-sonnet-4-6".to_string()
            } else {
                DEFAULT_MODEL.to_string()
            }
        });

        // Trigger background usage fetch so extra_usage is known before first API call
        let _ = tokio::runtime::Handle::try_current().map(|_| {
            tokio::spawn(async {
                let _ = crate::usage::get().await;
            })
        });

        let max_tokens = std::env::var("JCODE_ANTHROPIC_MAX_TOKENS")
            .ok()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .unwrap_or(DEFAULT_MAX_TOKENS);
        let reasoning_effort = crate::config::config()
            .provider
            .anthropic_reasoning_effort
            .as_deref()
            .and_then(Self::normalize_reasoning_effort)
            .map(|effort| Self::actual_effort_for_model(&model, &effort));

        Self {
            client: crate::provider::shared_http_client(),
            model: Arc::new(std::sync::RwLock::new(model)),
            reasoning_effort: Arc::new(std::sync::RwLock::new(reasoning_effort)),
            service_tier: Arc::new(std::sync::RwLock::new(None)),
            credentials: Arc::new(RwLock::new(None)),
            credential_mode: Arc::new(RwLock::new(AnthropicCredentialMode::from_runtime_env())),
            max_tokens,
            oauth_session_id: Uuid::new_v4().to_string(),
            oauth_preflight_done: Arc::new(AtomicBool::new(false)),
        }
    }

    fn normalized_model_key(model: &str) -> String {
        strip_1m_suffix(model).trim().to_ascii_lowercase()
    }

    fn model_supports_output_effort(model: &str) -> bool {
        let model = Self::normalized_model_key(model);
        model.contains("claude-mythos")
            || model.contains("claude-opus-4-8")
            || model.contains("claude-opus-4-7")
            || model.contains("claude-opus-4-6")
            || model.contains("claude-sonnet-4-6")
            || model.contains("claude-opus-4-5")
    }

    fn model_supports_adaptive_thinking(model: &str) -> bool {
        let model = Self::normalized_model_key(model);
        model.contains("claude-mythos")
            || model.contains("claude-opus-4-8")
            || model.contains("claude-opus-4-7")
            || model.contains("claude-opus-4-6")
            || model.contains("claude-sonnet-4-6")
    }

    fn model_supports_manual_thinking(model: &str) -> bool {
        let model = Self::normalized_model_key(model);
        model.contains("claude-opus-4-5")
            || model.contains("claude-3-7-sonnet")
            || model.contains("claude-sonnet-3-7")
    }

    fn model_supports_xhigh_effort(model: &str) -> bool {
        let model = Self::normalized_model_key(model);
        model.contains("claude-opus-4-8") || model.contains("claude-opus-4-7")
    }

    fn model_supports_reasoning_effort(model: &str) -> bool {
        Self::model_supports_output_effort(model) || Self::model_supports_manual_thinking(model)
    }

    fn normalize_reasoning_effort(raw: &str) -> Option<String> {
        let value = raw.trim().to_ascii_lowercase();
        if value.is_empty() || matches!(value.as_str(), "default" | "auto") {
            return None;
        }
        match value.as_str() {
            "off" | "disabled" => Some("none".to_string()),
            "none" | "low" | "medium" | "high" | "xhigh" | "max" => Some(value),
            other => {
                crate::logging::info(&format!(
                    "Warning: Unsupported Anthropic reasoning effort '{}'; expected none|low|medium|high|xhigh|max alias. Using the model maximum.",
                    other
                ));
                Some("max".to_string())
            }
        }
    }

    fn actual_effort_for_model(model: &str, effort: &str) -> String {
        if effort == "max" {
            if Self::model_supports_xhigh_effort(model) {
                "xhigh".to_string()
            } else {
                "high".to_string()
            }
        } else if effort == "xhigh" && !Self::model_supports_xhigh_effort(model) {
            "high".to_string()
        } else {
            effort.to_string()
        }
    }

    fn model_supports_priority_service_tier(model: &str) -> bool {
        Self::normalized_model_key(model).contains("claude-opus-4-8")
    }

    fn normalize_service_tier(raw: &str) -> Result<Option<String>> {
        let value = raw.trim().to_ascii_lowercase();
        match value.as_str() {
            "" | "default" => Ok(None),
            "off" | "standard" | "standard_only" => Ok(Some("standard_only".to_string())),
            // The Anthropic API uses `auto` for the latency-optimized tier. Keep
            // accepting `priority` because `/fast on` is shared with OpenAI.
            "priority" | "auto" => Ok(Some("auto".to_string())),
            other => anyhow::bail!(
                "Unsupported Anthropic service tier '{}'; expected priority/auto or off/standard_only",
                other
            ),
        }
    }

    fn current_service_tier_for_model(&self, model: &str) -> Option<String> {
        let tier = self
            .service_tier
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone());
        tier.filter(|_| Self::model_supports_priority_service_tier(model))
    }

    fn manual_thinking_budget(effort: &str, max_tokens: u32) -> Option<u32> {
        let desired = match effort {
            "low" => 1_024,
            "medium" => 4_096,
            "high" => 8_192,
            "xhigh" | "max" => 16_384,
            _ => return None,
        };
        let budget = desired.min(max_tokens.saturating_sub(1));
        (budget >= 1_024).then_some(budget)
    }

    fn build_reasoning_request_parts(
        &self,
        model: &str,
        is_oauth: bool,
    ) -> (Option<ApiThinking>, Option<ApiOutputConfig>, Option<f32>) {
        // `display.show_thinking` is a request to *see* the model's reasoning.
        // Anthropic only streams thinking summaries when a thinking request is
        // present, so opting into the display must also opt into generating it.
        let show_thinking = crate::config::config().display.show_thinking;
        self.build_reasoning_request_parts_inner(model, is_oauth, show_thinking)
    }

    fn build_reasoning_request_parts_inner(
        &self,
        model: &str,
        is_oauth: bool,
        show_thinking: bool,
    ) -> (Option<ApiThinking>, Option<ApiOutputConfig>, Option<f32>) {
        let effort = self.reasoning_effort();
        let effort = effort.as_deref().filter(|effort| *effort != "none");

        let output_config = effort
            .filter(|_| Self::model_supports_output_effort(model))
            .map(|effort| ApiOutputConfig {
                effort: Self::actual_effort_for_model(model, effort),
            });

        // When only the display toggle is on (no explicit effort), request
        // thinking without forcing `output_config`, so the model keeps its
        // default reasoning strength and only the thinking *display* is enabled.
        let thinking = if Self::model_supports_adaptive_thinking(model) {
            (effort.is_some() || show_thinking).then_some(ApiThinking::Adaptive {
                display: Some("summarized"),
            })
        } else if Self::model_supports_manual_thinking(model) {
            // Manual-thinking models need a concrete budget. Use the configured
            // effort, or fall back to a minimal budget when only the display
            // toggle is on.
            effort
                .or(show_thinking.then_some("low"))
                .and_then(|effort| Self::manual_thinking_budget(effort, self.max_tokens))
                .map(|budget_tokens| ApiThinking::Enabled { budget_tokens })
        } else {
            None
        };

        // Extended/adaptive thinking is incompatible with temperature. OAuth path
        // normally mirrors Claude Code's temperature=1.0, so omit it when thinking is active.
        let temperature = if is_oauth && thinking.is_none() {
            Some(1.0)
        } else {
            None
        };

        (thinking, output_config, temperature)
    }

    /// Get the access token from credentials
    /// Supports both OAuth tokens and direct API keys
    /// Automatically refreshes OAuth tokens when expired
    async fn get_access_token(&self) -> Result<(String, bool)> {
        let mode = *self.credential_mode.read().await;

        // Explicit API-key mode: use the direct API key and surface an error if
        // one is not configured (never silently fall back to OAuth).
        if matches!(mode, AnthropicCredentialMode::ApiKey) {
            let key = load_anthropic_api_key()?;
            return Ok((key, false)); // false = not OAuth
        }

        // Auto mode prefers OAuth (Claude subscription) when credentials are
        // available, falling back to the direct API key. This matches the
        // OpenAI provider's OAuth-first Auto behavior and what most Claude
        // Max/Pro users expect.
        if matches!(mode, AnthropicCredentialMode::Auto)
            && auth::claude::load_credentials().is_err()
        {
            if let Ok(key) = load_anthropic_api_key() {
                return Ok((key, false));
            }
        }

        self.get_oauth_access_token().await
    }

    async fn get_oauth_access_token(&self) -> Result<(String, bool)> {
        // Check cached credentials
        {
            let cached = self.credentials.read().await;
            if let Some(ref creds) = *cached {
                let now = chrono::Utc::now().timestamp_millis();
                // Return cached token if not expired (with 5 min buffer)
                if creds.expires_at > now + 300_000 {
                    return Ok((creds.access_token.clone(), true));
                }
            }
        }

        // Load fresh credentials or refresh expired ones
        let fresh_creds =
            auth::claude::load_credentials().context("Failed to load Claude credentials")?;

        if !fresh_creds.scopes.is_empty()
            && !oauth::claude_scopes_have_inference(&fresh_creds.scopes)
        {
            anyhow::bail!(
                "Claude OAuth credentials are missing the required user:inference scope (scopes: {}). Run `jcode login --provider claude` to mint a fresh Claude.ai OAuth token, or import/use a fresh Claude Code login.",
                fresh_creds.scopes.join(" ")
            );
        }

        let now = chrono::Utc::now().timestamp_millis();

        // Check if token needs refresh (expired or expiring within 5 minutes)
        if fresh_creds.expires_at < now + 300_000 && !fresh_creds.refresh_token.is_empty() {
            crate::logging::info("OAuth token expired or expiring soon, attempting refresh...");

            let active_label = auth::claude::active_account_label()
                .unwrap_or_else(auth::claude::primary_account_label);
            match oauth::refresh_claude_tokens_for_account(
                &fresh_creds.refresh_token,
                &active_label,
            )
            .await
            {
                Ok(refreshed) => {
                    crate::logging::info("OAuth token refreshed successfully");

                    // Cache the refreshed credentials
                    let mut cached = self.credentials.write().await;
                    *cached = Some(CachedCredentials {
                        access_token: refreshed.access_token.clone(),
                        refresh_token: refreshed.refresh_token,
                        expires_at: refreshed.expires_at,
                    });

                    return Ok((refreshed.access_token, true));
                }
                Err(e) => {
                    crate::logging::error(&format!("OAuth token refresh failed: {}", e));
                    // Fall through to try the possibly-expired token
                }
            }
        }

        // Cache and return the loaded credentials (even if expired, let the API reject it)
        let mut cached = self.credentials.write().await;
        *cached = Some(CachedCredentials {
            access_token: fresh_creds.access_token.clone(),
            refresh_token: fresh_creds.refresh_token,
            expires_at: fresh_creds.expires_at,
        });

        Ok((fresh_creds.access_token, true))
    }

    pub(crate) fn set_credential_mode(&self, mode: AnthropicCredentialMode) -> Result<()> {
        match mode {
            AnthropicCredentialMode::Auto => {}
            AnthropicCredentialMode::ApiKey => {
                load_anthropic_api_key()?;
            }
            AnthropicCredentialMode::OAuth => {
                auth::claude::load_credentials().context("Failed to load Claude credentials")?;
            }
        }
        let mut mode_guard = self.credential_mode.try_write().map_err(|_| {
            anyhow::anyhow!(
                "Cannot change Anthropic credential mode while a request is in progress"
            )
        })?;
        *mode_guard = mode;
        drop(mode_guard);
        if let Ok(mut cached) = self.credentials.try_write() {
            *cached = None;
        }
        // Keep the runtime provider identity in sync with the explicit credential
        // choice so UI surfaces (model picker, header widget) report the auth
        // method that requests will actually use, instead of inferring it from
        // credential presence. `Auto` leaves the existing identity untouched.
        if let Some(route) = mode.auth_route() {
            crate::env::set_var("JCODE_RUNTIME_PROVIDER", route.runtime_provider_key());
        }
        Ok(())
    }

    pub(crate) fn credential_mode_snapshot(&self) -> AnthropicCredentialMode {
        self.credential_mode
            .try_read()
            .map(|mode| *mode)
            .unwrap_or(AnthropicCredentialMode::Auto)
    }

    #[cfg(test)]
    pub(crate) async fn test_access_token_and_oauth_mode(&self) -> Result<(String, bool)> {
        self.get_access_token().await
    }

    /// Convert our Message type to Anthropic API format
    /// Also repairs dangling tool_uses by injecting synthetic tool_results
    fn format_messages(&self, messages: &[Message], is_oauth: bool) -> Vec<ApiMessage> {
        jcode_provider_anthropic::format_messages(messages, is_oauth)
    }

    /// Convert our ContentBlock to Anthropic API format
    #[cfg(test)]
    fn format_content_blocks(
        &self,
        blocks: &[ContentBlock],
        is_oauth: bool,
    ) -> Vec<ApiContentBlock> {
        jcode_provider_anthropic::format_content_blocks(blocks, is_oauth)
    }

    /// Convert tool definitions to Anthropic API format
    /// Adds cache_control to the last tool for prompt caching
    fn format_tools(&self, tools: &[ToolDefinition], is_oauth: bool) -> Vec<ApiTool> {
        jcode_provider_anthropic::format_tools(tools, is_oauth, is_cache_ttl_1h())
    }
}

impl Default for AnthropicProvider {
    fn default() -> Self {
        Self::new()
    }
}

fn log_anthropic_canonical_input(
    model: &str,
    format: &str,
    request: &ApiRequest,
    is_oauth: bool,
    split_prompt: bool,
) {
    let messages_value = serde_json::to_value(&request.messages).unwrap_or(Value::Null);
    let message_items = messages_value.as_array().cloned().unwrap_or_default();
    let system_value = request
        .system
        .as_ref()
        .and_then(|system| serde_json::to_value(system).ok());
    let tools_value = request
        .tools
        .as_ref()
        .and_then(|tools| serde_json::to_value(tools).ok());
    let payload = json!({
        "model": &request.model,
        "max_tokens": request.max_tokens,
        "system": system_value.as_ref(),
        "messages": messages_value,
        "tools": tools_value.as_ref(),
        "thinking": &request.thinking,
        "output_config": &request.output_config,
        "temperature": request.temperature,
    });

    super::fingerprint::log_provider_canonical_input(
        "anthropic",
        model,
        format,
        &payload,
        &message_items,
        system_value.as_ref(),
        tools_value.as_ref(),
        request.tools.as_ref().map(|tools| tools.len()),
        &[
            ("oauth", is_oauth.to_string()),
            ("split_prompt", split_prompt.to_string()),
        ],
    );
}

#[async_trait]
impl Provider for AnthropicProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let (token, is_oauth) = self.get_access_token().await?;
        if is_oauth {
            ensure_oauth_preflight(
                &self.client,
                &token,
                &self.oauth_session_id,
                &self.oauth_preflight_done,
            )
            .await?;
        }
        let model = self
            .model
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let api_model = strip_1m_suffix(&model).to_string();

        // Format request
        let api_messages = self.format_messages(messages, is_oauth);
        let api_tools = self.format_tools(tools, is_oauth);
        let (thinking, output_config, temperature) =
            self.build_reasoning_request_parts(&model, is_oauth);

        let request = ApiRequest {
            model: api_model,
            max_tokens: self.max_tokens,
            system: build_system_param(system, is_oauth),
            messages: format_messages_with_identity(api_messages, is_oauth),
            tools: if api_tools.is_empty() {
                None
            } else {
                Some(api_tools)
            },
            metadata: if is_oauth {
                Some(oauth_request_metadata(&self.oauth_session_id))
            } else {
                None
            },
            thinking,
            output_config,
            temperature,
            service_tier: self.current_service_tier_for_model(&model),
            stream: true,
        };

        log_anthropic_canonical_input(&model, "anthropic_messages", &request, is_oauth, false);

        crate::logging::info(&format!(
            "Anthropic transport: HTTPS SSE stream (oauth={})",
            is_oauth
        ));

        // Create channel for streaming events
        let (tx, rx) = mpsc::channel::<Result<StreamEvent>>(100);

        // Clone what we need for the async task
        let client = self.client.clone();
        let credentials = Arc::clone(&self.credentials);
        let oauth_session_id = self.oauth_session_id.clone();

        // Spawn task to handle streaming with retry logic.
        // This includes forced OAuth refresh on auth failures.
        tokio::spawn(async move {
            if tx
                .send(Ok(StreamEvent::ConnectionType {
                    connection: "https/sse".to_string(),
                }))
                .await
                .is_err()
            {
                return;
            }
            run_stream_with_retries(
                client,
                token,
                is_oauth,
                request,
                tx,
                credentials,
                model,
                oauth_session_id,
            )
            .await;
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    fn model(&self) -> String {
        self.model
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn set_model(&self, model: &str) -> Result<()> {
        // Native-1M models (Opus 4.8/4.7) no longer carry a redundant `[1m]`
        // alias. Gracefully migrate a stale `<model>[1m]` id (from old config or
        // a restored session) to its canonical form, since the suffix is a no-op
        // for these models.
        let model: &str = if is_1m_model(model)
            && matches!(
                jcode_provider_core::anthropic_context_mode(model),
                jcode_provider_core::AnthropicContextMode::Native1M
            ) {
            strip_1m_suffix(model)
        } else {
            model
        };
        if !crate::provider::known_anthropic_model_ids()
            .iter()
            .any(|known| known == model)
        {
            anyhow::bail!("Model {} not supported by Anthropic provider", model);
        }
        *self
            .model
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = model.to_string();
        match self.reasoning_effort.write() {
            Ok(mut guard) => {
                if let Some(current) = guard.clone() {
                    *guard = Some(Self::actual_effort_for_model(model, &current));
                }
            }
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                if let Some(current) = guard.clone() {
                    *guard = Some(Self::actual_effort_for_model(model, &current));
                }
            }
        }
        Ok(())
    }

    fn available_models(&self) -> Vec<&'static str> {
        AVAILABLE_MODELS.to_vec()
    }

    fn available_models_for_switching(&self) -> Vec<String> {
        crate::provider::cached_anthropic_model_ids()
            .unwrap_or_else(crate::provider::known_anthropic_model_ids)
    }

    fn available_models_display(&self) -> Vec<String> {
        self.available_models_for_switching()
    }

    fn reasoning_effort(&self) -> Option<String> {
        if !Self::model_supports_reasoning_effort(&self.model()) {
            return None;
        }
        let effort = self
            .reasoning_effort
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_else(|poisoned| poisoned.into_inner().clone());
        Some(effort.unwrap_or_else(|| "none".to_string()))
    }

    fn set_reasoning_effort(&self, effort: &str) -> Result<()> {
        let normalized = Self::normalize_reasoning_effort(effort);
        let model = self.model();
        if normalized.is_some() && !Self::model_supports_reasoning_effort(&model) {
            anyhow::bail!(
                "Reasoning effort is only supported for Claude 3.7 reasoning models and Claude 4.5+ models that expose Anthropic thinking/output_config"
            );
        }
        if normalized.as_deref() == Some("xhigh") && !Self::model_supports_xhigh_effort(&model) {
            anyhow::bail!("Anthropic xhigh effort is only supported for Claude Opus 4.7 models");
        }
        let normalized = normalized.map(|effort| Self::actual_effort_for_model(&model, &effort));
        match self.reasoning_effort.write() {
            Ok(mut guard) => {
                *guard = normalized;
                Ok(())
            }
            Err(poisoned) => {
                *poisoned.into_inner() = normalized;
                Ok(())
            }
        }
    }

    fn available_efforts(&self) -> Vec<&'static str> {
        let model = self.model();
        if !Self::model_supports_reasoning_effort(&model) {
            return vec![];
        }
        if Self::model_supports_xhigh_effort(&model) {
            vec!["none", "low", "medium", "high", "xhigh"]
        } else {
            vec!["none", "low", "medium", "high"]
        }
    }

    fn service_tier(&self) -> Option<String> {
        match self
            .current_service_tier_for_model(&self.model())
            .as_deref()
        {
            Some("auto") => Some("priority".to_string()),
            _ => None,
        }
    }

    fn set_service_tier(&self, service_tier: &str) -> Result<()> {
        let normalized = Self::normalize_service_tier(service_tier)?;
        if normalized.as_deref() == Some("auto")
            && !Self::model_supports_priority_service_tier(&self.model())
        {
            anyhow::bail!("Anthropic priority fast tier is only supported for Claude Opus 4.8");
        }
        *self
            .service_tier
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = normalized;
        Ok(())
    }

    fn available_service_tiers(&self) -> Vec<&'static str> {
        if Self::model_supports_priority_service_tier(&self.model()) {
            vec!["off", "priority"]
        } else {
            vec![]
        }
    }

    async fn prefetch_models(&self) -> Result<()> {
        let (token, is_oauth) = self.get_access_token().await?;
        if token.trim().is_empty() {
            return Ok(());
        }

        let catalog = if is_oauth {
            match crate::provider::fetch_anthropic_model_catalog_oauth(&token).await {
                Ok(catalog) => catalog,
                Err(err) => {
                    crate::logging::warn(&format!(
                        "Anthropic OAuth model catalog refresh failed; keeping fallback list: {}",
                        err
                    ));
                    return Ok(());
                }
            }
        } else {
            crate::provider::fetch_anthropic_model_catalog(&token).await?
        };
        crate::provider::persist_anthropic_model_catalog(&catalog);
        if !catalog.context_limits.is_empty() {
            crate::provider::populate_context_limits(catalog.context_limits);
        }
        if !catalog.available_models.is_empty() {
            crate::provider::populate_anthropic_models(catalog.available_models);
        }
        Ok(())
    }

    fn name(&self) -> &'static str {
        "anthropic"
    }

    fn supports_image_input(&self) -> bool {
        true
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self {
            client: self.client.clone(),
            model: Arc::new(std::sync::RwLock::new(
                self.model
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone(),
            )),
            reasoning_effort: Arc::new(std::sync::RwLock::new(self.reasoning_effort())),
            service_tier: Arc::new(std::sync::RwLock::new(self.service_tier())),
            credentials: Arc::new(RwLock::new(None)),
            credential_mode: Arc::clone(&self.credential_mode),
            max_tokens: self.max_tokens,
            oauth_session_id: self.oauth_session_id.clone(),
            oauth_preflight_done: Arc::new(AtomicBool::new(
                self.oauth_preflight_done.load(Ordering::Relaxed),
            )),
        })
    }

    async fn invalidate_credentials(&self) {
        let mut cached = self.credentials.write().await;
        *cached = None;
    }

    fn native_result_sender(&self) -> Option<NativeToolResultSender> {
        None // Direct API doesn't use native tool bridge
    }

    /// Split system prompt completion for better cache efficiency
    /// Static content is cached, dynamic content is not
    async fn complete_split(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system_static: &str,
        system_dynamic: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let (token, is_oauth) = self.get_access_token().await?;
        if is_oauth {
            ensure_oauth_preflight(
                &self.client,
                &token,
                &self.oauth_session_id,
                &self.oauth_preflight_done,
            )
            .await?;
        }
        let model = self
            .model
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let api_model = strip_1m_suffix(&model).to_string();

        // Format request
        let api_messages = self.format_messages(messages, is_oauth);
        let api_tools = self.format_tools(tools, is_oauth);
        let (thinking, output_config, temperature) =
            self.build_reasoning_request_parts(&model, is_oauth);

        let request = ApiRequest {
            model: api_model,
            max_tokens: self.max_tokens,
            system: build_system_param_split(system_static, system_dynamic, is_oauth),
            messages: format_messages_with_identity(api_messages, is_oauth),
            tools: if api_tools.is_empty() {
                None
            } else {
                Some(api_tools)
            },
            metadata: if is_oauth {
                Some(oauth_request_metadata(&self.oauth_session_id))
            } else {
                None
            },
            thinking,
            output_config,
            temperature,
            service_tier: self.current_service_tier_for_model(&model),
            stream: true,
        };

        log_anthropic_canonical_input(&model, "anthropic_messages_split", &request, is_oauth, true);

        crate::logging::info(&format!(
            "Anthropic transport: HTTPS SSE split stream (oauth={})",
            is_oauth
        ));

        // Create channel for streaming events
        let (tx, rx) = mpsc::channel::<Result<StreamEvent>>(100);

        // Clone what we need for the async task
        let client = self.client.clone();
        let credentials = Arc::clone(&self.credentials);
        let oauth_session_id = self.oauth_session_id.clone();

        // Spawn task to handle streaming with retry logic
        tokio::spawn(async move {
            if tx
                .send(Ok(StreamEvent::ConnectionType {
                    connection: "https/sse".to_string(),
                }))
                .await
                .is_err()
            {
                return;
            }
            run_stream_with_retries(
                client,
                token,
                is_oauth,
                request,
                tx,
                credentials,
                model,
                oauth_session_id,
            )
            .await;
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "stream retry helper needs auth/session/runtime knobs together and is kept local for clarity"
)]
async fn run_stream_with_retries(
    client: Client,
    initial_token: String,
    is_oauth: bool,
    request: ApiRequest,
    tx: mpsc::Sender<Result<StreamEvent>>,
    credentials: Arc<RwLock<Option<CachedCredentials>>>,
    model_name: String,
    oauth_session_id: String,
) {
    let mut token = initial_token;
    let mut last_error = None;
    let mut attempted_forced_refresh = false;

    for attempt in 0..MAX_RETRIES {
        if attempt > 0 {
            // Exponential backoff: 1s, 2s, 4s
            let delay = RETRY_BASE_DELAY_MS * (1 << (attempt - 1));
            let _ = tx
                .send(Ok(StreamEvent::ConnectionPhase {
                    phase: crate::message::ConnectionPhase::Retrying {
                        attempt: attempt + 1,
                        max: MAX_RETRIES,
                    },
                }))
                .await;
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            crate::logging::info(&format!(
                "Retrying Anthropic API request (attempt {}/{})",
                attempt + 1,
                MAX_RETRIES
            ));
        }

        match stream_response(
            client.clone(),
            token.clone(),
            is_oauth,
            request.clone(),
            tx.clone(),
            &model_name,
            &oauth_session_id,
        )
        .await
        {
            Ok(()) => return, // Success
            Err(e) => {
                let error_str = e.to_string().to_lowercase();

                // OAuth auth failures: force refresh and retry once immediately.
                if is_oauth && is_oauth_auth_error(&error_str) && !attempted_forced_refresh {
                    attempted_forced_refresh = true;
                    crate::logging::info(
                        "Anthropic OAuth authentication failed, forcing token refresh...",
                    );
                    let _ = tx
                        .send(Ok(StreamEvent::ConnectionPhase {
                            phase: crate::message::ConnectionPhase::Authenticating,
                        }))
                        .await;
                    match force_refresh_oauth_token(Arc::clone(&credentials)).await {
                        Ok(refreshed_token) => {
                            crate::logging::info(
                                "Forced OAuth token refresh succeeded, retrying request.",
                            );
                            token = refreshed_token;
                            last_error = Some(e);
                            continue;
                        }
                        Err(refresh_err) => {
                            let _ = tx
                                .send(Err(anyhow::anyhow!(
                                    "{}\n\nAutomatic Claude OAuth refresh failed: {}\nRun `jcode login --provider claude` (preferred) or `claude`, then retry.",
                                    e,
                                    refresh_err
                                )))
                                .await;
                            return;
                        }
                    }
                }

                // Check if this is a transient/retryable error
                if is_retryable_error(&error_str) && attempt + 1 < MAX_RETRIES {
                    crate::logging::info(&format!("Transient error, will retry: {}", e));
                    last_error = Some(e);
                    continue;
                }

                // Non-retryable or final attempt
                if is_oauth && is_oauth_auth_error(&error_str) {
                    let _ = tx
                        .send(Err(anyhow::anyhow!(
                            "{}\n\nClaude OAuth authentication failed. Run `jcode login --provider claude` (preferred) or `claude`, then retry.",
                            e
                        )))
                        .await;
                } else {
                    let _ = tx.send(Err(e)).await;
                }
                return;
            }
        }
    }

    // All retries exhausted
    if let Some(e) = last_error {
        let _ = tx
            .send(Err(anyhow::anyhow!(
                "Failed after {} retries: {}",
                MAX_RETRIES,
                e
            )))
            .await;
    }
}

async fn force_refresh_oauth_token(
    credentials: Arc<RwLock<Option<CachedCredentials>>>,
) -> Result<String> {
    let refresh_from_cache = {
        let cached = credentials.read().await;
        cached
            .as_ref()
            .map(|c| c.refresh_token.clone())
            .filter(|t| !t.is_empty())
    };

    let refresh_token = if let Some(token) = refresh_from_cache {
        token
    } else {
        let loaded = auth::claude::load_credentials()
            .context("Failed to load Claude credentials for forced refresh")?;
        if loaded.refresh_token.is_empty() {
            anyhow::bail!("No refresh token available in Claude credentials");
        }
        loaded.refresh_token
    };

    let active_label =
        auth::claude::active_account_label().unwrap_or_else(auth::claude::primary_account_label);
    let refreshed =
        match oauth::refresh_claude_tokens_for_account(&refresh_token, &active_label).await {
            Ok(refreshed) => refreshed,
            Err(err) => {
                anyhow::bail!("OAuth refresh endpoint rejected the refresh token: {err:#}");
            }
        };

    {
        let mut cached = credentials.write().await;
        *cached = Some(CachedCredentials {
            access_token: refreshed.access_token.clone(),
            refresh_token: refreshed.refresh_token,
            expires_at: refreshed.expires_at,
        });
    }

    Ok(refreshed.access_token)
}

/// Stream the response from Anthropic API
async fn stream_response(
    client: Client,
    token: String,
    is_oauth: bool,
    request: ApiRequest,
    tx: mpsc::Sender<Result<StreamEvent>>,
    model_name: &str,
    oauth_session_id: &str,
) -> Result<()> {
    use crate::message::ConnectionPhase;
    if std::env::var("JCODE_ANTHROPIC_DEBUG")
        .map(|v| v == "1")
        .unwrap_or(false)
        && let Ok(json) = serde_json::to_string_pretty(&request)
    {
        crate::logging::info(&format!("Anthropic request payload:\n{}", json));
    }

    let _ = tx
        .send(Ok(StreamEvent::ConnectionPhase {
            phase: ConnectionPhase::Connecting,
        }))
        .await;

    let connect_start = std::time::Instant::now();
    // Build request with appropriate auth headers
    let url = if is_oauth { API_URL_OAUTH } else { API_URL };

    let mut req = client
        .post(url)
        .header("anthropic-version", API_VERSION)
        .header("content-type", "application/json")
        .header(
            "accept",
            if is_oauth {
                "application/json"
            } else {
                "text/event-stream"
            },
        );

    if is_oauth {
        // OAuth tokens require:
        // 1. Bearer auth (NOT x-api-key)
        // 2. User-Agent matching Claude CLI
        // 3. Multiple beta headers
        // 4. ?beta=true query param (in URL above)
        let beta_header = anthropic_beta_header_with_thinking(
            oauth_beta_headers(model_name),
            request.thinking.is_some(),
        );
        req = apply_oauth_attribution_headers(
            req.header("Authorization", format!("Bearer {}", token))
                .header("User-Agent", CLAUDE_CLI_USER_AGENT)
                .header("anthropic-beta", beta_header),
            oauth_session_id,
        );
    } else {
        // Direct API keys use x-api-key
        // Include prompt-caching beta header
        let beta_header = if is_1m_model(model_name) {
            "prompt-caching-2024-07-31,context-1m-2025-08-07"
        } else {
            "prompt-caching-2024-07-31"
        };
        let beta_header =
            anthropic_beta_header_with_thinking(beta_header, request.thinking.is_some());
        req = req
            .header("x-api-key", &token)
            .header("anthropic-beta", beta_header);
    }

    let response = req
        .json(&request)
        .send()
        .await
        .context("Failed to send request to Anthropic API")?;

    let connect_ms = connect_start.elapsed().as_millis();
    crate::logging::info(&format!(
        "HTTP connection established in {}ms (status={})",
        connect_ms,
        response.status()
    ));

    if !response.status().is_success() {
        let status = response.status();
        let error_text = crate::util::http_error_body(response, "HTTP error").await;
        anyhow::bail!("Anthropic API error ({}): {}", status, error_text);
    }

    let _ = tx
        .send(Ok(StreamEvent::ConnectionPhase {
            phase: ConnectionPhase::WaitingForResponse,
        }))
        .await;

    // Parse SSE stream
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut current_tool_use: Option<ToolUseAccumulator> = None;
    let mut current_thinking_block = false;
    let mut input_tokens: Option<u64> = None;
    let mut output_tokens: Option<u64> = None;
    let mut cache_read_input_tokens: Option<u64> = None;
    let mut cache_creation_input_tokens: Option<u64> = None;

    const SSE_CHUNK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

    loop {
        let chunk = match tokio::time::timeout(SSE_CHUNK_TIMEOUT, stream.next()).await {
            Ok(Some(chunk_result)) => chunk_result.context("Error reading stream chunk")?,
            Ok(None) => break, // stream ended normally
            Err(_) => {
                crate::logging::warn("Anthropic SSE stream timed out (no data for 180s)");
                anyhow::bail!("Stream read timeout: no data received for 180 seconds");
            }
        };
        let chunk_str = String::from_utf8_lossy(&chunk);
        buffer.push_str(&chunk_str);

        // Process complete SSE events
        while let Some(event) = parse_sse_event(&mut buffer) {
            let events = process_sse_event(
                &event,
                &mut current_tool_use,
                &mut current_thinking_block,
                &mut input_tokens,
                &mut output_tokens,
                &mut cache_read_input_tokens,
                &mut cache_creation_input_tokens,
                is_oauth,
            );
            for stream_event in events {
                if let StreamEvent::Error { ref message, .. } = stream_event
                    && is_retryable_error(&message.to_lowercase())
                {
                    anyhow::bail!("Retryable stream error: {}", message);
                }
                if tx.send(Ok(stream_event)).await.is_err() {
                    return Ok(()); // Receiver dropped
                }
            }
        }
    }

    // Send final token usage if we have it
    if input_tokens.is_some() || output_tokens.is_some() {
        // Log cache usage for debugging
        if cache_read_input_tokens.is_some() || cache_creation_input_tokens.is_some() {
            crate::logging::info(&format!(
                "Prompt cache: read={:?} created={:?}",
                cache_read_input_tokens, cache_creation_input_tokens
            ));
        }
        let _ = tx
            .send(Ok(StreamEvent::TokenUsage {
                input_tokens,
                output_tokens,
                cache_read_input_tokens,
                cache_creation_input_tokens,
            }))
            .await;
    }

    Ok(())
}

/// Check if an error is transient and should be retried
fn is_retryable_error(error_str: &str) -> bool {
    crate::provider::is_transient_transport_error(error_str)
        // Server errors (5xx)
        || error_str.contains("500 internal server error")
        || error_str.contains("502 bad gateway")
        || error_str.contains("503 service unavailable")
        || error_str.contains("504 gateway timeout")
        || error_str.contains("overloaded")
        // Rate limiting (429)
        || error_str.contains("429 too many requests")
        || error_str.contains("rate limit")
        || error_str.contains("rate_limit")
        // API-level server errors (SSE error events)
        || error_str.contains("api_error")
        || error_str.contains("internal server error")
}

fn is_oauth_auth_error(error_str: &str) -> bool {
    error_str.contains("oauth token has expired")
        || error_str.contains("token has expired")
        || error_str.contains("authentication_error")
        || error_str.contains("invalid token")
        || error_str.contains("invalid_grant")
        || error_str.contains("does not meet scope requirement")
        || ((error_str.contains("401 unauthorized") || error_str.contains("403 forbidden"))
            && (error_str.contains("oauth") || error_str.contains("token")))
}

fn anthropic_beta_header_with_thinking(base: &str, thinking_enabled: bool) -> String {
    if thinking_enabled && !base.contains("interleaved-thinking-2025-05-14") {
        format!("{base},interleaved-thinking-2025-05-14")
    } else {
        base.to_string()
    }
}

/// Accumulator for tool_use blocks (input comes in chunks)
struct ToolUseAccumulator {
    input_json: String,
}

/// Parse a single SSE event from the buffer
fn parse_sse_event(buffer: &mut String) -> Option<SseEvent> {
    // Look for complete event (ends with double newline)
    let event_end = buffer.find("\n\n")?;
    let event_str = buffer[..event_end].to_string();
    buffer.drain(..event_end + 2);

    let mut event_type = String::new();
    let mut data = String::new();

    for line in event_str.lines() {
        if let Some(rest) = line.strip_prefix("event: ") {
            event_type = rest.to_string();
        } else if let Some(rest) = crate::util::sse_data_line(line) {
            data = rest.to_string();
        }
    }

    if event_type.is_empty() && data.is_empty() {
        return None;
    }

    Some(SseEvent { event_type, data })
}

/// SSE event from the stream
struct SseEvent {
    event_type: String,
    data: String,
}

/// Process an SSE event and return StreamEvents if applicable
fn process_sse_event(
    event: &SseEvent,
    current_tool_use: &mut Option<ToolUseAccumulator>,
    current_thinking_block: &mut bool,
    input_tokens: &mut Option<u64>,
    output_tokens: &mut Option<u64>,
    cache_read_input_tokens: &mut Option<u64>,
    cache_creation_input_tokens: &mut Option<u64>,
    is_oauth: bool,
) -> Vec<StreamEvent> {
    let mut events = Vec::new();

    match event.event_type.as_str() {
        "message_start" => {
            // Extract usage from message_start (includes cache info)
            if let Ok(parsed) = serde_json::from_str::<MessageStartEvent>(&event.data)
                && let Some(usage) = parsed.message.usage
            {
                *input_tokens = usage.input_tokens.map(|t| t as u64);
                *cache_read_input_tokens = usage.cache_read_input_tokens.map(|t| t as u64);
                *cache_creation_input_tokens = usage.cache_creation_input_tokens.map(|t| t as u64);
                if let Some(tier) = usage.service_tier.as_deref() {
                    crate::logging::info(&format!("Anthropic granted service_tier={}", tier));
                    if std::env::var("JCODE_LOG_SERVICE_TIER").is_ok() {
                        eprintln!("[anthropic] granted service_tier={tier}");
                    }
                }
            }
        }
        "content_block_start" => {
            if let Ok(parsed) = serde_json::from_str::<ContentBlockStartEvent>(&event.data) {
                match parsed.content_block {
                    ApiContentBlockStart::Text { .. } => {
                        // Text block starting - nothing to emit yet
                    }
                    ApiContentBlockStart::Thinking { _thinking, .. } => {
                        *current_thinking_block = true;
                        events.push(StreamEvent::ThinkingStart);
                        if !_thinking.is_empty() {
                            events.push(StreamEvent::ThinkingDelta(_thinking));
                        }
                    }
                    ApiContentBlockStart::RedactedThinking { .. } => {
                        *current_thinking_block = true;
                        events.push(StreamEvent::ThinkingStart);
                    }
                    ApiContentBlockStart::ToolUse { id, name } => {
                        let mapped_name = if is_oauth {
                            map_tool_name_from_oauth(&name)
                        } else {
                            name.clone()
                        };
                        // Start accumulating tool use
                        *current_tool_use = Some(ToolUseAccumulator {
                            input_json: String::new(),
                        });
                        events.push(StreamEvent::ToolUseStart {
                            id,
                            name: mapped_name,
                        });
                    }
                }
            }
        }
        "content_block_delta" => {
            if let Ok(parsed) = serde_json::from_str::<ContentBlockDeltaEvent>(&event.data) {
                match parsed.delta {
                    ApiDelta::TextDelta { text } => {
                        events.push(StreamEvent::TextDelta(text));
                    }
                    ApiDelta::InputJsonDelta { partial_json } => {
                        if let Some(tool) = current_tool_use {
                            tool.input_json.push_str(&partial_json);
                        }
                        events.push(StreamEvent::ToolInputDelta(partial_json));
                    }
                    ApiDelta::ThinkingDelta { thinking } => {
                        events.push(StreamEvent::ThinkingDelta(thinking));
                    }
                    ApiDelta::SignatureDelta { signature } => {
                        events.push(StreamEvent::ThinkingSignatureDelta(signature));
                    }
                }
            }
        }
        "content_block_stop" => {
            // If we were accumulating a tool_use, it's complete now
            if current_tool_use.take().is_some() {
                events.push(StreamEvent::ToolUseEnd);
            } else if *current_thinking_block {
                *current_thinking_block = false;
                events.push(StreamEvent::ThinkingEnd);
            }
        }
        "message_delta" => {
            if let Ok(parsed) = serde_json::from_str::<MessageDeltaEvent>(&event.data) {
                if let Some(usage) = parsed.usage {
                    *output_tokens = usage.output_tokens.map(|t| t as u64);
                }
                if let Some(stop_reason) = parsed.delta.stop_reason {
                    events.push(StreamEvent::MessageEnd {
                        stop_reason: Some(stop_reason),
                    });
                }
            }
        }
        "message_stop" => {
            // Final message stop - we may have already sent MessageEnd via message_delta
        }
        "ping" => {
            // Keepalive, ignore
        }
        "error" => {
            crate::logging::error(&format!("Anthropic stream error: {}", event.data));
            events.push(StreamEvent::Error {
                message: event.data.clone(),
                retry_after_secs: None,
            });
        }
        _ => {
            // Unknown event type, ignore
        }
    }

    events
}

// ============================================================================
// API Types
// ============================================================================

fn build_system_param(system: &str, is_oauth: bool) -> Option<ApiSystem> {
    jcode_provider_anthropic::build_system_param(system, is_oauth, is_cache_ttl_1h())
}

fn build_system_param_split(
    static_part: &str,
    dynamic_part: &str,
    is_oauth: bool,
) -> Option<ApiSystem> {
    jcode_provider_anthropic::build_system_param_split(
        static_part,
        dynamic_part,
        is_oauth,
        is_cache_ttl_1h(),
    )
}

fn format_messages_with_identity(messages: Vec<ApiMessage>, is_oauth: bool) -> Vec<ApiMessage> {
    jcode_provider_anthropic::format_messages_with_identity(messages, is_oauth, is_cache_ttl_1h())
}

#[cfg(test)]
fn add_message_cache_breakpoint(messages: &mut [ApiMessage]) {
    jcode_provider_anthropic::add_message_cache_breakpoint(messages, is_cache_ttl_1h())
}

// Response types for SSE parsing

#[derive(Deserialize)]
struct MessageStartEvent {
    message: MessageStartMessage,
}

#[derive(Deserialize)]
struct MessageStartMessage {
    usage: Option<UsageInfo>,
}

#[derive(Deserialize)]
struct ContentBlockStartEvent {
    #[serde(rename = "index")]
    _index: u32,
    content_block: ApiContentBlockStart,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ApiContentBlockStart {
    #[serde(rename = "text")]
    Text {
        #[serde(rename = "text")]
        _text: String,
    },
    #[serde(rename = "thinking")]
    Thinking {
        #[serde(default, rename = "thinking")]
        _thinking: String,
        #[serde(default, rename = "signature")]
        _signature: Option<String>,
    },
    #[serde(rename = "redacted_thinking")]
    RedactedThinking {
        #[serde(default, rename = "data")]
        _data: String,
    },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
}

#[derive(Deserialize)]
struct ContentBlockDeltaEvent {
    #[serde(rename = "index")]
    _index: u32,
    delta: ApiDelta,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ApiDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
    #[serde(rename = "signature_delta")]
    SignatureDelta {
        #[serde(rename = "signature")]
        signature: String,
    },
}

#[derive(Deserialize)]
struct MessageDeltaEvent {
    delta: MessageDeltaDelta,
    usage: Option<UsageInfo>,
}

#[derive(Deserialize)]
struct MessageDeltaDelta {
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct UsageInfo {
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
    cache_read_input_tokens: Option<u32>,
    cache_creation_input_tokens: Option<u32>,
    service_tier: Option<String>,
}

#[cfg(test)]
#[path = "anthropic_tests.rs"]
mod tests;
