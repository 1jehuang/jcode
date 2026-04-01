pub mod anthropic;
pub mod antigravity;
pub mod claude;
pub mod cli_common;
pub mod copilot;
pub mod cursor;
mod failover;
pub mod gemini;
pub mod jcode;
pub mod models;
mod selection;
pub mod openai;
pub(crate) mod openai_request;
pub mod openrouter;
pub mod pricing;

use crate::auth;
use crate::message::{ContentBlock, Message, Role, StreamEvent, ToolDefinition};
use anyhow::Result;
use async_trait::async_trait;
use futures::Stream;
#[cfg(test)]
use failover::FailoverDecision;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use std::sync::{Arc, RwLock};

// Re-export native tool result types for use by agent
pub use claude::{NativeToolResult, NativeToolResultSender};
pub(crate) use failover::{ProviderFailoverPrompt, parse_failover_prompt_message};
pub use jcode_provider_core::{
    CHEAPNESS_REFERENCE_INPUT_TOKENS, CHEAPNESS_REFERENCE_OUTPUT_TOKENS, ModelRoute,
    NativeCompactionResult, RouteBillingKind, RouteCheapnessEstimate, RouteCostConfidence,
    RouteCostSource, shared_http_client,
};

/// Stream of events from a provider
pub type EventStream = Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>;

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

    /// List models that should participate in cycle-model switching.
    ///
    /// Defaults to the provider's static switchable set. Providers with dynamic
    /// model catalogs can override this to expose a cached live list without
    /// forcing every caller to know whether the source is static or dynamic.
    fn available_models_for_switching(&self) -> Vec<String> {
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

    /// Return the currently preferred upstream provider (e.g., for OpenRouter routing display).
    fn preferred_provider(&self) -> Option<String> {
        None
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

    /// Called when auth credentials change (e.g., after login).
    /// Providers can use this to hot-add sub-providers.
    fn on_auth_changed(&self) {}

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

    /// Get the active service tier override (if applicable, e.g., OpenAI).
    fn service_tier(&self) -> Option<String> {
        None
    }

    /// Set the active service tier override (if applicable, e.g., OpenAI).
    fn set_service_tier(&self, _service_tier: &str) -> Result<()> {
        Err(anyhow::anyhow!(
            "This provider does not support service tier switching"
        ))
    }

    /// Get ordered list of available service tiers.
    fn available_service_tiers(&self) -> Vec<&'static str> {
        vec![]
    }

    /// Get the native compaction mode for the active provider, if any.
    fn native_compaction_mode(&self) -> Option<String> {
        None
    }

    /// Get the native compaction threshold in tokens for the active provider, if any.
    fn native_compaction_threshold_tokens(&self) -> Option<usize> {
        None
    }

    fn transport(&self) -> Option<String> {
        None
    }

    fn set_transport(&self, _transport: &str) -> Result<()> {
        Err(anyhow::anyhow!(
            "This provider does not support transport switching"
        ))
    }

    fn available_transports(&self) -> Vec<&'static str> {
        vec![]
    }

    /// Returns true if the provider executes tools internally (e.g., Claude Code CLI).
    /// When true, jcode should NOT execute tools locally - just record the tool calls.
    fn handles_tools_internally(&self) -> bool {
        false
    }

    /// Invalidate any cached credentials (e.g., after account switch).
    /// Providers that cache OAuth tokens should clear them.
    async fn invalidate_credentials(&self) {
        // Default: no-op
    }

    /// Set Copilot premium request conservation mode.
    fn set_premium_mode(&self, _mode: copilot::PremiumMode) {
        // Default: no-op (non-Copilot providers ignore this)
    }

    /// Get the current Copilot premium mode.
    fn premium_mode(&self) -> copilot::PremiumMode {
        copilot::PremiumMode::Normal
    }

    /// Returns true if jcode should use its own compaction for this provider.
    fn supports_compaction(&self) -> bool {
        false
    }

    /// Returns true if jcode should proactively run its own summary-based
    /// compaction for this provider during normal operation.
    ///
    /// Providers can override this to prefer a native/server-side compaction
    /// mechanism while still keeping local hard-compaction available as an
    /// emergency recovery path.
    fn uses_jcode_compaction(&self) -> bool {
        self.supports_compaction()
    }

    /// Ask the provider to produce a native compaction artifact for the supplied
    /// messages. Providers that do not support native compaction should return
    /// an error so callers can fall back to jcode's local summary compaction.
    async fn native_compact(
        &self,
        _messages: &[Message],
        _existing_summary_text: Option<&str>,
        _existing_openai_encrypted_content: Option<&str>,
    ) -> Result<NativeCompactionResult> {
        Err(anyhow::anyhow!(
            "This provider does not support native compaction"
        ))
    }

    /// Return the context window size (in tokens) for the current model.
    /// Providers should override this to return accurate, dynamic values.
    /// Falls back to hardcoded lookup if not overridden.
    fn context_window(&self) -> usize {
        context_limit_for_model_with_provider(&self.model(), Some(self.name()))
            .unwrap_or(DEFAULT_CONTEXT_LIMIT)
    }

    /// Create a new provider instance with the same credentials/config and model,
    /// but independent mutable state (e.g., model selection).
    fn fork(&self) -> Arc<dyn Provider>;

    /// Get a sender for native tool results (if the provider supports it).
    /// This is used by the Claude provider to send results back to a bridge (if any).
    fn native_result_sender(&self) -> Option<NativeToolResultSender> {
        None
    }

    /// Drain any startup notices (e.g., account auto-switch messages).
    /// Returns an empty vec by default. MultiProvider overrides this.
    fn drain_startup_notices(&self) -> Vec<String> {
        Vec::new()
    }

    /// Switch the active provider for the current session when supported.
    fn switch_active_provider_to(&self, _provider: &str) -> Result<()> {
        Err(anyhow::anyhow!(
            "This provider does not support active provider switching"
        ))
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
            tool_duration_ms: None,
        }];

        let response = self.complete(&messages, &[], system, None).await?;
        let mut result = String::new();
        tokio::pin!(response);

        while let Some(event) = response.next().await {
            match event {
                Ok(StreamEvent::TextDelta(text)) => result.push_str(&text),
                Ok(_) => {}
                Err(err) => return Err(err),
            }
        }

        Ok(result)
    }
}

/// Available models (shown in /model list)
pub const ALL_CLAUDE_MODELS: &[&str] = &[
    "claude-opus-4-6",
    "claude-opus-4-6[1m]",
    "claude-sonnet-4-6",
    "claude-sonnet-4-6[1m]",
    "claude-haiku-4-5",
    "claude-opus-4-5",
    "claude-sonnet-4-5",
    "claude-sonnet-4-20250514",
];

pub const ALL_OPENAI_MODELS: &[&str] = &[
    "gpt-5.4",
    "gpt-5.4-pro",
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

pub use self::models::{
    AccountModelAvailability, AccountModelAvailabilityState, AnthropicModelCatalog,
    DEFAULT_CONTEXT_LIMIT, ModelCapabilities, OpenAIModelCatalog,
    begin_openai_model_catalog_refresh, clear_all_model_unavailability_for_account,
    clear_all_provider_unavailability_for_account, clear_model_unavailable_for_account,
    clear_provider_unavailable_for_account, context_limit_for_model,
    context_limit_for_model_with_provider, fetch_anthropic_model_catalog,
    fetch_anthropic_model_catalog_oauth, fetch_openai_context_limits, fetch_openai_model_catalog,
    finish_openai_model_catalog_refresh, format_account_model_availability_detail,
    get_best_available_openai_model, is_model_available_for_account, known_anthropic_model_ids,
    known_openai_model_ids, model_availability_for_account,
    model_unavailability_detail_for_account, note_openai_model_catalog_refresh_attempt,
    populate_account_models, populate_anthropic_models, populate_context_limits,
    provider_for_model, provider_for_model_with_hint, provider_unavailability_detail_for_account,
    record_model_unavailable_for_account, record_provider_unavailable_for_account,
    refresh_openai_model_catalog_in_background, resolve_model_capabilities,
    should_refresh_openai_model_catalog,
};
use self::models::{
    ensure_model_allowed_for_subscription, filtered_display_models, filtered_model_routes,
    normalize_copilot_model_name,
};
use self::pricing::{cheapness_for_route, openrouter_pricing_from_model_pricing};

/// MultiProvider wraps multiple providers and allows seamless model switching
pub struct MultiProvider {
    /// Claude Code CLI provider
    claude: Option<claude::ClaudeProvider>,
    /// Direct Anthropic API provider (no Python dependency)
    anthropic: Option<anthropic::AnthropicProvider>,
    openai: Option<openai::OpenAIProvider>,
    /// GitHub Copilot API provider (direct API, hot-swappable after login)
    copilot_api: RwLock<Option<Arc<copilot::CopilotApiProvider>>>,
    /// Gemini provider (hot-swappable after login)
    gemini: RwLock<Option<gemini::GeminiProvider>>,
    /// Cursor provider (native/direct API, hot-swappable after login)
    cursor: RwLock<Option<Arc<cursor::CursorCliProvider>>>,
    /// OpenRouter API provider (200+ models from various providers, hot-swappable after login)
    openrouter: RwLock<Option<Arc<openrouter::OpenRouterProvider>>>,
    active: RwLock<ActiveProvider>,
    has_claude_creds: bool,
    has_openai_creds: bool,
    has_gemini_creds: bool,
    has_cursor_creds: bool,
    /// Use Claude CLI instead of direct API (legacy mode)
    use_claude_cli: bool,
    /// Notifications generated during provider/account auto-selection.
    /// The TUI should drain and display these on session start.
    startup_notices: RwLock<Vec<String>>,
    /// Optional explicit provider lock set by CLI `--provider`.
    /// When present, cross-provider fallback is disabled.
    forced_provider: Option<ActiveProvider>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ActiveProvider {
    Claude,
    OpenAI,
    Copilot,
    Gemini,
    Cursor,
    OpenRouter,
}

#[derive(Clone, Copy)]
enum CompletionMode<'a> {
    Unified {
        system: &'a str,
    },
    Split {
        system_static: &'a str,
        system_dynamic: &'a str,
    },
}

impl CompletionMode<'_> {
    fn log_suffix(self) -> &'static str {
        match self {
            CompletionMode::Unified { .. } => "",
            CompletionMode::Split { .. } => " (split)",
        }
    }

    fn switch_log_prefix(self) -> &'static str {
        match self {
            CompletionMode::Unified { .. } => "Auto-fallback",
            CompletionMode::Split { .. } => "Auto-fallback (split)",
        }
    }
}

impl MultiProvider {
    fn estimate_request_input(
        messages: &[Message],
        tools: &[ToolDefinition],
        mode: CompletionMode<'_>,
    ) -> (usize, usize) {
        let mut chars = serde_json::to_string(messages)
            .map(|value| value.len())
            .unwrap_or(0)
            + serde_json::to_string(tools)
                .map(|value| value.len())
                .unwrap_or(0);
        match mode {
            CompletionMode::Unified { system } => {
                chars += system.len();
            }
            CompletionMode::Split {
                system_static,
                system_dynamic,
            } => {
                chars += system_static.len() + system_dynamic.len();
            }
        }
        let tokens = chars / 4;
        (chars, tokens)
    }

    fn new_with_auth_status(auth_status: auth::AuthStatus) -> Self {
        let has_claude_creds = auth::claude::load_credentials().is_ok();
        let has_openai_creds = auth::codex::load_credentials().is_ok();
        let has_copilot_api = auth_status.copilot_has_api_token;
        // Treat expired Gemini OAuth as configured: GeminiProvider refreshes lazily on first use.
        let has_gemini_creds = auth::gemini::load_tokens().is_ok();
        let has_cursor_creds = matches!(auth_status.cursor, auth::AuthState::Available);
        let has_openrouter_creds = openrouter::OpenRouterProvider::has_credentials();

        // Check if we should use Claude CLI instead of direct API.
        // Set JCODE_USE_CLAUDE_CLI=1 to force the deprecated Claude CLI shell-out path.
        // Default is direct Anthropic API.
        let use_claude_cli = std::env::var("JCODE_USE_CLAUDE_CLI")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        if use_claude_cli {
            crate::logging::warn(
                "JCODE_USE_CLAUDE_CLI is deprecated and will be removed. Direct Anthropic API transport is the default.",
            );
        }

        let claude = if has_claude_creds && use_claude_cli {
            crate::logging::info(
                "Using deprecated Claude CLI provider (forced by JCODE_USE_CLAUDE_CLI=1)",
            );
            Some(claude::ClaudeProvider::new())
        } else {
            None
        };

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

        let copilot_api = if has_copilot_api {
            match copilot::CopilotApiProvider::new() {
                Ok(p) => {
                    crate::logging::info("Copilot API provider initialized (direct API)");
                    let provider = Arc::new(p);
                    let p_clone = provider.clone();
                    tokio::spawn(async move {
                        p_clone.detect_tier_and_set_default().await;
                    });
                    Some(provider)
                }
                Err(e) => {
                    crate::logging::info(&format!("Failed to initialize Copilot API: {}", e));
                    None
                }
            }
        } else {
            None
        };

        let gemini_provider = if has_gemini_creds {
            Some(gemini::GeminiProvider::new())
        } else {
            None
        };

        let cursor_provider = if has_cursor_creds {
            Some(Arc::new(cursor::CursorCliProvider::new()))
        } else {
            None
        };

        let openrouter = if has_openrouter_creds {
            match openrouter::OpenRouterProvider::new() {
                Ok(p) => Some(Arc::new(p)),
                Err(e) => {
                    crate::logging::info(&format!("Failed to initialize OpenRouter: {}", e));
                    None
                }
            }
        } else {
            None
        };

        let copilot_premium_zero = matches!(
            std::env::var("JCODE_COPILOT_PREMIUM").ok().as_deref(),
            Some("0")
        );
        let mut active = Self::auto_default_provider(
            openai.is_some(),
            claude.is_some() || anthropic.is_some(),
            copilot_api.is_some(),
            gemini_provider.is_some(),
            cursor_provider.is_some(),
            openrouter.is_some(),
            copilot_premium_zero,
        );

        if copilot_premium_zero && matches!(active, ActiveProvider::Copilot) {
            crate::logging::info(
                "Copilot premium mode is Zero (free requests) - defaulting to Copilot provider",
            );
        }

        let forced_provider = Self::forced_provider_from_env();
        let cfg = crate::config::config();
        if let Some(forced) = forced_provider {
            active = forced;
            let is_configured = match forced {
                ActiveProvider::Claude => claude.is_some() || anthropic.is_some(),
                ActiveProvider::OpenAI => openai.is_some(),
                ActiveProvider::Copilot => copilot_api.is_some(),
                ActiveProvider::Gemini => gemini_provider.is_some(),
                ActiveProvider::Cursor => cursor_provider.is_some(),
                ActiveProvider::OpenRouter => openrouter.is_some(),
            };
            if is_configured {
                crate::logging::info(&format!(
                    "Using forced provider '{}' from CLI/environment",
                    Self::provider_key(forced)
                ));
            } else {
                crate::logging::warn(&format!(
                    "Forced provider '{}' is not configured; requests will fail until credentials are available",
                    Self::provider_key(forced)
                ));
            }
        } else if let Some(ref pref) = cfg.provider.default_provider {
            if let Some(pref_provider) = Self::parse_provider_hint(pref) {
                let is_configured = match pref_provider {
                    ActiveProvider::Claude => claude.is_some() || anthropic.is_some(),
                    ActiveProvider::OpenAI => openai.is_some(),
                    ActiveProvider::Copilot => copilot_api.is_some(),
                    ActiveProvider::Gemini => gemini_provider.is_some(),
                    ActiveProvider::Cursor => cursor_provider.is_some(),
                    ActiveProvider::OpenRouter => openrouter.is_some(),
                };
                if is_configured {
                    active = pref_provider;
                    crate::logging::info(&format!(
                        "Using preferred provider '{}' from config",
                        pref
                    ));
                } else {
                    crate::logging::warn(&format!(
                        "Preferred provider '{}' is not configured, using auto-detected default",
                        pref
                    ));
                }
            } else {
                crate::logging::warn(&format!(
                    "Unknown default_provider '{}' in config (expected: claude|openai|copilot|gemini|cursor|openrouter)",
                    pref
                ));
            }
        }

        let result = Self {
            claude,
            anthropic,
            openai,
            copilot_api: RwLock::new(copilot_api),
            gemini: RwLock::new(gemini_provider),
            cursor: RwLock::new(cursor_provider),
            openrouter: RwLock::new(openrouter),
            active: RwLock::new(active),
            has_claude_creds,
            has_openai_creds,
            has_gemini_creds,
            has_cursor_creds,
            use_claude_cli,
            startup_notices: RwLock::new(Vec::new()),
            forced_provider,
        };

        if let Some(ref model) = cfg.provider.default_model {
            if let Err(e) = result.set_model(model) {
                crate::logging::warn(&format!(
                    "Failed to apply default_model '{}' from config: {}",
                    model, e
                ));
            } else {
                crate::logging::info(&format!("Applied default model '{}' from config", model));
            }
        }

        result.spawn_openai_catalog_refresh_if_needed();
        result.auto_select_active_multi_account();
        result
    }

    fn provider_is_configured(&self, provider: ActiveProvider) -> bool {
        match provider {
            ActiveProvider::Claude => self.anthropic.is_some() || self.claude.is_some(),
            ActiveProvider::OpenAI => self.openai.is_some(),
            ActiveProvider::Copilot => self.copilot_api.read().unwrap().is_some(),
            ActiveProvider::Gemini => self.gemini.read().unwrap().is_some(),
            ActiveProvider::Cursor => self.cursor.read().unwrap().is_some(),
            ActiveProvider::OpenRouter => self.openrouter.read().unwrap().is_some(),
        }
    }

    fn provider_precheck_unavailable_reason(&self, provider: ActiveProvider) -> Option<String> {
        match provider {
            ActiveProvider::Claude if self.is_claude_usage_exhausted() => {
                Some(Self::usage_exhausted_reason(provider))
            }
            ActiveProvider::OpenAI if self.is_openai_usage_exhausted() => {
                Some(Self::usage_exhausted_reason(provider))
            }
            _ => None,
        }
    }

    fn multi_account_provider_kind(
        provider: ActiveProvider,
    ) -> Option<crate::usage::MultiAccountProviderKind> {
        match provider {
            ActiveProvider::Claude => Some(crate::usage::MultiAccountProviderKind::Anthropic),
            ActiveProvider::OpenAI => Some(crate::usage::MultiAccountProviderKind::OpenAI),
            _ => None,
        }
    }

    fn account_usage_probe(provider: ActiveProvider) -> Option<crate::usage::AccountUsageProbe> {
        let kind = Self::multi_account_provider_kind(provider)?;
        crate::usage::account_usage_probe_sync(kind)
    }

    fn account_switch_guidance(provider: ActiveProvider) -> Option<String> {
        let probe = Self::account_usage_probe(provider)?;
        probe.switch_guidance().or_else(|| {
            (probe.current_exhausted() && probe.all_accounts_exhausted()).then(|| {
                format!(
                    "All {} accounts appear exhausted. Use `/usage` to inspect reset times.",
                    probe.provider.display_name()
                )
            })
        })
    }

    fn usage_exhausted_reason(provider: ActiveProvider) -> String {
        let mut reason = "OAuth usage exhausted".to_string();
        if let Some(guidance) = Self::account_switch_guidance(provider) {
            reason.push_str(". ");
            reason.push_str(&guidance);
        }
        reason
    }

    fn error_looks_like_usage_limit(summary: &str) -> bool {
        let lower = summary.to_ascii_lowercase();
        [
            "quota",
            "insufficient_quota",
            "rate limit",
            "rate_limit",
            "rate_limit_exceeded",
            "too many requests",
            "billing",
            "credit",
            "payment required",
            "usage exhausted",
            "limit reached",
            "429",
            "402",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
    }

    fn maybe_annotate_limit_summary(provider: ActiveProvider, summary: String) -> String {
        if !Self::error_looks_like_usage_limit(&summary) {
            return summary;
        }
        let Some(guidance) = Self::account_switch_guidance(provider) else {
            return summary;
        };
        if summary.contains(&guidance) {
            return summary;
        }
        format!("{}. {}", summary, guidance)
    }

    fn additional_no_provider_guidance(&self) -> Vec<String> {
        [ActiveProvider::Claude, ActiveProvider::OpenAI]
            .into_iter()
            .filter_map(Self::account_switch_guidance)
            .collect()
    }

    fn no_provider_available_error(&self, notes: &[String]) -> anyhow::Error {
        let mut msg = "No tokens/providers left: no usable provider right now. Anthropic/OpenAI usage may be exhausted and GitHub Copilot is not authenticated or currently unavailable.".to_string();
        if !notes.is_empty() {
            msg.push_str(" Details: ");
            msg.push_str(&notes.join(" | "));
        }
        let extra_guidance = self.additional_no_provider_guidance();
        if !extra_guidance.is_empty() {
            msg.push(' ');
            msg.push_str(&extra_guidance.join(" "));
        }
        msg.push_str(" Use `/usage` to check limits and `/login <provider>` to re-authenticate.");
        anyhow::anyhow!(msg)
    }

    async fn complete_with_failover(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        mode: CompletionMode<'_>,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        self.spawn_openai_catalog_refresh_if_needed();

        let active = self.active_provider();
        let sequence = Self::fallback_sequence_for(active, self.forced_provider);
        let mut notes: Vec<String> = Vec::new();
        let mut failover_reason: Option<String> = None;
        let (estimated_input_chars, estimated_input_tokens) =
            Self::estimate_request_input(messages, tools, mode);

        for candidate in sequence {
            let label = Self::provider_label(candidate);
            let key = Self::provider_key(candidate);

            if candidate != active && failover_reason.is_some() {
                let prompt = self.build_failover_prompt(
                    active,
                    candidate,
                    failover_reason
                        .clone()
                        .unwrap_or_else(|| "provider unavailable".to_string()),
                    estimated_input_chars,
                    estimated_input_tokens,
                );
                return Err(anyhow::anyhow!(prompt.to_error_message()));
            }

            if !self.provider_is_configured(candidate) {
                let note = format!("{}: not configured", label);
                if candidate == active {
                    crate::logging::warn(&format!(
                        "Failover{}: skipping active provider {} (not configured)",
                        mode.log_suffix(),
                        label
                    ));
                }
                notes.push(note);
                continue;
            }

            if let Some(detail) = provider_unavailability_detail_for_account(key) {
                let note = format!("{}: {}", label, detail);
                if candidate == active {
                    crate::logging::warn(&format!(
                        "Failover{}: skipping active provider {} - {}",
                        mode.log_suffix(),
                        label,
                        detail
                    ));
                    failover_reason = Some(detail.clone());
                }
                notes.push(note);
                continue;
            }

            if let Some(reason) = self.provider_precheck_unavailable_reason(candidate) {
                let note = format!("{}: {}", label, reason);
                if candidate == active {
                    crate::logging::warn(&format!(
                        "Failover{}: skipping active provider {} - {}",
                        mode.log_suffix(),
                        label,
                        reason
                    ));
                    failover_reason = Some(reason.clone());
                }
                notes.push(note);
                record_provider_unavailable_for_account(key, &reason);
                continue;
            }

            let attempt = match mode {
                CompletionMode::Unified { system } => {
                    self.complete_on_provider(candidate, messages, tools, system, resume_session_id)
                        .await
                }
                CompletionMode::Split {
                    system_static,
                    system_dynamic,
                } => {
                    self.complete_split_on_provider(
                        candidate,
                        messages,
                        tools,
                        system_static,
                        system_dynamic,
                        resume_session_id,
                    )
                    .await
                }
            };

            match attempt {
                Ok(stream) => {
                    clear_provider_unavailable_for_account(key);
                    if candidate != active {
                        self.set_active_provider(candidate);
                        let from_label = Self::provider_label(active);
                        let to_label = Self::provider_label(candidate);
                        crate::logging::info(&format!(
                            "{}: switched from {} to {}",
                            mode.switch_log_prefix(),
                            from_label,
                            to_label
                        ));
                        self.startup_notices.write().unwrap().push(format!(
                            "⚡ Auto-fallback: {} unavailable, switched to {}",
                            from_label, to_label
                        ));
                    }
                    return Ok(stream);
                }
                Err(err) => {
                    let summary =
                        Self::maybe_annotate_limit_summary(candidate, Self::summarize_error(&err));
                    let decision = Self::classify_failover_error(&err);
                    crate::logging::info(&format!(
                        "Provider {} failed{}: {} (failover={} decision={})",
                        label,
                        mode.log_suffix(),
                        summary,
                        decision.should_failover(),
                        decision.as_str()
                    ));
                    notes.push(format!("{}: {}", label, summary));
                    if decision.should_failover() {
                        if decision.should_mark_provider_unavailable() {
                            record_provider_unavailable_for_account(key, &summary);
                        }
                        if candidate == active {
                            failover_reason = Some(summary);
                        }
                    } else {
                        return Err(err);
                    }
                }
            }
        }

        Err(self.no_provider_available_error(&notes))
    }

    async fn complete_on_provider(
        &self,
        provider: ActiveProvider,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        match provider {
            ActiveProvider::Claude => {
                // Prefer direct Anthropic API if available.
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
                    Err(anyhow::anyhow!(
                        "OpenAI credentials not available. Run `jcode login --provider openai` to log in."
                    ))
                }
            }
            ActiveProvider::Copilot => {
                let copilot = self.copilot_api.read().unwrap().clone();
                if let Some(copilot) = copilot {
                    copilot
                        .complete(messages, tools, system, resume_session_id)
                        .await
                } else {
                    Err(anyhow::anyhow!(
                        "GitHub Copilot is not available. Run `jcode login --provider copilot`."
                    ))
                }
            }
            ActiveProvider::Gemini => {
                let gemini = self.gemini.read().unwrap().clone();
                if let Some(gemini) = gemini {
                    gemini
                        .complete(messages, tools, system, resume_session_id)
                        .await
                } else {
                    Err(anyhow::anyhow!(
                        "Gemini is not available. Run `jcode login --provider gemini`."
                    ))
                }
            }
            ActiveProvider::Cursor => {
                let cursor = self.cursor.read().unwrap().clone();
                if let Some(cursor) = cursor {
                    cursor
                        .complete(messages, tools, system, resume_session_id)
                        .await
                } else {
                    Err(anyhow::anyhow!(
                        "Cursor is not available. Run `jcode login --provider cursor`."
                    ))
                }
            }
            ActiveProvider::OpenRouter => {
                let openrouter = self.openrouter.read().unwrap().clone();
                if let Some(openrouter) = openrouter {
                    openrouter
                        .complete(messages, tools, system, resume_session_id)
                        .await
                } else {
                    Err(anyhow::anyhow!(
                        "OpenRouter credentials not available. Set OPENROUTER_API_KEY environment variable."
                    ))
                }
            }
        }
    }

    async fn complete_split_on_provider(
        &self,
        provider: ActiveProvider,
        messages: &[Message],
        tools: &[ToolDefinition],
        system_static: &str,
        system_dynamic: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        match provider {
            ActiveProvider::Claude => {
                // Prefer direct Anthropic API for best caching support.
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
                    Err(anyhow::anyhow!(
                        "OpenAI credentials not available. Run `jcode login --provider openai` to log in."
                    ))
                }
            }
            ActiveProvider::Copilot => {
                let copilot = self.copilot_api.read().unwrap().clone();
                if let Some(copilot) = copilot {
                    copilot
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
                        "GitHub Copilot is not available. Run `jcode login --provider copilot`."
                    ))
                }
            }
            ActiveProvider::Gemini => {
                let gemini = self.gemini.read().unwrap().clone();
                if let Some(gemini) = gemini {
                    gemini
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
                        "Gemini is not available. Run `jcode login --provider gemini`."
                    ))
                }
            }
            ActiveProvider::Cursor => {
                let cursor = self.cursor.read().unwrap().clone();
                if let Some(cursor) = cursor {
                    cursor
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
                        "Cursor is not available. Run `jcode login --provider cursor`."
                    ))
                }
            }
            ActiveProvider::OpenRouter => {
                let openrouter = self.openrouter.read().unwrap().clone();
                if let Some(openrouter) = openrouter {
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
                    Err(anyhow::anyhow!(
                        "OpenRouter credentials not available. Set OPENROUTER_API_KEY environment variable."
                    ))
                }
            }
        }
    }

    fn spawn_openai_catalog_refresh_if_needed(&self) {
        if !self.has_openai_creds {
            return;
        }
        if !begin_openai_model_catalog_refresh() {
            return;
        }

        let creds = auth::codex::load_credentials();
        let token = creds
            .as_ref()
            .ok()
            .map(|c| c.access_token.clone())
            .unwrap_or_default();
        refresh_openai_model_catalog_in_background(token, "multi-provider");
    }

    /// Create a new MultiProvider, detecting available credentials
    pub fn new() -> Self {
        Self::new_with_auth_status(auth::AuthStatus::check())
    }

    /// Create a startup-optimized MultiProvider that avoids expensive auth probes.
    pub fn new_fast() -> Self {
        Self::new_with_auth_status(auth::AuthStatus::check_fast())
    }

    /// Create with explicit initial provider preference
    pub fn with_preference(prefer_openai: bool) -> Self {
        let provider = Self::new();
        if provider.forced_provider.is_none() && prefer_openai && provider.openai.is_some() {
            *provider.active.write().unwrap() = ActiveProvider::OpenAI;
        }
        provider
    }

    pub fn with_preference_fast(prefer_openai: bool) -> Self {
        let provider = Self::new_fast();
        if provider.forced_provider.is_none() && prefer_openai && provider.openai.is_some() {
            *provider.active.write().unwrap() = ActiveProvider::OpenAI;
        }
        provider
    }

    fn active_provider(&self) -> ActiveProvider {
        *self.active.read().unwrap()
    }

    pub fn auto_select_active_multi_account(&self) {
        self.auto_select_multi_account_for_provider(self.active_provider());
    }

    /// Backward-compatible wrapper for the Anthropic-specific startup rotation entrypoint.
    pub fn auto_select_anthropic_account(&self) {
        self.auto_select_multi_account_for_provider(ActiveProvider::Claude);
    }

    pub fn auto_select_openai_account(&self) {
        self.auto_select_multi_account_for_provider(ActiveProvider::OpenAI);
    }

    fn auto_select_multi_account_for_provider(&self, provider: ActiveProvider) {
        if self.active_provider() != provider {
            return;
        }
        if !self.provider_is_configured(provider) {
            return;
        }

        let Some(probe) = Self::account_usage_probe(provider) else {
            return;
        };
        if !probe.has_multiple_accounts() || !probe.current_exhausted() {
            return;
        }

        let provider_name = probe.provider.display_name();
        if let Some(alternative) = probe.best_available_alternative() {
            crate::logging::info(&format!(
                "{} account '{}' is exhausted, switching to '{}' ({})",
                provider_name,
                probe.current_label,
                alternative.label,
                alternative.summary()
            ));

            match provider {
                ActiveProvider::Claude => {
                    crate::auth::claude::set_active_account_override(Some(
                        alternative.label.clone(),
                    ));
                    clear_all_provider_unavailability_for_account();
                    clear_all_model_unavailability_for_account();
                    if let Some(ref anthropic) = self.anthropic {
                        let _ = tokio::task::block_in_place(|| {
                            tokio::runtime::Handle::current()
                                .block_on(anthropic.invalidate_credentials())
                        });
                    }
                }
                ActiveProvider::OpenAI => {
                    crate::auth::codex::set_active_account_override(Some(
                        alternative.label.clone(),
                    ));
                    clear_all_provider_unavailability_for_account();
                    clear_all_model_unavailability_for_account();
                    if let Some(ref openai) = self.openai {
                        let _ = tokio::task::block_in_place(|| {
                            tokio::runtime::Handle::current()
                                .block_on(openai.invalidate_credentials())
                        });
                    }
                }
                _ => return,
            }

            let notice = format!(
                "⚡ Auto-switched {} account: **{}** -> **{}** (previous account exhausted)",
                provider_name, probe.current_label, alternative.label
            );
            self.startup_notices.write().unwrap().push(notice);
            return;
        }

        if probe.all_accounts_exhausted() {
            crate::logging::info(&format!("All {} accounts are exhausted", provider_name));
            let notice = format!(
                "⚠ All {} accounts exhausted - will fall back to other providers if available",
                provider_name
            );
            self.startup_notices.write().unwrap().push(notice);
        }
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

    fn is_openai_usage_exhausted_from_usage(usage: &crate::usage::OpenAIUsageData) -> bool {
        if !usage.has_limits() {
            return false;
        }

        let five_hour_exhausted = usage
            .five_hour
            .as_ref()
            .map(|w| w.usage_ratio >= 0.99)
            .unwrap_or(false);
        let seven_day_exhausted = usage
            .seven_day
            .as_ref()
            .map(|w| w.usage_ratio >= 0.99)
            .unwrap_or(false);

        five_hour_exhausted && seven_day_exhausted
    }

    fn is_openai_usage_exhausted(&self) -> bool {
        if self.openai.is_none() {
            return false;
        }

        let usage = crate::usage::get_openai_usage_sync();
        Self::is_openai_usage_exhausted_from_usage(&usage)
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
        self.complete_with_failover(
            messages,
            tools,
            CompletionMode::Unified { system },
            resume_session_id,
        )
        .await
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
        self.complete_with_failover(
            messages,
            tools,
            CompletionMode::Split {
                system_static,
                system_dynamic,
            },
            resume_session_id,
        )
        .await
    }

    fn name(&self) -> &str {
        match self.active_provider() {
            ActiveProvider::Claude => "Claude",
            ActiveProvider::OpenAI => "OpenAI",
            ActiveProvider::Copilot => "Copilot",
            ActiveProvider::Gemini => "Gemini",
            ActiveProvider::Cursor => "Cursor",
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
                .unwrap_or_else(|| "gpt-5.4".to_string()),
            ActiveProvider::Copilot => self
                .copilot_api
                .read()
                .unwrap()
                .as_ref()
                .map(|o| o.model())
                .unwrap_or_else(|| "claude-sonnet-4".to_string()),
            ActiveProvider::Gemini => self
                .gemini
                .read()
                .unwrap()
                .as_ref()
                .map(|o| o.model())
                .unwrap_or_else(|| "gemini-2.5-pro".to_string()),
            ActiveProvider::Cursor => self
                .cursor
                .read()
                .unwrap()
                .as_ref()
                .map(|o| o.model())
                .unwrap_or_else(|| "composer-1.5".to_string()),
            ActiveProvider::OpenRouter => self
                .openrouter
                .read()
                .unwrap()
                .as_ref()
                .map(|o| o.model())
                .unwrap_or_else(|| "anthropic/claude-sonnet-4".to_string()),
        }
    }

    fn set_model(&self, model: &str) -> Result<()> {
        self.spawn_openai_catalog_refresh_if_needed();

        ensure_model_allowed_for_subscription(model)?;

        // Handle explicit "copilot:" prefix from model picker
        if let Some(copilot_model) = model.strip_prefix("copilot:") {
            if let Some(forced) = self.forced_provider {
                if forced != ActiveProvider::Copilot {
                    let copilot_guard = self.copilot_api.read().unwrap();
                    if copilot_guard.is_none() {
                        anyhow::bail!(
                            "Model '{}' requires GitHub Copilot but Copilot credentials are not configured. Run `jcode login --provider copilot` first.",
                            copilot_model
                        );
                    }
                    drop(copilot_guard);
                    crate::logging::info(&format!(
                        "Switching from {} to GitHub Copilot for model '{}'",
                        Self::provider_label(forced),
                        copilot_model,
                    ));
                }
            }
            let copilot_guard = self.copilot_api.read().unwrap();
            if copilot_guard.is_some() {
                *self.active.write().unwrap() = ActiveProvider::Copilot;
                if let Some(ref copilot) = *copilot_guard {
                    copilot.set_model(copilot_model)?;
                }
                return Ok(());
            }
            drop(copilot_guard);
            if self.forced_provider == Some(ActiveProvider::Copilot) {
                return Err(anyhow::anyhow!(
                    "GitHub Copilot is locked by --provider but is not configured. Run `jcode login --provider copilot`."
                ));
            }
            // Copilot not available - fall through with the bare model name
            // so we can try routing it to another provider (e.g. claude-opus-4.6
            // can be served by Anthropic as claude-opus-4-6).
            crate::logging::info(&format!(
                "Copilot not available for '{}', trying other providers",
                copilot_model
            ));
            return self.set_model(copilot_model);
        }

        // Handle explicit "cursor:" prefix from model picker/default config
        if let Some(cursor_model) = model.strip_prefix("cursor:") {
            if let Some(forced) = self.forced_provider {
                if forced != ActiveProvider::Cursor {
                    let cursor_guard = self.cursor.read().unwrap();
                    if cursor_guard.is_none() {
                        anyhow::bail!(
                            "Model '{}' requires Cursor but Cursor credentials are not configured. Run `jcode login --provider cursor` first.",
                            cursor_model
                        );
                    }
                    drop(cursor_guard);
                    crate::logging::info(&format!(
                        "Switching from {} to Cursor for model '{}'",
                        Self::provider_label(forced),
                        cursor_model,
                    ));
                }
            }
            let cursor_guard = self.cursor.read().unwrap();
            if cursor_guard.is_some() {
                *self.active.write().unwrap() = ActiveProvider::Cursor;
                if let Some(ref cursor) = *cursor_guard {
                    cursor.set_model(cursor_model)?;
                }
                return Ok(());
            }
            drop(cursor_guard);
            if self.forced_provider == Some(ActiveProvider::Cursor) {
                return Err(anyhow::anyhow!(
                    "Cursor is locked by --provider but is not configured. Run `jcode login --provider cursor`."
                ));
            }
            crate::logging::info(&format!(
                "Cursor not available for '{}', trying other providers",
                cursor_model
            ));
            return self.set_model(cursor_model);
        }

        // Normalize Copilot-style model names (dots -> hyphens) to canonical form.
        // e.g. "claude-opus-4.6" -> "claude-opus-4-6" so Anthropic/OpenAI accept it.
        let model = if let Some(canonical) = normalize_copilot_model_name(model) {
            canonical
        } else {
            model
        };

        // Detect which provider this model belongs to
        let target_provider = provider_for_model(model);
        if let Some(forced) = self.forced_provider {
            if let Some(target) = target_provider {
                let target_active = match target {
                    "claude" => ActiveProvider::Claude,
                    "openai" => ActiveProvider::OpenAI,
                    "gemini" => ActiveProvider::Gemini,
                    "cursor" => ActiveProvider::Cursor,
                    "openrouter" => ActiveProvider::OpenRouter,
                    _ => forced,
                };
                if target_active != forced {
                    let has_target_creds = match target_active {
                        ActiveProvider::Claude => self.claude.is_some() || self.anthropic.is_some(),
                        ActiveProvider::OpenAI => self.openai.is_some(),
                        ActiveProvider::Gemini => self.gemini.read().unwrap().is_some(),
                        ActiveProvider::Cursor => self.cursor.read().unwrap().is_some(),
                        ActiveProvider::OpenRouter => self.openrouter.read().unwrap().is_some(),
                        ActiveProvider::Copilot => self.copilot_api.read().unwrap().is_some(),
                    };
                    if !has_target_creds {
                        anyhow::bail!(
                            "Model '{}' belongs to {} but {} credentials are not configured. Run `jcode login --provider {}` first.",
                            model,
                            Self::provider_label(target_active),
                            Self::provider_label(target_active),
                            Self::provider_key(target_active),
                        );
                    }
                    crate::logging::info(&format!(
                        "Switching from {} to {} for model '{}'",
                        Self::provider_label(forced),
                        Self::provider_label(target_active),
                        model,
                    ));
                }
            }
        }

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
        } else if target_provider == Some("gemini") {
            let gemini_guard = self.gemini.read().unwrap();
            if gemini_guard.is_none() {
                return Err(anyhow::anyhow!(
                    "Gemini credentials not available. Run `jcode login --provider gemini` first."
                ));
            }
            *self.active.write().unwrap() = ActiveProvider::Gemini;
            if let Some(ref gemini) = *gemini_guard {
                gemini.set_model(model)
            } else {
                Ok(())
            }
        } else if target_provider == Some("cursor") {
            let cursor_guard = self.cursor.read().unwrap();
            if cursor_guard.is_none() {
                return Err(anyhow::anyhow!(
                    "Cursor credentials not available. Run `jcode login --provider cursor` first."
                ));
            }
            *self.active.write().unwrap() = ActiveProvider::Cursor;
            if let Some(ref cursor) = *cursor_guard {
                cursor.set_model(model)
            } else {
                Ok(())
            }
        } else if target_provider == Some("openrouter") {
            let openrouter = self.openrouter.read().unwrap().clone();
            if openrouter.is_none() {
                return Err(anyhow::anyhow!(
                    "OpenRouter credentials not available. Set OPENROUTER_API_KEY environment variable."
                ));
            }
            // Switch active provider to OpenRouter
            *self.active.write().unwrap() = ActiveProvider::OpenRouter;
            if let Some(openrouter) = openrouter {
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
                ActiveProvider::Copilot => {
                    let copilot_guard = self.copilot_api.read().unwrap();
                    if let Some(ref copilot) = *copilot_guard {
                        copilot.set_model(model)
                    } else {
                        Err(anyhow::anyhow!("Unknown model: {}", model))
                    }
                }
                ActiveProvider::Gemini => {
                    let gemini_guard = self.gemini.read().unwrap();
                    if let Some(ref gemini) = *gemini_guard {
                        gemini.set_model(model)
                    } else {
                        Err(anyhow::anyhow!("Unknown model: {}", model))
                    }
                }
                ActiveProvider::Cursor => {
                    let cursor_guard = self.cursor.read().unwrap();
                    if let Some(ref cursor) = *cursor_guard {
                        cursor.set_model(model)
                    } else {
                        Err(anyhow::anyhow!("Unknown model: {}", model))
                    }
                }
                ActiveProvider::OpenRouter => {
                    let openrouter = self.openrouter.read().unwrap().clone();
                    if let Some(openrouter) = openrouter {
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

    fn available_models_for_switching(&self) -> Vec<String> {
        match self.active_provider() {
            ActiveProvider::Claude => {
                if let Some(ref anthropic) = self.anthropic {
                    anthropic.available_models_for_switching()
                } else if let Some(ref claude) = self.claude {
                    claude.available_models_for_switching()
                } else {
                    Vec::new()
                }
            }
            ActiveProvider::OpenAI => self
                .openai
                .as_ref()
                .map(|openai| openai.available_models_for_switching())
                .unwrap_or_default(),
            ActiveProvider::Copilot => self
                .copilot_api
                .read()
                .unwrap()
                .as_ref()
                .map(|copilot| copilot.available_models_for_switching())
                .unwrap_or_default(),
            ActiveProvider::Gemini => self
                .gemini
                .read()
                .unwrap()
                .as_ref()
                .map(|gemini| gemini.available_models_for_switching())
                .unwrap_or_default(),
            ActiveProvider::Cursor => self
                .cursor
                .read()
                .unwrap()
                .as_ref()
                .map(|cursor| cursor.available_models_for_switching())
                .unwrap_or_default(),
            ActiveProvider::OpenRouter => self
                .openrouter
                .read()
                .unwrap()
                .as_ref()
                .map(|openrouter| openrouter.available_models_for_switching())
                .unwrap_or_default(),
        }
    }

    fn available_models_display(&self) -> Vec<String> {
        let mut models = Vec::new();
        models.extend(known_anthropic_model_ids());
        models.extend(known_openai_model_ids());
        {
            let copilot_guard = self.copilot_api.read().unwrap();
            if let Some(ref copilot) = *copilot_guard {
                for m in copilot.available_models_display() {
                    if !models.contains(&m) {
                        models.push(m);
                    }
                }
            }
        }
        {
            let gemini_guard = self.gemini.read().unwrap();
            if let Some(ref gemini) = *gemini_guard {
                for m in gemini.available_models_display() {
                    if !models.contains(&m) {
                        models.push(m);
                    }
                }
            }
        }
        {
            let cursor_guard = self.cursor.read().unwrap();
            if let Some(ref cursor) = *cursor_guard {
                for m in cursor.available_models_display() {
                    if !models.contains(&m) {
                        models.push(m);
                    }
                }
            }
        }
        if let Some(openrouter) = self.openrouter.read().unwrap().clone() {
            models.extend(openrouter.available_models_display());
        }
        filtered_display_models(models)
    }

    fn available_providers_for_model(&self, model: &str) -> Vec<String> {
        if model.contains('/') {
            if let Some(openrouter) = self.openrouter.read().unwrap().clone() {
                return openrouter.available_providers_for_model(model);
            }
        }
        Vec::new()
    }

    fn provider_details_for_model(&self, model: &str) -> Vec<(String, String)> {
        if model.contains('/') {
            if let Some(openrouter) = self.openrouter.read().unwrap().clone() {
                return openrouter.provider_details_for_model(model);
            }
        }
        Vec::new()
    }

    fn preferred_provider(&self) -> Option<String> {
        if let Some(openrouter) = self.openrouter.read().unwrap().clone() {
            if matches!(*self.active.read().unwrap(), ActiveProvider::OpenRouter) {
                return openrouter.preferred_provider();
            }
        }
        None
    }

    fn model_routes(&self) -> Vec<ModelRoute> {
        self.spawn_openai_catalog_refresh_if_needed();

        let mut routes = Vec::new();
        let has_oauth = self.has_claude_creds && !self.use_claude_cli;
        let has_api_key = std::env::var("ANTHROPIC_API_KEY").is_ok();

        // Anthropic models (oauth and/or api-key)
        let is_max = crate::auth::claude::is_max_subscription();
        for model in known_anthropic_model_ids() {
            let is_1m = model.ends_with("[1m]");
            let is_opus = model.contains("opus");

            let model_defaults_1m = crate::provider::anthropic::effectively_1m(&model);
            let (available, detail) =
                if is_1m && !model_defaults_1m && !crate::usage::has_extra_usage() {
                    (false, "requires extra usage".to_string())
                } else if is_opus && !is_max && has_oauth && !has_api_key {
                    (false, "requires Max subscription".to_string())
                } else {
                    (true, String::new())
                };

            if has_oauth {
                routes.push(ModelRoute {
                    model: model.to_string(),
                    provider: "Anthropic".to_string(),
                    api_method: "claude-oauth".to_string(),
                    available,
                    detail: detail.clone(),
                    cheapness: cheapness_for_route(&model, "Anthropic", "claude-oauth"),
                });
            }
            if has_api_key {
                // API key = pay-per-token, no subscription tier restriction on Opus
                // but 1M context still requires extra usage
                let (ak_available, ak_detail) = if is_1m && !crate::usage::has_extra_usage() {
                    (false, "requires extra usage".to_string())
                } else {
                    (true, String::new())
                };
                routes.push(ModelRoute {
                    model: model.to_string(),
                    provider: "Anthropic".to_string(),
                    api_method: "api-key".to_string(),
                    available: ak_available,
                    detail: ak_detail,
                    cheapness: cheapness_for_route(&model, "Anthropic", "api-key"),
                });
            }
            if !has_oauth && !has_api_key {
                routes.push(ModelRoute {
                    model: model.to_string(),
                    provider: "Anthropic".to_string(),
                    api_method: "claude-oauth".to_string(),
                    available: false,
                    detail: "no credentials".to_string(),
                    cheapness: cheapness_for_route(&model, "Anthropic", "claude-oauth"),
                });
            }
        }

        // OpenAI models
        for model in known_openai_model_ids() {
            let availability = model_availability_for_account(&model);
            let (available, detail) = if !self.has_openai_creds {
                (false, "no credentials".to_string())
            } else {
                match availability.state {
                    AccountModelAvailabilityState::Available => (true, String::new()),
                    AccountModelAvailabilityState::Unavailable => (
                        false,
                        format_account_model_availability_detail(&availability)
                            .unwrap_or_else(|| "not available".to_string()),
                    ),
                    AccountModelAvailabilityState::Unknown => {
                        let detail = format_account_model_availability_detail(&availability)
                            .unwrap_or_else(|| "availability unknown".to_string());
                        (true, detail)
                    }
                }
            };
            let cheapness = cheapness_for_route(&model, "OpenAI", "openai-oauth");
            routes.push(ModelRoute {
                model,
                provider: "OpenAI".to_string(),
                api_method: "openai-oauth".to_string(),
                available,
                detail,
                cheapness,
            });
        }

        // GitHub Copilot models
        {
            let copilot_guard = self.copilot_api.read().unwrap();
            if let Some(ref copilot) = *copilot_guard {
                let copilot_models = copilot.available_models_display();
                for model in copilot_models {
                    let cheapness = cheapness_for_route(&model, "Copilot", "copilot");
                    routes.push(ModelRoute {
                        model,
                        provider: "Copilot".to_string(),
                        api_method: "copilot".to_string(),
                        available: true,
                        detail: String::new(),
                        cheapness,
                    });
                }
            } else if copilot::CopilotApiProvider::has_credentials() {
                routes.push(ModelRoute {
                    model: "copilot models".to_string(),
                    provider: "Copilot".to_string(),
                    api_method: "copilot".to_string(),
                    available: false,
                    detail: "not initialized yet".to_string(),
                    cheapness: cheapness_for_route("claude-sonnet-4-6", "Copilot", "copilot"),
                });
            }
        }

        // Gemini models
        {
            let gemini_guard = self.gemini.read().unwrap();
            if let Some(ref gemini) = *gemini_guard {
                for model in gemini.available_models_display() {
                    routes.push(ModelRoute {
                        model,
                        provider: "Gemini".to_string(),
                        api_method: "code-assist-oauth".to_string(),
                        available: true,
                        detail: String::new(),
                        cheapness: None,
                    });
                }
            }
        }

        // Cursor models
        {
            let cursor_guard = self.cursor.read().unwrap();
            if let Some(ref cursor) = *cursor_guard {
                for model in cursor.available_models_display() {
                    routes.push(ModelRoute {
                        model,
                        provider: "Cursor".to_string(),
                        api_method: "cursor".to_string(),
                        available: true,
                        detail: String::new(),
                        cheapness: None,
                    });
                }
            }
        }

        // OpenRouter models (with per-provider endpoints)
        let has_openrouter = self.openrouter.read().unwrap().is_some();
        if let Some(openrouter) = self.openrouter.read().unwrap().clone() {
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
                    available: has_openrouter,
                    detail: auto_detail,
                    cheapness: cheapness_for_route(&model, "auto", "openrouter"),
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
                            available: has_openrouter,
                            detail,
                            cheapness: openrouter_pricing_from_model_pricing(
                                &ep.pricing,
                                RouteCostSource::OpenRouterEndpoint,
                                RouteCostConfidence::High,
                                Some(format!(
                                    "OpenRouter endpoint pricing for {}",
                                    ep.provider_name
                                )),
                            ),
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
                cheapness: None,
            });
        }

        // Also add Claude/OpenAI models via openrouter as alternative routes
        if has_openrouter {
            for model in known_anthropic_model_ids() {
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
                            cheapness: openrouter_pricing_from_model_pricing(
                                &ep.pricing,
                                RouteCostSource::OpenRouterEndpoint,
                                RouteCostConfidence::High,
                                Some(format!(
                                    "OpenRouter endpoint pricing for {}",
                                    ep.provider_name
                                )),
                            ),
                        });
                    }
                } else {
                    routes.push(ModelRoute {
                        model: model.to_string(),
                        provider: "Anthropic".to_string(),
                        api_method: "openrouter".to_string(),
                        available: true,
                        detail: String::new(),
                        cheapness: cheapness_for_route(&or_model, "Anthropic", "openrouter"),
                    });
                }
            }

            for model in ALL_OPENAI_MODELS {
                let or_model = format!("openai/{}", model);
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
                            cheapness: openrouter_pricing_from_model_pricing(
                                &ep.pricing,
                                RouteCostSource::OpenRouterEndpoint,
                                RouteCostConfidence::High,
                                Some(format!(
                                    "OpenRouter endpoint pricing for {}",
                                    ep.provider_name
                                )),
                            ),
                        });
                    }
                } else {
                    routes.push(ModelRoute {
                        model: model.to_string(),
                        provider: "OpenAI".to_string(),
                        api_method: "openrouter".to_string(),
                        available: true,
                        detail: String::new(),
                        cheapness: cheapness_for_route(&or_model, "OpenAI", "openrouter"),
                    });
                }
            }
        }

        filtered_model_routes(routes)
    }

    async fn prefetch_models(&self) -> Result<()> {
        if let Some(ref anthropic) = self.anthropic {
            anthropic.prefetch_models().await?;
        }
        if let Some(ref claude) = self.claude {
            claude.prefetch_models().await?;
        }
        if let Some(ref openai) = self.openai {
            openai.prefetch_models().await?;
        }
        let openrouter = { self.openrouter.read().unwrap().clone() };
        if let Some(openrouter) = openrouter {
            openrouter.prefetch_models().await?;
        }
        {
            let copilot = self.copilot_api.read().unwrap().clone();
            if let Some(copilot) = copilot {
                copilot.prefetch_models().await?;
            }
        }
        {
            let gemini = self.gemini.read().unwrap().clone();
            if let Some(gemini) = gemini {
                gemini.prefetch_models().await?;
            }
        }
        {
            let cursor = self.cursor.read().unwrap().clone();
            if let Some(cursor) = cursor {
                cursor.prefetch_models().await?;
            }
        }
        Ok(())
    }

    fn on_auth_changed(&self) {
        if openrouter::OpenRouterProvider::has_credentials() {
            match openrouter::OpenRouterProvider::new() {
                Ok(provider) => {
                    crate::logging::info(
                        "Hot-initialized OpenRouter/OpenAI-compatible provider after auth change",
                    );
                    *self.openrouter.write().unwrap() = Some(Arc::new(provider));
                }
                Err(e) => {
                    crate::logging::info(&format!(
                        "Failed to hot-initialize OpenRouter/OpenAI-compatible provider after auth change: {}",
                        e
                    ));
                }
            }
        }

        let already_has = self.copilot_api.read().unwrap().is_some();
        if !already_has {
            let status = crate::auth::AuthStatus::check();
            if status.copilot_has_api_token {
                match copilot::CopilotApiProvider::new() {
                    Ok(p) => {
                        crate::logging::info("Hot-initialized Copilot API provider after login");
                        let provider = Arc::new(p);
                        let p_clone = provider.clone();
                        tokio::spawn(async move {
                            p_clone.detect_tier_and_set_default().await;
                        });
                        *self.copilot_api.write().unwrap() = Some(provider);
                    }
                    Err(e) => {
                        crate::logging::info(&format!(
                            "Failed to hot-initialize Copilot API after login: {}",
                            e
                        ));
                    }
                }
            }
        }

        let already_has_gemini = self.gemini.read().unwrap().is_some();
        if !already_has_gemini && crate::auth::gemini::load_tokens().is_ok() {
            crate::logging::info("Hot-initialized Gemini provider after login");
            *self.gemini.write().unwrap() = Some(gemini::GeminiProvider::new());
        }

        let already_has_cursor = self.cursor.read().unwrap().is_some();
        if !already_has_cursor
            && matches!(
                crate::auth::AuthStatus::check().cursor,
                crate::auth::AuthState::Available
            )
        {
            crate::logging::info("Hot-initialized Cursor provider after login");
            *self.cursor.write().unwrap() = Some(Arc::new(cursor::CursorCliProvider::new()));
        }
    }

    async fn invalidate_credentials(&self) {
        if let Some(ref anthropic) = self.anthropic {
            anthropic.invalidate_credentials().await;
        }
        if let Some(ref openai) = self.openai {
            openai.invalidate_credentials().await;
        }
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
            ActiveProvider::Copilot => self
                .copilot_api
                .read()
                .unwrap()
                .as_ref()
                .map(|o| o.handles_tools_internally())
                .unwrap_or(false),
            ActiveProvider::Gemini => false,
            ActiveProvider::Cursor => self
                .cursor
                .read()
                .unwrap()
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
            ActiveProvider::Copilot => None,
            ActiveProvider::Gemini => None,
            ActiveProvider::Cursor => None,
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
            ActiveProvider::Copilot => vec![],
            ActiveProvider::Gemini => vec![],
            ActiveProvider::Cursor => vec![],
            _ => vec![],
        }
    }

    fn service_tier(&self) -> Option<String> {
        match self.active_provider() {
            ActiveProvider::OpenAI => self.openai.as_ref().and_then(|o| o.service_tier()),
            _ => None,
        }
    }

    fn set_service_tier(&self, service_tier: &str) -> Result<()> {
        match self.active_provider() {
            ActiveProvider::OpenAI => self
                .openai
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("OpenAI provider not available"))?
                .set_service_tier(service_tier),
            _ => Err(anyhow::anyhow!(
                "Service tier switching is only supported for OpenAI models"
            )),
        }
    }

    fn available_service_tiers(&self) -> Vec<&'static str> {
        match self.active_provider() {
            ActiveProvider::OpenAI => self
                .openai
                .as_ref()
                .map(|o| o.available_service_tiers())
                .unwrap_or_default(),
            _ => vec![],
        }
    }

    fn native_compaction_mode(&self) -> Option<String> {
        match self.active_provider() {
            ActiveProvider::OpenAI => self
                .openai
                .as_ref()
                .and_then(|o| o.native_compaction_mode()),
            _ => None,
        }
    }

    fn native_compaction_threshold_tokens(&self) -> Option<usize> {
        match self.active_provider() {
            ActiveProvider::OpenAI => self
                .openai
                .as_ref()
                .and_then(|o| o.native_compaction_threshold_tokens()),
            _ => None,
        }
    }

    fn transport(&self) -> Option<String> {
        match self.active_provider() {
            ActiveProvider::OpenAI => self.openai.as_ref().and_then(|o| o.transport()),
            _ => None,
        }
    }

    fn set_transport(&self, transport: &str) -> Result<()> {
        match self.active_provider() {
            ActiveProvider::OpenAI => self
                .openai
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("OpenAI provider not available"))?
                .set_transport(transport),
            _ => Err(anyhow::anyhow!(
                "Transport switching is only supported for OpenAI models"
            )),
        }
    }

    fn available_transports(&self) -> Vec<&'static str> {
        match self.active_provider() {
            ActiveProvider::OpenAI => self
                .openai
                .as_ref()
                .map(|o| o.available_transports())
                .unwrap_or_default(),
            ActiveProvider::Gemini => vec![],
            ActiveProvider::Cursor => vec![],
            _ => vec![],
        }
    }

    fn supports_compaction(&self) -> bool {
        match self.active_provider() {
            ActiveProvider::Claude => {
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
            ActiveProvider::Copilot => self
                .copilot_api
                .read()
                .unwrap()
                .as_ref()
                .map(|o| o.supports_compaction())
                .unwrap_or(false),
            ActiveProvider::Gemini => self
                .gemini
                .read()
                .unwrap()
                .as_ref()
                .map(|o| o.supports_compaction())
                .unwrap_or(false),
            ActiveProvider::Cursor => self
                .cursor
                .read()
                .unwrap()
                .as_ref()
                .map(|o| o.supports_compaction())
                .unwrap_or(false),
            ActiveProvider::OpenRouter => self
                .openrouter
                .read()
                .unwrap()
                .as_ref()
                .map(|o| o.supports_compaction())
                .unwrap_or(false),
        }
    }

    fn uses_jcode_compaction(&self) -> bool {
        match self.active_provider() {
            ActiveProvider::Claude => {
                if self.anthropic.is_some() {
                    true
                } else {
                    self.claude
                        .as_ref()
                        .map(|c| c.uses_jcode_compaction())
                        .unwrap_or(false)
                }
            }
            ActiveProvider::OpenAI => self
                .openai
                .as_ref()
                .map(|o| o.uses_jcode_compaction())
                .unwrap_or(false),
            ActiveProvider::Copilot => self
                .copilot_api
                .read()
                .unwrap()
                .as_ref()
                .map(|o| o.uses_jcode_compaction())
                .unwrap_or(false),
            ActiveProvider::Gemini => self
                .gemini
                .read()
                .unwrap()
                .as_ref()
                .map(|o| o.uses_jcode_compaction())
                .unwrap_or(false),
            ActiveProvider::Cursor => self
                .cursor
                .read()
                .unwrap()
                .as_ref()
                .map(|o| o.uses_jcode_compaction())
                .unwrap_or(false),
            ActiveProvider::OpenRouter => self
                .openrouter
                .read()
                .unwrap()
                .as_ref()
                .map(|o| o.uses_jcode_compaction())
                .unwrap_or(false),
        }
    }

    async fn native_compact(
        &self,
        messages: &[Message],
        existing_summary_text: Option<&str>,
        existing_openai_encrypted_content: Option<&str>,
    ) -> Result<NativeCompactionResult> {
        match self.active_provider() {
            ActiveProvider::Claude => {
                if let Some(anthropic) = self.anthropic.as_ref() {
                    anthropic
                        .native_compact(
                            messages,
                            existing_summary_text,
                            existing_openai_encrypted_content,
                        )
                        .await
                } else if let Some(claude) = self.claude.as_ref() {
                    claude
                        .native_compact(
                            messages,
                            existing_summary_text,
                            existing_openai_encrypted_content,
                        )
                        .await
                } else {
                    Err(anyhow::anyhow!("Claude provider unavailable"))
                }
            }
            ActiveProvider::OpenAI => {
                if let Some(openai) = self.openai.as_ref() {
                    openai
                        .native_compact(
                            messages,
                            existing_summary_text,
                            existing_openai_encrypted_content,
                        )
                        .await
                } else {
                    Err(anyhow::anyhow!("OpenAI provider unavailable"))
                }
            }
            ActiveProvider::Copilot => {
                let provider = self.copilot_api.read().unwrap().clone();
                if let Some(copilot) = provider {
                    copilot
                        .native_compact(
                            messages,
                            existing_summary_text,
                            existing_openai_encrypted_content,
                        )
                        .await
                } else {
                    Err(anyhow::anyhow!("Copilot provider unavailable"))
                }
            }
            ActiveProvider::Gemini => {
                let provider = self.gemini.read().unwrap().clone();
                if let Some(gemini) = provider {
                    gemini
                        .native_compact(
                            messages,
                            existing_summary_text,
                            existing_openai_encrypted_content,
                        )
                        .await
                } else {
                    Err(anyhow::anyhow!("Gemini provider unavailable"))
                }
            }
            ActiveProvider::Cursor => {
                let provider = self.cursor.read().unwrap().clone();
                if let Some(cursor) = provider {
                    cursor
                        .native_compact(
                            messages,
                            existing_summary_text,
                            existing_openai_encrypted_content,
                        )
                        .await
                } else {
                    Err(anyhow::anyhow!("Cursor provider unavailable"))
                }
            }
            ActiveProvider::OpenRouter => {
                let provider = self.openrouter.read().unwrap().clone();
                if let Some(openrouter) = provider {
                    openrouter
                        .native_compact(
                            messages,
                            existing_summary_text,
                            existing_openai_encrypted_content,
                        )
                        .await
                } else {
                    Err(anyhow::anyhow!("OpenRouter provider unavailable"))
                }
            }
        }
    }

    fn set_premium_mode(&self, mode: copilot::PremiumMode) {
        if let Some(ref copilot) = *self.copilot_api.read().unwrap() {
            copilot.set_premium_mode(mode);
        }
    }

    fn premium_mode(&self) -> copilot::PremiumMode {
        if let Some(ref copilot) = *self.copilot_api.read().unwrap() {
            copilot.get_premium_mode()
        } else {
            copilot::PremiumMode::Normal
        }
    }

    fn drain_startup_notices(&self) -> Vec<String> {
        std::mem::take(&mut *self.startup_notices.write().unwrap())
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
            ActiveProvider::Copilot => self
                .copilot_api
                .read()
                .unwrap()
                .as_ref()
                .map(|o| o.context_window())
                .unwrap_or(DEFAULT_CONTEXT_LIMIT),
            ActiveProvider::Gemini => self
                .gemini
                .read()
                .unwrap()
                .as_ref()
                .map(|o| o.context_window())
                .unwrap_or(DEFAULT_CONTEXT_LIMIT),
            ActiveProvider::Cursor => self
                .cursor
                .read()
                .unwrap()
                .as_ref()
                .map(|o| o.context_window())
                .unwrap_or(DEFAULT_CONTEXT_LIMIT),
            ActiveProvider::OpenRouter => self
                .openrouter
                .read()
                .unwrap()
                .as_ref()
                .map(|o| o.context_window())
                .unwrap_or(DEFAULT_CONTEXT_LIMIT),
        }
    }

    fn fork(&self) -> Arc<dyn Provider> {
        let current_model = self.model();
        let active = self.active_provider();

        let claude = if matches!(active, ActiveProvider::Claude) && self.claude.is_some() {
            Some(claude::ClaudeProvider::new())
        } else {
            None
        };
        let anthropic = if self.anthropic.is_some() {
            Some(anthropic::AnthropicProvider::new())
        } else {
            None
        };
        let openai = if self.openai.is_some() {
            auth::codex::load_credentials()
                .ok()
                .map(openai::OpenAIProvider::new)
        } else {
            None
        };
        let copilot_api = self.copilot_api.read().unwrap().clone();
        let gemini_provider = self.gemini.read().unwrap().clone();
        let cursor_provider = if self.cursor.read().unwrap().is_some() {
            Some(Arc::new(cursor::CursorCliProvider::new()))
        } else {
            None
        };
        let openrouter = if self.openrouter.read().unwrap().is_some() {
            openrouter::OpenRouterProvider::new().ok().map(Arc::new)
        } else {
            None
        };

        let provider = Self {
            claude,
            anthropic,
            openai,
            copilot_api: RwLock::new(copilot_api),
            gemini: RwLock::new(gemini_provider),
            cursor: RwLock::new(cursor_provider),
            openrouter: RwLock::new(openrouter),
            active: RwLock::new(active),
            has_claude_creds: self.has_claude_creds,
            has_openai_creds: self.has_openai_creds,
            has_gemini_creds: self.has_gemini_creds,
            has_cursor_creds: self.has_cursor_creds,
            use_claude_cli: self.use_claude_cli,
            startup_notices: RwLock::new(Vec::new()),
            forced_provider: self.forced_provider,
        };

        provider.spawn_openai_catalog_refresh_if_needed();
        if matches!(active, ActiveProvider::Copilot) {
            let _ = provider.set_model(&format!("copilot:{}", current_model));
        } else if matches!(active, ActiveProvider::Cursor) {
            let _ = provider.set_model(&format!("cursor:{}", current_model));
        } else {
            let _ = provider.set_model(&current_model);
        }
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
            ActiveProvider::Copilot => None,
            ActiveProvider::Gemini => None,
            ActiveProvider::Cursor => None,
            ActiveProvider::OpenRouter => None,
        }
    }

    fn switch_active_provider_to(&self, provider: &str) -> Result<()> {
        let target = Self::parse_provider_hint(provider)
            .ok_or_else(|| anyhow::anyhow!("Unknown provider `{}`", provider))?;
        if !self.provider_is_configured(target) {
            anyhow::bail!(
                "Provider `{}` is not configured in this session",
                Self::provider_key(target)
            );
        }
        self.set_active_provider(target);
        self.auto_select_multi_account_for_provider(target);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_clean_provider_test_env<T>(f: impl FnOnce() -> T) -> T {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let prev_home = std::env::var_os("JCODE_HOME");
        let prev_subscription =
            std::env::var_os(crate::subscription_catalog::JCODE_SUBSCRIPTION_ACTIVE_ENV);
        crate::env::set_var("JCODE_HOME", temp.path());
        crate::subscription_catalog::clear_runtime_env();
        crate::auth::claude::set_active_account_override(None);
        crate::auth::codex::set_active_account_override(None);

        let result = f();

        crate::auth::claude::set_active_account_override(None);
        crate::auth::codex::set_active_account_override(None);
        if let Some(prev_home) = prev_home {
            crate::env::set_var("JCODE_HOME", prev_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
        if let Some(prev_subscription) = prev_subscription {
            crate::env::set_var(
                crate::subscription_catalog::JCODE_SUBSCRIPTION_ACTIVE_ENV,
                prev_subscription,
            );
        } else {
            crate::env::remove_var(crate::subscription_catalog::JCODE_SUBSCRIPTION_ACTIVE_ENV);
        }
        crate::subscription_catalog::clear_runtime_env();
        result
    }

    fn test_multi_provider_with_cursor() -> MultiProvider {
        MultiProvider {
            claude: None,
            anthropic: None,
            openai: None,
            copilot_api: RwLock::new(None),
            gemini: RwLock::new(None),
            cursor: RwLock::new(Some(Arc::new(cursor::CursorCliProvider::new()))),
            openrouter: RwLock::new(None),
            active: RwLock::new(ActiveProvider::Cursor),
            has_claude_creds: false,
            has_openai_creds: false,
            has_gemini_creds: false,
            has_cursor_creds: true,
            use_claude_cli: false,
            startup_notices: RwLock::new(Vec::new()),
            forced_provider: None,
        }
    }

    #[test]
    fn test_provider_for_model_claude() {
        assert_eq!(provider_for_model("claude-opus-4-6"), Some("claude"));
        assert_eq!(provider_for_model("claude-opus-4-6[1m]"), Some("claude"));
        assert_eq!(provider_for_model("claude-sonnet-4-6"), Some("claude"));
    }

    #[test]
    fn test_provider_for_model_openai() {
        assert_eq!(provider_for_model("gpt-5.2-codex"), Some("openai"));
        assert_eq!(provider_for_model("gpt-5.4"), Some("openai"));
        assert_eq!(provider_for_model("gpt-5.4[1m]"), Some("openai"));
        assert_eq!(provider_for_model("gpt-5.4-pro"), Some("openai"));
    }

    #[test]
    fn test_provider_for_model_gemini() {
        assert_eq!(provider_for_model("gemini-2.5-pro"), Some("gemini"));
        assert_eq!(provider_for_model("gemini-2.5-flash"), Some("gemini"));
        assert_eq!(provider_for_model("gemini-3-pro-preview"), Some("gemini"));
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
    fn test_provider_for_model_cursor() {
        assert_eq!(provider_for_model("composer-2-fast"), Some("cursor"));
        assert_eq!(provider_for_model("composer-2"), Some("cursor"));
        assert_eq!(provider_for_model("sonnet-4.6"), Some("cursor"));
        assert_eq!(provider_for_model("gpt-5"), Some("openai"));
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
    fn test_context_limit_gpt_5_4() {
        assert_eq!(context_limit_for_model("gpt-5.4"), Some(1_000_000));
        assert_eq!(context_limit_for_model("gpt-5.4-pro"), Some(1_000_000));
        assert_eq!(context_limit_for_model("gpt-5.4[1m]"), Some(1_000_000));
    }

    #[test]
    fn test_context_limit_respects_provider_hint() {
        assert_eq!(
            context_limit_for_model_with_provider("gpt-5.4", Some("openai")),
            Some(1_000_000)
        );
        assert_eq!(
            context_limit_for_model_with_provider("gpt-5.4", Some("copilot")),
            Some(128_000)
        );
        assert_eq!(
            context_limit_for_model_with_provider("claude-sonnet-4-6[1m]", Some("claude")),
            Some(1_048_576)
        );
    }

    #[test]
    fn test_resolve_model_capabilities_uses_provider_hint() {
        let openai = resolve_model_capabilities("gpt-5.4", Some("openai"));
        assert_eq!(openai.provider.as_deref(), Some("openai"));
        assert_eq!(openai.context_window, Some(1_000_000));

        let copilot = resolve_model_capabilities("gpt-5.4", Some("copilot"));
        assert_eq!(copilot.provider.as_deref(), Some("copilot"));
        assert_eq!(copilot.context_window, Some(128_000));

        let gemini = resolve_model_capabilities("gemini-2.5-pro", Some("gemini"));
        assert_eq!(gemini.provider.as_deref(), Some("gemini"));
        assert_eq!(gemini.context_window, Some(1_000_000));
    }

    #[test]
    fn test_normalize_model_id_strips_1m_suffix() {
        assert_eq!(models::normalize_model_id("gpt-5.4[1m]"), "gpt-5.4");
        assert_eq!(models::normalize_model_id(" GPT-5.4[1M] "), "gpt-5.4");
    }

    #[test]
    fn test_merge_openai_model_ids_appends_dynamic_oauth_models() {
        let models = models::merge_openai_model_ids(vec![
            "gpt-5.4".to_string(),
            "gpt-5.4-fast-preview".to_string(),
            "gpt-5.4-fast-preview".to_string(),
            " gpt-5.5-experimental ".to_string(),
        ]);

        assert!(models.iter().any(|model| model == "gpt-5.4"));
        assert!(models.iter().any(|model| model == "gpt-5.4-fast-preview"));
        assert!(models.iter().any(|model| model == "gpt-5.5-experimental"));
        assert_eq!(
            models
                .iter()
                .filter(|model| model.as_str() == "gpt-5.4-fast-preview")
                .count(),
            1
        );
    }

    #[test]
    fn test_merge_anthropic_model_ids_appends_dynamic_models() {
        let models = models::merge_anthropic_model_ids(vec![
            "claude-opus-4-6".to_string(),
            "claude-sonnet-5-preview".to_string(),
            "claude-sonnet-5-preview".to_string(),
            " claude-haiku-5-beta ".to_string(),
        ]);

        assert!(models.iter().any(|model| model == "claude-opus-4-6"));
        assert!(models.iter().any(|model| model == "claude-opus-4-6[1m]"));
        assert!(
            models
                .iter()
                .any(|model| model == "claude-sonnet-5-preview")
        );
        assert!(models.iter().any(|model| model == "claude-haiku-5-beta"));
        assert_eq!(
            models
                .iter()
                .filter(|model| model.as_str() == "claude-sonnet-5-preview")
                .count(),
            1
        );
    }

    #[test]
    fn test_parse_anthropic_model_catalog_reads_context_limits() {
        let data = serde_json::json!({
            "data": [
                {
                    "id": "claude-opus-4-6",
                    "max_input_tokens": 1_048_576
                },
                {
                    "id": "claude-sonnet-5-preview",
                    "max_input_tokens": 333_000
                }
            ]
        });

        let catalog = models::parse_anthropic_model_catalog(&data);
        assert!(
            catalog
                .available_models
                .contains(&"claude-opus-4-6".to_string())
        );
        assert!(
            catalog
                .available_models
                .contains(&"claude-sonnet-5-preview".to_string())
        );
        assert_eq!(
            catalog.context_limits.get("claude-opus-4-6"),
            Some(&1_048_576)
        );
        assert_eq!(
            catalog.context_limits.get("claude-sonnet-5-preview"),
            Some(&333_000)
        );
    }

    #[test]
    fn test_context_limit_claude() {
        with_clean_provider_test_env(|| {
            // Default (no subscription info = assumes Max) -> 1M for opus/sonnet 4.6
            assert_eq!(context_limit_for_model("claude-opus-4-6"), Some(1_048_576));
            assert_eq!(
                context_limit_for_model("claude-sonnet-4-6"),
                Some(1_048_576)
            );
            assert_eq!(
                context_limit_for_model("claude-opus-4-6[1m]"),
                Some(1_048_576)
            );
            assert_eq!(
                context_limit_for_model("claude-sonnet-4-6[1m]"),
                Some(1_048_576)
            );
        });
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

    #[test]
    fn test_fallback_sequence_includes_all_providers() {
        assert_eq!(
            MultiProvider::fallback_sequence(ActiveProvider::Claude),
            vec![
                ActiveProvider::Claude,
                ActiveProvider::OpenAI,
                ActiveProvider::Copilot,
                ActiveProvider::Gemini,
                ActiveProvider::Cursor,
                ActiveProvider::OpenRouter,
            ]
        );
        assert_eq!(
            MultiProvider::fallback_sequence(ActiveProvider::OpenAI),
            vec![
                ActiveProvider::OpenAI,
                ActiveProvider::Claude,
                ActiveProvider::Copilot,
                ActiveProvider::Gemini,
                ActiveProvider::Cursor,
                ActiveProvider::OpenRouter,
            ]
        );
        assert_eq!(
            MultiProvider::fallback_sequence(ActiveProvider::Copilot),
            vec![
                ActiveProvider::Copilot,
                ActiveProvider::Claude,
                ActiveProvider::OpenAI,
                ActiveProvider::Gemini,
                ActiveProvider::Cursor,
                ActiveProvider::OpenRouter,
            ]
        );
        assert_eq!(
            MultiProvider::fallback_sequence(ActiveProvider::Gemini),
            vec![
                ActiveProvider::Gemini,
                ActiveProvider::Claude,
                ActiveProvider::OpenAI,
                ActiveProvider::Copilot,
                ActiveProvider::Cursor,
                ActiveProvider::OpenRouter,
            ]
        );
        assert_eq!(
            MultiProvider::fallback_sequence(ActiveProvider::OpenRouter),
            vec![
                ActiveProvider::OpenRouter,
                ActiveProvider::Claude,
                ActiveProvider::OpenAI,
                ActiveProvider::Copilot,
                ActiveProvider::Gemini,
                ActiveProvider::Cursor,
            ]
        );
    }

    #[test]
    fn test_parse_provider_hint_supports_known_values() {
        assert_eq!(
            MultiProvider::parse_provider_hint("claude"),
            Some(ActiveProvider::Claude)
        );
        assert_eq!(
            MultiProvider::parse_provider_hint("Anthropic"),
            Some(ActiveProvider::Claude)
        );
        assert_eq!(
            MultiProvider::parse_provider_hint("openai"),
            Some(ActiveProvider::OpenAI)
        );
        assert_eq!(
            MultiProvider::parse_provider_hint("copilot"),
            Some(ActiveProvider::Copilot)
        );
        assert_eq!(
            MultiProvider::parse_provider_hint("gemini"),
            Some(ActiveProvider::Gemini)
        );
        assert_eq!(
            MultiProvider::parse_provider_hint("openrouter"),
            Some(ActiveProvider::OpenRouter)
        );
        assert_eq!(
            MultiProvider::parse_provider_hint("cursor"),
            Some(ActiveProvider::Cursor)
        );
    }

    #[test]
    fn test_cursor_models_are_included_in_available_models_display_when_configured() {
        with_clean_provider_test_env(|| {
            let provider = test_multi_provider_with_cursor();
            let models = provider.available_models_display();
            assert!(models.iter().any(|model| model == "composer-2-fast"));
            assert!(models.iter().any(|model| model == "composer-2"));
        });
    }

    #[test]
    fn test_cursor_models_are_included_in_model_routes_when_configured() {
        with_clean_provider_test_env(|| {
            let provider = test_multi_provider_with_cursor();
            let routes = provider.model_routes();
            assert!(routes.iter().any(|route| {
                route.model == "composer-2-fast"
                    && route.provider == "Cursor"
                    && route.api_method == "cursor"
                    && route.available
            }));
        });
    }

    #[test]
    fn test_set_model_switches_to_cursor_for_cursor_models() {
        with_clean_provider_test_env(|| {
            let provider = test_multi_provider_with_cursor();
            *provider.active.write().unwrap() = ActiveProvider::Claude;

            provider
                .set_model("composer-2-fast")
                .expect("cursor model should route to Cursor");

            assert_eq!(provider.active_provider(), ActiveProvider::Cursor);
            assert_eq!(provider.model(), "composer-2-fast");
        });
    }

    #[test]
    fn test_set_model_supports_explicit_cursor_prefix() {
        with_clean_provider_test_env(|| {
            let provider = test_multi_provider_with_cursor();
            *provider.active.write().unwrap() = ActiveProvider::OpenAI;

            provider
                .set_model("cursor:gpt-5")
                .expect("explicit cursor prefix should force Cursor route");

            assert_eq!(provider.active_provider(), ActiveProvider::Cursor);
            assert_eq!(provider.model(), "gpt-5");
        });
    }

    #[test]
    fn test_forced_provider_disables_cross_provider_fallback_sequence() {
        assert_eq!(
            MultiProvider::fallback_sequence_for(
                ActiveProvider::Claude,
                Some(ActiveProvider::OpenAI)
            ),
            vec![ActiveProvider::Claude, ActiveProvider::OpenAI]
        );
        assert_eq!(
            MultiProvider::fallback_sequence_for(
                ActiveProvider::OpenAI,
                Some(ActiveProvider::OpenAI)
            ),
            vec![ActiveProvider::OpenAI]
        );
        assert_eq!(
            MultiProvider::fallback_sequence_for(ActiveProvider::Claude, None),
            MultiProvider::fallback_sequence(ActiveProvider::Claude)
        );
    }

    #[test]
    fn test_set_model_rejects_cross_provider_without_creds() {
        let _guard = crate::storage::lock_test_env();
        crate::subscription_catalog::clear_runtime_env();
        crate::env::remove_var("JCODE_ACTIVE_PROVIDER");
        crate::env::remove_var("JCODE_FORCE_PROVIDER");

        let provider = MultiProvider {
            claude: None,
            anthropic: None,
            openai: None,
            copilot_api: RwLock::new(None),
            gemini: RwLock::new(None),
            cursor: RwLock::new(None),
            openrouter: RwLock::new(None),
            active: RwLock::new(ActiveProvider::OpenAI),
            has_claude_creds: false,
            has_openai_creds: false,
            has_gemini_creds: false,
            has_cursor_creds: false,
            use_claude_cli: false,
            startup_notices: RwLock::new(Vec::new()),
            forced_provider: Some(ActiveProvider::OpenAI),
        };

        let err = provider
            .set_model("claude-sonnet-4-6")
            .expect_err("model routing should reject when target provider has no creds");
        assert!(
            err.to_string().contains("credentials are not configured"),
            "expected credentials error, got: {}",
            err
        );
    }

    #[test]
    fn test_openai_usage_exhaustion_requires_both_windows() {
        let mut usage = crate::usage::OpenAIUsageData::default();
        usage.five_hour = Some(crate::usage::OpenAIUsageWindow {
            name: "5h".to_string(),
            usage_ratio: 1.0,
            resets_at: None,
        });
        usage.seven_day = Some(crate::usage::OpenAIUsageWindow {
            name: "7d".to_string(),
            usage_ratio: 0.50,
            resets_at: None,
        });
        assert!(!MultiProvider::is_openai_usage_exhausted_from_usage(&usage));

        usage.seven_day = Some(crate::usage::OpenAIUsageWindow {
            name: "7d".to_string(),
            usage_ratio: 1.0,
            resets_at: None,
        });
        assert!(MultiProvider::is_openai_usage_exhausted_from_usage(&usage));
    }

    #[test]
    fn test_auto_default_prefers_openai_over_claude_when_both_available() {
        let active =
            MultiProvider::auto_default_provider(true, true, false, false, false, false, false);
        assert_eq!(active, ActiveProvider::OpenAI);
    }

    #[test]
    fn test_auto_default_prefers_copilot_when_zero_premium_mode_enabled() {
        let active = MultiProvider::auto_default_provider(true, true, true, true, true, true, true);
        assert_eq!(active, ActiveProvider::Copilot);
    }

    #[test]
    fn test_should_failover_on_403_forbidden() {
        let err = anyhow::anyhow!(
            "Copilot token exchange failed (HTTP 403 Forbidden): not accessible by integration"
        );
        assert!(MultiProvider::classify_failover_error(&err).should_failover());
    }

    #[test]
    fn test_should_failover_on_token_exchange_failed() {
        let msg = r#"Copilot token exchange failed (HTTP 403 Forbidden): {"error_details":{"title":"Contact Support"}}"#;
        let err = anyhow::anyhow!("{}", msg);
        assert!(MultiProvider::classify_failover_error(&err).should_failover());
    }

    #[test]
    fn test_should_failover_on_access_denied() {
        let err = anyhow::anyhow!("Access denied: account suspended");
        assert!(MultiProvider::classify_failover_error(&err).should_failover());
    }

    #[test]
    fn test_should_failover_when_status_code_starts_message() {
        let err = anyhow::anyhow!("401 unauthorized");
        assert!(MultiProvider::classify_failover_error(&err).should_failover());
        assert_eq!(
            MultiProvider::classify_failover_error(&err),
            FailoverDecision::RetryAndMarkUnavailable
        );
    }

    #[test]
    fn test_should_not_failover_on_non_standalone_status_digits() {
        let err = anyhow::anyhow!("backend returned code 14290");
        assert!(!MultiProvider::classify_failover_error(&err).should_failover());
    }

    #[test]
    fn test_context_limit_error_fails_over_without_marking_provider_unavailable() {
        let err = anyhow::anyhow!("Context length exceeded maximum context window");
        assert!(MultiProvider::classify_failover_error(&err).should_failover());
        assert_eq!(
            MultiProvider::classify_failover_error(&err),
            FailoverDecision::RetryNextProvider
        );
    }

    #[test]
    fn test_should_not_failover_on_generic_error() {
        let err = anyhow::anyhow!("Connection timed out");
        assert!(!MultiProvider::classify_failover_error(&err).should_failover());
    }

    #[test]
    fn test_no_provider_error_mentions_tokens_and_details() {
        let provider = MultiProvider {
            claude: None,
            anthropic: None,
            openai: None,
            copilot_api: RwLock::new(None),
            gemini: RwLock::new(None),
            cursor: RwLock::new(None),
            openrouter: RwLock::new(None),
            active: RwLock::new(ActiveProvider::OpenAI),
            has_claude_creds: false,
            has_openai_creds: false,
            has_gemini_creds: false,
            has_cursor_creds: false,
            use_claude_cli: false,
            startup_notices: RwLock::new(Vec::new()),
            forced_provider: None,
        };
        let err = provider.no_provider_available_error(&[
            "OpenAI: rate limited".to_string(),
            "GitHub Copilot: not configured".to_string(),
        ]);
        let text = err.to_string();
        assert!(text.contains("No tokens/providers left"));
        assert!(text.contains("OpenAI: rate limited"));
        assert!(text.contains("GitHub Copilot: not configured"));
    }

    #[test]
    fn test_openai_provider_unavailability_is_scoped_per_account() {
        let _guard = crate::storage::lock_test_env();

        crate::auth::codex::set_active_account_override(Some("work".to_string()));
        clear_all_provider_unavailability_for_account();
        record_provider_unavailable_for_account("openai", "work rate limit");
        assert!(
            provider_unavailability_detail_for_account("openai")
                .unwrap_or_default()
                .contains("work rate limit")
        );

        crate::auth::codex::set_active_account_override(Some("personal".to_string()));
        clear_all_provider_unavailability_for_account();
        assert!(provider_unavailability_detail_for_account("openai").is_none());

        crate::auth::codex::set_active_account_override(Some("work".to_string()));
        assert!(
            provider_unavailability_detail_for_account("openai")
                .unwrap_or_default()
                .contains("work rate limit")
        );

        clear_all_provider_unavailability_for_account();
        crate::auth::codex::set_active_account_override(None);
    }

    #[test]
    fn test_openai_model_catalog_is_scoped_per_account() {
        let _guard = crate::storage::lock_test_env();
        let work_model = "scoped-work-model-123";
        let personal_model = "scoped-personal-model-456";

        crate::auth::codex::set_active_account_override(Some("work".to_string()));
        populate_account_models(vec![work_model.to_string()]);
        assert!(known_openai_model_ids().contains(&work_model.to_string()));
        assert!(!known_openai_model_ids().contains(&personal_model.to_string()));

        crate::auth::codex::set_active_account_override(Some("personal".to_string()));
        assert!(!known_openai_model_ids().contains(&work_model.to_string()));
        populate_account_models(vec![personal_model.to_string()]);
        assert!(known_openai_model_ids().contains(&personal_model.to_string()));
        assert!(!known_openai_model_ids().contains(&work_model.to_string()));

        crate::auth::codex::set_active_account_override(Some("work".to_string()));
        assert!(known_openai_model_ids().contains(&work_model.to_string()));
        assert!(!known_openai_model_ids().contains(&personal_model.to_string()));

        crate::auth::codex::set_active_account_override(None);
    }

    #[test]
    fn test_normalize_copilot_model_name_claude() {
        assert_eq!(
            normalize_copilot_model_name("claude-opus-4.6"),
            Some("claude-opus-4-6")
        );
        assert_eq!(
            normalize_copilot_model_name("claude-sonnet-4.6"),
            Some("claude-sonnet-4-6")
        );
        assert_eq!(
            normalize_copilot_model_name("claude-sonnet-4.5"),
            Some("claude-sonnet-4-5")
        );
        assert_eq!(
            normalize_copilot_model_name("claude-haiku-4.5"),
            Some("claude-haiku-4-5")
        );
    }

    #[test]
    fn test_normalize_copilot_model_name_already_canonical() {
        assert_eq!(normalize_copilot_model_name("claude-opus-4-6"), None);
        assert_eq!(normalize_copilot_model_name("claude-sonnet-4-6"), None);
        assert_eq!(normalize_copilot_model_name("gpt-5.3-codex"), None);
    }

    #[test]
    fn test_normalize_copilot_model_name_unknown() {
        assert_eq!(normalize_copilot_model_name("gemini-3-pro-preview"), None);
        assert_eq!(normalize_copilot_model_name("grok-code-fast-1"), None);
    }

    #[test]
    fn test_provider_for_model_copilot_dot_notation() {
        assert_eq!(provider_for_model("claude-opus-4.6"), Some("claude"));
        assert_eq!(provider_for_model("claude-sonnet-4.6"), Some("claude"));
        assert_eq!(provider_for_model("claude-haiku-4.5"), Some("claude"));
        assert_eq!(provider_for_model("gpt-4.1"), Some("openai"));
    }

    #[test]
    fn test_subscription_model_guard_allows_only_curated_models_when_enabled() {
        let _guard = crate::storage::lock_test_env();
        crate::subscription_catalog::clear_runtime_env();
        crate::subscription_catalog::apply_runtime_env();

        assert!(ensure_model_allowed_for_subscription("moonshotai/kimi-k2.5").is_ok());
        assert!(ensure_model_allowed_for_subscription("kimi/k2.5").is_ok());
        assert!(ensure_model_allowed_for_subscription("gpt-5.4").is_err());

        crate::subscription_catalog::clear_runtime_env();
    }

    #[test]
    fn test_filtered_display_models_respects_curated_subscription_catalog() {
        let _guard = crate::storage::lock_test_env();
        crate::subscription_catalog::clear_runtime_env();
        crate::subscription_catalog::apply_runtime_env();

        let filtered = filtered_display_models(vec![
            "gpt-5.4".to_string(),
            "moonshotai/kimi-k2.5".to_string(),
            "openrouter/healer-alpha".to_string(),
        ]);

        assert_eq!(
            filtered,
            vec![
                "moonshotai/kimi-k2.5".to_string(),
                "openrouter/healer-alpha".to_string()
            ]
        );

        crate::subscription_catalog::clear_runtime_env();
    }

    #[test]
    fn test_subscription_filters_do_not_activate_from_saved_credentials_alone() {
        let _guard = crate::storage::lock_test_env();
        crate::subscription_catalog::clear_runtime_env();
        crate::env::set_var(crate::subscription_catalog::JCODE_API_KEY_ENV, "test-key");

        assert!(ensure_model_allowed_for_subscription("gpt-5.4").is_ok());
        assert_eq!(
            filtered_display_models(vec![
                "gpt-5.4".to_string(),
                "moonshotai/kimi-k2.5".to_string(),
            ]),
            vec!["gpt-5.4".to_string(), "moonshotai/kimi-k2.5".to_string()]
        );

        crate::env::remove_var(crate::subscription_catalog::JCODE_API_KEY_ENV);
        crate::subscription_catalog::clear_runtime_env();
    }
}
