//! Lightweight sidecar client for fast, cheap model calls.
//!
//! Used for memory relevance verification and other quick tasks that don't
//! need the full Agent SDK infrastructure.
//!
//! Automatically selects the best available backend:
//! - OpenAI (gpt-5.3-codex-spark) if Codex credentials are available
//! - Claude (claude-haiku-4-5-20241022) if Claude credentials are available

use crate::auth;
use anyhow::{Context, Result};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

/// Fast/cheap OpenAI model used when Codex credentials are available.
pub const SIDECAR_OPENAI_MODEL: &str = "gpt-5.3-codex-spark";
const SIDECAR_OPENAI_OAUTH_FALLBACK_MODEL: &str = "gpt-5.4";
const SIDECAR_OPENAI_OAUTH_FALLBACK_REASONING: &str = "low";

/// Fast/cheap Claude model used when only Claude credentials are available.
const SIDECAR_CLAUDE_MODEL: &str = "claude-haiku-4-5-20241022";

/// OpenAI Responses API
const OPENAI_API_BASE: &str = "https://api.openai.com/v1";
const CHATGPT_API_BASE: &str = "https://chatgpt.com/backend-api/codex";
const OPENAI_RESPONSES_PATH: &str = "responses";
const OPENAI_ORIGINATOR: &str = "codex_cli_rs";

/// Claude Messages API endpoint (with beta=true for OAuth)
const CLAUDE_API_URL: &str = "https://api.anthropic.com/v1/messages?beta=true";

/// Claude Messages API endpoint for direct API-key access (no OAuth beta flag).
const CLAUDE_API_KEY_URL: &str = "https://api.anthropic.com/v1/messages";

/// User-Agent for OAuth requests (must match Claude CLI format)
const CLAUDE_CLI_USER_AGENT: &str = "claude-cli/1.0.0";

/// Beta headers required for OAuth
const OAUTH_BETA_HEADERS: &str = "oauth-2025-04-20,claude-code-20250219";

/// Claude Code identity block required for OAuth direct API access
const CLAUDE_CODE_IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";
const CLAUDE_CODE_JCODE_NOTICE: &str = "You are jcode, powered by Claude Code. You are a third-party CLI, not the official Claude Code CLI.";

/// Maximum tokens for sidecar responses (keep small for speed/cost)
const DEFAULT_MAX_TOKENS: u32 = 1024;

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
    /// Optional explicit reasoning effort override (OpenAI Responses API).
    /// When `Some`, this effort is always sent; when `None`, the default
    /// per-model behavior applies. Used by the memory benchmark to pin
    /// GPT-5.5 with no thinking.
    reasoning_override: Option<String>,
}

impl Sidecar {
    /// Create a new sidecar client, auto-selecting the best available backend.
    /// Prefers OpenAI (codex-spark) if creds exist, falls back to Claude.
    pub fn new() -> Self {
        let configured_model = crate::config::config().agents.memory_model.clone();
        Self::with_configured_model(configured_model)
    }

    fn with_configured_model(configured_model: Option<String>) -> Self {
        let (backend, model) = if let Some(model) = configured_model {
            match crate::provider::provider_for_model(&model) {
                Some("openai") => (SidecarBackend::OpenAI, model),
                Some("claude") => (SidecarBackend::Claude, model),
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
            reasoning_override: None,
        }
    }

    /// Pick the best available sidecar backend.
    ///
    /// Preference order:
    /// 1. OpenAI codex-spark (dedicated fast/cheap OAuth path) if Codex creds exist.
    /// 2. Claude haiku (dedicated fast/cheap OAuth path) if Claude creds exist.
    /// 3. The live agent provider (works for EVERY provider jcode supports:
    ///    Copilot, Antigravity, Gemini, Cursor, Bedrock, OpenRouter, and even
    ///    OpenAI/Claude API-key setups), dispatched via `complete_simple`.
    ///
    /// Only when no provider is registered at all do we fall back to Claude,
    /// which then fails on use with a clear credentials error.
    fn auto_select_backend() -> (SidecarBackend, String) {
        if auth::codex::load_credentials().is_ok() {
            (SidecarBackend::OpenAI, SIDECAR_OPENAI_MODEL.to_string())
        } else if auth::claude::load_credentials().is_ok() {
            (SidecarBackend::Claude, SIDECAR_CLAUDE_MODEL.to_string())
        } else if let Some(provider) = crate::provider::active_provider_fork() {
            // Dispatch through whatever provider the user is running on. The
            // model string is informational here; the provider already has the
            // user's selected model and routes accordingly.
            (SidecarBackend::Provider, provider.model())
        } else {
            // No credentials and no live provider: default to Claude so the
            // eventual error message is actionable.
            (SidecarBackend::Claude, SIDECAR_CLAUDE_MODEL.to_string())
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
        let provider = crate::provider::active_provider_fork().context(
            "No active provider registered for sidecar; memory features require a logged-in provider",
        )?;
        provider
            .complete_simple(user_message, system)
            .await
            .context("Sidecar completion via active provider failed")
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
        let base = if is_chatgpt_mode {
            CHATGPT_API_BASE
        } else {
            OPENAI_API_BASE
        };
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

                let fallback = self
                    .complete_openai_with_model(
                        &url,
                        creds.access_token.as_str(),
                        creds.account_id.as_deref(),
                        is_chatgpt_mode,
                        system,
                        user_message,
                        SIDECAR_OPENAI_OAUTH_FALLBACK_MODEL,
                        Some(SIDECAR_OPENAI_OAUTH_FALLBACK_REASONING),
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
                        // Both the codex-spark model and the gpt-5.4 OAuth
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
            collect_openai_sse_text(response)
                .await
                .map_err(OpenAiSidecarError::other)
        } else {
            let result: serde_json::Value = response
                .json()
                .await
                .context("Failed to parse OpenAI API response")
                .map_err(OpenAiSidecarError::other)?;
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

        let request = ClaudeMessagesRequest {
            model: &self.model,
            max_tokens: self.max_tokens,
            system: build_claude_system_param(system),
            messages: vec![ClaudeMessage {
                role: "user",
                content: user_message,
            }],
        };

        let response = crate::provider::anthropic::apply_oauth_attribution_headers(
            self.client
                .post(CLAUDE_API_URL)
                .header("Authorization", format!("Bearer {}", creds.access_token))
                .header("User-Agent", CLAUDE_CLI_USER_AGENT)
                .header("anthropic-version", "2023-06-01")
                .header("anthropic-beta", OAUTH_BETA_HEADERS)
                .header("content-type", "application/json")
                .json(&request),
            &crate::provider::anthropic::new_oauth_request_id(),
        )
        .send()
        .await
        .context("Failed to send request to Claude API")?;

        Self::parse_claude_response(response).await
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
        let request = ClaudeMessagesRequest {
            model: &self.model,
            max_tokens: self.max_tokens,
            system: build_claude_api_key_system_param(system),
            messages: vec![ClaudeMessage {
                role: "user",
                content: user_message,
            }],
        };

        let response = self
            .client
            .post(CLAUDE_API_KEY_URL)
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "prompt-caching-2024-07-31")
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await
            .context("Failed to send request to Claude API")?;

        Self::parse_claude_response(response).await
    }

    /// Shared response parsing for both Claude credential paths.
    async fn parse_claude_response(response: reqwest::Response) -> Result<String> {
        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!("Claude API error ({}): {}", status, error_text);
        }

        let result: ClaudeMessagesResponse = response
            .json()
            .await
            .context("Failed to parse Claude API response")?;

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

fn resolve_openai_request_model(
    preferred_model: &str,
    is_chatgpt_mode: bool,
) -> (&str, Option<&'static str>) {
    if !is_chatgpt_mode || preferred_model != SIDECAR_OPENAI_MODEL {
        return (preferred_model, None);
    }

    match crate::provider::is_model_available_for_account(SIDECAR_OPENAI_MODEL) {
        Some(false) => (
            SIDECAR_OPENAI_OAUTH_FALLBACK_MODEL,
            Some(SIDECAR_OPENAI_OAUTH_FALLBACK_REASONING),
        ),
        _ => (SIDECAR_OPENAI_MODEL, None),
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
            Self::Api { status, body } => {
                anyhow::anyhow!("OpenAI API error ({}): {}", status, body)
            }
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
async fn collect_openai_sse_text(response: reqwest::Response) -> Result<String> {
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
}

// Claude API types

#[derive(Serialize)]
struct ClaudeMessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<ClaudeApiSystem<'a>>,
    messages: Vec<ClaudeMessage<'a>>,
}

#[derive(Serialize)]
struct ClaudeMessage<'a> {
    role: &'a str,
    content: &'a str,
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
    #[serde(rename = "usage")]
    _usage: Option<ClaudeUsage>,
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
    #[serde(rename = "input_tokens")]
    _input_tokens: u32,
    #[serde(rename = "output_tokens")]
    _output_tokens: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::codex;
    use std::ffi::OsString;

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
        assert_eq!(SIDECAR_FAST_MODEL, "gpt-5.3-codex-spark");
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

        let sidecar = Sidecar::with_configured_model(None);
        assert_eq!(sidecar.backend, SidecarBackend::OpenAI);
        assert_eq!(sidecar.model, SIDECAR_OPENAI_MODEL);
        codex::set_active_account_override(None);
        crate::auth::claude::set_active_account_override(None);
    }

    #[test]
    fn test_chatgpt_oauth_keeps_spark_when_available() {
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
        assert_eq!(reasoning, None);

        codex::set_active_account_override(None);
    }

    #[test]
    fn test_chatgpt_oauth_falls_back_to_gpt_5_4_low_when_spark_unavailable() {
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
    fn test_build_openai_request_adds_low_reasoning_only_for_fallback() {
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

        let spark_request =
            build_openai_request(SIDECAR_OPENAI_MODEL, "system", "hello", true, None);
        assert!(spark_request.get("reasoning").is_none());
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

        let sidecar = Sidecar::with_configured_model(None);
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
            let sidecar = Sidecar::with_configured_model(None);
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
