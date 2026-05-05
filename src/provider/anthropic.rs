//! Direct Anthropic API provider
//!
//! Uses the Anthropic Messages API directly without the Python SDK.
//! This provides better control and eliminates the Python dependency.

use super::{EventStream, NativeToolResultSender, Provider};
use crate::auth;
use crate::auth::oauth;
use crate::message::{ContentBlock, Message, Role, StreamEvent, ToolDefinition};
use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{RwLock, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

static CACHE_TTL_1H: AtomicBool = AtomicBool::new(false);

/// Enable or disable the 1-hour cache TTL (default: 5-minute)
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

/// Claude Code CLI version - single source of truth for all version strings.
/// Update this constant when syncing with the official CLI version.
const CLAUDE_CLI_VERSION: &str = "2.1.123";

/// User-Agent for OAuth requests, matching the official Claude Code CLI.
pub(crate) fn claude_cli_user_agent() -> String {
    format!("claude-cli/{} (external, sdk-cli)", CLAUDE_CLI_VERSION)
}

/// Claude Code billing attribution text observed in the official CLI's system
/// prompt blocks.
pub(crate) fn oauth_billing_header() -> String {
    format!("cc_version={}; cc_entrypoint=sdk-cli; cch=33f85;", CLAUDE_CLI_VERSION)
}

/// Beta headers required for OAuth (tool-enabled Claude Code style)
pub(crate) const OAUTH_BETA_HEADERS: &str = "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14,context-management-2025-06-27,prompt-caching-scope-2026-01-05,advisor-tool-2026-03-01,advanced-tool-use-2025-11-20,effort-2025-11-24";

/// Beta headers with 1M context
const OAUTH_BETA_HEADERS_1M: &str = "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14,context-management-2025-06-27,prompt-caching-scope-2026-01-05,advisor-tool-2026-03-01,advanced-tool-use-2025-11-20,effort-2025-11-24,context-1m-2025-08-07";

/// Get the appropriate beta headers based on model
fn oauth_beta_headers(model: &str) -> &'static str {
    if is_1m_model(model) {
        OAUTH_BETA_HEADERS_1M
    } else {
        OAUTH_BETA_HEADERS
    }
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

fn stainless_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    }
}

fn stainless_os() -> &'static str {
    match std::env::consts::OS {
        "linux" => "Linux",
        "macos" => "MacOS",
        "windows" => "Windows",
        other => other,
    }
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
        reqwest::header::HeaderValue::from_str(&claude_cli_user_agent())
            .unwrap_or_else(|_| reqwest::header::HeaderValue::from_static("claude-cli/2.1.123")),
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

/// Check if a model name explicitly requests 1M context via suffix (e.g. "claude-opus-4-6[1m]")
fn is_1m_model(model: &str) -> bool {
    model.ends_with("[1m]")
}

/// Check if a model explicitly requests 1M context via the [1m] suffix.
pub fn effectively_1m(model: &str) -> bool {
    is_1m_model(model)
}

/// Strip the [1m] suffix to get the actual API model name
fn strip_1m_suffix(model: &str) -> &str {
    model.strip_suffix("[1m]").unwrap_or(model)
}

/// Default model
const DEFAULT_MODEL: &str = "claude-opus-4-6";

/// API version header
const API_VERSION: &str = "2023-06-01";

/// Claude Agent SDK identity block observed in the official Claude Code client.
const CLAUDE_CODE_IDENTITY: &str = "You are a Claude agent, built on Anthropic's Claude Agent SDK.";

fn map_tool_name_for_oauth(name: &str) -> String {
    match name {
        "bash" => "Bash",
        "read" => "Read",
        "write" => "Write",
        "edit" => "Edit",
        "glob" => "Glob",
        "grep" => "Grep",
        "subagent" => "Agent",
        "schedule" => "ScheduleWakeup",
        "skill_manage" => "Skill",
        _ => name,
    }
    .to_string()
}

fn map_tool_name_from_oauth(name: &str) -> String {
    match name {
        "Bash" => "bash",
        "Read" => "read",
        "Write" => "write",
        "Edit" => "edit",
        "Glob" => "glob",
        "Grep" => "grep",
        "Agent" => "subagent",
        "ScheduleWakeup" => "schedule",
        "Skill" => "skill_manage",
        // ToolSearch intentionally has no direct local analogue yet.
        _ => name,
    }
    .to_string()
}

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

/// Direct Anthropic API provider
pub struct AnthropicProvider {
    client: Client,
    model: Arc<std::sync::RwLock<String>>,
    /// Cached OAuth credentials (None if using API key)
    credentials: Arc<RwLock<Option<CachedCredentials>>>,
    max_tokens: u32,
    oauth_session_id: String,
    oauth_preflight_done: Arc<AtomicBool>,
}

impl AnthropicProvider {
    fn is_usage_exhausted() -> bool {
        let usage = crate::usage::get_sync();
        usage.five_hour >= 0.99 && usage.seven_day >= 0.99
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

        Self {
            client: crate::provider::shared_http_client(),
            model: Arc::new(std::sync::RwLock::new(model)),
            credentials: Arc::new(RwLock::new(None)),
            max_tokens,
            oauth_session_id: Uuid::new_v4().to_string(),
            oauth_preflight_done: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Get the access token from credentials
    /// Supports both OAuth tokens and direct API keys
    /// Automatically refreshes OAuth tokens when expired
    async fn get_access_token(&self) -> Result<(String, bool)> {
        // First check for direct API key in environment
        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            return Ok((key, false)); // false = not OAuth
        }

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

    /// Convert our Message type to Anthropic API format
    /// Also repairs dangling tool_uses by injecting synthetic tool_results
    fn format_messages(&self, messages: &[Message], is_oauth: bool) -> Vec<ApiMessage> {
        use std::collections::HashSet;

        // First pass: collect all tool_use IDs and tool_result IDs
        let mut tool_use_ids: HashSet<String> = HashSet::new();
        let mut tool_result_ids: HashSet<String> = HashSet::new();

        for msg in messages {
            for block in &msg.content {
                match block {
                    ContentBlock::ToolUse { id, .. } => {
                        tool_use_ids.insert(id.clone());
                    }
                    ContentBlock::ToolResult { tool_use_id, .. } => {
                        tool_result_ids.insert(tool_use_id.clone());
                    }
                    _ => {}
                }
            }
        }

        // Find dangling tool_uses (no matching tool_result)
        let dangling: HashSet<_> = tool_use_ids.difference(&tool_result_ids).cloned().collect();
        if !dangling.is_empty() {
            crate::logging::info(&format!(
                "[anthropic] Repairing {} dangling tool_use(s) by injecting synthetic tool_results",
                dangling.len()
            ));
        }

        // Second pass: build messages, injecting synthetic tool_results after assistant messages
        // that have dangling tool_uses
        let mut result: Vec<ApiMessage> = Vec::new();

        for msg in messages {
            let role = match msg.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            };

            let content = self.format_content_blocks(&msg.content, is_oauth);

            if !content.is_empty() {
                result.push(ApiMessage {
                    role: role.to_string(),
                    content,
                });
            }

            // If this is an assistant message with dangling tool_uses, inject synthetic results
            if matches!(msg.role, Role::Assistant) {
                let mut synthetic_results: Vec<ApiContentBlock> = Vec::new();
                for block in &msg.content {
                    if let ContentBlock::ToolUse { id, .. } = block
                        && dangling.contains(id)
                    {
                        synthetic_results.push(ApiContentBlock::ToolResult {
                            tool_use_id: crate::message::sanitize_tool_id(id),
                            content: ToolResultContent::Text(
                                "[Session interrupted before tool execution completed]".to_string(),
                            ),
                            is_error: true,
                        });
                    }
                }
                if !synthetic_results.is_empty() {
                    result.push(ApiMessage {
                        role: "user".to_string(),
                        content: synthetic_results,
                    });
                }
            }
        }

        // Third pass: merge consecutive messages of the same role
        // Anthropic API requires strictly alternating user/assistant messages
        let pre_merge_count = result.len();
        let mut merged: Vec<ApiMessage> = Vec::new();
        for msg in result {
            if let Some(last) = merged.last_mut()
                && last.role == msg.role
            {
                last.content.extend(msg.content);
                continue;
            }
            merged.push(msg);
        }

        if merged.len() != pre_merge_count {
            crate::logging::info(&format!(
                "[anthropic] Merged {} consecutive same-role messages",
                pre_merge_count - merged.len()
            ));
        }

        // Validate: check each assistant message with tool_use has matching tool_result in next user message
        for (i, msg) in merged.iter().enumerate() {
            if msg.role == "assistant" {
                let tool_uses: Vec<&String> = msg
                    .content
                    .iter()
                    .filter_map(|b| {
                        if let ApiContentBlock::ToolUse { id, .. } = b {
                            Some(id)
                        } else {
                            None
                        }
                    })
                    .collect();

                if !tool_uses.is_empty() {
                    // Check next message
                    if let Some(next) = merged.get(i + 1) {
                        if next.role != "user" {
                            crate::logging::warn(&format!(
                                "[anthropic] Message {} has tool_use but next message is {} (should be user)",
                                i, next.role
                            ));
                        } else {
                            let tool_results: std::collections::HashSet<&String> = next
                                .content
                                .iter()
                                .filter_map(|b| {
                                    if let ApiContentBlock::ToolResult { tool_use_id, .. } = b {
                                        Some(tool_use_id)
                                    } else {
                                        None
                                    }
                                })
                                .collect();

                            for tu_id in &tool_uses {
                                if !tool_results.contains(*tu_id) {
                                    crate::logging::warn(&format!(
                                        "[anthropic] Message {} has tool_use {} but no matching tool_result in message {}",
                                        i,
                                        tu_id,
                                        i + 1
                                    ));
                                }
                            }
                        }
                    } else {
                        crate::logging::warn(&format!(
                            "[anthropic] Message {} has tool_use but no next message",
                            i
                        ));
                    }
                }
            }
        }

        merged
    }

    /// Convert our ContentBlock to Anthropic API format
    fn format_content_blocks(
        &self,
        blocks: &[ContentBlock],
        is_oauth: bool,
    ) -> Vec<ApiContentBlock> {
        let mut result: Vec<ApiContentBlock> = Vec::new();
        for block in blocks {
            match block {
                ContentBlock::Text { text, .. } => {
                    result.push(ApiContentBlock::Text {
                        text: text.clone(),
                        cache_control: None,
                    });
                }
                ContentBlock::ToolUse { id, name, input } => {
                    result.push(ApiContentBlock::ToolUse {
                        id: crate::message::sanitize_tool_id(id),
                        name: if is_oauth {
                            map_tool_name_for_oauth(name)
                        } else {
                            name.clone()
                        },
                        input: if input.is_object() {
                            input.clone()
                        } else {
                            serde_json::json!({})
                        },
                        cache_control: None,
                    });
                }
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    result.push(ApiContentBlock::ToolResult {
                        tool_use_id: crate::message::sanitize_tool_id(tool_use_id),
                        content: ToolResultContent::Text(content.clone()),
                        is_error: is_error.unwrap_or(false),
                    });
                }
                ContentBlock::Image { media_type, data } => {
                    let img_block = ToolResultContentBlock::Image {
                        source: ApiImageSource {
                            kind: "base64".to_string(),
                            media_type: media_type.clone(),
                            data: data.clone(),
                        },
                    };
                    if let Some(ApiContentBlock::ToolResult { content, .. }) = result.last_mut() {
                        match content {
                            ToolResultContent::Text(text) => {
                                let text_block = ToolResultContentBlock::Text {
                                    text: std::mem::take(text),
                                };
                                *content = ToolResultContent::Blocks(vec![text_block, img_block]);
                            }
                            ToolResultContent::Blocks(blocks) => {
                                blocks.push(img_block);
                            }
                        }
                    } else {
                        result.push(ApiContentBlock::Image {
                            source: ApiImageSource {
                                kind: "base64".to_string(),
                                media_type: media_type.clone(),
                                data: data.clone(),
                            },
                        });
                    }
                }
                _ => {}
            }
        }
        result
    }

    /// Convert tool definitions to Anthropic API format
    /// Adds cache_control to the last tool for prompt caching
    fn format_tools(&self, tools: &[ToolDefinition], is_oauth: bool) -> Vec<ApiTool> {
        if is_oauth {
            return vec![
                ApiTool {
                    name: "Agent".to_string(),
                    description: "Launch a new agent to handle complex, multi-step tasks."
                        .to_string(),
                    input_schema: json!({"type":"object","properties":{"description":{"type":"string"},"prompt":{"type":"string"},"subagent_type":{"type":"string"},"run_in_background":{"type":"boolean"}},"required":["description","prompt"],"additionalProperties":false}),
                    cache_control: None,
                },
                ApiTool {
                    name: "Bash".to_string(),
                    description: "Executes a given bash command and returns its output."
                        .to_string(),
                    input_schema: json!({"type":"object","properties":{"command":{"type":"string"},"timeout":{"type":"integer"},"run_in_background":{"type":"boolean"}},"required":["command"],"additionalProperties":false}),
                    cache_control: None,
                },
                ApiTool {
                    name: "Edit".to_string(),
                    description: "Performs exact string replacements in files.".to_string(),
                    input_schema: json!({"type":"object","properties":{"file_path":{"type":"string"},"old_string":{"type":"string"},"new_string":{"type":"string"},"replace_all":{"type":"boolean","default":false}},"required":["file_path","old_string","new_string"],"additionalProperties":false}),
                    cache_control: None,
                },
                ApiTool {
                    name: "Glob".to_string(),
                    description: "Fast file pattern matching tool.".to_string(),
                    input_schema: json!({"type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string"}},"required":["pattern"],"additionalProperties":false}),
                    cache_control: None,
                },
                ApiTool {
                    name: "Grep".to_string(),
                    description: "A powerful search tool built on ripgrep.".to_string(),
                    input_schema: json!({"type":"object","properties":{"pattern":{"type":"string"},"path":{"type":"string"},"glob":{"type":"string"},"output_mode":{"type":"string","enum":["content","files_with_matches","count"]},"-B":{"type":"number"},"-A":{"type":"number"},"-C":{"type":"number"},"context":{"type":"number"},"-n":{"type":"boolean"},"-i":{"type":"boolean"},"type":{"type":"string"},"head_limit":{"type":"number"},"offset":{"type":"number"},"multiline":{"type":"boolean"}},"required":["pattern"],"additionalProperties":false}),
                    cache_control: None,
                },
                ApiTool {
                    name: "Read".to_string(),
                    description: "Reads a file from the local filesystem.".to_string(),
                    input_schema: json!({"type":"object","properties":{"file_path":{"type":"string"},"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","exclusiveMinimum":0},"pages":{"type":"string"}},"required":["file_path"],"additionalProperties":false}),
                    cache_control: None,
                },
                ApiTool {
                    name: "ScheduleWakeup".to_string(),
                    description: "Schedule when to resume work in /loop dynamic mode.".to_string(),
                    input_schema: json!({"type":"object","properties":{"delaySeconds":{"type":"number"},"reason":{"type":"string"},"prompt":{"type":"string"}},"required":["delaySeconds","reason","prompt"],"additionalProperties":false}),
                    cache_control: None,
                },
                ApiTool {
                    name: "Skill".to_string(),
                    description: "Execute a skill within the main conversation".to_string(),
                    input_schema: json!({"type":"object","properties":{"skill":{"type":"string"},"args":{"type":"string"}},"required":["skill"],"additionalProperties":false}),
                    cache_control: None,
                },
                ApiTool {
                    name: "ToolSearch".to_string(),
                    description:
                        "Fetches full schema definitions for deferred tools so they can be called."
                            .to_string(),
                    input_schema: json!({"type":"object","properties":{"query":{"type":"string"},"max_results":{"type":"number","default":5}},"required":["query","max_results"],"additionalProperties":false}),
                    cache_control: None,
                },
                ApiTool {
                    name: "Write".to_string(),
                    description: "Writes a file to the local filesystem.".to_string(),
                    input_schema: json!({"type":"object","properties":{"file_path":{"type":"string"},"content":{"type":"string"}},"required":["file_path","content"],"additionalProperties":false}),
                    cache_control: Some(CacheControlParam::ephemeral()),
                },
            ];
        }

        let len = tools.len();
        tools
            .iter()
            .enumerate()
            .map(|(i, tool)| ApiTool {
                name: tool.name.clone(),
                description: tool.description.clone(),
                input_schema: tool.input_schema.clone(),
                cache_control: if i == len - 1 {
                    Some(CacheControlParam::ephemeral())
                } else {
                    None
                },
            })
            .collect()
    }
}

impl Default for AnthropicProvider {
    fn default() -> Self {
        Self::new()
    }
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
            temperature: if is_oauth { Some(1.0) } else { None },
            stream: true,
        };

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
            credentials: Arc::new(RwLock::new(None)),
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
            temperature: if is_oauth { Some(1.0) } else { None },
            stream: true,
        };

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
        req = apply_oauth_attribution_headers(
            req.header("Authorization", format!("Bearer {}", token))
                .header("User-Agent", claude_cli_user_agent())
                .header("anthropic-beta", oauth_beta_headers(model_name)),
            oauth_session_id,
        );
    } else {
        // Direct API keys use x-api-key
        // Include prompt-caching beta header
        req = req.header("x-api-key", &token).header(
            "anthropic-beta",
            if is_1m_model(model_name) {
                "prompt-caching-2024-07-31,context-1m-2025-08-07"
            } else {
                "prompt-caching-2024-07-31"
            },
        );
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

        // Prevent unbounded buffer growth from malformed SSE streams
        const MAX_SSE_BUFFER_SIZE: usize = 10 * 1024 * 1024; // 10 MB
        if buffer.len() > MAX_SSE_BUFFER_SIZE {
            // Try to find the last complete event boundary and trim
            if let Some(last_boundary) = buffer.rfind("\n\n") {
                let remaining = buffer.split_off(last_boundary + 2);
                buffer = remaining;
            } else {
                anyhow::bail!("SSE buffer exceeded {} bytes without complete event boundary", MAX_SSE_BUFFER_SIZE);
            }
        }

        // Process complete SSE events
        while let Some(event) = parse_sse_event(&mut buffer) {
            let events = process_sse_event(
                &event,
                &mut current_tool_use,
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
            }
        }
        "content_block_start" => {
            if let Ok(parsed) = serde_json::from_str::<ContentBlockStartEvent>(&event.data) {
                match parsed.content_block {
                    ApiContentBlockStart::Text { .. } => {
                        // Text block starting - nothing to emit yet
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
                }
            }
        }
        "content_block_stop" => {
            // If we were accumulating a tool_use, it's complete now
            if current_tool_use.take().is_some() {
                events.push(StreamEvent::ToolUseEnd);
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

#[derive(Serialize, Clone)]
struct ApiRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<ApiSystem>,
    messages: Vec<ApiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ApiTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<ApiMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    stream: bool,
}

#[derive(Serialize, Clone)]
struct ApiMetadata {
    user_id: String,
}

#[derive(Serialize, Clone)]
#[serde(untagged)]
enum ApiSystem {
    Blocks(Vec<ApiSystemBlock>),
}

/// Cache control for prompt caching
#[derive(Serialize, Clone)]
struct CacheControlParam {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    ttl: Option<&'static str>,
}

impl CacheControlParam {
    fn ephemeral() -> Self {
        if is_cache_ttl_1h() {
            Self::ephemeral_1h()
        } else {
            Self {
                kind: "ephemeral",
                ttl: None,
            }
        }
    }

    fn ephemeral_1h() -> Self {
        Self {
            kind: "ephemeral",
            ttl: Some("1h"),
        }
    }
}

#[derive(Serialize, Clone)]
struct ApiSystemBlock {
    #[serde(rename = "type")]
    block_type: &'static str,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControlParam>,
}

fn build_system_param(system: &str, is_oauth: bool) -> Option<ApiSystem> {
    build_system_param_split(system, "", is_oauth)
}

/// Build system param with split static/dynamic content for better caching
fn build_system_param_split(
    static_part: &str,
    dynamic_part: &str,
    is_oauth: bool,
) -> Option<ApiSystem> {
    if is_oauth {
        let mut blocks = Vec::new();
        blocks.push(ApiSystemBlock {
            block_type: "text",
            text: format!("x-anthropic-billing-header: {}", oauth_billing_header()),
            cache_control: None,
        });
        blocks.push(ApiSystemBlock {
            block_type: "text",
            text: CLAUDE_CODE_IDENTITY.to_string(),
            cache_control: None,
        });
        // Static content - CACHED (instruction files, base prompt, skills)
        if !static_part.is_empty() {
            blocks.push(ApiSystemBlock {
                block_type: "text",
                text: static_part.to_string(),
                cache_control: Some(CacheControlParam::ephemeral()),
            });
        }
        // Dynamic content - NOT cached (date, git status, memory)
        if !dynamic_part.is_empty() {
            blocks.push(ApiSystemBlock {
                block_type: "text",
                text: dynamic_part.to_string(),
                cache_control: None,
            });
        }
        return Some(ApiSystem::Blocks(blocks));
    }

    // Non-OAuth: use block format with cache control for static part only
    let has_static = !static_part.is_empty();
    let has_dynamic = !dynamic_part.is_empty();

    if !has_static && !has_dynamic {
        None
    } else {
        let mut blocks = Vec::new();
        if has_static {
            blocks.push(ApiSystemBlock {
                block_type: "text",
                text: static_part.to_string(),
                cache_control: Some(CacheControlParam::ephemeral()),
            });
        }
        if has_dynamic {
            blocks.push(ApiSystemBlock {
                block_type: "text",
                text: dynamic_part.to_string(),
                cache_control: None,
            });
        }
        Some(ApiSystem::Blocks(blocks))
    }
}

fn format_messages_with_identity(messages: Vec<ApiMessage>, _is_oauth: bool) -> Vec<ApiMessage> {
    let mut out = messages;

    // Add cache breakpoints for both OAuth and non-OAuth paths
    add_message_cache_breakpoint(&mut out);

    out
}

/// Add cache_control to messages for conversation caching.
///
/// Strategy: sliding two-marker window
///   - Second-to-last assistant message → READ marker (re-uses cache snapshot from previous turn)
///   - Last assistant message           → WRITE marker (creates new snapshot for the next turn)
///
/// This ensures each turn N+1 reads from turn N's conversation cache, paying only
/// cache_read_input_tokens for the already-cached history instead of full input tokens.
///
/// Budget: system (1) + tools (1) + messages (up to 2) = 4 total, within Anthropic's limit.
fn add_message_cache_breakpoint(messages: &mut [ApiMessage]) {
    crate::logging::info(&format!(
        "Conversation caching: {} messages to process",
        messages.len()
    ));

    if messages.len() < 3 {
        // Need at least: user + assistant + user to be worth caching
        crate::logging::info("Conversation caching: too few messages, skipping");
        return;
    }

    // Collect indices of up to 2 most recent assistant messages (newest first)
    let mut assistant_indices: Vec<usize> = Vec::with_capacity(2);
    for (i, msg) in messages.iter().enumerate().rev() {
        if msg.role == "assistant" {
            assistant_indices.push(i);
            if assistant_indices.len() == 2 {
                break;
            }
        }
    }

    if assistant_indices.is_empty() {
        crate::logging::info("Conversation caching: no assistant message found");
        return;
    }

    // Place cache_control on both (newest = WRITE for next turn, older = READ from prev turn)
    let total = assistant_indices.len();
    for (slot, &idx) in assistant_indices.iter().enumerate() {
        let label = if slot == 0 {
            "WRITE (newest)"
        } else {
            "READ (prev-turn)"
        };
        let mut added = false;
        if let Some(msg) = messages.get_mut(idx) {
            for block in msg.content.iter_mut().rev() {
                match block {
                    ApiContentBlock::Text { cache_control, .. }
                    | ApiContentBlock::ToolUse { cache_control, .. } => {
                        *cache_control = Some(CacheControlParam::ephemeral());
                        added = true;
                        break;
                    }
                    _ => {}
                }
            }
        }
        if added {
            crate::logging::info(&format!(
                "Conversation caching: breakpoint {}/{} at message {} [{}]",
                slot + 1,
                total,
                idx,
                label
            ));
        } else {
            crate::logging::info(&format!(
                "Conversation caching: no cacheable block in assistant message {} [{}]",
                idx, label
            ));
        }
    }
}

#[derive(Serialize, Clone)]
struct ApiMessage {
    role: String,
    content: Vec<ApiContentBlock>,
}

#[derive(Serialize, Clone)]
#[serde(tag = "type")]
enum ApiContentBlock {
    #[serde(rename = "text")]
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControlParam>,
    },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControlParam>,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: ToolResultContent,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },
    #[serde(rename = "image")]
    Image { source: ApiImageSource },
}

#[derive(Serialize, Clone)]
#[serde(untagged)]
enum ToolResultContent {
    Text(String),
    Blocks(Vec<ToolResultContentBlock>),
}

#[derive(Serialize, Clone)]
#[serde(tag = "type")]
enum ToolResultContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { source: ApiImageSource },
}

#[derive(Serialize, Clone)]
struct ApiImageSource {
    #[serde(rename = "type")]
    kind: String,
    media_type: String,
    data: String,
}

#[derive(Serialize, Clone)]
struct ApiTool {
    name: String,
    description: String,
    input_schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<CacheControlParam>,
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
}

#[cfg(test)]
#[path = "anthropic_tests.rs"]
mod tests;
