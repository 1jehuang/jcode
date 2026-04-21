mod accessors;
mod account_failover;
pub mod anthropic;
pub mod antigravity;
pub mod claude;
pub mod cli_common;
pub mod copilot;
pub mod cursor;
mod dispatch;
mod failover;
pub mod gemini;
pub mod jcode;
pub mod models;
pub mod openai;
pub(crate) mod openai_request;
pub mod openrouter;
pub mod pricing;
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
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
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

fn should_eager_detect_copilot_tier() -> bool {
    std::env::var("JCODE_NON_INTERACTIVE").is_err()
}

pub(crate) fn anthropic_oauth_route_availability(model: &str) -> (bool, String) {
    if model.ends_with("[1m]") && !crate::usage::has_extra_usage() {
        (false, "requires extra usage".to_string())
    } else if model.contains("opus") && !crate::auth::claude::is_max_subscription() {
        (false, "requires Max subscription".to_string())
    } else {
        (true, String::new())
    }
}

pub(crate) fn anthropic_api_key_route_availability(model: &str) -> (bool, String) {
    if model.ends_with("[1m]") && !crate::usage::has_extra_usage() {
        (false, "requires extra usage".to_string())
    } else {
        (true, String::new())
    }
}

/// Stream of events from a provider
pub type EventStream = Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelCatalogRefreshSummary {
    pub model_count_before: usize,
    pub model_count_after: usize,
    pub models_added: usize,
    pub models_removed: usize,
    pub route_count_before: usize,
    pub route_count_after: usize,
    pub routes_added: usize,
    pub routes_removed: usize,
    pub routes_changed: usize,
}

pub fn summarize_model_catalog_refresh(
    before_models: Vec<String>,
    after_models: Vec<String>,
    before_routes: Vec<ModelRoute>,
    after_routes: Vec<ModelRoute>,
) -> ModelCatalogRefreshSummary {
    fn is_display_only_age_suffix(detail: &str) -> bool {
        let detail = detail.trim();
        ["m ago", "h ago", "d ago"]
            .iter()
            .find_map(|suffix| detail.strip_suffix(suffix))
            .is_some_and(|prefix| !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_digit()))
    }

    fn normalize_route_refresh_detail(detail: &str) -> String {
        let detail = detail.trim();
        if detail.is_empty() {
            return String::new();
        }
        if is_display_only_age_suffix(detail) {
            return String::new();
        }
        if let Some((prefix, suffix)) = detail.rsplit_once(',')
            && is_display_only_age_suffix(suffix)
        {
            return prefix.trim_end().trim_end_matches(',').trim().to_string();
        }
        detail.to_string()
    }

    let before_model_set: BTreeSet<String> = before_models.into_iter().collect();
    let after_model_set: BTreeSet<String> = after_models.into_iter().collect();

    let before_route_map: BTreeMap<(String, String, String), (bool, String, Option<u64>)> =
        before_routes
            .into_iter()
            .map(|route| {
                let estimated_cost = route.estimated_reference_cost_micros();
                (
                    (route.model, route.provider, route.api_method),
                    (
                        route.available,
                        normalize_route_refresh_detail(&route.detail),
                        estimated_cost,
                    ),
                )
            })
            .collect();
    let after_route_map: BTreeMap<(String, String, String), (bool, String, Option<u64>)> =
        after_routes
            .into_iter()
            .map(|route| {
                let estimated_cost = route.estimated_reference_cost_micros();
                (
                    (route.model, route.provider, route.api_method),
                    (
                        route.available,
                        normalize_route_refresh_detail(&route.detail),
                        estimated_cost,
                    ),
                )
            })
            .collect();

    let models_added = after_model_set.difference(&before_model_set).count();
    let models_removed = before_model_set.difference(&after_model_set).count();
    let routes_added = after_route_map
        .keys()
        .filter(|key| !before_route_map.contains_key(*key))
        .count();
    let routes_removed = before_route_map
        .keys()
        .filter(|key| !after_route_map.contains_key(*key))
        .count();
    let routes_changed = after_route_map
        .iter()
        .filter(|(key, value)| {
            before_route_map
                .get(*key)
                .is_some_and(|before| before != *value)
        })
        .count();

    ModelCatalogRefreshSummary {
        model_count_before: before_model_set.len(),
        model_count_after: after_model_set.len(),
        models_added,
        models_removed,
        route_count_before: before_route_map.len(),
        route_count_after: after_route_map.len(),
        routes_added,
        routes_removed,
        routes_changed,
    }
}

pub fn is_listable_model_name(model: &str) -> bool {
    let trimmed = model.trim();
    !trimmed.is_empty() && !matches!(trimmed, "copilot models" | "openrouter models")
}

pub fn openrouter_catalog_model_id(model: &str) -> Option<String> {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.contains('/') {
        return Some(trimmed.to_string());
    }

    match provider_for_model(trimmed) {
        Some("claude") => Some(format!("anthropic/{}", trimmed)),
        Some("openai") => Some(format!("openai/{}", trimmed)),
        Some("openrouter") => Some(trimmed.to_string()),
        _ => None,
    }
}

pub fn listable_model_names_from_routes(routes: &[ModelRoute]) -> Vec<String> {
    let mut models = Vec::new();
    let mut seen = BTreeSet::new();
    for route in routes {
        if is_listable_model_name(&route.model) && seen.insert(route.model.clone()) {
            models.push(route.model.clone());
        }
    }
    models
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
use self::pricing::{cheapness_for_route, openrouter_pricing_from_model_pricing};
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
    async fn try_same_provider_account_failover(
        &self,
        provider: ActiveProvider,
        messages: &[Message],
        tools: &[ToolDefinition],
        mode: CompletionMode<'_>,
        initial_reason: &str,
        notes: &mut Vec<String>,
    ) -> Result<Option<EventStream>> {
        if !same_provider_account_failover_enabled() {
            return Ok(None);
        }

        let original_label = active_account_label_for_provider(provider);
        let Some(original_label) = original_label else {
            return Ok(None);
        };

        let alternatives = same_provider_account_candidates(provider);
        if alternatives.is_empty() {
            return Ok(None);
        }

        let provider_key = Self::provider_key(provider);
        let provider_label = Self::provider_label(provider);

        for alternative_label in &alternatives {
            crate::logging::info(&format!(
                "Same-provider failover{}: retrying {} using account '{}'",
                mode.log_suffix(),
                provider_label,
                alternative_label
            ));

            set_account_override_for_provider(provider, Some(alternative_label.clone()));
            clear_provider_unavailable_for_account(provider_key);
            if provider == ActiveProvider::OpenAI {
                clear_all_model_unavailability_for_account();
            }
            self.invalidate_provider_credentials_for_account_switch(provider)
                .await;

            let attempt = match mode {
                CompletionMode::Unified { system } => {
                    self.complete_on_provider(provider, messages, tools, system, None)
                        .await
                }
                CompletionMode::Split {
                    system_static,
                    system_dynamic,
                } => {
                    self.complete_split_on_provider(
                        provider,
                        messages,
                        tools,
                        system_static,
                        system_dynamic,
                        None,
                    )
                    .await
                }
            };

            match attempt {
                Ok(stream) => {
                    self.startup_notices
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(format!(
                        "⚡ Auto-switched {} account: {} → {}. To turn this off, set `[provider].same_provider_account_failover = false` in `~/.jcode/config.toml` or export `JCODE_SAME_PROVIDER_ACCOUNT_FAILOVER=false`.",
                        provider_label, original_label, alternative_label
                    ));
                    return Ok(Some(stream));
                }
                Err(err) => {
                    let summary =
                        maybe_annotate_limit_summary(provider, Self::summarize_error(&err));
                    let decision = Self::classify_failover_error(&err);
                    crate::logging::info(&format!(
                        "Same-provider account {} failed{}: {} (failover={} decision={})",
                        alternative_label,
                        mode.log_suffix(),
                        summary,
                        decision.should_failover(),
                        decision.as_str()
                    ));
                    notes.push(format!(
                        "{} account {}: {}",
                        provider_label, alternative_label, summary
                    ));
                    if decision.should_mark_provider_unavailable() {
                        record_provider_unavailable_for_account(provider_key, &summary);
                    }
                }
            }
        }

        set_account_override_for_provider(provider, Some(original_label));
        self.invalidate_provider_credentials_for_account_switch(provider)
            .await;
        if provider == ActiveProvider::OpenAI {
            clear_all_model_unavailability_for_account();
        }

        crate::logging::info(&format!(
            "Same-provider failover{} exhausted all alternate {} accounts after: {}",
            mode.log_suffix(),
            provider_label,
            initial_reason
        ));

        Ok(None)
    }

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
                .unwrap_or_else(|| "gpt-5.4".to_string()),
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

        // Handle explicit "copilot:" prefix from model picker
        if let Some(copilot_model) = model.strip_prefix("copilot:") {
            if let Some(forced) = self.forced_provider
                && forced != ActiveProvider::Copilot
            {
                let copilot_guard = self
                    .copilot_api
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
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
            let copilot_guard = self
                .copilot_api
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if copilot_guard.is_some() {
                *self
                    .active
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = ActiveProvider::Copilot;
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

        if let Some(antigravity_model) = model.strip_prefix("antigravity:") {
            if let Some(forced) = self.forced_provider
                && forced != ActiveProvider::Antigravity
            {
                let antigravity_guard = self
                    .antigravity
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if antigravity_guard.is_none() {
                    anyhow::bail!(
                        "Model '{}' requires Antigravity but Antigravity credentials are not configured. Run `jcode login --provider antigravity` first.",
                        antigravity_model
                    );
                }
                drop(antigravity_guard);
                crate::logging::info(&format!(
                    "Switching from {} to Antigravity for model '{}'",
                    Self::provider_label(forced),
                    antigravity_model,
                ));
            }
            let antigravity_guard = self
                .antigravity
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if antigravity_guard.is_some() {
                *self
                    .active
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = ActiveProvider::Antigravity;
                if let Some(ref antigravity) = *antigravity_guard {
                    antigravity.set_model(antigravity_model)?;
                }
                return Ok(());
            }
            drop(antigravity_guard);
            if self.forced_provider == Some(ActiveProvider::Antigravity) {
                return Err(anyhow::anyhow!(
                    "Antigravity is locked by --provider but is not configured. Run `jcode login --provider antigravity`."
                ));
            }
            crate::logging::info(&format!(
                "Antigravity not available for '{}', trying other providers",
                antigravity_model
            ));
            return self.set_model(antigravity_model);
        }

        // Handle explicit "cursor:" prefix from model picker/default config
        if let Some(cursor_model) = model.strip_prefix("cursor:") {
            if let Some(forced) = self.forced_provider
                && forced != ActiveProvider::Cursor
            {
                let cursor_guard = self
                    .cursor
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
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
            let cursor_guard = self
                .cursor
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if cursor_guard.is_some() {
                *self
                    .active
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = ActiveProvider::Cursor;
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

        if let Some((base_model, provider_pin)) = model.rsplit_once('@')
            && !provider_pin.trim().is_empty()
            && let Some(openrouter_model) = openrouter_catalog_model_id(base_model)
        {
            let openrouter = self
                .openrouter
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            if openrouter.is_none() {
                return Err(anyhow::anyhow!(
                    "OpenRouter credentials not available. Set OPENROUTER_API_KEY environment variable."
                ));
            }
            *self
                .active
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = ActiveProvider::OpenRouter;
            if let Some(openrouter) = openrouter {
                return openrouter.set_model(&format!("{}@{}", openrouter_model, provider_pin));
            }
        }

        // Detect which provider this model belongs to
        let target_provider = provider_for_model(model);
        if let Some(forced) = self.forced_provider
            && let Some(target) = target_provider
        {
            let target_active = match target {
                "claude" => ActiveProvider::Claude,
                "openai" => ActiveProvider::OpenAI,
                "antigravity" => ActiveProvider::Antigravity,
                "gemini" => ActiveProvider::Gemini,
                "cursor" => ActiveProvider::Cursor,
                "openrouter" => ActiveProvider::OpenRouter,
                _ => forced,
            };
            if target_active != forced {
                let has_target_creds = match target_active {
                    ActiveProvider::Claude => self.has_claude_runtime(),
                    ActiveProvider::OpenAI => self.openai_provider().is_some(),
                    ActiveProvider::Antigravity => self.antigravity_provider().is_some(),
                    ActiveProvider::Gemini => self.gemini_provider().is_some(),
                    ActiveProvider::Cursor => self.cursor_provider().is_some(),
                    ActiveProvider::OpenRouter => self.openrouter_provider().is_some(),
                    ActiveProvider::Copilot => self.copilot_provider().is_some(),
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

        if target_provider == Some("claude") {
            if !self.has_claude_runtime() {
                return Err(anyhow::anyhow!(
                    "Claude credentials not available. Run `claude` to log in first."
                ));
            }
            // Switch active provider to Claude
            *self
                .active
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = ActiveProvider::Claude;
            // Set on whichever is available
            if let Some(anthropic) = self.anthropic_provider() {
                anthropic.set_model(model)
            } else if let Some(claude) = self.claude_provider() {
                claude.set_model(model)
            } else {
                Ok(())
            }
        } else if target_provider == Some("openai") {
            if self.openai_provider().is_none() {
                return Err(anyhow::anyhow!(
                    "OpenAI credentials not available. Run `jcode login --provider openai` first."
                ));
            }
            // Switch active provider to OpenAI
            *self
                .active
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = ActiveProvider::OpenAI;
            if let Some(openai) = self.openai_provider() {
                openai.set_model(model)
            } else {
                Ok(())
            }
        } else if target_provider == Some("gemini") {
            let gemini = self.gemini_provider();
            if gemini.is_none() {
                return Err(anyhow::anyhow!(
                    "Gemini credentials not available. Run `jcode login --provider gemini` first."
                ));
            }
            *self
                .active
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = ActiveProvider::Gemini;
            if let Some(gemini) = gemini {
                gemini.set_model(model)
            } else {
                Ok(())
            }
        } else if target_provider == Some("antigravity") {
            let antigravity = self.antigravity_provider();
            if antigravity.is_none() {
                return Err(anyhow::anyhow!(
                    "Antigravity credentials not available. Run `jcode login --provider antigravity` first."
                ));
            }
            *self
                .active
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = ActiveProvider::Antigravity;
            if let Some(antigravity) = antigravity {
                antigravity.set_model(model)
            } else {
                Ok(())
            }
        } else if target_provider == Some("cursor") {
            let cursor_guard = self
                .cursor
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if cursor_guard.is_none() {
                return Err(anyhow::anyhow!(
                    "Cursor credentials not available. Run `jcode login --provider cursor` first."
                ));
            }
            *self
                .active
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = ActiveProvider::Cursor;
            if let Some(ref cursor) = *cursor_guard {
                cursor.set_model(model)
            } else {
                Ok(())
            }
        } else if target_provider == Some("openrouter") {
            let openrouter = self
                .openrouter
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            if openrouter.is_none() {
                return Err(anyhow::anyhow!(
                    "OpenRouter credentials not available. Set OPENROUTER_API_KEY environment variable."
                ));
            }
            // Switch active provider to OpenRouter
            *self
                .active
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = ActiveProvider::OpenRouter;
            if let Some(openrouter) = openrouter {
                openrouter.set_model(model)
            } else {
                Ok(())
            }
        } else {
            // Unknown model - try current provider
            match self.active_provider() {
                ActiveProvider::Claude => {
                    if let Some(anthropic) = self.anthropic_provider() {
                        anthropic.set_model(model)
                    } else if let Some(claude) = self.claude_provider() {
                        claude.set_model(model)
                    } else {
                        Err(anyhow::anyhow!("Unknown model: {}", model))
                    }
                }
                ActiveProvider::OpenAI => {
                    if let Some(openai) = self.openai_provider() {
                        openai.set_model(model)
                    } else {
                        Err(anyhow::anyhow!("Unknown model: {}", model))
                    }
                }
                ActiveProvider::Copilot => {
                    if let Some(copilot) = self.copilot_provider() {
                        copilot.set_model(model)
                    } else {
                        Err(anyhow::anyhow!("Unknown model: {}", model))
                    }
                }
                ActiveProvider::Antigravity => {
                    if let Some(antigravity) = self.antigravity_provider() {
                        antigravity.set_model(model)
                    } else {
                        Err(anyhow::anyhow!("Unknown model: {}", model))
                    }
                }
                ActiveProvider::Gemini => {
                    if let Some(gemini) = self.gemini_provider() {
                        gemini.set_model(model)
                    } else {
                        Err(anyhow::anyhow!("Unknown model: {}", model))
                    }
                }
                ActiveProvider::Cursor => {
                    if let Some(cursor) = self.cursor_provider() {
                        cursor.set_model(model)
                    } else {
                        Err(anyhow::anyhow!("Unknown model: {}", model))
                    }
                }
                ActiveProvider::OpenRouter => {
                    if let Some(openrouter) = self.openrouter_provider() {
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
        self.spawn_anthropic_catalog_refresh_if_needed();
        self.spawn_openai_catalog_refresh_if_needed();

        let mut routes = Vec::new();
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
            if let Some(copilot) = self.copilot_provider() {
                let copilot_models = copilot.available_models_display();
                let detail = copilot.model_catalog_detail();
                let copilot_models_empty = copilot_models.is_empty();
                for model in copilot_models {
                    let cheapness = cheapness_for_route(&model, "Copilot", "copilot");
                    routes.push(ModelRoute {
                        model,
                        provider: "Copilot".to_string(),
                        api_method: "copilot".to_string(),
                        available: true,
                        detail: detail.clone(),
                        cheapness,
                    });
                }
                if copilot_models_empty && copilot::CopilotApiProvider::has_credentials() {
                    routes.push(ModelRoute {
                        model: "copilot models".to_string(),
                        provider: "Copilot".to_string(),
                        api_method: "copilot".to_string(),
                        available: false,
                        detail,
                        cheapness: cheapness_for_route("claude-sonnet-4-6", "Copilot", "copilot"),
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
            let mut scheduled_endpoint_refreshes = 0usize;
            for model in openrouter.available_models_display() {
                let cached = openrouter::load_endpoints_disk_cache_public(&model);
                let cache_age = cached.as_ref().map(|(_, age)| *age);
                if model == current_openrouter_model || scheduled_endpoint_refreshes < 8 {
                    if openrouter.maybe_schedule_endpoint_refresh_for_display(
                        &model,
                        cache_age,
                        "model picker route hydration",
                    ) {
                        scheduled_endpoint_refreshes += 1;
                    }
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
                crate::auth::AuthStatus::check().cursor,
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
