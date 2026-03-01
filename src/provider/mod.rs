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
    "claude-opus-4-6[1m]",
    "claude-sonnet-4-6",
    "claude-sonnet-4-6[1m]",
    "claude-haiku-4-5",
    "claude-opus-4-5",
    "claude-sonnet-4-5",
    "claude-sonnet-4-20250514",
];

pub const ALL_OPENAI_MODELS: &[&str] = &[
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
    model.trim().to_ascii_lowercase()
}

fn normalize_provider_id(provider: &str) -> String {
    provider.trim().to_ascii_lowercase()
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
    }
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
    // Check dynamic cache first (populated from API)
    if let Some(limit) = get_cached_context_limit(model) {
        return Some(limit);
    }

    // Hardcoded fallbacks
    let model = model.to_lowercase();

    // [1m] suffix explicitly requests 1M context
    let (model, is_1m) = if let Some(base) = model.strip_suffix("[1m]") {
        (base.to_string(), true)
    } else {
        (model, false)
    };
    let model = model.as_str();

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
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ActiveProvider {
    Claude,
    OpenAI,
    Copilot,
    OpenRouter,
}

impl MultiProvider {
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
            _ => None,
        }
    }

    fn fallback_sequence(active: ActiveProvider) -> Vec<ActiveProvider> {
        match active {
            // OAuth-first fallback, then Copilot CLI. Keep OpenRouter out of automatic
            // exhaustion fallback to avoid silently switching to direct API billing.
            ActiveProvider::Claude => {
                vec![
                    ActiveProvider::Claude,
                    ActiveProvider::OpenAI,
                    ActiveProvider::Copilot,
                ]
            }
            ActiveProvider::OpenAI => {
                vec![
                    ActiveProvider::OpenAI,
                    ActiveProvider::Claude,
                    ActiveProvider::Copilot,
                ]
            }
            ActiveProvider::Copilot => {
                vec![
                    ActiveProvider::Copilot,
                    ActiveProvider::Claude,
                    ActiveProvider::OpenAI,
                ]
            }
            ActiveProvider::OpenRouter => vec![ActiveProvider::OpenRouter],
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

    fn should_failover_on_error(err: &anyhow::Error) -> bool {
        let lower = err.to_string().to_ascii_lowercase();

        let quota_or_limit = [
            "quota",
            "insufficient_quota",
            "rate limit",
            "rate_limit",
            "too many requests",
            " 429",
            "(429",
            "billing",
            "credit",
            "payment required",
            "usage exhausted",
            "token limit",
            "limit reached",
        ]
        .iter()
        .any(|needle| lower.contains(needle));

        let auth_or_availability = [
            "credentials not available",
            "token expired",
            "re-authenticate",
            "unauthorized",
            "forbidden",
            "not available for your account",
        ]
        .iter()
        .any(|needle| lower.contains(needle));

        quota_or_limit || auth_or_availability
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
        let mut active = if claude.is_some() || anthropic.is_some() {
            ActiveProvider::Claude
        } else if openai.is_some() {
            ActiveProvider::OpenAI
        } else if copilot_api.is_some() {
            ActiveProvider::Copilot
        } else if openrouter.is_some() {
            ActiveProvider::OpenRouter
        } else {
            // No credentials - default to Claude (will fail on use)
            ActiveProvider::Claude
        };

        // Apply configured default_provider from config/env if the provider is available
        let cfg = crate::config::config();
        if let Some(ref pref) = cfg.provider.default_provider {
            let preferred = match pref.as_str() {
                "claude" | "anthropic" => Some(ActiveProvider::Claude),
                "openai" => Some(ActiveProvider::OpenAI),
                "copilot" => Some(ActiveProvider::Copilot),
                "openrouter" => Some(ActiveProvider::OpenRouter),
                _ => {
                    crate::logging::warn(&format!(
                        "Unknown default_provider '{}' in config (expected: claude|openai|copilot|openrouter)",
                        pref
                    ));
                    None
                }
            };
            if let Some(pref_provider) = preferred {
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
        self.spawn_openai_catalog_refresh_if_needed();

        let active = self.active_provider();
        let sequence = Self::fallback_sequence(active);
        let mut notes: Vec<String> = Vec::new();

        for candidate in sequence {
            let label = Self::provider_label(candidate);
            let key = Self::provider_key(candidate);

            if !self.provider_is_configured(candidate) {
                notes.push(format!("{}: not configured", label));
                continue;
            }

            if let Some(detail) = provider_unavailability_detail_for_account(key) {
                notes.push(format!("{}: {}", label, detail));
                continue;
            }

            if let Some(reason) = self.provider_precheck_unavailable_reason(candidate) {
                notes.push(format!("{}: {}", label, reason));
                record_provider_unavailable_for_account(key, &reason);
                continue;
            }

            match self
                .complete_on_provider(candidate, messages, tools, system, resume_session_id)
                .await
            {
                Ok(stream) => {
                    clear_provider_unavailable_for_account(key);
                    if candidate != active {
                        self.set_active_provider(candidate);
                        crate::logging::info(&format!(
                            "Auto-fallback: switched active provider from {:?} to {:?}",
                            active, candidate
                        ));
                    }
                    return Ok(stream);
                }
                Err(err) => {
                    let summary = Self::summarize_error(&err);
                    notes.push(format!("{}: {}", label, summary));
                    if Self::should_failover_on_error(&err) {
                        record_provider_unavailable_for_account(key, &summary);
                    } else {
                        return Err(err);
                    }
                }
            }
        }

        Err(Self::no_provider_available_error(&notes))
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
        self.spawn_openai_catalog_refresh_if_needed();

        let active = self.active_provider();
        let sequence = Self::fallback_sequence(active);
        let mut notes: Vec<String> = Vec::new();

        for candidate in sequence {
            let label = Self::provider_label(candidate);
            let key = Self::provider_key(candidate);

            if !self.provider_is_configured(candidate) {
                notes.push(format!("{}: not configured", label));
                continue;
            }

            if let Some(detail) = provider_unavailability_detail_for_account(key) {
                notes.push(format!("{}: {}", label, detail));
                continue;
            }

            if let Some(reason) = self.provider_precheck_unavailable_reason(candidate) {
                notes.push(format!("{}: {}", label, reason));
                record_provider_unavailable_for_account(key, &reason);
                continue;
            }

            match self
                .complete_split_on_provider(
                    candidate,
                    messages,
                    tools,
                    system_static,
                    system_dynamic,
                    resume_session_id,
                )
                .await
            {
                Ok(stream) => {
                    clear_provider_unavailable_for_account(key);
                    if candidate != active {
                        self.set_active_provider(candidate);
                        crate::logging::info(&format!(
                            "Auto-fallback: switched active provider from {:?} to {:?}",
                            active, candidate
                        ));
                    }
                    return Ok(stream);
                }
                Err(err) => {
                    let summary = Self::summarize_error(&err);
                    notes.push(format!("{}: {}", label, summary));
                    if Self::should_failover_on_error(&err) {
                        record_provider_unavailable_for_account(key, &summary);
                    } else {
                        return Err(err);
                    }
                }
            }
        }

        Err(Self::no_provider_available_error(&notes))
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
                .unwrap_or_else(|| "gpt-5.3-codex-spark".to_string()),
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
            let copilot_guard = self.copilot_api.read().unwrap();
            if copilot_guard.is_none() {
                return Err(anyhow::anyhow!(
                    "GitHub Copilot is not available. Run `jcode login --provider copilot`."
                ));
            }
            *self.active.write().unwrap() = ActiveProvider::Copilot;
            if let Some(ref copilot) = *copilot_guard {
                copilot.set_model(copilot_model)?;
            }

            return Ok(());
        }

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
        models.extend(ALL_OPENAI_MODELS.iter().map(|m| (*m).to_string()));
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
                });
            }
            if !has_oauth && !has_api_key {
                routes.push(ModelRoute {
                    model: model.to_string(),
                    provider: "Anthropic".to_string(),
                    api_method: "claude-oauth".to_string(),
                    available: false,
                    detail: "no credentials".to_string(),
                });
            }
        }

        // OpenAI models
        for model in ALL_OPENAI_MODELS {
            let availability = model_availability_for_account(model);
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
            routes.push(ModelRoute {
                model: model.to_string(),
                provider: "OpenAI".to_string(),
                api_method: "openai-oauth".to_string(),
                available,
                detail,
            });
        }

        // GitHub Copilot models
        {
            let copilot_guard = self.copilot_api.read().unwrap();
            if let Some(ref copilot) = *copilot_guard {
                let copilot_models = copilot.available_models_display();
                for model in copilot_models {
                    routes.push(ModelRoute {
                        model,
                        provider: "Copilot".to_string(),
                        api_method: "copilot".to_string(),
                        available: true,
                        detail: String::new(),
                    });
                }
            } else if copilot::CopilotApiProvider::has_credentials() {
                routes.push(ModelRoute {
                    model: "copilot models".to_string(),
                    provider: "Copilot".to_string(),
                    api_method: "copilot".to_string(),
                    available: false,
                    detail: "not initialized yet".to_string(),
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
                        });
                    }
                } else {
                    routes.push(ModelRoute {
                        model: model.to_string(),
                        provider: "OpenAI".to_string(),
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
    fn test_fallback_sequence_prefers_oauth_then_copilot() {
        assert_eq!(
            MultiProvider::fallback_sequence(ActiveProvider::Claude),
            vec![
                ActiveProvider::Claude,
                ActiveProvider::OpenAI,
                ActiveProvider::Copilot,
            ]
        );
        assert_eq!(
            MultiProvider::fallback_sequence(ActiveProvider::OpenAI),
            vec![
                ActiveProvider::OpenAI,
                ActiveProvider::Claude,
                ActiveProvider::Copilot,
            ]
        );
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
}
