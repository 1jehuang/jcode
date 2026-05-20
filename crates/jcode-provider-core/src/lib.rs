pub mod anthropic;
pub mod catalog_refresh;
pub mod failover;
pub mod models;
pub mod openai_schema;
pub mod pricing;
pub mod selection;

pub use anthropic::{
    ANTHROPIC_OAUTH_BETA_HEADERS, ANTHROPIC_OAUTH_BETA_HEADERS_1M, anthropic_effectively_1m,
    anthropic_is_1m_model, anthropic_map_tool_name_for_oauth, anthropic_map_tool_name_from_oauth,
    anthropic_oauth_beta_headers, anthropic_stainless_arch, anthropic_stainless_os,
    anthropic_strip_1m_suffix,
};
pub use catalog_refresh::{ModelCatalogRefreshSummary, summarize_model_catalog_refresh};
pub use failover::{
    FailoverDecision, ProviderFailoverPrompt, classify_failover_error_message,
    parse_failover_prompt_message,
};
pub use models::{
    ALL_CLAUDE_MODELS, ALL_OPENAI_MODELS, DEFAULT_CONTEXT_LIMIT, ModelCapabilities,
    context_limit_for_model, context_limit_for_model_with_provider,
    context_limit_for_model_with_provider_and_cache, is_listable_model_name,
    normalize_copilot_model_name, provider_for_model as core_provider_for_model,
    provider_for_model_with_hint as core_provider_for_model_with_hint, provider_key_from_hint,
};
pub use selection::{
    ActiveProvider, ProviderAvailability, auto_default_provider, dedupe_model_routes,
    explicit_model_provider_prefix, fallback_sequence, model_name_for_provider,
    parse_provider_hint, provider_from_model_key, provider_key, provider_label,
};

use anyhow::Result;
use async_trait::async_trait;
use futures::Stream;
use jcode_message_types::{
    ContentBlock, Message, Role, StreamEvent, ToolDefinition, messages_with_dynamic_system_context,
};
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

/// Stream of events from a provider.
pub type EventStream = Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>;

/// Provider trait for LLM backends.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Send messages and get a streaming response.
    /// resume_session_id: Optional session ID to resume a previous conversation (provider-specific).
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream>;

    /// Send messages with split system prompt for better caching.
    async fn complete_split(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system_static: &str,
        system_dynamic: &str,
        resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let dynamic_messages = messages_with_dynamic_system_context(messages, system_dynamic);
        self.complete(&dynamic_messages, tools, system_static, resume_session_id)
            .await
    }

    /// Get the provider name.
    fn name(&self) -> &str;

    /// Get the model identifier being used.
    fn model(&self) -> String {
        "unknown".to_string()
    }

    /// Whether this provider path can safely receive `ContentBlock::Image` inputs.
    fn supports_image_input(&self) -> bool {
        false
    }

    /// Set the model to use (returns error if model not supported).
    fn set_model(&self, _model: &str) -> Result<()> {
        Err(anyhow::anyhow!(
            "This provider does not support model switching"
        ))
    }

    /// List available models for this provider.
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
    fn provider_details_for_model(&self, _model: &str) -> Vec<(String, String)> {
        Vec::new()
    }

    /// Return the currently preferred upstream provider.
    fn preferred_provider(&self) -> Option<String> {
        None
    }

    /// Get all model routes for the unified picker.
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
    fn on_auth_changed(&self) {}

    /// Called when auth credentials change for an already-open session that
    /// should learn about refreshed credentials without being silently moved to
    /// a newly activated provider/profile.
    fn on_auth_changed_preserve_current_provider(&self) {
        self.on_auth_changed();
    }

    /// Get the reasoning effort level (if applicable).
    fn reasoning_effort(&self) -> Option<String> {
        None
    }

    /// Set the reasoning effort level (if applicable).
    fn set_reasoning_effort(&self, _effort: &str) -> Result<()> {
        Err(anyhow::anyhow!(
            "This provider does not support reasoning effort"
        ))
    }

    /// Get ordered list of available reasoning effort levels.
    fn available_efforts(&self) -> Vec<&'static str> {
        vec![]
    }

    /// Get the active service tier override (if applicable).
    fn service_tier(&self) -> Option<String> {
        None
    }

    /// Set the active service tier override (if applicable).
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

    /// Returns true if the provider executes tools internally.
    fn handles_tools_internally(&self) -> bool {
        false
    }

    /// Invalidate any cached credentials.
    async fn invalidate_credentials(&self) {}

    /// Set Copilot premium request conservation mode.
    fn set_premium_mode(&self, _mode: PremiumMode) {}

    /// Get the current Copilot premium mode.
    fn premium_mode(&self) -> PremiumMode {
        PremiumMode::Normal
    }

    /// Returns true if jcode should use its own compaction for this provider.
    fn supports_compaction(&self) -> bool {
        false
    }

    /// Returns true if jcode should proactively run its own summary-based compaction.
    fn uses_jcode_compaction(&self) -> bool {
        self.supports_compaction()
    }

    /// Ask the provider to produce a native compaction artifact.
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
    fn context_window(&self) -> usize {
        context_limit_for_model_with_provider(&self.model(), Some(self.name()))
            .unwrap_or(DEFAULT_CONTEXT_LIMIT)
    }

    /// Create a new provider instance with independent mutable state.
    fn fork(&self) -> Arc<dyn Provider>;

    /// Get a sender for native tool results (if the provider supports it).
    fn native_result_sender(&self) -> Option<NativeToolResultSender> {
        None
    }

    /// Drain any startup notices.
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

/// Premium request conservation mode for Copilot-compatible providers.
/// 0 = normal (every user message is premium)
/// 1 = one premium per session (first user message only, rest are agent)
/// 2 = zero premium (all requests sent as agent)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PremiumMode {
    Normal = 0,
    OnePerSession = 1,
    Zero = 2,
}

/// Channel for sending provider-native tool results back to a provider bridge.
pub type NativeToolResultSender = tokio::sync::mpsc::Sender<NativeToolResult>;

/// Native tool result to send back to provider bridges that delegate tool execution to jcode.
#[derive(Debug, Clone, Serialize)]
pub struct NativeToolResult {
    #[serde(rename = "type")]
    pub msg_type: &'static str,
    pub request_id: String,
    pub result: NativeToolResultPayload,
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct NativeToolResultPayload {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl NativeToolResult {
    pub fn success(request_id: String, output: String) -> Self {
        Self {
            msg_type: "native_tool_result",
            request_id,
            result: NativeToolResultPayload {
                output: Some(output),
                error: None,
            },
            is_error: false,
        }
    }

    pub fn error(request_id: String, error: String) -> Self {
        Self {
            msg_type: "native_tool_result",
            request_id,
            result: NativeToolResultPayload {
                output: None,
                error: Some(error),
            },
            is_error: true,
        }
    }
}

/// Canonical User-Agent for generic outbound Jcode HTTP requests.
pub const JCODE_USER_AGENT: &str = concat!("jcode/", env!("CARGO_PKG_VERSION"));

/// Shared HTTP client for all generic provider requests. Creating a `reqwest::Client` is expensive
/// (~10ms due to TLS init, connection pool setup), so we reuse a single instance. Provider-specific
/// transports may override the User-Agent on individual requests when they intentionally need to
/// match an official client.
pub fn shared_http_client() -> reqwest::Client {
    use std::sync::OnceLock;
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .user_agent(JCODE_USER_AGENT)
                .connect_timeout(Duration::from_secs(15))
                .tcp_keepalive(Some(Duration::from_secs(30)))
                .pool_idle_timeout(Duration::from_secs(90))
                .pool_max_idle_per_host(8)
                .build()
                .unwrap_or_else(|err| {
                    eprintln!("jcode: failed to build shared provider HTTP client: {err}");
                    match reqwest::Client::builder()
                        .user_agent(JCODE_USER_AGENT)
                        .build()
                    {
                        Ok(client) => client,
                        Err(fallback_err) => {
                            eprintln!(
                                "jcode: failed to build fallback provider HTTP client: {fallback_err}"
                            );
                            reqwest::Client::new()
                        }
                    }
                })
        })
        .clone()
}

#[derive(Debug, Clone)]
pub struct NativeCompactionResult {
    pub summary_text: Option<String>,
    pub openai_encrypted_content: Option<String>,
}

/// A single route to access a model: model + provider + API method
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelRoute {
    pub model: String,
    pub provider: String,
    pub api_method: String,
    pub available: bool,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cheapness: Option<RouteCheapnessEstimate>,
}

impl ModelRoute {
    pub fn estimated_reference_cost_micros(&self) -> Option<u64> {
        self.cheapness
            .as_ref()
            .and_then(|estimate| estimate.estimated_reference_cost_micros)
    }
}

pub const CHEAPNESS_REFERENCE_INPUT_TOKENS: u64 = 25_000;
pub const CHEAPNESS_REFERENCE_OUTPUT_TOKENS: u64 = 5_000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RouteBillingKind {
    Metered,
    Subscription,
    IncludedQuota,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RouteCostSource {
    PublicApiPricing,
    PublicPlanPricing,
    RuntimePlan,
    OpenRouterEndpoint,
    OpenRouterCatalog,
    Heuristic,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RouteCostConfidence {
    Exact,
    High,
    Medium,
    Low,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RouteCheapnessEstimate {
    pub billing_kind: RouteBillingKind,
    pub source: RouteCostSource,
    pub confidence: RouteCostConfidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monthly_price_micros: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_price_per_mtok_micros: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_price_per_mtok_micros: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_price_per_mtok_micros: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub included_requests_per_month: Option<u64>,
    pub reference_input_tokens: u64,
    pub reference_output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_reference_cost_micros: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl RouteCheapnessEstimate {
    pub fn metered(
        source: RouteCostSource,
        confidence: RouteCostConfidence,
        input_price_per_mtok_micros: u64,
        output_price_per_mtok_micros: u64,
        cache_read_price_per_mtok_micros: Option<u64>,
        note: impl Into<Option<String>>,
    ) -> Self {
        Self {
            billing_kind: RouteBillingKind::Metered,
            source,
            confidence,
            monthly_price_micros: None,
            input_price_per_mtok_micros: Some(input_price_per_mtok_micros),
            output_price_per_mtok_micros: Some(output_price_per_mtok_micros),
            cache_read_price_per_mtok_micros,
            included_requests_per_month: None,
            reference_input_tokens: CHEAPNESS_REFERENCE_INPUT_TOKENS,
            reference_output_tokens: CHEAPNESS_REFERENCE_OUTPUT_TOKENS,
            estimated_reference_cost_micros: Some(reference_request_cost_micros(
                input_price_per_mtok_micros,
                output_price_per_mtok_micros,
            )),
            note: note.into(),
        }
    }

    pub fn subscription(
        source: RouteCostSource,
        confidence: RouteCostConfidence,
        monthly_price_micros: u64,
        included_requests_per_month: Option<u64>,
        note: impl Into<Option<String>>,
    ) -> Self {
        Self {
            billing_kind: RouteBillingKind::Subscription,
            source,
            confidence,
            monthly_price_micros: Some(monthly_price_micros),
            input_price_per_mtok_micros: None,
            output_price_per_mtok_micros: None,
            cache_read_price_per_mtok_micros: None,
            included_requests_per_month,
            reference_input_tokens: CHEAPNESS_REFERENCE_INPUT_TOKENS,
            reference_output_tokens: CHEAPNESS_REFERENCE_OUTPUT_TOKENS,
            estimated_reference_cost_micros: included_requests_per_month
                .map(|count| monthly_price_micros / count.max(1)),
            note: note.into(),
        }
    }

    pub fn included_quota(
        source: RouteCostSource,
        confidence: RouteCostConfidence,
        monthly_price_micros: u64,
        included_requests_per_month: Option<u64>,
        estimated_reference_cost_micros: Option<u64>,
        note: impl Into<Option<String>>,
    ) -> Self {
        Self {
            billing_kind: RouteBillingKind::IncludedQuota,
            source,
            confidence,
            monthly_price_micros: Some(monthly_price_micros),
            input_price_per_mtok_micros: None,
            output_price_per_mtok_micros: None,
            cache_read_price_per_mtok_micros: None,
            included_requests_per_month,
            reference_input_tokens: CHEAPNESS_REFERENCE_INPUT_TOKENS,
            reference_output_tokens: CHEAPNESS_REFERENCE_OUTPUT_TOKENS,
            estimated_reference_cost_micros,
            note: note.into(),
        }
    }
}

fn reference_request_cost_micros(
    input_price_per_mtok_micros: u64,
    output_price_per_mtok_micros: u64,
) -> u64 {
    input_price_per_mtok_micros.saturating_mul(CHEAPNESS_REFERENCE_INPUT_TOKENS) / 1_000_000
        + output_price_per_mtok_micros.saturating_mul(CHEAPNESS_REFERENCE_OUTPUT_TOKENS) / 1_000_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metered_estimate_computes_reference_cost() {
        let estimate = RouteCheapnessEstimate::metered(
            RouteCostSource::Heuristic,
            RouteCostConfidence::Low,
            2_000_000,
            8_000_000,
            None,
            None,
        );
        assert_eq!(estimate.estimated_reference_cost_micros, Some(90_000));
    }

    #[test]
    fn shared_http_client_reuses_builder() {
        let _a = shared_http_client();
        let _b = shared_http_client();
    }

    #[test]
    fn canonical_user_agent_identifies_jcode() {
        assert!(JCODE_USER_AGENT.starts_with("jcode/"));
    }
}
