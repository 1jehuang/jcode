use super::*;

pub(crate) fn normalize_model_id(model: &str) -> String {
    jcode_provider_core::model_id::canonical(model)
}

pub(super) fn normalize_provider_id(provider: &str) -> String {
    provider.trim().to_ascii_lowercase()
}

pub(super) fn openai_account_scope_from_label(label: Option<String>) -> String {
    label
        .map(|label| label.trim().to_string())
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| "default".to_string())
}

/// Catalog scope for the ChatGPT/Codex OAuth route of the active account.
/// Kept as the bare account label for compatibility with existing on-disk
/// snapshots.
pub(super) fn current_openai_account_scope() -> String {
    openai_account_scope_from_label(auth::codex::active_account_label())
}

const OPENAI_API_KEY_SCOPE_PREFIX: &str = "api-key::";

/// Catalog scope for an OpenAI credential.
///
/// The ChatGPT/Codex OAuth catalog (`backend-api/codex/models`) and the
/// platform API-key catalog (`/v1/models`) list different models for the same
/// person, so they must never share a scope. Sharing one let whichever refresh
/// landed last overwrite the other, and switching to a model that only one
/// route offers failed with "Unsupported OpenAI model" until the other route
/// refreshed again. API keys are fingerprinted so changing keys cannot reuse
/// another key's availability; the key itself never becomes a cache key.
pub fn openai_catalog_scope_for_credential(is_chatgpt_mode: bool, access_token: &str) -> String {
    if is_chatgpt_mode {
        current_openai_account_scope()
    } else {
        openai_api_key_catalog_scope(access_token)
    }
}

pub(super) fn openai_api_key_catalog_scope(api_key: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = format!("{:x}", Sha256::digest(api_key.trim().as_bytes()));
    format!("{OPENAI_API_KEY_SCOPE_PREFIX}{}", &digest[..16])
}

pub(super) fn is_openai_api_key_catalog_scope(scope: &str) -> bool {
    scope.starts_with(OPENAI_API_KEY_SCOPE_PREFIX)
}

/// Scope of the configured platform API key, if any.
pub(super) fn configured_openai_api_key_catalog_scope() -> Option<String> {
    crate::provider_catalog::load_api_key_from_env_or_config("OPENAI_API_KEY", "openai.env")
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
        .map(|key| openai_api_key_catalog_scope(&key))
}

/// The `openai-api` OpenAI-compatible profile lists the same platform
/// `/v1/models` for the same key. Use it to seed the API-key scope before the
/// native runtime has fetched its own snapshot, so a model the picker offered
/// from that listing is not rejected against the static fallback list.
pub(super) fn openai_api_profile_cached_model_ids() -> Option<Vec<String>> {
    let resolved = crate::provider_catalog::resolve_openai_compatible_profile(
        crate::provider_catalog::OPENAI_NATIVE_OPENAI_COMPAT_PROFILE,
    );
    super::super::cached_live_models_for_openai_compatible_profile(&resolved)
        .map(|(models, _)| models)
}

pub(super) fn current_claude_account_scope() -> String {
    auth::claude::active_account_label()
        .map(|label| label.trim().to_string())
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| "default".to_string())
}

pub(super) fn current_anthropic_catalog_scope() -> String {
    // Match the credential-resolution order used by the Anthropic provider:
    // the API key can come from the process env *or* the persisted
    // anthropic.env file. Checking only the env var made env-file-keyed
    // sessions read/write the OAuth scope while requests actually used the
    // API key, so the `api-key` catalog scope went permanently stale.
    if super::super::anthropic::load_anthropic_api_key().is_ok() {
        anthropic_catalog_scope_for_route(false)
    } else {
        anthropic_catalog_scope_for_route(true)
    }
}

/// Resolve a catalog scope for the credential route actually being used.
/// API keys are fingerprinted so changing organizations cannot reuse access
/// claims from a different key. OAuth refresh tokens never form cache keys.
pub fn anthropic_catalog_scope_for_route(oauth: bool) -> String {
    if oauth {
        format!("oauth::{}", current_claude_account_scope())
    } else {
        use sha2::{Digest, Sha256};
        let key = super::super::anthropic::load_anthropic_api_key().unwrap_or_default();
        format!("api-key::{:x}", Sha256::digest(key.as_bytes()))
    }
}

pub(super) fn provider_runtime_scope_key(provider: &str, account_label: Option<&str>) -> String {
    let normalized = normalize_provider_id(provider);
    match normalized.as_str() {
        "openai" => format!(
            "openai::{}",
            openai_account_scope_from_label(account_label.map(|label| label.to_string()))
        ),
        "claude" | "anthropic" => format!(
            "claude::{}",
            account_label
                .map(|label| label.trim().to_string())
                .filter(|label| !label.is_empty())
                .unwrap_or_else(current_claude_account_scope)
        ),
        _ => format!("{}::global", normalized),
    }
}

pub(super) fn current_provider_runtime_scope_key(provider: &str) -> String {
    let normalized = normalize_provider_id(provider);
    match normalized.as_str() {
        "openai" => provider_runtime_scope_key(provider, Some(&current_openai_account_scope())),
        "claude" | "anthropic" => {
            provider_runtime_scope_key(provider, Some(&current_claude_account_scope()))
        }
        _ => provider_runtime_scope_key(provider, None),
    }
}

pub(super) fn openai_static_model_ids() -> Vec<String> {
    let mut models: Vec<String> = ALL_OPENAI_MODELS.iter().map(|m| (*m).to_string()).collect();

    // Only advertise the explicit [1m] alias when the live catalog we fetched
    // says this backend exposes a >=1M context window for GPT-5.4.
    if cached_context_limit_for_model("gpt-5.4").unwrap_or_default() >= 1_000_000 {
        if let Some(index) = models.iter().position(|model| model == "gpt-5.4") {
            models.insert(index + 1, "gpt-5.4[1m]".to_string());
        } else {
            models.push("gpt-5.4[1m]".to_string());
        }
    }

    models
}

pub(super) fn anthropic_static_model_ids() -> Vec<String> {
    ALL_CLAUDE_MODELS.iter().map(|m| (*m).to_string()).collect()
}

pub(super) fn model_ids_with_context_aliases(models: Vec<String>) -> Vec<String> {
    let mut deduped = Vec::new();
    let mut seen = HashSet::new();

    for model in models {
        let normalized = normalize_model_id(&model);
        if normalized.is_empty() {
            continue;
        }
        if seen.insert(model.clone()) {
            deduped.push(model);
        }
        if model_exposes_1m_alias(&normalized) {
            let alias = format!("{}[1m]", normalized);
            if seen.insert(alias.clone()) {
                deduped.push(alias);
            }
        }
    }

    deduped
}

/// Whether a `<model>[1m]` long-context picker alias should be surfaced.
///
/// For *known* Claude models this is authoritative: only opt-in 1M models (Opus
/// 4.6, Sonnet 4.6) get an alias. Native-1M models (Opus 4.8, 4.7) already use
/// 1M by default, so a `[1m]` alias would be a redundant duplicate, and
/// 200K-only models (Sonnet 4.5, which the live catalog wrongly advertises as
/// 1M) get no alias. Unknown/future Claude ids and all non-Claude models keep
/// the prior behavior: alias when the cached catalog limit is >= 1M.
pub(super) fn model_exposes_1m_alias(normalized_model: &str) -> bool {
    if normalized_model.starts_with("claude-") {
        let mode = jcode_provider_core::anthropic_context_mode(normalized_model);
        // Only trust the classifier for models it actually recognizes; for
        // anything it maps to `Standard` we can't tell a genuine 200K model from
        // an unrecognized future one, so fall back to the catalog heuristic.
        if mode != jcode_provider_core::AnthropicContextMode::Standard {
            return mode.exposes_1m_alias();
        }
    }
    cached_context_limit_for_model(normalized_model).unwrap_or_default() >= 1_000_000
}

pub(super) fn live_catalog_model_ids(
    service: &ModelCatalogService,
    scope: &str,
) -> Option<Vec<String>> {
    service.model_ids(scope).map(model_ids_with_context_aliases)
}

pub(super) fn load_openai_catalog_from_disk(scope: &str) -> Option<Vec<String>> {
    hydrate_catalog_cache_from_disk(
        OPENAI_MODEL_CATALOG_CACHE_FILE,
        scope,
        &OPENAI_MODEL_CATALOG_SERVICE,
    )
}

pub(super) fn load_anthropic_catalog_from_disk(scope: &str) -> Option<Vec<String>> {
    hydrate_catalog_cache_from_disk(
        ANTHROPIC_MODEL_CATALOG_CACHE_FILE,
        scope,
        &ANTHROPIC_MODEL_CATALOG_SERVICE,
    )
}

pub(super) fn observed_at_unix_secs(observed_at: SystemTime) -> u64 {
    observed_at
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

pub(super) fn system_time_from_unix_secs(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

pub(super) fn model_catalog_cache_path(file_name: &str) -> Result<PathBuf> {
    Ok(crate::storage::app_config_dir()?.join(file_name))
}

pub(super) fn load_persisted_model_catalog_store(
    file_name: &str,
) -> Option<PersistedModelCatalogStore> {
    let path = model_catalog_cache_path(file_name).ok()?;
    crate::storage::read_json(&path).ok()
}

pub(super) fn save_persisted_model_catalog_store(
    file_name: &str,
    store: &(impl Serialize + ?Sized),
) {
    let Ok(path) = model_catalog_cache_path(file_name) else {
        return;
    };
    if let Err(err) = crate::storage::write_json(&path, store) {
        crate::logging::warn(&format!(
            "Failed to persist model catalog cache {}: {}",
            path.display(),
            err
        ));
    }
}

pub(super) fn persist_scoped_model_catalog(
    file_name: &str,
    scope: &str,
    models: &[String],
    context_limits: &HashMap<String, usize>,
    reasoning_efforts: &HashMap<String, Vec<String>>,
    service_tiers: &HashMap<String, Vec<String>>,
    observed_at: SystemTime,
) {
    if models.is_empty() {
        return;
    }

    // Independently refreshed routes share one file. Serialize read-modify-write
    // so simultaneous API/OAuth completions cannot discard the other's scope.
    static STORE_WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = STORE_WRITE_LOCK
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    let mut store = load_persisted_model_catalog_store(file_name).unwrap_or_default();
    let view = borrowed_catalog_store(
        &mut store,
        scope,
        BorrowedScope {
            models,
            context_limits,
            reasoning_efforts,
            service_tiers,
            observed_at_unix_secs: observed_at_unix_secs(observed_at),
        },
    );
    save_persisted_model_catalog_store(file_name, &view);
}

pub(super) fn hydrate_catalog_cache_from_disk(
    file_name: &str,
    scope: &str,
    service: &ModelCatalogService,
) -> Option<Vec<String>> {
    let mut store = load_persisted_model_catalog_store(file_name)?;
    let persisted = store.scopes.remove(scope)?;
    if persisted.models.is_empty() {
        return None;
    }

    let mut normalized = HashSet::new();
    for model in &persisted.models {
        let normalized_model = normalize_model_id(model);
        if !normalized_model.is_empty() {
            normalized.insert(normalized_model);
        }
    }
    if normalized.is_empty() {
        return None;
    }

    let observed_at = system_time_from_unix_secs(persisted.observed_at_unix_secs);
    service.hydrate_scope_models_from_snapshot(scope, normalized, observed_at);
    if !persisted.context_limits.is_empty() {
        populate_context_limits(persisted.context_limits);
    }

    Some(model_ids_with_context_aliases(persisted.models))
}

pub fn cached_anthropic_model_ids() -> Option<Vec<String>> {
    let scope = current_anthropic_catalog_scope();
    cached_anthropic_model_ids_for_scope(&scope)
}

pub fn cached_anthropic_model_ids_for_scope(scope: &str) -> Option<Vec<String>> {
    live_catalog_model_ids(&ANTHROPIC_MODEL_CATALOG_SERVICE, scope)
        .or_else(|| load_anthropic_catalog_from_disk(scope))
}

pub fn cached_openai_model_ids() -> Option<Vec<String>> {
    cached_openai_model_ids_for_scope(&current_openai_account_scope())
}

pub fn cached_openai_model_ids_for_scope(scope: &str) -> Option<Vec<String>> {
    live_catalog_model_ids(&OPENAI_MODEL_CATALOG_SERVICE, scope)
        .or_else(|| load_openai_catalog_from_disk(scope))
        .or_else(|| {
            is_openai_api_key_catalog_scope(scope)
                .then(openai_api_profile_cached_model_ids)
                .flatten()
        })
}

/// Return model-specific OpenAI reasoning capabilities for the active account
/// from its scoped disk snapshot. Live refreshes update provider-local state
/// directly; this keeps startup exact before the first refresh completes.
pub fn cached_openai_reasoning_efforts() -> Option<HashMap<String, Vec<String>>> {
    cached_openai_reasoning_efforts_for_scope(&current_openai_account_scope())
}

pub fn cached_openai_reasoning_efforts_for_scope(
    scope: &str,
) -> Option<HashMap<String, Vec<String>>> {
    let mut store = load_persisted_model_catalog_store(OPENAI_MODEL_CATALOG_CACHE_FILE)?;
    let efforts = store.scopes.remove(scope)?.reasoning_efforts;
    (!efforts.is_empty()).then_some(efforts)
}

/// Return the account-scoped service tier snapshot. Missing model entries are
/// unknown, but present empty vectors are authoritative capability denials.
pub fn cached_openai_service_tiers_for_scope(scope: &str) -> Option<HashMap<String, Vec<String>>> {
    let mut store = load_persisted_model_catalog_store(OPENAI_MODEL_CATALOG_CACHE_FILE)?;
    Some(store.scopes.remove(scope)?.service_tiers)
}

pub fn persist_openai_model_catalog(catalog: &OpenAIModelCatalog) {
    persist_openai_model_catalog_for_scope(&current_openai_account_scope(), catalog);
}

pub fn persist_openai_model_catalog_for_scope(scope: &str, catalog: &OpenAIModelCatalog) {
    persist_scoped_model_catalog(
        OPENAI_MODEL_CATALOG_CACHE_FILE,
        scope,
        &catalog.available_models,
        &catalog.context_limits,
        &catalog.reasoning_efforts,
        &catalog.service_tiers,
        SystemTime::now(),
    );
}

pub fn persist_anthropic_model_catalog(catalog: &AnthropicModelCatalog) {
    persist_anthropic_model_catalog_for_scope(&current_anthropic_catalog_scope(), catalog);
}

pub fn persist_anthropic_model_catalog_for_scope(scope: &str, catalog: &AnthropicModelCatalog) {
    persist_scoped_model_catalog(
        ANTHROPIC_MODEL_CATALOG_CACHE_FILE,
        scope,
        &catalog.available_models,
        &catalog.context_limits,
        &HashMap::new(),
        &HashMap::new(),
        SystemTime::now(),
    );
}

/// Look up a cached context limit for a model.
pub fn cached_context_limit_for_model(model: &str) -> Option<usize> {
    let cache = CONTEXT_LIMIT_CACHE.read().ok()?;
    cache.get(model).copied()
}

/// Populate the context limit cache from API-provided model data.
/// Called once at startup when OpenAI OAuth credentials are available.
pub fn populate_context_limits(models: HashMap<String, usize>) {
    if let Ok(mut cache) = CONTEXT_LIMIT_CACHE.write() {
        for (model, limit) in models {
            crate::logging::info(&format!(
                "Context limit cache: {} = {}k",
                model,
                limit / 1000
            ));
            cache.insert(model, limit);
        }
    }
}

/// Populate the context limit cache from named provider model configs in the
/// user's config file.
///
/// Custom OpenAI-compatible providers that lack a usable `/v1/models` endpoint
/// rely on per-model `context_window` config. That value is honored by the
/// provider instance's own `context_window()` method, but every other
/// resolution path (TUI info widget, compaction budget, model switching) goes
/// through the global [`CONTEXT_LIMIT_CACHE`] via
/// [`context_limit_for_model_with_provider`]. Seed that cache here so the
/// configured limit is respected globally instead of falling back to
/// [`DEFAULT_CONTEXT_LIMIT`].
pub fn populate_context_limits_from_config() {
    populate_context_limits_from_config_value(crate::config::config());
}

/// Seed the global context-limit cache from an explicit config reference.
///
/// Runtime model specs reach the lookup in several shapes, so each configured
/// model is seeded under every key the lookup can normalize to (issue #421):
/// - the bare lowercased id (`qwen3.6-35b-a2000-128k`);
/// - the slash base (`x.gguf` for `/opt/models/x.gguf`), because
///   `model_id_for_capability_lookup` reduces slash-containing ids to their
///   final segment;
/// - the profile-qualified spec (`cachyai-a2000:qwen3.6-35b-a2000-128k`),
///   because session-restored models keep the `<profile>:` routing prefix and
///   non-slash qualified specs are looked up verbatim.
pub fn populate_context_limits_from_config_value(cfg: &crate::config::Config) {
    let mut limits = HashMap::new();
    for (profile_id, provider_cfg) in cfg.providers.iter() {
        for model in &provider_cfg.models {
            let Some(limit) = model.context_window else {
                continue;
            };
            for key in config_context_limit_cache_keys(profile_id, &model.id) {
                limits.insert(key, limit);
            }
        }
    }
    if !limits.is_empty() {
        populate_context_limits(limits);
    }
}

/// Cache keys under which a configured per-model `context_window` must be
/// discoverable so every runtime lookup shape resolves to it. See
/// [`populate_context_limits_from_config_value`].
pub(crate) fn config_context_limit_cache_keys(profile_id: &str, model_id: &str) -> Vec<String> {
    let id = model_id.trim().to_ascii_lowercase();
    if id.is_empty() {
        return Vec::new();
    }
    let mut keys = vec![id.clone()];
    let slash_base = jcode_provider_core::model_id::slash_base(&id).to_string();
    if slash_base != id && !slash_base.is_empty() {
        keys.push(slash_base);
    }
    let profile = profile_id.trim().to_ascii_lowercase();
    if !profile.is_empty() {
        keys.push(format!("{profile}:{id}"));
    }
    keys
}

/// Populate the account-available model list (called once at startup from the Codex API).
pub fn populate_account_models(slugs: Vec<String>) {
    populate_account_models_for_scope(&current_openai_account_scope(), slugs);
}

pub fn populate_anthropic_models(slugs: Vec<String>) {
    populate_anthropic_models_for_scope(&current_anthropic_catalog_scope(), slugs);
}

pub fn populate_account_models_for_scope(scope: &str, slugs: Vec<String>) {
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

        let mut sorted: Vec<String> = normalized.iter().cloned().collect();
        sorted.sort();
        crate::logging::info(&format!(
            "Account available models [{}]: {}",
            scope,
            sorted.join(", ")
        ));
        OPENAI_MODEL_CATALOG_SERVICE.replace_scope_models(
            scope,
            normalized.clone(),
            SystemTime::now(),
        );
        OPENAI_MODEL_CATALOG_SERVICE.note_attempt(scope);
        for model in &normalized {
            OPENAI_MODEL_CATALOG_SERVICE.clear_runtime_model_unavailable(scope, model);
        }
        crate::bus::Bus::global().publish_models_updated();
    }
}

pub fn populate_anthropic_models_for_scope(scope: &str, slugs: Vec<String>) {
    if slugs.is_empty() {
        return;
    }

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

    let mut sorted: Vec<String> = normalized.iter().cloned().collect();
    sorted.sort();
    crate::logging::info(&format!(
        "Anthropic available models [{}]: {}",
        scope,
        sorted.join(", ")
    ));
    ANTHROPIC_MODEL_CATALOG_SERVICE.replace_scope_models(scope, normalized, SystemTime::now());
    crate::bus::Bus::global().publish_models_updated();
}

/// Refresh the OpenAI model catalog in the background.
///
/// `is_chatgpt_mode` is the authoritative discriminator for which endpoint to
/// hit and must come from the loaded credential's shape
/// (`OpenAIProvider::is_chatgpt_mode`), never from sniffing the token string or
/// the requested credential *intent*. ChatGPT/Codex OAuth sessions use the
/// `backend-api/codex/models` endpoint; platform API keys (`sk-*`) use
/// `api.openai.com/v1/models`, which rejects Codex tokens (and vice versa) with
/// a 401.
pub fn refresh_openai_model_catalog_in_background(
    access_token: String,
    is_chatgpt_mode: bool,
    context: &'static str,
) {
    let scope = openai_catalog_scope_for_credential(is_chatgpt_mode, &access_token);
    refresh_openai_model_catalog_in_background_for_scope(
        access_token,
        is_chatgpt_mode,
        context,
        scope,
    );
}

/// Refresh a catalog for a captured credential scope, never the global active login.
pub fn refresh_openai_model_catalog_in_background_for_scope(
    access_token: String,
    is_chatgpt_mode: bool,
    context: &'static str,
    scope: String,
) {
    if access_token.trim().is_empty() {
        finish_openai_model_catalog_refresh_for_scope(&scope);
        return;
    }

    let use_platform_api = !is_chatgpt_mode;

    tokio::spawn(async move {
        let refresh_result = if use_platform_api {
            fetch_openai_api_key_model_catalog(&access_token).await
        } else {
            fetch_openai_model_catalog(&access_token).await
        };
        match refresh_result {
            Ok(catalog)
                if !catalog.available_models.is_empty() || !catalog.context_limits.is_empty() =>
            {
                crate::logging::info(&format!(
                    "Refreshed OpenAI model catalog ({}{}): {} available, {} with context limits",
                    context,
                    if use_platform_api {
                        ", platform-api"
                    } else {
                        ", codex-api"
                    },
                    catalog.available_models.len(),
                    catalog.context_limits.len()
                ));
                persist_openai_model_catalog_for_scope(&scope, &catalog);
                if !catalog.context_limits.is_empty() {
                    populate_context_limits(catalog.context_limits);
                }
                if !catalog.available_models.is_empty() {
                    populate_account_models_for_scope(&scope, catalog.available_models);
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
                    "Failed to refresh OpenAI model catalog from {} ({}): {}",
                    if use_platform_api {
                        "platform API"
                    } else {
                        "Codex API"
                    },
                    context,
                    e
                ));
            }
        }
        note_openai_model_catalog_refresh_attempt_for_scope(&scope);
        finish_openai_model_catalog_refresh_for_scope(&scope);
    });
}

pub fn record_model_unavailable_for_account(model: &str, reason: &str) {
    record_model_unavailable_for_scope(&current_openai_account_scope(), model, reason);
}

pub fn record_model_unavailable_for_scope(scope: &str, model: &str, reason: &str) {
    let model = normalize_model_id(model);
    if model.is_empty() {
        return;
    }
    OPENAI_MODEL_CATALOG_SERVICE.record_runtime_model_unavailable(scope, &model, reason);
    super::super::bump_catalog_generation();
}

pub fn clear_model_unavailable_for_account(model: &str) {
    clear_model_unavailable_for_scope(&current_openai_account_scope(), model);
}

pub fn clear_model_unavailable_for_scope(scope: &str, model: &str) {
    let model = normalize_model_id(model);
    if model.is_empty() {
        return;
    }
    if OPENAI_MODEL_CATALOG_SERVICE.clear_runtime_model_unavailable(scope, &model) {
        super::super::bump_catalog_generation();
    }
}
