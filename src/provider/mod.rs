pub mod anthropic;
pub mod antigravity;
pub mod claude;
pub mod cli_common;
pub mod copilot;
pub mod cursor;
pub mod openai;
pub mod openrouter;

use crate::auth;
use crate::message::{ContentBlock, Message, Role, StreamEvent, ToolDefinition};
use anyhow::Result;
use async_trait::async_trait;
use futures::Stream;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

// Re-export native tool result types for use by agent
pub use claude::{NativeToolResult, NativeToolResultSender};

/// Stream of events from a provider
pub type EventStream = Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>;

/// A single route to access a model: model + provider + API method
#[derive(Debug, Clone)]
pub struct ModelRoute {
    pub model: String,
    pub provider: String,
    pub api_method: String,
    pub available: bool,
    pub detail: String,
}

/// Provider trait for LLM backends
#[async_trait]
pub trait Provider: Send + Sync {
    /// Send messages and get a streaming response
    /// resume_session_id: Optional session ID to resume a previous conversation (provider-specific)
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream>;

    /// Send messages with split system prompt for better caching
    /// system_static: Static content (CLAUDE.md, base prompt) - cached
    /// system_dynamic: Dynamic content (date, git status, memory) - not cached
    /// Default implementation combines them and calls complete()
    async fn complete_split(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system_static: &str,
        system_dynamic: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        // Default: combine static and dynamic parts
        let combined = if system_dynamic.is_empty() {
            system_static.to_string()
        } else if system_static.is_empty() {
            system_dynamic.to_string()
        } else {
            format!("{}\n\n{}", system_static, system_dynamic)
        };
        self.complete(messages, tools, &combined, resume_session_id)
            .await
    }

    /// Get the provider name
    fn name(&self) -> &str;

    /// Get the model identifier being used
    fn model(&self) -> String {
        "unknown".to_string()
    }

    /// Set the model to use (returns error if model not supported)
    fn set_model(&self, _model: &str) -> Result<()> {
        Err(anyhow::anyhow!(
            "This provider does not support model switching"
        ))
    }

    /// List available models for this provider
    fn available_models(&self) -> Vec<&'static str> {
        vec![]
    }

    /// List available models for display/autocomplete (may be dynamic).
    fn available_models_display(&self) -> Vec<String> {
        self.available_models()
            .iter()
            .map(|m| (*m).to_string())
            .collect()
    }

    /// List known providers for a model (OpenRouter-style @provider autocomplete).
    fn available_providers_for_model(&self, _model: &str) -> Vec<String> {
        Vec::new()
    }

    /// Provider details for model picker: Vec<(provider_name, detail_string)>.
    /// Uses cached endpoint data when available (sync, no network).
    fn provider_details_for_model(&self, _model: &str) -> Vec<(String, String)> {
        Vec::new()
    }

    /// Get all model routes for the unified picker.
    /// Returns every (model, provider, api_method, available, detail) combination.
    fn model_routes(&self) -> Vec<ModelRoute> {
        Vec::new()
    }

    /// Prefetch any dynamic model lists (default: no-op).
    async fn prefetch_models(&self) -> Result<()> {
        Ok(())
    }

    /// Get the reasoning effort level (if applicable, e.g., OpenAI)
    fn reasoning_effort(&self) -> Option<String> {
        None
    }

    /// Set the reasoning effort level (if applicable, e.g., OpenAI)
    fn set_reasoning_effort(&self, _effort: &str) -> Result<()> {
        Err(anyhow::anyhow!(
            "This provider does not support reasoning effort"
        ))
    }

    /// Get ordered list of available reasoning effort levels
    fn available_efforts(&self) -> Vec<&'static str> {
        vec![]
    }

    /// Returns true if the provider executes tools internally (e.g., Claude Code CLI).
    /// When true, jcode should NOT execute tools locally - just record the tool calls.
    fn handles_tools_internally(&self) -> bool {
        false
    }

    /// Returns true if jcode should use its own compaction for this provider.
    fn supports_compaction(&self) -> bool {
        false
    }

    /// Return the context window size (in tokens) for the current model.
    /// Providers should override this to return accurate, dynamic values.
    /// Falls back to hardcoded lookup if not overridden.
    fn context_window(&self) -> usize {
        context_limit_for_model(&self.model()).unwrap_or(DEFAULT_CONTEXT_LIMIT)
    }

    /// Create a new provider instance with the same credentials/config and model,
    /// but independent mutable state (e.g., model selection).
    fn fork(&self) -> Arc<dyn Provider>;

    /// Get a sender for native tool results (if the provider supports it).
    /// This is used by the Claude provider to send results back to a bridge (if any).
    fn native_result_sender(&self) -> Option<NativeToolResultSender> {
        None
    }

    /// Simple completion that returns text directly (no streaming).
    /// Useful for internal tasks like compaction summaries.
    /// Default implementation uses complete() and collects the response.
    async fn complete_simple(&self, prompt: &str, system: &str) -> Result<String> {
        use futures::StreamExt;

        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: prompt.to_string(),
                cache_control: None,
            }],
            timestamp: None,
        }];

        let response = self.complete(&messages, &[], system, None).await?;
        let mut result = String::new();
        tokio::pin!(response);

        while let Some(event) = response.next().await {
            if let Ok(StreamEvent::TextDelta(text)) = event {
                result.push_str(&text);
            }
        }

        Ok(result)
    }
}

/// Available models (shown in /model list)
pub const ALL_CLAUDE_MODELS: &[&str] = &[
    "claude-opus-4-6",
    "claude-sonnet-4-6",
    "claude-opus-4-5-20251101",
    "claude-sonnet-4-20250514",
    "claude-haiku-4-5-20241022",
];

pub const ALL_OPENAI_MODELS: &[&str] = &[
    "codex-mini-latest",
    "gpt-5.3-codex",
    "gpt-5.3-codex-spark",
    "gpt-5.2-chat-latest",
    "gpt-5.2-codex",
    "gpt-5.2-pro",
    "gpt-5.1-codex-mini",
    "gpt-5.1-codex-max",
    "gpt-5.2",
    "gpt-5.1-chat-latest",
    "gpt-5.1",
    "gpt-5.1-codex",
    "gpt-5-chat-latest",
    "gpt-5-codex",
    "gpt-5-codex-mini",
    "gpt-5-pro",
    "gpt-5-mini",
    "gpt-5-nano",
    "gpt-5",
];

/// Default context window size when model-specific data isn't known.
pub const DEFAULT_CONTEXT_LIMIT: usize = 200_000;

/// Dynamic cache of model context window sizes, populated from API at startup.
static CONTEXT_LIMIT_CACHE: std::sync::LazyLock<RwLock<HashMap<String, usize>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));

/// Dynamic cache of models actually available for this account (populated from Codex API).
/// When populated, only models in this set should be offered/accepted for the OpenAI provider.
static ACCOUNT_AVAILABLE_MODELS: std::sync::LazyLock<RwLock<Option<Vec<String>>>> =
    std::sync::LazyLock::new(|| RwLock::new(None));

/// Look up a cached context limit for a model.
fn get_cached_context_limit(model: &str) -> Option<usize> {
    let cache = CONTEXT_LIMIT_CACHE.read().ok()?;
    cache.get(model).copied()
}

/// Populate the context limit cache from API-provided model data.
/// Called once at startup when OpenAI OAuth credentials are available.
pub fn populate_context_limits(models: HashMap<String, usize>) {
    if let Ok(mut cache) = CONTEXT_LIMIT_CACHE.write() {
        for (model, limit) in &models {
            crate::logging::info(&format!(
                "Context limit cache: {} = {}k",
                model,
                limit / 1000
            ));
            cache.insert(model.clone(), *limit);
        }
    }
}

/// Populate the account-available model list (called once at startup from the Codex API).
pub fn populate_account_models(slugs: Vec<String>) {
    if !slugs.is_empty() {
        if let Ok(mut available) = ACCOUNT_AVAILABLE_MODELS.write() {
            crate::logging::info(&format!(
                "Account available models: {}",
                slugs.join(", ")
            ));
            *available = Some(slugs);
        }
    }
}

/// Check if a model is available for the current account.
/// Returns None if the dynamic list hasn't been fetched yet (assume available).
/// Returns Some(true) if the model is in the account's available list.
/// Returns Some(false) if the model is NOT in the account's available list.
pub fn is_model_available_for_account(model: &str) -> Option<bool> {
    let cache = ACCOUNT_AVAILABLE_MODELS.read().ok()?;
    match cache.as_ref() {
        Some(models) => Some(models.iter().any(|m| m == model)),
        None => None,
    }
}

/// Preferred model order for fallback selection.
/// If the desired model isn't available, we try these in order.
const OPENAI_MODEL_PREFERENCE: &[&str] = &[
    "gpt-5.3-codex-spark",
    "gpt-5.3-codex",
    "gpt-5.2-codex",
    "gpt-5.1-codex-max",
    "gpt-5.1-codex",
];

/// Get the best available OpenAI model, falling back through the preference list.
/// Returns None if the dynamic model list hasn't been fetched yet.
pub fn get_best_available_openai_model() -> Option<String> {
    let cache = ACCOUNT_AVAILABLE_MODELS.read().ok()?;
    let models = cache.as_ref()?;
    for preferred in OPENAI_MODEL_PREFERENCE {
        if models.iter().any(|m| m == preferred) {
            return Some(preferred.to_string());
        }
    }
    models.first().cloned()
}

/// Fetch context window sizes from the Codex backend API.
/// Returns a map of model slug -> context_window tokens.
pub async fn fetch_openai_context_limits(access_token: &str) -> Result<HashMap<String, usize>> {
    let client = reqwest::Client::new();
    let resp = client
        .get("https://chatgpt.com/backend-api/codex/models?client_version=1.0.0")
        .header("Authorization", format!("Bearer {}", access_token))
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("Failed to fetch model context limits: {}", resp.status());
    }

    let data: serde_json::Value = resp.json().await?;
    let mut limits = HashMap::new();

    if let Some(models) = data.get("models").and_then(|m| m.as_array()) {
        for model in models {
            if let (Some(slug), Some(ctx)) = (
                model.get("slug").and_then(|s| s.as_str()),
                model.get("context_window").and_then(|c| c.as_u64()),
            ) {
                limits.insert(slug.to_string(), ctx as usize);
            }
        }
    }

    Ok(limits)
}

/// Return the context window size in tokens for a given model, if known.
///
/// First checks the dynamic cache (populated from the Codex backend API at startup),
/// then falls back to hardcoded defaults.
pub fn context_limit_for_model(model: &str) -> Option<usize> {
    // Check dynamic cache first (populated from API)
    if let Some(limit) = get_cached_context_limit(model) {
        return Some(limit);
    }

    // Hardcoded fallbacks
    let model = model.to_lowercase();

    // Spark variant has a smaller context window than the full codex model
    if model.starts_with("gpt-5.3-codex-spark") {
        return Some(128_000);
    }

    if model.starts_with("gpt-5.2-chat")
        || model.starts_with("gpt-5.1-chat")
        || model.starts_with("gpt-5-chat")
    {
        return Some(128_000);
    }

    // Most GPT-5.x codex/reasoning models: 272k per Codex backend API
    if model.starts_with("gpt-5") {
        return Some(272_000);
    }

    if model.starts_with("claude-opus-4-6") || model.starts_with("claude-opus-4.6") {
        return Some(1_048_576);
    }

    if model.starts_with("claude-sonnet-4-6") || model.starts_with("claude-sonnet-4.6") {
        return Some(1_048_576);
    }

    if model.starts_with("claude-opus-4-5") || model.starts_with("claude-opus-4.5") {
        return Some(200_000);
    }

    None
}

/// Detect which provider a model belongs to
pub fn provider_for_model(model: &str) -> Option<&'static str> {
    if ALL_CLAUDE_MODELS.contains(&model) {
        Some("claude")
    } else if ALL_OPENAI_MODELS.contains(&model) {
        Some("openai")
    } else if model.contains('/') {
        // OpenRouter uses provider/model format (e.g., "anthropic/claude-sonnet-4")
        Some("openrouter")
    } else {
        None
    }
}

/// MultiProvider wraps multiple providers and allows seamless model switching
pub struct MultiProvider {
    /// Claude Code CLI provider
    claude: Option<claude::ClaudeProvider>,
    /// Direct Anthropic API provider (no Python dependency)
    anthropic: Option<anthropic::AnthropicProvider>,
    openai: Option<openai::OpenAIProvider>,
    /// OpenRouter API provider (200+ models from various providers)
    openrouter: Option<openrouter::OpenRouterProvider>,
    active: RwLock<ActiveProvider>,
    has_claude_creds: bool,
    has_openai_creds: bool,
    has_openrouter_creds: bool,
    /// Use Claude CLI instead of direct API (legacy mode)
    use_claude_cli: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum ActiveProvider {
    Claude,
    OpenAI,
    OpenRouter,
}

impl MultiProvider {
    /// Create a new MultiProvider, detecting available credentials
    pub fn new() -> Self {
        let has_claude_creds = auth::claude::load_credentials().is_ok();
        let has_openai_creds = auth::codex::load_credentials().is_ok();
        let has_openrouter_creds = openrouter::OpenRouterProvider::has_credentials();

        // Check if we should use Claude CLI instead of direct API.
        // Set JCODE_USE_CLAUDE_CLI=1 to use Claude Code CLI (deprecated legacy mode).
        // Default is now direct Anthropic API for simpler session management.
        let use_claude_cli = std::env::var("JCODE_USE_CLAUDE_CLI")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        if use_claude_cli {
            crate::logging::warn(
                "JCODE_USE_CLAUDE_CLI is deprecated. Direct Anthropic API transport is preferred.",
            );
        }

        // Initialize providers based on available credentials
        // Claude CLI provider (legacy - shells out to `claude` binary)
        let claude = if has_claude_creds && use_claude_cli {
            crate::logging::info("Using Claude CLI provider (JCODE_USE_CLAUDE_CLI=1)");
            Some(claude::ClaudeProvider::new())
        } else {
            None
        };

        // Direct Anthropic API provider (default - no subprocess, jcode owns all state)
        let anthropic = if has_claude_creds && !use_claude_cli {
            Some(anthropic::AnthropicProvider::new())
        } else {
            None
        };

        let openai = if has_openai_creds {
            auth::codex::load_credentials()
                .ok()
                .map(openai::OpenAIProvider::new)
        } else {
            None
        };

        // OpenRouter provider (access 200+ models via OPENROUTER_API_KEY)
        let openrouter = if has_openrouter_creds {
            match openrouter::OpenRouterProvider::new() {
                Ok(p) => Some(p),
                Err(e) => {
                    crate::logging::info(&format!("Failed to initialize OpenRouter: {}", e));
                    None
                }
            }
        } else {
            None
        };

        // Default to Claude if available, otherwise OpenAI, then OpenRouter
        let active = if claude.is_some() || anthropic.is_some() {
            ActiveProvider::Claude
        } else if openai.is_some() {
            ActiveProvider::OpenAI
        } else if openrouter.is_some() {
            ActiveProvider::OpenRouter
        } else {
            // No credentials - default to Claude (will fail on use)
            ActiveProvider::Claude
        };

        let result = Self {
            claude,
            anthropic,
            openai,
            openrouter,
            active: RwLock::new(active),
            has_claude_creds,
            has_openai_creds,
            has_openrouter_creds,
            use_claude_cli,
        };

        // Spawn background fetch for dynamic context limits from Codex API
        if has_openai_creds {
            if let Ok(creds) = auth::codex::load_credentials() {
                let token = creds.access_token.clone();
                if !token.is_empty() {
                    tokio::spawn(async move {
                        match fetch_openai_context_limits(&token).await {
                            Ok(limits) if !limits.is_empty() => {
                                crate::logging::info(&format!(
                                    "Fetched context limits for {} OpenAI models from API",
                                    limits.len()
                                ));
                                let slugs: Vec<String> = limits.keys().cloned().collect();
                                populate_context_limits(limits);
                                populate_account_models(slugs);
                            }
                            Ok(_) => {
                                crate::logging::info(
                                    "Codex models API returned no models with context_window",
                                );
                            }
                            Err(e) => {
                                crate::logging::info(&format!(
                                    "Failed to fetch context limits from Codex API: {}",
                                    e
                                ));
                            }
                        }
                    });
                }
            }
        }

        result
    }

    /// Create with explicit initial provider preference
    pub fn with_preference(prefer_openai: bool) -> Self {
        let provider = Self::new();
        if prefer_openai && provider.openai.is_some() {
            *provider.active.write().unwrap() = ActiveProvider::OpenAI;
        }
        provider
    }

    fn active_provider(&self) -> ActiveProvider {
        *self.active.read().unwrap()
    }
}

impl Default for MultiProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl MultiProvider {
    /// Check if Anthropic OAuth usage is exhausted (both 5hr and 7d at 100%)
    fn is_claude_usage_exhausted(&self) -> bool {
        // Only check if we have Anthropic credentials
        if self.anthropic.is_none() && self.claude.is_none() {
            return false;
        }

        let usage = crate::usage::get_sync();
        // Consider exhausted if both windows are at 99% or higher
        // (give a small buffer for rounding/display issues)
        usage.five_hour >= 0.99 && usage.seven_day >= 0.99
    }

    /// Auto-fallback to OpenRouter with kimi-k2.5 if Claude is exhausted
    fn try_fallback_to_openrouter(&self) -> Option<&openrouter::OpenRouterProvider> {
        if self.is_claude_usage_exhausted() {
            if let Some(ref openrouter) = self.openrouter {
                // Switch to OpenRouter and set kimi-k2.5 as the model
                *self.active.write().unwrap() = ActiveProvider::OpenRouter;
                // Try to set the model to kimi-k2.5
                let _ = openrouter.set_model("moonshotai/kimi-k2-5");
                crate::logging::info(
                    "Auto-switched to OpenRouter (kimi-k2.5) - Claude OAuth usage exhausted",
                );
                return Some(openrouter);
            }
        }
        None
    }
}

#[async_trait]
impl Provider for MultiProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        match self.active_provider() {
            ActiveProvider::Claude => {
                // Check if Claude usage is exhausted and fallback is available
                if let Some(openrouter) = self.try_fallback_to_openrouter() {
                    return openrouter
                        .complete(messages, tools, system, resume_session_id)
                        .await;
                }

                // Prefer direct Anthropic API if available
                if let Some(ref anthropic) = self.anthropic {
                    anthropic
                        .complete(messages, tools, system, resume_session_id)
                        .await
                } else if let Some(ref claude) = self.claude {
                    claude
                        .complete(messages, tools, system, resume_session_id)
                        .await
                } else {
                    Err(anyhow::anyhow!(
                        "Claude credentials not available. Run `claude` to log in."
                    ))
                }
            }
            ActiveProvider::OpenAI => {
                if let Some(ref openai) = self.openai {
                    openai
                        .complete(messages, tools, system, resume_session_id)
                        .await
                } else {
                    Err(anyhow::anyhow!("OpenAI credentials not available. Run `jcode login --provider openai` to log in."))
                }
            }
            ActiveProvider::OpenRouter => {
                if let Some(ref openrouter) = self.openrouter {
                    openrouter
                        .complete(messages, tools, system, resume_session_id)
                        .await
                } else {
                    Err(anyhow::anyhow!("OpenRouter credentials not available. Set OPENROUTER_API_KEY environment variable."))
                }
            }
        }
    }

    /// Split system prompt completion - delegates to underlying provider for better caching
    async fn complete_split(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system_static: &str,
        system_dynamic: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        match self.active_provider() {
            ActiveProvider::Claude => {
                // Check if Claude usage is exhausted and fallback is available
                if let Some(openrouter) = self.try_fallback_to_openrouter() {
                    return openrouter
                        .complete_split(
                            messages,
                            tools,
                            system_static,
                            system_dynamic,
                            resume_session_id,
                        )
                        .await;
                }

                // Prefer direct Anthropic API for best caching support
                if let Some(ref anthropic) = self.anthropic {
                    anthropic
                        .complete_split(
                            messages,
                            tools,
                            system_static,
                            system_dynamic,
                            resume_session_id,
                        )
                        .await
                } else if let Some(ref claude) = self.claude {
                    // Claude CLI doesn't support split, fall back to combined
                    claude
                        .complete_split(
                            messages,
                            tools,
                            system_static,
                            system_dynamic,
                            resume_session_id,
                        )
                        .await
                } else {
                    Err(anyhow::anyhow!(
                        "Claude credentials not available. Run `claude` to log in."
                    ))
                }
            }
            ActiveProvider::OpenAI => {
                if let Some(ref openai) = self.openai {
                    // OpenAI doesn't support split caching, use default combined
                    openai
                        .complete_split(
                            messages,
                            tools,
                            system_static,
                            system_dynamic,
                            resume_session_id,
                        )
                        .await
                } else {
                    Err(anyhow::anyhow!("OpenAI credentials not available. Run `jcode login --provider openai` to log in."))
                }
            }
            ActiveProvider::OpenRouter => {
                if let Some(ref openrouter) = self.openrouter {
                    // OpenRouter doesn't support split caching, use default combined
                    openrouter
                        .complete_split(
                            messages,
                            tools,
                            system_static,
                            system_dynamic,
                            resume_session_id,
                        )
                        .await
                } else {
                    Err(anyhow::anyhow!("OpenRouter credentials not available. Set OPENROUTER_API_KEY environment variable."))
                }
            }
        }
    }

    fn name(&self) -> &str {
        match self.active_provider() {
            ActiveProvider::Claude => "Claude",
            ActiveProvider::OpenAI => "OpenAI",
            ActiveProvider::OpenRouter => "OpenRouter",
        }
    }

    fn model(&self) -> String {
        match self.active_provider() {
            ActiveProvider::Claude => {
                // Prefer anthropic if available
                if let Some(ref anthropic) = self.anthropic {
                    anthropic.model()
                } else if let Some(ref claude) = self.claude {
                    claude.model()
                } else {
                    "claude-opus-4-5-20251101".to_string()
                }
            }
            ActiveProvider::OpenAI => self
                .openai
                .as_ref()
                .map(|o| o.model())
                .unwrap_or_else(|| "gpt-5.3-codex-spark".to_string()),
            ActiveProvider::OpenRouter => self
                .openrouter
                .as_ref()
                .map(|o| o.model())
                .unwrap_or_else(|| "anthropic/claude-sonnet-4".to_string()),
        }
    }

    fn set_model(&self, model: &str) -> Result<()> {
        // Detect which provider this model belongs to
        let target_provider = provider_for_model(model);

        if target_provider == Some("claude") {
            if self.claude.is_none() && self.anthropic.is_none() {
                return Err(anyhow::anyhow!(
                    "Claude credentials not available. Run `claude` to log in first."
                ));
            }
            // Switch active provider to Claude
            *self.active.write().unwrap() = ActiveProvider::Claude;
            // Set on whichever is available
            if let Some(ref anthropic) = self.anthropic {
                anthropic.set_model(model)
            } else if let Some(ref claude) = self.claude {
                claude.set_model(model)
            } else {
                Ok(())
            }
        } else if target_provider == Some("openai") {
            if self.openai.is_none() {
                return Err(anyhow::anyhow!(
                    "OpenAI credentials not available. Run `jcode login --provider openai` first."
                ));
            }
            // Switch active provider to OpenAI
            *self.active.write().unwrap() = ActiveProvider::OpenAI;
            if let Some(ref openai) = self.openai {
                openai.set_model(model)
            } else {
                Ok(())
            }
        } else if target_provider == Some("openrouter") {
            if self.openrouter.is_none() {
                return Err(anyhow::anyhow!(
                    "OpenRouter credentials not available. Set OPENROUTER_API_KEY environment variable."
                ));
            }
            // Switch active provider to OpenRouter
            *self.active.write().unwrap() = ActiveProvider::OpenRouter;
            if let Some(ref openrouter) = self.openrouter {
                openrouter.set_model(model)
            } else {
                Ok(())
            }
        } else {
            // Unknown model - try current provider
            match self.active_provider() {
                ActiveProvider::Claude => {
                    if let Some(ref anthropic) = self.anthropic {
                        anthropic.set_model(model)
                    } else if let Some(ref claude) = self.claude {
                        claude.set_model(model)
                    } else {
                        Err(anyhow::anyhow!("Unknown model: {}", model))
                    }
                }
                ActiveProvider::OpenAI => {
                    if let Some(ref openai) = self.openai {
                        openai.set_model(model)
                    } else {
                        Err(anyhow::anyhow!("Unknown model: {}", model))
                    }
                }
                ActiveProvider::OpenRouter => {
                    if let Some(ref openrouter) = self.openrouter {
                        openrouter.set_model(model)
                    } else {
                        Err(anyhow::anyhow!("Unknown model: {}", model))
                    }
                }
            }
        }
    }

    fn available_models(&self) -> Vec<&'static str> {
        let mut models = Vec::new();
        models.extend_from_slice(ALL_CLAUDE_MODELS);
        models.extend_from_slice(ALL_OPENAI_MODELS);
        models
    }

    fn available_models_display(&self) -> Vec<String> {
        let mut models = Vec::new();
        models.extend(ALL_CLAUDE_MODELS.iter().map(|m| (*m).to_string()));
        models.extend(ALL_OPENAI_MODELS.iter().map(|m| (*m).to_string()));
        if let Some(ref openrouter) = self.openrouter {
            models.extend(openrouter.available_models_display());
        }
        models
    }

    fn available_providers_for_model(&self, model: &str) -> Vec<String> {
        if model.contains('/') {
            if let Some(ref openrouter) = self.openrouter {
                return openrouter.available_providers_for_model(model);
            }
        }
        Vec::new()
    }

    fn provider_details_for_model(&self, model: &str) -> Vec<(String, String)> {
        if model.contains('/') {
            if let Some(ref openrouter) = self.openrouter {
                return openrouter.provider_details_for_model(model);
            }
        }
        Vec::new()
    }

    fn model_routes(&self) -> Vec<ModelRoute> {
        let mut routes = Vec::new();
        let has_oauth = self.has_claude_creds && !self.use_claude_cli;
        let has_api_key = std::env::var("ANTHROPIC_API_KEY").is_ok();

        // Anthropic models (oauth and/or api-key)
        for model in ALL_CLAUDE_MODELS {
            if has_oauth {
                routes.push(ModelRoute {
                    model: model.to_string(),
                    provider: "Anthropic".to_string(),
                    api_method: "oauth".to_string(),
                    available: true,
                    detail: String::new(),
                });
            }
            if has_api_key {
                routes.push(ModelRoute {
                    model: model.to_string(),
                    provider: "Anthropic".to_string(),
                    api_method: "api-key".to_string(),
                    available: true,
                    detail: String::new(),
                });
            }
            if !has_oauth && !has_api_key {
                // Show as unavailable
                routes.push(ModelRoute {
                    model: model.to_string(),
                    provider: "Anthropic".to_string(),
                    api_method: "oauth".to_string(),
                    available: false,
                    detail: "no credentials".to_string(),
                });
            }
        }

        // OpenAI models
        for model in ALL_OPENAI_MODELS {
            let (available, detail) = if !self.has_openai_creds {
                (false, "no credentials".to_string())
            } else if let Some(false) = is_model_available_for_account(model) {
                (false, "not available for your plan".to_string())
            } else {
                (true, String::new())
            };
            routes.push(ModelRoute {
                model: model.to_string(),
                provider: "OpenAI".to_string(),
                api_method: "api-key".to_string(),
                available,
                detail,
            });
        }

        // OpenRouter models (with per-provider endpoints)
        if let Some(ref openrouter) = self.openrouter {
            for model in openrouter.available_models_display() {
                let cached = openrouter::load_endpoints_disk_cache_public(&model);
                let age_str = cached.as_ref().map(|(_, age)| {
                    if *age < 3600 {
                        format!("{}m ago", age / 60)
                    } else if *age < 86400 {
                        format!("{}h ago", age / 3600)
                    } else {
                        format!("{}d ago", age / 86400)
                    }
                });
                // Auto route: hint which provider it would likely pick
                let auto_detail = cached
                    .as_ref()
                    .and_then(|(eps, _)| eps.first().map(|ep| format!("→ {}", ep.provider_name)))
                    .unwrap_or_default();
                routes.push(ModelRoute {
                    model: model.clone(),
                    provider: "auto".to_string(),
                    api_method: "openrouter".to_string(),
                    available: self.has_openrouter_creds,
                    detail: auto_detail,
                });
                // Add per-provider routes from endpoints cache
                if let Some((ref endpoints, _)) = cached {
                    let stale_suffix = age_str.as_deref().unwrap_or("");
                    for ep in endpoints {
                        let mut detail = ep.detail_string();
                        if !stale_suffix.is_empty() && !detail.is_empty() {
                            detail = format!("{}, {}", detail, stale_suffix);
                        } else if !stale_suffix.is_empty() {
                            detail = stale_suffix.to_string();
                        }
                        routes.push(ModelRoute {
                            model: model.clone(),
                            provider: ep.provider_name.clone(),
                            api_method: "openrouter".to_string(),
                            available: self.has_openrouter_creds,
                            detail,
                        });
                    }
                }
            }
        } else {
            // OpenRouter not configured - show a few popular models as unavailable
            routes.push(ModelRoute {
                model: "openrouter models".to_string(),
                provider: "—".to_string(),
                api_method: "openrouter".to_string(),
                available: false,
                detail: "OPENROUTER_API_KEY not set".to_string(),
            });
        }

        // Also add Claude/OpenAI models via openrouter as alternative routes
        if self.has_openrouter_creds {
            for model in ALL_CLAUDE_MODELS {
                let or_model = format!("anthropic/{}", model);
                if let Some((endpoints, _)) =
                    openrouter::load_endpoints_disk_cache_public(&or_model)
                {
                    for ep in &endpoints {
                        routes.push(ModelRoute {
                            model: model.to_string(),
                            provider: ep.provider_name.clone(),
                            api_method: "openrouter".to_string(),
                            available: true,
                            detail: ep.detail_string(),
                        });
                    }
                } else {
                    routes.push(ModelRoute {
                        model: model.to_string(),
                        provider: "Anthropic".to_string(),
                        api_method: "openrouter".to_string(),
                        available: true,
                        detail: String::new(),
                    });
                }
            }
        }

        routes
    }

    async fn prefetch_models(&self) -> Result<()> {
        if let Some(ref openrouter) = self.openrouter {
            openrouter.prefetch_models().await?;
        }
        Ok(())
    }

    fn handles_tools_internally(&self) -> bool {
        match self.active_provider() {
            ActiveProvider::Claude => {
                // Direct API does NOT handle tools internally - jcode executes them
                if self.anthropic.is_some() {
                    false
                } else {
                    self.claude
                        .as_ref()
                        .map(|c| c.handles_tools_internally())
                        .unwrap_or(false)
                }
            }
            ActiveProvider::OpenAI => self
                .openai
                .as_ref()
                .map(|o| o.handles_tools_internally())
                .unwrap_or(false),
            ActiveProvider::OpenRouter => false, // jcode executes tools
        }
    }

    fn reasoning_effort(&self) -> Option<String> {
        match self.active_provider() {
            ActiveProvider::Claude => None,
            ActiveProvider::OpenAI => self.openai.as_ref().and_then(|o| o.reasoning_effort()),
            ActiveProvider::OpenRouter => None,
        }
    }

    fn set_reasoning_effort(&self, effort: &str) -> Result<()> {
        match self.active_provider() {
            ActiveProvider::OpenAI => self
                .openai
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("OpenAI provider not available"))?
                .set_reasoning_effort(effort),
            _ => Err(anyhow::anyhow!(
                "Reasoning effort is only supported for OpenAI models"
            )),
        }
    }

    fn available_efforts(&self) -> Vec<&'static str> {
        match self.active_provider() {
            ActiveProvider::OpenAI => self
                .openai
                .as_ref()
                .map(|o| o.available_efforts())
                .unwrap_or_default(),
            _ => vec![],
        }
    }

    fn supports_compaction(&self) -> bool {
        match self.active_provider() {
            ActiveProvider::Claude => {
                // Direct API supports compaction
                if self.anthropic.is_some() {
                    true
                } else {
                    self.claude
                        .as_ref()
                        .map(|c| c.supports_compaction())
                        .unwrap_or(false)
                }
            }
            ActiveProvider::OpenAI => self
                .openai
                .as_ref()
                .map(|o| o.supports_compaction())
                .unwrap_or(false),
            ActiveProvider::OpenRouter => self
                .openrouter
                .as_ref()
                .map(|o| o.supports_compaction())
                .unwrap_or(false),
        }
    }

    fn context_window(&self) -> usize {
        match self.active_provider() {
            ActiveProvider::Claude => {
                if let Some(ref anthropic) = self.anthropic {
                    anthropic.context_window()
                } else if let Some(ref claude) = self.claude {
                    claude.context_window()
                } else {
                    DEFAULT_CONTEXT_LIMIT
                }
            }
            ActiveProvider::OpenAI => self
                .openai
                .as_ref()
                .map(|o| o.context_window())
                .unwrap_or(DEFAULT_CONTEXT_LIMIT),
            ActiveProvider::OpenRouter => self
                .openrouter
                .as_ref()
                .map(|o| o.context_window())
                .unwrap_or(DEFAULT_CONTEXT_LIMIT),
        }
    }

    fn fork(&self) -> Arc<dyn Provider> {
        let current_model = self.model();
        let active = self.active_provider();
        let provider = MultiProvider::new();
        // Set the active provider based on what was active before
        match active {
            ActiveProvider::Claude => {} // Default
            ActiveProvider::OpenAI => {
                if provider.openai.is_some() {
                    *provider.active.write().unwrap() = ActiveProvider::OpenAI;
                }
            }
            ActiveProvider::OpenRouter => {
                if provider.openrouter.is_some() {
                    *provider.active.write().unwrap() = ActiveProvider::OpenRouter;
                }
            }
        }
        let _ = provider.set_model(&current_model);
        Arc::new(provider)
    }

    fn native_result_sender(&self) -> Option<NativeToolResultSender> {
        match self.active_provider() {
            // Direct API doesn't use native result sender
            ActiveProvider::Claude => {
                if self.anthropic.is_some() {
                    None
                } else {
                    self.claude.as_ref().and_then(|c| c.native_result_sender())
                }
            }
            ActiveProvider::OpenAI => None,
            ActiveProvider::OpenRouter => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_for_model_claude() {
        assert_eq!(
            provider_for_model("claude-opus-4-5-20251101"),
            Some("claude")
        );
    }

    #[test]
    fn test_provider_for_model_openai() {
        assert_eq!(provider_for_model("gpt-5.2-codex"), Some("openai"));
    }

    #[test]
    fn test_provider_for_model_openrouter() {
        // OpenRouter uses provider/model format
        assert_eq!(
            provider_for_model("anthropic/claude-sonnet-4"),
            Some("openrouter")
        );
        assert_eq!(provider_for_model("openai/gpt-4o"), Some("openrouter"));
        assert_eq!(
            provider_for_model("google/gemini-2.0-flash"),
            Some("openrouter")
        );
        assert_eq!(
            provider_for_model("meta-llama/llama-3.1-405b"),
            Some("openrouter")
        );
    }

    #[test]
    fn test_provider_for_model_unknown() {
        assert_eq!(provider_for_model("unknown-model"), None);
    }

    #[test]
    fn test_context_limit_spark_vs_codex() {
        assert_eq!(
            context_limit_for_model("gpt-5.3-codex-spark"),
            Some(128_000)
        );
        assert_eq!(context_limit_for_model("gpt-5.3-codex"), Some(272_000));
        assert_eq!(context_limit_for_model("gpt-5.2-codex"), Some(272_000));
        assert_eq!(context_limit_for_model("gpt-5-codex"), Some(272_000));
    }

    #[test]
    fn test_context_limit_claude() {
        assert_eq!(context_limit_for_model("claude-opus-4-6"), Some(1_048_576));
        assert_eq!(context_limit_for_model("claude-sonnet-4-6"), Some(1_048_576));
    }

    #[test]
    fn test_context_limit_dynamic_cache() {
        populate_context_limits(
            [("test-model-xyz".to_string(), 64_000)]
                .into_iter()
                .collect(),
        );
        assert_eq!(context_limit_for_model("test-model-xyz"), Some(64_000));
    }
}
