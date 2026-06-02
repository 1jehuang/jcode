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

    /// Human-readable description of the auth method the active provider will
    /// actually use for the next request (e.g. "OAuth" or "API key"), or `None`
    /// when there is no meaningful OAuth-vs-API-key distinction. UI surfaces use
    /// this to report the auth method accurately instead of inferring it from
    /// which credentials happen to be configured.
    fn active_auth_method_label(&self) -> Option<&'static str> {
        None
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

    /// Select a structured model route.
    ///
    /// Most single-runtime providers can treat this as `set_model(model)`. Provider
    /// orchestrators should override this to activate the exact runtime identified
    /// by [`RouteSelection::runtime_key`] instead of reparsing a lossy model string.
    fn set_route_selection(&self, selection: &RouteSelection) -> Result<()> {
        self.set_model(&selection.model)
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

/// Exact runtime identity for a selected model route.
///
/// A runtime key identifies the concrete endpoint/auth/account slot that will
/// send requests. It is intentionally more precise than a display provider
/// label: for example, OpenRouter and NVIDIA NIM both speak an OpenAI-compatible
/// protocol, but they must have different runtime keys because they use
/// different endpoints, auth, catalogs, and routing semantics.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RuntimeKey {
    ClaudeOAuth,
    AnthropicApiKey,
    OpenAIOAuth,
    OpenAIApiKey,
    OpenRouter,
    OpenAiCompatible {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile_id: Option<String>,
    },
    Copilot,
    Gemini,
    Cursor,
    Bedrock,
    Antigravity,
    CodeAssistOAuth,
    RemoteCatalog,
    Current,
    Other(String),
}

impl RuntimeKey {
    pub fn from_api_method(api_method: &ModelRouteApiMethod, _provider_label: &str) -> Self {
        match api_method {
            ModelRouteApiMethod::ClaudeOAuth => Self::ClaudeOAuth,
            ModelRouteApiMethod::AnthropicApiKey => Self::AnthropicApiKey,
            ModelRouteApiMethod::OpenAIOAuth => Self::OpenAIOAuth,
            ModelRouteApiMethod::OpenAIApiKey => Self::OpenAIApiKey,
            ModelRouteApiMethod::OpenRouter => Self::OpenRouter,
            ModelRouteApiMethod::OpenAiCompatible { profile_id } => Self::OpenAiCompatible {
                profile_id: profile_id.clone(),
            },
            ModelRouteApiMethod::Copilot => Self::Copilot,
            ModelRouteApiMethod::Cursor => Self::Cursor,
            ModelRouteApiMethod::Bedrock => Self::Bedrock,
            ModelRouteApiMethod::CodeAssistOAuth => Self::CodeAssistOAuth,
            ModelRouteApiMethod::AntigravityHttps => Self::Antigravity,
            ModelRouteApiMethod::RemoteCatalog => Self::RemoteCatalog,
            ModelRouteApiMethod::Current => Self::Current,
            ModelRouteApiMethod::Other(method) => Self::Other(method.clone()),
        }
    }

    pub fn stable_id(&self) -> String {
        match self {
            Self::ClaudeOAuth => "claude-oauth".to_string(),
            Self::AnthropicApiKey => "anthropic-api-key".to_string(),
            Self::OpenAIOAuth => "openai-oauth".to_string(),
            Self::OpenAIApiKey => "openai-api-key".to_string(),
            Self::OpenRouter => "openrouter".to_string(),
            Self::OpenAiCompatible { profile_id } => profile_id
                .as_deref()
                .map(|profile_id| format!("openai-compatible:{profile_id}"))
                .unwrap_or_else(|| "openai-compatible".to_string()),
            Self::Copilot => "copilot".to_string(),
            Self::Gemini => "gemini".to_string(),
            Self::Cursor => "cursor".to_string(),
            Self::Bedrock => "bedrock".to_string(),
            Self::Antigravity => "antigravity".to_string(),
            Self::CodeAssistOAuth => "code-assist-oauth".to_string(),
            Self::RemoteCatalog => "remote-catalog".to_string(),
            Self::Current => "current".to_string(),
            Self::Other(value) => value.clone(),
        }
    }
}

/// Structured model route selection.
///
/// This is the internal source of truth for picker/RPC driven model selection.
/// Human string specs such as `openai-api:gpt-5` should be parsed into this type
/// at the command boundary instead of being used as the runtime identity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RouteSelection {
    pub model: String,
    pub runtime_key: RuntimeKey,
    pub api_method: String,
    pub provider_label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

impl RouteSelection {
    pub fn from_model_route(route: &ModelRoute) -> Self {
        let api_method = route.api_method_kind();
        Self {
            model: route.model.clone(),
            runtime_key: RuntimeKey::from_api_method(&api_method, &route.provider),
            api_method: route.api_method.clone(),
            provider_label: route.provider.clone(),
            detail: route.detail.clone(),
        }
    }
}

/// Typed view of [`ModelRoute::api_method`].
///
/// The wire format intentionally remains a string so older clients and saved
/// catalogs continue to round-trip, but routing/picker code should parse it at
/// module boundaries instead of scattering string comparisons everywhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelRouteApiMethod {
    ClaudeOAuth,
    AnthropicApiKey,
    OpenAIOAuth,
    OpenAIApiKey,
    OpenRouter,
    OpenAiCompatible { profile_id: Option<String> },
    Copilot,
    Cursor,
    Bedrock,
    CodeAssistOAuth,
    AntigravityHttps,
    RemoteCatalog,
    Current,
    Other(String),
}

impl ModelRouteApiMethod {
    pub fn parse(value: &str) -> Self {
        let trimmed = value.trim();
        let lower = trimmed.to_ascii_lowercase();
        match lower.as_str() {
            "claude" | "claude-oauth" => Self::ClaudeOAuth,
            "api-key" | "claude-api" | "anthropic-api-key" => Self::AnthropicApiKey,
            "openai" | "openai-oauth" => Self::OpenAIOAuth,
            "openai-api" | "openai-api-key" => Self::OpenAIApiKey,
            "openrouter" => Self::OpenRouter,
            "openai-compatible" => Self::OpenAiCompatible { profile_id: None },
            "copilot" => Self::Copilot,
            "cursor" => Self::Cursor,
            "bedrock" => Self::Bedrock,
            "code-assist-oauth" => Self::CodeAssistOAuth,
            "https" => Self::AntigravityHttps,
            "remote-catalog" => Self::RemoteCatalog,
            "current" => Self::Current,
            _ => {
                if let Some(("openai-compatible", profile_id)) = lower.split_once(':') {
                    let profile_id = profile_id.trim();
                    Self::OpenAiCompatible {
                        profile_id: (!profile_id.is_empty()).then(|| profile_id.to_string()),
                    }
                } else {
                    Self::Other(trimmed.to_string())
                }
            }
        }
    }

    pub fn profile_id(&self) -> Option<&str> {
        match self {
            Self::OpenAiCompatible {
                profile_id: Some(profile_id),
            } => Some(profile_id.as_str()),
            _ => None,
        }
    }

    pub fn is_openai_compatible(&self) -> bool {
        matches!(self, Self::OpenAiCompatible { .. })
    }

    pub fn is_openrouter(&self) -> bool {
        matches!(self, Self::OpenRouter)
    }

    pub fn is_copilot(&self) -> bool {
        matches!(self, Self::Copilot)
    }

    pub fn is_cursor(&self) -> bool {
        matches!(self, Self::Cursor)
    }

    pub fn is_bedrock(&self) -> bool {
        matches!(self, Self::Bedrock)
    }

    pub fn matches_openai_compatible_profile(&self, provider_id: &str) -> bool {
        self.profile_id()
            .is_some_and(|profile_id| profile_id.eq_ignore_ascii_case(provider_id))
    }

    pub fn is_anthropic_credential_route(&self) -> bool {
        matches!(self, Self::ClaudeOAuth | Self::AnthropicApiKey)
    }

    pub fn is_openai_credential_route(&self) -> bool {
        matches!(self, Self::OpenAIOAuth | Self::OpenAIApiKey)
    }

    pub fn display_label(&self) -> String {
        match self {
            Self::ClaudeOAuth | Self::OpenAIOAuth | Self::CodeAssistOAuth => "oauth".to_string(),
            Self::AnthropicApiKey | Self::OpenAIApiKey | Self::OpenAiCompatible { .. } => {
                "api key".to_string()
            }
            Self::OpenRouter => "openrouter".to_string(),
            Self::Copilot => "copilot".to_string(),
            Self::Cursor => "cursor".to_string(),
            Self::Bedrock => "bedrock".to_string(),
            Self::AntigravityHttps => "https".to_string(),
            Self::RemoteCatalog => "remote-catalog".to_string(),
            Self::Current => "current".to_string(),
            Self::Other(method) => method
                .split_once(':')
                .map(|(method, _)| method)
                .unwrap_or(method)
                .to_string(),
        }
    }
}

pub fn normalize_model_route_provider_label(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .replace([' ', '_', '-'], "")
}

pub fn model_route_provider_labels_match(route_provider: &str, current_provider: &str) -> bool {
    let route = normalize_model_route_provider_label(route_provider);
    let current = normalize_model_route_provider_label(current_provider);
    if route.is_empty() || current.is_empty() {
        return false;
    }
    if route == current {
        return true;
    }

    matches!(
        (current.as_str(), route.as_str()),
        ("claude" | "anthropic", "anthropic" | "claude")
            | ("openai", "openai")
            | ("gemini" | "google", "gemini" | "google")
            | ("antigravity", "antigravity")
            | (
                "copilot" | "copilotcode" | "githubcopilot",
                "copilot" | "githubcopilot"
            )
            | ("cursor", "cursor")
            | ("bedrock" | "awsbedrock", "bedrock" | "awsbedrock")
            | ("openrouter", "openrouter" | "auto")
    )
}

pub fn model_route_provider_labels_related(route_provider: &str, login_provider: &str) -> bool {
    let route = normalize_model_route_provider_label(route_provider);
    let login = normalize_model_route_provider_label(login_provider);
    if route.is_empty() || login.is_empty() {
        return false;
    }
    if route == login || route.contains(&login) || login.contains(&route) {
        return true;
    }
    model_route_provider_labels_match(&route, &login)
}

pub fn model_route_provider_matches_key(
    route_provider_key: Option<&str>,
    route_provider_label: &str,
    desired_provider: &str,
) -> bool {
    let desired_provider = desired_provider.trim();
    if desired_provider.is_empty() {
        return false;
    }
    if let Some(route_provider_key) = route_provider_key
        && normalize_model_route_provider_label(route_provider_key)
            == normalize_model_route_provider_label(desired_provider)
    {
        return true;
    }
    model_route_provider_labels_match(route_provider_label, desired_provider)
}

pub fn model_route_metadata_is_recommended(
    model: &str,
    provider: &str,
    api_method: &str,
    available: bool,
) -> bool {
    if !available {
        return false;
    }
    let api_method = ModelRouteApiMethod::parse(api_method);
    match model {
        "gpt-5.5" => {
            matches!(&api_method, ModelRouteApiMethod::OpenAIOAuth)
                && model_route_provider_labels_match(provider, "openai")
        }
        "claude-opus-4-8" => {
            matches!(
                &api_method,
                ModelRouteApiMethod::ClaudeOAuth | ModelRouteApiMethod::AnthropicApiKey
            ) && model_route_provider_labels_match(provider, "anthropic")
        }
        _ => false,
    }
}

impl ModelRoute {
    pub fn api_method_kind(&self) -> ModelRouteApiMethod {
        ModelRouteApiMethod::parse(&self.api_method)
    }

    pub fn estimated_reference_cost_micros(&self) -> Option<u64> {
        self.cheapness
            .as_ref()
            .and_then(|estimate| estimate.estimated_reference_cost_micros)
    }
}

/// Canonical snapshot of a provider's model catalog at a point in time.
///
/// This is the local contract shared by server-side providers, remote clients,
/// and persisted remote catalog caches. The websocket wire format may still
/// flatten these fields for backwards compatibility, but internal code should
/// pass catalog state as this single value instead of loose parallel vectors.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelCatalogSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_model: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub available_models: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_routes: Vec<ModelRoute>,
}

impl ModelCatalogSnapshot {
    pub fn new(
        provider_name: Option<String>,
        provider_model: Option<String>,
        available_models: Vec<String>,
        model_routes: Vec<ModelRoute>,
    ) -> Self {
        Self {
            provider_name,
            provider_model,
            available_models,
            model_routes,
        }
    }

    pub fn from_provider(provider: &dyn Provider) -> Self {
        Self::new(
            Some(provider.name().to_string()),
            Some(provider.model()),
            provider.available_models_display(),
            provider.model_routes(),
        )
    }

    pub fn has_routes(&self) -> bool {
        !self.model_routes.is_empty()
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

    #[test]
    fn model_route_api_method_parser_keeps_profile_identity() {
        assert_eq!(
            ModelRouteApiMethod::parse("openai-compatible:cerebras"),
            ModelRouteApiMethod::OpenAiCompatible {
                profile_id: Some("cerebras".to_string())
            }
        );
        assert!(
            ModelRouteApiMethod::parse("openai-compatible:cerebras")
                .matches_openai_compatible_profile("CEREBRAS")
        );
        assert_eq!(
            ModelRouteApiMethod::parse("openai-api"),
            ModelRouteApiMethod::OpenAIApiKey
        );
        assert_eq!(
            ModelRouteApiMethod::parse("claude-api"),
            ModelRouteApiMethod::AnthropicApiKey
        );
    }

    #[test]
    fn model_route_provider_label_matching_uses_aliases_without_substring_false_positives() {
        assert!(model_route_provider_labels_match("Anthropic", "Claude"));
        assert!(model_route_provider_labels_match("auto", "OpenRouter"));
        assert!(model_route_provider_labels_match(
            "GitHub Copilot",
            "Copilot"
        ));
        assert!(model_route_provider_labels_match("AWS Bedrock", "Bedrock"));
        assert!(!model_route_provider_labels_match(
            "OpenRouter/OpenAI",
            "OpenAI"
        ));
        assert!(!model_route_provider_labels_match("OpenAI", "OpenRouter"));
        assert!(!model_route_provider_labels_match("", ""));
        assert!(!model_route_provider_labels_related("OpenAI", ""));
    }

    #[test]
    fn model_route_provider_key_matching_prefers_explicit_route_key() {
        assert!(model_route_provider_matches_key(
            Some("cerebras"),
            "Cerebras Cloud",
            "CEREBRAS"
        ));
        assert!(model_route_provider_matches_key(
            None,
            "Anthropic",
            "Claude"
        ));
        assert!(!model_route_provider_matches_key(
            Some("cerebras"),
            "Cerebras",
            "groq"
        ));
    }

    #[test]
    fn model_route_recommendation_policy_is_provider_aware() {
        assert!(model_route_metadata_is_recommended(
            "gpt-5.5",
            "OpenAI",
            "openai-oauth",
            true
        ));
        assert!(!model_route_metadata_is_recommended(
            "gpt-5.5",
            "OpenAI",
            "openai-api-key",
            true
        ));
        assert!(!model_route_metadata_is_recommended(
            "gpt-5.5", "Copilot", "copilot", true
        ));
        assert!(!model_route_metadata_is_recommended(
            "gpt-5.5",
            "OpenAI",
            "openai-oauth",
            false
        ));
        assert!(model_route_metadata_is_recommended(
            "claude-opus-4-8",
            "Anthropic",
            "claude-oauth",
            true
        ));
        assert!(model_route_metadata_is_recommended(
            "claude-opus-4-8",
            "Anthropic",
            "claude-api",
            true
        ));
        assert!(!model_route_metadata_is_recommended(
            "claude-opus-4-8",
            "Anthropic",
            "openrouter",
            true
        ));
        assert!(!model_route_metadata_is_recommended(
            "deepseek/deepseek-v4-pro",
            "auto",
            "openrouter",
            true
        ));
    }

    struct SnapshotTestProvider;

    #[async_trait]
    impl Provider for SnapshotTestProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[ToolDefinition],
            _system: &str,
            _resume_session_id: Option<&str>,
        ) -> Result<EventStream> {
            unreachable!("snapshot test does not call complete")
        }

        fn name(&self) -> &str {
            "snapshot-provider"
        }

        fn model(&self) -> String {
            "snapshot-model".to_string()
        }

        fn available_models_display(&self) -> Vec<String> {
            vec!["snapshot-model".to_string()]
        }

        fn model_routes(&self) -> Vec<ModelRoute> {
            vec![ModelRoute {
                model: "snapshot-model".to_string(),
                provider: "Snapshot".to_string(),
                api_method: "snapshot-api".to_string(),
                available: true,
                detail: "test route".to_string(),
                cheapness: None,
            }]
        }

        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(SnapshotTestProvider)
        }
    }

    #[test]
    fn model_catalog_snapshot_materializes_provider_catalog_contract() {
        let snapshot = ModelCatalogSnapshot::from_provider(&SnapshotTestProvider);

        assert_eq!(snapshot.provider_name.as_deref(), Some("snapshot-provider"));
        assert_eq!(snapshot.provider_model.as_deref(), Some("snapshot-model"));
        assert_eq!(snapshot.available_models, ["snapshot-model"]);
        assert!(snapshot.has_routes());
        assert_eq!(snapshot.model_routes[0].api_method, "snapshot-api");
    }

    #[test]
    fn runtime_key_distinguishes_openrouter_from_direct_compatible_profile() {
        assert_eq!(
            RuntimeKey::from_api_method(&ModelRouteApiMethod::parse("openrouter"), "auto"),
            RuntimeKey::OpenRouter
        );
        assert_eq!(
            RuntimeKey::from_api_method(
                &ModelRouteApiMethod::parse("openai-compatible:nvidia-nim"),
                "NVIDIA NIM",
            ),
            RuntimeKey::OpenAiCompatible {
                profile_id: Some("nvidia-nim".to_string())
            }
        );
    }

    #[test]
    fn route_selection_preserves_runtime_identity_from_model_route() {
        let selection = RouteSelection::from_model_route(&ModelRoute {
            model: "openrouter/owl-alpha".to_string(),
            provider: "OpenRouter".to_string(),
            api_method: "openrouter".to_string(),
            available: true,
            detail: "https://openrouter.ai/api/v1".to_string(),
            cheapness: None,
        });
        assert_eq!(selection.model, "openrouter/owl-alpha");
        assert_eq!(selection.runtime_key, RuntimeKey::OpenRouter);
        assert_eq!(selection.api_method, "openrouter");

        let selection = RouteSelection::from_model_route(&ModelRoute {
            model: "nvidia/example".to_string(),
            provider: "NVIDIA NIM".to_string(),
            api_method: "openai-compatible:nvidia-nim".to_string(),
            available: true,
            detail: "https://integrate.api.nvidia.com/v1".to_string(),
            cheapness: None,
        });
        assert_eq!(
            selection.runtime_key,
            RuntimeKey::OpenAiCompatible {
                profile_id: Some("nvidia-nim".to_string())
            }
        );
        assert_eq!(selection.provider_label, "NVIDIA NIM");
    }
}
