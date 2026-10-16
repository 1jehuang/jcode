//! Lightweight sidecar client for fast, cheap model calls.
//!
//! Used for memory relevance verification and other quick tasks that don't
//! need the full Agent SDK infrastructure.
//!
//! Automatically selects the best available backend:
//! - OpenAI (gpt-5.6-luna, reasoning=none) if Codex credentials are available
//! - Claude (claude-haiku-4-5-20241022) if Claude credentials are available

use crate::auth;
use anyhow::{Context, Result};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

/// Fast/cheap OpenAI model used when Codex credentials are available.
pub const SIDECAR_OPENAI_MODEL: &str = "gpt-5.6-luna";
const SIDECAR_OPENAI_REASONING: &str = "none";
const SIDECAR_OPENAI_OAUTH_FALLBACK_MODEL: &str = "gpt-5.4";
const SIDECAR_OPENAI_OAUTH_FALLBACK_REASONING: &str = "low";

/// Fast/cheap Claude model used when only Claude credentials are available.
const SIDECAR_CLAUDE_MODEL: &str = "claude-haiku-4-5-20251001";

/// OpenAI Responses API
const OPENAI_API_BASE: &str = "https://api.openai.com/v1";
const CHATGPT_API_BASE: &str = "https://chatgpt.com/backend-api/codex";
const OPENAI_RESPONSES_PATH: &str = "responses";
const OPENAI_ORIGINATOR: &str = "codex_cli_rs";

/// Claude Messages API endpoint (with beta=true for OAuth)
const CLAUDE_API_URL: &str = "https://api.anthropic.com/v1/messages?beta=true";

/// Claude Messages API endpoint for direct API-key access (no OAuth beta flag).
const CLAUDE_API_KEY_URL: &str = "https://api.anthropic.com/v1/messages";

/// Beta headers required for OAuth. When `agents.memory_effort` adds
/// `output_config` effort or manual thinking, the request also needs the
/// effort/thinking betas (mirrors the runtime's header handling).
const OAUTH_BETA_HEADERS: &str =
    "oauth-2025-04-20,claude-code-20250219,effort-2025-11-24,interleaved-thinking-2025-05-14";

/// Claude Code identity block required for OAuth direct API access
const CLAUDE_CODE_IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";
const CLAUDE_CODE_JCODE_NOTICE: &str = "You are jcode, powered by Claude Code. You are a third-party CLI, not the official Claude Code CLI.";

/// Maximum tokens for sidecar responses (keep small for speed/cost)
const DEFAULT_MAX_TOKENS: u32 = 1024;
const CLAUDE_MIN_THINKING_BUDGET: u32 = 1_024;
const CLAUDE_THINKING_ANSWER_HEADROOM: u32 = 2_048;

/// Whether retrying a failed sidecar request can reasonably succeed without a
/// configuration or credential change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidecarErrorKind {
    Transient,
    Permanent,
}

#[derive(Debug)]
struct SidecarHttpError {
    provider: &'static str,
    status: StatusCode,
    body: String,
}

impl fmt::Display for SidecarHttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} API error ({}): {}",
            self.provider, self.status, self.body
        )
    }
}

impl std::error::Error for SidecarHttpError {}

/// Classify a sidecar failure for retry policy. HTTP client/auth/request errors
/// are permanent; throttling, server failures, and transport failures are
/// transient. Unknown provider errors retain the conservative retry behavior.
pub fn classify_error(error: &anyhow::Error) -> SidecarErrorKind {
    if let Some(error) = error.downcast_ref::<SidecarHttpError>() {
        return classify_http_status(error.status);
    }
    for cause in error.chain() {
        if let Some(error) = cause.downcast_ref::<reqwest::Error>() {
            if let Some(status) = error.status() {
                return classify_http_status(status);
            }
            return SidecarErrorKind::Transient;
        }
    }

    // Provider-backed sidecars may not expose a typed HTTP error yet.
    let message = error.to_string().to_ascii_lowercase();
    if [
        "400",
        "401",
        "403",
        "404",
        "bad request",
        "unauthorized",
        "forbidden",
        "not_found_error",
    ]
    .iter()
    .any(|needle| message.contains(needle))
    {
        SidecarErrorKind::Permanent
    } else {
        SidecarErrorKind::Transient
    }
}

fn classify_http_status(status: StatusCode) -> SidecarErrorKind {
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        SidecarErrorKind::Transient
    } else if status.is_client_error() {
        SidecarErrorKind::Permanent
    } else {
        SidecarErrorKind::Transient
    }
}

/// Which backend the sidecar is using
#[derive(Debug, Clone, Copy, PartialEq)]
enum SidecarBackend {
    OpenAI,
    Claude,
    /// Dispatch through the live agent provider (`crate::provider::active_provider_fork`).
    /// Used when neither OpenAI nor Claude OAuth credentials are present but the
    /// user is running on another provider (Copilot, Antigravity, Gemini,
    /// Cursor, Bedrock, OpenRouter). This is what makes the memory sidecar work
    /// on ALL providers instead of only the two with dedicated HTTP clients.
    Provider,
}

/// Lightweight client for fast sidecar calls
#[derive(Clone)]
pub struct Sidecar {
    client: reqwest::Client,
    model: String,
    max_tokens: u32,
    backend: SidecarBackend,
    /// Provider snapshot selected with the model. Keeping the fork here avoids
    /// re-resolving a mutable global provider between sidecar construction and
    /// dispatch, which can lose an OpenAI-compatible profile and fall back to
    /// the OpenRouter default model.
    provider: Option<Arc<dyn crate::provider::Provider>>,
    /// Optional explicit reasoning effort override (OpenAI Responses API).
    /// When `Some`, this effort is always sent; when `None`, the default
    /// per-model behavior applies. Used by the memory benchmark to pin
    /// GPT-5.5 with no thinking.
    reasoning_override: Option<String>,
}

impl Sidecar {
    /// Create a new sidecar client, auto-selecting the best available backend.
    /// Prefers OpenAI (GPT-5.6 Luna with no reasoning) if creds exist, falls back to Claude.
    pub fn new() -> Self {
        let configured_model = crate::config::config().agents.memory_model.clone();
        // An empty/whitespace effort from the config file (the env override
        // already trims) must mean "unset", not an invalid `""` effort pin.
        // Surrounding spaces would flow into the request as `" low "`, so trim
        // before storing: the OpenAI pin and the Claude budget lookup both
        // match exact strings.
        let configured_effort = crate::config::config()
            .agents
            .memory_effort
            .clone()
            .map(|e| e.trim().to_string())
            .filter(|e| !e.is_empty());
        Self::with_configured_model(configured_model, configured_effort)
    }

    pub fn for_session(session: &crate::session::Session) -> Self {
        let model = session
            .effective_agent_model("memory", crate::config::Config::load().agents.memory_model);
        let inherit = model.as_deref() == Some("inherit");
        let model = if inherit {
            session.model.clone()
        } else {
            model
        };
        // Read the configured effort up front: the provider fork branch below
        // returns early, and skipping this read there meant `for_session`
        // ignored `agents.memory_effort` whenever a live provider could serve
        // the configured model (greptile). The fork pin happens after the
        // fork's model is selected, so `set_model_with_auth_refresh` never
        // clobbers the effort.
        let configured_effort = crate::config::Config::load()
            .agents
            .memory_effort
            .clone()
            .map(|e| e.trim().to_string())
            .filter(|e| !e.is_empty());
        if let Some(model) = model.as_deref()
            && let Some(provider) = crate::provider::active_provider_fork()
        {
            let request = if inherit {
                crate::provider::MultiProvider::model_switch_request_for_session_route(
                    model,
                    session.provider_key.as_deref(),
                    session.route_api_method.as_deref(),
                )
            } else {
                model.to_string()
            };
            if crate::provider::set_model_with_auth_refresh(provider.as_ref(), &request).is_ok() {
                // The fork is independent of the main agent (per the Provider
                // fork contract), so pinning `agents.memory_effort` on it
                // never disturbs the user's live session effort. Providers
                // without an effort knob reject; keep their default.
                if let Some(effort) = configured_effort.as_deref()
                    && let Err(err) = provider.set_reasoning_effort(effort)
                {
                    crate::logging::warn(&format!(
                        "Ignoring memory effort '{effort}' for provider sidecar: {err}"
                    ));
                }
                return Self {
                    client: crate::provider::shared_http_client(),
                    model: provider.model(),
                    max_tokens: DEFAULT_MAX_TOKENS,
                    backend: SidecarBackend::Provider,
                    provider: Some(provider),
                    reasoning_override: configured_effort,
                };
            }
        }
        // No live routing provider: retain extraction auto-selection as fallback
        // for unsupported coordinator models and strip transport prefixes for
        // the dedicated OpenAI/Claude clients.
        let model = model.map(|model| {
            model
                .split_once(':')
                .map(|(_, id)| id.to_string())
                .unwrap_or(model)
        });
        Self::with_configured_model(model, configured_effort)
    }

    fn with_configured_model(
        configured_model: Option<String>,
        configured_effort: Option<String>,
    ) -> Self {
        let (backend, model, provider) = if let Some(model) = configured_model {
            match crate::provider::provider_for_model(&model) {
                Some("openai") => (SidecarBackend::OpenAI, model, None),
                Some("claude") => (SidecarBackend::Claude, model, None),
                _ => {
                    crate::logging::warn(&format!(
                        "Ignoring unsupported memory sidecar model override '{}'; expected an OpenAI or Claude model",
                        model
                    ));
                    Self::auto_select_backend()
                }
            }
        } else {
            Self::auto_select_backend()
        };

        Self {
            client: crate::provider::shared_http_client(),
            model,
            max_tokens: DEFAULT_MAX_TOKENS,
            backend,
            provider: provider.inspect(|fork| {
                // The fork is independent of the main agent (per the Provider
                // fork contract), so pinning `agents.memory_effort` on it
                // never disturbs the user's live session effort. Providers
                // without an effort knob reject; keep their default.
                if let Some(effort) = configured_effort.as_deref()
                    && let Err(err) = fork.set_reasoning_effort(effort)
                {
                    crate::logging::warn(&format!(
                        "Ignoring memory effort '{effort}' for provider sidecar: {err}"
                    ));
                }
            }),
            // `agents.memory_effort` pins the reasoning effort on the OpenAI
            // sidecar path; the Claude path derives its thinking budget from
            // it per request (see `claude_reasoning_parts`).
            reasoning_override: configured_effort,
        }
    }

    /// Pick the best available sidecar backend.
    ///
    /// Preference order:
    /// 1. OpenAI GPT-5.6 Luna at reasoning=none if Codex creds exist.
    /// 2. Claude haiku (dedicated fast/cheap OAuth path) if Claude creds exist.
    /// 3. The live agent provider (works for EVERY provider jcode supports:
    ///    Copilot, Antigravity, Gemini, Cursor, Bedrock, OpenRouter, and even
    ///    OpenAI/Claude API-key setups), dispatched via `complete_simple`.
    ///
    /// Only when no provider is registered at all do we fall back to Claude,
    /// which then fails on use with a clear credentials error.
    fn auto_select_backend() -> (
        SidecarBackend,
        String,
        Option<Arc<dyn crate::provider::Provider>>,
    ) {
        if auth::codex::load_credentials().is_ok() {
            (
                SidecarBackend::OpenAI,
                SIDECAR_OPENAI_MODEL.to_string(),
                None,
            )
        } else if auth::claude::load_credentials().is_ok() {
            (
                SidecarBackend::Claude,
                SIDECAR_CLAUDE_MODEL.to_string(),
                None,
            )
        } else if let Some(provider) = crate::provider::active_provider_fork() {
            // Dispatch through whatever provider the user is running on. The
            // model string is informational here; the provider already has the
            // user's selected model and routes accordingly.
            (SidecarBackend::Provider, provider.model(), Some(provider))
        } else {
            // No credentials and no live provider: default to Claude so the
            // eventual error message is actionable.
            (
                SidecarBackend::Claude,
                SIDECAR_CLAUDE_MODEL.to_string(),
                None,
            )
        }
    }

    /// Whether a usable LLM backend is actually reachable for the sidecar right
    /// now. Unlike [`Sidecar::auto_select_backend`] this does NOT fall back to a
    /// Claude placeholder when nothing is logged in: it returns `true` only when
    /// real Codex/Claude credentials exist or a live agent provider is
    /// registered.
    ///
    /// Re-evaluated live (reads credentials/provider state on each call) so that
    /// adding or removing a login is reflected without a restart. This is the
    /// signal the memory system uses to decide whether the LLM precision judge
    /// can run; if it returns `false`, memory's sidecar mode is treated as
    /// unavailable rather than silently degrading to the no-LLM path.
    pub fn llm_backend_available() -> bool {
        auth::codex::load_credentials().is_ok()
            || auth::claude::load_credentials().is_ok()
            || crate::provider::active_provider_fork().is_some()
    }

    /// Return the currently selected sidecar model name.
    pub fn model_name(&self) -> &str {
        &self.model
    }

    /// Construct a sidecar pinned to a specific Claude model (used by the
    /// memory recall benchmark judge so the relevance labels come from a strong,
    /// fixed model regardless of the user's configured memory model).
    pub fn with_claude_model(model: impl Into<String>) -> Self {
        Self {
            client: crate::provider::shared_http_client(),
            model: model.into(),
            max_tokens: DEFAULT_MAX_TOKENS,
            backend: SidecarBackend::Claude,
            provider: None,
            reasoning_override: None,
        }
    }

    /// Construct a sidecar pinned to a specific OpenAI model via Codex/OpenAI
    /// OAuth, with an optional explicit reasoning effort (e.g. "none"/"minimal"
    /// for no-thinking). Used by the memory recall benchmark judge.
    pub fn with_openai_model(model: impl Into<String>, reasoning_effort: Option<String>) -> Self {
        Self {
            client: crate::provider::shared_http_client(),
            model: model.into(),
            max_tokens: DEFAULT_MAX_TOKENS,
            backend: SidecarBackend::OpenAI,
            provider: None,
            reasoning_override: reasoning_effort,
        }
    }

    /// Return the currently selected backend label.
    pub fn backend_name(&self) -> &'static str {
        match self.backend {
            SidecarBackend::OpenAI => "openai",
            SidecarBackend::Claude => "claude",
            SidecarBackend::Provider => "provider",
        }
    }

    /// Simple completion - send a prompt, get a response.
    /// Routes to the correct API based on the detected backend.
    pub async fn complete(&self, system: &str, user_message: &str) -> Result<String> {
        match self.backend {
            SidecarBackend::OpenAI => self.complete_openai(system, user_message).await,
            SidecarBackend::Claude => self.complete_claude(system, user_message).await,
            SidecarBackend::Provider => self.complete_via_provider(system, user_message).await,
        }
    }

    /// Complete via the live agent provider (`complete_simple`).
    ///
    /// This is the universal path: it works for every provider jcode supports,
    /// because `complete_simple` is a default method on the `Provider` trait that
    /// collects the streamed `TextDelta`s into a single string. The provider was
    /// forked at construction time, so it carries the user's selected model.
    async fn complete_via_provider(&self, system: &str, user_message: &str) -> Result<String> {
        let provider = self.provider.as_ref().context(
            "No active provider registered for sidecar; memory features require a logged-in provider",
        )?;
        let (text, usage) = provider
            .complete_simple_with_usage(user_message, system)
            .await
            .context("Sidecar completion via active provider failed")?;
        crate::telemetry::record_simple_completion_usage(
            None,
            provider.name(),
            &provider.model(),
            crate::telemetry::UsageSource::Sidecar,
            usage,
        );
        Ok(text)
    }

    /// Complete via OpenAI Responses API.
    ///
    /// - Direct API key mode: non-streaming, simple JSON response.
    /// - ChatGPT OAuth mode: streaming SSE (required by chatgpt.com endpoint).
    ///   Prefer codex-spark there too, but fall back to GPT-5.4 with low
    ///   reasoning if spark is unavailable for the current account.
    async fn complete_openai(&self, system: &str, user_message: &str) -> Result<String> {
        let creds = auth::codex::load_credentials()
            .context("Failed to load OpenAI/Codex credentials for sidecar")?;

        let is_chatgpt_mode = !creds.refresh_token.is_empty() || creds.id_token.is_some();
        let base = openai_responses_base(is_chatgpt_mode);
        let url = format!("{}/{}", base.trim_end_matches('/'), OPENAI_RESPONSES_PATH);

        let (primary_model, resolved_reasoning) =
            resolve_openai_request_model(&self.model, is_chatgpt_mode);
        // An explicit reasoning override (e.g. benchmark judge pinning GPT-5.5
        // to no-thinking) always wins over the per-model default.
        let primary_reasoning: Option<&str> =
            self.reasoning_override.as_deref().or(resolved_reasoning);

        match self
            .complete_openai_with_model(
                &url,
                creds.access_token.as_str(),
                creds.account_id.as_deref(),
                is_chatgpt_mode,
                system,
                user_message,
                primary_model,
                primary_reasoning,
            )
            .await
        {
            Ok(text) => {
                crate::provider::clear_model_unavailable_for_account(primary_model);
                Ok(text)
            }
            Err(OpenAiSidecarError::Api { status, body })
                if is_chatgpt_mode
                    && primary_model == SIDECAR_OPENAI_MODEL
                    && is_openai_model_unavailable(status, &body) =>
            {
                let reason = classify_openai_model_unavailable(status, &body)
                    .unwrap_or_else(|| format!("model denied by OpenAI API (status {})", status));
                crate::provider::record_model_unavailable_for_account(primary_model, &reason);
                crate::logging::info(&format!(
                    "Sidecar fallback: {} unavailable in ChatGPT OAuth mode; retrying {} with reasoning={} ({})",
                    primary_model,
                    SIDECAR_OPENAI_OAUTH_FALLBACK_MODEL,
                    SIDECAR_OPENAI_OAUTH_FALLBACK_REASONING,
                    reason
                ));

                // The retry keeps the configured effort (`agents.memory_effort`):
                // an override must survive the fallback, and with `none`
                // configured the replacement request must not start thinking.
                // Only without an override does the fallback model keep its
                // own low-effort default.
                let fallback_reasoning: Option<&str> = self
                    .reasoning_override
                    .as_deref()
                    .or(Some(SIDECAR_OPENAI_OAUTH_FALLBACK_REASONING));
                let fallback = self
                    .complete_openai_with_model(
                        &url,
                        creds.access_token.as_str(),
                        creds.account_id.as_deref(),
                        is_chatgpt_mode,
                        system,
                        user_message,
                        SIDECAR_OPENAI_OAUTH_FALLBACK_MODEL,
                        fallback_reasoning,
                    )
                    .await;

                match fallback {
                    Ok(text) => {
                        crate::provider::clear_model_unavailable_for_account(
                            SIDECAR_OPENAI_OAUTH_FALLBACK_MODEL,
                        );
                        Ok(text)
                    }
                    Err(OpenAiSidecarError::Api { status, body })
                        if is_openai_model_unavailable(status, &body)
                            && auth::claude::load_credentials().is_ok() =>
                    {
                        // Both GPT-5.6 Luna and the gpt-5.4 OAuth
                        // fallback are denied for this ChatGPT account. Rather
                        // than dead-end the sidecar, fall back to Claude haiku
                        // when Claude credentials are available.
                        let reason = classify_openai_model_unavailable(status, &body)
                            .unwrap_or_else(|| {
                                format!("model denied by OpenAI API (status {})", status)
                            });
                        crate::provider::record_model_unavailable_for_account(
                            SIDECAR_OPENAI_OAUTH_FALLBACK_MODEL,
                            &reason,
                        );
                        crate::logging::info(&format!(
                            "Sidecar fallback: {} also unavailable in ChatGPT OAuth mode; falling back to Claude {} ({})",
                            SIDECAR_OPENAI_OAUTH_FALLBACK_MODEL, SIDECAR_CLAUDE_MODEL, reason
                        ));
                        let claude = Self {
                            client: self.client.clone(),
                            model: SIDECAR_CLAUDE_MODEL.to_string(),
                            max_tokens: self.max_tokens,
                            backend: SidecarBackend::Claude,
                            provider: None,
                            reasoning_override: None,
                        };
                        claude.complete_claude(system, user_message).await
                    }
                    Err(err) => Err(err.into_anyhow()),
                }
            }
            Err(err) => Err(err.into_anyhow()),
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "OpenAI sidecar call needs endpoint, auth, account, mode, prompts, model, and reasoning effort"
    )]
    async fn complete_openai_with_model(
        &self,
        url: &str,
        access_token: &str,
        account_id: Option<&str>,
        is_chatgpt_mode: bool,
        system: &str,
        user_message: &str,
        model: &str,
        reasoning_effort: Option<&str>,
    ) -> std::result::Result<String, OpenAiSidecarError> {
        let request = build_openai_request(
            model,
            system,
            user_message,
            is_chatgpt_mode,
            reasoning_effort,
        );

        let mut builder = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json");

        if is_chatgpt_mode {
            builder = builder.header("originator", OPENAI_ORIGINATOR);
            if let Some(account_id) = account_id {
                builder = builder.header("chatgpt-account-id", account_id);
            }
        }

        let response = builder
            .json(&request)
            .send()
            .await
            .context("Failed to send request to OpenAI API")
            .map_err(OpenAiSidecarError::other)?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(OpenAiSidecarError::Api { status, body });
        }

        if is_chatgpt_mode {
            collect_openai_sse_text(response, model)
                .await
                .map_err(OpenAiSidecarError::other)
        } else {
            let result: serde_json::Value = response
                .json()
                .await
                .context("Failed to parse OpenAI API response")
                .map_err(OpenAiSidecarError::other)?;
            record_openai_sidecar_usage(model, result.get("usage"));
            extract_openai_response_text(&result).map_err(OpenAiSidecarError::other)
        }
    }

    /// Complete via Claude Messages API
    async fn complete_claude(&self, system: &str, user_message: &str) -> Result<String> {
        // Respect the runtime's pinned Anthropic credential mode. The main agent
        // may be running in API-key mode (`claude-api`), where the org forbids
        // OAuth and Anthropic returns a 403 "OAuth authentication is currently
        // not allowed for this organization." The sidecar previously hardcoded
        // the OAuth path, so memory calls (consensus judge, extraction) failed
        // even though the main agent worked fine on the API key. Mirror the main
        // provider's resolution: use the direct API key when API-key mode is
        // pinned (or when no OAuth credentials exist but a key does), and fall
        // back to the API key if an OAuth request is rejected as forbidden.
        if anthropic_sidecar_prefers_api_key()
            && let Ok(key) = crate::provider::anthropic::load_anthropic_api_key()
        {
            return self
                .complete_claude_api_key(system, user_message, &key)
                .await;
        }

        match self.complete_claude_oauth(system, user_message).await {
            Ok(text) => Ok(text),
            Err(err) if is_anthropic_oauth_forbidden(&err) => {
                match crate::provider::anthropic::load_anthropic_api_key() {
                    Ok(key) => {
                        crate::logging::info(
                            "Sidecar Claude: OAuth forbidden for organization; falling back to API key",
                        );
                        self.complete_claude_api_key(system, user_message, &key)
                            .await
                    }
                    Err(_) => Err(err),
                }
            }
            Err(err) => Err(err),
        }
    }

    /// OAuth (Claude subscription) completion path.
    async fn complete_claude_oauth(&self, system: &str, user_message: &str) -> Result<String> {
        let creds = auth::claude::load_credentials()
            .context("Failed to load Claude credentials for sidecar")?;

        let (thinking, output_config, request_max_tokens) = claude_reasoning_parts(
            &self.model,
            self.reasoning_override.as_deref(),
            self.max_tokens,
        );
        let request = ClaudeMessagesRequest {
            model: &self.model,
            max_tokens: request_max_tokens,
            system: build_claude_system_param(system),
            messages: vec![ClaudeMessage {
                role: "user",
                content: user_message,
            }],
            thinking,
            output_config,
        };

        let response = crate::provider::anthropic::apply_oauth_attribution_headers(
            self.client
                .post(CLAUDE_API_URL)
                .header("Authorization", format!("Bearer {}", creds.access_token))
                .header(
                    "User-Agent",
                    crate::provider::anthropic::CLAUDE_CLI_USER_AGENT,
                )
                .header("anthropic-version", "2023-06-01")
                .header("anthropic-beta", OAUTH_BETA_HEADERS)
                .header("content-type", "application/json")
                .json(&request),
            &crate::provider::anthropic::new_oauth_request_id(),
        )
        .send()
        .await
        .context("Failed to send request to Claude API")?;

        Self::parse_claude_response(response, &self.model).await
    }

    /// Direct API-key completion path (`x-api-key`).
    ///
    /// Unlike the OAuth path this must NOT inject the "You are Claude Code"
    /// identity spoof: that block is only valid for the OAuth/subscription
    /// endpoint and a direct API key talks to the standard Messages API.
    async fn complete_claude_api_key(
        &self,
        system: &str,
        user_message: &str,
        api_key: &str,
    ) -> Result<String> {
        let (thinking, output_config, request_max_tokens) = claude_reasoning_parts(
            &self.model,
            self.reasoning_override.as_deref(),
            self.max_tokens,
        );
        let request = ClaudeMessagesRequest {
            model: &self.model,
            max_tokens: request_max_tokens,
            system: build_claude_api_key_system_param(system),
            messages: vec![ClaudeMessage {
                role: "user",
                content: user_message,
            }],
            thinking,
            output_config,
        };

        // Manual/adaptive thinking needs the interleaved-thinking beta
        // (mirrors the runtime's `anthropic_beta_header_with_thinking`).
        let api_key_betas = if request.thinking.is_some() {
            "prompt-caching-2024-07-31,interleaved-thinking-2025-05-14"
        } else {
            "prompt-caching-2024-07-31"
        };
        let response = self
            .client
            .post(CLAUDE_API_KEY_URL)
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", api_key_betas)
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .context("Failed to send request to Claude API")?;

        Self::parse_claude_response(response, &self.model).await
    }

    /// Shared response parsing for both Claude credential paths.
    async fn parse_claude_response(response: reqwest::Response, model: &str) -> Result<String> {
        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(SidecarHttpError {
                provider: "Claude",
                status,
                body: error_text,
            }
            .into());
        }

        let result: ClaudeMessagesResponse = response
            .json()
            .await
            .context("Failed to parse Claude API response")?;

        if let Some(usage) = result.usage.as_ref() {
            crate::telemetry::record_provider_usage(
                None,
                "claude",
                model,
                crate::telemetry::UsageSource::Sidecar,
                crate::telemetry::ProviderUsage {
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                    cache_read_input_tokens: usage.cache_read_input_tokens,
                    cache_creation_input_tokens: usage.cache_creation_input_tokens,
                },
            );
        }

        let text = result
            .content
            .into_iter()
            .filter_map(|block| {
                if let ClaudeContentBlock::Text { text } = block {
                    Some(text)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("");

        Ok(text)
    }

    /// Check if a memory is relevant to the current context
    /// Returns (is_relevant, explanation)
    pub async fn check_relevance(
        &self,
        memory_content: &str,
        current_context: &str,
    ) -> Result<(bool, String)> {
        let system = r#"You are a memory relevance checker. Your job is to determine if a stored memory is relevant to the current context.

Respond in this exact format:
RELEVANT: yes/no
REASON: <brief explanation>

Be conservative - only say "yes" if the memory would actually be useful for the current task."#;

        let prompt = format!(
            "## Stored Memory\n{}\n\n## Current Context\n{}\n\nIs this memory relevant to the current context?",
            memory_content, current_context
        );

        let response = self.complete(system, &prompt).await?;

        // Parse response
        let mut is_relevant = false;
        for line in response.lines() {
            let line = line.trim();
            if line.len() >= 9 && line[..9].eq_ignore_ascii_case("relevant:") {
                let value = line[9..].trim();
                is_relevant = value.eq_ignore_ascii_case("yes") || value.starts_with("yes");
                break;
            }
        }
        let reason = response
            .lines()
            .find(|line| line.to_lowercase().starts_with("reason:"))
            .map(|line| line.trim_start_matches(|c: char| !c.is_alphabetic()).trim())
            .unwrap_or(&response)
            .to_string();

        Ok((is_relevant, reason))
    }

    /// Check if new information contradicts existing information
    /// Returns true if the two statements are contradictory
    pub async fn check_contradiction(
        &self,
        new_content: &str,
        existing_content: &str,
    ) -> Result<bool> {
        let system = "You are a contradiction detector. Given two statements, determine if the new information directly contradicts the existing information. Reply with exactly YES or NO.";

        let prompt = format!(
            "## Existing Information\n{}\n\n## New Information\n{}\n\nDoes the new information contradict the existing information?",
            existing_content, new_content
        );

        let response = self.complete(system, &prompt).await?;
        let trimmed = response.trim().to_uppercase();
        Ok(trimmed.starts_with("YES"))
    }

    /// Extract memories from a session transcript
    pub async fn extract_memories(&self, transcript: &str) -> Result<Vec<ExtractedMemory>> {
        self.extract_memories_with_existing(transcript, &[]).await
    }

    /// Extract memories from a session transcript, aware of what's already stored.
    pub async fn extract_memories_with_existing(
        &self,
        transcript: &str,
        existing: &[String],
    ) -> Result<Vec<ExtractedMemory>> {
        let mut system = String::from(
            r#"You are a memory extraction assistant. Extract important NEW learnings from the conversation that should be remembered for future sessions.

Categories (use EXACTLY one of these):
- fact: Technical facts about the codebase, architecture, patterns, dependencies, tools, environment
- preference: User preferences, workflow habits, UX expectations, coding style, conventions, how they want the assistant to behave
- correction: Mistakes that were corrected, bugs found and fixed, wrong assumptions, things the user corrected
- entity: Named entities worth tracking - people, projects, services, repos, teams

Categorization rules:
- If it describes what the USER WANTS or HOW THEY LIKE THINGS, it is "preference", not "fact"
- If it describes a BUG FIX or MISTAKE, it is "correction", not "fact"
- "fact" is for objective technical information about code/systems, not user behavior

IMPORTANT - Do NOT extract:
- Transient debugging details, compile errors, or intermediate build steps
- Specific commit hashes, git operations, or "changes were committed/pushed" details
- Line-by-line code changes like "X was updated to Y in file Z" - these belong in git history, not memory
- Self-evident project context (e.g., the project name, repo URL, language) that is already in the system prompt
- Redundant variations of information already known (check the "Already known" list carefully)

Quality bar: Only extract information that would ACTUALLY BE USEFUL if recalled in a future session on a different topic. Ask: "Would a developer benefit from knowing this weeks from now?"

For each memory, output in this format (one per line):
CATEGORY|CONTENT|TRUST

Where:
- CATEGORY is one of: fact, preference, correction, entity
- CONTENT is a concise statement (1-2 sentences max, under 200 characters preferred)
- TRUST is one of: high (user stated), medium (observed), low (inferred)

Output ONLY the formatted lines, no other text. If no NEW memories worth extracting, output nothing."#,
        );

        if !existing.is_empty() {
            system.push_str("\n\nAlready known (do NOT re-extract these or close paraphrases):\n");
            for mem in existing.iter().take(80) {
                system.push_str("- ");
                system.push_str(crate::util::truncate_str(mem, 150));
                system.push('\n');
            }
        }

        let response = self.complete(&system, transcript).await?;

        let memories = response
            .lines()
            .filter(|line| line.contains('|'))
            .filter_map(|line| {
                let parts: Vec<&str> = line.split('|').collect();
                if parts.len() >= 3 {
                    Some(ExtractedMemory {
                        category: parts[0].trim().to_lowercase(),
                        content: parts[1].trim().to_string(),
                        trust: parts[2].trim().to_lowercase(),
                    })
                } else {
                    None
                }
            })
            .collect();

        Ok(memories)
    }
}

impl Default for Sidecar {
    fn default() -> Self {
        Self::new()
    }
}

/// The public model constant for backward compatibility in tests.
#[cfg(test)]
pub const SIDECAR_FAST_MODEL: &str = SIDECAR_OPENAI_MODEL;

/// Resolve the Responses API base for the sidecar: the ChatGPT backend in
/// OAuth mode, the standard API base in API-key mode. A test-only override
/// (`JCODE_SIDECAR_TEST_API_BASE`) redirects both to a loopback fixture so
/// retry/fallback behavior can be asserted against real HTTP requests
/// instead of constants; it is ignored outside `cfg(test)`.
fn openai_responses_base(is_chatgpt_mode: bool) -> String {
    #[cfg(test)]
    if let Ok(base) = std::env::var("JCODE_SIDECAR_TEST_API_BASE")
        && !base.trim().is_empty()
    {
        return base.trim().trim_end_matches('/').to_string();
    }
    if is_chatgpt_mode {
        CHATGPT_API_BASE.to_string()
    } else {
        OPENAI_API_BASE.to_string()
    }
}

fn resolve_openai_request_model(
    preferred_model: &str,
    is_chatgpt_mode: bool,
) -> (&str, Option<&'static str>) {
    if preferred_model != SIDECAR_OPENAI_MODEL {
        return (preferred_model, None);
    }

    match (
        is_chatgpt_mode,
        crate::provider::is_model_available_for_account(SIDECAR_OPENAI_MODEL),
    ) {
        (true, Some(false)) => (
            SIDECAR_OPENAI_OAUTH_FALLBACK_MODEL,
            Some(SIDECAR_OPENAI_OAUTH_FALLBACK_REASONING),
        ),
        _ => (SIDECAR_OPENAI_MODEL, Some(SIDECAR_OPENAI_REASONING)),
    }
}

fn build_openai_request(
    model: &str,
    system: &str,
    user_message: &str,
    stream: bool,
    reasoning_effort: Option<&str>,
) -> serde_json::Value {
    let mut instructions = String::new();
    if !system.is_empty() {
        instructions.push_str(system);
    }

    let mut request = serde_json::json!({
        "model": model,
        "instructions": instructions,
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{
                "type": "input_text",
                "text": user_message,
            }],
        }],
        "stream": stream,
        "store": false,
    });

    if let Some(effort) = reasoning_effort {
        request["reasoning"] = serde_json::json!({ "effort": effort });
    }

    request
}

fn classify_openai_model_unavailable(status: StatusCode, body: &str) -> Option<String> {
    let lower = body.to_ascii_lowercase();
    let mentions_model = lower.contains("model")
        || lower.contains("slug")
        || lower.contains("engine")
        || lower.contains("deployment");
    let unavailable = lower.contains("not available")
        || lower.contains("unavailable")
        || lower.contains("does not have access")
        || lower.contains("not enabled")
        || lower.contains("not found")
        || lower.contains("unknown model")
        || lower.contains("unsupported model")
        || lower.contains("invalid model");

    if !mentions_model || !unavailable {
        return None;
    }

    if matches!(
        status,
        StatusCode::NOT_FOUND
            | StatusCode::FORBIDDEN
            | StatusCode::BAD_REQUEST
            | StatusCode::UNPROCESSABLE_ENTITY
    ) {
        let trimmed = body.trim();
        return Some(if trimmed.is_empty() {
            format!("model denied by OpenAI API (status {})", status)
        } else {
            format!(
                "model denied by OpenAI API (status {}): {}",
                status, trimmed
            )
        });
    }

    None
}

fn is_openai_model_unavailable(status: StatusCode, body: &str) -> bool {
    classify_openai_model_unavailable(status, body).is_some()
}

enum OpenAiSidecarError {
    Api { status: StatusCode, body: String },
    Other(anyhow::Error),
}

impl OpenAiSidecarError {
    fn other(err: anyhow::Error) -> Self {
        Self::Other(err)
    }

    fn into_anyhow(self) -> anyhow::Error {
        match self {
            Self::Api { status, body } => SidecarHttpError {
                provider: "OpenAI",
                status,
                body,
            }
            .into(),
            Self::Other(err) => err,
        }
    }
}

/// A memory extracted by the sidecar
#[derive(Debug, Clone)]
pub struct ExtractedMemory {
    pub category: String,
    pub content: String,
    pub trust: String,
}

/// Collect text from an OpenAI Responses API SSE stream.
///
/// Parses `data: <json>` lines and accumulates text deltas from
/// `response.output_text.delta` events, stopping on completion/done.
async fn collect_openai_sse_text(response: reqwest::Response, model: &str) -> Result<String> {
    use futures::StreamExt;
    let mut stream = response.bytes_stream();
    let mut text = String::new();
    let mut buf = String::new();

    while let Some(chunk) = stream.next().await {
        let bytes = chunk.context("Error reading SSE stream")?;
        buf.push_str(&String::from_utf8_lossy(&bytes));

        // Process all complete lines in the buffer
        while let Some(newline_pos) = buf.find('\n') {
            let line = buf[..newline_pos].trim_end_matches('\r').to_string();
            buf = buf[newline_pos + 1..].to_string();

            if let Some(data) = crate::util::sse_data_line(&line) {
                if data == "[DONE]" {
                    return Ok(text);
                }
                if let Ok(event) = serde_json::from_str::<SseEvent>(data) {
                    match event.kind.as_str() {
                        "response.output_text.delta" => {
                            if let Some(delta) = event.delta {
                                text.push_str(&delta);
                            }
                        }
                        "response.completed" | "response.incomplete" => {
                            record_openai_sidecar_usage(
                                model,
                                event.response.as_ref().and_then(|r| r.get("usage")),
                            );
                            return Ok(text);
                        }
                        "response.failed" | "error" => {
                            let msg = event
                                .error
                                .as_ref()
                                .and_then(|e| e.as_str())
                                .unwrap_or("unknown error");
                            anyhow::bail!("OpenAI SSE error: {}", msg);
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    Ok(text)
}

/// Parsed OpenAI Responses API `usage` object. OpenAI reports cached prompt
/// tokens as a subset of `input_tokens` (`input_tokens_details.cached_tokens`),
/// so the total stays `input + output`; cache reads are surfaced separately for
/// pricing, the same convention the agent path uses for OpenAI providers.
fn parse_openai_usage(usage: &serde_json::Value) -> Option<crate::telemetry::ProviderUsage> {
    let input = usage.get("input_tokens").and_then(|v| v.as_u64());
    let output = usage.get("output_tokens").and_then(|v| v.as_u64());
    if input.is_none() && output.is_none() {
        return None;
    }
    let cached = usage
        .get("input_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(|v| v.as_u64());
    Some(crate::telemetry::ProviderUsage {
        input_tokens: input.unwrap_or(0),
        output_tokens: output.unwrap_or(0),
        cache_read_input_tokens: cached,
        cache_creation_input_tokens: None,
    })
}

fn record_openai_sidecar_usage(model: &str, usage: Option<&serde_json::Value>) {
    if let Some(parsed) = usage.and_then(parse_openai_usage) {
        crate::telemetry::record_provider_usage(
            None,
            "openai",
            model,
            crate::telemetry::UsageSource::Sidecar,
            parsed,
        );
    }
}

/// Extract text from a non-streaming OpenAI Responses API JSON response.
fn extract_openai_response_text(result: &serde_json::Value) -> Result<String> {
    let mut text = String::new();
    if let Some(output) = result.get("output").and_then(|v| v.as_array()) {
        for item in output {
            let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if item_type == "message"
                && let Some(content) = item.get("content").and_then(|v| v.as_array())
            {
                for block in content {
                    let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    if (block_type == "output_text" || block_type == "text")
                        && let Some(t) = block.get("text").and_then(|v| v.as_str())
                    {
                        text.push_str(t);
                    }
                }
            }
        }
    }
    Ok(text)
}

#[derive(Deserialize)]
struct SseEvent {
    #[serde(rename = "type")]
    kind: String,
    delta: Option<String>,
    error: Option<serde_json::Value>,
    #[serde(default)]
    response: Option<serde_json::Value>,
}

// Claude API types

#[derive(Serialize)]
struct ClaudeMessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<ClaudeApiSystem<'a>>,
    messages: Vec<ClaudeMessage<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<ClaudeThinking>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_config: Option<ClaudeOutputConfig>,
}

#[derive(Serialize)]
struct ClaudeMessage<'a> {
    role: &'a str,
    content: &'a str,
}

/// Reasoning thinking control for the Claude sidecar path, derived from
/// `agents.memory_effort` and the model's capability set (single source of
/// truth: `jcode_provider_core::anthropic_reasoning_caps`).
///
/// Mirrors the runtime's `build_reasoning_request_parts_with_effort`:
/// - `output_config` effort when the model supports it,
/// - a manual thinking budget for manual-thinking models,
/// - adaptive thinking for adaptive-capable models (Sonnet 4.6 style: the
///   request must carry `thinking` for the configured effort to take
///   effect),
/// - nothing when the model has no reasoning control or the effort is `none`.
///
/// Claude requires `thinking.budget_tokens` to be strictly smaller than
/// `max_tokens`. When the configured response limit would clamp the effort's
/// budget, the budget wins and the request limit is raised so memory
/// extraction can still think at the requested level and return text.
#[derive(Serialize, Debug)]
#[serde(rename_all = "snake_case", tag = "type")]
enum ClaudeThinking {
    Enabled {
        budget_tokens: u32,
    },
    Adaptive {
        #[serde(skip_serializing_if = "Option::is_none")]
        display: Option<&'static str>,
    },
}

/// Effort control for models that accept `output_config: {effort}`.
#[derive(Serialize)]
struct ClaudeOutputConfig {
    effort: String,
}

/// Reasoning request parts for the Claude sidecar path, derived from
/// `agents.memory_effort` and the model's capability set (single source of
/// truth: `jcode_provider_core::anthropic_reasoning_caps`).
///
/// Mirrors the runtime's `build_reasoning_request_parts_with_effort`:
/// - `output_config` effort when the model supports it,
/// - a manual thinking budget otherwise,
/// - nothing when the model has no reasoning control or the effort is `none`.
///
/// Claude requires `thinking.budget_tokens` to be strictly smaller than
/// `max_tokens`. When a small configured response limit would leave no answer
/// room above the minimum thinking budget, the request limit is raised so memory
/// extraction can still use manual thinking and return text.
fn claude_reasoning_parts(
    model: &str,
    effort: Option<&str>,
    max_tokens: u32,
) -> (Option<ClaudeThinking>, Option<ClaudeOutputConfig>, u32) {
    let Some(effort) = effort else {
        return (None, None, max_tokens);
    };
    let caps = jcode_provider_core::anthropic::anthropic_reasoning_caps(model);
    // Always-on thinking models (Opus 5.5, Fable 5.1) cannot disable thinking,
    // so `none` means the lowest supported effort, same mapping as the main
    // Claude runtime (`build_reasoning_request_parts_for_budget`). Without
    // this the early return below would drop the effort control entirely and
    // leave the model at its default (higher) effort.
    let effort = if effort == "none"
        && jcode_provider_core::anthropic::anthropic_thinking_always_on(model)
    {
        "low"
    } else {
        effort
    };
    if !caps.supports_reasoning_effort() || effort == "none" {
        return (None, None, max_tokens);
    }
    // Both controls are independent, like the runtime: models with both
    // (Opus 4.5) send output_config effort AND a manual budget.
    let output_config = caps.output_effort.then(|| {
        // Clamp levels the model doesn't accept, same ladder as the runtime.
        let resolved = if effort == "minimal" {
            "low"
        } else if matches!(effort, "max" | "xhigh") && !caps.xhigh_effort && !caps.max_effort {
            "high"
        } else if effort == "max" && !caps.max_effort {
            "xhigh"
        } else if effort == "xhigh" && !caps.xhigh_effort {
            "high"
        } else {
            effort
        };
        ClaudeOutputConfig {
            effort: resolved.to_string(),
        }
    });
    // Manual-thinking models need a concrete budget; adaptive-capable models
    // need the `thinking` field present (without it the request runs without
    // thinking and `output_config.effort` alone does nothing); others rely on
    // output_config above.
    let thinking = if caps.manual_thinking {
        let budget = match effort {
            "minimal" | "low" => CLAUDE_MIN_THINKING_BUDGET,
            "medium" => 4_096,
            "high" => 8_192,
            "xhigh" | "max" => 16_384,
            _ => return (None, output_config, max_tokens),
        };
        // The effort-selected budget wins over the configured response limit:
        // clamping it down to `max_tokens - 1` collapses every effort above
        // `low` into the 1,024 minimum (greptile). Keep the requested budget
        // and raise the request limit instead so the answer keeps room.
        Some(ClaudeThinking::Enabled {
            budget_tokens: budget,
        })
    } else if caps.adaptive_thinking {
        // Mirrors the runtime's `reasoning_request::adaptive_thinking`: the
        // request carries `thinking` so the configured effort actually
        // enables adaptive thinking (greptile: without this branch Sonnet 4.6
        // ran memory extraction with no thinking at all).
        Some(ClaudeThinking::Adaptive {
            display: Some("summarized"),
        })
    } else {
        None
    };
    let request_max_tokens = match &thinking {
        Some(ClaudeThinking::Enabled { budget_tokens }) => {
            max_tokens.max(budget_tokens + CLAUDE_THINKING_ANSWER_HEADROOM)
        }
        // Adaptive thinking has no budget: the request limit is unchanged.
        _ => max_tokens,
    };
    (thinking, output_config, request_max_tokens)
}

#[derive(Serialize)]
#[serde(untagged)]
enum ClaudeApiSystem<'a> {
    Blocks(Vec<ClaudeApiSystemBlock<'a>>),
}

#[derive(Serialize)]
struct ClaudeApiSystemBlock<'a> {
    #[serde(rename = "type")]
    block_type: &'static str,
    text: &'a str,
}

fn build_claude_system_param(system: &str) -> Option<ClaudeApiSystem<'_>> {
    let mut blocks = Vec::new();
    blocks.push(ClaudeApiSystemBlock {
        block_type: "text",
        text: CLAUDE_CODE_IDENTITY,
    });
    blocks.push(ClaudeApiSystemBlock {
        block_type: "text",
        text: CLAUDE_CODE_JCODE_NOTICE,
    });
    if !system.is_empty() {
        blocks.push(ClaudeApiSystemBlock {
            block_type: "text",
            text: system,
        });
    }
    Some(ClaudeApiSystem::Blocks(blocks))
}

/// Build the system param for the direct API-key path.
///
/// The "You are Claude Code" identity spoof and jcode notice are only valid
/// for the OAuth/subscription endpoint; a direct API key talks to the standard
/// Messages API and must not impersonate the official CLI. So this only carries
/// the caller's own system prompt (if any).
fn build_claude_api_key_system_param(system: &str) -> Option<ClaudeApiSystem<'_>> {
    if system.is_empty() {
        return None;
    }
    Some(ClaudeApiSystem::Blocks(vec![ClaudeApiSystemBlock {
        block_type: "text",
        text: system,
    }]))
}

/// Whether the sidecar's Claude backend should use the direct API key rather
/// than OAuth. True when the runtime is pinned to Anthropic API-key mode
/// (`claude-api`), or when no OAuth credentials are present at all. Mirrors the
/// main provider's resolution so memory features authenticate the same way the
/// agent does.
fn anthropic_sidecar_prefers_api_key() -> bool {
    match jcode_provider_core::runtime_env_pinned_mode(
        jcode_provider_core::DualAuthProvider::Anthropic,
    ) {
        Some(jcode_provider_core::AuthMode::ApiKey) => true,
        Some(jcode_provider_core::AuthMode::Oauth) => false,
        None => auth::claude::load_credentials().is_err(),
    }
}

/// Recognize the Anthropic "OAuth not allowed for this organization" 403 so the
/// sidecar can transparently fall back to the API key.
fn is_anthropic_oauth_forbidden(err: &anyhow::Error) -> bool {
    let msg = err.to_string();
    msg.contains("403")
        && (msg.contains("OAuth authentication is currently not allowed")
            || msg.contains("permission_error"))
}

#[derive(Deserialize)]
struct ClaudeMessagesResponse {
    content: Vec<ClaudeContentBlock>,
    usage: Option<ClaudeUsage>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ClaudeContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct ClaudeUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::codex;
    use std::ffi::OsString;

    #[test]
    fn parses_openai_responses_usage_with_cached_subset() {
        let usage = serde_json::json!({
            "input_tokens": 1200,
            "input_tokens_details": {"cached_tokens": 900},
            "output_tokens": 80,
            "total_tokens": 1280
        });
        let parsed = parse_openai_usage(&usage).expect("usage parsed");
        assert_eq!(parsed.input_tokens, 1200);
        assert_eq!(parsed.output_tokens, 80);
        assert_eq!(parsed.cache_read_input_tokens, Some(900));
        assert_eq!(parsed.cache_creation_input_tokens, None);
        assert!(parse_openai_usage(&serde_json::json!({})).is_none());
    }

    #[test]
    fn sse_completed_event_exposes_response_usage() {
        let data = r#"{"type":"response.completed","response":{"usage":{"input_tokens":10,"output_tokens":3}}}"#;
        let event: SseEvent = serde_json::from_str(data).expect("sse event");
        let usage = event
            .response
            .as_ref()
            .and_then(|r| r.get("usage"))
            .and_then(parse_openai_usage)
            .expect("usage");
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 3);
    }

    #[test]
    fn claude_response_usage_includes_cache_buckets() {
        let body = r#"{"content":[{"type":"text","text":"ok"}],"usage":{"input_tokens":5,"output_tokens":2,"cache_read_input_tokens":400,"cache_creation_input_tokens":30}}"#;
        let parsed: ClaudeMessagesResponse = serde_json::from_str(body).expect("claude response");
        let usage = parsed.usage.expect("usage");
        assert_eq!(usage.input_tokens, 5);
        assert_eq!(usage.output_tokens, 2);
        assert_eq!(usage.cache_read_input_tokens, Some(400));
        assert_eq!(usage.cache_creation_input_tokens, Some(30));
    }

    struct EnvVarGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set_path(key: &'static str, value: &std::path::Path) -> Self {
            let previous = std::env::var_os(key);
            crate::env::set_var(key, value);
            Self { key, previous }
        }

        fn unset(key: &'static str) -> Self {
            let previous = std::env::var_os(key);
            crate::env::remove_var(key);
            Self { key, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(previous) = &self.previous {
                crate::env::set_var(self.key, previous);
            } else {
                crate::env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn test_sidecar_fast_model() {
        assert_eq!(SIDECAR_FAST_MODEL, "gpt-5.6-luna");
        assert_eq!(SIDECAR_CLAUDE_MODEL, "claude-haiku-4-5-20251001");
    }

    #[test]
    fn sidecar_http_error_classifies_permanent_client_failures() {
        for status in [
            StatusCode::BAD_REQUEST,
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::NOT_FOUND,
        ] {
            let error: anyhow::Error = SidecarHttpError {
                provider: "test",
                status,
                body: "failure".to_string(),
            }
            .into();
            assert_eq!(classify_error(&error), SidecarErrorKind::Permanent);
        }
    }

    #[test]
    fn sidecar_http_error_classifies_retryable_failures() {
        for status in [StatusCode::TOO_MANY_REQUESTS, StatusCode::BAD_GATEWAY] {
            let error: anyhow::Error = SidecarHttpError {
                provider: "test",
                status,
                body: "failure".to_string(),
            }
            .into();
            assert_eq!(classify_error(&error), SidecarErrorKind::Transient);
        }
        assert_eq!(
            classify_error(&anyhow::anyhow!("connection reset")),
            SidecarErrorKind::Transient
        );
    }

    #[test]
    fn test_backend_selection_prefers_openai() {
        // Make backend selection deterministic by isolating credentials.
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().expect("create temp jcode home");
        let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
        let _openai = EnvVarGuard::unset("OPENAI_API_KEY");

        codex::upsert_account_from_tokens("openai-1", "sk-test-key-123", "", None, None)
            .expect("write OpenAI test auth");
        crate::auth::claude::upsert_account(crate::auth::claude::AnthropicAccount {
            label: "claude-1".to_string(),
            access: "claude-access".to_string(),
            refresh: "claude-refresh".to_string(),
            expires: 4_102_444_800_000,
            email: None,
            scopes: Vec::new(),
            subscription_type: None,
        })
        .expect("write Claude test auth");

        let sidecar = Sidecar::with_configured_model(None, None);
        assert_eq!(sidecar.backend, SidecarBackend::OpenAI);
        assert_eq!(sidecar.model, SIDECAR_OPENAI_MODEL);
        codex::set_active_account_override(None);
        crate::auth::claude::set_active_account_override(None);
    }

    #[test]
    fn configured_memory_effort_pins_reasoning_override() {
        // The effort pin is backend-independent: no credential isolation is
        // needed, only the reasoning override itself matters.
        let sidecar = Sidecar::with_configured_model(None, Some("low".to_string()));
        assert_eq!(sidecar.reasoning_override.as_deref(), Some("low"));

        // Unset effort leaves the per-model default in place.
        let sidecar = Sidecar::with_configured_model(None, None);
        assert_eq!(sidecar.reasoning_override, None);
    }

    #[test]
    fn sidecar_new_ignores_empty_memory_effort() {
        // A blank effort in the config file must mean "unset", never an
        // invalid `""`/whitespace pin that would reach the OpenAI request.
        // Padded values must trim to the bare token: providers match exact
        // strings, so a raw `" low "` would break the pin lookup.
        // Drives the real config file parse via the global config cache.
        let _guard = crate::storage::lock_test_env();
        let saved_home = std::env::var_os("JCODE_HOME");
        let dir = tempfile::TempDir::new().expect("tempdir");
        crate::env::set_var("JCODE_HOME", dir.path());
        std::fs::write(
            dir.path().join("config.toml"),
            "[agents]\nmemory_effort = \"   \"\n",
        )
        .expect("write config");
        let sidecar = Sidecar::new();
        assert_eq!(sidecar.reasoning_override, None);

        std::fs::write(
            dir.path().join("config.toml"),
            "[agents]\nmemory_effort = \" low \"\n",
        )
        .expect("write config");
        let sidecar = Sidecar::new();
        assert_eq!(
            sidecar.reasoning_override.as_deref(),
            Some("low"),
            "padded effort must trim, not pin the raw spaced string"
        );

        std::fs::write(
            dir.path().join("config.toml"),
            "[agents]\nmemory_effort = \"low\"\n",
        )
        .expect("write config");
        let sidecar = Sidecar::new();
        assert_eq!(sidecar.reasoning_override.as_deref(), Some("low"));

        // Restore the saved home: the temp dir dies with this test, and a
        // process-wide JCODE_HOME pointing at a deleted directory poisons
        // later tests.
        if let Some(home) = saved_home {
            crate::env::set_var("JCODE_HOME", home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[test]
    fn claude_reasoning_parts_follow_model_caps() {
        // Manual-thinking model (Opus 4.5): concrete budget + output_config.
        let (thinking, output_config, request_max_tokens) =
            claude_reasoning_parts("claude-opus-4-5", Some("low"), 8_192);
        assert_eq!(request_max_tokens, 8_192);
        assert_eq!(output_config.expect("effort").effort, "low");
        match thinking {
            Some(ClaudeThinking::Enabled { budget_tokens }) => assert_eq!(budget_tokens, 1_024),
            other => panic!("expected manual thinking budget, got {other:?}"),
        }

        // Modern ladder model (Opus 4.7): output_config effort AND adaptive
        // thinking (FULL caps; pre-fix the thinking field was omitted).
        let (thinking, output_config, request_max_tokens) =
            claude_reasoning_parts("claude-opus-4-7", Some("medium"), DEFAULT_MAX_TOKENS);
        assert!(
            matches!(&thinking, Some(ClaudeThinking::Adaptive { .. })),
            "expected adaptive thinking, got {thinking:?}"
        );
        assert_eq!(request_max_tokens, DEFAULT_MAX_TOKENS);
        assert_eq!(output_config.expect("effort").effort, "medium");

        // Opus 4.6 has no xhigh: clamps down to high.
        let (_, output_config, _) =
            claude_reasoning_parts("claude-opus-4-6", Some("xhigh"), DEFAULT_MAX_TOKENS);
        assert_eq!(output_config.expect("effort").effort, "high");

        // `minimal` maps to `low` on the output_config ladder.
        let (_, output_config, _) =
            claude_reasoning_parts("claude-opus-4-7", Some("minimal"), DEFAULT_MAX_TOKENS);
        assert_eq!(output_config.expect("effort").effort, "low");

        // `none` disables reasoning on models that support a real off switch.
        let (thinking, output_config, request_max_tokens) =
            claude_reasoning_parts("claude-opus-4-7", Some("none"), DEFAULT_MAX_TOKENS);
        assert!(thinking.is_none() && output_config.is_none());
        assert_eq!(request_max_tokens, DEFAULT_MAX_TOKENS);

        // Always-on thinking models (Opus 5.5, Fable 5.1) cannot disable
        // thinking, so `none` maps to the lowest supported effort, same as
        // the main Claude runtime. The request must carry the effort control
        // and the adaptive `thinking` field (without `thinking` the request
        // runs without reasoning and the effort control alone does nothing).
        for model in ["claude-opus-5-5", "claude-fable-5-1"] {
            let (thinking, output_config, request_max_tokens) =
                claude_reasoning_parts(model, Some("none"), DEFAULT_MAX_TOKENS);
            assert_eq!(request_max_tokens, DEFAULT_MAX_TOKENS);
            assert_eq!(
                output_config
                    .as_ref()
                    .expect("always-on model must keep the effort control")
                    .effort,
                "low",
                "{model}: `none` must map to `low`"
            );
            match &thinking {
                Some(ClaudeThinking::Adaptive { display }) => {
                    assert_eq!(*display, Some("summarized"), "{model}");
                }
                other => panic!("{model}: expected adaptive thinking, got {other:?}"),
            }
        }

        // Sonnet 4.6 (adaptive + output effort, no xhigh): the request must
        // carry BOTH `thinking: {type: adaptive}` and `output_config.effort`.
        // Pre-fix, `thinking` was omitted entirely and Sonnet 4.6 ran memory
        // extraction without any thinking (greptile).
        let (thinking, output_config, request_max_tokens) =
            claude_reasoning_parts("claude-sonnet-4-6", Some("medium"), DEFAULT_MAX_TOKENS);
        assert_eq!(
            request_max_tokens, DEFAULT_MAX_TOKENS,
            "adaptive thinking adds no budget"
        );
        assert_eq!(
            output_config.expect("Sonnet 4.6 must keep effort").effort,
            "medium"
        );
        match &thinking {
            Some(ClaudeThinking::Adaptive { display }) => {
                assert_eq!(*display, Some("summarized"));
            }
            other => panic!("expected adaptive thinking for Sonnet 4.6, got {other:?}"),
        }

        // `none` on Sonnet 4.6: nothing is added (it has a real off switch).
        let (thinking, output_config, request_max_tokens) =
            claude_reasoning_parts("claude-sonnet-4-6", Some("none"), DEFAULT_MAX_TOKENS);
        assert!(thinking.is_none() && output_config.is_none());
        assert_eq!(request_max_tokens, DEFAULT_MAX_TOKENS);

        // No effort: nothing added, request stays minimal.
        let (thinking, output_config, request_max_tokens) =
            claude_reasoning_parts(SIDECAR_CLAUDE_MODEL, None, 1_024);
        assert!(thinking.is_none() && output_config.is_none());
        assert_eq!(request_max_tokens, 1_024);

        // Small configured limit must NOT clamp the effort budget (greptile:
        // the old `min(max_tokens - 1)` collapsed every effort above `low` to
        // the 1,024 minimum). The budget wins and the request limit rises.
        let (thinking, _, request_max_tokens) =
            claude_reasoning_parts("claude-sonnet-3-7", Some("high"), 2_000);
        match thinking {
            Some(ClaudeThinking::Enabled { budget_tokens }) => {
                assert_eq!(budget_tokens, 8_192, "effort budget must survive");
                assert!(budget_tokens < request_max_tokens);
                assert_eq!(request_max_tokens, 8_192 + CLAUDE_THINKING_ANSWER_HEADROOM);
            }
            other => panic!("expected preserved budget, got {other:?}"),
        }

        // Sonnet 3.7 (manual-only) at the sidecar's default 1,024-token
        // limit: the configured budget is preserved and the request limit
        // rises to leave answer room.
        let (thinking, output_config, request_max_tokens) =
            claude_reasoning_parts("claude-sonnet-3-7", Some("low"), DEFAULT_MAX_TOKENS);
        assert!(output_config.is_none(), "Sonnet 3.7 has no output_config");
        match thinking {
            Some(ClaudeThinking::Enabled { budget_tokens }) => {
                assert_eq!(budget_tokens, CLAUDE_MIN_THINKING_BUDGET);
                assert!(budget_tokens < request_max_tokens);
                assert!(request_max_tokens >= budget_tokens + CLAUDE_THINKING_ANSWER_HEADROOM);
            }
            other => panic!("expected minimum thinking budget, got {other:?}"),
        }

        // Higher efforts on Sonnet 3.7 must scale the budget, not collapse
        // to the minimum (the pre-fix bug: medium/high/max all sent 1,024).
        for (effort, want_budget) in [("medium", 4_096u32), ("high", 8_192), ("max", 16_384)] {
            let (thinking, _, request_max_tokens) =
                claude_reasoning_parts("claude-sonnet-3-7", Some(effort), DEFAULT_MAX_TOKENS);
            match thinking {
                Some(ClaudeThinking::Enabled { budget_tokens }) => {
                    assert_eq!(budget_tokens, want_budget, "effort {effort}");
                    assert!(budget_tokens < request_max_tokens, "effort {effort}");
                }
                other => panic!("expected budget for effort {effort}, got {other:?}"),
            }
        }

        let (thinking, _, request_max_tokens) =
            claude_reasoning_parts("claude-sonnet-3-7", Some("low"), 8_192);
        match thinking {
            Some(ClaudeThinking::Enabled { budget_tokens }) => {
                assert_eq!(budget_tokens, CLAUDE_MIN_THINKING_BUDGET);
                assert!(budget_tokens < request_max_tokens);
            }
            other => panic!("expected minimum thinking budget, got {other:?}"),
        }
        assert_eq!(request_max_tokens, 8_192);

        // Sidecar default (Haiku 4.5) and unknown families: no reasoning
        // control, nothing added.
        for model in [SIDECAR_CLAUDE_MODEL, "totally-unknown-model"] {
            let (thinking, output_config, request_max_tokens) =
                claude_reasoning_parts(model, Some("high"), DEFAULT_MAX_TOKENS);
            assert!(thinking.is_none() && output_config.is_none(), "{model}");
            assert_eq!(request_max_tokens, DEFAULT_MAX_TOKENS);
        }
    }

    #[test]
    fn claude_request_omits_reasoning_fields_when_unset() {
        let (thinking, output_config, request_max_tokens) =
            claude_reasoning_parts(SIDECAR_CLAUDE_MODEL, None, 1_024);
        let request = ClaudeMessagesRequest {
            model: SIDECAR_CLAUDE_MODEL,
            max_tokens: request_max_tokens,
            system: None,
            messages: vec![ClaudeMessage {
                role: "user",
                content: "hi",
            }],
            thinking,
            output_config,
        };
        let json = serde_json::to_value(&request).expect("serialize");
        assert!(json.get("thinking").is_none(), "{json}");
        assert!(json.get("output_config").is_none(), "{json}");
    }

    #[test]
    fn claude_request_serializes_manual_thinking() {
        let (thinking, output_config, request_max_tokens) =
            claude_reasoning_parts("claude-opus-4-5", Some("low"), 8_192);
        let request = ClaudeMessagesRequest {
            model: "claude-opus-4-5",
            max_tokens: request_max_tokens,
            system: None,
            messages: vec![ClaudeMessage {
                role: "user",
                content: "hi",
            }],
            thinking,
            output_config,
        };
        let json = serde_json::to_value(&request).expect("serialize");
        assert_eq!(
            json.get("thinking").and_then(|t| t.get("type")),
            Some(&serde_json::json!("enabled")),
            "{json}"
        );
        assert_eq!(
            json.pointer("/thinking/budget_tokens"),
            Some(&serde_json::json!(1_024)),
            "{json}"
        );
    }

    #[test]
    fn claude_request_raises_manual_thinking_max_tokens() {
        let (thinking, output_config, request_max_tokens) =
            claude_reasoning_parts("claude-sonnet-3-7", Some("low"), DEFAULT_MAX_TOKENS);
        let request = ClaudeMessagesRequest {
            model: "claude-sonnet-3-7",
            max_tokens: request_max_tokens,
            system: None,
            messages: vec![ClaudeMessage {
                role: "user",
                content: "hi",
            }],
            thinking,
            output_config,
        };
        let json = serde_json::to_value(&request).expect("serialize");
        let budget_tokens = json
            .pointer("/thinking/budget_tokens")
            .and_then(|value| value.as_u64())
            .expect("thinking budget");
        let max_tokens = json
            .get("max_tokens")
            .and_then(|value| value.as_u64())
            .expect("max tokens");

        assert!(budget_tokens < max_tokens, "{json}");
        assert!(
            max_tokens >= budget_tokens + u64::from(CLAUDE_THINKING_ANSWER_HEADROOM),
            "{json}"
        );
        assert_eq!(json.get("output_config"), None, "{json}");
    }

    #[test]
    fn sidecar_unsupported_model_override_falls_back_to_auto_select() {
        let sidecar = Sidecar::with_configured_model(
            Some("totally-unknown-model".to_string()),
            Some("low".to_string()),
        );
        // The configured effort survives the model fallback: it pins the
        // OpenAI sidecar path whichever model it lands on.
        assert_eq!(sidecar.reasoning_override.as_deref(), Some("low"));
    }

    #[test]
    fn test_chatgpt_oauth_uses_luna_with_no_reasoning_when_available() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().expect("create temp jcode home");
        let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
        codex::set_active_account_override(Some("openai-1".to_string()));
        crate::provider::clear_all_model_unavailability_for_account();
        crate::provider::populate_account_models(vec![
            SIDECAR_OPENAI_MODEL.to_string(),
            SIDECAR_OPENAI_OAUTH_FALLBACK_MODEL.to_string(),
        ]);

        let (model, reasoning) = resolve_openai_request_model(SIDECAR_OPENAI_MODEL, true);
        assert_eq!(model, SIDECAR_OPENAI_MODEL);
        assert_eq!(reasoning, Some(SIDECAR_OPENAI_REASONING));

        codex::set_active_account_override(None);
    }

    #[test]
    fn test_chatgpt_oauth_falls_back_to_gpt_5_4_low_when_luna_unavailable() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().expect("create temp jcode home");
        let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
        codex::set_active_account_override(Some("openai-1".to_string()));
        crate::provider::clear_all_model_unavailability_for_account();
        crate::provider::populate_account_models(vec![
            SIDECAR_OPENAI_OAUTH_FALLBACK_MODEL.to_string(),
        ]);

        let (model, reasoning) = resolve_openai_request_model(SIDECAR_OPENAI_MODEL, true);
        assert_eq!(model, SIDECAR_OPENAI_OAUTH_FALLBACK_MODEL);
        assert_eq!(reasoning, Some(SIDECAR_OPENAI_OAUTH_FALLBACK_REASONING));

        codex::set_active_account_override(None);
    }

    #[test]
    fn test_build_openai_request_uses_configured_default_and_fallback_reasoning() {
        let request = build_openai_request(
            SIDECAR_OPENAI_OAUTH_FALLBACK_MODEL,
            "system",
            "hello",
            true,
            Some(SIDECAR_OPENAI_OAUTH_FALLBACK_REASONING),
        );
        assert_eq!(request["model"], SIDECAR_OPENAI_OAUTH_FALLBACK_MODEL);
        assert_eq!(
            request["reasoning"],
            serde_json::json!({"effort": SIDECAR_OPENAI_OAUTH_FALLBACK_REASONING})
        );

        let luna_request = build_openai_request(
            SIDECAR_OPENAI_MODEL,
            "system",
            "hello",
            true,
            Some(SIDECAR_OPENAI_REASONING),
        );
        assert_eq!(luna_request["model"], SIDECAR_OPENAI_MODEL);
        assert_eq!(
            luna_request["reasoning"],
            serde_json::json!({"effort": SIDECAR_OPENAI_REASONING})
        );
    }

    #[test]
    fn test_openai_api_key_mode_uses_luna_with_no_reasoning() {
        let (model, reasoning) = resolve_openai_request_model(SIDECAR_OPENAI_MODEL, false);
        assert_eq!(model, SIDECAR_OPENAI_MODEL);
        assert_eq!(reasoning, Some(SIDECAR_OPENAI_REASONING));
    }

    // ---- ChatGPT Luna-unavailable retry (greptile P2: memory retries ignored effort)

    /// Loopback Responses-API fixture: denies `gpt-5.6-luna` with the
    /// model-unavailable body the production classifier recognizes, then
    /// accepts the fallback model. Records every request body so tests can
    /// assert what BOTH the first request and its replacement actually sent.
    /// Success replies are SSE (ChatGPT OAuth mode requires streaming).
    struct LunaUnavailableFixture {
        requests: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    }

    impl LunaUnavailableFixture {
        /// Bind a loopback port, return the fixture plus its base URL (known
        /// before the handler thread takes ownership of the listener).
        fn spawn() -> (Self, String) {
            use std::io::{Read, Write};
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let base_url = format!("http://{}", listener.local_addr().unwrap());
            listener.set_nonblocking(true).unwrap();
            let requests: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>> =
                std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let recorder = std::sync::Arc::clone(&requests);
            std::thread::spawn(move || {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                let mut served = 0;
                while served < 2 && std::time::Instant::now() < deadline {
                    let (mut stream, _) = match listener.accept() {
                        Ok(value) => value,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(std::time::Duration::from_millis(5));
                            continue;
                        }
                        Err(error) => panic!("{error}"),
                    };
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                        .unwrap();
                    let mut bytes = Vec::new();
                    let (header_end, length) = loop {
                        let mut chunk = [0; 4096];
                        let n = stream.read(&mut chunk).unwrap();
                        assert!(n > 0);
                        bytes.extend_from_slice(&chunk[..n]);
                        if let Some(offset) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                            let headers = String::from_utf8_lossy(&bytes[..offset]);
                            let length = headers
                                .lines()
                                .find_map(|line| {
                                    let (key, value) = line.split_once(':')?;
                                    key.eq_ignore_ascii_case("content-length")
                                        .then(|| value.trim().parse::<usize>().unwrap())
                                })
                                .unwrap_or(0);
                            break (offset + 4, length);
                        }
                    };
                    while bytes.len() < header_end + length {
                        let mut chunk = [0; 4096];
                        let n = stream.read(&mut chunk).unwrap();
                        assert!(n > 0);
                        bytes.extend_from_slice(&chunk[..n]);
                    }
                    let body: serde_json::Value =
                        serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
                    recorder.lock().unwrap().push(body.clone());
                    let response = if body["model"] == SIDECAR_OPENAI_MODEL {
                        // Same denial the production classifier recognizes
                        // ("model ... not available" + 404).
                        served += 1;
                        (
                            "HTTP/1.1 404 Not Found",
                            serde_json::json!({
                                "error": {
                                    "message": format!(
                                        "Model '{}' is not available for your account",
                                        SIDECAR_OPENAI_MODEL
                                    )
                                }
                            })
                            .to_string(),
                        )
                    } else {
                        served += 1;
                        let sse = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n\
                             data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}}}\n\n\
                             data: [DONE]\n\n"
                            .to_string();
                        ("HTTP/1.1 200 OK", sse)
                    };
                    let (status, payload) = response;
                    write!(
                        stream,
                        "{status}\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        payload.len()
                    )
                    .unwrap();
                    stream.write_all(payload.as_bytes()).unwrap();
                }
            });
            (Self { requests }, base_url)
        }

        fn recorded_requests(&self) -> Vec<serde_json::Value> {
            self.requests.lock().unwrap().clone()
        }
    }

    /// Drive the sidecar through the real ChatGPT OAuth retry path against a
    /// loopback fixture: the first request targets Luna, gets the
    /// model-unavailable 404, and the replacement runs against the fallback
    /// model. Asserts BOTH HTTP requests carry the expected reasoning effort:
    /// the configured `agents.memory_effort` must survive the retry (greptile:
    /// the retry used to hardcode `low`, re-enabling thinking under `none`).
    fn assert_retry_preserves_effort(
        configured_effort: Option<&str>,
        expected_first: &str,
        expected_retry: &str,
    ) {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().expect("create temp jcode home");
        let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
        let _openai = EnvVarGuard::unset("OPENAI_API_KEY");

        let (fixture, base_url) = LunaUnavailableFixture::spawn();
        let _base = EnvVarGuard::set_path(
            "JCODE_SIDECAR_TEST_API_BASE",
            std::path::Path::new(&base_url),
        );

        // ChatGPT OAuth credentials: a non-empty refresh token selects
        // chatgpt mode (streaming SSE), matching greptile's repro scenario.
        codex::upsert_account_from_tokens(
            "openai-chatgpt-1",
            "sidecar-test-access",
            "sidecar-test-refresh",
            None,
            None,
        )
        .expect("write ChatGPT test auth");
        codex::set_active_account_override(Some("openai-chatgpt-1".to_string()));
        crate::provider::clear_all_model_unavailability_for_account();
        // Luna available: the first request targets Luna so the retry runs.
        crate::provider::populate_account_models(vec![SIDECAR_OPENAI_MODEL.to_string()]);

        let sidecar = Sidecar::with_configured_model(None, configured_effort.map(str::to_string));
        assert_eq!(sidecar.backend_name(), "openai");

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let out = rt
            .block_on(sidecar.complete("be brief", "say ok"))
            .expect("sidecar completion should succeed via retry");
        assert_eq!(out, "ok");

        let requests = fixture.recorded_requests();
        assert_eq!(
            requests.len(),
            2,
            "expected the Luna request plus its replacement, got: {requests:?}"
        );
        let first = &requests[0];
        let retry = &requests[1];
        assert_eq!(first["model"], SIDECAR_OPENAI_MODEL);
        assert_eq!(retry["model"], SIDECAR_OPENAI_OAUTH_FALLBACK_MODEL);
        for (label, request, expected) in [
            ("first", first, expected_first),
            ("retry", retry, expected_retry),
        ] {
            assert_eq!(
                request["reasoning"]["effort"],
                serde_json::json!(expected),
                "{label} request must carry effort {expected:?}, got: {request}"
            );
        }

        codex::set_active_account_override(None);
        crate::provider::clear_all_model_unavailability_for_account();
    }

    #[test]
    fn luna_unavailable_retry_preserves_configured_effort() {
        // `agents.memory_effort = "low"`: the retry keeps low (which happens
        // to match the fallback default, but must come from the config).
        assert_retry_preserves_effort(Some("low"), "low", "low");
        // `agents.memory_effort = "medium"`: without the fix the retry sent
        // "low" instead of the configured medium.
        assert_retry_preserves_effort(Some("medium"), "medium", "medium");
        // `agents.memory_effort = "none"`: without the fix the retry sent
        // "low", silently re-enabling reasoning the user disabled.
        assert_retry_preserves_effort(Some("none"), "none", "none");
    }

    #[test]
    fn luna_unavailable_retry_without_override_keeps_fallback_default() {
        // No `agents.memory_effort`: the per-model defaults apply. Luna's
        // `none` on the first request, the fallback's `low` on the retry.
        assert_retry_preserves_effort(None, "none", "low");
    }

    // ---- Provider-backed sidecar (works on ALL providers) -------------------

    /// Minimal provider stub that echoes a fixed reply for `complete`, so the
    /// default `complete_simple` path the sidecar uses can be exercised without
    /// network access. Stands in for any of the 8 real providers.
    struct StubProvider {
        name: &'static str,
        reply: String,
    }

    #[async_trait::async_trait]
    impl crate::provider::Provider for StubProvider {
        async fn complete(
            &self,
            _messages: &[crate::message::Message],
            _tools: &[crate::message::ToolDefinition],
            _system: &str,
            _resume_session_id: Option<&str>,
        ) -> Result<crate::provider::EventStream> {
            let reply = self.reply.clone();
            let stream = futures::stream::once(async move {
                Ok(jcode_message_types::StreamEvent::TextDelta(reply))
            });
            Ok(Box::pin(stream))
        }

        fn name(&self) -> &str {
            self.name
        }

        fn model(&self) -> String {
            format!("{}-model", self.name)
        }

        fn fork(&self) -> std::sync::Arc<dyn crate::provider::Provider> {
            std::sync::Arc::new(StubProvider {
                name: self.name,
                reply: self.reply.clone(),
            })
        }
    }

    /// Stub that records the model it was switched to.
    struct SwitchableProvider(std::sync::Mutex<String>);

    #[async_trait::async_trait]
    impl crate::provider::Provider for SwitchableProvider {
        async fn complete(
            &self,
            _messages: &[crate::message::Message],
            _tools: &[crate::message::ToolDefinition],
            _system: &str,
            _resume_session_id: Option<&str>,
        ) -> Result<crate::provider::EventStream> {
            anyhow::bail!("no network in tests")
        }

        fn name(&self) -> &str {
            "switchable"
        }

        fn model(&self) -> String {
            self.0.lock().unwrap().clone()
        }

        fn set_model(&self, model: &str) -> Result<()> {
            *self.0.lock().unwrap() = model.to_string();
            Ok(())
        }

        fn fork(&self) -> std::sync::Arc<dyn crate::provider::Provider> {
            std::sync::Arc::new(SwitchableProvider(std::sync::Mutex::new(self.model())))
        }
    }

    /// The local TUI end-of-session path builds its sidecar with
    /// `for_session`, so a session's `/agents memory` choice must win over the
    /// global default there too.
    #[test]
    fn session_sidecar_uses_session_memory_model_override() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().expect("create temp jcode home");
        let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
        std::fs::write(
            temp.path().join("config.toml"),
            "[agents]\nmemory_model = \"global-memory-model\"\n",
        )
        .unwrap();
        crate::provider::set_active_provider(std::sync::Arc::new(SwitchableProvider(
            std::sync::Mutex::new("coordinator-model".into()),
        )));
        let mut session =
            crate::session::Session::create_with_id("sidecar_memory".into(), None, None);
        session.save_prepared().unwrap();
        session
            .set_agent_model_override("memory", Some("session-memory-model".into()))
            .unwrap();
        assert_eq!(
            Sidecar::for_session(&session).model_name(),
            "session-memory-model"
        );
        session.set_agent_model_override("memory", None).unwrap();
        assert_eq!(
            Sidecar::for_session(&session).model_name(),
            "global-memory-model"
        );
    }

    /// A stub whose set_reasoning_effort records the request, so tests can
    /// verify the sidecar pins `agents.memory_effort` on the fork. The fork
    /// shares the recorder so the test observes what the sidecar's fork got.
    #[derive(Default)]
    struct EffortRecordingStub {
        set_effort: std::sync::Arc<std::sync::Mutex<Option<String>>>,
        rejected: std::sync::atomic::AtomicBool,
    }

    #[async_trait::async_trait]
    impl crate::provider::Provider for EffortRecordingStub {
        async fn complete(
            &self,
            _messages: &[crate::message::Message],
            _tools: &[crate::message::ToolDefinition],
            _system: &str,
            _resume_session_id: Option<&str>,
        ) -> Result<crate::provider::EventStream> {
            let stream = futures::stream::once(async {
                Ok(jcode_message_types::StreamEvent::TextDelta(
                    "ok".to_string(),
                ))
            });
            Ok(Box::pin(stream))
        }

        fn name(&self) -> &str {
            "effort-stub"
        }

        fn model(&self) -> String {
            "effort-stub-model".to_string()
        }

        fn fork(&self) -> std::sync::Arc<dyn crate::provider::Provider> {
            std::sync::Arc::new(EffortRecordingStub {
                set_effort: std::sync::Arc::clone(&self.set_effort),
                rejected: std::sync::atomic::AtomicBool::new(
                    self.rejected.load(std::sync::atomic::Ordering::Relaxed),
                ),
            })
        }

        fn set_reasoning_effort(&self, effort: &str) -> Result<()> {
            if self.rejected.load(std::sync::atomic::Ordering::Relaxed) {
                anyhow::bail!("stub rejects efforts");
            }
            *self.set_effort.lock().unwrap() = Some(effort.to_string());
            Ok(())
        }
    }

    impl EffortRecordingStub {
        fn recorded_effort(&self) -> Option<String> {
            self.set_effort.lock().unwrap().clone()
        }
    }

    #[test]
    fn sidecar_pins_memory_effort_on_provider_fork() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().expect("create temp jcode home");
        let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
        let _openai = EnvVarGuard::unset("OPENAI_API_KEY");

        let stub = std::sync::Arc::new(EffortRecordingStub::default());
        crate::provider::set_active_provider(stub.clone());

        let sidecar = Sidecar::with_configured_model(None, Some("low".to_string()));
        assert_eq!(sidecar.backend_name(), "provider");
        assert_eq!(
            stub.recorded_effort(),
            Some("low".to_string()),
            "the fork must receive the configured memory effort"
        );
    }

    #[test]
    fn sidecar_ignores_rejected_memory_effort_on_provider_fork() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().expect("create temp jcode home");
        let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
        let _openai = EnvVarGuard::unset("OPENAI_API_KEY");

        let stub = std::sync::Arc::new(EffortRecordingStub {
            set_effort: std::sync::Arc::new(std::sync::Mutex::new(None)),
            rejected: std::sync::atomic::AtomicBool::new(true),
        });
        crate::provider::set_active_provider(stub.clone());

        // Must construct without panicking; the rejection is a logged warning.
        let sidecar = Sidecar::with_configured_model(None, Some("low".to_string()));
        assert_eq!(sidecar.backend_name(), "provider");
        assert_eq!(stub.recorded_effort(), None);
    }

    /// With NO OpenAI/Claude credentials, the sidecar must select the live
    /// agent provider (the universal path) instead of failing. This is the core
    /// guarantee that memory features work on every provider, not just two.
    #[test]
    fn sidecar_uses_active_provider_when_no_oauth_creds() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().expect("create temp jcode home");
        let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
        let _openai = EnvVarGuard::unset("OPENAI_API_KEY");

        // Simulate running on a non-OpenAI/Claude provider (e.g. Gemini).
        crate::provider::set_active_provider(std::sync::Arc::new(StubProvider {
            name: "gemini",
            reply: "[2,1]".to_string(),
        }));

        let sidecar = Sidecar::with_configured_model(None, None);
        assert_eq!(
            sidecar.backend_name(),
            "provider",
            "with no OAuth creds, the sidecar must route through the active provider"
        );

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let out = rt
            .block_on(sidecar.complete("rank these", "1. a\n2. b"))
            .expect("provider-backed completion should succeed");
        assert_eq!(out, "[2,1]", "sidecar must return the provider's text");
    }

    #[test]
    fn provider_sidecar_keeps_the_route_selected_at_construction() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().expect("create temp jcode home");
        let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
        let _openai = EnvVarGuard::unset("OPENAI_API_KEY");

        crate::provider::set_active_provider(std::sync::Arc::new(StubProvider {
            name: "configured-profile",
            reply: "configured-profile-response".to_string(),
        }));
        let sidecar = Sidecar::with_configured_model(None, None);

        // Model switches and catalog refreshes can replace the process-global
        // provider while a background memory request is queued. Dispatch must
        // still use the exact profile fork selected above.
        crate::provider::set_active_provider(std::sync::Arc::new(StubProvider {
            name: "fallback-route",
            reply: "wrong-fallback-response".to_string(),
        }));

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let out = rt
            .block_on(sidecar.complete("extract memories", "conversation"))
            .expect("provider-backed completion should retain its route");

        assert_eq!(sidecar.model_name(), "configured-profile-model");
        assert_eq!(out, "configured-profile-response");
    }

    /// Every provider jcode supports should drive the sidecar end-to-end via the
    /// universal `complete_simple` path. We iterate over each provider label to
    /// make the "works for ALL providers" guarantee explicit and regression-proof.
    #[test]
    fn sidecar_provider_path_works_for_all_providers() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().expect("create temp jcode home");
        let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
        let _openai = EnvVarGuard::unset("OPENAI_API_KEY");

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        for provider in [
            "claude",
            "openai",
            "copilot",
            "antigravity",
            "gemini",
            "cursor",
            "bedrock",
            "openrouter",
        ] {
            crate::provider::set_active_provider(std::sync::Arc::new(StubProvider {
                name: provider,
                reply: "[1]".to_string(),
            }));
            let sidecar = Sidecar::with_configured_model(None, None);
            assert_eq!(
                sidecar.backend_name(),
                "provider",
                "{provider}: sidecar should use the provider path with no OAuth creds"
            );
            let out = rt
                .block_on(sidecar.complete("sys", "user"))
                .unwrap_or_else(|e| panic!("{provider}: provider-backed completion failed: {e}"));
            assert_eq!(out, "[1]", "{provider}: sidecar must echo provider output");
        }
    }

    #[test]
    fn test_is_anthropic_oauth_forbidden() {
        // The exact error string the sidecar surfaces from a forbidden OAuth org.
        let forbidden = anyhow::anyhow!(
            "Claude API error (403 Forbidden): {{\"type\":\"error\",\"error\":{{\"type\":\"permission_error\",\"message\":\"OAuth authentication is currently not allowed for this organization.\"}}}}"
        );
        assert!(is_anthropic_oauth_forbidden(&forbidden));

        // Unrelated failures must NOT trigger the API-key fallback.
        assert!(!is_anthropic_oauth_forbidden(&anyhow::anyhow!(
            "Claude API error (401 Unauthorized): bad token"
        )));
        assert!(!is_anthropic_oauth_forbidden(&anyhow::anyhow!(
            "Failed to send request to Claude API"
        )));
        // A 403 from a permission_error (the organization gate) still counts even
        // if the human-readable message phrasing changes slightly.
        assert!(is_anthropic_oauth_forbidden(&anyhow::anyhow!(
            "Claude API error (403 Forbidden): {{\"error\":{{\"type\":\"permission_error\"}}}}"
        )));
    }

    #[test]
    fn test_build_claude_api_key_system_param_omits_identity_spoof() {
        // API-key path must NOT impersonate the official Claude Code CLI.
        let none = build_claude_api_key_system_param("");
        assert!(none.is_none(), "empty system => no system param");

        let ClaudeApiSystem::Blocks(blocks) =
            build_claude_api_key_system_param("be terse").expect("system present");
        assert_eq!(blocks.len(), 1, "only the caller's system prompt is sent");
        assert_eq!(blocks[0].text, "be terse");

        // The OAuth builder, by contrast, injects the Claude Code identity spoof.
        let ClaudeApiSystem::Blocks(oauth_blocks) =
            build_claude_system_param("be terse").expect("oauth system present");
        assert!(
            oauth_blocks.iter().any(|b| b.text == CLAUDE_CODE_IDENTITY),
            "oauth path keeps the identity block"
        );
    }

    #[test]
    fn test_anthropic_sidecar_prefers_api_key_respects_pinned_mode() {
        // Pinning the runtime to API-key mode must make the sidecar prefer the key.
        let _g =
            EnvVarGuard::set_path("JCODE_RUNTIME_PROVIDER", std::path::Path::new("claude-api"));
        assert!(
            anthropic_sidecar_prefers_api_key(),
            "claude-api runtime => prefer API key"
        );

        // Pinning to OAuth mode must NOT prefer the key.
        let _g2 = EnvVarGuard::set_path("JCODE_RUNTIME_PROVIDER", std::path::Path::new("claude"));
        assert!(
            !anthropic_sidecar_prefers_api_key(),
            "claude (oauth) runtime => do not force API key"
        );
    }
}
