mod accessors;
mod account_failover;
pub mod activation;
pub mod anthropic;
pub mod antigravity;
pub mod bedrock;
mod catalog_routes;
pub mod claude;
pub mod copilot;
pub mod cursor;
mod dispatch;
pub mod external;
mod failover;
mod fingerprint;
pub mod gemini;
mod image_clamp;
pub mod jcode;
pub mod models;
mod multi_provider;
pub mod openai;
pub mod openai_request;
pub mod openrouter;
pub mod pricing;
mod registry;
mod route_builders;
mod routing;
mod selection;
mod startup;
mod state;

use crate::auth;
use crate::message::{Message, ToolDefinition};
use account_failover::{
    account_usage_probe, active_account_label_for_provider, maybe_annotate_limit_summary,
    same_provider_account_candidates, same_provider_account_failover_enabled,
    set_account_override_for_provider,
};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
#[cfg(test)]
use jcode_provider_core::FailoverDecision;
use registry::ProviderRegistry;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, RwLock};

pub use catalog_routes::{
    append_simplified_anthropic_model_routes, remote_current_openai_compatible_route_for_model,
    remote_model_is_server_copilot_only, remote_model_routes_fallback,
    remote_model_routes_lightweight_fallback, remote_model_should_offer_copilot_route,
    remote_openai_compatible_route_for_model, simplified_model_routes_for_picker,
};
pub use jcode_provider_core::attempt_tracker;
pub use jcode_provider_core::cli_provider_arg_for_session_key;
pub use jcode_provider_core::{
    ALL_CLAUDE_MODELS, ALL_OPENAI_MODELS, CHATGPT_WEB_MODEL, CHEAPNESS_REFERENCE_INPUT_TOKENS,
    CHEAPNESS_REFERENCE_OUTPUT_TOKENS, CredentialMode, DEFAULT_CONTEXT_LIMIT, EventStream,
    JCODE_USER_AGENT, ModelCapabilities, ModelCatalogRefreshSummary, ModelRoute,
    ModelRouteApiMethod, NativeCompactionResult, NativeToolResult, NativeToolResultSender,
    PremiumMode, Provider, RouteBillingKind, RouteCheapnessEstimate, RouteCostConfidence,
    RouteCostSource, RouteSelection, RuntimeKey, dedupe_model_routes,
    explicit_model_provider_prefix, fresh_transport_client, model_name_for_provider,
    normalize_copilot_model_name, provider_from_model_key, shared_http_client,
    summarize_model_catalog_refresh,
};
pub use jcode_provider_core::{
    FallbackPickOptions, error_looks_like_credential_failure, model_route_provider_labels_match,
    normalize_model_route_provider_label, pick_next_fallback_route,
    pick_next_fallback_route_with_options,
};
pub use jcode_provider_core::{ProviderFailoverPrompt, parse_failover_prompt_message};
pub use route_builders::{
    build_anthropic_oauth_route, build_chatgpt_web_route, build_copilot_route,
    build_openai_api_key_route, build_openai_oauth_route, build_openrouter_auto_route,
    build_openrouter_endpoint_route, build_openrouter_fallback_provider_route,
    is_listable_model_name, listable_model_names_from_routes, openrouter_catalog_model_id,
};
pub(crate) use routing::{
    anthropic_api_key_route_availability, anthropic_oauth_route_availability,
};

/// Process-wide handle to the live agent provider.
///
/// The memory sidecar ([`crate::sidecar::Sidecar`]) needs to make small,
/// cheap model calls (rerank / relevance / extraction). It has dedicated fast
/// paths for OpenAI (codex-spark) and Claude (haiku) OAuth, but jcode also runs
/// on Copilot, Antigravity, Gemini, Cursor, Bedrock, and OpenRouter. For those
/// providers there is no standalone sidecar HTTP client, so the sidecar falls
/// back to *this* handle and dispatches through the already-working
/// [`Provider::complete_simple`] path. `Server::new` registers the active
/// provider here at startup.
static ACTIVE_PROVIDER: RwLock<Option<Arc<dyn Provider>>> = RwLock::new(None);

/// Register the live agent provider so background helpers (memory sidecar) can
/// reach whatever provider the user is actually running on. Safe to call more
/// than once; the most recent registration wins.
pub fn set_active_provider(provider: Arc<dyn Provider>) {
    *ACTIVE_PROVIDER
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(provider);
}

/// Fetch the registered active provider, if any. Returns a forked handle so the
/// caller gets an independent provider instance (per the [`Provider::fork`]
/// contract) that will not interfere with the main agent's model selection.
pub fn active_provider_fork() -> Option<Arc<dyn Provider>> {
    ACTIVE_PROVIDER
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .map(|p| p.fork())
}

/// Provider-agnostic streaming idle timeout: max seconds to wait between
/// streamed chunks/events before treating the connection as dead. Resolved
/// from `[provider] stream_idle_timeout_secs` / `JCODE_STREAM_IDLE_TIMEOUT_SECS`
/// (default 180). Shared by every streaming provider path so slow reasoning
/// models that think silently for minutes don't trip a premature timeout on
/// one transport but not another (issue #434).
pub fn stream_idle_timeout() -> std::time::Duration {
    let secs = crate::config::config()
        .provider
        .stream_idle_timeout_secs
        .max(1);
    std::time::Duration::from_secs(secs)
}

/// Whether reasoning deltas should be persisted in session history for later
/// provider context reconstruction.
///
/// Display is controlled separately by `display.show_thinking`. Persist only
/// when a provider request builder can safely send the stored block back in
/// the provider-native shape. Anthropic is included only because we preserve
/// its thinking signatures in `ContentBlock::AnthropicThinking`.
pub fn stores_reasoning_content_for_context(provider_name: &str) -> bool {
    if !crate::config::config().provider.preserve_reasoning_context {
        return false;
    }
    matches!(
        provider_name.to_ascii_lowercase().as_str(),
        "openrouter" | "anthropic" | "openai"
    )
}

// Keep inactive direct profiles on the same 15-minute soft-refresh cadence as
// the active OpenRouter/OpenAI-compatible runtime. We continue serving the
// cached routes immediately while a background refresh updates the catalog.
const OPENAI_COMPATIBLE_PROFILE_CATALOG_SOFT_REFRESH_SECS: u64 = 15 * 60;

fn openai_compatible_profile_catalog_cache_is_stale(cached_at: u64, now: u64) -> bool {
    now.saturating_sub(cached_at) >= OPENAI_COMPATIBLE_PROFILE_CATALOG_SOFT_REFRESH_SECS
}

fn cached_live_models_for_openai_compatible_profile(
    resolved: &crate::provider_catalog::ResolvedOpenAiCompatibleProfile,
) -> Option<(Vec<String>, bool)> {
    let cache = jcode_provider_openrouter::load_disk_cache_entry_for_namespace(&resolved.id)?;
    let cache_is_stale = jcode_provider_openrouter::current_unix_secs()
        .map(|now| openai_compatible_profile_catalog_cache_is_stale(cache.cached_at, now))
        .unwrap_or(false);
    let source_api_base = cache
        .source_api_base
        .as_deref()
        .and_then(crate::provider_catalog::normalize_api_base)?;
    let expected_api_base = crate::provider_catalog::normalize_api_base(&resolved.api_base)?;
    if source_api_base != expected_api_base {
        return None;
    }

    let models = cache
        .models
        .into_iter()
        .map(|model| model.id.trim().to_string())
        .filter(|model| !model.is_empty())
        .collect::<Vec<_>>();
    if models.is_empty() {
        None
    } else {
        Some((models, cache_is_stale))
    }
}

fn direct_openai_compatible_profile_routes(
    profile: crate::provider_catalog::OpenAiCompatibleProfile,
) -> Vec<ModelRoute> {
    let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
    let static_models = crate::provider_catalog::openai_compatible_profile_static_models(profile);
    let (mut models, from_live_catalog) = if let Some((models, cache_is_stale)) =
        cached_live_models_for_openai_compatible_profile(&resolved)
    {
        if cache_is_stale {
            crate::provider::openrouter::maybe_schedule_openai_compatible_profile_catalog_refresh(
                profile,
                "inactive direct profile stale route cache",
            );
        }
        (models, true)
    } else {
        crate::provider::openrouter::maybe_schedule_openai_compatible_profile_catalog_refresh(
            profile,
            "inactive direct profile route cache miss",
        );
        let mut models = static_models;
        if models.is_empty()
            && let Some(default_model) = resolved.default_model.as_ref()
            && !default_model.trim().is_empty()
        {
            models.push(default_model.trim().to_string());
        }
        (models, false)
    };

    let provider = resolved.display_name.clone();
    let api_method = format!("openai-compatible:{}", resolved.id);
    let detail = if from_live_catalog {
        resolved.api_base.clone()
    } else if resolved.api_base.trim().is_empty() {
        "fallback: static provider model list".to_string()
    } else {
        format!(
            "{}; fallback: static provider model list",
            resolved.api_base
        )
    };

    let mut routes = Vec::new();
    for model in models.drain(..) {
        if !is_listable_model_name(&model)
            || !crate::provider_catalog::openai_compatible_profile_model_supports_chat(
                &resolved.id,
                &model,
            )
            || routes.iter().any(|route: &ModelRoute| route.model == model)
        {
            continue;
        }

        routes.push(ModelRoute {
            model,
            provider: provider.clone(),
            api_method: api_method.clone(),
            available: true,
            detail: detail.clone(),
            cheapness: None,
        });
    }

    routes
}

fn standard_openrouter_profile_configured() -> bool {
    crate::provider_catalog::load_env_value_from_env_or_config(
        "OPENROUTER_API_KEY",
        "openrouter.env",
    )
    .is_some()
}

fn configured_standard_openrouter_profile_routes() -> Vec<ModelRoute> {
    let Some(cache) = jcode_provider_openrouter::load_disk_cache_entry_for_namespace("openrouter")
    else {
        return Vec::new();
    };

    let source_matches_openrouter = cache
        .source_api_base
        .as_deref()
        .and_then(crate::provider_catalog::normalize_api_base)
        .map(|base| base.contains("openrouter.ai"))
        .unwrap_or(false);
    if !source_matches_openrouter {
        return Vec::new();
    }

    let available = standard_openrouter_profile_configured();
    cache
        .models
        .into_iter()
        .map(|model| model.id.trim().to_string())
        .filter(|model| is_listable_model_name(model))
        .map(|model| build_openrouter_auto_route(&model, available, String::new()))
        .collect()
}

pub fn set_model_with_auth_refresh(provider: &dyn Provider, model: &str) -> Result<()> {
    match provider.set_model(model) {
        Ok(()) => Ok(()),
        Err(first_err) => {
            let first_message = first_err.to_string();
            crate::logging::auth_event(
                "auth_changed_retry_after_set_model_failure",
                provider.name(),
                &[("reason", first_message.as_str())],
            );
            // Use the preserve-current-provider variant: this is a retry for an
            // already-open session, so refreshing auth from disk must NOT swap a
            // user-defined named OpenAI-compatible profile slot for a generic
            // OpenRouter runtime (which would lose `profile_id` and re-introduce
            // the `<profile>:<model>` prefix on the wire). See #408.
            provider.on_auth_changed_preserve_current_provider();
            provider.set_model(model).map_err(|second_err| {
                anyhow::anyhow!(
                    "{} (retried after reloading auth from disk: {})",
                    first_message,
                    second_err
                )
            })
        }
    }
}

use self::dispatch::CompletionMode;
pub use self::models::{
    AccountModelAvailability, AccountModelAvailabilityState, AnthropicModelCatalog,
    ModelCatalogHttpStatus, OpenAIModelCatalog, begin_anthropic_model_catalog_refresh,
    begin_openai_model_catalog_refresh, cached_anthropic_model_ids, cached_openai_model_ids,
    cached_openai_reasoning_efforts, clear_all_model_unavailability_for_account,
    clear_all_provider_unavailability_for_account, clear_model_unavailable_for_account,
    clear_provider_unavailable_for_account, context_limit_for_model,
    context_limit_for_model_with_provider, fetch_anthropic_model_catalog,
    fetch_anthropic_model_catalog_oauth, fetch_openai_api_key_model_catalog,
    fetch_openai_context_limits, fetch_openai_model_catalog,
    finish_anthropic_model_catalog_refresh_for_scope, finish_openai_model_catalog_refresh,
    format_account_model_availability_detail, get_best_available_openai_model,
    is_model_available_for_account, known_anthropic_model_ids, known_openai_model_ids,
    model_availability_for_account, model_unavailability_detail_for_account,
    note_openai_model_catalog_refresh_attempt, openai_platform_api_key_configured,
    persist_anthropic_model_catalog, persist_openai_model_catalog, populate_account_models,
    populate_anthropic_models, populate_context_limits, populate_context_limits_from_config,
    populate_context_limits_from_config_value, provider_for_model, provider_for_model_with_hint,
    provider_unavailability_detail_for_account, record_model_unavailable_for_account,
    record_provider_unavailable_for_account, refresh_openai_model_catalog_in_background,
    resolve_model_capabilities, should_refresh_anthropic_model_catalog,
    should_refresh_openai_model_catalog,
};
pub use self::selection::DefaultModelSelection;
use self::selection::{ActiveProvider, ProviderAvailability};
use self::state::ProviderState;
pub use self::state::{ProviderModelSelectionSource, ProviderRuntimeState, ProviderStateEvent};

/// MultiProvider wraps multiple providers and allows seamless model switching
pub struct MultiProvider {
    /// Claude Code CLI provider
    claude: RwLock<Option<Arc<dyn Provider>>>,
    /// Direct Anthropic API provider (no Python dependency)
    anthropic: RwLock<Option<Arc<dyn Provider>>>,
    openai: RwLock<Option<Arc<dyn Provider>>>,
    /// GitHub Copilot API provider (direct API, hot-swappable after login).
    /// Held as `dyn Provider`: the concrete runtime lives downstream in
    /// `jcode-provider-copilot-runtime` and is instantiated through
    /// `external::instantiate_external_provider`.
    copilot_api: RwLock<Option<Arc<dyn Provider>>>,
    /// Antigravity provider (direct HTTPS, hot-swappable after login). Held as
    /// `dyn Provider`: the concrete runtime lives downstream in
    /// `jcode-provider-antigravity-runtime` and is instantiated through
    /// `external::instantiate_external_provider`.
    antigravity: RwLock<Option<Arc<dyn Provider>>>,
    /// Gemini provider (hot-swappable after login). Held as `dyn Provider`:
    /// the concrete runtime lives downstream in `jcode-provider-gemini-runtime`
    /// and is instantiated through `external::instantiate_external_provider`.
    gemini: RwLock<Option<Arc<dyn Provider>>>,
    /// Cursor provider (native/direct API, hot-swappable after login). Held as
    /// `dyn Provider`: the concrete runtime lives downstream in
    /// `jcode-provider-cursor-runtime` and is instantiated through
    /// `external::instantiate_external_provider`.
    cursor: RwLock<Option<Arc<dyn Provider>>>,
    /// AWS Bedrock provider (native Converse/ConverseStream, IAM/SigV4)
    bedrock: RwLock<Option<Arc<bedrock::BedrockProvider>>>,
    /// OpenRouter API provider
    openrouter: RwLock<Option<Arc<dyn Provider>>>,
    /// Direct OpenAI-compatible runtimes keyed by profile id.
    ///
    /// These use the same wire protocol implementation as OpenRouter, but must
    /// not occupy the real OpenRouter slot. Keeping them separate prevents a
    /// compatible endpoint selection from corrupting later OpenRouter model
    /// switches, catalog display, or auth refresh handling.
    openai_compatible_profiles: RwLock<HashMap<String, Arc<dyn Provider>>>,
    active_openai_compatible_profile: RwLock<Option<String>>,
    active: RwLock<ActiveProvider>,
    /// Use Claude CLI instead of direct API (legacy mode)
    use_claude_cli: bool,
    /// Notifications generated during provider/account auto-selection.
    /// The TUI should drain and display these on session start.
    startup_notices: RwLock<Vec<String>>,
    /// CLI/environment selection to use when creating fresh sessions. This is
    /// only an initial preference and never restricts later model switches.
    initial_provider: Option<ActiveProvider>,
    /// Short-TTL memo for the full route-catalog build.
    ///
    /// Building the catalog is expensive (per-route pricing lookups, endpoint
    /// cache reads, credential probes) and the shared server rebuilds it for
    /// every connection whenever a `ModelsUpdated` bus event fans out. During
    /// a burst of client spawns that multiplied into hundreds of builds within
    /// a couple of seconds, saturating every core. The memo collapses those
    /// into one build per TTL window; auth/model changes invalidate it
    /// explicitly so pickers never see stale routes after a switch.
    routes_memo: Mutex<Option<RoutesMemoEntry>>,
    /// Number of model-catalog prefetches launched by the latest auth refresh.
    /// Shared by forks so the server can wait for real work rather than sleeping
    /// through a fixed quiet period after login.
    post_auth_refreshes_pending: Arc<std::sync::atomic::AtomicUsize>,
}

/// Memoized route catalog with the inputs that decide its freshness: build
/// time (short TTL), the auth generation at build time (bumped by
/// `AuthStatus::invalidate_cache()` on login/logout/credential edits), and the
/// catalog generation (bumped by prefetch/refresh completions).
#[derive(Clone)]
struct RoutesMemoEntry {
    built_at: std::time::Instant,
    auth_generation: u64,
    catalog_generation: u64,
    routes: Vec<ModelRoute>,
    /// `listable_model_names_from_routes(&routes)`, cached because the
    /// non-chat-model heuristic string-scans every route name and callers
    /// (catalog snapshots) ask for names and routes together.
    listable_models: Vec<String>,
}

/// Process-wide route-catalog memo shared across `MultiProvider` instances.
///
/// The shared server forks one `MultiProvider` per client connection, so a
/// per-instance memo cannot deduplicate the builds triggered by a burst of
/// simultaneous client spawns: every fresh fork still built its own catalog.
/// Catalog content is derived almost entirely from process-global state
/// (credential files, disk caches, config), so identical forks can share one
/// build. Instance-specific inputs (active provider/model/profile) are folded
/// into the memo key; anything not captured is bounded by the short TTL and
/// the auth/catalog generations.
static GLOBAL_ROUTES_MEMO: LazyLock<Mutex<HashMap<String, RoutesMemoEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Single-flight guard for catalog builds. During a client connect burst every
/// connection calls `model_routes()` at nearly the same instant; without this
/// they all miss the still-empty memo and build the same catalog in parallel
/// (a thundering herd that pegs every core). Holding this lock across the
/// build makes followers block (sleep, not spin) until the leader publishes
/// its result, which they then serve from the shared memo.
static GLOBAL_ROUTES_BUILD_LOCK: Mutex<()> = Mutex::new(());

/// Bumped whenever provider catalogs change out-of-band (prefetch completion,
/// forced catalog refresh, auth changes). Invalidates every shared memo entry.
static CATALOG_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn catalog_generation() -> u64 {
    CATALOG_GENERATION.load(std::sync::atomic::Ordering::Relaxed)
}

impl MultiProvider {
    /// Drop this instance's route-catalog memo. Use for changes that are
    /// captured by [`Self::routes_memo_key`] (model/provider/profile switches):
    /// the shared memo stays valid because those instances key differently.
    fn invalidate_routes_memo(&self) {
        if let Ok(mut memo) = self.routes_memo.lock() {
            *memo = None;
        }
    }

    /// Drop every memoized catalog in the process. Use for changes that alter
    /// catalog *content* beyond the memo key: credential changes and catalog
    /// prefetch/refresh completions. Deliberately not called from set_model /
    /// set_active_provider, which run once per shared-server fork during
    /// connect bursts and would otherwise defeat the shared memo.
    fn invalidate_routes_memo_globally(&self) {
        self.invalidate_routes_memo();
        CATALOG_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Key identifying the instance-specific state that feeds the route
    /// catalog. Two `MultiProvider` instances with equal keys (given equal
    /// auth/catalog generations) produce equivalent catalogs, so shared-server
    /// forks can reuse one build. The current model matters because the active
    /// OpenRouter model gets priority endpoint-refresh scheduling and detail
    /// annotations in the catalog; the configured-provider bitmap matters
    /// because each configured runtime contributes its own route family.
    fn routes_memo_key(&self) -> String {
        let active = self.active_provider();
        let credential_mode = self.credential_mode();
        let profile = self
            .active_openai_compatible_profile
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .unwrap_or_default();
        let mut compat_profiles: Vec<String> = self
            .openai_compatible_profiles
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .keys()
            .cloned()
            .collect();
        compat_profiles.sort();
        let configured = [
            ("cl", self.claude_provider().is_some()),
            ("an", self.anthropic_provider().is_some()),
            ("oa", self.openai_provider().is_some()),
            ("co", self.copilot_provider().is_some()),
            ("ag", self.antigravity_provider().is_some()),
            ("ge", self.gemini_provider().is_some()),
            ("cu", self.cursor_provider().is_some()),
            ("be", self.bedrock_provider().is_some()),
            ("or", self.openrouter_provider().is_some()),
        ]
        .iter()
        .filter(|(_, present)| *present)
        .map(|(tag, _)| *tag)
        .collect::<Vec<_>>()
        .join(",");
        format!(
            "{}|{}|{}|{:?}|{}|{}|{}|{}",
            // Scope by home so sandboxes (tests, JCODE_HOME switches) never
            // share catalogs that were built from different credential files.
            std::env::var("JCODE_HOME").unwrap_or_default(),
            Self::provider_key(active),
            self.model(),
            credential_mode,
            profile,
            self.use_claude_cli,
            configured,
            compat_profiles.join(","),
        )
    }

    /// Return a fresh memoized catalog entry (routes + listable model names),
    /// building it at most once per TTL window per catalog-relevant state.
    ///
    /// Freshness is keyed on a short TTL plus the auth and catalog
    /// generations. Lookup order: this instance's memo, the process-wide
    /// shared memo (so shared-server forks reuse one build), then a
    /// single-flight build that followers wait on instead of duplicating.
    fn fresh_routes_memo_entry(&self) -> RoutesMemoEntry {
        const ROUTES_MEMO_TTL: std::time::Duration = std::time::Duration::from_secs(3);

        let auth_generation = pricing::auth_pricing_generation();
        let catalog_gen = catalog_generation();
        let fresh = |entry: &RoutesMemoEntry| {
            entry.auth_generation == auth_generation
                && entry.catalog_generation == catalog_gen
                && entry.built_at.elapsed() < ROUTES_MEMO_TTL
        };

        // Fast path: this instance already built (or copied) a fresh catalog.
        if let Ok(memo) = self.routes_memo.lock()
            && let Some(entry) = memo.as_ref()
            && fresh(entry)
        {
            return entry.clone();
        }

        // Shared path: another instance with the same catalog-relevant state
        // (typically a fresh fork on the shared server) built one already.
        let shared_key = self.routes_memo_key();
        let try_shared = || -> Option<RoutesMemoEntry> {
            let shared = GLOBAL_ROUTES_MEMO.lock().ok()?;
            let entry = shared.get(&shared_key)?;
            if !fresh(entry) {
                return None;
            }
            let entry = entry.clone();
            if let Ok(mut memo) = self.routes_memo.lock() {
                *memo = Some(entry.clone());
            }
            Some(entry)
        };
        if let Some(entry) = try_shared() {
            return entry;
        }

        // Single-flight: serialize builds so a connect burst produces one
        // build and N-1 memo hits instead of N parallel builds.
        let _build_guard = GLOBAL_ROUTES_BUILD_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Re-check after acquiring the lock: the leader that held it may have
        // just published exactly the entry this instance needs.
        if let Some(entry) = try_shared() {
            return entry;
        }

        let routes = catalog_routes::multiprovider_model_routes(self);
        let entry = RoutesMemoEntry {
            built_at: std::time::Instant::now(),
            auth_generation,
            catalog_generation: catalog_gen,
            listable_models: listable_model_names_from_routes(&routes),
            routes,
        };
        if let Ok(mut memo) = self.routes_memo.lock() {
            *memo = Some(entry.clone());
        }
        if let Ok(mut shared) = GLOBAL_ROUTES_MEMO.lock() {
            // Tiny keyspace (active provider + model + profile); prune stale
            // entries opportunistically so it cannot grow unbounded.
            shared.retain(|_, existing| fresh(existing));
            shared.insert(shared_key, entry.clone());
        }
        entry
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

        // Downscale any images whose pixel dimensions exceed provider per-image
        // limits before they reach the wire. Resuming a session with >20 large
        // screenshots otherwise trips Anthropic's many-image 2000px cap and the
        // whole turn is rejected (#381). Only clones when a clamp is required.
        let clamped_messages = image_clamp::clamp_outbound_images(messages);
        let messages: &[Message] = clamped_messages.as_deref().unwrap_or(messages);

        let active = self.active_provider();
        let sequence = Self::fallback_sequence(active);
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
                    self.record_provider_activity(candidate);
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

    /// Record which login/credential just served a request in the
    /// cross-provider activity ledger (drives `/usage` recency sorting).
    /// Spawned off-thread: the ledger does file IO and a request was already
    /// accepted, so this must never block or fail the completion path.
    fn record_provider_activity(&self, provider: ActiveProvider) {
        let source_key = self.activity_source_key(provider);
        tokio::task::spawn_blocking(move || {
            crate::provider_activity::record_use(&source_key);
        });
    }

    /// Ledger source key for the credential `provider` will use right now.
    /// Mirrors `active_resolved_credential` for the dual-auth providers and
    /// the runtime profile resolution for the OpenRouter slot, but resolves
    /// against the *passed* provider so failover candidates attribute
    /// correctly even before `set_active_provider` runs.
    fn activity_source_key(&self, provider: ActiveProvider) -> String {
        match provider {
            ActiveProvider::Claude => {
                let uses_api_key = self
                    .anthropic_provider()
                    .map(|anthropic| match anthropic.credential_mode() {
                        anthropic::AnthropicCredentialMode::ApiKey => true,
                        anthropic::AnthropicCredentialMode::OAuth => false,
                        anthropic::AnthropicCredentialMode::Auto => {
                            crate::auth::claude::load_credentials().is_err()
                        }
                    })
                    .unwrap_or(false);
                if uses_api_key {
                    "claude:api-key".to_string()
                } else {
                    let label = crate::auth::claude::active_account_label()
                        .unwrap_or_else(|| "default".to_string());
                    format!("claude:oauth:{}", label)
                }
            }
            ActiveProvider::OpenAI => {
                let uses_api_key = self
                    .openai_provider()
                    .map(|openai| match openai.credential_mode() {
                        openai::OpenAICredentialMode::ApiKey => true,
                        openai::OpenAICredentialMode::OAuth => false,
                        openai::OpenAICredentialMode::Auto => {
                            crate::auth::codex::load_oauth_credentials().is_err()
                        }
                    })
                    .unwrap_or(false);
                if uses_api_key {
                    "openai:api-key".to_string()
                } else {
                    let label = crate::auth::codex::active_account_label()
                        .unwrap_or_else(|| "default".to_string());
                    format!("openai:oauth:{}", label)
                }
            }
            ActiveProvider::OpenRouter => {
                // The OpenRouter slot multiplexes the public aggregator, the
                // jcode subscription, and direct OpenAI-compatible profiles.
                let label = self
                    .active_openrouter_execution_provider()
                    .map(|execution| execution.runtime_display_name())
                    .unwrap_or_else(|| "OpenRouter".to_string());
                let runtime = std::env::var("JCODE_RUNTIME_PROVIDER").ok();
                crate::provider_activity::source_key_for_provider_label(&label, runtime.as_deref())
            }
            other => Self::provider_key(other).to_string(),
        }
    }

    fn openai_compatible_model_prefix(
        model: &str,
    ) -> Option<(crate::provider_catalog::OpenAiCompatibleProfile, &str)> {
        let (prefix, rest) = model.split_once(':')?;
        if explicit_model_provider_prefix(model).is_some() {
            return None;
        }
        let rest = rest.trim();
        if rest.is_empty() {
            return None;
        }

        let profile = crate::provider_catalog::openai_compatible_profile_by_id(prefix)?;
        Some((profile, rest))
    }

    /// Parse a `<name>:<model>` spec whose prefix is a user-defined named
    /// provider profile from config (`[providers.<name>]`). Built-in provider
    /// prefixes and catalog profile ids take precedence and never reach here.
    fn named_provider_profile_model_prefix(model: &str) -> Option<(String, String)> {
        let (prefix, rest) = model.split_once(':')?;
        if explicit_model_provider_prefix(model).is_some()
            || Self::openai_compatible_model_prefix(model).is_some()
        {
            return None;
        }
        let prefix = prefix.trim();
        let rest = rest.trim();
        if prefix.is_empty() || rest.is_empty() {
            return None;
        }
        crate::config::config()
            .providers
            .contains_key(prefix)
            .then(|| (prefix.to_string(), rest.to_string()))
    }

    /// Bind (or reuse) the runtime for a named config provider profile and
    /// select `model` on it (issue #444).
    fn set_model_on_named_provider_profile(&self, profile_name: &str, model: &str) -> Result<()> {
        let model = model.trim();
        if model.is_empty() {
            anyhow::bail!("Model cannot be empty");
        }
        let config = crate::config::config()
            .providers
            .get(profile_name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Unknown provider profile '{}'", profile_name))?;

        let expected_api_method = format!("openai-compatible:{}", profile_name);
        let registry = ProviderRegistry::new(self);
        let provider = {
            let existing = registry
                .compatible_profile(profile_name)
                .filter(|provider| {
                    provider
                        .direct_openai_compatible_route_parts()
                        .map(|(_provider, api_method, _detail)| api_method == expected_api_method)
                        .unwrap_or(false)
                });
            if let Some(provider) = existing {
                provider
            } else {
                let provider = external::instantiate_openrouter_runtime(
                    external::OpenRouterRuntimeSpec::NamedProfile {
                        name: profile_name.to_string(),
                        config,
                    },
                )?;
                registry
                    .install_compatible_profile(profile_name.to_string(), Arc::clone(&provider));
                provider
            }
        };
        provider.set_model(model)?;
        registry.set_active_compatible_profile(profile_name.to_string());
        self.set_active_provider(ActiveProvider::OpenRouter);
        Ok(())
    }

    fn set_model_on_provider(&self, provider: ActiveProvider, model: &str) -> Result<()> {
        self.set_model_on_provider_with_credential_modes(provider, model, None, None)
    }

    fn set_model_on_provider_with_credential_modes(
        &self,
        provider: ActiveProvider,
        model: &str,
        openai_credential_mode: Option<openai::OpenAICredentialMode>,
        anthropic_credential_mode: Option<anthropic::AnthropicCredentialMode>,
    ) -> Result<()> {
        let model = model.trim();
        if model.is_empty() {
            anyhow::bail!("Model cannot be empty");
        }

        self.reconcile_auth_if_provider_missing(provider);

        match provider {
            ActiveProvider::Claude => {
                let model = model_name_for_provider(provider, model);
                if let Some(anthropic) = self.anthropic_provider() {
                    if let Some(mode) = anthropic_credential_mode {
                        anthropic.set_credential_mode(mode)?;
                    }
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
                    // No OpenAI runtime: still run the same model-name
                    // validation the runtime itself would. A cross-provider
                    // model under a forced/locked OpenAI selection must report
                    // the real problem (wrong model family), not demand a
                    // login that would never make the model valid. Keeps the
                    // error independent of which credentials exist on disk.
                    if !known_openai_model_ids().iter().any(|known| known == model) {
                        anyhow::bail!(
                            "Unsupported OpenAI model '{}'. Use /model to choose from the models available to your account.",
                            model
                        );
                    }
                    anyhow::bail!(
                        "OpenAI credentials not available. Run `jcode login --provider openai` first."
                    );
                };
                if let Some(mode) = openai_credential_mode {
                    openai.set_credential_mode(mode)?;
                }
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
            ActiveProvider::Bedrock => {
                let Some(bedrock) = self.bedrock_provider() else {
                    anyhow::bail!(
                        "AWS Bedrock credentials not available. Configure AWS credentials and region first."
                    );
                };
                bedrock.set_model(model)?;
                self.set_active_provider(ActiveProvider::Bedrock);
                Ok(())
            }
            ActiveProvider::OpenRouter => {
                self.clear_active_openai_compatible_profile();
                // Decide whether the slot must be rebound to the real
                // OpenRouter API-key runtime. Rebinding repairs a slot left
                // flavored as a *known catalog profile* runtime by startup
                // profile env (e.g. a Cerebras login applied globally, then
                // the slot was built as Cerebras), so an OpenRouter-targeted
                // switch reaches the real aggregator again. But a *custom*
                // OpenAI-compatible endpoint (generic profile or named config
                // profile) owns the slot
                // legitimately: its model IDs are provider-local and must not
                // be re-routed through OpenRouter (or fail outright because no
                // OPENROUTER_API_KEY is configured).
                let needs_rebind = match self.openrouter_provider().as_deref() {
                    None => true,
                    Some(provider) => {
                        !provider.supports_provider_routing_features()
                            && provider
                                .direct_openai_compatible_route_parts()
                                .and_then(|(_provider, api_method, _detail)| {
                                    api_method
                                    .strip_prefix("openai-compatible:")
                                    .map(str::trim)
                                    .and_then(
                                        crate::provider_catalog::openai_compatible_profile_by_id,
                                    )
                                })
                                .map(|profile| {
                                    profile.id != crate::provider_catalog::OPENAI_COMPAT_PROFILE.id
                                })
                                .unwrap_or(false)
                    }
                };
                if needs_rebind {
                    let provider = external::instantiate_openrouter_runtime(
                        external::OpenRouterRuntimeSpec::OpenRouterApiKey,
                    )?;
                    *self
                        .openrouter
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(provider);
                }

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

    fn set_model_on_openai_compatible_profile(
        &self,
        profile: crate::provider_catalog::OpenAiCompatibleProfile,
        model: &str,
    ) -> Result<()> {
        let model = model.trim();
        if model.is_empty() {
            anyhow::bail!("Model cannot be empty");
        }
        let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
        if !crate::provider_catalog::openai_compatible_profile_is_configured(profile) {
            anyhow::bail!(
                "{} credentials not available. Run `jcode login --provider {}` first.",
                resolved.display_name,
                resolved.id,
            );
        }

        let profile_id = resolved.id.clone();
        let registry = ProviderRegistry::new(self);
        let provider = {
            let existing = registry.compatible_profile(&profile_id).filter(|provider| {
                provider
                    .direct_openai_compatible_route_parts()
                    .and_then(|(_provider, api_method, _detail)| {
                        api_method
                            .strip_prefix("openai-compatible:")
                            .map(|profile| profile.trim().to_string())
                    })
                    .as_deref()
                    == Some(profile_id.as_str())
            });
            if let Some(provider) = existing {
                provider
            } else {
                let provider = external::instantiate_openrouter_runtime(
                    external::OpenRouterRuntimeSpec::CompatibleProfile(profile),
                )?;
                registry.install_compatible_profile(profile_id.clone(), Arc::clone(&provider));
                provider
            }
        };
        provider.set_model(model)?;
        registry.set_active_compatible_profile(profile_id);
        self.set_active_provider(ActiveProvider::OpenRouter);
        Ok(())
    }

    fn should_replace_openrouter_after_auth_change(
        existing: &dyn Provider,
        candidate: &dyn Provider,
    ) -> bool {
        if existing.supports_provider_routing_features()
            != candidate.supports_provider_routing_features()
        {
            return false;
        }

        let existing_direct = existing
            .direct_openai_compatible_route_parts()
            .map(|(_provider, api_method, _detail)| api_method);
        let candidate_direct = candidate
            .direct_openai_compatible_route_parts()
            .map(|(_provider, api_method, _detail)| api_method);

        existing_direct == candidate_direct
    }

    fn handle_auth_changed(&self, preserve_existing_openrouter_profile: bool) {
        crate::logging::auth_event("auth_changed_received", "multi-provider", &[]);
        // Credentials feed route availability/pricing, so every memoized
        // catalog in the process is stale the moment auth changes.
        self.invalidate_routes_memo_globally();
        // Auth just changed, so discard any stale full/fast snapshots before
        // using cheap local probes to hot-initialize newly configured providers.
        crate::auth::AuthStatus::invalidate_cache();

        if self.use_claude_cli {
            if self.claude_provider().is_none()
                && crate::auth::claude::load_credentials().is_ok()
                && let Some(claude) =
                    external::instantiate_expected_external_provider(external::CLAUDE_CLI_RUNTIME)
            {
                crate::logging::info("Hot-initialized Claude CLI provider after auth change");
                *self
                    .claude
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(claude);
            }
        } else if self.anthropic_provider().is_none()
            && (crate::auth::claude::load_credentials().is_ok()
                || crate::provider_catalog::load_api_key_from_env_or_config(
                    "ANTHROPIC_API_KEY",
                    "anthropic.env",
                )
                .is_some())
        {
            crate::logging::info("Hot-initialized Anthropic provider after auth change");
            *self
                .anthropic
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                external::instantiate_expected_external_provider(external::ANTHROPIC_RUNTIME);
        }

        if let Some(openai) = self.openai_provider() {
            openai.reload_credentials();
        } else if crate::auth::codex::load_credentials().is_ok()
            && let Some(openai) =
                external::instantiate_expected_external_provider(external::OPENAI_RUNTIME)
        {
            crate::logging::info("Hot-initialized OpenAI provider after auth change");
            *self
                .openai
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(openai);
        }

        if openrouter::has_credentials() {
            match external::instantiate_openrouter_runtime(external::OpenRouterRuntimeSpec::Default)
            {
                Ok(provider) => {
                    let should_install = if preserve_existing_openrouter_profile {
                        self.openrouter_provider()
                            .as_deref()
                            .map(|existing| {
                                Self::should_replace_openrouter_after_auth_change(
                                    existing,
                                    provider.as_ref(),
                                )
                            })
                            .unwrap_or(true)
                    } else {
                        true
                    };
                    if should_install {
                        crate::logging::info(
                            "Hot-initialized OpenRouter/OpenAI-compatible provider after auth change",
                        );
                        *self
                            .openrouter
                            .write()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(provider);
                    } else {
                        crate::logging::info(
                            "Preserved existing OpenRouter/OpenAI-compatible provider after unrelated auth change",
                        );
                    }
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
            // The composition-root factory schedules tier detection itself.
            if status.copilot_has_api_token
                && let Some(provider) =
                    external::instantiate_expected_external_provider(external::COPILOT_RUNTIME)
            {
                crate::logging::info("Hot-initialized Copilot API provider after login");
                *self
                    .copilot_api
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(provider);
            }
        }

        let already_has_antigravity = self.antigravity_provider().is_some();
        if !already_has_antigravity
            && crate::auth::antigravity::load_tokens().is_ok()
            && let Some(antigravity) =
                external::instantiate_expected_external_provider(external::ANTIGRAVITY_RUNTIME)
        {
            crate::logging::info("Hot-initialized Antigravity provider after login");
            *self
                .antigravity
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(antigravity);
        }

        let already_has_gemini = self.gemini_provider().is_some();
        if !already_has_gemini
            && crate::auth::gemini::load_tokens().is_ok()
            && let Some(gemini) =
                external::instantiate_expected_external_provider(external::GEMINI_RUNTIME)
        {
            crate::logging::info("Hot-initialized Gemini provider after login");
            *self
                .gemini
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(gemini);
        }

        let already_has_cursor = self.cursor_provider().is_some();
        if !already_has_cursor
            && crate::auth::AuthStatus::check_fast()
                .assessment_for_provider(crate::provider_catalog::CURSOR_LOGIN_PROVIDER)
                .is_available()
            && let Some(cursor) =
                external::instantiate_expected_external_provider(external::CURSOR_RUNTIME)
        {
            crate::logging::info("Hot-initialized Cursor provider after login");
            *self
                .cursor
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(cursor);
        }

        let already_has_bedrock = self.bedrock_provider().is_some();
        if !already_has_bedrock && bedrock::BedrockProvider::has_credentials() {
            crate::logging::info("Hot-initialized AWS Bedrock provider after login");
            *self
                .bedrock
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some(Arc::new(bedrock::BedrockProvider::new()));
        }

        if let Some(anthropic) = self.anthropic_provider() {
            self.spawn_post_auth_model_refresh(anthropic, "Anthropic");
        }
        if let Some(claude) = self.claude_provider() {
            self.spawn_post_auth_model_refresh(claude, "Claude");
        }
        if let Some(openai) = self.openai_provider() {
            self.spawn_post_auth_model_refresh(openai, "OpenAI");
        }
        if let Some(antigravity) = self.antigravity_provider() {
            self.spawn_post_auth_model_refresh(antigravity, "Antigravity");
        }
        if let Some(gemini) = self.gemini_provider() {
            self.spawn_post_auth_model_refresh(gemini, "Gemini");
        }
        if let Some(cursor) = self.cursor_provider() {
            self.spawn_post_auth_model_refresh(cursor, "Cursor");
        }
        if let Some(openrouter) = self.openrouter_provider() {
            self.spawn_post_auth_model_refresh(openrouter, "OpenRouter");
        }
        if let Some(bedrock) = self.bedrock_provider() {
            self.spawn_post_auth_model_refresh(bedrock, "AWS Bedrock");
        }
        crate::logging::auth_event("auth_changed_completed", "multi-provider", &[]);
    }

    pub(super) fn set_config_default_model(
        &self,
        model: &str,
        default_provider: Option<&str>,
    ) -> Result<()> {
        let model = model.trim();
        if model.is_empty() {
            anyhow::bail!("Model cannot be empty");
        }

        // The model picker persists default_model as a full model spec that
        // may carry an explicit provider/credential prefix (e.g.
        // `claude-api:claude-fable-5`). Provider-local `set_model`
        // implementations validate bare model ids, so a prefixed spec must go
        // through the canonical prefix-aware path. Handing the raw spec to a
        // single provider would make it reject the id and silently keep its
        // fallback default model.
        if explicit_model_provider_prefix(model).is_some()
            || Self::openai_compatible_model_prefix(model).is_some()
        {
            return self.set_model(model);
        }

        // A configured default_provider is a routing decision, not just a
        // startup hint. Treat default_model as provider-local when the config
        // names a concrete provider/profile so global model-name heuristics
        // cannot undo that decision. This is especially important for
        // OpenAI-compatible gateways whose model IDs often look like built-in
        // OpenAI, Anthropic, or OpenRouter models.
        if let Some(pref) = default_provider.and_then(|pref| {
            let trimmed = pref.trim();
            (!trimmed.is_empty()).then_some(trimmed)
        }) && let Some(selection) =
            Self::resolve_config_provider_selection(pref, crate::config::config())
        {
            // A known OpenAI-compatible catalog profile (deepseek, zai, ...)
            // must be handled profile-locally. Its `active_provider()` maps to
            // the shared OpenRouter slot, but routing through the generic
            // OpenRouter path would trigger the OpenRouter rebind logic, which
            // replaces the profile runtime with a plain OpenRouter API-key
            // runtime and fails when OPENROUTER_API_KEY is not configured --
            // silently dropping the configured default (issue #448).
            if let selection::ConfigProviderSelection::OpenAiCompatibleProfile(profile_id) =
                &selection
                && let Some(profile) =
                    crate::provider_catalog::openai_compatible_profile_by_id(profile_id)
            {
                return self.set_model_on_openai_compatible_profile(profile, model);
            }

            // Same reasoning for user-defined named provider profiles from
            // config: bind the named profile runtime directly instead of the
            // generic OpenRouter slot path.
            if let selection::ConfigProviderSelection::NamedProfile(profile_name) = &selection {
                return self.set_model_on_named_provider_profile(profile_name, model);
            }

            // A dual-auth config provider key (`anthropic-api`, `claude-oauth`,
            // `openai-api`, ...) also pins the OAuth-vs-API credential. Carry
            // that through so the active credential -- and every surface that
            // reads it (header auth tag, model picker) -- matches the route the
            // user configured, instead of leaving the provider in Auto mode
            // (which prefers OAuth) and silently mislabeling an API default.
            //
            // Bare provider keys (`claude`, `anthropic`, `openai`) intentionally
            // do NOT pin a credential: they keep Auto mode (so an API-only user
            // with `default_provider = "claude"` still resolves their key
            // instead of failing to load absent OAuth credentials).
            let pinned = jcode_provider_core::AuthRoute::parse_explicit_credential_prefix(pref);
            let anthropic_credential_mode = pinned.and_then(|route| {
                matches!(
                    route.provider,
                    jcode_provider_core::DualAuthProvider::Anthropic
                )
                .then(|| match route.mode {
                    jcode_provider_core::AuthMode::ApiKey => {
                        anthropic::AnthropicCredentialMode::ApiKey
                    }
                    jcode_provider_core::AuthMode::Oauth => {
                        anthropic::AnthropicCredentialMode::OAuth
                    }
                })
            });
            let openai_credential_mode = pinned.and_then(|route| {
                matches!(
                    route.provider,
                    jcode_provider_core::DualAuthProvider::OpenAI
                )
                .then(|| match route.mode {
                    jcode_provider_core::AuthMode::ApiKey => openai::OpenAICredentialMode::ApiKey,
                    jcode_provider_core::AuthMode::Oauth => openai::OpenAICredentialMode::OAuth,
                })
            });
            return self.set_model_on_provider_with_credential_modes(
                selection.active_provider(),
                model,
                openai_credential_mode,
                anthropic_credential_mode,
            );
        }

        self.set_model(model)
    }

    fn fork_model_switch_request(&self, active: ActiveProvider, current_model: &str) -> String {
        let prefix = match active {
            ActiveProvider::Claude => {
                if let Some(anthropic) = self.anthropic_provider() {
                    // OAuth/ApiKey emit their canonical model prefix; Auto keeps
                    // the bare provider key (route without pinning a credential).
                    anthropic
                        .credential_mode()
                        .auth_route(jcode_provider_core::DualAuthProvider::Anthropic)
                        .map(|route| route.model_prefix())
                        .unwrap_or("claude")
                } else {
                    "claude"
                }
            }
            ActiveProvider::OpenAI => {
                if let Some(openai) = self.openai_provider() {
                    openai
                        .credential_mode()
                        .auth_route(jcode_provider_core::DualAuthProvider::OpenAI)
                        .map(|route| route.model_prefix())
                        .unwrap_or("openai")
                } else {
                    "openai"
                }
            }
            ActiveProvider::Copilot => "copilot",
            ActiveProvider::Antigravity => "antigravity",
            ActiveProvider::Gemini => "gemini",
            ActiveProvider::Cursor => "cursor",
            ActiveProvider::Bedrock => "bedrock",
            ActiveProvider::OpenRouter => {
                if let Some(openrouter) = self.active_openrouter_execution_provider()
                    && let Some((_provider, api_method, _detail)) =
                        openrouter.direct_openai_compatible_route_parts()
                    && let Some(profile_id) = api_method
                        .strip_prefix("openai-compatible:")
                        .map(str::trim)
                        .filter(|profile_id| !profile_id.is_empty())
                {
                    return format!("{profile_id}:{current_model}");
                }
                if let Some(openrouter) = self.openrouter_provider()
                    && let Some(provider_pin) = openrouter.explicit_provider_pin_for_current_model()
                {
                    return format!("openrouter:{current_model}@{provider_pin}");
                }
                "openrouter"
            }
        };
        format!("{prefix}:{current_model}")
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
            ActiveProvider::Bedrock => "Bedrock",
            ActiveProvider::OpenRouter => "OpenRouter",
        }
    }

    fn display_name(&self) -> String {
        // The OpenRouter slot multiplexes the public aggregator and every
        // direct OpenAI-compatible profile (NVIDIA NIM, DeepSeek, ...). Ask the
        // active execution runtime for its own label so the UI reflects the
        // profile selected at runtime rather than the fixed "OpenRouter" name.
        if matches!(self.active_provider(), ActiveProvider::OpenRouter)
            && let Some(execution) = self.active_openrouter_execution_provider()
        {
            return execution.runtime_display_name();
        }
        self.name().to_string()
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
                    jcode_provider_core::DEFAULT_CLAUDE_MODEL.to_string()
                }
            }
            ActiveProvider::OpenAI => self
                .openai_provider()
                .map(|o| o.model())
                .unwrap_or_else(|| jcode_provider_core::DEFAULT_OPENAI_MODEL.to_string()),
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
                .unwrap_or_else(|| "composer-2.5".to_string()),
            ActiveProvider::Bedrock => self
                .bedrock_provider()
                .map(|o| o.model())
                .unwrap_or_else(|| "anthropic.claude-3-5-sonnet-20241022-v2:0".to_string()),
            ActiveProvider::OpenRouter => self
                .active_openrouter_execution_provider()
                .map(|o| o.model())
                .unwrap_or_else(|| "anthropic/claude-sonnet-4".to_string()),
        }
    }

    fn active_resolved_credential(&self) -> Option<jcode_provider_core::ResolvedCredential> {
        use jcode_provider_core::ResolvedCredential;
        match self.active_provider() {
            ActiveProvider::Claude => {
                let anthropic = self.anthropic_provider()?;
                Some(match anthropic.credential_mode() {
                    anthropic::AnthropicCredentialMode::OAuth => ResolvedCredential::Oauth,
                    anthropic::AnthropicCredentialMode::ApiKey => ResolvedCredential::ApiKey,
                    // Auto prefers OAuth (Claude subscription) when available,
                    // otherwise falls back to the API key. Mirror that exactly.
                    anthropic::AnthropicCredentialMode::Auto => {
                        if crate::auth::claude::load_credentials().is_ok() {
                            ResolvedCredential::Oauth
                        } else {
                            ResolvedCredential::ApiKey
                        }
                    }
                })
            }
            ActiveProvider::OpenAI => {
                let openai = self.openai_provider()?;
                Some(match openai.credential_mode() {
                    openai::OpenAICredentialMode::OAuth => ResolvedCredential::Oauth,
                    openai::OpenAICredentialMode::ApiKey => ResolvedCredential::ApiKey,
                    // Auto resolves to OAuth first when available, otherwise API key.
                    openai::OpenAICredentialMode::Auto => {
                        if crate::auth::codex::load_oauth_credentials().is_ok() {
                            ResolvedCredential::Oauth
                        } else {
                            ResolvedCredential::ApiKey
                        }
                    }
                })
            }
            _ => None,
        }
    }

    fn credential_mode(&self) -> CredentialMode {
        let active = self.active_provider();
        match active {
            ActiveProvider::Claude => self
                .anthropic_provider()
                .map(|provider| provider.credential_mode())
                .unwrap_or(CredentialMode::Auto),
            ActiveProvider::OpenAI => self
                .openai_provider()
                .map(|provider| provider.credential_mode())
                .unwrap_or(CredentialMode::Auto),
            _ => CredentialMode::Auto,
        }
    }

    fn set_credential_mode(&self, mode: CredentialMode) -> Result<()> {
        let active = self.active_provider();
        match active {
            ActiveProvider::Claude => self
                .anthropic_provider()
                .ok_or_else(|| anyhow!("Anthropic provider is not configured"))?
                .set_credential_mode(mode)?,
            ActiveProvider::OpenAI => self
                .openai_provider()
                .ok_or_else(|| anyhow!("OpenAI provider is not configured"))?
                .set_credential_mode(mode)?,
            _ if mode == CredentialMode::Auto => return Ok(()),
            _ => anyhow::bail!(
                "Provider {} does not support OAuth/API-key credential selection",
                Self::provider_label(active)
            ),
        }
        self.set_active_provider(active);
        Ok(())
    }

    fn active_explicit_credential(&self) -> Option<jcode_provider_core::ResolvedCredential> {
        use jcode_provider_core::ResolvedCredential;
        // Only report an *explicit* in-memory pin. Auto mode returns `None` so
        // callers fall back to their cheaper cached heuristic without forcing
        // a disk read on every frame. This stays in lockstep with
        // `active_resolved_credential`'s explicit arms above.
        match self.active_provider() {
            ActiveProvider::Claude => match self.anthropic_provider()?.credential_mode() {
                anthropic::AnthropicCredentialMode::OAuth => Some(ResolvedCredential::Oauth),
                anthropic::AnthropicCredentialMode::ApiKey => Some(ResolvedCredential::ApiKey),
                anthropic::AnthropicCredentialMode::Auto => None,
            },
            ActiveProvider::OpenAI => match self.openai_provider()?.credential_mode() {
                openai::OpenAICredentialMode::OAuth => Some(ResolvedCredential::Oauth),
                openai::OpenAICredentialMode::ApiKey => Some(ResolvedCredential::ApiKey),
                openai::OpenAICredentialMode::Auto => None,
            },
            _ => None,
        }
    }

    fn supports_image_input(&self) -> bool {
        match self.active_provider() {
            ActiveProvider::Claude => self
                .anthropic_provider()
                .map(|provider| provider.supports_image_input())
                .or_else(|| {
                    self.claude_provider()
                        .map(|provider| provider.supports_image_input())
                })
                .unwrap_or(false),
            ActiveProvider::OpenAI => self
                .openai_provider()
                .map(|provider| provider.supports_image_input())
                .unwrap_or(false),
            ActiveProvider::Copilot => self
                .copilot_provider()
                .map(|provider| provider.supports_image_input())
                .unwrap_or(false),
            ActiveProvider::Antigravity => self
                .antigravity_provider()
                .map(|provider| provider.supports_image_input())
                .unwrap_or(false),
            ActiveProvider::Gemini => self
                .gemini_provider()
                .map(|provider| provider.supports_image_input())
                .unwrap_or(false),
            ActiveProvider::Cursor => self
                .cursor_provider()
                .map(|provider| provider.supports_image_input())
                .unwrap_or(false),
            ActiveProvider::Bedrock => self
                .bedrock_provider()
                .map(|provider| provider.supports_image_input())
                .unwrap_or(false),
            ActiveProvider::OpenRouter => self
                .active_openrouter_execution_provider()
                .map(|provider| provider.supports_image_input())
                .unwrap_or(false),
        }
    }

    fn set_model(&self, model: &str) -> Result<()> {
        self.spawn_anthropic_catalog_refresh_if_needed();
        self.spawn_openai_catalog_refresh_if_needed();
        // Model/profile switches change route availability details; rebuild
        // the catalog on next read instead of serving the memoized copy.
        self.invalidate_routes_memo();

        let requested_model = model.trim();
        if requested_model.is_empty() {
            anyhow::bail!("Model cannot be empty");
        }

        if let Some((profile, target_model)) = Self::openai_compatible_model_prefix(requested_model)
        {
            return self.set_model_on_openai_compatible_profile(profile, target_model);
        }

        // User-defined named provider profiles from config (`[providers.<name>]`).
        // The model picker emits `<name>:<model>` specs for their routes
        // (issue #444), so the switch must bind that profile's runtime instead
        // of falling through to global model-name heuristics.
        if let Some((profile_name, target_model)) =
            Self::named_provider_profile_model_prefix(requested_model)
        {
            return self.set_model_on_named_provider_profile(&profile_name, &target_model);
        }

        // Provider-prefixed model names are explicit routing directives. They
        // must never silently fall through to another provider when the target
        // is unavailable.
        if let Some((target, prefix, target_model)) =
            explicit_model_provider_prefix(requested_model)
        {
            // The single canonical parser decides whether this prefix pins a
            // dual-auth credential (and which provider/mode). Bare `claude:` /
            // `openai:` prefixes route without pinning a credential.
            let pinned = jcode_provider_core::AuthRoute::parse_explicit_credential_prefix(prefix);
            let openai_credential_mode = pinned.and_then(|route| {
                matches!(
                    route.provider,
                    jcode_provider_core::DualAuthProvider::OpenAI
                )
                .then(|| match route.mode {
                    jcode_provider_core::AuthMode::ApiKey => openai::OpenAICredentialMode::ApiKey,
                    jcode_provider_core::AuthMode::Oauth => openai::OpenAICredentialMode::OAuth,
                })
            });
            let anthropic_credential_mode = pinned.and_then(|route| {
                matches!(
                    route.provider,
                    jcode_provider_core::DualAuthProvider::Anthropic
                )
                .then(|| match route.mode {
                    jcode_provider_core::AuthMode::ApiKey => {
                        anthropic::AnthropicCredentialMode::ApiKey
                    }
                    jcode_provider_core::AuthMode::Oauth => {
                        anthropic::AnthropicCredentialMode::OAuth
                    }
                })
            });
            if openai_credential_mode.is_some() || anthropic_credential_mode.is_some() {
                return self.set_model_on_provider_with_credential_modes(
                    target,
                    target_model,
                    openai_credential_mode,
                    anthropic_credential_mode,
                );
            }
            return self.set_model_on_provider(target, target_model);
        }

        // A custom OpenAI-compatible endpoint owns opaque, provider-local model
        // IDs. Keep unprefixed names on that active endpoint, even when they
        // resemble a globally known model family. Picker and slash-command
        // route specs carry explicit prefixes, so the branch above can switch
        // to any configured provider at any time.
        if self.active_provider() == ActiveProvider::OpenRouter
            && self
                .active_openrouter_execution_provider()
                .is_some_and(|provider| !provider.supports_provider_routing_features())
        {
            return self.set_model_on_provider(ActiveProvider::OpenRouter, requested_model);
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

        // Detect which provider an unprefixed model belongs to.
        let target_provider = provider_for_model(model);
        if let Some(target_provider) = target_provider
            && let Some(target) = provider_from_model_key(target_provider)
        {
            self.set_model_on_provider(target, model)
        } else {
            // Unknown model - try current provider.
            self.set_model_on_provider(self.active_provider(), model)
        }
    }

    fn set_route_selection(&self, selection: &RouteSelection) -> Result<()> {
        if selection.model.trim().is_empty() {
            anyhow::bail!("Model cannot be empty");
        }

        // Routing-prefix policy lives once in RouteSelection::routed_model_spec
        // so this orchestrator and every single-runtime provider agree on the
        // spec string. set_model then dispatches it to the right sub-provider.
        self.set_model(&selection.routed_model_spec())
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
            ActiveProvider::Bedrock => self
                .bedrock_provider()
                .map(|bedrock| bedrock.available_models_for_switching())
                .unwrap_or_default(),
            ActiveProvider::OpenRouter => self
                .active_openrouter_execution_provider()
                .map(|openrouter| openrouter.available_models_for_switching())
                .unwrap_or_default(),
        }
    }

    fn available_models_display(&self) -> Vec<String> {
        self.fresh_routes_memo_entry().listable_models
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
        self.fresh_routes_memo_entry().routes
    }

    async fn prefetch_models(&self) -> Result<()> {
        let anthropic = self.anthropic_provider();
        let claude = self.claude_provider();
        let openai = self.openai_provider();
        let openrouter = self.openrouter_provider();
        let copilot = self
            .copilot_api
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let antigravity = self.antigravity_provider();
        let gemini = self.gemini_provider();
        let cursor = self.cursor_provider();
        let bedrock = self.bedrock_provider();

        let (
            anthropic_result,
            claude_result,
            openai_result,
            openrouter_result,
            copilot_result,
            antigravity_result,
            gemini_result,
            cursor_result,
            bedrock_result,
        ) = tokio::join!(
            async {
                match anthropic {
                    Some(provider) => provider.prefetch_models().await,
                    None => Ok(()),
                }
            },
            async {
                match claude {
                    Some(provider) => provider.prefetch_models().await,
                    None => Ok(()),
                }
            },
            async {
                match openai {
                    Some(provider) => provider.prefetch_models().await,
                    None => Ok(()),
                }
            },
            async {
                match openrouter {
                    Some(provider) => provider.prefetch_models().await,
                    None => Ok(()),
                }
            },
            async {
                match copilot {
                    Some(provider) => provider.prefetch_models().await,
                    None => Ok(()),
                }
            },
            async {
                match antigravity {
                    Some(provider) => provider.prefetch_models().await,
                    None => Ok(()),
                }
            },
            async {
                match gemini {
                    Some(provider) => provider.prefetch_models().await,
                    None => Ok(()),
                }
            },
            async {
                match cursor {
                    Some(provider) => provider.prefetch_models().await,
                    None => Ok(()),
                }
            },
            async {
                match bedrock {
                    Some(provider) => provider.prefetch_models().await,
                    None => Ok(()),
                }
            },
        );

        let active_provider = self.active_provider();
        let mut errors = Vec::new();
        let mut optional_errors = Vec::new();
        for (provider_name, result) in [
            ("anthropic", anthropic_result),
            ("claude", claude_result),
            ("openai", openai_result),
            ("openrouter", openrouter_result),
            ("copilot", copilot_result),
            ("antigravity", antigravity_result),
            ("gemini", gemini_result),
            ("cursor", cursor_result),
            ("bedrock", bedrock_result),
        ] {
            if let Err(err) = result {
                let is_active = matches!(
                    (active_provider, provider_name),
                    (ActiveProvider::Claude, "anthropic" | "claude")
                        | (ActiveProvider::OpenAI, "openai")
                        | (ActiveProvider::OpenRouter, "openrouter")
                        | (ActiveProvider::Copilot, "copilot")
                        | (ActiveProvider::Antigravity, "antigravity")
                        | (ActiveProvider::Gemini, "gemini")
                        | (ActiveProvider::Cursor, "cursor")
                        | (ActiveProvider::Bedrock, "bedrock")
                );
                if !is_active || matches!(provider_name, "bedrock") {
                    optional_errors.push(format!("{provider_name}: {err}"));
                } else {
                    errors.push(format!("{provider_name}: {err}"));
                }
            }
        }

        if !optional_errors.is_empty() {
            crate::logging::warn(&format!(
                "Optional model catalog refresh failed: {}",
                optional_errors.join("; ")
            ));
        }

        if !errors.is_empty() {
            return Err(anyhow!("{}", errors.join("; ")));
        }

        // Fresh catalogs may have arrived; retire every memoized copy.
        self.invalidate_routes_memo_globally();
        Ok(())
    }

    fn on_auth_changed(&self) {
        self.handle_auth_changed(false);
    }

    fn on_auth_changed_preserve_current_provider(&self) {
        self.handle_auth_changed(true);
    }

    fn auth_model_refresh_pending(&self) -> bool {
        self.post_auth_refreshes_pending
            .load(std::sync::atomic::Ordering::Acquire)
            > 0
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
            ActiveProvider::Bedrock => false, // jcode executes Bedrock tool calls
            ActiveProvider::OpenRouter => false, // jcode executes tools
        }
    }

    fn reasoning_effort(&self) -> Option<String> {
        match self.active_provider() {
            ActiveProvider::Claude => {
                if self.use_claude_cli {
                    None
                } else {
                    self.anthropic_provider()
                        .and_then(|provider| provider.reasoning_effort())
                }
            }
            ActiveProvider::OpenAI => self.openai_provider().and_then(|o| o.reasoning_effort()),
            ActiveProvider::Copilot => None,
            ActiveProvider::Antigravity => None,
            ActiveProvider::Gemini => None,
            ActiveProvider::Cursor => None,
            ActiveProvider::Bedrock => None,
            ActiveProvider::OpenRouter => self
                .active_openrouter_execution_provider()
                .and_then(|o| o.reasoning_effort()),
        }
    }

    fn set_reasoning_effort(&self, effort: &str) -> Result<()> {
        match self.active_provider() {
            ActiveProvider::Claude if !self.use_claude_cli => self
                .anthropic_provider()
                .ok_or_else(|| anyhow::anyhow!("Anthropic provider not available"))?
                .set_reasoning_effort(effort),
            ActiveProvider::OpenAI => self
                .openai_provider()
                .ok_or_else(|| anyhow::anyhow!("OpenAI provider not available"))?
                .set_reasoning_effort(effort),
            ActiveProvider::OpenRouter => self
                .active_openrouter_execution_provider()
                .ok_or_else(|| anyhow::anyhow!("OpenAI-compatible provider not available"))?
                .set_reasoning_effort(effort),
            _ => Err(anyhow::anyhow!(
                "Reasoning effort is only supported for OpenAI, Anthropic, and compatible reasoning models"
            )),
        }
    }

    fn available_efforts(&self) -> Vec<&'static str> {
        match self.active_provider() {
            ActiveProvider::Claude if !self.use_claude_cli => self
                .anthropic_provider()
                .map(|provider| provider.available_efforts())
                .unwrap_or_default(),
            ActiveProvider::OpenAI => self
                .openai_provider()
                .map(|o| o.available_efforts())
                .unwrap_or_default(),
            ActiveProvider::OpenRouter => self
                .active_openrouter_execution_provider()
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
            ActiveProvider::Claude if !self.use_claude_cli => {
                self.anthropic_provider().and_then(|a| a.service_tier())
            }
            ActiveProvider::OpenAI => self.openai_provider().and_then(|o| o.service_tier()),
            _ => None,
        }
    }

    fn set_service_tier(&self, service_tier: &str) -> Result<()> {
        match self.active_provider() {
            ActiveProvider::Claude if !self.use_claude_cli => self
                .anthropic_provider()
                .ok_or_else(|| anyhow::anyhow!("Anthropic provider not available"))?
                .set_service_tier(service_tier),
            ActiveProvider::OpenAI => self
                .openai_provider()
                .ok_or_else(|| anyhow::anyhow!("OpenAI provider not available"))?
                .set_service_tier(service_tier),
            _ => Err(anyhow::anyhow!(
                "Service tier switching is only supported for OpenAI models and Claude Opus 4.8"
            )),
        }
    }

    fn available_service_tiers(&self) -> Vec<&'static str> {
        match self.active_provider() {
            ActiveProvider::Claude if !self.use_claude_cli => self
                .anthropic_provider()
                .map(|a| a.available_service_tiers())
                .unwrap_or_default(),
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
            ActiveProvider::Bedrock => self
                .bedrock_provider()
                .map(|o| o.uses_jcode_compaction())
                .unwrap_or(false),
            ActiveProvider::OpenRouter => self
                .active_openrouter_execution_provider()
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
            ActiveProvider::Bedrock => false,
            ActiveProvider::OpenRouter => self
                .active_openrouter_execution_provider()
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
            ActiveProvider::Bedrock => Err(anyhow::anyhow!(
                "AWS Bedrock does not support native compaction"
            )),
            ActiveProvider::OpenRouter => {
                let provider = self.active_openrouter_execution_provider();
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

    fn set_premium_mode(&self, mode: PremiumMode) {
        if let Some(copilot) = self.copilot_provider() {
            copilot.set_premium_mode(mode);
        }
    }

    fn premium_mode(&self) -> PremiumMode {
        if let Some(copilot) = self.copilot_provider() {
            copilot.premium_mode()
        } else {
            PremiumMode::Normal
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
            ActiveProvider::Bedrock => self
                .bedrock_provider()
                .map(|o| o.context_window())
                .unwrap_or(DEFAULT_CONTEXT_LIMIT),
            ActiveProvider::OpenRouter => self
                .active_openrouter_execution_provider()
                .map(|o| o.context_window())
                .unwrap_or(DEFAULT_CONTEXT_LIMIT),
        }
    }

    fn fork(&self) -> Arc<dyn Provider> {
        let current_model = self.model();
        let active = self.active_provider();

        let claude = if matches!(active, ActiveProvider::Claude) && self.claude_provider().is_some()
        {
            external::instantiate_expected_external_provider(external::CLAUDE_CLI_RUNTIME)
        } else {
            None
        };
        let anthropic = if self.anthropic_provider().is_some() {
            external::instantiate_expected_external_provider(external::ANTHROPIC_RUNTIME)
        } else {
            None
        };
        let openai = if self.openai_provider().is_some() {
            external::instantiate_expected_external_provider(external::OPENAI_RUNTIME)
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
            external::instantiate_expected_external_provider(external::CURSOR_RUNTIME)
        } else {
            None
        };
        let bedrock_provider = if self.bedrock_provider().is_some() {
            Some(Arc::new(bedrock::BedrockProvider::new()))
        } else {
            None
        };
        let openrouter = if self
            .openrouter
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
        {
            external::instantiate_openrouter_runtime(external::OpenRouterRuntimeSpec::Default).ok()
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
            bedrock: RwLock::new(bedrock_provider),
            openrouter: RwLock::new(openrouter),
            openai_compatible_profiles: RwLock::new(HashMap::new()),
            active_openai_compatible_profile: RwLock::new(None),
            active: RwLock::new(active),
            use_claude_cli: self.use_claude_cli,
            startup_notices: RwLock::new(Vec::new()),
            initial_provider: self.initial_provider,
            routes_memo: Mutex::new(None),
            post_auth_refreshes_pending: Arc::clone(&self.post_auth_refreshes_pending),
        };

        provider.spawn_anthropic_catalog_refresh_if_needed();
        provider.spawn_openai_catalog_refresh_if_needed();
        let switch_request = self.fork_model_switch_request(active, &current_model);
        let _ = provider.set_model(&switch_request);
        Arc::new(provider)
    }

    fn fork_for_new_session(&self) -> Arc<dyn Provider> {
        // A shared server can outlive config changes made by a model picker in a
        // client process. Ordinary `fork()` intentionally preserves this
        // template's current selection, which is correct for resumed sessions and
        // in-flight helper work but stale for a brand-new session. Reconstruct the
        // orchestrator so the reloadable config cache and current auth state choose
        // the provider/model again.
        let provider = Self::new_fast();

        // An explicit CLI initial provider/model remains the starting selection
        // for new sessions, while each session can switch freely afterward.
        if self.initial_provider.is_some() {
            let active = self.active_provider();
            let current_model = self.model();
            let switch_request = self.fork_model_switch_request(active, &current_model);
            if let Err(error) = provider.set_model(&switch_request) {
                crate::logging::warn(&format!(
                    "Failed to preserve initial provider model '{}' for new session: {}",
                    switch_request, error
                ));
            }
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
            ActiveProvider::Bedrock => None,
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

/// Get the prompt cache TTL in seconds for a given provider name.
/// Returns None if the provider doesn't support prompt caching or TTL is unknown.
pub fn cache_ttl_for_provider(provider: &str) -> Option<u64> {
    cache_ttl_for_provider_model(provider, None)
}

/// Get the prompt cache TTL in seconds for a given provider/model pair.
///
/// This is provider cache-retention policy: it depends only on provider
/// families (anthropic/openai/...) and their model capabilities, so it lives
/// in `provider` rather than the UI layer.
pub fn cache_ttl_for_provider_model(provider: &str, model: Option<&str>) -> Option<u64> {
    match provider.to_lowercase().as_str() {
        "anthropic" | "claude" => Some(if anthropic::is_cache_ttl_1h() {
            60 * 60
        } else {
            300
        }),
        "openai" => {
            if model
                .map(openai::supports_extended_prompt_cache_retention)
                .unwrap_or(false)
            {
                Some(24 * 60 * 60)
            } else {
                Some(300)
            }
        }
        "openrouter" => Some(300),
        "jcode subscription" => Some(300),
        "gemini" => Some(300),
        "copilot" => None,
        "cursor" => None,
        "antigravity" => None,
        _ => None,
    }
}

#[cfg(test)]
mod tests;
