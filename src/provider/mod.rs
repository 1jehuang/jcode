pub mod anthropic;
pub mod antigravity;
pub mod claude;
pub mod cli_common;
pub mod copilot;
pub mod cursor;
pub mod openai;
pub mod openrouter;

use crate::auth;

/// Shared HTTP client for all providers. Creating a `reqwest::Client` is expensive
/// (~10ms due to TLS init, connection pool setup), so we reuse a single instance.
pub(crate) fn shared_http_client() -> reqwest::Client {
    use std::sync::OnceLock;
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| reqwest::Client::new()).clone()
}
use crate::message::{ContentBlock, Message, Role, StreamEvent, ToolDefinition};
use anyhow::Result;
use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime};

// Re-export native tool result types for use by agent
pub use claude::{NativeToolResult, NativeToolResultSender};

/// Stream of events from a provider
pub type EventStream = Pin<Box<dyn Stream<Item = Result<StreamEvent>> + Send>>;

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

fn usd_to_micros(usd: f64) -> u64 {
    (usd * 1_000_000.0).round() as u64
}

fn usd_per_token_str_to_micros_per_mtok(raw: &str) -> Option<u64> {
    raw.trim()
        .parse::<f64>()
        .ok()
        .map(|usd_per_token| (usd_per_token * 1_000_000_000_000.0).round() as u64)
}

fn reference_request_cost_micros(
    input_price_per_mtok_micros: u64,
    output_price_per_mtok_micros: u64,
) -> u64 {
    input_price_per_mtok_micros.saturating_mul(CHEAPNESS_REFERENCE_INPUT_TOKENS) / 1_000_000
        + output_price_per_mtok_micros.saturating_mul(CHEAPNESS_REFERENCE_OUTPUT_TOKENS) / 1_000_000
}

fn anthropic_api_pricing(model: &str) -> Option<RouteCheapnessEstimate> {
    let base = model.strip_suffix("[1m]").unwrap_or(model);
    let long_context = model.ends_with("[1m]");
    match base {
        "claude-opus-4-6" => Some(RouteCheapnessEstimate::metered(
            RouteCostSource::PublicApiPricing,
            RouteCostConfidence::Exact,
            usd_to_micros(if long_context { 10.0 } else { 5.0 }),
            usd_to_micros(if long_context { 37.5 } else { 25.0 }),
            Some(usd_to_micros(if long_context { 1.0 } else { 0.5 })),
            Some(if long_context {
                "Anthropic API long-context pricing".to_string()
            } else {
                "Anthropic API pricing".to_string()
            }),
        )),
        "claude-sonnet-4-6" => Some(RouteCheapnessEstimate::metered(
            RouteCostSource::PublicApiPricing,
            RouteCostConfidence::Exact,
            usd_to_micros(if long_context { 6.0 } else { 3.0 }),
            usd_to_micros(if long_context { 22.5 } else { 15.0 }),
            Some(usd_to_micros(if long_context { 0.6 } else { 0.3 })),
            Some(if long_context {
                "Anthropic API long-context pricing".to_string()
            } else {
                "Anthropic API pricing".to_string()
            }),
        )),
        "claude-haiku-4-5" => Some(RouteCheapnessEstimate::metered(
            RouteCostSource::PublicApiPricing,
            RouteCostConfidence::Exact,
            usd_to_micros(1.0),
            usd_to_micros(5.0),
            Some(usd_to_micros(0.1)),
            Some("Anthropic API pricing".to_string()),
        )),
        "claude-opus-4-5" => Some(RouteCheapnessEstimate::metered(
            RouteCostSource::Heuristic,
            RouteCostConfidence::Medium,
            usd_to_micros(5.0),
            usd_to_micros(25.0),
            Some(usd_to_micros(0.5)),
            Some("Estimated from Opus 4.6 API pricing".to_string()),
        )),
        "claude-sonnet-4-5" | "claude-sonnet-4-20250514" => Some(RouteCheapnessEstimate::metered(
            RouteCostSource::Heuristic,
            RouteCostConfidence::Medium,
            usd_to_micros(3.0),
            usd_to_micros(15.0),
            Some(usd_to_micros(0.3)),
            Some("Estimated from Sonnet 4.6 API pricing".to_string()),
        )),
        _ => None,
    }
}

fn anthropic_oauth_subscription_type() -> Option<String> {
    auth::claude::get_subscription_type().map(|raw| raw.trim().to_ascii_lowercase())
}

fn anthropic_oauth_pricing(model: &str) -> RouteCheapnessEstimate {
    let subscription = anthropic_oauth_subscription_type();
    let base = model.strip_suffix("[1m]").unwrap_or(model);
    let is_opus = base.contains("opus");
    let is_1m = model.ends_with("[1m]");

    match subscription.as_deref() {
        Some("max") => RouteCheapnessEstimate::subscription(
            RouteCostSource::RuntimePlan,
            RouteCostConfidence::Medium,
            usd_to_micros(100.0),
            None,
            Some(if is_opus {
                "Claude Max plan; Opus access included".to_string()
            } else if is_1m {
                "Claude Max plan; 1M context depends on extra usage availability".to_string()
            } else {
                "Claude Max plan".to_string()
            }),
        ),
        Some("pro") => RouteCheapnessEstimate::subscription(
            RouteCostSource::RuntimePlan,
            RouteCostConfidence::Medium,
            usd_to_micros(20.0),
            None,
            Some(if is_1m {
                "Claude Pro plan; 1M context depends on extra usage availability".to_string()
            } else {
                "Claude Pro plan".to_string()
            }),
        ),
        Some(other) => RouteCheapnessEstimate::subscription(
            RouteCostSource::RuntimePlan,
            RouteCostConfidence::Low,
            usd_to_micros(20.0),
            None,
            Some(format!(
                "Claude OAuth plan '{}'; assumed Pro-like pricing",
                other
            )),
        ),
        None => RouteCheapnessEstimate::subscription(
            RouteCostSource::PublicPlanPricing,
            RouteCostConfidence::Low,
            usd_to_micros(if is_opus { 100.0 } else { 20.0 }),
            None,
            Some(if is_opus {
                "Opus access implies Claude Max-like subscription pricing".to_string()
            } else {
                "Claude OAuth subscription pricing (plan not detected)".to_string()
            }),
        ),
    }
}

fn openai_effective_auth_mode() -> &'static str {
    match auth::codex::load_credentials() {
        Ok(creds) if !creds.refresh_token.is_empty() || creds.id_token.is_some() => "oauth",
        Ok(_) => "api-key",
        Err(_) => {
            if std::env::var("OPENAI_API_KEY")
                .ok()
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false)
            {
                "api-key"
            } else {
                "oauth"
            }
        }
    }
}

fn openai_api_pricing(model: &str) -> Option<RouteCheapnessEstimate> {
    let base = model.strip_suffix("[1m]").unwrap_or(model);
    match base {
        "gpt-5.4" | "gpt-5.4-pro" => Some(RouteCheapnessEstimate::metered(
            RouteCostSource::PublicApiPricing,
            RouteCostConfidence::High,
            usd_to_micros(2.5),
            usd_to_micros(15.0),
            Some(usd_to_micros(0.25)),
            Some("OpenAI API pricing".to_string()),
        )),
        "gpt-5.3-codex" | "gpt-5.2-codex" | "gpt-5.2" | "gpt-5.1" | "gpt-5.1-codex" => {
            Some(RouteCheapnessEstimate::metered(
                RouteCostSource::Heuristic,
                RouteCostConfidence::Low,
                usd_to_micros(2.5),
                usd_to_micros(15.0),
                Some(usd_to_micros(0.25)),
                Some("Estimated from GPT-5.4 API pricing".to_string()),
            ))
        }
        "gpt-5.3-codex-spark" | "gpt-5.1-codex-mini" => Some(RouteCheapnessEstimate::metered(
            RouteCostSource::Heuristic,
            RouteCostConfidence::Low,
            usd_to_micros(0.25),
            usd_to_micros(2.0),
            Some(usd_to_micros(0.025)),
            Some("Estimated from GPT-5 mini API pricing".to_string()),
        )),
        "gpt-5.1-codex-max"
        | "gpt-5.2-pro"
        | "gpt-5-chat-latest"
        | "gpt-5.1-chat-latest"
        | "gpt-5.2-chat-latest"
        | "gpt-5-codex"
        | "gpt-5" => Some(RouteCheapnessEstimate::metered(
            RouteCostSource::Heuristic,
            RouteCostConfidence::Low,
            usd_to_micros(2.5),
            usd_to_micros(15.0),
            Some(usd_to_micros(0.25)),
            Some("Estimated from GPT-5.4 API pricing".to_string()),
        )),
        _ => None,
    }
}

fn openai_oauth_pricing(model: &str) -> RouteCheapnessEstimate {
    let base = model.strip_suffix("[1m]").unwrap_or(model);
    let likely_pro = base.contains("pro") || base == "gpt-5.4";
    RouteCheapnessEstimate::subscription(
        RouteCostSource::PublicPlanPricing,
        RouteCostConfidence::Low,
        usd_to_micros(if likely_pro { 200.0 } else { 20.0 }),
        None,
        Some(if likely_pro {
            "ChatGPT subscription estimate; advanced GPT-5 access treated as Pro-like".to_string()
        } else {
            "ChatGPT subscription estimate".to_string()
        }),
    )
}

fn copilot_pricing(model: &str) -> RouteCheapnessEstimate {
    let mode = std::env::var("JCODE_COPILOT_PREMIUM").ok();
    let is_zero = matches!(mode.as_deref(), Some("0"));
    let is_one = matches!(mode.as_deref(), Some("1"));
    let likely_premium_model = model.contains("opus") || model.contains("gpt-5.4");
    let monthly_price = if likely_premium_model {
        usd_to_micros(39.0)
    } else {
        usd_to_micros(10.0)
    };
    let included_requests = if likely_premium_model { 1_500 } else { 300 };
    let estimated_reference = if is_zero {
        Some(0)
    } else if is_one {
        Some(monthly_price / included_requests)
    } else {
        Some(monthly_price / included_requests)
    };

    RouteCheapnessEstimate::included_quota(
        RouteCostSource::RuntimePlan,
        if is_zero {
            RouteCostConfidence::High
        } else {
            RouteCostConfidence::Medium
        },
        monthly_price,
        Some(included_requests),
        estimated_reference,
        Some(if is_zero {
            "Copilot zero-premium mode: jcode will send requests as agent/non-premium when possible"
                .to_string()
        } else if likely_premium_model {
            "Copilot premium-request estimate using Pro+/premium pricing".to_string()
        } else {
            "Copilot estimate using Pro included premium requests".to_string()
        }),
    )
}

fn openrouter_pricing_from_model_pricing(
    pricing: &openrouter::ModelPricing,
    source: RouteCostSource,
    confidence: RouteCostConfidence,
    note: Option<String>,
) -> Option<RouteCheapnessEstimate> {
    let input = pricing
        .prompt
        .as_deref()
        .and_then(usd_per_token_str_to_micros_per_mtok)?;
    let output = pricing
        .completion
        .as_deref()
        .and_then(usd_per_token_str_to_micros_per_mtok)?;
    let cache = pricing
        .input_cache_read
        .as_deref()
        .and_then(usd_per_token_str_to_micros_per_mtok);
    Some(RouteCheapnessEstimate::metered(
        source, confidence, input, output, cache, note,
    ))
}

fn openrouter_route_pricing(model: &str, provider: &str) -> Option<RouteCheapnessEstimate> {
    let cache = openrouter::load_endpoints_disk_cache_public(model);
    if let Some((endpoints, _)) = cache.as_ref() {
        if provider == "auto" {
            if let Some(best) = endpoints.first() {
                return openrouter_pricing_from_model_pricing(
                    &best.pricing,
                    RouteCostSource::OpenRouterEndpoint,
                    RouteCostConfidence::High,
                    Some(format!(
                        "OpenRouter auto route currently prefers {}",
                        best.provider_name
                    )),
                );
            }
        }
        if let Some(endpoint) = endpoints.iter().find(|ep| ep.provider_name == provider) {
            return openrouter_pricing_from_model_pricing(
                &endpoint.pricing,
                RouteCostSource::OpenRouterEndpoint,
                RouteCostConfidence::High,
                Some(format!("OpenRouter endpoint pricing for {}", provider)),
            );
        }
    }

    openrouter::load_model_pricing_disk_cache_public(model).and_then(|pricing| {
        openrouter_pricing_from_model_pricing(
            &pricing,
            RouteCostSource::OpenRouterCatalog,
            RouteCostConfidence::Medium,
            Some("OpenRouter model catalog pricing".to_string()),
        )
    })
}

#[allow(dead_code)]
fn cheapness_for_route(
    model: &str,
    provider: &str,
    api_method: &str,
) -> Option<RouteCheapnessEstimate> {
    match api_method {
        "claude-oauth" => Some(anthropic_oauth_pricing(model)),
        "api-key" if provider == "Anthropic" => anthropic_api_pricing(model),
        "openai-oauth" => {
            if openai_effective_auth_mode() == "api-key" {
                openai_api_pricing(model)
            } else {
                Some(openai_oauth_pricing(model))
            }
        }
        "copilot" => Some(copilot_pricing(model)),
        "openrouter" => {
            let model_id = if model.contains('/') {
                model.to_string()
            } else if ALL_CLAUDE_MODELS.contains(&model) {
                format!("anthropic/{}", model)
            } else if ALL_OPENAI_MODELS.contains(&model) {
                format!("openai/{}", model)
            } else {
                model.to_string()
            };
            openrouter_route_pricing(&model_id, provider)
        }
        _ => None,
    }
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

/// Default context window size when model-specific data isn't known.
pub const DEFAULT_CONTEXT_LIMIT: usize = 200_000;

/// Dynamic cache of model context window sizes, populated from API at startup.
static CONTEXT_LIMIT_CACHE: std::sync::LazyLock<RwLock<HashMap<String, usize>>> =
    std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));

#[derive(Debug, Clone)]
struct RuntimeModelUnavailability {
    reason: String,
    recorded_at: Instant,
    observed_at: SystemTime,
}

#[derive(Debug, Clone)]
struct RuntimeProviderUnavailability {
    reason: String,
    recorded_at: Instant,
    observed_at: SystemTime,
}

/// Dynamic cache of models actually available for this account (populated from Codex API).
/// When populated, only models in this set should be offered/accepted for the OpenAI provider.
static ACCOUNT_AVAILABLE_MODELS: std::sync::LazyLock<RwLock<Option<HashSet<String>>>> =
    std::sync::LazyLock::new(|| RwLock::new(None));
static ACCOUNT_AVAILABLE_MODELS_FETCHED_AT: std::sync::LazyLock<RwLock<Option<Instant>>> =
    std::sync::LazyLock::new(|| RwLock::new(None));
static ACCOUNT_AVAILABLE_MODELS_OBSERVED_AT: std::sync::LazyLock<RwLock<Option<SystemTime>>> =
    std::sync::LazyLock::new(|| RwLock::new(None));
static ACCOUNT_RUNTIME_UNAVAILABLE_MODELS: std::sync::LazyLock<
    RwLock<HashMap<String, RuntimeModelUnavailability>>,
> = std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));
static ACCOUNT_RUNTIME_UNAVAILABLE_PROVIDERS: std::sync::LazyLock<
    RwLock<HashMap<String, RuntimeProviderUnavailability>>,
> = std::sync::LazyLock::new(|| RwLock::new(HashMap::new()));
static ACCOUNT_MODEL_REFRESH_LAST_ATTEMPT: std::sync::LazyLock<RwLock<Option<Instant>>> =
    std::sync::LazyLock::new(|| RwLock::new(None));
static ACCOUNT_MODEL_REFRESH_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
const ACCOUNT_MODEL_CACHE_TTL: Duration = Duration::from_secs(30 * 60);
const RUNTIME_UNAVAILABLE_TTL: Duration = Duration::from_secs(10 * 60);
const PROVIDER_RUNTIME_UNAVAILABLE_TTL: Duration = Duration::from_secs(5 * 60);
const ACCOUNT_MODEL_REFRESH_RETRY_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountModelAvailabilityState {
    Available,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct AccountModelAvailability {
    pub state: AccountModelAvailabilityState,
    pub reason: Option<String>,
    pub source: &'static str,
    pub observed_at: Option<SystemTime>,
}

fn format_elapsed_duration_short(elapsed: Duration) -> String {
    if elapsed.as_secs() < 60 {
        format!("{}s", elapsed.as_secs())
    } else if elapsed.as_secs() < 3600 {
        format!("{}m", elapsed.as_secs() / 60)
    } else if elapsed.as_secs() < 86_400 {
        format!("{}h", elapsed.as_secs() / 3600)
    } else {
        format!("{}d", elapsed.as_secs() / 86_400)
    }
}

pub fn format_account_model_availability_detail(
    availability: &AccountModelAvailability,
) -> Option<String> {
    let base = match availability.state {
        AccountModelAvailabilityState::Available => return None,
        AccountModelAvailabilityState::Unavailable | AccountModelAvailabilityState::Unknown => {
            availability
                .reason
                .clone()
                .unwrap_or_else(|| "availability unknown".to_string())
        }
    };

    let mut meta_parts = vec![availability.source.to_string()];
    if let Some(observed_at) = availability.observed_at {
        if let Ok(elapsed) = SystemTime::now().duration_since(observed_at) {
            meta_parts.push(format!("{} ago", format_elapsed_duration_short(elapsed)));
        }
    }

    if meta_parts.is_empty() {
        Some(base)
    } else {
        Some(format!("{} ({})", base, meta_parts.join(", ")))
    }
}

fn normalize_model_id(model: &str) -> String {
    let normalized = model.trim().to_ascii_lowercase();
    normalized
        .strip_suffix("[1m]")
        .unwrap_or(&normalized)
        .to_string()
}

fn normalize_provider_id(provider: &str) -> String {
    provider.trim().to_ascii_lowercase()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCapabilities {
    pub provider: Option<String>,
    pub context_window: Option<usize>,
}

fn provider_key_from_hint(provider_hint: Option<&str>) -> Option<&'static str> {
    let normalized = normalize_provider_id(provider_hint?);
    match normalized.as_str() {
        "anthropic" | "claude" => Some("claude"),
        "openai" => Some("openai"),
        "openrouter" => Some("openrouter"),
        "copilot" | "github copilot" => Some("copilot"),
        _ => None,
    }
}

fn model_id_for_capability_lookup(model: &str, provider: Option<&str>) -> (String, bool) {
    let normalized = model.trim().to_ascii_lowercase();
    let (base, is_1m) = if let Some(base) = normalized.strip_suffix("[1m]") {
        (base.to_string(), true)
    } else {
        (normalized, false)
    };

    let lookup = if matches!(provider, Some("openrouter")) || base.contains('/') {
        base.rsplit('/').next().unwrap_or(&base).to_string()
    } else {
        base
    };

    (lookup, is_1m)
}

fn copilot_context_limit_for_model(model: &str) -> usize {
    match model {
        "claude-sonnet-4" | "claude-sonnet-4-6" | "claude-sonnet-4.6" => 128_000,
        "claude-opus-4-6" | "claude-opus-4.6" | "claude-opus-4.6-fast" => 200_000,
        "claude-opus-4.5" | "claude-opus-4-5" => 200_000,
        "claude-sonnet-4.5" | "claude-sonnet-4-5" => 200_000,
        "claude-haiku-4.5" | "claude-haiku-4-5" => 200_000,
        "gpt-4o" | "gpt-4o-mini" => 128_000,
        m if m.starts_with("gpt-4o") => 128_000,
        m if m.starts_with("gpt-4.1") => 128_000,
        m if m.starts_with("gpt-5") => 128_000,
        "o3-mini" | "o4-mini" => 128_000,
        m if m.starts_with("gemini-2.0-flash") => 1_000_000,
        m if m.starts_with("gemini-2.5") => 1_000_000,
        m if m.starts_with("gemini-3") => 1_000_000,
        _ => 128_000,
    }
}

fn fallback_context_limit_for_model(model: &str, provider_hint: Option<&str>) -> Option<usize> {
    let provider = provider_key_from_hint(provider_hint).or_else(|| provider_for_model(model));
    let (model, is_1m) = model_id_for_capability_lookup(model, provider);
    let model = model.as_str();

    if !matches!(provider, Some("claude" | "copilot")) {
        if let Some(limit) = get_cached_context_limit(model) {
            return Some(limit);
        }
    }

    if matches!(provider, Some("copilot")) {
        return Some(copilot_context_limit_for_model(model));
    }

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

    // GPT-5.4-family models should default to the long-context window.
    // The live Codex OAuth catalog can still override this via the dynamic cache above.
    if model.starts_with("gpt-5.4") {
        return Some(1_000_000);
    }

    // Most GPT-5.x codex/reasoning models: 272k per Codex backend API
    if model.starts_with("gpt-5") {
        return Some(272_000);
    }

    if model.starts_with("claude-opus-4-6") || model.starts_with("claude-opus-4.6") {
        return Some(if is_1m { 1_048_576 } else { 200_000 });
    }

    if model.starts_with("claude-sonnet-4-6") || model.starts_with("claude-sonnet-4.6") {
        return Some(if is_1m { 1_048_576 } else { 200_000 });
    }

    if model.starts_with("claude-opus-4-5") || model.starts_with("claude-opus-4.5") {
        return Some(200_000);
    }

    None
}

fn openai_static_model_ids() -> Vec<String> {
    let mut models: Vec<String> = ALL_OPENAI_MODELS.iter().map(|m| (*m).to_string()).collect();

    // Only advertise the explicit [1m] alias when the live catalog we fetched
    // says this backend exposes a >=1M context window for GPT-5.4.
    if get_cached_context_limit("gpt-5.4").unwrap_or_default() >= 1_000_000 {
        if let Some(index) = models.iter().position(|model| model == "gpt-5.4") {
            models.insert(index + 1, "gpt-5.4[1m]".to_string());
        } else {
            models.push("gpt-5.4[1m]".to_string());
        }
    }

    models
}

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
        let mut normalized = HashSet::new();
        for slug in slugs {
            let slug = normalize_model_id(&slug);
            if !slug.is_empty() {
                normalized.insert(slug);
            }
        }
        if normalized.is_empty() {
            return;
        }

        if let Ok(mut available) = ACCOUNT_AVAILABLE_MODELS.write() {
            let mut sorted: Vec<String> = normalized.iter().cloned().collect();
            sorted.sort();
            crate::logging::info(&format!("Account available models: {}", sorted.join(", ")));
            *available = Some(normalized.clone());
        }
        if let Ok(mut fetched_at) = ACCOUNT_AVAILABLE_MODELS_FETCHED_AT.write() {
            *fetched_at = Some(Instant::now());
        }
        if let Ok(mut observed_at) = ACCOUNT_AVAILABLE_MODELS_OBSERVED_AT.write() {
            *observed_at = Some(SystemTime::now());
        }
        if let Ok(mut last_attempt) = ACCOUNT_MODEL_REFRESH_LAST_ATTEMPT.write() {
            *last_attempt = Some(Instant::now());
        }
        if let Ok(mut unavailable) = ACCOUNT_RUNTIME_UNAVAILABLE_MODELS.write() {
            unavailable.retain(|model, _| !normalized.contains(model));
        }
        crate::bus::Bus::global().publish(crate::bus::BusEvent::ModelsUpdated);
    }
}

fn merge_openai_model_ids(dynamic_models: Vec<String>) -> Vec<String> {
    let mut models = openai_static_model_ids();
    let mut seen: HashSet<String> = models
        .iter()
        .map(|model| normalize_model_id(model))
        .collect();
    let mut extras = Vec::new();

    for model in dynamic_models {
        let trimmed = model.trim();
        if trimmed.is_empty() {
            continue;
        }

        let normalized = normalize_model_id(trimmed);
        if normalized.is_empty() || !seen.insert(normalized) {
            continue;
        }

        extras.push(trimmed.to_string());
    }

    extras.sort();
    models.extend(extras);
    models
}

pub fn known_openai_model_ids() -> Vec<String> {
    let dynamic_models = ACCOUNT_AVAILABLE_MODELS
        .read()
        .ok()
        .and_then(|cache| {
            cache
                .as_ref()
                .map(|models| models.iter().cloned().collect())
        })
        .unwrap_or_default();
    merge_openai_model_ids(dynamic_models)
}

pub fn note_openai_model_catalog_refresh_attempt() {
    if let Ok(mut last_attempt) = ACCOUNT_MODEL_REFRESH_LAST_ATTEMPT.write() {
        *last_attempt = Some(Instant::now());
    }
}

fn openai_model_catalog_refresh_throttled() -> bool {
    let Ok(last_attempt) = ACCOUNT_MODEL_REFRESH_LAST_ATTEMPT.read() else {
        return false;
    };

    last_attempt
        .as_ref()
        .map(|at| at.elapsed() < ACCOUNT_MODEL_REFRESH_RETRY_INTERVAL)
        .unwrap_or(false)
}

pub fn should_refresh_openai_model_catalog() -> bool {
    if account_model_cache_is_fresh() {
        return false;
    }
    if openai_model_catalog_refresh_throttled() {
        return false;
    }
    !ACCOUNT_MODEL_REFRESH_IN_FLIGHT.load(Ordering::Relaxed)
}

pub fn begin_openai_model_catalog_refresh() -> bool {
    if !should_refresh_openai_model_catalog() {
        return false;
    }
    if ACCOUNT_MODEL_REFRESH_IN_FLIGHT
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return false;
    }

    note_openai_model_catalog_refresh_attempt();
    true
}

pub fn finish_openai_model_catalog_refresh() {
    ACCOUNT_MODEL_REFRESH_IN_FLIGHT.store(false, Ordering::SeqCst);
}

fn account_model_cache_is_fresh() -> bool {
    let Ok(guard) = ACCOUNT_AVAILABLE_MODELS_FETCHED_AT.read() else {
        return false;
    };
    guard
        .as_ref()
        .map(|fetched_at| fetched_at.elapsed() <= ACCOUNT_MODEL_CACHE_TTL)
        .unwrap_or(false)
}

fn runtime_model_unavailability(model: &str) -> Option<RuntimeModelUnavailability> {
    let key = normalize_model_id(model);
    if key.is_empty() {
        return None;
    }

    let mut unavailable = ACCOUNT_RUNTIME_UNAVAILABLE_MODELS.write().ok()?;
    if let Some(entry) = unavailable.get(&key) {
        if entry.recorded_at.elapsed() <= RUNTIME_UNAVAILABLE_TTL {
            return Some(entry.clone());
        }
        unavailable.remove(&key);
    }
    None
}

fn account_snapshot_model_available(model: &str) -> Option<bool> {
    if !account_model_cache_is_fresh() {
        return None;
    }
    let key = normalize_model_id(model);
    if key.is_empty() {
        return None;
    }

    let cache = ACCOUNT_AVAILABLE_MODELS.read().ok()?;
    let models = cache.as_ref()?;
    Some(models.contains(&key))
}

fn account_models_observed_at() -> Option<SystemTime> {
    ACCOUNT_AVAILABLE_MODELS_OBSERVED_AT
        .read()
        .ok()
        .and_then(|v| *v)
}

pub fn refresh_openai_model_catalog_in_background(access_token: String, context: &'static str) {
    if access_token.trim().is_empty() {
        finish_openai_model_catalog_refresh();
        return;
    }

    tokio::spawn(async move {
        let refresh_result = fetch_openai_model_catalog(&access_token).await;
        match refresh_result {
            Ok(catalog)
                if !catalog.available_models.is_empty() || !catalog.context_limits.is_empty() =>
            {
                crate::logging::info(&format!(
                    "Refreshed OpenAI model catalog ({}): {} available, {} with context limits",
                    context,
                    catalog.available_models.len(),
                    catalog.context_limits.len()
                ));
                if !catalog.context_limits.is_empty() {
                    populate_context_limits(catalog.context_limits.clone());
                }
                if !catalog.available_models.is_empty() {
                    populate_account_models(catalog.available_models.clone());
                }
            }
            Ok(_) => {
                crate::logging::info(&format!(
                    "Codex models API refresh returned no model catalog data ({})",
                    context
                ));
            }
            Err(e) => {
                crate::logging::info(&format!(
                    "Failed to refresh OpenAI model catalog from Codex API ({}): {}",
                    context, e
                ));
            }
        }
        finish_openai_model_catalog_refresh();
    });
}

pub fn record_model_unavailable_for_account(model: &str, reason: &str) {
    let key = normalize_model_id(model);
    if key.is_empty() {
        return;
    }

    if let Ok(mut unavailable) = ACCOUNT_RUNTIME_UNAVAILABLE_MODELS.write() {
        unavailable.insert(
            key,
            RuntimeModelUnavailability {
                reason: reason.trim().to_string(),
                recorded_at: Instant::now(),
                observed_at: SystemTime::now(),
            },
        );
    }
}

pub fn clear_model_unavailable_for_account(model: &str) {
    let key = normalize_model_id(model);
    if key.is_empty() {
        return;
    }

    if let Ok(mut unavailable) = ACCOUNT_RUNTIME_UNAVAILABLE_MODELS.write() {
        unavailable.remove(&key);
    }
}

fn runtime_provider_unavailability(provider: &str) -> Option<RuntimeProviderUnavailability> {
    let key = normalize_provider_id(provider);
    if key.is_empty() {
        return None;
    }

    let mut unavailable = ACCOUNT_RUNTIME_UNAVAILABLE_PROVIDERS.write().ok()?;
    if let Some(entry) = unavailable.get(&key) {
        if entry.recorded_at.elapsed() <= PROVIDER_RUNTIME_UNAVAILABLE_TTL {
            return Some(entry.clone());
        }
        unavailable.remove(&key);
    }
    None
}

pub fn record_provider_unavailable_for_account(provider: &str, reason: &str) {
    let key = normalize_provider_id(provider);
    if key.is_empty() {
        return;
    }

    if let Ok(mut unavailable) = ACCOUNT_RUNTIME_UNAVAILABLE_PROVIDERS.write() {
        unavailable.insert(
            key,
            RuntimeProviderUnavailability {
                reason: reason.trim().to_string(),
                recorded_at: Instant::now(),
                observed_at: SystemTime::now(),
            },
        );
    }
}

pub fn clear_provider_unavailable_for_account(provider: &str) {
    let key = normalize_provider_id(provider);
    if key.is_empty() {
        return;
    }

    if let Ok(mut unavailable) = ACCOUNT_RUNTIME_UNAVAILABLE_PROVIDERS.write() {
        unavailable.remove(&key);
    }
}

/// Clear all runtime model unavailability markers.
pub fn clear_all_model_unavailability_for_account() {
    if let Ok(mut unavailable) = ACCOUNT_RUNTIME_UNAVAILABLE_MODELS.write() {
        unavailable.clear();
    }
}

/// Clear all runtime provider unavailability markers.
pub fn clear_all_provider_unavailability_for_account() {
    if let Ok(mut unavailable) = ACCOUNT_RUNTIME_UNAVAILABLE_PROVIDERS.write() {
        unavailable.clear();
    }
}

pub fn provider_unavailability_detail_for_account(provider: &str) -> Option<String> {
    let entry = runtime_provider_unavailability(provider)?;

    let mut detail = entry.reason;
    if let Ok(elapsed) = SystemTime::now().duration_since(entry.observed_at) {
        detail.push_str(&format!(
            " (runtime-error, {} ago)",
            format_elapsed_duration_short(elapsed)
        ));
    }

    Some(detail)
}

pub fn model_unavailability_detail_for_account(model: &str) -> Option<String> {
    let availability = model_availability_for_account(model);
    format_account_model_availability_detail(&availability)
}

/// Check if a model is available for the current account.
/// Returns None when availability is currently unknown (e.g. stale/missing snapshot).
/// Returns Some(true) when available and Some(false) when unavailable.
pub fn is_model_available_for_account(model: &str) -> Option<bool> {
    match model_availability_for_account(model).state {
        AccountModelAvailabilityState::Available => Some(true),
        AccountModelAvailabilityState::Unavailable => Some(false),
        AccountModelAvailabilityState::Unknown => None,
    }
}

pub fn model_availability_for_account(model: &str) -> AccountModelAvailability {
    if let Some(runtime) = runtime_model_unavailability(model) {
        return AccountModelAvailability {
            state: AccountModelAvailabilityState::Unavailable,
            reason: Some(runtime.reason),
            source: "runtime-error",
            observed_at: Some(runtime.observed_at),
        };
    }

    if !account_model_cache_is_fresh() {
        return AccountModelAvailability {
            state: AccountModelAvailabilityState::Unknown,
            reason: Some("availability snapshot is stale".to_string()),
            source: "account-snapshot",
            observed_at: account_models_observed_at(),
        };
    }

    match account_snapshot_model_available(model) {
        Some(true) => AccountModelAvailability {
            state: AccountModelAvailabilityState::Available,
            reason: None,
            source: "account-snapshot",
            observed_at: account_models_observed_at(),
        },
        Some(false) => AccountModelAvailability {
            state: AccountModelAvailabilityState::Unavailable,
            reason: Some("not available for your account".to_string()),
            source: "account-snapshot",
            observed_at: account_models_observed_at(),
        },
        None => AccountModelAvailability {
            state: AccountModelAvailabilityState::Unknown,
            reason: Some("no availability snapshot yet".to_string()),
            source: "account-snapshot",
            observed_at: account_models_observed_at(),
        },
    }
}

/// Preferred model order for fallback selection.
/// If the desired model isn't available, we try these in order.
const OPENAI_MODEL_PREFERENCE: &[&str] = &[
    "gpt-5.4",
    "gpt-5.3-codex-spark",
    "gpt-5.3-codex",
    "gpt-5.2-codex",
    "gpt-5.1-codex-max",
    "gpt-5.1-codex",
];

/// Get the best available OpenAI model, falling back through the preference list.
/// Returns None if the dynamic model list hasn't been fetched yet.
pub fn get_best_available_openai_model() -> Option<String> {
    if !account_model_cache_is_fresh() {
        return None;
    }
    let cache = ACCOUNT_AVAILABLE_MODELS.read().ok()?;
    let models = cache.as_ref()?;

    for preferred in OPENAI_MODEL_PREFERENCE {
        if models.contains(*preferred) && runtime_model_unavailability(preferred).is_none() {
            return Some(preferred.to_string());
        }
    }

    let mut sorted_models: Vec<String> = models.iter().cloned().collect();
    sorted_models.sort();
    sorted_models
        .into_iter()
        .find(|model| runtime_model_unavailability(model).is_none())
}

#[derive(Debug, Clone, Default)]
pub struct OpenAIModelCatalog {
    pub available_models: Vec<String>,
    pub context_limits: HashMap<String, usize>,
}

fn parse_openai_model_catalog(data: &serde_json::Value) -> OpenAIModelCatalog {
    let models = data
        .get("models")
        .and_then(|m| m.as_array())
        .or_else(|| {
            data.get("data")
                .and_then(|d| d.get("models"))
                .and_then(|m| m.as_array())
        })
        .or_else(|| data.get("data").and_then(|d| d.as_array()))
        .or_else(|| data.as_array());

    let mut available: HashSet<String> = HashSet::new();
    let mut limits: HashMap<String, usize> = HashMap::new();

    for model in models.into_iter().flatten() {
        let Some(slug) = model
            .get("slug")
            .or_else(|| model.get("id"))
            .or_else(|| model.get("model"))
            .and_then(|s| s.as_str())
        else {
            continue;
        };

        let slug = normalize_model_id(slug);
        if slug.is_empty() {
            continue;
        }

        available.insert(slug.clone());

        if let Some(ctx) = model
            .get("context_window")
            .or_else(|| model.get("context_length"))
            .and_then(|c| c.as_u64())
        {
            limits.insert(slug, ctx as usize);
        }
    }

    let mut available_models: Vec<String> = available.into_iter().collect();
    available_models.sort();

    OpenAIModelCatalog {
        available_models,
        context_limits: limits,
    }
}

/// Fetch model availability and context windows from the Codex backend API.
pub async fn fetch_openai_model_catalog(access_token: &str) -> Result<OpenAIModelCatalog> {
    note_openai_model_catalog_refresh_attempt();

    let client = shared_http_client();
    let resp = client
        .get("https://chatgpt.com/backend-api/codex/models?client_version=1.0.0")
        .header("Authorization", format!("Bearer {}", access_token))
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("Failed to fetch model context limits: {}", resp.status());
    }

    let data: serde_json::Value = resp.json().await?;
    Ok(parse_openai_model_catalog(&data))
}

/// Fetch context window sizes from the Codex backend API.
/// Returns a map of model slug -> context_window tokens.
pub async fn fetch_openai_context_limits(access_token: &str) -> Result<HashMap<String, usize>> {
    Ok(fetch_openai_model_catalog(access_token)
        .await?
        .context_limits)
}

/// Return the context window size in tokens for a given model, if known.
///
/// First checks the dynamic cache (populated from the Codex backend API at startup),
/// then falls back to hardcoded defaults.
pub fn context_limit_for_model(model: &str) -> Option<usize> {
    context_limit_for_model_with_provider(model, None)
}

pub fn context_limit_for_model_with_provider(
    model: &str,
    provider_hint: Option<&str>,
) -> Option<usize> {
    fallback_context_limit_for_model(model, provider_hint)
}

pub fn resolve_model_capabilities(model: &str, provider_hint: Option<&str>) -> ModelCapabilities {
    let provider = provider_for_model_with_hint(model, provider_hint).map(str::to_string);
    let context_window = context_limit_for_model_with_provider(model, provider_hint);
    ModelCapabilities {
        provider,
        context_window,
    }
}

/// Normalize a Copilot-style model name to the canonical form used by our
/// provider model lists. Copilot uses dots in version numbers (e.g.
/// `claude-opus-4.6`) while our canonical lists use hyphens (`claude-opus-4-6`).
/// Returns None if no normalization is needed (model already canonical or unknown).
fn normalize_copilot_model_name(model: &str) -> Option<&'static str> {
    for canonical in ALL_CLAUDE_MODELS.iter().chain(ALL_OPENAI_MODELS.iter()) {
        if *canonical == model {
            return None;
        }
    }
    let normalized = model.replace('.', "-");
    for canonical in ALL_CLAUDE_MODELS.iter().chain(ALL_OPENAI_MODELS.iter()) {
        if *canonical == normalized {
            return Some(canonical);
        }
    }
    None
}

/// Detect which provider a model belongs to
pub fn provider_for_model_with_hint(
    model: &str,
    provider_hint: Option<&str>,
) -> Option<&'static str> {
    if let Some(provider) = provider_key_from_hint(provider_hint) {
        return Some(provider);
    }

    let model = model.trim();
    if ALL_CLAUDE_MODELS.contains(&model) {
        Some("claude")
    } else if ALL_OPENAI_MODELS.contains(&model) {
        Some("openai")
    } else if model.contains('/') {
        Some("openrouter")
    } else if model.starts_with("claude-") {
        Some("claude")
    } else if model.starts_with("gpt-") {
        Some("openai")
    } else {
        None
    }
}

/// Detect which provider a model belongs to
pub fn provider_for_model(model: &str) -> Option<&'static str> {
    provider_for_model_with_hint(model, None)
}

/// MultiProvider wraps multiple providers and allows seamless model switching
pub struct MultiProvider {
    /// Claude Code CLI provider
    claude: Option<claude::ClaudeProvider>,
    /// Direct Anthropic API provider (no Python dependency)
    anthropic: Option<anthropic::AnthropicProvider>,
    openai: Option<openai::OpenAIProvider>,
    /// GitHub Copilot API provider (direct API, hot-swappable after login)
    copilot_api: RwLock<Option<Arc<copilot::CopilotApiProvider>>>,
    /// OpenRouter API provider (200+ models from various providers)
    openrouter: Option<openrouter::OpenRouterProvider>,
    active: RwLock<ActiveProvider>,
    has_claude_creds: bool,
    has_openai_creds: bool,
    has_openrouter_creds: bool,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FailoverDecision {
    None,
    RetryNextProvider,
    RetryAndMarkUnavailable,
}

impl FailoverDecision {
    fn should_failover(self) -> bool {
        !matches!(self, Self::None)
    }

    fn should_mark_provider_unavailable(self) -> bool {
        matches!(self, Self::RetryAndMarkUnavailable)
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::RetryNextProvider => "retry-next-provider",
            Self::RetryAndMarkUnavailable => "retry-and-mark-unavailable",
        }
    }
}

impl MultiProvider {
    fn auto_default_provider(
        openai: bool,
        claude: bool,
        copilot: bool,
        openrouter: bool,
        copilot_premium_zero: bool,
    ) -> ActiveProvider {
        if copilot_premium_zero && copilot {
            ActiveProvider::Copilot
        } else if openai {
            ActiveProvider::OpenAI
        } else if claude {
            ActiveProvider::Claude
        } else if copilot {
            ActiveProvider::Copilot
        } else if openrouter {
            ActiveProvider::OpenRouter
        } else {
            ActiveProvider::Claude
        }
    }

    fn parse_provider_hint(value: &str) -> Option<ActiveProvider> {
        match value.trim().to_ascii_lowercase().as_str() {
            "claude" | "anthropic" => Some(ActiveProvider::Claude),
            "openai" => Some(ActiveProvider::OpenAI),
            "copilot" => Some(ActiveProvider::Copilot),
            "openrouter" => Some(ActiveProvider::OpenRouter),
            _ => None,
        }
    }

    fn forced_provider_from_env() -> Option<ActiveProvider> {
        let force = std::env::var("JCODE_FORCE_PROVIDER")
            .ok()
            .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
            .unwrap_or(false);
        if !force {
            return None;
        }

        std::env::var("JCODE_ACTIVE_PROVIDER")
            .ok()
            .and_then(|value| Self::parse_provider_hint(&value))
    }

    fn fallback_sequence_for(
        active: ActiveProvider,
        forced_provider: Option<ActiveProvider>,
    ) -> Vec<ActiveProvider> {
        if let Some(forced) = forced_provider {
            if active == forced {
                vec![forced]
            } else {
                vec![active, forced]
            }
        } else {
            Self::fallback_sequence(active)
        }
    }

    fn provider_label(provider: ActiveProvider) -> &'static str {
        match provider {
            ActiveProvider::Claude => "Anthropic",
            ActiveProvider::OpenAI => "OpenAI",
            ActiveProvider::Copilot => "GitHub Copilot",
            ActiveProvider::OpenRouter => "OpenRouter",
        }
    }

    fn provider_key(provider: ActiveProvider) -> &'static str {
        match provider {
            ActiveProvider::Claude => "claude",
            ActiveProvider::OpenAI => "openai",
            ActiveProvider::Copilot => "copilot",
            ActiveProvider::OpenRouter => "openrouter",
        }
    }

    fn set_active_provider(&self, provider: ActiveProvider) {
        *self.active.write().unwrap() = provider;
    }

    fn provider_is_configured(&self, provider: ActiveProvider) -> bool {
        match provider {
            ActiveProvider::Claude => self.anthropic.is_some() || self.claude.is_some(),
            ActiveProvider::OpenAI => self.openai.is_some(),
            ActiveProvider::Copilot => self.copilot_api.read().unwrap().is_some(),
            ActiveProvider::OpenRouter => self.openrouter.is_some(),
        }
    }

    fn provider_precheck_unavailable_reason(&self, provider: ActiveProvider) -> Option<String> {
        match provider {
            ActiveProvider::Claude if self.is_claude_usage_exhausted() => {
                Some("OAuth usage exhausted".to_string())
            }
            ActiveProvider::OpenAI if self.is_openai_usage_exhausted() => {
                Some("OAuth usage exhausted".to_string())
            }
            _ => None,
        }
    }

    fn fallback_sequence(active: ActiveProvider) -> Vec<ActiveProvider> {
        match active {
            ActiveProvider::Claude => {
                vec![
                    ActiveProvider::Claude,
                    ActiveProvider::OpenAI,
                    ActiveProvider::Copilot,
                    ActiveProvider::OpenRouter,
                ]
            }
            ActiveProvider::OpenAI => {
                vec![
                    ActiveProvider::OpenAI,
                    ActiveProvider::Claude,
                    ActiveProvider::Copilot,
                    ActiveProvider::OpenRouter,
                ]
            }
            ActiveProvider::Copilot => {
                vec![
                    ActiveProvider::Copilot,
                    ActiveProvider::Claude,
                    ActiveProvider::OpenAI,
                    ActiveProvider::OpenRouter,
                ]
            }
            ActiveProvider::OpenRouter => {
                vec![
                    ActiveProvider::OpenRouter,
                    ActiveProvider::Claude,
                    ActiveProvider::OpenAI,
                    ActiveProvider::Copilot,
                ]
            }
        }
    }

    fn summarize_error(err: &anyhow::Error) -> String {
        err.to_string()
            .lines()
            .next()
            .unwrap_or("unknown error")
            .trim()
            .to_string()
    }

    fn contains_standalone_status_code(haystack: &str, code: &str) -> bool {
        let haystack_bytes = haystack.as_bytes();
        let code_len = code.len();

        haystack.match_indices(code).any(|(start, _)| {
            let before_ok = start == 0 || !haystack_bytes[start - 1].is_ascii_digit();
            let end = start + code_len;
            let after_ok = end == haystack_bytes.len() || !haystack_bytes[end].is_ascii_digit();
            before_ok && after_ok
        })
    }

    fn classify_failover_error(err: &anyhow::Error) -> FailoverDecision {
        let lower = err.to_string().to_ascii_lowercase();

        let request_size_or_context = [
            "context length",
            "context_length",
            "context window",
            "maximum context",
            "prompt is too long",
            "input is too long",
            "too many tokens",
            "max tokens",
            "token limit",
            "token_limit",
            "request too large",
            "payload too large",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
            || Self::contains_standalone_status_code(&lower, "413");

        if request_size_or_context {
            // Request-specific: retry other providers, but do not blacklist this provider.
            return FailoverDecision::RetryNextProvider;
        }

        let quota_or_limit = [
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
        ]
        .iter()
        .any(|needle| lower.contains(needle))
            || Self::contains_standalone_status_code(&lower, "429")
            || Self::contains_standalone_status_code(&lower, "402");

        if quota_or_limit {
            return FailoverDecision::RetryAndMarkUnavailable;
        }

        let auth_or_availability = [
            "credentials not available",
            "token expired",
            "re-authenticate",
            "unauthorized",
            "forbidden",
            "not available for your account",
            "not accessible by integration",
            "feature_flag_blocked",
            "contact support",
            "token exchange failed",
            "account suspended",
            "account disabled",
            "access denied",
            "permission denied",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
            || Self::contains_standalone_status_code(&lower, "401")
            || Self::contains_standalone_status_code(&lower, "403");

        if auth_or_availability {
            return FailoverDecision::RetryAndMarkUnavailable;
        }

        FailoverDecision::None
    }

    fn no_provider_available_error(notes: &[String]) -> anyhow::Error {
        let mut msg = "No tokens/providers left: no usable provider right now. Anthropic/OpenAI usage may be exhausted and GitHub Copilot is not authenticated or currently unavailable.".to_string();
        if !notes.is_empty() {
            msg.push_str(" Details: ");
            msg.push_str(&notes.join(" | "));
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

        for candidate in sequence {
            let label = Self::provider_label(candidate);
            let key = Self::provider_key(candidate);

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
                    let summary = Self::summarize_error(&err);
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
                    } else {
                        return Err(err);
                    }
                }
            }
        }

        Err(Self::no_provider_available_error(&notes))
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
                    Err(anyhow::anyhow!("OpenAI credentials not available. Run `jcode login --provider openai` to log in."))
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
                    Err(anyhow::anyhow!("OpenAI credentials not available. Run `jcode login --provider openai` to log in."))
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
            ActiveProvider::OpenRouter => {
                if let Some(ref openrouter) = self.openrouter {
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
        let has_claude_creds = auth::claude::load_credentials().is_ok();
        let has_openai_creds = auth::codex::load_credentials().is_ok();
        let auth_status = auth::AuthStatus::check();
        let has_copilot_api = auth_status.copilot_has_api_token;
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

        // Default to OAuth/CLI providers first. Keep direct API (OpenRouter) lowest.
        // When Copilot premium mode is Zero (free requests), prefer Copilot over
        // paid OAuth providers so we don't spend money unnecessarily.
        let copilot_premium_zero = matches!(
            std::env::var("JCODE_COPILOT_PREMIUM").ok().as_deref(),
            Some("0")
        );
        let mut active = Self::auto_default_provider(
            openai.is_some(),
            claude.is_some() || anthropic.is_some(),
            copilot_api.is_some(),
            openrouter.is_some(),
            copilot_premium_zero,
        );

        if copilot_premium_zero && matches!(active, ActiveProvider::Copilot) {
            crate::logging::info(
                "Copilot premium mode is Zero (free requests) - defaulting to Copilot provider",
            );
        }

        let forced_provider = Self::forced_provider_from_env();

        // Apply configured default_provider from config/env if the provider is available,
        // unless CLI/provider lock explicitly forced one via env.
        let cfg = crate::config::config();
        if let Some(forced) = forced_provider {
            active = forced;
            let is_configured = match forced {
                ActiveProvider::Claude => claude.is_some() || anthropic.is_some(),
                ActiveProvider::OpenAI => openai.is_some(),
                ActiveProvider::Copilot => copilot_api.is_some(),
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
                    "Unknown default_provider '{}' in config (expected: claude|openai|copilot|openrouter)",
                    pref
                ));
            }
        }

        let result = Self {
            claude,
            anthropic,
            openai,
            copilot_api: RwLock::new(copilot_api),
            openrouter,
            active: RwLock::new(active),
            has_claude_creds,
            has_openai_creds,
            has_openrouter_creds,
            use_claude_cli,
            startup_notices: RwLock::new(Vec::new()),
            forced_provider,
        };

        // Apply configured default_model from config/env
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

        // Prime OpenAI model/account availability in the background.
        result.spawn_openai_catalog_refresh_if_needed();

        // Try to auto-rotate Anthropic accounts if current one is exhausted.
        result.auto_select_anthropic_account();

        result
    }

    /// Create with explicit initial provider preference
    pub fn with_preference(prefer_openai: bool) -> Self {
        let provider = Self::new();
        if provider.forced_provider.is_none() && prefer_openai && provider.openai.is_some() {
            *provider.active.write().unwrap() = ActiveProvider::OpenAI;
        }
        provider
    }

    fn active_provider(&self) -> ActiveProvider {
        *self.active.read().unwrap()
    }

    /// Try to select a non-exhausted Anthropic account for this session.
    /// Called at session start. Uses cached usage data - if no data is available yet,
    /// does nothing (the per-request fallback handles it later).
    /// Only rotates between Anthropic OAuth accounts, never switches to API billing.
    pub fn auto_select_anthropic_account(&self) {
        if self.active_provider() != ActiveProvider::Claude {
            return;
        }
        if self.anthropic.is_none() && self.claude.is_none() {
            return;
        }

        let accounts = match crate::auth::claude::list_accounts() {
            Ok(a) if a.len() > 1 => a,
            _ => return,
        };

        let usage = crate::usage::get_sync();
        let is_exhausted = usage.five_hour >= 0.99 && usage.seven_day >= 0.99;
        if !is_exhausted {
            return;
        }
        if usage.fetched_at.is_none() {
            return;
        }

        let current_label =
            crate::auth::claude::active_account_label().unwrap_or_else(|| "default".to_string());

        crate::logging::info(&format!(
            "Anthropic account '{}' is exhausted (5h: {:.0}%, 7d: {:.0}%), checking other accounts...",
            current_label,
            usage.five_hour * 100.0,
            usage.seven_day * 100.0,
        ));

        for account in &accounts {
            if account.label == current_label {
                continue;
            }

            if let Ok(other_usage) = crate::usage::fetch_usage_for_account_sync(
                &account.access,
                &account.refresh,
                account.expires,
            ) {
                let other_exhausted =
                    other_usage.five_hour >= 0.99 && other_usage.seven_day >= 0.99;
                if !other_exhausted {
                    crate::logging::info(&format!(
                        "Switching to Anthropic account '{}' (5h: {:.0}%, 7d: {:.0}%)",
                        account.label,
                        other_usage.five_hour * 100.0,
                        other_usage.seven_day * 100.0,
                    ));
                    crate::auth::claude::set_active_account_override(Some(account.label.clone()));
                    if let Some(ref anthropic) = self.anthropic {
                        let _ = tokio::task::block_in_place(|| {
                            tokio::runtime::Handle::current()
                                .block_on(anthropic.invalidate_credentials())
                        });
                    }
                    let notice = format!(
                        "⚡ Auto-switched Anthropic account: **{}** -> **{}** (previous account exhausted)",
                        current_label, account.label
                    );
                    self.startup_notices.write().unwrap().push(notice);
                    return;
                } else {
                    crate::logging::info(&format!(
                        "Anthropic account '{}' also exhausted (5h: {:.0}%, 7d: {:.0}%)",
                        account.label,
                        other_usage.five_hour * 100.0,
                        other_usage.seven_day * 100.0,
                    ));
                }
            }
        }

        crate::logging::info("All Anthropic accounts are exhausted");
        let notice =
            "⚠ All Anthropic accounts exhausted - will fall back to other providers if available"
                .to_string();
        self.startup_notices.write().unwrap().push(notice);
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
            ActiveProvider::OpenRouter => self
                .openrouter
                .as_ref()
                .map(|o| o.model())
                .unwrap_or_else(|| "anthropic/claude-sonnet-4".to_string()),
        }
    }

    fn set_model(&self, model: &str) -> Result<()> {
        self.spawn_openai_catalog_refresh_if_needed();

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
                    "openrouter" => ActiveProvider::OpenRouter,
                    _ => forced,
                };
                if target_active != forced {
                    let has_target_creds = match target_active {
                        ActiveProvider::Claude => self.claude.is_some() || self.anthropic.is_some(),
                        ActiveProvider::OpenAI => self.openai.is_some(),
                        ActiveProvider::OpenRouter => self.openrouter.is_some(),
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
                ActiveProvider::Copilot => {
                    let copilot_guard = self.copilot_api.read().unwrap();
                    if let Some(ref copilot) = *copilot_guard {
                        copilot.set_model(model)
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

    fn preferred_provider(&self) -> Option<String> {
        if let Some(ref openrouter) = self.openrouter {
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
        for model in ALL_CLAUDE_MODELS {
            let is_1m = model.ends_with("[1m]");
            let is_opus = model.contains("opus");

            let (available, detail) = if is_1m && !crate::usage::has_extra_usage() {
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
                    cheapness: Some(anthropic_oauth_pricing(model)),
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
                    cheapness: anthropic_api_pricing(model),
                });
            }
            if !has_oauth && !has_api_key {
                routes.push(ModelRoute {
                    model: model.to_string(),
                    provider: "Anthropic".to_string(),
                    api_method: "claude-oauth".to_string(),
                    available: false,
                    detail: "no credentials".to_string(),
                    cheapness: Some(anthropic_oauth_pricing(model)),
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
            let cheapness = if openai_effective_auth_mode() == "api-key" {
                openai_api_pricing(&model).unwrap_or_else(|| openai_oauth_pricing(&model))
            } else {
                openai_oauth_pricing(&model)
            };
            routes.push(ModelRoute {
                model,
                provider: "OpenAI".to_string(),
                api_method: "openai-oauth".to_string(),
                available,
                detail,
                cheapness: Some(cheapness),
            });
        }

        // GitHub Copilot models
        {
            let copilot_guard = self.copilot_api.read().unwrap();
            if let Some(ref copilot) = *copilot_guard {
                let copilot_models = copilot.available_models_display();
                for model in copilot_models {
                    let cheapness = copilot_pricing(&model);
                    routes.push(ModelRoute {
                        model,
                        provider: "Copilot".to_string(),
                        api_method: "copilot".to_string(),
                        available: true,
                        detail: String::new(),
                        cheapness: Some(cheapness),
                    });
                }
            } else if copilot::CopilotApiProvider::has_credentials() {
                routes.push(ModelRoute {
                    model: "copilot models".to_string(),
                    provider: "Copilot".to_string(),
                    api_method: "copilot".to_string(),
                    available: false,
                    detail: "not initialized yet".to_string(),
                    cheapness: Some(copilot_pricing("claude-sonnet-4-6")),
                });
            }
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
                let auto_cheapness = openrouter_route_pricing(&model, "auto");
                routes.push(ModelRoute {
                    model: model.clone(),
                    provider: "auto".to_string(),
                    api_method: "openrouter".to_string(),
                    available: self.has_openrouter_creds,
                    detail: auto_detail,
                    cheapness: auto_cheapness,
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
                        cheapness: openrouter_route_pricing(&or_model, "Anthropic"),
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
                        cheapness: openrouter_route_pricing(&or_model, "OpenAI"),
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

    fn on_auth_changed(&self) {
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
    }

    async fn invalidate_credentials(&self) {
        if let Some(ref anthropic) = self.anthropic {
            anthropic.invalidate_credentials().await;
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
            ActiveProvider::OpenRouter => false, // jcode executes tools
        }
    }

    fn reasoning_effort(&self) -> Option<String> {
        match self.active_provider() {
            ActiveProvider::Claude => None,
            ActiveProvider::OpenAI => self.openai.as_ref().and_then(|o| o.reasoning_effort()),
            ActiveProvider::Copilot => None,
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
            _ => vec![],
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
            ActiveProvider::OpenRouter => self
                .openrouter
                .as_ref()
                .map(|o| o.supports_compaction())
                .unwrap_or(false),
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
        let openrouter = if self.openrouter.is_some() {
            openrouter::OpenRouterProvider::new().ok()
        } else {
            None
        };

        let provider = Self {
            claude,
            anthropic,
            openai,
            copilot_api: RwLock::new(copilot_api),
            openrouter,
            active: RwLock::new(active),
            has_claude_creds: self.has_claude_creds,
            has_openai_creds: self.has_openai_creds,
            has_openrouter_creds: self.has_openrouter_creds,
            use_claude_cli: self.use_claude_cli,
            startup_notices: RwLock::new(Vec::new()),
            forced_provider: self.forced_provider,
        };

        provider.spawn_openai_catalog_refresh_if_needed();
        if matches!(active, ActiveProvider::Copilot) {
            let _ = provider.set_model(&format!("copilot:{}", current_model));
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
            ActiveProvider::OpenRouter => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    }

    #[test]
    fn test_normalize_model_id_strips_1m_suffix() {
        assert_eq!(normalize_model_id("gpt-5.4[1m]"), "gpt-5.4");
        assert_eq!(normalize_model_id(" GPT-5.4[1M] "), "gpt-5.4");
    }

    #[test]
    fn test_merge_openai_model_ids_appends_dynamic_oauth_models() {
        let models = merge_openai_model_ids(vec![
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
    fn test_context_limit_claude() {
        assert_eq!(context_limit_for_model("claude-opus-4-6"), Some(200_000));
        assert_eq!(context_limit_for_model("claude-sonnet-4-6"), Some(200_000));
        assert_eq!(
            context_limit_for_model("claude-opus-4-6[1m]"),
            Some(1_048_576)
        );
        assert_eq!(
            context_limit_for_model("claude-sonnet-4-6[1m]"),
            Some(1_048_576)
        );
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
                ActiveProvider::OpenRouter,
            ]
        );
        assert_eq!(
            MultiProvider::fallback_sequence(ActiveProvider::OpenAI),
            vec![
                ActiveProvider::OpenAI,
                ActiveProvider::Claude,
                ActiveProvider::Copilot,
                ActiveProvider::OpenRouter,
            ]
        );
        assert_eq!(
            MultiProvider::fallback_sequence(ActiveProvider::Copilot),
            vec![
                ActiveProvider::Copilot,
                ActiveProvider::Claude,
                ActiveProvider::OpenAI,
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
            MultiProvider::parse_provider_hint("openrouter"),
            Some(ActiveProvider::OpenRouter)
        );
        assert_eq!(MultiProvider::parse_provider_hint("cursor"), None);
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
        let provider = MultiProvider {
            claude: None,
            anthropic: None,
            openai: None,
            copilot_api: RwLock::new(None),
            openrouter: None,
            active: RwLock::new(ActiveProvider::OpenAI),
            has_claude_creds: false,
            has_openai_creds: false,
            has_openrouter_creds: false,
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
        let active = MultiProvider::auto_default_provider(true, true, false, false, false);
        assert_eq!(active, ActiveProvider::OpenAI);
    }

    #[test]
    fn test_auto_default_prefers_copilot_when_zero_premium_mode_enabled() {
        let active = MultiProvider::auto_default_provider(true, true, true, true, true);
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
        let err = MultiProvider::no_provider_available_error(&[
            "OpenAI: rate limited".to_string(),
            "GitHub Copilot: not configured".to_string(),
        ]);
        let text = err.to_string();
        assert!(text.contains("No tokens/providers left"));
        assert!(text.contains("OpenAI: rate limited"));
        assert!(text.contains("GitHub Copilot: not configured"));
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
}
