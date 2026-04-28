mod accessors;
mod account_failover;
pub mod anthropic;
pub mod antigravity;
mod catalog_refresh;
pub mod claude;
pub mod cli_common;
pub mod copilot;
pub mod cursor;
mod dispatch;
mod failover;
pub mod gemini;
pub mod jcode;
pub mod models;
mod multi_provider;
pub mod openai;
pub(crate) mod openai_request;
pub mod openrouter;
pub mod pricing;
mod route_builders;
mod routing;
mod selection;
mod startup;

use crate::auth;
use crate::message::{ContentBlock, Message, Role, StreamEvent, ToolDefinition};
use account_failover::{
    account_usage_probe, active_account_label_for_provider, maybe_annotate_limit_summary,
    same_provider_account_candidates, same_provider_account_failover_enabled,
    set_account_override_for_provider,
};
use anyhow::Result;
use async_trait::async_trait;
#[cfg(test)]
use failover::FailoverDecision;
use futures::Stream;
use std::borrow::Cow;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

// Re-export native tool result types for use by agent
pub use catalog_refresh::{ModelCatalogRefreshSummary, summarize_model_catalog_refresh};
pub use claude::{NativeToolResult, NativeToolResultSender};
pub(crate) use failover::{ProviderFailoverPrompt, parse_failover_prompt_message};
pub use jcode_provider_core::{
    CHEAPNESS_REFERENCE_INPUT_TOKENS, CHEAPNESS_REFERENCE_OUTPUT_TOKENS, ModelRoute,
    NativeCompactionResult, RouteBillingKind, RouteCheapnessEstimate, RouteCostConfidence,
    RouteCostSource, shared_http_client,
};
pub use route_builders::{
    build_anthropic_oauth_route, build_copilot_route, build_openai_oauth_route,
    build_openrouter_auto_route, build_openrouter_endpoint_route,
    build_openrouter_fallback_provider_route, is_listable_model_name,
    listable_model_names_from_routes, openrouter_catalog_model_id,
};
pub(crate) use routing::{
    anthropic_api_key_route_availability, anthropic_oauth_route_availability,
    is_transient_transport_error, should_eager_detect_copilot_tier,
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
    /// system_static: Static content (instruction files, base prompt) - cached
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
            .filter(|model| is_listable_model_name(model))
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

    /// Force-refresh model catalog data and return a before/after summary.
    async fn refresh_model_catalog(&self) -> Result<ModelCatalogRefreshSummary> {
        let before_models = self.available_models_display();
        let before_routes = self.model_routes();
        self.prefetch_models().await?;
        let after_models = self.available_models_display();
        let after_routes = self.model_routes();
        Ok(summarize_model_catalog_refresh(
            before_models,
            after_models,
            before_routes,
            after_routes,
        ))
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
    "gpt-5.5",
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

use self::dispatch::CompletionMode;
use self::models::normalize_copilot_model_name;
pub use self::models::{
    AccountModelAvailability, AccountModelAvailabilityState, AnthropicModelCatalog,
    DEFAULT_CONTEXT_LIMIT, ModelCapabilities, OpenAIModelCatalog,
    begin_anthropic_model_catalog_refresh, begin_openai_model_catalog_refresh,
    cached_anthropic_model_ids, cached_openai_model_ids,
    clear_all_model_unavailability_for_account, clear_all_provider_unavailability_for_account,
    clear_model_unavailable_for_account, clear_provider_unavailable_for_account,
    context_limit_for_model, context_limit_for_model_with_provider, fetch_anthropic_model_catalog,
    fetch_anthropic_model_catalog_oauth, fetch_openai_context_limits, fetch_openai_model_catalog,
    finish_anthropic_model_catalog_refresh_for_scope, finish_openai_model_catalog_refresh,
    format_account_model_availability_detail, get_best_available_openai_model,
    is_model_available_for_account, known_anthropic_model_ids, known_openai_model_ids,
    model_availability_for_account, model_unavailability_detail_for_account,
    note_openai_model_catalog_refresh_attempt, persist_anthropic_model_catalog,
    persist_openai_model_catalog, populate_account_models, populate_anthropic_models,
    populate_context_limits, provider_for_model, provider_for_model_with_hint,
    provider_unavailability_detail_for_account, record_model_unavailable_for_account,
    record_provider_unavailable_for_account, refresh_openai_model_catalog_in_background,
    resolve_model_capabilities, should_refresh_anthropic_model_catalog,
    should_refresh_openai_model_catalog,
};
use self::pricing::cheapness_for_route;
use self::selection::{ActiveProvider, ProviderAvailability};

/// MultiProvider wraps multiple providers and allows seamless model switching
pub struct MultiProvider {
    /// Claude Code CLI provider
    claude: RwLock<Option<Arc<claude::ClaudeProvider>>>,
    /// Direct Anthropic API provider (no Python dependency)
    anthropic: RwLock<Option<Arc<anthropic::AnthropicProvider>>>,
    openai: RwLock<Option<Arc<openai::OpenAIProvider>>>,
    /// GitHub Copilot API provider (direct API, hot-swappable after login)
    copilot_api: RwLock<Option<Arc<copilot::CopilotApiProvider>>>,
    /// Antigravity provider (CLI-backed, hot-swappable after login)
    antigravity: RwLock<Option<Arc<antigravity::AntigravityCliProvider>>>,
    /// Gemini provider (hot-swappable after login)
    gemini: RwLock<Option<Arc<gemini::GeminiProvider>>>,
    /// Cursor provider (native/direct API, hot-swappable after login)
    cursor: RwLock<Option<Arc<cursor::CursorCliProvider>>>,
    /// OpenRouter API provider (200+ models from various providers, hot-swappable after login)
    openrouter: RwLock<Option<Arc<openrouter::OpenRouterProvider>>>,
    active: RwLock<ActiveProvider>,
    /// Use Claude CLI instead of direct API (legacy mode)
    use_claude_cli: bool,
    /// Notifications generated during provider/account auto-selection.
    /// The TUI should drain and display these on session start.
    startup_notices: RwLock<Vec<String>>,
    /// Optional explicit provider lock set by CLI `--provider`.
    /// When present, cross-provider fallback is disabled.
    forced_provider: Option<ActiveProvider>,
}

impl MultiProvider {
    #[cfg(test)]
    fn same_provider_account_candidates(provider: ActiveProvider) -> Vec<String> {
        account_failover::same_provider_account_candidates(provider)
    }

    async fn complete_with_failover(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        mode: CompletionMode<'_>,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        self.spawn_anthropic_catalog_refresh_if_needed();
        self.spawn_openai_catalog_refresh_if_needed();

        let detected_active = self.active_provider();
        let active = if let Some(forced) = self.forced_provider {
            if detected_active != forced {
                crate::logging::warn(&format!(
                    "Provider lock corrected active provider from {} to {} before request",
                    Self::provider_label(detected_active),
                    Self::provider_label(forced),
                ));
                self.set_active_provider(forced);
            }
            forced
        } else {
            detected_active
        };
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
                        self.startup_notices
                            .write()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push(format!(
                                "⚡ Auto-fallback: {} unavailable, switched to {}",
                                from_label, to_label
                            ));
                    }
                    return Ok(stream);
                }
                Err(err) => {
                    let summary =
                        maybe_annotate_limit_summary(candidate, Self::summarize_error(&err));
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
                        if candidate == active
                            && let Some(stream) = self
                                .try_same_provider_account_failover(
                                    candidate, messages, tools, mode, &summary, &mut notes,
                                )
                                .await?
                        {
                            return Ok(stream);
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

    fn provider_from_model_key(key: &str) -> Option<ActiveProvider> {
        match key {
            "claude" => Some(ActiveProvider::Claude),
            "openai" => Some(ActiveProvider::OpenAI),
            "copilot" => Some(ActiveProvider::Copilot),
            "antigravity" => Some(ActiveProvider::Antigravity),
            "gemini" => Some(ActiveProvider::Gemini),
            "cursor" => Some(ActiveProvider::Cursor),
            "openrouter" => Some(ActiveProvider::OpenRouter),
            _ => None,
        }
    }

    fn explicit_model_provider_prefix(model: &str) -> Option<(ActiveProvider, &'static str, &str)> {
        if let Some(rest) = model.strip_prefix("copilot:") {
            Some((ActiveProvider::Copilot, "copilot:", rest))
        } else if let Some(rest) = model.strip_prefix("antigravity:") {
            Some((ActiveProvider::Antigravity, "antigravity:", rest))
        } else if let Some(rest) = model.strip_prefix("cursor:") {
            Some((ActiveProvider::Cursor, "cursor:", rest))
        } else {
            None
        }
    }

    fn ensure_provider_lock_allows_model_target(
        &self,
        target: ActiveProvider,
        requested_model: &str,
    ) -> Result<()> {
        let Some(forced) = self.forced_provider else {
            return Ok(());
        };
        if forced == target {
            return Ok(());
        }
        anyhow::bail!(
            "Model '{}' targets {} but --provider is locked to {}. Remove the provider-specific model prefix or use `--provider {}`.",
            requested_model,
            Self::provider_label(target),
            Self::provider_label(forced),
            Self::provider_key(target),
        );
    }

    fn model_name_for_provider<'a>(provider: ActiveProvider, model: &'a str) -> Cow<'a, str> {
        if matches!(provider, ActiveProvider::Claude)
            && let Some(canonical) = normalize_copilot_model_name(model)
        {
            return Cow::Borrowed(canonical);
        }
        Cow::Borrowed(model)
    }

    fn set_model_on_provider(&self, provider: ActiveProvider, model: &str) -> Result<()> {
        let model = model.trim();
        if model.is_empty() {
            anyhow::bail!("Model cannot be empty");
        }

        match provider {
            ActiveProvider::Claude => {
                let model = Self::model_name_for_provider(provider, model);
                if let Some(anthropic) = self.anthropic_provider() {
                    anthropic.set_model(&model)?;
                } else if let Some(claude) = self.claude_provider() {
                    claude.set_model(&model)?;
                } else {
                    anyhow::bail!(
                        "Claude credentials not available. Run `jcode login --provider claude` first."
                    );
                }
                self.set_active_provider(ActiveProvider::Claude);
                Ok(())
            }
            ActiveProvider::OpenAI => {
                let Some(openai) = self.openai_provider() else {
                    anyhow::bail!(
                        "OpenAI credentials not available. Run `jcode login --provider openai` first."
                    );
                };
                openai.set_model(model)?;
                self.set_active_provider(ActiveProvider::OpenAI);
                Ok(())
            }
            ActiveProvider::Copilot => {
                let Some(copilot) = self.copilot_provider() else {
                    anyhow::bail!(
                        "GitHub Copilot credentials not available. Run `jcode login --provider copilot` first."
                    );
                };
                copilot.set_model(model)?;
                self.set_active_provider(ActiveProvider::Copilot);
                Ok(())
            }
            ActiveProvider::Antigravity => {
                let Some(antigravity) = self.antigravity_provider() else {
                    anyhow::bail!(
                        "Antigravity credentials not available. Run `jcode login --provider antigravity` first."
                    );
                };
                antigravity.set_model(model)?;
                self.set_active_provider(ActiveProvider::Antigravity);
                Ok(())
            }
            ActiveProvider::Gemini => {
                let Some(gemini) = self.gemini_provider() else {
                    anyhow::bail!(
                        "Gemini credentials not available. Run `jcode login --provider gemini` first."
                    );
                };
                gemini.set_model(model)?;
                self.set_active_provider(ActiveProvider::Gemini);
                Ok(())
            }
            ActiveProvider::Cursor => {
                let Some(cursor) = self.cursor_provider() else {
                    anyhow::bail!(
                        "Cursor credentials not available. Run `jcode login --provider cursor` first."
                    );
                };
                cursor.set_model(model)?;
                self.set_active_provider(ActiveProvider::Cursor);
                Ok(())
            }
            ActiveProvider::OpenRouter => {
                let Some(openrouter) = self.openrouter_provider() else {
                    anyhow::bail!(
                        "OpenRouter/OpenAI-compatible credentials not available. Set the configured API key or run `jcode login --provider openrouter` first."
                    );
                };
                openrouter.set_model(model)?;
                self.set_active_provider(ActiveProvider::OpenRouter);
                Ok(())
            }
        }
    }
}

impl Default for MultiProvider {
    fn default() -> Self {
        Self::new()
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
            ActiveProvider::Antigravity => "Antigravity",
            ActiveProvider::Gemini => "Gemini",
            ActiveProvider::Cursor => "Cursor",
            ActiveProvider::OpenRouter => "OpenRouter",
        }
    }

    fn model(&self) -> String {
        match self.active_provider() {
            ActiveProvider::Claude => {
                // Prefer anthropic if available
                if let Some(anthropic) = self.anthropic_provider() {
                    anthropic.model()
                } else if let Some(claude) = self.claude_provider() {
                    claude.model()
                } else {
                    "claude-opus-4-5-20251101".to_string()
                }
            }
            ActiveProvider::OpenAI => self
                .openai_provider()
                .map(|o| o.model())
                .unwrap_or_else(|| "gpt-5.5".to_string()),
            ActiveProvider::Copilot => self
                .copilot_provider()
                .map(|o| o.model())
                .unwrap_or_else(|| "claude-sonnet-4".to_string()),
            ActiveProvider::Antigravity => self
                .antigravity_provider()
                .map(|o| o.model())
                .unwrap_or_else(|| "default".to_string()),
            ActiveProvider::Gemini => self
                .gemini_provider()
                .map(|o| o.model())
                .unwrap_or_else(|| "gemini-2.5-pro".to_string()),
            ActiveProvider::Cursor => self
                .cursor_provider()
                .map(|o| o.model())
                .unwrap_or_else(|| "composer-1.5".to_string()),
            ActiveProvider::OpenRouter => self
                .openrouter_provider()
                .map(|o| o.model())
                .unwrap_or_else(|| "anthropic/claude-sonnet-4".to_string()),
        }
    }

    fn set_model(&self, model: &str) -> Result<()> {
        self.spawn_anthropic_catalog_refresh_if_needed();
        self.spawn_openai_catalog_refresh_if_needed();

        let requested_model = model.trim();
        if requested_model.is_empty() {
            anyhow::bail!("Model cannot be empty");
        }

        // Provider-prefixed model names are explicit routing directives. They
        // must never silently fall through to another provider when the target
        // is unavailable or when --provider locks a different backend.
        if let Some((target, _prefix, target_model)) =
            Self::explicit_model_provider_prefix(requested_model)
        {
            self.ensure_provider_lock_allows_model_target(target, requested_model)?;
            return self.set_model_on_provider(target, target_model);
        }

        // A CLI --provider lock means the model string is provider-local. Do
        // not apply global Claude/OpenAI/OpenRouter heuristics here: custom
        // OpenAI-compatible endpoints often use model IDs that look like other
        // providers' IDs, and GitHub Copilot uses Claude-looking dotted names.
        if let Some(forced) = self.forced_provider {
            return self.set_model_on_provider(forced, requested_model);
        }

        // Normalize Copilot-style model names (dots -> hyphens) to canonical form.
        // e.g. "claude-opus-4.6" -> "claude-opus-4-6" so Anthropic accepts it.
        let model = if let Some(canonical) = normalize_copilot_model_name(requested_model) {
            canonical
        } else {
            requested_model
        };

        if let Some((base_model, provider_pin)) = model.rsplit_once('@')
            && !provider_pin.trim().is_empty()
            && let Some(openrouter_model) = openrouter_catalog_model_id(base_model)
        {
            return self.set_model_on_provider(
                ActiveProvider::OpenRouter,
                &format!("{}@{}", openrouter_model, provider_pin),
            );
        }

        // Detect which provider this model belongs to when no explicit
        // --provider lock was requested.
        let target_provider = provider_for_model(model);
        if let Some(target_provider) = target_provider
            && let Some(target) = Self::provider_from_model_key(target_provider)
        {
            self.set_model_on_provider(target, model)
        } else {
            // Unknown model - try current provider.
            self.set_model_on_provider(self.active_provider(), model)
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
                if let Some(anthropic) = self.anthropic_provider() {
                    anthropic.available_models_for_switching()
                } else if let Some(claude) = self.claude_provider() {
                    claude.available_models_for_switching()
                } else {
                    Vec::new()
                }
            }
            ActiveProvider::OpenAI => self
                .openai_provider()
                .map(|openai| openai.available_models_for_switching())
                .unwrap_or_default(),
            ActiveProvider::Copilot => self
                .copilot_provider()
                .map(|copilot| copilot.available_models_for_switching())
                .unwrap_or_default(),
            ActiveProvider::Antigravity => self
                .antigravity_provider()
                .map(|antigravity| antigravity.available_models_for_switching())
                .unwrap_or_default(),
            ActiveProvider::Gemini => self
                .gemini_provider()
                .map(|gemini| gemini.available_models_for_switching())
                .unwrap_or_default(),
            ActiveProvider::Cursor => self
                .cursor_provider()
                .map(|cursor| cursor.available_models_for_switching())
                .unwrap_or_default(),
            ActiveProvider::OpenRouter => self
                .openrouter_provider()
                .map(|openrouter| openrouter.available_models_for_switching())
                .unwrap_or_default(),
        }
    }

    fn available_models_display(&self) -> Vec<String> {
        listable_model_names_from_routes(&self.model_routes())
    }

    fn available_providers_for_model(&self, model: &str) -> Vec<String> {
        if let Some(model) = openrouter_catalog_model_id(model)
            && let Some(openrouter) = self.openrouter_provider()
        {
            return openrouter.available_providers_for_model(&model);
        }
        Vec::new()
    }

    fn provider_details_for_model(&self, model: &str) -> Vec<(String, String)> {
        if let Some(model) = openrouter_catalog_model_id(model)
            && let Some(openrouter) = self.openrouter_provider()
        {
            return openrouter.provider_details_for_model(&model);
        }
        Vec::new()
    }

    fn preferred_provider(&self) -> Option<String> {
        if let Some(openrouter) = self.openrouter_provider()
            && matches!(
                *self
                    .active
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
                ActiveProvider::OpenRouter
            )
        {
            return openrouter.preferred_provider();
        }
        None
    }

    fn model_routes(&self) -> Vec<ModelRoute> {
        let routes_started = std::time::Instant::now();
        self.spawn_anthropic_catalog_refresh_if_needed();
        self.spawn_openai_catalog_refresh_if_needed();

        let mut routes = Vec::new();
        let mut openrouter_models = 0usize;
        let mut openrouter_endpoint_cache_hits = 0usize;
        let mut openrouter_endpoint_routes = 0usize;
        let mut openrouter_scheduled_endpoint_refreshes = 0usize;
        let has_oauth = self.has_claude_runtime();
        let has_api_key = std::env::var("ANTHROPIC_API_KEY").is_ok();
        let anthropic_models = if let Some(anthropic) = self.anthropic_provider() {
            anthropic.available_models_for_switching()
        } else if let Some(claude) = self.claude_provider() {
            claude.available_models_for_switching()
        } else {
            known_anthropic_model_ids()
        };
        let openai_models = if let Some(openai) = self.openai_provider() {
            openai.available_models_for_switching()
        } else {
            known_openai_model_ids()
        };

        // Anthropic models (oauth and/or api-key)
        for model in anthropic_models {
            let (available, detail) = if has_oauth && !has_api_key {
                anthropic_oauth_route_availability(&model)
            } else {
                (true, String::new())
            };

            if has_oauth {
                routes.push(build_anthropic_oauth_route(
                    &model,
                    available,
                    detail.clone(),
                ));
            }
            if has_api_key {
                let (ak_available, ak_detail) = anthropic_api_key_route_availability(&model);
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
        for model in openai_models {
            let availability = model_availability_for_account(&model);
            let (available, detail) = if self.openai_provider().is_none() {
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
            routes.push(build_openai_oauth_route(&model, available, detail));
        }

        // GitHub Copilot models
        {
            if let Some(copilot) = self.copilot_provider() {
                let copilot_models = copilot.available_models_display();
                let detail = copilot.model_catalog_detail();
                let copilot_models_empty = copilot_models.is_empty();
                for model in copilot_models {
                    routes.push(build_copilot_route(&model, true, detail.clone()));
                }
                if copilot_models_empty && copilot::CopilotApiProvider::has_credentials() {
                    routes.push(build_copilot_route("copilot models", false, detail));
                }
            } else if copilot::CopilotApiProvider::has_credentials() {
                routes.push(build_copilot_route(
                    "copilot models",
                    false,
                    "not initialized yet",
                ));
            }
        }

        // Gemini models
        {
            if let Some(gemini) = self.gemini_provider() {
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

        // Antigravity models
        {
            if let Some(antigravity) = self.antigravity_provider() {
                routes.extend(antigravity.model_routes());
            }
        }

        // Cursor models
        {
            if let Some(cursor) = self.cursor_provider() {
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
        let has_openrouter = self.openrouter_provider().is_some();
        if let Some(openrouter) = self.openrouter_provider() {
            let current_openrouter_model = openrouter.model();
            let supports_openrouter_provider_features =
                openrouter.supports_provider_routing_features();
            let mut scheduled_endpoint_refreshes = 0usize;
            for model in openrouter.available_models_display() {
                openrouter_models += 1;
                let cached = openrouter::load_endpoints_disk_cache_public(&model);
                let cache_age = cached.as_ref().map(|(_, age)| *age);
                if (model == current_openrouter_model || scheduled_endpoint_refreshes < 8)
                    && openrouter.maybe_schedule_endpoint_refresh_for_display(
                        &model,
                        cache_age,
                        "model picker route hydration",
                    )
                {
                    scheduled_endpoint_refreshes += 1;
                    openrouter_scheduled_endpoint_refreshes += 1;
                }
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
                if supports_openrouter_provider_features {
                    routes.push(build_openrouter_auto_route(
                        &model,
                        has_openrouter,
                        auto_detail,
                    ));
                } else {
                    routes.push(ModelRoute {
                        model: model.clone(),
                        provider: "OpenAI-compatible".to_string(),
                        api_method: "openai-compatible".to_string(),
                        available: has_openrouter,
                        detail: "custom endpoint".to_string(),
                        cheapness: None,
                    });
                }
                // Add per-provider routes from endpoints cache
                if let Some((ref endpoints, _)) = cached {
                    openrouter_endpoint_cache_hits += 1;
                    let stale_suffix = age_str.as_deref().unwrap_or("");
                    for ep in endpoints {
                        openrouter_endpoint_routes += 1;
                        routes.push(build_openrouter_endpoint_route(
                            &model,
                            ep,
                            has_openrouter,
                            Some(stale_suffix),
                        ));
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
                    openrouter_endpoint_cache_hits += 1;
                    for ep in &endpoints {
                        openrouter_endpoint_routes += 1;
                        routes.push(build_openrouter_endpoint_route(&model, ep, true, None));
                    }
                } else {
                    routes.push(build_openrouter_fallback_provider_route(
                        &model,
                        &or_model,
                        "Anthropic",
                    ));
                }
            }

            for model in ALL_OPENAI_MODELS {
                let or_model = format!("openai/{}", model);
                if let Some((endpoints, _)) =
                    openrouter::load_endpoints_disk_cache_public(&or_model)
                {
                    openrouter_endpoint_cache_hits += 1;
                    for ep in &endpoints {
                        openrouter_endpoint_routes += 1;
                        routes.push(build_openrouter_endpoint_route(model, ep, true, None));
                    }
                } else {
                    routes.push(build_openrouter_fallback_provider_route(
                        model, &or_model, "OpenAI",
                    ));
                }
            }
        }

        let total_ms = routes_started.elapsed().as_millis();
        if total_ms >= 250 || std::env::var("JCODE_LOG_MODEL_PICKER_TIMING").is_ok() {
            crate::logging::info(&format!(
                "[TIMING] model_routes: routes={}, openrouter_configured={}, openrouter_models={}, openrouter_endpoint_cache_hits={}, openrouter_endpoint_routes={}, openrouter_scheduled_endpoint_refreshes={}, total={}ms",
                routes.len(),
                has_openrouter,
                openrouter_models,
                openrouter_endpoint_cache_hits,
                openrouter_endpoint_routes,
                openrouter_scheduled_endpoint_refreshes,
                total_ms,
            ));
        }

        routes
    }

    async fn prefetch_models(&self) -> Result<()> {
        if let Some(anthropic) = self.anthropic_provider() {
            anthropic.prefetch_models().await?;
        }
        if let Some(claude) = self.claude_provider() {
            claude.prefetch_models().await?;
        }
        if let Some(openai) = self.openai_provider() {
            openai.prefetch_models().await?;
        }
        let openrouter = self.openrouter_provider();
        if let Some(openrouter) = openrouter {
            openrouter.prefetch_models().await?;
        }
        {
            let copilot = self
                .copilot_api
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            if let Some(copilot) = copilot {
                copilot.prefetch_models().await?;
            }
        }
        {
            let antigravity = self.antigravity_provider();
            if let Some(antigravity) = antigravity {
                antigravity.prefetch_models().await?;
            }
        }
        {
            let gemini = self.gemini_provider();
            if let Some(gemini) = gemini {
                gemini.prefetch_models().await?;
            }
        }
        {
            let cursor = self.cursor_provider();
            if let Some(cursor) = cursor {
                cursor.prefetch_models().await?;
            }
        }
        Ok(())
    }

    fn on_auth_changed(&self) {
        // Auth just changed, so discard any stale full/fast snapshots before
        // using cheap local probes to hot-initialize newly configured providers.
        crate::auth::AuthStatus::invalidate_cache();

        if self.use_claude_cli {
            if self.claude_provider().is_none() && crate::auth::claude::load_credentials().is_ok() {
                crate::logging::info("Hot-initialized Claude CLI provider after auth change");
                *self
                    .claude
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                    Some(Arc::new(claude::ClaudeProvider::new()));
            }
        } else if self.anthropic_provider().is_none()
            && crate::auth::claude::load_credentials().is_ok()
        {
            crate::logging::info("Hot-initialized Anthropic provider after auth change");
            *self
                .anthropic
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some(Arc::new(anthropic::AnthropicProvider::new()));
        }

        if let Some(openai) = self.openai_provider() {
            openai.reload_credentials_now();
        } else if let Ok(credentials) = crate::auth::codex::load_credentials() {
            crate::logging::info("Hot-initialized OpenAI provider after auth change");
            *self
                .openai
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some(Arc::new(openai::OpenAIProvider::new(credentials)));
        }

        if openrouter::OpenRouterProvider::has_credentials() {
            match openrouter::OpenRouterProvider::new() {
                Ok(provider) => {
                    crate::logging::info(
                        "Hot-initialized OpenRouter/OpenAI-compatible provider after auth change",
                    );
                    *self
                        .openrouter
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                        Some(Arc::new(provider));
                }
                Err(e) => {
                    crate::logging::info(&format!(
                        "Failed to hot-initialize OpenRouter/OpenAI-compatible provider after auth change: {}",
                        e
                    ));
                }
            }
        }

        let already_has = self.copilot_provider().is_some();
        if !already_has {
            let status = crate::auth::AuthStatus::check_fast();
            if status.copilot_has_api_token {
                match copilot::CopilotApiProvider::new() {
                    Ok(p) => {
                        crate::logging::info("Hot-initialized Copilot API provider after login");
                        let provider = Arc::new(p);
                        let p_clone = provider.clone();
                        tokio::spawn(async move {
                            p_clone.detect_tier_and_set_default().await;
                        });
                        *self
                            .copilot_api
                            .write()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(provider);
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

        let already_has_antigravity = self.antigravity_provider().is_some();
        if !already_has_antigravity && crate::auth::antigravity::load_tokens().is_ok() {
            crate::logging::info("Hot-initialized Antigravity provider after login");
            *self
                .antigravity
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some(Arc::new(antigravity::AntigravityCliProvider::new()));
        }

        let already_has_gemini = self.gemini_provider().is_some();
        if !already_has_gemini && crate::auth::gemini::load_tokens().is_ok() {
            crate::logging::info("Hot-initialized Gemini provider after login");
            *self
                .gemini
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some(Arc::new(gemini::GeminiProvider::new()));
        }

        let already_has_cursor = self.cursor_provider().is_some();
        if !already_has_cursor
            && matches!(
                crate::auth::AuthStatus::check_fast().cursor,
                crate::auth::AuthState::Available
            )
        {
            crate::logging::info("Hot-initialized Cursor provider after login");
            *self
                .cursor
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some(Arc::new(cursor::CursorCliProvider::new()));
        }
        if let Some(anthropic) = self.anthropic_provider() {
            Self::spawn_post_auth_model_refresh(anthropic, "Anthropic");
        }
        if let Some(claude) = self.claude_provider() {
            Self::spawn_post_auth_model_refresh(claude, "Claude");
        }
        if let Some(openai) = self.openai_provider() {
            Self::spawn_post_auth_model_refresh(openai, "OpenAI");
        }
        if let Some(antigravity) = self.antigravity_provider() {
            Self::spawn_post_auth_model_refresh(antigravity, "Antigravity");
        }
        if let Some(gemini) = self.gemini_provider() {
            Self::spawn_post_auth_model_refresh(gemini, "Gemini");
        }
        if let Some(cursor) = self.cursor_provider() {
            Self::spawn_post_auth_model_refresh(cursor, "Cursor");
        }
        if let Some(openrouter) = self.openrouter_provider() {
            Self::spawn_post_auth_model_refresh(openrouter, "OpenRouter");
        }
    }

    async fn invalidate_credentials(&self) {
        if let Some(anthropic) = self.anthropic_provider() {
            anthropic.invalidate_credentials().await;
        }
        if let Some(openai) = self.openai_provider() {
            openai.invalidate_credentials().await;
        }
    }

    fn handles_tools_internally(&self) -> bool {
        match self.active_provider() {
            ActiveProvider::Claude => {
                // Direct API does NOT handle tools internally - jcode executes them
                if self.anthropic_provider().is_some() {
                    false
                } else {
                    self.claude_provider()
                        .map(|c| c.handles_tools_internally())
                        .unwrap_or(false)
                }
            }
            ActiveProvider::OpenAI => self
                .openai_provider()
                .map(|o| o.handles_tools_internally())
                .unwrap_or(false),
            ActiveProvider::Copilot => self
                .copilot_provider()
                .map(|o| o.handles_tools_internally())
                .unwrap_or(false),
            ActiveProvider::Antigravity => false,
            ActiveProvider::Gemini => false,
            ActiveProvider::Cursor => self
                .cursor_provider()
                .map(|o| o.handles_tools_internally())
                .unwrap_or(false),
            ActiveProvider::OpenRouter => false, // jcode executes tools
        }
    }

    fn reasoning_effort(&self) -> Option<String> {
        match self.active_provider() {
            ActiveProvider::Claude => None,
            ActiveProvider::OpenAI => self.openai_provider().and_then(|o| o.reasoning_effort()),
            ActiveProvider::Copilot => None,
            ActiveProvider::Antigravity => None,
            ActiveProvider::Gemini => None,
            ActiveProvider::Cursor => None,
            ActiveProvider::OpenRouter => None,
        }
    }

    fn set_reasoning_effort(&self, effort: &str) -> Result<()> {
        match self.active_provider() {
            ActiveProvider::OpenAI => self
                .openai_provider()
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
                .openai_provider()
                .map(|o| o.available_efforts())
                .unwrap_or_default(),
            ActiveProvider::Copilot => vec![],
            ActiveProvider::Antigravity => vec![],
            ActiveProvider::Gemini => vec![],
            ActiveProvider::Cursor => vec![],
            _ => vec![],
        }
    }

    fn service_tier(&self) -> Option<String> {
        match self.active_provider() {
            ActiveProvider::OpenAI => self.openai_provider().and_then(|o| o.service_tier()),
            _ => None,
        }
    }

    fn set_service_tier(&self, service_tier: &str) -> Result<()> {
        match self.active_provider() {
            ActiveProvider::OpenAI => self
                .openai_provider()
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
                .openai_provider()
                .map(|o| o.available_service_tiers())
                .unwrap_or_default(),
            _ => vec![],
        }
    }

    fn native_compaction_mode(&self) -> Option<String> {
        match self.active_provider() {
            ActiveProvider::OpenAI => self
                .openai_provider()
                .and_then(|o| o.native_compaction_mode()),
            _ => None,
        }
    }

    fn native_compaction_threshold_tokens(&self) -> Option<usize> {
        match self.active_provider() {
            ActiveProvider::OpenAI => self
                .openai_provider()
                .and_then(|o| o.native_compaction_threshold_tokens()),
            _ => None,
        }
    }

    fn transport(&self) -> Option<String> {
        match self.active_provider() {
            ActiveProvider::OpenAI => self.openai_provider().and_then(|o| o.transport()),
            _ => None,
        }
    }

    fn set_transport(&self, transport: &str) -> Result<()> {
        match self.active_provider() {
            ActiveProvider::OpenAI => self
                .openai_provider()
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
                .openai_provider()
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
                if self.anthropic_provider().is_some() {
                    true
                } else {
                    self.claude_provider()
                        .map(|c| c.supports_compaction())
                        .unwrap_or(false)
                }
            }
            ActiveProvider::OpenAI => self
                .openai_provider()
                .map(|o| o.supports_compaction())
                .unwrap_or(false),
            ActiveProvider::Copilot => self
                .copilot_provider()
                .map(|o| o.supports_compaction())
                .unwrap_or(false),
            ActiveProvider::Antigravity => self
                .antigravity_provider()
                .map(|o| o.supports_compaction())
                .unwrap_or(false),
            ActiveProvider::Gemini => self
                .gemini_provider()
                .map(|o| o.supports_compaction())
                .unwrap_or(false),
            ActiveProvider::Cursor => self
                .cursor_provider()
                .map(|o| o.supports_compaction())
                .unwrap_or(false),
            ActiveProvider::OpenRouter => self
                .openrouter_provider()
                .map(|o| o.supports_compaction())
                .unwrap_or(false),
        }
    }

    fn uses_jcode_compaction(&self) -> bool {
        match self.active_provider() {
            ActiveProvider::Claude => {
                if self.anthropic_provider().is_some() {
                    true
                } else {
                    self.claude_provider()
                        .map(|c| c.uses_jcode_compaction())
                        .unwrap_or(false)
                }
            }
            ActiveProvider::OpenAI => self
                .openai_provider()
                .map(|o| o.uses_jcode_compaction())
                .unwrap_or(false),
            ActiveProvider::Copilot => self
                .copilot_provider()
                .map(|o| o.uses_jcode_compaction())
                .unwrap_or(false),
            ActiveProvider::Antigravity => self
                .antigravity_provider()
                .map(|o| o.uses_jcode_compaction())
                .unwrap_or(false),
            ActiveProvider::Gemini => self
                .gemini_provider()
                .map(|o| o.uses_jcode_compaction())
                .unwrap_or(false),
            ActiveProvider::Cursor => self
                .cursor_provider()
                .map(|o| o.uses_jcode_compaction())
                .unwrap_or(false),
            ActiveProvider::OpenRouter => self
                .openrouter_provider()
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
                if let Some(anthropic) = self.anthropic_provider() {
                    anthropic
                        .native_compact(
                            messages,
                            existing_summary_text,
                            existing_openai_encrypted_content,
                        )
                        .await
                } else if let Some(claude) = self.claude_provider() {
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
                if let Some(openai) = self.openai_provider() {
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
                let provider = self.copilot_provider();
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
            ActiveProvider::Antigravity => Err(anyhow::anyhow!(
                "Antigravity does not support native compaction"
            )),
            ActiveProvider::Gemini => {
                let provider = self.gemini_provider();
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
                let provider = self.cursor_provider();
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
                let provider = self.openrouter_provider();
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
        if let Some(copilot) = self.copilot_provider() {
            copilot.set_premium_mode(mode);
        }
    }

    fn premium_mode(&self) -> copilot::PremiumMode {
        if let Some(copilot) = self.copilot_provider() {
            copilot.get_premium_mode()
        } else {
            copilot::PremiumMode::Normal
        }
    }

    fn drain_startup_notices(&self) -> Vec<String> {
        std::mem::take(
            &mut *self
                .startup_notices
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }

    fn context_window(&self) -> usize {
        match self.active_provider() {
            ActiveProvider::Claude => {
                if let Some(anthropic) = self.anthropic_provider() {
                    anthropic.context_window()
                } else if let Some(claude) = self.claude_provider() {
                    claude.context_window()
                } else {
                    DEFAULT_CONTEXT_LIMIT
                }
            }
            ActiveProvider::OpenAI => self
                .openai_provider()
                .map(|o| o.context_window())
                .unwrap_or(DEFAULT_CONTEXT_LIMIT),
            ActiveProvider::Copilot => self
                .copilot_provider()
                .map(|o| o.context_window())
                .unwrap_or(DEFAULT_CONTEXT_LIMIT),
            ActiveProvider::Antigravity => self
                .antigravity_provider()
                .map(|o| o.context_window())
                .unwrap_or(DEFAULT_CONTEXT_LIMIT),
            ActiveProvider::Gemini => self
                .gemini_provider()
                .map(|o| o.context_window())
                .unwrap_or(DEFAULT_CONTEXT_LIMIT),
            ActiveProvider::Cursor => self
                .cursor_provider()
                .map(|o| o.context_window())
                .unwrap_or(DEFAULT_CONTEXT_LIMIT),
            ActiveProvider::OpenRouter => self
                .openrouter_provider()
                .map(|o| o.context_window())
                .unwrap_or(DEFAULT_CONTEXT_LIMIT),
        }
    }

    fn fork(&self) -> Arc<dyn Provider> {
        let current_model = self.model();
        let active = self.active_provider();

        let claude = if matches!(active, ActiveProvider::Claude) && self.claude_provider().is_some()
        {
            Some(Arc::new(claude::ClaudeProvider::new()))
        } else {
            None
        };
        let anthropic = if self.anthropic_provider().is_some() {
            Some(Arc::new(anthropic::AnthropicProvider::new()))
        } else {
            None
        };
        let openai = if self.openai_provider().is_some() {
            auth::codex::load_credentials()
                .ok()
                .map(openai::OpenAIProvider::new)
                .map(Arc::new)
        } else {
            None
        };
        let copilot_api = self
            .copilot_api
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let antigravity_provider = self
            .antigravity
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let gemini_provider = self
            .gemini
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let cursor_provider = if self
            .cursor
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
        {
            Some(Arc::new(cursor::CursorCliProvider::new()))
        } else {
            None
        };
        let openrouter = if self
            .openrouter
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
        {
            openrouter::OpenRouterProvider::new().ok().map(Arc::new)
        } else {
            None
        };

        let provider = Self {
            claude: RwLock::new(claude),
            anthropic: RwLock::new(anthropic),
            openai: RwLock::new(openai),
            copilot_api: RwLock::new(copilot_api),
            antigravity: RwLock::new(antigravity_provider),
            gemini: RwLock::new(gemini_provider),
            cursor: RwLock::new(cursor_provider),
            openrouter: RwLock::new(openrouter),
            active: RwLock::new(active),
            use_claude_cli: self.use_claude_cli,
            startup_notices: RwLock::new(Vec::new()),
            forced_provider: self.forced_provider,
        };

        provider.spawn_anthropic_catalog_refresh_if_needed();
        provider.spawn_openai_catalog_refresh_if_needed();
        if matches!(active, ActiveProvider::Copilot) {
            let _ = provider.set_model(&format!("copilot:{}", current_model));
        } else if matches!(active, ActiveProvider::Antigravity) {
            let _ = provider.set_model(&format!("antigravity:{}", current_model));
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
                if self.anthropic_provider().is_some() {
                    None
                } else {
                    self.claude_provider()
                        .and_then(|c| c.native_result_sender())
                }
            }
            ActiveProvider::OpenAI => None,
            ActiveProvider::Copilot => None,
            ActiveProvider::Antigravity => None,
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
mod tests;
