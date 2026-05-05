pub mod catalog_refresh;
pub mod failover;
pub mod models;
pub mod openai_schema;
pub mod pricing;
pub mod selection;

pub use catalog_refresh::{ModelCatalogRefreshSummary, summarize_model_catalog_refresh};
pub use failover::{
    FailoverDecision, ProviderFailoverPrompt, classify_failover_error_message,
    parse_failover_prompt_message,
};
pub use models::{
    ALL_CLAUDE_MODELS, ALL_OPENAI_MODELS, DEFAULT_CONTEXT_LIMIT, ModelCapabilities,
    context_limit_for_model, context_limit_for_model_with_provider,
    context_limit_for_model_with_provider_and_cache, normalize_copilot_model_name,
    provider_for_model as core_provider_for_model,
    provider_for_model_with_hint as core_provider_for_model_with_hint, provider_key_from_hint,
};
pub use selection::{
    ActiveProvider, ProviderAvailability, auto_default_provider, fallback_sequence,
    parse_provider_hint, provider_key, provider_label,
};

use serde::{Deserialize, Serialize};
use std::time::Duration;

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

/// Shared HTTP client for all providers. Creating a `reqwest::Client` is expensive
/// (~10ms due to TLS init, connection pool setup), so we reuse a single instance.
pub fn shared_http_client() -> reqwest::Client {
    use std::sync::OnceLock;
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(15))
                .tcp_keepalive(Some(Duration::from_secs(30)))
                .pool_idle_timeout(Duration::from_secs(90))
                .pool_max_idle_per_host(8)
                .build()
                .unwrap_or_else(|err| {
                    eprintln!("jcode: failed to build shared provider HTTP client: {err}");
                    reqwest::Client::new()
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
}
