use super::*;
use crate::tui::session_picker::{self, OverlayAction, PickerResult, ResumeTarget, SessionPicker};
use crate::tui::{
    AccountPickerAction, InlineInteractiveState, PickerAction, PickerEntry, PickerKind,
    PickerOption,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

#[path = "inline_interactive/helpers.rs"]
mod helpers;
#[path = "inline_interactive/openers.rs"]
mod openers;
#[path = "inline_interactive/preview.rs"]
mod preview;
#[path = "inline_interactive/preview_request.rs"]
mod preview_request;
use helpers::{
    agent_model_default_summary, agent_model_target_label, catchup_candidates,
    catchup_queue_position, model_entry_base_name, model_entry_saved_spec,
    openrouter_route_model_id, picker_route_model_spec, picker_route_selection,
    save_agent_model_override,
};

const REMOTE_MODEL_CATALOG_CACHE_FILE: &str = "remote_model_catalog_cache.json";
const REMOTE_MODEL_CATALOG_CACHE_VERSION: u8 = 3;
const REMOTE_MODEL_CATALOG_CACHE_MAX_AGE_SECS: u64 = 24 * 60 * 60;
const REMOTE_MODEL_CATALOG_CACHE_MAX_BYTES: u64 = 16 * 1024 * 1024;
const REMOTE_MODEL_CATALOG_MAX_ROUTES: usize = 10_000;
const REMOTE_MODEL_CATALOG_MAX_MODEL_BYTES: usize = 4 * 1024;
const REMOTE_MODEL_CATALOG_MAX_PROVIDER_BYTES: usize = 512;
const REMOTE_MODEL_CATALOG_MAX_DETAIL_BYTES: usize = 16 * 1024;
const MODEL_PICKER_USAGE_FILE: &str = "model_picker_usage.json";
const MODEL_PICKER_USAGE_VERSION: u8 = 1;
const MODEL_PICKER_FAVORITES_FILE: &str = "model_picker_favorites.json";
const MODEL_PICKER_FAVORITES_VERSION: u8 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RemoteModelCatalogCache {
    version: u8,
    #[serde(default)]
    origin: String,
    #[serde(flatten)]
    snapshot: jcode_provider_core::ModelCatalogSnapshot,
    observed_at_unix_secs: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ModelPickerUsageEntry {
    count: u32,
    last_selected_unix_secs: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ModelPickerUsageStore {
    version: u8,
    selections: HashMap<String, ModelPickerUsageEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ModelPickerFavoritesStore {
    version: u8,
    favorites: HashSet<String>,
}

fn model_picker_usage_path() -> Option<std::path::PathBuf> {
    crate::storage::app_config_dir()
        .ok()
        .map(|dir| dir.join(MODEL_PICKER_USAGE_FILE))
}

fn model_picker_favorites_path() -> Option<std::path::PathBuf> {
    crate::storage::app_config_dir()
        .ok()
        .map(|dir| dir.join(MODEL_PICKER_FAVORITES_FILE))
}

/// Whether a picker route's runtime can apply a per-request reasoning effort.
/// Effort rows are only rendered for these routes; other routes (Copilot,
/// Bedrock, Antigravity CLI, remote-catalog placeholders, ...) get one plain
/// row per model because a picked effort could not actually be applied.
fn route_supports_reasoning_effort(api_method: &str) -> bool {
    use crate::provider::ModelRouteApiMethod as Method;
    match Method::parse(api_method) {
        Method::ClaudeOAuth
        | Method::AnthropicApiKey
        | Method::OpenAIOAuth
        | Method::OpenAIApiKey
        | Method::OpenRouter
        | Method::OpenAiCompatible {
            profile_id: Some(_),
        } => true,
        Method::JcodeSubscription
        | Method::OpenAiCompatible { profile_id: None }
        | Method::Copilot
        | Method::Cursor
        | Method::Bedrock
        | Method::CodeAssistOAuth
        | Method::AntigravityHttps
        | Method::RemoteCatalog
        | Method::Current
        | Method::Other(_) => false,
    }
}

/// Apply the `provider.model_picker_providers` allowlist (issue #460).
///
/// Each allowlist entry can name a provider label ("openai", "llama.cpp",
/// "anthropic"), a route api method ("claude-oauth", "openrouter",
/// "openai-compatible:myprofile"), or a bare openai-compatible profile id
/// ("myprofile"). Matching is case/format-insensitive via the shared provider
/// label normalizer. Routes for the active model are always kept so the
/// current selection never disappears from the picker, and a filter that
/// matches nothing falls back to the unfiltered list instead of an empty
/// picker.
fn filter_routes_by_provider_allowlist(
    routes: Vec<crate::provider::ModelRoute>,
    allowlist: Option<&[String]>,
    current_model: &str,
) -> Vec<crate::provider::ModelRoute> {
    use crate::provider::normalize_model_route_provider_label as normalize;

    let Some(allowlist) = allowlist else {
        return routes;
    };
    let allowed: Vec<String> = allowlist
        .iter()
        .map(|entry| normalize(entry))
        .filter(|entry| !entry.is_empty())
        .collect();
    if allowed.is_empty() {
        return routes;
    }

    let route_matches = |route: &crate::provider::ModelRoute| -> bool {
        let provider = normalize(&route.provider);
        let api_method = normalize(&route.api_method);
        // "openai-compatible:myprofile" normalizes to "openaicompatible:myprofile";
        // also expose the bare profile id for convenience.
        let profile_id = route
            .api_method
            .split_once(':')
            .map(|(_, profile)| normalize(profile))
            .unwrap_or_default();
        allowed.iter().any(|entry| {
            *entry == provider
                || *entry == api_method
                || (!profile_id.is_empty() && *entry == profile_id)
                || crate::provider::model_route_provider_labels_match(&route.provider, entry)
        })
    };

    let filtered: Vec<crate::provider::ModelRoute> = routes
        .iter()
        .filter(|route| route.model == current_model || route_matches(route))
        .cloned()
        .collect();
    if filtered.is_empty() {
        routes
    } else {
        filtered
    }
}

fn model_picker_usage_key(model_name: &str, route: &PickerOption, effort: Option<&str>) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}",
        model_name,
        route.provider,
        route.api_method,
        effort.unwrap_or("")
    )
}

fn load_model_picker_usage_store() -> ModelPickerUsageStore {
    let Some(path) = model_picker_usage_path() else {
        return ModelPickerUsageStore::default();
    };
    let Ok(bytes) = std::fs::read(path) else {
        return ModelPickerUsageStore::default();
    };
    let Ok(mut store) = serde_json::from_slice::<ModelPickerUsageStore>(&bytes) else {
        return ModelPickerUsageStore::default();
    };
    if store.version != MODEL_PICKER_USAGE_VERSION {
        return ModelPickerUsageStore::default();
    }
    store.selections.retain(|_, entry| entry.count > 0);
    store
}

fn save_model_picker_usage_store(store: &ModelPickerUsageStore) {
    let Some(path) = model_picker_usage_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_vec_pretty(store) {
        let _ = std::fs::write(path, json);
    }
}

fn model_picker_usage_score(
    store: &ModelPickerUsageStore,
    model_name: &str,
    route: &PickerOption,
    effort: Option<&str>,
) -> u32 {
    store
        .selections
        .get(&model_picker_usage_key(model_name, route, effort))
        .map(|entry| entry.count.saturating_mul(100).saturating_add(50))
        .unwrap_or(0)
}

fn record_model_picker_selection(model_name: &str, route: &PickerOption, effort: Option<&str>) {
    let mut store = load_model_picker_usage_store();
    store.version = MODEL_PICKER_USAGE_VERSION;
    let key = model_picker_usage_key(model_name, route, effort);
    let entry = store.selections.entry(key).or_default();
    entry.count = entry.count.saturating_add(1);
    entry.last_selected_unix_secs = remote_model_catalog_observed_at_unix_secs();
    save_model_picker_usage_store(&store);
}

fn load_model_picker_favorites_store() -> ModelPickerFavoritesStore {
    let Some(path) = model_picker_favorites_path() else {
        return ModelPickerFavoritesStore::default();
    };
    let Ok(bytes) = std::fs::read(path) else {
        return ModelPickerFavoritesStore::default();
    };
    let Ok(mut store) = serde_json::from_slice::<ModelPickerFavoritesStore>(&bytes) else {
        return ModelPickerFavoritesStore::default();
    };
    if store.version != MODEL_PICKER_FAVORITES_VERSION {
        return ModelPickerFavoritesStore::default();
    }
    store.favorites.retain(|key| !key.trim().is_empty());
    store
}

fn save_model_picker_favorites_store(store: &ModelPickerFavoritesStore) {
    let Some(path) = model_picker_favorites_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_vec_pretty(store) {
        let _ = std::fs::write(path, json);
    }
}

fn model_picker_is_favorite(
    store: &ModelPickerFavoritesStore,
    model_name: &str,
    route: &PickerOption,
    effort: Option<&str>,
) -> bool {
    store
        .favorites
        .contains(&model_picker_usage_key(model_name, route, effort))
}

fn picker_is_runtime_model_picker(picker: &InlineInteractiveState) -> bool {
    picker.kind == PickerKind::Model
        && picker
            .entries
            .iter()
            .any(|entry| matches!(entry.action, PickerAction::Model))
}

fn key_char_eq_ignore_ascii_case(code: KeyCode, expected: char) -> bool {
    matches!(code, KeyCode::Char(c) if c.eq_ignore_ascii_case(&expected))
}

fn remote_model_catalog_cache_path() -> Option<std::path::PathBuf> {
    crate::storage::app_config_dir()
        .ok()
        .map(|dir| dir.join(REMOTE_MODEL_CATALOG_CACHE_FILE))
}

fn remote_model_catalog_cache_origin() -> String {
    crate::server::socket_path().to_string_lossy().into_owned()
}

fn remote_catalog_text_is_safe(value: &str, max_bytes: usize, allow_empty: bool) -> bool {
    value.len() <= max_bytes
        && (allow_empty || !value.trim().is_empty())
        && !value.chars().any(char::is_control)
}

fn remote_catalog_api_method_is_safe(api_method: &str) -> bool {
    use crate::provider::ModelRouteApiMethod as Method;
    match Method::parse(api_method) {
        Method::Other(_) | Method::Current => false,
        Method::OpenAiCompatible {
            profile_id: Some(profile_id),
        } => {
            profile_id.len() <= 128
                && profile_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        }
        _ => true,
    }
}

fn remote_model_catalog_snapshot_is_safe(
    snapshot: &jcode_provider_core::ModelCatalogSnapshot,
) -> bool {
    if snapshot.available_models.len() > REMOTE_MODEL_CATALOG_MAX_ROUTES
        || snapshot.model_routes.len() > REMOTE_MODEL_CATALOG_MAX_ROUTES
    {
        return false;
    }
    if snapshot.provider_name.as_deref().is_some_and(|value| {
        !remote_catalog_text_is_safe(value, REMOTE_MODEL_CATALOG_MAX_PROVIDER_BYTES, false)
    }) || snapshot.provider_model.as_deref().is_some_and(|value| {
        !remote_catalog_text_is_safe(value, REMOTE_MODEL_CATALOG_MAX_MODEL_BYTES, false)
    }) {
        return false;
    }
    if snapshot.available_models.iter().any(|model| {
        !remote_catalog_text_is_safe(model, REMOTE_MODEL_CATALOG_MAX_MODEL_BYTES, false)
    }) {
        return false;
    }
    snapshot.model_routes.iter().all(|route| {
        remote_catalog_text_is_safe(&route.model, REMOTE_MODEL_CATALOG_MAX_MODEL_BYTES, false)
            && remote_catalog_text_is_safe(
                &route.provider,
                REMOTE_MODEL_CATALOG_MAX_PROVIDER_BYTES,
                false,
            )
            && remote_catalog_text_is_safe(
                &route.api_method,
                REMOTE_MODEL_CATALOG_MAX_PROVIDER_BYTES,
                false,
            )
            && remote_catalog_text_is_safe(
                &route.detail,
                REMOTE_MODEL_CATALOG_MAX_DETAIL_BYTES,
                true,
            )
            && remote_catalog_api_method_is_safe(&route.api_method)
    })
}

fn remote_model_catalog_cache_is_fresh(cache: &RemoteModelCatalogCache, now: u64) -> bool {
    cache.observed_at_unix_secs <= now.saturating_add(5 * 60)
        && now.saturating_sub(cache.observed_at_unix_secs)
            <= REMOTE_MODEL_CATALOG_CACHE_MAX_AGE_SECS
}

fn remote_model_catalog_observed_at_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn model_picker_route_is_current(
    model_name: &str,
    route: &PickerOption,
    current_model: &str,
    current_provider: &str,
) -> bool {
    if model_name != current_model {
        return false;
    }
    // Remote sessions whose catalog arrives as names-only do not carry a
    // provider name, so `current_provider` is the generic "remote"
    // placeholder. The model name was synthesized into a real provider route
    // (Copilot/OpenAI/...), so a provider-label comparison would never match
    // and the current model would not preselect. Fall back to name-only
    // matching in that case.
    if current_provider.trim().eq_ignore_ascii_case("remote") {
        return true;
    }
    jcode_provider_core::model_route_provider_labels_match(&route.provider, current_provider)
}

const RECOMMENDED_MODELS: &[&str] = &["gpt-5.5", "claude-opus-4-8"];

fn model_picker_recommendation_rank(name: &str) -> usize {
    RECOMMENDED_MODELS
        .iter()
        .position(|model| *model == name)
        .unwrap_or(usize::MAX)
}

fn model_picker_route_is_recommended(model_name: &str, route: &PickerOption) -> bool {
    RECOMMENDED_MODELS.contains(&model_name)
        && jcode_provider_core::model_route_metadata_is_recommended(
            model_name,
            &route.provider,
            &route.api_method,
            route.available,
        )
}

fn model_picker_provider_hint_from_model_spec(model_spec: &str) -> Option<(&str, &str)> {
    let (provider_hint, bare_model) = model_spec.split_once(':')?;
    let provider_hint = provider_hint.trim();
    let bare_model = bare_model.trim();
    if provider_hint.is_empty() || bare_model.is_empty() {
        return None;
    }

    let normalized = provider_hint.to_ascii_lowercase();
    if matches!(
        normalized.as_str(),
        "claude"
            | "anthropic"
            | "openai"
            | "copilot"
            | "cursor"
            | "antigravity"
            | "bedrock"
            | "openrouter"
            | "gemini"
    ) || crate::provider_catalog::openai_compatible_profile_by_id(provider_hint).is_some()
    {
        Some((provider_hint, bare_model))
    } else {
        None
    }
}

fn model_picker_route_provider_matches_key(
    route_provider_key: Option<&str>,
    route_provider_label: &str,
    desired_provider: &str,
) -> bool {
    jcode_provider_core::model_route_provider_matches_key(
        route_provider_key,
        route_provider_label,
        desired_provider,
    )
}

fn model_picker_route_is_default(
    model_name: &str,
    route: &PickerOption,
    config_default_model: Option<&str>,
    config_default_provider: Option<&str>,
) -> bool {
    let Some(default_model) = config_default_model
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return false;
    };

    let selection = crate::provider::MultiProvider::default_model_selection_from_route(
        model_name,
        &route.api_method,
        &route.provider,
    );
    let provider_matches = |provider: &str| {
        model_picker_route_provider_matches_key(
            selection.provider_key.as_deref(),
            &route.provider,
            provider,
        )
    };

    let model_matches_bare_or_exact = default_model == selection.model_spec
        || default_model == model_name
        || model_picker_provider_hint_from_model_spec(default_model)
            .map(|(_, bare_model)| bare_model == model_name)
            .unwrap_or(false);

    if let Some(default_provider) = config_default_provider
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return model_matches_bare_or_exact && provider_matches(default_provider);
    }

    if default_model == selection.model_spec {
        return true;
    }

    if let Some((provider_hint, bare_model)) =
        model_picker_provider_hint_from_model_spec(default_model)
    {
        return bare_model == model_name && provider_matches(provider_hint);
    }

    if let Some((bare_model, provider_label)) = default_model.rsplit_once('@') {
        return bare_model == model_name
            && jcode_provider_core::model_route_provider_labels_match(
                &route.provider,
                provider_label,
            );
    }

    // Legacy configs may only contain a bare model. In that case the persisted
    // data cannot identify the route, so keep the previous model-only marker.
    default_model == model_name
}

impl App {
    pub(super) fn remote_model_catalog_snapshot(
        &self,
    ) -> jcode_provider_core::ModelCatalogSnapshot {
        jcode_provider_core::ModelCatalogSnapshot::new(
            self.remote_provider_name.clone(),
            self.remote_provider_model.clone(),
            self.remote_available_entries.clone(),
            self.remote_model_options.clone(),
        )
    }

    pub(super) fn replace_remote_model_catalog_snapshot(
        &mut self,
        snapshot: jcode_provider_core::ModelCatalogSnapshot,
    ) -> bool {
        let mut provider_meta_changed = false;
        let mut provider_name_changed = false;
        if let Some(name) = snapshot.provider_name
            && self.remote_provider_name.as_deref() != Some(name.as_str())
        {
            self.remote_provider_name = Some(name);
            provider_meta_changed = true;
            provider_name_changed = true;
        }
        if let Some(model) = snapshot.provider_model
            && self.remote_provider_model.as_deref() != Some(model.as_str())
        {
            self.update_context_limit_for_model(&model);
            self.remote_provider_model = Some(model);
            provider_meta_changed = true;
        }
        // A names-only snapshot (models without route expansion) arrives when the
        // server downgrades an oversized AvailableModelsUpdated frame. Keep the
        // previously known detailed routes in that case; the picker synthesizes
        // fallback routes for any newly appearing models. If the provider
        // identity changed, the old routes are stale and must be dropped.
        let names_only = snapshot.model_routes.is_empty() && !snapshot.available_models.is_empty();
        self.remote_available_entries = snapshot.available_models;
        if !names_only || provider_name_changed {
            self.remote_model_options = snapshot.model_routes;
        }
        self.invalidate_model_picker_cache();
        provider_meta_changed
    }

    /// Ensure every advertised remote model has at least one picker route.
    ///
    /// Detailed route expansion can lag behind the model-name catalog (stale
    /// disk cache, names-only catalog updates). Without this, newly released
    /// models are invisible in the picker even though the server lists them.
    ///
    /// A model also needs re-synthesis when its persisted routes predate an
    /// auth method: an older session may have baked an OAuth-only fallback
    /// route into the cache, which would otherwise permanently hide the
    /// API-key route for that model.
    fn append_jcode_subscription_routes(
        &self,
        routes: &mut Vec<crate::provider::ModelRoute>,
        require_credentials: bool,
        require_remote_advertisement: bool,
    ) {
        if require_credentials && !crate::subscription_catalog::has_credentials() {
            return;
        }

        let tier = crate::subscription_catalog::effective_tier();
        let existing = routes
            .iter()
            .filter(|route| {
                route
                    .api_method
                    .eq_ignore_ascii_case(crate::subscription_catalog::JCODE_ROUTE_API_METHOD)
            })
            .filter_map(|route| crate::subscription_catalog::canonical_model_id(&route.model))
            .collect::<HashSet<_>>();
        for model in crate::subscription_catalog::curated_models()
            .iter()
            .filter(|model| {
                tier.allows(model.min_tier)
                    && !existing.contains(model.id)
                    && (!require_remote_advertisement
                        || self.remote_available_entries.iter().any(|available| {
                            crate::subscription_catalog::canonical_model_id(available)
                                == Some(model.id)
                        }))
            })
        {
            routes.push(crate::provider::ModelRoute {
                model: model.id.to_string(),
                provider: crate::subscription_catalog::JCODE_PROVIDER_DISPLAY_NAME.to_string(),
                api_method: crate::subscription_catalog::JCODE_ROUTE_API_METHOD.to_string(),
                available: true,
                detail: crate::subscription_catalog::routing_policy_detail(model),
                cheapness: None,
            });
        }
    }

    fn extend_remote_routes_for_uncovered_models(
        &self,
        routes: &mut Vec<crate::provider::ModelRoute>,
    ) {
        if !self.is_remote || self.remote_available_entries.is_empty() {
            return;
        }
        // Jcode subscription routes are a complete, server-managed catalog.
        // Do not mix in locally configured Anthropic/OpenAI credentials merely
        // because a curated model also belongs to one of those upstreams.
        let provider_is_jcode_subscription =
            self.remote_provider_name.as_deref().is_some_and(|name| {
                name.eq_ignore_ascii_case(crate::subscription_catalog::JCODE_PROVIDER_DISPLAY_NAME)
            });
        if provider_is_jcode_subscription {
            routes.clear();
            self.append_jcode_subscription_routes(routes, false, false);
            return;
        }
        let poisoned_by_jcode_subscription = !routes.is_empty()
            && routes.iter().all(|route| {
                route
                    .api_method
                    .eq_ignore_ascii_case(crate::subscription_catalog::JCODE_ROUTE_API_METHOD)
            });
        if poisoned_by_jcode_subscription {
            // Version 1 could turn a mixed provider catalog into all-Jcode rows
            // after seeing just one managed subscription route. Rebuild ordinary
            // routes from the names catalog, then append only the current tier's
            // actual subscription entitlements.
            *routes = crate::provider::remote_model_routes_fallback(
                self.remote_provider_name.as_deref(),
                &self.remote_available_entries,
            );
            self.append_jcode_subscription_routes(routes, false, true);
            return;
        }
        let mut methods_by_model: std::collections::HashMap<&str, HashSet<&str>> =
            std::collections::HashMap::new();
        for route in routes.iter() {
            methods_by_model
                .entry(route.model.as_str())
                .or_default()
                .insert(route.api_method.as_str());
        }
        let auth = crate::auth::AuthStatus::check_fast();
        let bedrock_available = auth.bedrock != crate::auth::AuthState::NotConfigured
            || crate::provider::bedrock::BedrockProvider::has_credentials();
        let missing: Vec<String> = self
            .remote_available_entries
            .iter()
            .filter(|model| match methods_by_model.get(model.as_str()) {
                None => true,
                Some(methods) => {
                    let missing_anthropic_method = crate::provider::provider_for_model(model)
                        == Some("claude")
                        && !model.contains('/')
                        && ((auth.anthropic.has_api_key && !methods.contains("claude-api"))
                            || (auth.anthropic.has_oauth && !methods.contains("claude-oauth")));
                    let missing_bedrock_method = bedrock_available
                        && crate::provider::bedrock::BedrockProvider::is_bedrock_model_id(model)
                        && !methods.contains("bedrock");
                    missing_anthropic_method || missing_bedrock_method
                }
            })
            .cloned()
            .collect();
        if !missing.is_empty() {
            let existing: HashSet<(String, String, String)> = routes
                .iter()
                .map(|route| {
                    (
                        route.model.clone(),
                        route.provider.clone(),
                        route.api_method.clone(),
                    )
                })
                .collect();
            for route in crate::provider::remote_model_routes_fallback(
                self.remote_provider_name.as_deref(),
                &missing,
            ) {
                if !existing.contains(&(
                    route.model.clone(),
                    route.provider.clone(),
                    route.api_method.clone(),
                )) {
                    routes.push(route);
                }
            }
        }
        // Detailed provider hydration describes ordinary configured routes. A
        // signed-in Jcode subscriber still needs the managed route for each
        // entitled curated model alongside those Anthropic/OpenAI/etc. rows.
        // The curated client catalog is versioned with the backend and is the
        // authority for managed subscription entitlements. Do not hide newly
        // launched subscription models behind a stale remote names snapshot.
        self.append_jcode_subscription_routes(routes, true, false);
    }

    fn hydrate_remote_model_catalog_snapshot(
        &mut self,
        snapshot: jcode_provider_core::ModelCatalogSnapshot,
    ) -> bool {
        if !snapshot.has_routes() {
            return false;
        }

        if self.remote_provider_name.is_none() {
            self.remote_provider_name = snapshot.provider_name;
        }
        if self.remote_provider_model.is_none() {
            self.remote_provider_model = snapshot.provider_model;
        }
        if self.remote_available_entries.is_empty() {
            self.remote_available_entries = snapshot.available_models;
        }
        self.remote_model_options = snapshot.model_routes;
        self.invalidate_model_picker_cache();
        true
    }

    pub(super) fn persist_remote_model_catalog_cache(&self) {
        if !self.is_remote || self.remote_model_options.is_empty() {
            return;
        }

        let snapshot = self.remote_model_catalog_snapshot();
        if !remote_model_catalog_snapshot_is_safe(&snapshot) {
            crate::logging::warn("Refusing to persist an invalid remote model catalog");
            return;
        }

        let Some(path) = remote_model_catalog_cache_path() else {
            return;
        };
        let cache = RemoteModelCatalogCache {
            version: REMOTE_MODEL_CATALOG_CACHE_VERSION,
            origin: remote_model_catalog_cache_origin(),
            snapshot,
            observed_at_unix_secs: remote_model_catalog_observed_at_unix_secs(),
        };
        if let Err(error) = crate::storage::write_json(&path, &cache) {
            crate::logging::warn(&format!(
                "Failed to persist remote model catalog cache {}: {}",
                path.display(),
                error
            ));
        }
    }

    fn hydrate_remote_model_catalog_cache(&mut self) -> bool {
        if !self.is_remote || !self.remote_model_options.is_empty() {
            return false;
        }

        let Some(path) = remote_model_catalog_cache_path() else {
            return false;
        };
        if std::fs::metadata(&path)
            .ok()
            .is_some_and(|metadata| metadata.len() > REMOTE_MODEL_CATALOG_CACHE_MAX_BYTES)
        {
            crate::logging::warn(&format!(
                "Ignoring oversized remote model catalog cache {}",
                path.display()
            ));
            return false;
        }
        let Ok(cache) = crate::storage::read_json::<RemoteModelCatalogCache>(&path) else {
            return false;
        };
        if cache.version != REMOTE_MODEL_CATALOG_CACHE_VERSION
            || cache.origin != remote_model_catalog_cache_origin()
            || !remote_model_catalog_cache_is_fresh(
                &cache,
                remote_model_catalog_observed_at_unix_secs(),
            )
            || !remote_model_catalog_snapshot_is_safe(&cache.snapshot)
        {
            return false;
        }

        self.hydrate_remote_model_catalog_snapshot(cache.snapshot)
    }

    pub(super) fn invalidate_model_picker_cache(&mut self) {
        self.model_picker_cache = None;
        self.model_picker_catalog_revision = self.model_picker_catalog_revision.wrapping_add(1);
        self.pending_model_picker_load = None;
        self.model_picker_load_request_id = self.model_picker_load_request_id.wrapping_add(1);
    }

    fn model_route_cache_marker(route: &crate::provider::ModelRoute) -> String {
        format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
            route.model, route.provider, route.api_method, route.available, route.detail
        )
    }

    fn model_picker_cache_signature(
        &self,
        current_model: &str,
        config_default_model: Option<String>,
        config_default_provider: Option<String>,
        current_effort: Option<String>,
        available_efforts: &[&str],
    ) -> ModelPickerCacheSignature {
        ModelPickerCacheSignature {
            is_remote: self.is_remote,
            provider_name: if self.is_remote {
                self.remote_provider_name
                    .clone()
                    .unwrap_or_else(|| "remote".to_string())
            } else {
                self.provider.name().to_string()
            },
            current_model: current_model.to_string(),
            config_default_model,
            config_default_provider,
            reasoning_effort: current_effort,
            available_efforts: available_efforts
                .iter()
                .map(|effort| (*effort).to_string())
                .collect(),
            simplified_model_picker: crate::perf::tui_policy().simplified_model_picker,
            catalog_revision: self.model_picker_catalog_revision,
            remote_provider_name: self.remote_provider_name.clone(),
            remote_available_len: self.remote_available_entries.len(),
            remote_available_first: self.remote_available_entries.first().cloned(),
            remote_available_last: self.remote_available_entries.last().cloned(),
            remote_routes_len: self.remote_model_options.len(),
            remote_routes_first: self
                .remote_model_options
                .first()
                .map(Self::model_route_cache_marker),
            remote_routes_last: self
                .remote_model_options
                .last()
                .map(Self::model_route_cache_marker),
        }
    }

    fn open_cached_model_picker_if_fresh(
        &mut self,
        signature: &ModelPickerCacheSignature,
        picker_started: std::time::Instant,
        preserve_input: bool,
    ) -> bool {
        let Some(cache) = self.model_picker_cache.as_ref() else {
            return false;
        };
        if cache.signature != *signature {
            return false;
        }

        let entries = cache.entries.clone();
        let entry_count = entries.len();
        let route_count = cache.route_count;
        let model_count = cache.model_count;
        self.inline_view_state = None;
        self.inline_interactive_state = Some(InlineInteractiveState {
            kind: PickerKind::Model,
            filtered: (0..entry_count).collect(),
            entries,
            selected: 0,
            column: 0,
            filter: String::new(),
            preview: false,
        });
        if !preserve_input {
            self.input.clear();
            self.cursor_pos = 0;
        }

        if std::env::var("JCODE_LOG_MODEL_PICKER_TIMING").is_ok() {
            crate::logging::info(&format!(
                "[TIMING] model_picker_open: cache_hit=true, remote={}, simplified={}, routes={}, models={}, entries={}, total={}ms",
                self.is_remote,
                crate::perf::tui_policy().simplified_model_picker,
                route_count,
                model_count,
                entry_count,
                picker_started.elapsed().as_millis(),
            ));
        }
        true
    }

    fn should_cache_model_picker_entries(model_count: usize, route_count: usize) -> bool {
        // A single model/route result is commonly a startup fallback (for example, the
        // current model while the real provider catalog is still loading). Caching that
        // fallback makes `/model` look permanently collapsed to just the active model.
        model_count > 1 && route_count > 1
    }

    fn simplified_model_routes_for_picker(
        &self,
        current_model: &str,
    ) -> Vec<crate::provider::ModelRoute> {
        crate::provider::simplified_model_routes_for_picker(
            self.provider.name(),
            current_model,
            self.provider.available_models_display(),
        )
    }

    pub(super) fn open_model_picker(&mut self) {
        self.open_model_picker_inner(false);
    }

    fn open_model_picker_preserving_input(&mut self) {
        self.open_model_picker_inner(true);
    }

    /// Apply a completed auth-driven catalog refresh to an already-open picker.
    /// Login/import can finish while `/model` is visible, so merely invalidating
    /// its cache would leave the stale pre-login entries on screen until the user
    /// closed and reopened it.
    pub(super) fn finish_auth_catalog_refresh(&mut self) {
        let was_pending = self.auth_catalog_refresh_pending;
        self.auth_catalog_refresh_pending = false;
        self.invalidate_model_picker_cache();
        let picker_open = self
            .inline_interactive_state
            .as_ref()
            .is_some_and(picker_is_runtime_model_picker);
        if was_pending && picker_open {
            self.open_model_picker_preserving_input();
        }
    }

    fn open_model_picker_inner(&mut self, preserve_input: bool) {
        let picker_started = std::time::Instant::now();
        const RECENT_AUTH_BOOST_TTL: std::time::Duration = std::time::Duration::from_secs(5 * 60);
        if self
            .recent_authenticated_provider
            .as_ref()
            .map(|(_, at)| at.elapsed() > RECENT_AUTH_BOOST_TTL)
            .unwrap_or(false)
        {
            self.recent_authenticated_provider = None;
            self.invalidate_model_picker_cache();
        }

        if self.is_remote && self.remote_model_options.is_empty() {
            self.hydrate_remote_model_catalog_cache();
        }

        let current_model = if self.is_remote {
            self.remote_provider_model
                .clone()
                .unwrap_or_else(|| "unknown".to_string())
        } else {
            self.provider.model().to_string()
        };

        // Never present the old catalog as authoritative immediately after a
        // login/import. Local mode clears this when the provider's synchronous
        // auth activation finishes; remote mode clears it when the server sends
        // the final catalog activity after all provider work completes.
        if self.auth_catalog_refresh_pending {
            self.open_loading_model_picker(&current_model);
            return;
        }

        let config = crate::config::config();
        let config_default_model = config.provider.default_model.clone();
        let config_default_provider = config.provider.default_provider.clone();

        let current_effort = if self.is_remote {
            self.remote_reasoning_effort.clone()
        } else {
            self.provider.reasoning_effort()
        };
        let available_efforts = if self.is_remote {
            inferred_reasoning_efforts(
                self.remote_provider_name.as_deref(),
                self.remote_provider_model.as_deref(),
            )
        } else {
            self.provider.available_efforts()
        };

        let cache_signature = self.model_picker_cache_signature(
            &current_model,
            config_default_model.clone(),
            config_default_provider.clone(),
            current_effort.clone(),
            &available_efforts,
        );
        if self.open_cached_model_picker_if_fresh(&cache_signature, picker_started, preserve_input)
        {
            return;
        }

        if !self.is_remote && !crate::perf::tui_policy().simplified_model_picker {
            let routes_started = std::time::Instant::now();
            let routes = self.simplified_model_routes_for_picker(&current_model);
            let routes_ms = routes_started.elapsed().as_millis();
            self.open_model_picker_with_routes(
                cache_signature.clone(),
                picker_started,
                routes,
                routes_ms,
                preserve_input,
                false,
            );
            if self.inline_interactive_state.is_some() {
                self.set_status_notice("Updating model routes…");
            } else {
                self.open_loading_model_picker(&current_model);
            }
            self.start_model_picker_route_load(cache_signature, picker_started);
            return;
        }

        let routes_started = std::time::Instant::now();
        let routes: Vec<crate::provider::ModelRoute> = if self.is_remote {
            if !self.remote_model_options.is_empty() {
                let mut routes = std::mem::take(&mut self.remote_model_options);
                self.extend_remote_routes_for_uncovered_models(&mut routes);
                let routes_ms = routes_started.elapsed().as_millis();
                self.remote_model_options = self.open_model_picker_with_routes(
                    cache_signature,
                    picker_started,
                    routes,
                    routes_ms,
                    preserve_input,
                    true,
                );
                return;
            }
            // Names-only remote catalog: synthesize properly classified
            // provider routes (Comtegra/Copilot/Bedrock/Gemini/OpenRouter/…)
            // rather than a generic "remote-catalog" placeholder. This is the
            // final route set for this open (there is no async upgrade after
            // it), and the full fallback only reads local config/disk caches,
            // so it is cheap enough for the cold-open path.
            self.build_remote_model_routes_fallback()
        } else {
            self.simplified_model_routes_for_picker(&current_model)
        };
        let routes_ms = routes_started.elapsed().as_millis();

        let _ = self.open_model_picker_with_routes(
            cache_signature,
            picker_started,
            routes,
            routes_ms,
            preserve_input,
            true,
        );
    }

    fn open_loading_model_picker(&mut self, current_model: &str) {
        let model_label = if current_model.trim().is_empty() || current_model == "unknown" {
            "Loading models…".to_string()
        } else {
            current_model.to_string()
        };
        self.inline_view_state = None;
        self.inline_interactive_state = Some(InlineInteractiveState {
            kind: PickerKind::Model,
            filtered: vec![0],
            entries: vec![PickerEntry {
                name: model_label,
                options: vec![PickerOption {
                    provider: self.provider.name().to_string(),
                    api_method: "current".to_string(),
                    available: true,
                    detail: "updating model list…".to_string(),
                    estimated_reference_cost_micros: None,
                }],
                action: PickerAction::Model,
                selected_option: 0,
                is_current: true,
                is_default: false,
                is_favorite: false,
                recommended: false,
                recommendation_rank: usize::MAX,
                usage_score: 0,
                old: false,
                created_date: None,
                effort: None,
            }],
            selected: 0,
            column: 0,
            filter: String::new(),
            preview: false,
        });
        self.set_status_notice("Updating model list…");
    }

    fn start_model_picker_route_load(
        &mut self,
        signature: ModelPickerCacheSignature,
        picker_started: std::time::Instant,
    ) {
        self.model_picker_load_request_id = self.model_picker_load_request_id.wrapping_add(1);
        let request_id = self.model_picker_load_request_id;
        let provider = self.provider.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let build = move || {
            let routes_started = std::time::Instant::now();
            let routes = provider.model_routes();
            let routes_ms = routes_started.elapsed().as_millis();
            let _ = tx.send(Ok(ModelPickerRoutesResult { routes, routes_ms }));
        };

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn_blocking(build);
        } else {
            std::thread::spawn(build);
        }

        self.pending_model_picker_load = Some(PendingModelPickerLoad {
            request_id,
            signature,
            picker_started,
            receiver: rx,
        });
    }

    pub(super) fn poll_model_picker_load(&mut self) -> bool {
        let Some(pending) = self.pending_model_picker_load.as_ref() else {
            return false;
        };

        let received = match pending.receiver.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.pending_model_picker_load = None;
                self.set_status_notice("Model list update failed");
                return true;
            }
        };

        let Some(pending) = self.pending_model_picker_load.take() else {
            return false;
        };
        if pending.request_id != self.model_picker_load_request_id {
            return false;
        }

        let current_model = if self.is_remote {
            self.remote_provider_model
                .clone()
                .unwrap_or_else(|| "unknown".to_string())
        } else {
            self.provider.model().to_string()
        };
        let config = crate::config::config();
        let config_default_model = config.provider.default_model.clone();
        let config_default_provider = config.provider.default_provider.clone();
        let current_effort = if self.is_remote {
            self.remote_reasoning_effort.clone()
        } else {
            self.provider.reasoning_effort()
        };
        let available_efforts = if self.is_remote {
            inferred_reasoning_efforts(
                self.remote_provider_name.as_deref(),
                self.remote_provider_model.as_deref(),
            )
        } else {
            self.provider.available_efforts()
        };
        let current_signature = self.model_picker_cache_signature(
            &current_model,
            config_default_model,
            config_default_provider,
            current_effort,
            &available_efforts,
        );
        if current_signature != pending.signature {
            return false;
        }

        match received {
            Ok(result) => {
                self.open_model_picker_with_routes(
                    pending.signature,
                    pending.picker_started,
                    result.routes,
                    result.routes_ms,
                    true,
                    true,
                );
                if self.inline_interactive_state.is_some() {
                    self.set_status_notice("Model list updated");
                }
                true
            }
            Err(error) => {
                self.set_status_notice(format!("Model list update failed: {}", error));
                true
            }
        }
    }

    fn open_model_picker_with_routes(
        &mut self,
        cache_signature: ModelPickerCacheSignature,
        picker_started: std::time::Instant,
        routes: Vec<crate::provider::ModelRoute>,
        routes_ms: u128,
        preserve_input: bool,
        cache_entries: bool,
    ) -> Vec<crate::provider::ModelRoute> {
        use std::collections::BTreeMap;

        let current_model = if self.is_remote {
            self.remote_provider_model
                .clone()
                .unwrap_or_else(|| "unknown".to_string())
        } else {
            self.provider.model().to_string()
        };
        let config = crate::config::config();
        let config_default_model = config.provider.default_model.clone();
        let config_default_provider = config.provider.default_provider.clone();
        let current_effort = if self.is_remote {
            self.remote_reasoning_effort.clone()
        } else {
            self.provider.reasoning_effort()
        };

        let is_config_default = |name: &str, route: &PickerOption| -> bool {
            model_picker_route_is_default(
                name,
                route,
                config_default_model.as_deref(),
                config_default_provider.as_deref(),
            )
        };

        let routes = if routes.is_empty() && self.is_remote && current_model != "unknown" {
            vec![crate::provider::ModelRoute {
                model: current_model.clone(),
                provider: self
                    .remote_provider_name
                    .clone()
                    .unwrap_or_else(|| "current".to_string()),
                api_method: "current".to_string(),
                available: true,
                detail: "catalog still loading".to_string(),
                cheapness: None,
            }]
        } else {
            routes
        };
        let routes = crate::provider::dedupe_model_routes(routes);
        let routes = filter_routes_by_provider_allowlist(
            routes,
            config.provider.model_picker_providers.as_deref(),
            &current_model,
        );

        if routes.is_empty() {
            self.inline_interactive_state = None;
            self.push_display_message(DisplayMessage::system(
                crate::tui::app::model_context::no_models_available_message(self.is_remote),
            ));
            self.set_status_notice("No models available");
            return routes;
        }

        let grouping_started = std::time::Instant::now();
        let mut model_order: Vec<String> = Vec::new();
        let mut model_options: BTreeMap<String, Vec<PickerOption>> = BTreeMap::new();
        for r in &routes {
            if !model_options.contains_key(&r.model) {
                model_order.push(r.model.clone());
            }
            model_options
                .entry(r.model.clone())
                .or_default()
                .push(PickerOption {
                    provider: r.provider.clone(),
                    api_method: r.api_method.clone(),
                    available: r.available,
                    detail: r.detail.clone(),
                    estimated_reference_cost_micros: r.estimated_reference_cost_micros(),
                });
        }
        let grouping_ms = grouping_started.elapsed().as_millis();

        fn route_sort_key(r: &PickerOption) -> (u8, u8, u64, String) {
            let avail = if r.available { 0 } else { 1 };
            let method = match crate::provider::ModelRouteApiMethod::parse(&r.api_method) {
                crate::provider::ModelRouteApiMethod::ClaudeOAuth
                | crate::provider::ModelRouteApiMethod::OpenAIOAuth
                | crate::provider::ModelRouteApiMethod::OpenAIApiKey => 0,
                crate::provider::ModelRouteApiMethod::AnthropicApiKey
                | crate::provider::ModelRouteApiMethod::OpenAiCompatible { .. } => 1,
                crate::provider::ModelRouteApiMethod::Cursor => 2,
                crate::provider::ModelRouteApiMethod::Copilot => 3,
                crate::provider::ModelRouteApiMethod::OpenRouter => 4,
                _ => 5,
            };
            let cheapness = r.estimated_reference_cost_micros.unwrap_or(u64::MAX);
            (avail, method, cheapness, r.provider.clone())
        }

        fn route_matches_recent_auth(route_provider: &str, login_provider: &str) -> bool {
            jcode_provider_core::model_route_provider_labels_related(route_provider, login_provider)
        }

        let timestamp_started = std::time::Instant::now();
        let openrouter_created_timestamps =
            crate::provider::openrouter::load_model_timestamp_index();
        let timestamp_ms = timestamp_started.elapsed().as_millis();
        let openrouter_created_timestamp = |model: &str| {
            crate::provider::openrouter::model_created_timestamp_from_index(
                model,
                &openrouter_created_timestamps,
            )
        };

        let latest_recommended_ts: Option<u64> = RECOMMENDED_MODELS
            .iter()
            .filter_map(|m| openrouter_created_timestamp(m))
            .max();
        let old_threshold_secs = latest_recommended_ts
            .map(|ts| ts.saturating_sub(30 * 86400))
            .unwrap_or(0);

        fn format_created(ts: u64) -> String {
            use chrono::{TimeZone, Utc};
            if let Some(dt) = Utc.timestamp_opt(ts as i64, 0).single() {
                dt.format("%b %Y").to_string()
            } else {
                String::new()
            }
        }

        let current_provider = if self.is_remote {
            self.remote_provider_name
                .clone()
                .unwrap_or_else(|| "remote".to_string())
        } else {
            self.provider.name().to_string()
        };
        let recent_auth_provider = self
            .recent_authenticated_provider
            .as_ref()
            .map(|(provider, _)| provider.as_str());

        let entries_started = std::time::Instant::now();
        let usage_store = load_model_picker_usage_store();
        let favorites_store = load_model_picker_favorites_store();
        let mut entries: Vec<PickerEntry> = Vec::new();
        for name in &model_order {
            let mut entry_routes = model_options.remove(name).unwrap_or_default();
            entry_routes.sort_by_key(route_sort_key);
            let recently_authenticated = recent_auth_provider
                .map(|provider| {
                    entry_routes
                        .iter()
                        .any(|route| route_matches_recent_auth(&route.provider, provider))
                })
                .unwrap_or(false);
            if recently_authenticated {
                for route in &mut entry_routes {
                    if recent_auth_provider
                        .map(|provider| route_matches_recent_auth(&route.provider, provider))
                        .unwrap_or(false)
                        && !route.detail.contains("recently added")
                    {
                        route.detail = if route.detail.trim().is_empty() {
                            "recently added".to_string()
                        } else {
                            format!("recently added · {}", route.detail)
                        };
                    }
                }
            }

            // Expand each route only across the effort ladder its runtime can
            // actually apply. The same model can be reachable through native
            // OpenAI (where `max` is real) and OpenRouter (where `max` aliases
            // `xhigh`), so model-id-only inference over-advertises values.
            let mut effort_routes = Vec::new();
            let mut plain_routes = Vec::new();
            let mut model_efforts = Vec::new();
            for route in entry_routes {
                let efforts = if route_supports_reasoning_effort(&route.api_method) {
                    inferred_reasoning_efforts(Some(&route.api_method), Some(name))
                } else {
                    Vec::new()
                };
                if efforts.is_empty() {
                    plain_routes.push(route);
                } else {
                    for effort in &efforts {
                        if !model_efforts.contains(effort) {
                            model_efforts.push(*effort);
                        }
                    }
                    effort_routes.push((route, efforts));
                }
            }

            if !effort_routes.is_empty() {
                for effort in &model_efforts {
                    // Swarm modes (swarm / swarm-deep) are orchestration rungs on
                    // the effort ladder, not per-model reasoning variants. They
                    // must not generate `model (swarm)` picker rows.
                    if crate::prompt::is_swarm_mode_effort(effort) {
                        continue;
                    }
                    let effort_label = match *effort {
                        "xhigh" => "xhigh",
                        "max" => "max",
                        "high" => "high",
                        "medium" => "med",
                        "low" => "low",
                        "none" => "none",
                        other => other,
                    };
                    let display_name = format!("{} ({})", name, effort_label);
                    let effort_matches_current =
                        *name == current_model && current_effort.as_deref() == Some(*effort);
                    let or_created = openrouter_created_timestamp(name);
                    for (route, route_efforts) in &effort_routes {
                        if !route_efforts.contains(effort) {
                            continue;
                        }
                        let is_this_current = effort_matches_current
                            && model_picker_route_is_current(
                                name,
                                route,
                                &current_model,
                                &current_provider,
                            );
                        entries.push(PickerEntry {
                            name: display_name.clone(),
                            options: vec![route.clone()],
                            action: PickerAction::Model,
                            selected_option: 0,
                            is_current: is_this_current,
                            recommended: *effort == "high"
                                && model_picker_route_is_recommended(name, route),
                            recommendation_rank: model_picker_recommendation_rank(name),
                            usage_score: model_picker_usage_score(
                                &usage_store,
                                name,
                                route,
                                Some(effort),
                            ),
                            old: old_threshold_secs > 0
                                && or_created.map(|t| t < old_threshold_secs).unwrap_or(false),
                            created_date: or_created.map(format_created),
                            effort: Some(effort.to_string()),
                            is_default: is_config_default(name, route),
                            is_favorite: model_picker_is_favorite(
                                &favorites_store,
                                name,
                                route,
                                Some(effort),
                            ),
                        });
                    }
                }
            }
            {
                let or_created = openrouter_created_timestamp(name);
                let is_old = old_threshold_secs > 0
                    && or_created.map(|t| t < old_threshold_secs).unwrap_or(false);
                for route in plain_routes {
                    let is_recommended = model_picker_route_is_recommended(name, &route);
                    let is_current = model_picker_route_is_current(
                        name,
                        &route,
                        &current_model,
                        &current_provider,
                    );
                    let is_default = is_config_default(name, &route);
                    entries.push(PickerEntry {
                        name: name.clone(),
                        options: vec![route.clone()],
                        action: PickerAction::Model,
                        selected_option: 0,
                        is_current,
                        recommended: is_recommended,
                        recommendation_rank: model_picker_recommendation_rank(name),
                        usage_score: model_picker_usage_score(&usage_store, name, &route, None),
                        old: is_old,
                        created_date: or_created.map(format_created),
                        effort: None,
                        is_default,
                        is_favorite: model_picker_is_favorite(&favorites_store, name, &route, None),
                    });
                }
            }
        }

        entries.sort_by(|a, b| {
            let a_current = if a.is_current { 0u8 } else { 1 };
            let b_current = if b.is_current { 0u8 } else { 1 };
            let a_recent = if a
                .options
                .iter()
                .any(|option| option.detail.contains("recently added"))
            {
                0u8
            } else {
                1
            };
            let b_recent = if b
                .options
                .iter()
                .any(|option| option.detail.contains("recently added"))
            {
                0u8
            } else {
                1
            };
            let a_rec = if a.recommended { 0u8 } else { 1 };
            let b_rec = if b.recommended { 0u8 } else { 1 };
            let a_favorite = if a.is_favorite { 0u8 } else { 1 };
            let b_favorite = if b.is_favorite { 0u8 } else { 1 };
            let a_usage = std::cmp::Reverse(a.usage_score);
            let b_usage = std::cmp::Reverse(b.usage_score);
            let a_rec_rank = if a.recommended {
                a.recommendation_rank
            } else {
                usize::MAX
            };
            let b_rec_rank = if b.recommended {
                b.recommendation_rank
            } else {
                usize::MAX
            };
            let a_avail = if a.options.first().map(|r| r.available).unwrap_or(false) {
                0u8
            } else {
                1
            };
            let b_avail = if b.options.first().map(|r| r.available).unwrap_or(false) {
                0u8
            } else {
                1
            };
            let a_old = if a.old { 1u8 } else { 0 };
            let b_old = if b.old { 1u8 } else { 0 };
            a_current
                .cmp(&b_current)
                .then(a_favorite.cmp(&b_favorite))
                .then(a_recent.cmp(&b_recent))
                .then(a_usage.cmp(&b_usage))
                .then(a_rec.cmp(&b_rec))
                .then(a_rec_rank.cmp(&b_rec_rank))
                .then(a_avail.cmp(&b_avail))
                .then(a_old.cmp(&b_old))
                .then(a.name.cmp(&b.name))
                .then_with(|| {
                    a.active_option()
                        .map(|route| route.provider.as_str())
                        .cmp(&b.active_option().map(|route| route.provider.as_str()))
                })
                .then_with(|| {
                    a.active_option()
                        .map(|route| route.api_method.as_str())
                        .cmp(&b.active_option().map(|route| route.api_method.as_str()))
                })
        });
        let entries_ms = entries_started.elapsed().as_millis();
        let total_ms = picker_started.elapsed().as_millis();

        if total_ms >= 250 || std::env::var("JCODE_LOG_MODEL_PICKER_TIMING").is_ok() {
            crate::logging::info(&format!(
                "[TIMING] model_picker_open: remote={}, simplified={}, routes={}, models={}, entries={}, routes={}ms, grouping={}ms, timestamps={}ms, entries_sort={}ms, total={}ms",
                self.is_remote,
                crate::perf::tui_policy().simplified_model_picker,
                routes.len(),
                model_order.len(),
                entries.len(),
                routes_ms,
                grouping_ms,
                timestamp_ms,
                entries_ms,
                total_ms,
            ));
        }

        // Always-on structured summary of what the picker actually presented.
        // Pairs with `model_routes_summary` (catalog side) so a shared log shows
        // both how many routes were built and how many survived into the picker
        // UI, plus the per-provider breakdown the user sees. This is the key
        // evidence for "configured provider missing from /model" reports.
        {
            use std::collections::BTreeMap;
            let mut by_provider: BTreeMap<String, usize> = BTreeMap::new();
            let mut available_entries = 0usize;
            for entry in &entries {
                if let Some(route) = entry.active_option() {
                    if route.available {
                        available_entries += 1;
                    }
                    let key = route.provider.trim().to_ascii_lowercase().replace(' ', "_");
                    let key = if key.is_empty() {
                        "unknown".to_string()
                    } else {
                        key
                    };
                    *by_provider.entry(key).or_insert(0) += 1;
                }
            }
            let per_provider = by_provider
                .into_iter()
                .map(|(provider, count)| format!("{provider}:{count}"))
                .collect::<Vec<_>>()
                .join(",");
            crate::logging::event_info(
                "model_picker_open",
                vec![
                    ("remote", self.is_remote.to_string()),
                    (
                        "simplified",
                        crate::perf::tui_policy()
                            .simplified_model_picker
                            .to_string(),
                    ),
                    ("routes_in", routes.len().to_string()),
                    ("models", model_order.len().to_string()),
                    ("entries", entries.len().to_string()),
                    ("entries_available", available_entries.to_string()),
                    ("current_model", current_model.clone()),
                    ("current_provider", current_provider.clone()),
                    (
                        "recent_auth_provider",
                        recent_auth_provider.unwrap_or("none").to_string(),
                    ),
                    ("by_provider", per_provider),
                    ("total_ms", total_ms.to_string()),
                ],
            );
        }

        let previous_picker = self.inline_interactive_state.as_ref().and_then(|picker| {
            if picker.kind == PickerKind::Model {
                Some((
                    picker.preview,
                    picker.filter.clone(),
                    picker.selected,
                    picker.column,
                ))
            } else {
                None
            }
        });
        let saved_input = if preserve_input {
            Some((self.input.clone(), self.cursor_pos))
        } else {
            None
        };

        self.inline_view_state = None;
        if cache_entries && Self::should_cache_model_picker_entries(model_order.len(), routes.len())
        {
            self.model_picker_cache = Some(ModelPickerCache {
                signature: cache_signature,
                entries: entries.clone(),
                route_count: routes.len(),
                model_count: model_order.len(),
            });
        } else {
            self.model_picker_cache = None;
        }
        self.inline_interactive_state = Some(InlineInteractiveState {
            kind: PickerKind::Model,
            filtered: (0..entries.len()).collect(),
            entries,
            selected: 0,
            column: 0,
            filter: String::new(),
            preview: false,
        });

        if let Some((preview, filter, selected, column)) = previous_picker
            && let Some(ref mut picker) = self.inline_interactive_state
        {
            picker.preview = preview;
            picker.filter = filter;
            picker.selected = selected.min(picker.filtered.len().saturating_sub(1));
            picker.column = column.min(picker.max_navigable_column());
            Self::apply_inline_interactive_filter(picker);
        }

        if let Some((input, cursor_pos)) = saved_input {
            self.input = input;
            self.cursor_pos = cursor_pos;
        } else {
            self.input.clear();
            self.cursor_pos = 0;
        }
        routes
    }

    pub(in crate::tui::app) fn debug_model_picker_live_json(
        &mut self,
        visible_limit: Option<usize>,
    ) -> String {
        let previous_inline_view = self.inline_view_state.clone();
        let previous_inline_interactive = self.inline_interactive_state.clone();
        let previous_model_picker_cache = self.model_picker_cache.clone();
        let previous_pending_model_picker_load = self.pending_model_picker_load.take();
        let previous_model_picker_load_request_id = self.model_picker_load_request_id;
        let previous_input = self.input.clone();
        let previous_cursor_pos = self.cursor_pos;
        let previous_status_notice = self.status_notice.clone();

        if self.is_remote && self.remote_model_options.is_empty() {
            self.hydrate_remote_model_catalog_cache();
        }

        let started = std::time::Instant::now();
        let current_model = if self.is_remote {
            self.remote_provider_model
                .clone()
                .unwrap_or_else(|| "unknown".to_string())
        } else {
            self.provider.model().to_string()
        };
        let config = crate::config::config();
        let config_default_model = config.provider.default_model.clone();
        let config_default_provider = config.provider.default_provider.clone();
        let current_effort = if self.is_remote {
            self.remote_reasoning_effort.clone()
        } else {
            self.provider.reasoning_effort()
        };
        let available_efforts = if self.is_remote {
            inferred_reasoning_efforts(
                self.remote_provider_name.as_deref(),
                self.remote_provider_model.as_deref(),
            )
        } else {
            self.provider.available_efforts()
        };
        let signature = self.model_picker_cache_signature(
            &current_model,
            config_default_model,
            config_default_provider,
            current_effort,
            &available_efforts,
        );

        let routes_started = std::time::Instant::now();
        let routes: Vec<crate::provider::ModelRoute> = if self.is_remote {
            if !self.remote_model_options.is_empty() {
                let mut routes = std::mem::take(&mut self.remote_model_options);
                self.extend_remote_routes_for_uncovered_models(&mut routes);
                routes
            } else {
                self.build_remote_model_routes_lightweight_fallback(&current_model)
            }
        } else if crate::perf::tui_policy().simplified_model_picker {
            self.simplified_model_routes_for_picker(&current_model)
        } else {
            self.provider.model_routes()
        };
        let routes_ms = routes_started.elapsed().as_millis();
        let raw_route_count = routes.len();
        let mut raw_static_fallback_by_provider =
            std::collections::BTreeMap::<String, usize>::new();
        for route in &routes {
            if route
                .detail
                .contains("fallback: static provider model list")
            {
                *raw_static_fallback_by_provider
                    .entry(route.provider.clone())
                    .or_default() += 1;
            }
        }
        let raw_static_fallback_count: usize = raw_static_fallback_by_provider.values().sum();
        let raw_routes = routes
            .iter()
            .take(visible_limit.unwrap_or(200))
            .map(|route| {
                serde_json::json!({
                    "model": route.model,
                    "provider": route.provider,
                    "api_method": route.api_method,
                    "available": route.available,
                    "detail": route.detail,
                    "estimated_reference_cost_micros": route.estimated_reference_cost_micros(),
                })
            })
            .collect::<Vec<_>>();

        let routes =
            self.open_model_picker_with_routes(signature, started, routes, routes_ms, false, false);
        let picker_json = self.debug_picker_state_json(visible_limit);
        let picker_value: serde_json::Value = serde_json::from_str(&picker_json)
            .unwrap_or_else(|_| serde_json::json!({ "error": "failed to serialize picker" }));
        let mut picker_static_fallback_by_provider =
            std::collections::BTreeMap::<String, usize>::new();
        if let Some(picker) = self.inline_interactive_state.as_ref() {
            for entry in &picker.entries {
                if let Some(route) = entry.active_option()
                    && route
                        .detail
                        .contains("fallback: static provider model list")
                {
                    *picker_static_fallback_by_provider
                        .entry(route.provider.clone())
                        .or_default() += 1;
                }
            }
        }
        let picker_static_fallback_count: usize = picker_static_fallback_by_provider.values().sum();

        self.inline_view_state = previous_inline_view;
        self.inline_interactive_state = previous_inline_interactive;
        self.model_picker_cache = previous_model_picker_cache;
        self.pending_model_picker_load = previous_pending_model_picker_load;
        self.model_picker_load_request_id = previous_model_picker_load_request_id;
        if self.is_remote && self.remote_model_options.is_empty() {
            self.remote_model_options = routes;
        }
        self.input = previous_input;
        self.cursor_pos = previous_cursor_pos;
        self.status_notice = previous_status_notice;

        serde_json::to_string_pretty(&serde_json::json!({
            "source_of_truth": "materialized_tui_model_picker",
            "remote": self.is_remote,
            "provider_name": if self.is_remote {
                self.remote_provider_name.clone().unwrap_or_else(|| "remote".to_string())
            } else {
                self.provider.name().to_string()
            },
            "current_model": current_model,
            "raw_route_count": raw_route_count,
            "raw_route_sample_count": raw_routes.len(),
            "raw_static_fallback_count": raw_static_fallback_count,
            "raw_static_fallback_by_provider": raw_static_fallback_by_provider,
            "picker_static_fallback_count": picker_static_fallback_count,
            "picker_static_fallback_by_provider": picker_static_fallback_by_provider,
            "routes_ms": routes_ms,
            "total_ms": started.elapsed().as_millis(),
            "raw_routes": raw_routes,
            "picker": picker_value,
        }))
        .unwrap_or_else(|_| "{}".to_string())
    }

    pub(super) fn build_remote_model_routes_fallback(&self) -> Vec<crate::provider::ModelRoute> {
        crate::provider::remote_model_routes_fallback(
            self.remote_provider_name.as_deref(),
            &self.remote_available_entries,
        )
    }

    fn build_remote_model_routes_lightweight_fallback(
        &self,
        current_model: &str,
    ) -> Vec<crate::provider::ModelRoute> {
        crate::provider::remote_model_routes_lightweight_fallback(
            self.remote_provider_name.as_deref(),
            &self.remote_available_entries,
            current_model,
        )
    }

    /// When a runtime model-picker preview is visible, route its favorite/default
    /// hotkeys to the focused picker handler. Returns true if the key was consumed.
    pub(super) fn model_picker_preview_hotkey(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> Result<bool> {
        let active = self
            .inline_interactive_state
            .as_ref()
            .map(|picker| picker.preview && picker_is_runtime_model_picker(picker))
            .unwrap_or(false);
        if !active {
            return Ok(false);
        }
        // Use Ctrl+O (set default) and Ctrl+N (toggle favorite) so the picker
        // preview no longer steals Ctrl+B / Ctrl+F / Alt+F, which are the tmux
        // prefix and readline word-navigation keys users rely on while editing
        // the `/model` command line. Cycling favorites stays on Shift+Tab.
        let is_default =
            modifiers.contains(KeyModifiers::CONTROL) && key_char_eq_ignore_ascii_case(code, 'o');
        let is_favorite =
            modifiers.contains(KeyModifiers::CONTROL) && key_char_eq_ignore_ascii_case(code, 'n');
        if is_default || is_favorite {
            self.handle_inline_interactive_key(code, modifiers)?;
            return Ok(true);
        }
        Ok(false)
    }

    pub(super) fn handle_inline_interactive_preview_key(
        &mut self,
        code: &KeyCode,
        modifiers: KeyModifiers,
    ) -> Result<bool> {
        let is_preview = self
            .inline_interactive_state
            .as_ref()
            .is_some_and(|p| p.preview);
        if !is_preview {
            return Ok(false);
        }
        match code {
            KeyCode::Down => {
                if let Some(picker) = self.inline_interactive_state.as_mut() {
                    let max = picker.filtered.len().saturating_sub(1);
                    picker.selected = (picker.selected + 1).min(max);
                }
                Ok(true)
            }
            KeyCode::Up => {
                if let Some(picker) = self.inline_interactive_state.as_mut() {
                    picker.selected = picker.selected.saturating_sub(1);
                }
                Ok(true)
            }
            KeyCode::Char('j') if modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(picker) = self.inline_interactive_state.as_mut() {
                    let max = picker.filtered.len().saturating_sub(1);
                    picker.selected = (picker.selected + 1).min(max);
                }
                Ok(true)
            }
            KeyCode::Char('k') if modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(picker) = self.inline_interactive_state.as_mut() {
                    picker.selected = picker.selected.saturating_sub(1);
                }
                Ok(true)
            }
            KeyCode::PageDown => {
                if let Some(picker) = self.inline_interactive_state.as_mut() {
                    let max = picker.filtered.len().saturating_sub(1);
                    picker.selected = (picker.selected + 5).min(max);
                }
                Ok(true)
            }
            KeyCode::PageUp => {
                if let Some(picker) = self.inline_interactive_state.as_mut() {
                    picker.selected = picker.selected.saturating_sub(5);
                }
                Ok(true)
            }
            KeyCode::Enter => {
                if let Some(ref mut picker) = self.inline_interactive_state {
                    if picker.filtered.is_empty() {
                        self.inline_interactive_state = None;
                        self.input.clear();
                        self.cursor_pos = 0;
                        return Ok(true);
                    }
                    // `/login` + immediate Enter should not silently launch the
                    // first provider's login flow. Without a filter or an
                    // explicit selection there is no clear user choice yet, so
                    // activate the picker and let them pick deliberately.
                    if picker.kind == PickerKind::Login
                        && picker.filter.is_empty()
                        && picker.selected == 0
                    {
                        picker.preview = false;
                        picker.column = 0;
                        self.input.clear();
                        self.cursor_pos = 0;
                        return Ok(true);
                    }
                    picker.preview = false;
                    if picker.kind == PickerKind::Usage {
                        picker.column = 0;
                        self.input.clear();
                        self.cursor_pos = 0;
                        self.request_usage_report();
                        return Ok(true);
                    }
                    picker.column = picker.preview_activation_column();
                }
                self.input.clear();
                self.cursor_pos = 0;
                self.handle_inline_interactive_key(KeyCode::Enter, modifiers)?;
                Ok(true)
            }
            KeyCode::Esc => {
                self.inline_interactive_state = None;
                self.input.clear();
                self.cursor_pos = 0;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn handle_account_picker_selection(&mut self, action: AccountPickerAction) {
        match action {
            AccountPickerAction::Switch { provider_id, label } => {
                if self.is_remote {
                    self.pending_account_picker_action = Some(AccountPickerAction::Switch {
                        provider_id: provider_id.clone(),
                        label: label.clone(),
                    });
                    self.set_status_notice(format!("Account → {} ({})", label, provider_id));
                    return;
                }

                match provider_id.as_str() {
                    "claude" => self.switch_account(&label),
                    "openai" => self.switch_openai_account(&label),
                    _ => self.push_display_message(DisplayMessage::error(format!(
                        "Provider `{}` does not support account switching.",
                        provider_id
                    ))),
                }
            }
            AccountPickerAction::Add { provider_id } => match provider_id.as_str() {
                "claude" => match crate::auth::claude::next_account_label() {
                    Ok(label) => self.start_claude_login_for_account(&label),
                    Err(e) => self.push_display_message(DisplayMessage::error(format!(
                        "Failed to prepare Claude account: {}",
                        e
                    ))),
                },
                "openai" => match crate::auth::codex::next_account_label() {
                    Ok(label) => self.start_openai_login_for_account(&label),
                    Err(e) => self.push_display_message(DisplayMessage::error(format!(
                        "Failed to prepare OpenAI account: {}",
                        e
                    ))),
                },
                _ => self.push_display_message(DisplayMessage::error(format!(
                    "Provider `{}` does not support multiple accounts.",
                    provider_id
                ))),
            },
            AccountPickerAction::Replace { provider_id, label } => match provider_id.as_str() {
                "claude" => self.start_claude_login_for_account(&label),
                "openai" => self.start_openai_login_for_account(&label),
                _ => self.push_display_message(DisplayMessage::error(format!(
                    "Provider `{}` does not support account replacement.",
                    provider_id
                ))),
            },
            AccountPickerAction::OpenCenter { provider_filter } => {
                self.open_account_center(provider_filter.as_deref())
            }
        }
    }

    pub(super) fn open_session_picker(&mut self) {
        let current_dir = self.session.working_dir.clone();
        let (mut picker, status) = if let Some((server_groups, orphan_sessions)) =
            session_picker::load_cached_sessions_grouped()
        {
            (
                SessionPicker::new_grouped(server_groups, orphan_sessions),
                "Refreshing sessions...",
            )
        } else {
            (SessionPicker::loading(), "Loading sessions...")
        };
        picker.set_current_dir(current_dir);
        picker.set_current_session_id(Some(super::commands::active_session_id(self)));
        self.session_picker_overlay = Some(RefCell::new(picker));
        self.session_picker_mode = SessionPickerMode::Resume;
        self.set_status_notice(status);
        self.start_session_picker_load();
    }

    /// Open the active sessions manager: the session picker scoped to live
    /// (open) sessions, showing which are still working on a response and
    /// which are ready for input. Reached via Left arrow on an empty input
    /// (when `display.active_sessions_manager` is enabled) or `/active`.
    pub(super) fn open_active_sessions_picker(&mut self) {
        let current_dir = self.session.working_dir.clone();
        let (mut picker, status) = if let Some((server_groups, orphan_sessions)) =
            session_picker::load_cached_sessions_grouped()
        {
            (
                SessionPicker::new_grouped(server_groups, orphan_sessions),
                "Refreshing active sessions...",
            )
        } else {
            (SessionPicker::loading(), "Loading active sessions...")
        };
        picker.set_current_dir(current_dir);
        picker.set_current_session_id(Some(super::commands::active_session_id(self)));
        picker.activate_active_filter();
        self.session_picker_overlay = Some(RefCell::new(picker));
        self.session_picker_mode = SessionPickerMode::ActiveSessions;
        self.set_status_notice(status);
        self.start_session_picker_load();
    }

    /// Opt-in Left-arrow gesture: pressing Left on an empty input opens the
    /// active sessions manager. Gated behind `display.active_sessions_manager`
    /// so the default input behavior is unchanged. Returns true when the
    /// gesture fired.
    pub(super) fn maybe_open_active_sessions_on_left(&mut self) -> bool {
        if !self.input.is_empty() || self.cursor_pos != 0 {
            return false;
        }
        if !crate::config::config().display.active_sessions_manager {
            return false;
        }
        self.open_active_sessions_picker();
        true
    }

    /// Tick hook: while the session picker overlay is up, periodically refresh
    /// the live presence snapshot so working/ready badges (and the Active view
    /// membership) track reality. Returns true when a redraw is needed.
    pub(super) fn poll_session_picker_presence(&mut self) -> bool {
        let Some(picker_cell) = self.session_picker_overlay.as_ref() else {
            return false;
        };
        picker_cell.borrow_mut().maybe_refresh_live_presence()
    }

    fn start_session_picker_load(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.pending_session_picker_load = Some(super::PendingSessionPickerLoad { receiver: rx });

        tokio::task::spawn_blocking(move || {
            let result = session_picker::load_sessions_grouped();
            let _ = tx.send(result);
        });
    }

    /// Rebuild the picker overlay from a freshly loaded session list, applying
    /// the filter for the active picker mode. Returns true when the overlay was
    /// (re)built so the caller can request a redraw.
    fn apply_loaded_session_picker(
        &mut self,
        server_groups: Vec<session_picker::ServerGroup>,
        orphan_sessions: Vec<session_picker::SessionInfo>,
    ) -> bool {
        // When a picker overlay is already on screen (the common case: the cached
        // list rendered instantly and this is the async full-refresh landing),
        // reseed it in place so the user's selection, scroll, search, focus, and
        // multi-select survive the swap. Rebuilding a fresh picker here used to
        // yank the view out from under the user a second or two after they opened
        // `/resume`, which felt like a lag/jump.
        let has_overlay = self.session_picker_overlay.is_some();
        if has_overlay {
            let notice = match self.session_picker_mode {
                SessionPickerMode::Resume => {
                    if let Some(existing) = self.session_picker_overlay.as_ref() {
                        existing
                            .borrow_mut()
                            .reseed_grouped(server_groups, orphan_sessions);
                    }
                    "Sessions loaded"
                }
                SessionPickerMode::CatchUp => {
                    if let Some(existing) = self.session_picker_overlay.as_ref() {
                        let mut picker = existing.borrow_mut();
                        // Keep the catch-up filter active; reseed preserves it.
                        picker.activate_catchup_filter();
                        picker.reseed_grouped(server_groups, orphan_sessions);
                    }
                    "Catch Up sessions loaded"
                }
                SessionPickerMode::ActiveSessions => {
                    if let Some(existing) = self.session_picker_overlay.as_ref() {
                        let mut picker = existing.borrow_mut();
                        // Keep the active filter; reseed preserves it and
                        // refreshes the live presence snapshot.
                        picker.activate_active_filter();
                        picker.reseed_grouped(server_groups, orphan_sessions);
                    }
                    "Active sessions loaded"
                }
                SessionPickerMode::Onboarding => return false,
            };
            self.set_status_notice(notice);
            return true;
        }

        match self.session_picker_mode {
            SessionPickerMode::Resume => {
                let mut picker = SessionPicker::new_grouped(server_groups, orphan_sessions);
                picker.set_current_dir(self.session.working_dir.clone());
                picker.set_current_session_id(Some(super::commands::active_session_id(self)));
                self.session_picker_overlay = Some(RefCell::new(picker));
                self.set_status_notice("Sessions loaded");
                true
            }
            SessionPickerMode::CatchUp => {
                let mut picker = SessionPicker::new_grouped(server_groups, orphan_sessions);
                picker.activate_catchup_filter();
                picker.set_current_dir(self.session.working_dir.clone());
                self.session_picker_overlay = Some(RefCell::new(picker));
                self.set_status_notice("Catch Up sessions loaded");
                true
            }
            SessionPickerMode::ActiveSessions => {
                let mut picker = SessionPicker::new_grouped(server_groups, orphan_sessions);
                picker.set_current_dir(self.session.working_dir.clone());
                picker.set_current_session_id(Some(super::commands::active_session_id(self)));
                picker.activate_active_filter();
                self.session_picker_overlay = Some(RefCell::new(picker));
                self.set_status_notice("Active sessions loaded");
                true
            }
            // Onboarding constructs its action-only picker synchronously, so it
            // never flows through this async path.
            SessionPickerMode::Onboarding => false,
        }
    }

    pub(super) fn poll_session_picker_load(&mut self) -> bool {
        let recv_result = {
            let Some(pending) = self.pending_session_picker_load.as_ref() else {
                return false;
            };
            pending.receiver.try_recv()
        };

        let picker_active = self.session_picker_overlay.is_some()
            && matches!(
                self.session_picker_mode,
                SessionPickerMode::Resume
                    | SessionPickerMode::CatchUp
                    | SessionPickerMode::ActiveSessions
            );

        match recv_result {
            Ok(Ok((server_groups, orphan_sessions))) => {
                self.pending_session_picker_load = None;
                if picker_active {
                    return self.apply_loaded_session_picker(server_groups, orphan_sessions);
                }
                false
            }
            Ok(Err(e)) => {
                self.pending_session_picker_load = None;
                if picker_active {
                    self.session_picker_overlay = None;
                    self.push_display_message(DisplayMessage::error(format!(
                        "Failed to load sessions: {}",
                        e
                    )));
                    self.set_status_notice("Session load failed");
                    return true;
                }
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.pending_session_picker_load = None;
                if picker_active {
                    self.session_picker_overlay = None;
                    self.push_display_message(DisplayMessage::error(
                        "Session loading stopped before returning a result.".to_string(),
                    ));
                    self.set_status_notice("Session load failed");
                    return true;
                }
                false
            }
        }
    }

    pub(super) fn open_catchup_picker(&mut self) {
        let current_session_id = super::commands::active_session_id(self);
        if catchup_candidates(&current_session_id).is_empty() {
            self.push_display_message(DisplayMessage::system(
                "No sessions currently need catch up.".to_string(),
            ));
            self.set_status_notice("Catch Up: none waiting");
            return;
        }

        // Show the picker overlay immediately (using the cached list when
        // available) and load the full session list off-thread. This keeps the
        // live TUI responsive instead of blocking on a multi-hundred-ms scan of
        // every historical session.
        let mut picker = if let Some((server_groups, orphan_sessions)) =
            session_picker::load_cached_sessions_grouped()
        {
            let mut picker = SessionPicker::new_grouped(server_groups, orphan_sessions);
            picker.activate_catchup_filter();
            picker
        } else {
            SessionPicker::loading()
        };
        // Ensure the filter is applied even on the loading placeholder so the
        // refreshed list lands in the catch-up view.
        picker.activate_catchup_filter();
        self.session_picker_overlay = Some(RefCell::new(picker));
        self.session_picker_mode = SessionPickerMode::CatchUp;
        self.set_status_notice("Loading Catch Up sessions...");
        self.start_session_picker_load();
    }

    pub(super) fn handle_session_picker_selection(&mut self, targets: &[ResumeTarget]) {
        if targets.is_empty() {
            return;
        }

        if self.session_picker_mode == SessionPickerMode::CatchUp {
            let current_session_id = super::commands::active_session_id(self);
            let mut names = Vec::with_capacity(targets.len());
            for target in targets {
                let ResumeTarget::JcodeSession { session_id } = target else {
                    continue;
                };
                let queue_position = catchup_queue_position(&current_session_id, session_id);
                self.queue_catchup_resume(
                    session_id.to_string(),
                    Some(current_session_id.clone()),
                    queue_position,
                    true,
                );
                names.push(
                    crate::id::extract_session_name(session_id)
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| session_id.to_string()),
                );
            }

            if names.len() == 1 {
                self.push_display_message(DisplayMessage::system(format!(
                    "Queued Catch Up for {}.",
                    names[0],
                )));
                self.set_status_notice(format!("Catch Up → {}", names[0]));
            } else {
                self.push_display_message(DisplayMessage::system(format!(
                    "Queued Catch Up for {} sessions: {}.",
                    names.len(),
                    names.join(", "),
                )));
                self.set_status_notice(format!("Catch Up → {} sessions", names.len()));
            }
            return;
        }

        let default_cwd = std::env::current_dir().unwrap_or_default();
        let socket = std::env::var("JCODE_SOCKET").ok();
        let mut spawned = 0usize;
        let mut failed = Vec::new();
        let mut names = Vec::with_capacity(targets.len());

        for target in targets {
            let mut cwd = default_cwd.clone();
            if let Some(picker_cell) = self.session_picker_overlay.as_ref() {
                let picker = picker_cell.borrow();
                if let Some(session) = picker.session_for_target(target)
                    && let Some(dir) = session.working_dir.as_deref()
                    && std::path::Path::new(dir).is_dir()
                {
                    cwd = std::path::PathBuf::from(dir);
                }
            }

            let name = match target {
                ResumeTarget::JcodeSession { session_id } => {
                    crate::id::extract_session_name(session_id)
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| session_id.to_string())
                }
                ResumeTarget::ClaudeCodeSession { session_id, .. } => {
                    format!(
                        "Claude Code {}",
                        jcode_core::util::truncate_str(session_id, 8)
                    )
                }
                ResumeTarget::CodexSession { session_id, .. } => {
                    format!("Codex {}", jcode_core::util::truncate_str(session_id, 8))
                }
                ResumeTarget::PiSession { session_path } => std::path::Path::new(session_path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("Pi session")
                    .to_string(),
                ResumeTarget::OpenCodeSession { session_id, .. } => {
                    format!("OpenCode {}", jcode_core::util::truncate_str(session_id, 8))
                }
                ResumeTarget::CursorSession { session_id, .. } => {
                    format!("Cursor {}", jcode_core::util::truncate_str(session_id, 8))
                }
            };
            let resolved_target = match crate::import::resolve_resume_target_to_jcode(target) {
                Ok(target) => target,
                Err(err) => {
                    failed.push(format!("failed to import {}: {}", name, err));
                    continue;
                }
            };

            match spawn_resume_target_in_new_terminal(&resolved_target, &cwd, socket.as_deref()) {
                Ok(true) => {
                    spawned += 1;
                    names.push(name);
                }
                Ok(false) | Err(_) => {
                    // No terminal emulator could be spawned. For a single jcode
                    // session, fall back to resuming in the current terminal
                    // instead of dead-ending with a manual command (issue #203).
                    if targets.len() == 1
                        && spawned == 0
                        && matches!(resolved_target, ResumeTarget::JcodeSession { .. })
                    {
                        self.handle_session_picker_current_terminal_selection(
                            std::slice::from_ref(target),
                        );
                        return;
                    }
                    failed.push(resume_target_manual_command(
                        &resolved_target,
                        socket.as_deref(),
                    ));
                }
            }
        }

        if spawned > 0 && failed.is_empty() {
            if names.len() == 1 {
                self.push_display_message(DisplayMessage::system(format!(
                    "Resumed {} in new window.",
                    names[0],
                )));
                self.set_status_notice(format!("Resumed {}", names[0]));
            } else {
                self.push_display_message(DisplayMessage::system(format!(
                    "Resumed {} sessions in new windows: {}.",
                    names.len(),
                    names.join(", "),
                )));
                self.set_status_notice(format!("Resumed {} sessions", names.len()));
            }
            return;
        }

        let manual: Vec<String> = failed.iter().map(|cmd| format!("  {}", cmd)).collect();

        if spawned > 0 {
            self.push_display_message(DisplayMessage::system(format!(
                "Resumed {} session(s) in new windows. {} failed:\n{}",
                spawned,
                failed.len(),
                manual.join("\n")
            )));
            self.set_status_notice(format!("Resumed {} session(s)", spawned));
        } else {
            self.push_display_message(DisplayMessage::system(format!(
                "No terminal found. Resume manually:\n{}",
                manual.join("\n")
            )));
        }
    }

    pub(super) fn handle_session_picker_current_terminal_selection(
        &mut self,
        targets: &[ResumeTarget],
    ) {
        let Some(target) = targets.first() else {
            return;
        };

        let name = match target {
            ResumeTarget::JcodeSession { session_id } => {
                crate::id::extract_session_name(session_id)
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| session_id.to_string())
            }
            ResumeTarget::ClaudeCodeSession { session_id, .. } => {
                format!(
                    "Claude Code {}",
                    jcode_core::util::truncate_str(session_id, 8)
                )
            }
            ResumeTarget::CodexSession { session_id, .. } => {
                format!("Codex {}", jcode_core::util::truncate_str(session_id, 8))
            }
            ResumeTarget::PiSession { session_path } => std::path::Path::new(session_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Pi session")
                .to_string(),
            ResumeTarget::OpenCodeSession { session_id, .. } => {
                format!("OpenCode {}", jcode_core::util::truncate_str(session_id, 8))
            }
            ResumeTarget::CursorSession { session_id, .. } => {
                format!("Cursor {}", jcode_core::util::truncate_str(session_id, 8))
            }
        };

        let resolved_target = match crate::import::resolve_resume_target_to_jcode(target) {
            Ok(target) => target,
            Err(err) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to import {}: {}",
                    name, err
                )));
                return;
            }
        };

        let ResumeTarget::JcodeSession { session_id } = resolved_target else {
            self.push_display_message(DisplayMessage::error(format!(
                "Cannot resume {} in the current terminal.",
                name
            )));
            return;
        };

        // Selecting the session we are already in (visible in the Active view,
        // labeled "current") should simply close the picker, not re-resume.
        if session_id == super::commands::active_session_id(self) {
            self.session_picker_overlay = None;
            self.session_picker_mode = SessionPickerMode::Resume;
            self.set_status_notice("Already in this session");
            return;
        }

        if targets.len() > 1 {
            self.push_display_message(DisplayMessage::system(format!(
                "Selected {} sessions; resuming {} in this terminal.",
                targets.len(),
                name
            )));
        }
        self.workspace_client.queue_resume_session(session_id);
        self.session_picker_overlay = None;
        self.session_picker_mode = SessionPickerMode::Resume;
        self.set_status_notice(format!("Switching → {}", name));
    }

    fn handle_live_claude_takeover(&mut self, target: &ResumeTarget) -> bool {
        let ResumeTarget::ClaudeCodeSession { session_id, .. } = target else {
            self.push_display_message(DisplayMessage::error(
                "Live takeover is only available for Claude Code sessions.",
            ));
            return false;
        };
        let display_id = jcode_core::util::truncate_str(session_id, 8);
        self.set_status_notice(format!("Preparing Claude takeover → {display_id}"));

        let resolved = match crate::import::take_over_live_claude_session(target) {
            Ok(target) => target,
            Err(err) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Claude takeover failed: {err}"
                )));
                self.set_status_notice("Claude takeover did not complete");
                return false;
            }
        };
        let ResumeTarget::JcodeSession { session_id } = resolved else {
            self.push_display_message(DisplayMessage::error(
                "Claude takeover did not produce a Jcode session.",
            ));
            return false;
        };

        self.push_display_message(DisplayMessage::system(format!(
            "Claude Code exited and its transcript was prepared as {session_id}."
        )));
        self.workspace_client.queue_resume_session(session_id);
        self.session_picker_overlay = None;
        self.session_picker_mode = SessionPickerMode::Resume;
        self.set_status_notice(format!("Taking over Claude → {display_id}"));
        true
    }

    pub(super) fn handle_batch_crash_restore(&mut self, session_ids: &[String]) {
        let recovered = match crate::session::recover_crashed_sessions_by_ids(session_ids) {
            Ok(ids) => ids,
            Err(e) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "Failed to recover crashed sessions: {}",
                    e
                )));
                return;
            }
        };

        if recovered.is_empty() {
            self.push_display_message(DisplayMessage::system(
                "No crashed sessions found in the selected restore group.".to_string(),
            ));
            return;
        }

        let exe = launch_client_executable();
        let cwd = std::env::current_dir().unwrap_or_default();
        let socket = std::env::var("JCODE_SOCKET").ok();
        let mut spawned = 0usize;
        let mut failed = Vec::new();

        for session_id in &recovered {
            let mut session_cwd = cwd.clone();
            if let Ok(session) = crate::session::Session::load_startup_stub(session_id)
                && let Some(dir) = session.working_dir.as_deref()
                && std::path::Path::new(dir).is_dir()
            {
                session_cwd = std::path::PathBuf::from(dir);
            }

            match spawn_in_new_terminal(&exe, session_id, &session_cwd, socket.as_deref()) {
                Ok(true) => spawned += 1,
                Ok(false) => failed.push(session_id.clone()),
                Err(e) => {
                    crate::logging::error(&format!(
                        "Failed to spawn session {}: {}",
                        session_id, e
                    ));
                    failed.push(session_id.clone());
                }
            }
        }

        // Single recovered session that could not get a new terminal: resume it
        // in the current terminal instead of forcing a manual command (#203).
        if spawned == 0 && recovered.len() == 1 && failed.len() == 1 {
            self.handle_session_picker_current_terminal_selection(&[ResumeTarget::JcodeSession {
                session_id: recovered[0].clone(),
            }]);
            return;
        }
        if spawned > 0 && failed.is_empty() {
            self.push_display_message(DisplayMessage::system(format!(
                "Restored {} crashed session(s) in new windows.",
                spawned
            )));
            self.set_status_notice(format!("Restored {} session(s)", spawned));
        } else if spawned > 0 {
            let manual: Vec<String> = failed
                .iter()
                .map(|id| format!("  jcode --resume {}", id))
                .collect();
            self.push_display_message(DisplayMessage::system(format!(
                "Restored {} session(s) in new windows. {} failed:\n{}",
                spawned,
                failed.len(),
                manual.join("\n")
            )));
        } else {
            let manual: Vec<String> = recovered
                .iter()
                .map(|id| format!("  jcode --resume {}", id))
                .collect();
            self.push_display_message(DisplayMessage::system(format!(
                "No terminal found. Resume manually:\n{}",
                manual.join("\n")
            )));
        }
    }

    pub(super) fn handle_session_picker_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> Result<()> {
        let action = {
            let Some(picker_cell) = self.session_picker_overlay.as_ref() else {
                return Ok(());
            };
            let mut picker = picker_cell.borrow_mut();
            picker.handle_overlay_key(code, modifiers)?
        };
        match action {
            OverlayAction::Continue => {}
            OverlayAction::Close => {
                self.session_picker_overlay = None;
                if self.session_picker_mode == SessionPickerMode::Onboarding {
                    // Escaping the onboarding choice starts a clean new session.
                    self.session_picker_mode = SessionPickerMode::Resume;
                    self.onboarding_show_suggestions();
                } else {
                    self.session_picker_mode = SessionPickerMode::Resume;
                }
            }
            OverlayAction::Selected(result)
                if matches!(self.session_picker_mode, SessionPickerMode::Onboarding) =>
            {
                let ids = match result {
                    PickerResult::Selected(ids)
                    | PickerResult::SelectedInNewTerminal(ids)
                    | PickerResult::SelectedInCurrentTerminal(ids) => ids,
                    PickerResult::TakeOverClaude(target) => {
                        if self.handle_live_claude_takeover(&target) {
                            self.onboarding_finish();
                        }
                        return Ok(());
                    }
                    PickerResult::RestoreCrashedGroup(_) => Vec::new(),
                    PickerResult::StartNewSession => {
                        // User explicitly chose to start fresh; close the picker
                        // and show the onboarding suggestion cards.
                        self.session_picker_overlay = None;
                        self.session_picker_mode = SessionPickerMode::Resume;
                        self.onboarding_show_suggestions();
                        return Ok(());
                    }
                    PickerResult::ReviewRecentProject => {
                        self.session_picker_overlay = None;
                        self.session_picker_mode = SessionPickerMode::Resume;
                        self.onboarding_start_recent_project_review();
                        return Ok(());
                    }
                };
                self.session_picker_overlay = None;
                self.session_picker_mode = SessionPickerMode::Resume;
                if ids.is_empty() {
                    self.onboarding_show_suggestions();
                } else {
                    // Single-select: resume only the first chosen transcript.
                    self.handle_session_picker_current_terminal_selection(&ids[..1]);
                    self.onboarding_finish();
                }
            }
            OverlayAction::Selected(PickerResult::Selected(ids))
            | OverlayAction::Selected(PickerResult::SelectedInNewTerminal(ids)) => {
                self.handle_session_picker_selection(&ids);
                if let Some(picker_cell) = self.session_picker_overlay.as_ref() {
                    picker_cell.borrow_mut().clear_selected_sessions();
                }
            }
            OverlayAction::Selected(PickerResult::SelectedInCurrentTerminal(ids)) => {
                if self.session_picker_mode == SessionPickerMode::CatchUp {
                    self.handle_session_picker_selection(&ids);
                    if let Some(picker_cell) = self.session_picker_overlay.as_ref() {
                        picker_cell.borrow_mut().clear_selected_sessions();
                    }
                } else {
                    self.handle_session_picker_current_terminal_selection(&ids);
                }
            }
            OverlayAction::Selected(PickerResult::TakeOverClaude(target)) => {
                let _ = self.handle_live_claude_takeover(&target);
            }
            OverlayAction::Selected(PickerResult::RestoreCrashedGroup(session_ids)) => {
                self.handle_batch_crash_restore(&session_ids);
            }
            OverlayAction::Selected(PickerResult::StartNewSession) => {
                // Only the onboarding picker emits this, and that case is
                // handled by the onboarding arm above. Outside onboarding,
                // treat it as a no-op close.
                self.session_picker_overlay = None;
                self.session_picker_mode = SessionPickerMode::Resume;
            }
            OverlayAction::Selected(PickerResult::ReviewRecentProject) => {
                // Only the onboarding picker emits this. Outside onboarding,
                // close defensively without launching a proactive turn.
                self.session_picker_overlay = None;
                self.session_picker_mode = SessionPickerMode::Resume;
            }
        }
        Ok(())
    }

    fn toggle_selected_model_favorite(&mut self) {
        let Some((entry_name, is_favorite, store)) = (|| {
            let picker = self.inline_interactive_state.as_mut()?;
            if !picker_is_runtime_model_picker(picker) || picker.filtered.is_empty() {
                return None;
            }
            let idx = picker.filtered[picker.selected];
            let entry = picker.entries.get_mut(idx)?;
            if !matches!(entry.action, PickerAction::Model) {
                return None;
            }
            let base_name = model_entry_base_name(entry);
            let effort = entry.effort.clone();
            let route = entry.options.get(entry.selected_option).cloned()?;
            let key = model_picker_usage_key(&base_name, &route, effort.as_deref());
            let mut store = load_model_picker_favorites_store();
            store.version = MODEL_PICKER_FAVORITES_VERSION;
            let is_favorite = if store.favorites.remove(&key) {
                false
            } else {
                store.favorites.insert(key);
                true
            };
            entry.is_favorite = is_favorite;
            Some((entry.name.clone(), is_favorite, store))
        })() else {
            return;
        };
        save_model_picker_favorites_store(&store);
        self.invalidate_model_picker_cache();
        let action = if is_favorite {
            "Favorited"
        } else {
            "Unfavorited"
        };
        self.set_status_notice(format!("{} {}", action, entry_name));
    }

    fn cycle_selected_model_favorite(&mut self) {
        let selected_name = (|| {
            let picker = self.inline_interactive_state.as_mut()?;
            if !picker_is_runtime_model_picker(picker) || picker.filtered.is_empty() {
                return None;
            }
            let total = picker.filtered.len();
            for offset in 1..=total {
                let next = (picker.selected + offset) % total;
                let entry_idx = picker.filtered[next];
                if picker
                    .entries
                    .get(entry_idx)
                    .map(|entry| entry.is_favorite)
                    .unwrap_or(false)
                {
                    picker.selected = next;
                    picker.column = 0;
                    return picker
                        .entries
                        .get(entry_idx)
                        .map(|entry| entry.name.clone());
                }
            }
            None
        })();
        if let Some(entry_name) = selected_name {
            self.set_status_notice(format!("Favorite → {}", entry_name));
        } else {
            self.set_status_notice("No favorited models yet. Use Ctrl+N to favorite one.");
        }
    }

    pub(super) fn cycle_model_favorite_hotkey(&mut self) {
        if self
            .inline_interactive_state
            .as_ref()
            .map(picker_is_runtime_model_picker)
            .unwrap_or(false)
        {
            self.cycle_selected_model_favorite();
            return;
        }

        self.open_model_picker_preserving_input();
        if !self
            .inline_interactive_state
            .as_ref()
            .map(picker_is_runtime_model_picker)
            .unwrap_or(false)
        {
            self.set_status_notice("Model favorites unavailable until model routes finish loading");
            return;
        }
        self.cycle_selected_model_favorite();
        let _ = self.handle_inline_interactive_key(KeyCode::Enter, KeyModifiers::NONE);
    }

    pub(super) fn handle_inline_interactive_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> Result<()> {
        match code {
            KeyCode::Esc => {
                if let Some(ref mut picker) = self.inline_interactive_state
                    && !picker.filter.is_empty()
                {
                    picker.filter.clear();
                    Self::apply_inline_interactive_filter(picker);
                    return Ok(());
                }
                self.inline_interactive_state = None;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let vim_nav = self
                    .inline_interactive_state
                    .as_ref()
                    .map(|picker| picker.uses_compact_navigation())
                    .unwrap_or(false);
                if matches!(code, KeyCode::Char('k'))
                    && !modifiers.contains(KeyModifiers::CONTROL)
                    && !vim_nav
                {
                    if let Some(ref mut picker) = self.inline_interactive_state {
                        picker.filter.push('k');
                        Self::apply_inline_interactive_filter(picker);
                    }
                    return Ok(());
                }
                if let Some(ref mut picker) = self.inline_interactive_state {
                    if picker.column == 0 {
                        picker.selected = picker.selected.saturating_sub(1);
                    } else if let Some(&idx) = picker.filtered.get(picker.selected) {
                        let entry = &mut picker.entries[idx];
                        entry.selected_option = entry.selected_option.saturating_sub(1);
                    }
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let vim_nav = self
                    .inline_interactive_state
                    .as_ref()
                    .map(|picker| picker.uses_compact_navigation())
                    .unwrap_or(false);
                if matches!(code, KeyCode::Char('j'))
                    && !modifiers.contains(KeyModifiers::CONTROL)
                    && !vim_nav
                {
                    if let Some(ref mut picker) = self.inline_interactive_state {
                        picker.filter.push('j');
                        Self::apply_inline_interactive_filter(picker);
                    }
                    return Ok(());
                }
                if let Some(ref mut picker) = self.inline_interactive_state {
                    if picker.column == 0 {
                        let max = picker.filtered.len().saturating_sub(1);
                        picker.selected = (picker.selected + 1).min(max);
                    } else if let Some(&idx) = picker.filtered.get(picker.selected) {
                        let entry = &mut picker.entries[idx];
                        let max = entry.options.len().saturating_sub(1);
                        entry.selected_option = (entry.selected_option + 1).min(max);
                    }
                }
            }
            KeyCode::Right => {
                if let Some(ref mut picker) = self.inline_interactive_state {
                    if picker.uses_compact_navigation() {
                        return Ok(());
                    }
                    if picker.column < picker.max_navigable_column()
                        && let Some(&idx) = picker.filtered.get(picker.selected)
                        && (picker.entries[idx].options.len() > 1 || picker.column > 0)
                    {
                        picker.column += 1;
                    }
                }
            }
            KeyCode::BackTab => {
                if self
                    .inline_interactive_state
                    .as_ref()
                    .map(picker_is_runtime_model_picker)
                    .unwrap_or(false)
                {
                    self.cycle_selected_model_favorite();
                    return Ok(());
                }
                if let Some(ref mut picker) = self.inline_interactive_state {
                    if picker.uses_compact_navigation() {
                        return Ok(());
                    }
                    if picker.column > 0 {
                        picker.column -= 1;
                    }
                }
            }
            KeyCode::Left => {
                if let Some(ref mut picker) = self.inline_interactive_state {
                    if picker.uses_compact_navigation() {
                        return Ok(());
                    }
                    if picker.column > 0 {
                        picker.column -= 1;
                    }
                }
            }
            KeyCode::Tab => {
                if let Some(ref mut picker) = self.inline_interactive_state {
                    if picker.uses_compact_navigation() {
                        return Ok(());
                    }
                    if picker.column == 0 && !picker.filter.is_empty() {
                        Self::tab_complete_inline_interactive_filter(picker);
                    } else if picker.column < picker.max_navigable_column()
                        && let Some(&idx) = picker.filtered.get(picker.selected)
                        && (picker.entries[idx].options.len() > 1 || picker.column > 0)
                    {
                        picker.column += 1;
                    }
                }
            }
            code if modifiers.contains(KeyModifiers::CONTROL)
                && key_char_eq_ignore_ascii_case(code, 'o') =>
            {
                if let Some(ref picker) = self.inline_interactive_state {
                    if !picker_is_runtime_model_picker(picker) {
                        return Ok(());
                    }
                    if picker.filtered.is_empty() {
                        return Ok(());
                    }
                    let idx = picker.filtered[picker.selected];
                    let entry = &picker.entries[idx];
                    if !matches!(entry.action, PickerAction::Model) {
                        return Ok(());
                    }
                    let route = entry.options.get(entry.selected_option);

                    let bare_name = model_entry_base_name(entry);

                    let (model_spec, provider_key) = if let Some(r) = route {
                        let selection =
                            crate::provider::MultiProvider::default_model_selection_from_route(
                                &bare_name,
                                &r.api_method,
                                &r.provider,
                            );
                        (selection.model_spec, selection.provider_key)
                    } else {
                        (bare_name.clone(), None)
                    };

                    let notice = format!(
                        "Default → {} via {}",
                        model_spec,
                        provider_key.as_deref().unwrap_or("auto")
                    );

                    match crate::config::Config::set_default_model(
                        Some(&model_spec),
                        provider_key.as_deref(),
                    ) {
                        Ok(()) => {
                            self.invalidate_model_picker_cache();
                            if let Some(ref mut picker) = self.inline_interactive_state {
                                for entry in &mut picker.entries {
                                    entry.is_default = false;
                                }
                                if let Some(entry) = picker.entries.get_mut(idx) {
                                    entry.is_default = true;
                                }
                            }
                            self.push_display_message(DisplayMessage::system(format!(
                                "Saved default model: {} via {}. This affects future sessions.",
                                model_spec,
                                provider_key.as_deref().unwrap_or("auto")
                            )));
                            self.set_status_notice(notice)
                        }
                        Err(e) => self.set_status_notice(format!("Failed to save default: {}", e)),
                    }
                }
            }
            code if modifiers.contains(KeyModifiers::CONTROL)
                && key_char_eq_ignore_ascii_case(code, 'n') =>
            {
                self.toggle_selected_model_favorite();
            }
            KeyCode::Enter => {
                let Some(ref mut picker) = self.inline_interactive_state else {
                    return Ok(());
                };
                if picker.filtered.is_empty() {
                    return Ok(());
                }
                let idx = picker.filtered[picker.selected];
                let entry = picker.entries[idx].clone();

                if matches!(entry.action, PickerAction::Model) {
                    if picker.column == 0 && entry.options.len() > 1 {
                        picker.column = 1;
                        return Ok(());
                    }
                    if picker.column == 1 {
                        picker.column = picker.max_navigable_column();
                        return Ok(());
                    }
                }

                let route = &entry.options[entry.selected_option];

                if !route.available {
                    let detail = if route.detail.is_empty() {
                        "not available".to_string()
                    } else {
                        route.detail.clone()
                    };
                    self.inline_interactive_state = None;
                    self.set_status_notice(format!("{} - {}", entry.name, detail));
                    return Ok(());
                }

                match entry.action {
                    PickerAction::Account(selection) => {
                        self.inline_interactive_state = None;
                        self.handle_account_picker_selection(selection);
                    }
                    PickerAction::Login(provider) => {
                        self.inline_interactive_state = None;
                        self.start_login_provider(provider);
                    }
                    PickerAction::Logout(provider) => {
                        self.inline_interactive_state = None;
                        self.start_logout_provider(provider);
                    }
                    PickerAction::LogoutAll => {
                        self.inline_interactive_state = None;
                        self.start_logout_all();
                    }
                    PickerAction::Usage {
                        title,
                        subtitle,
                        status,
                        detail_lines,
                        ..
                    } => {
                        self.inline_interactive_state = None;
                        let mut content = vec![format!("# {}", title), subtitle];
                        content.push(format!("status: {}", status.label_for_display()));
                        content.extend(detail_lines);
                        self.push_display_message(DisplayMessage::usage(content.join("\n")));
                        self.set_status_notice(format!("Usage → {}", title));
                    }
                    PickerAction::AgentTarget(target) => {
                        self.open_agent_model_picker(target);
                    }
                    PickerAction::AgentModelChoice {
                        target,
                        clear_override,
                    } => {
                        self.inline_interactive_state = None;
                        let result = if clear_override {
                            save_agent_model_override(target, None)
                        } else {
                            let spec = model_entry_saved_spec(&entry);
                            save_agent_model_override(target, Some(&spec))
                        };
                        match result {
                            Ok(()) => {
                                let label = agent_model_target_label(target);
                                if clear_override {
                                    self.push_display_message(DisplayMessage::system(format!(
                                        "{} model override cleared. It now inherits `{}`.",
                                        label,
                                        agent_model_default_summary(target, self)
                                    )));
                                    self.set_status_notice(format!("{} model: inherit", label));
                                } else {
                                    let spec = model_entry_saved_spec(&entry);
                                    self.push_display_message(DisplayMessage::system(format!(
                                        "Saved {} model override: `{}`.",
                                        label, spec
                                    )));
                                    self.set_status_notice(format!("{} model → {}", label, spec));
                                }
                            }
                            Err(error) => {
                                self.push_display_message(DisplayMessage::error(format!(
                                    "Failed to save {} model override: {}",
                                    agent_model_target_label(target),
                                    error
                                )));
                                self.set_status_notice("Agent model save failed");
                            }
                        }
                    }
                    PickerAction::Model => {
                        if !route.available {
                            self.push_display_message(DisplayMessage::error(
                                crate::tui::app::model_context::unavailable_model_route_message(
                                    &entry.name,
                                    &route.provider,
                                    &route.detail,
                                    self.is_remote,
                                ),
                            ));
                            self.set_status_notice("Model unavailable");
                            return Ok(());
                        }

                        let bare_name = model_entry_base_name(&entry);
                        let spec = if crate::provider::ModelRouteApiMethod::parse(&route.api_method)
                            .is_openrouter()
                            && route.provider == "auto"
                        {
                            openrouter_route_model_id(&bare_name)
                        } else {
                            picker_route_model_spec(&entry, route)
                        };
                        let route_selection = picker_route_selection(&entry, route);

                        let effort = entry.effort.clone();
                        record_model_picker_selection(&bare_name, route, effort.as_deref());
                        let method_label =
                            crate::provider::ModelRouteApiMethod::parse(&route.api_method)
                                .display_label();
                        let notice = format!(
                            "Model → {} via {} ({})",
                            entry.name, route.provider, method_label
                        );
                        let route_detail = route.detail.trim().to_string();

                        // Record exactly which model spec + route the user chose
                        // and how it will be applied. Pairs with the server-side
                        // model-switch logs so we can trace a `/model` choice all
                        // the way to the provider endpoint that ends up serving it
                        // (issues #292/#278: switch routes to wrong endpoint).
                        crate::logging::event_info(
                            "model_picker_select",
                            vec![
                                ("entry", entry.name.clone()),
                                ("spec", spec.clone()),
                                ("provider", route.provider.clone()),
                                ("api_method", route.api_method.clone()),
                                ("route_provider", route_selection.provider_label.clone()),
                                ("route_model", route_selection.model.clone()),
                                ("route_api_method", route_selection.api_method.clone()),
                                (
                                    "effort",
                                    effort.clone().unwrap_or_else(|| "none".to_string()),
                                ),
                                ("remote", self.is_remote.to_string()),
                            ],
                        );

                        if self.is_remote {
                            self.inline_interactive_state = None;
                            self.upstream_provider = None;
                            self.status_detail = None;
                            // Track the chosen method client-side so post-error
                            // fallback picks know which credential path the
                            // active route uses (remote sessions have no other
                            // route bookkeeping).
                            self.session.route_api_method =
                                Some(route_selection.api_method.clone());
                            self.pending_route_selection = Some(route_selection);
                            self.pending_model_switch = Some(spec);
                            // In remote mode `self.provider` is a local
                            // stand-in, so applying the picked effort variant
                            // to it does not reach the server. Stage it so the
                            // remote dispatcher forwards it right after the
                            // model switch; otherwise the server keeps its
                            // configured default (low) and silently runs e.g.
                            // "gpt-5.5 (high)" at low effort (issue #427).
                            self.pending_reasoning_effort = effort.clone();
                        } else {
                            match self.provider.set_route_selection(&route_selection) {
                                Ok(()) => {
                                    self.inline_interactive_state = None;
                                    self.provider_session_id = None;
                                    self.session.provider_session_id = None;
                                    self.upstream_provider = None;
                                    self.status_detail = None;
                                    self.invalidate_model_picker_cache();
                                    let active_model = self.provider.model();
                                    self.update_context_limit_for_model(&active_model);
                                    self.session.provider_key = crate::provider::MultiProvider::session_provider_key_after_model_switch(
                                        &spec,
                                        self.provider.name(),
                                        self.session.provider_key.as_deref(),
                                    );
                                    self.session.model = Some(active_model.clone());
                                    self.session.route_api_method =
                                        Some(route_selection.api_method.clone());
                                    let _ = self.session.save();
                                    crate::logging::event_info(
                                        "model_picker_select_applied",
                                        vec![
                                            ("spec", spec.clone()),
                                            ("active_model", active_model),
                                            ("provider", self.provider.name().to_string()),
                                            ("api_method", route_selection.api_method.clone()),
                                        ],
                                    );
                                }
                                Err(error) => {
                                    crate::logging::event_error(
                                        "model_picker_select_failed",
                                        vec![
                                            ("spec", spec.clone()),
                                            ("provider", route.provider.clone()),
                                            ("api_method", route_selection.api_method.clone()),
                                            ("error", error.to_string()),
                                        ],
                                    );
                                    self.push_display_message(DisplayMessage::error(
                                        crate::tui::app::model_context::model_switch_failure_message(
                                            &error.to_string(),
                                            self.is_remote,
                                        ),
                                    ));
                                    self.set_status_notice("Model switch failed");
                                    return Ok(());
                                }
                            }
                        }
                        if let Some(effort) = effort {
                            let _ = self.provider.set_reasoning_effort(&effort);
                        }
                        if !route_detail.is_empty() {
                            self.push_display_message(DisplayMessage::system(format!(
                                "{}\n{}",
                                notice, route_detail
                            )));
                        }
                        self.set_status_notice(if route_detail.is_empty() {
                            notice
                        } else {
                            format!("{} · {}", notice, route_detail)
                        });
                        // First-run onboarding: a model choice advances the flow.
                        self.onboarding_after_model_select();
                    }
                }
            }
            KeyCode::Backspace => {
                if let Some(ref mut picker) = self.inline_interactive_state
                    && picker.filter.pop().is_some()
                {
                    Self::apply_inline_interactive_filter(picker);
                }
            }
            KeyCode::Char(c) => {
                if let Some(ref mut picker) = self.inline_interactive_state
                    && !c.is_whitespace()
                {
                    picker.filter.push(c);
                    Self::apply_inline_interactive_filter(picker);
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn picker_fuzzy_score(pattern: &str, text: &str) -> Option<i32> {
        jcode_fuzzy::fuzzy_score_tokens(pattern, text)
    }

    pub(super) fn apply_inline_interactive_filter(picker: &mut InlineInteractiveState) {
        if picker.filter.is_empty() {
            picker.filtered = (0..picker.entries.len()).collect();
        } else {
            let query = picker.filter.trim();
            let mut scored: Vec<(usize, bool, i32)> = picker
                .entries
                .iter()
                .enumerate()
                .filter_map(|(i, m)| {
                    let filter_text = picker.filter_text(m);
                    Self::picker_fuzzy_score(&picker.filter, &filter_text).map(|s| {
                        let usage_bonus = m.usage_score.min(i32::MAX as u32) as i32;
                        let bonus = usage_bonus + if m.recommended { 5 } else { 0 };
                        (
                            i,
                            m.name.eq_ignore_ascii_case(query),
                            s.saturating_add(bonus),
                        )
                    })
                })
                .collect();
            scored.sort_by(|a, b| {
                b.1.cmp(&a.1)
                    .then(b.2.cmp(&a.2))
                    .then(
                        picker.entries[a.0]
                            .recommendation_rank
                            .cmp(&picker.entries[b.0].recommendation_rank),
                    )
                    .then(picker.entries[a.0].name.cmp(&picker.entries[b.0].name))
            });
            picker.filtered = scored.into_iter().map(|(i, _, _)| i).collect();
        }
        if picker.filtered.is_empty() {
            picker.selected = 0;
        } else {
            picker.selected = picker.selected.min(picker.filtered.len() - 1);
        }
    }

    pub(super) fn tab_complete_inline_interactive_filter(picker: &mut InlineInteractiveState) {
        if picker.filtered.is_empty() {
            return;
        }
        if picker.filtered.len() == 1 {
            let name = picker.entries[picker.filtered[0]].name.clone();
            picker.filter = name;
            Self::apply_inline_interactive_filter(picker);
            return;
        }
        let names: Vec<&str> = picker
            .filtered
            .iter()
            .map(|&i| picker.entries[i].name.as_str())
            .collect();
        let first = names[0].to_lowercase();
        let first_chars: Vec<char> = first.chars().collect();
        let mut prefix_len = first_chars.len();
        for name in names.iter().skip(1) {
            let lower = (*name).to_lowercase();
            let chars: Vec<char> = lower.chars().collect();
            let mut common = 0;
            for (a, b) in first_chars.iter().zip(chars.iter()) {
                if a == b {
                    common += 1;
                } else {
                    break;
                }
            }
            prefix_len = prefix_len.min(common);
        }
        if prefix_len > picker.filter.len() {
            let first_original = &picker.entries[picker.filtered[0]].name;
            picker.filter = first_original[..prefix_len].to_string();
            Self::apply_inline_interactive_filter(picker);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        REMOTE_MODEL_CATALOG_CACHE_MAX_AGE_SECS, REMOTE_MODEL_CATALOG_CACHE_VERSION,
        REMOTE_MODEL_CATALOG_MAX_DETAIL_BYTES, RemoteModelCatalogCache,
        filter_routes_by_provider_allowlist, key_char_eq_ignore_ascii_case,
        model_picker_route_is_current, model_picker_route_is_default,
        model_picker_route_is_recommended, picker_is_runtime_model_picker,
        remote_model_catalog_cache_is_fresh, remote_model_catalog_cache_origin,
        remote_model_catalog_snapshot_is_safe, route_supports_reasoning_effort,
    };
    use crate::tui::{
        AgentModelTarget, App, InlineInteractiveState, PickerAction, PickerEntry, PickerKind,
        PickerOption,
    };
    use crossterm::event::KeyCode;

    fn picker_entry(name: &str, provider: &str, usage_score: u32) -> PickerEntry {
        PickerEntry {
            name: name.to_string(),
            options: vec![picker_option(provider)],
            action: PickerAction::Model,
            selected_option: 0,
            is_current: false,
            is_default: false,
            is_favorite: false,
            recommended: false,
            recommendation_rank: usize::MAX,
            usage_score,
            old: false,
            created_date: None,
            effort: None,
        }
    }

    fn picker_option_with_method(provider: &str, api_method: &str) -> PickerOption {
        PickerOption {
            provider: provider.to_string(),
            api_method: api_method.to_string(),
            available: true,
            detail: String::new(),
            estimated_reference_cost_micros: None,
        }
    }

    fn picker_option(provider: &str) -> PickerOption {
        picker_option_with_method(provider, "test")
    }

    #[test]
    fn model_picker_hotkey_char_matching_is_case_insensitive() {
        assert!(key_char_eq_ignore_ascii_case(KeyCode::Char('f'), 'f'));
        assert!(key_char_eq_ignore_ascii_case(KeyCode::Char('F'), 'f'));
        assert!(key_char_eq_ignore_ascii_case(KeyCode::Char('D'), 'd'));
        assert!(!key_char_eq_ignore_ascii_case(KeyCode::Char('x'), 'f'));
    }

    #[test]
    fn runtime_model_picker_scope_excludes_agent_model_picker() {
        let runtime = InlineInteractiveState {
            kind: PickerKind::Model,
            filtered: vec![0],
            entries: vec![picker_entry("gpt-5.5", "OpenAI", 0)],
            selected: 0,
            column: 0,
            filter: String::new(),
            preview: false,
        };
        let mut agent_entry = picker_entry("Swarm / subagent", "gpt-5 default", 0);
        agent_entry.action = PickerAction::AgentTarget(AgentModelTarget::Swarm);
        let agent = InlineInteractiveState {
            kind: PickerKind::Model,
            filtered: vec![0],
            entries: vec![agent_entry],
            selected: 0,
            column: 0,
            filter: String::new(),
            preview: false,
        };

        assert!(picker_is_runtime_model_picker(&runtime));
        assert!(!picker_is_runtime_model_picker(&agent));
    }

    #[test]
    fn model_picker_fuzzy_filter_prefers_previously_selected_route() {
        let mut picker = InlineInteractiveState {
            kind: PickerKind::Model,
            filtered: vec![0, 1],
            entries: vec![
                picker_entry("claude-opus-4.6", "Cursor", 0),
                picker_entry("claude-opus-4.5", "Anthropic", 150),
            ],
            selected: 0,
            column: 0,
            filter: "opus".to_string(),
            preview: false,
        };

        App::apply_inline_interactive_filter(&mut picker);

        assert_eq!(picker.filtered, vec![1, 0]);
    }

    #[test]
    fn model_picker_fuzzy_filter_tolerates_common_typos() {
        let mut picker = InlineInteractiveState {
            kind: PickerKind::Model,
            filtered: vec![0, 1],
            entries: vec![
                picker_entry("gpt-5-codex", "OpenAI", 0),
                picker_entry("claude-opus-4.6", "Anthropic", 0),
            ],
            selected: 0,
            column: 0,
            filter: "codxe".to_string(),
            preview: false,
        };

        App::apply_inline_interactive_filter(&mut picker);

        assert_eq!(picker.filtered, vec![0]);
    }

    #[test]
    fn model_picker_exact_name_outranks_longer_frequently_used_prefix() {
        let mut picker = InlineInteractiveState {
            kind: PickerKind::Model,
            filtered: vec![0, 1],
            entries: vec![
                picker_entry("gpt-5.5", "OpenAI", 150),
                picker_entry("gpt-5", "OpenAI", 0),
            ],
            selected: 0,
            column: 0,
            filter: "gpt-5".to_string(),
            preview: false,
        };

        App::apply_inline_interactive_filter(&mut picker);

        assert_eq!(picker.filtered, vec![1, 0]);
    }

    #[test]
    fn model_picker_current_route_requires_matching_provider() {
        let openai_route = picker_option("OpenAI");
        let copilot_route = picker_option("Copilot");

        assert!(model_picker_route_is_current(
            "gpt-5.5",
            &openai_route,
            "gpt-5.5",
            "OpenAI",
        ));
        assert!(!model_picker_route_is_current(
            "gpt-5.5",
            &copilot_route,
            "gpt-5.5",
            "OpenAI",
        ));
    }

    #[test]
    fn model_picker_current_route_allows_provider_aliases() {
        assert!(jcode_provider_core::model_route_provider_labels_match(
            "Anthropic",
            "Claude"
        ));
        assert!(jcode_provider_core::model_route_provider_labels_match(
            "auto",
            "OpenRouter"
        ));
        assert!(jcode_provider_core::model_route_provider_labels_match(
            "GitHub Copilot",
            "Copilot"
        ));
        assert!(jcode_provider_core::model_route_provider_labels_match(
            "AWS Bedrock",
            "Bedrock"
        ));
    }

    #[test]
    fn model_picker_provider_match_does_not_use_substring_false_positives() {
        assert!(!jcode_provider_core::model_route_provider_labels_match(
            "OpenRouter/OpenAI",
            "OpenAI"
        ));
        assert!(!jcode_provider_core::model_route_provider_labels_match(
            "OpenAI",
            "OpenRouter"
        ));
    }

    #[test]
    fn model_picker_default_route_requires_matching_provider_when_config_has_provider() {
        let openai_route = picker_option_with_method("OpenAI", "openai-oauth");
        let copilot_route = picker_option_with_method("Copilot", "copilot");

        assert!(model_picker_route_is_default(
            "gpt-5.5",
            &openai_route,
            Some("gpt-5.5"),
            Some("openai"),
        ));
        assert!(!model_picker_route_is_default(
            "gpt-5.5",
            &copilot_route,
            Some("gpt-5.5"),
            Some("openai"),
        ));
    }

    #[test]
    fn model_picker_default_route_marks_anthropic_api_config_provider() {
        // Regression: config `default_provider = "anthropic-api"` is the
        // dual-auth spelling of the route keyed `anthropic-api-key`. The picker
        // must still mark the Anthropic API-key route as the default ★ even
        // though the two spellings normalize differently, and must NOT mark the
        // OAuth route for the same model.
        let api_route = picker_option_with_method("Anthropic", "anthropic-api-key");
        let oauth_route = picker_option_with_method("Anthropic", "claude-oauth");

        assert!(model_picker_route_is_default(
            "claude-opus-4-8",
            &api_route,
            Some("claude-opus-4-8"),
            Some("anthropic-api"),
        ));
        assert!(!model_picker_route_is_default(
            "claude-opus-4-8",
            &oauth_route,
            Some("claude-opus-4-8"),
            Some("anthropic-api"),
        ));

        // The equivalent `claude-api` spelling behaves identically.
        assert!(model_picker_route_is_default(
            "claude-opus-4-8",
            &api_route,
            Some("claude-opus-4-8"),
            Some("claude-api"),
        ));
    }

    #[test]
    fn model_picker_default_route_honors_provider_prefixed_model_specs() {
        let openai_route = picker_option_with_method("OpenAI", "openai-oauth");
        let copilot_route = picker_option_with_method("Copilot", "copilot");

        assert!(model_picker_route_is_default(
            "gpt-5.5",
            &copilot_route,
            Some("copilot:gpt-5.5"),
            None,
        ));
        assert!(!model_picker_route_is_default(
            "gpt-5.5",
            &openai_route,
            Some("copilot:gpt-5.5"),
            None,
        ));
    }

    #[test]
    fn model_picker_default_route_matches_openrouter_endpoint_specs() {
        let openrouter_openai_route = picker_option_with_method("OpenAI", "openrouter");

        assert!(model_picker_route_is_default(
            "gpt-5.5",
            &openrouter_openai_route,
            Some("openai/gpt-5.5@OpenAI"),
            Some("openrouter"),
        ));
        assert!(!model_picker_route_is_default(
            "gpt-5.5",
            &openrouter_openai_route,
            Some("anthropic/gpt-5.5@OpenAI"),
            Some("openrouter"),
        ));
    }

    #[test]
    fn model_picker_recommended_route_is_provider_aware() {
        let openai_oauth_route = picker_option_with_method("OpenAI", "openai-oauth");
        let openai_api_key_route = picker_option_with_method("OpenAI", "openai-api-key");
        let copilot_route = picker_option_with_method("Copilot", "copilot");
        let claude_oauth_route = picker_option_with_method("Anthropic", "claude-oauth");
        let claude_openrouter_route = picker_option_with_method("Anthropic", "openrouter");
        let openrouter_auto_route = picker_option_with_method("auto", "openrouter");
        let openrouter_provider_route = picker_option_with_method("DeepSeek", "openrouter");
        let deepseek_direct_route =
            picker_option_with_method("DeepSeek", "openai-compatible:deepseek");
        let unavailable_openai_oauth_route = PickerOption {
            available: false,
            ..openai_oauth_route.clone()
        };

        assert!(model_picker_route_is_recommended(
            "gpt-5.5",
            &openai_oauth_route
        ));
        assert!(!model_picker_route_is_recommended(
            "gpt-5.5",
            &openai_api_key_route
        ));
        assert!(!model_picker_route_is_recommended(
            "gpt-5.5",
            &copilot_route
        ));
        assert!(!model_picker_route_is_recommended(
            "gpt-5.5",
            &unavailable_openai_oauth_route,
        ));

        // Current policy (see jcode-provider-core): claude-opus-4-8 is the
        // recommended Anthropic flagship; older Opus and OpenRouter/Copilot
        // routes are not recommended.
        assert!(model_picker_route_is_recommended(
            "claude-opus-4-8",
            &claude_oauth_route,
        ));
        assert!(!model_picker_route_is_recommended(
            "claude-opus-4-7",
            &claude_oauth_route,
        ));
        assert!(!model_picker_route_is_recommended(
            "claude-opus-4-8",
            &claude_openrouter_route,
        ));
        assert!(!model_picker_route_is_recommended(
            "claude-opus-4-8",
            &copilot_route,
        ));

        // DeepSeek routes are no longer in the recommended set at all.
        assert!(!model_picker_route_is_recommended(
            "deepseek/deepseek-v4-pro",
            &openrouter_auto_route,
        ));
        assert!(!model_picker_route_is_recommended(
            "deepseek/deepseek-v4-pro",
            &deepseek_direct_route,
        ));
        assert!(!model_picker_route_is_recommended(
            "deepseek/deepseek-v4-pro",
            &openrouter_provider_route,
        ));
    }

    #[test]
    fn remote_model_catalog_cache_keeps_flattened_legacy_schema() {
        let cache: RemoteModelCatalogCache = serde_json::from_value(serde_json::json!({
            "version": 1,
            "provider_name": "OpenAI",
            "provider_model": "gpt-5.5",
            "available_models": ["gpt-5.5"],
            "model_routes": [{
                "model": "gpt-5.5",
                "provider": "OpenAI",
                "api_method": "openai-oauth",
                "available": true,
                "detail": "OAuth"
            }],
            "observed_at_unix_secs": 123,
        }))
        .expect("legacy flattened remote cache should deserialize");

        assert_eq!(cache.snapshot.provider_name.as_deref(), Some("OpenAI"));
        assert_eq!(cache.snapshot.provider_model.as_deref(), Some("gpt-5.5"));
        assert_eq!(cache.snapshot.available_models, ["gpt-5.5"]);
        assert_eq!(cache.snapshot.model_routes.len(), 1);
        assert!(cache.origin.is_empty());
        assert_eq!(
            cache.snapshot.model_routes[0].api_method_kind(),
            crate::provider::ModelRouteApiMethod::OpenAIOAuth
        );

        let serialized = serde_json::to_value(&cache).expect("cache should serialize");
        assert_eq!(serialized["provider_name"], "OpenAI");
        assert!(serialized.get("snapshot").is_none());
    }

    #[test]
    fn remote_model_catalog_cache_rejects_stale_and_future_timestamps() {
        let snapshot = jcode_provider_core::ModelCatalogSnapshot::new(
            Some("OpenAI".to_string()),
            Some("gpt-5.5".to_string()),
            vec!["gpt-5.5".to_string()],
            vec![model_route("gpt-5.5", "OpenAI", "openai-oauth")],
        );
        let now = REMOTE_MODEL_CATALOG_CACHE_MAX_AGE_SECS + 10_000;
        let mut cache = RemoteModelCatalogCache {
            version: REMOTE_MODEL_CATALOG_CACHE_VERSION,
            origin: remote_model_catalog_cache_origin(),
            snapshot,
            observed_at_unix_secs: now,
        };

        assert!(remote_model_catalog_cache_is_fresh(&cache, now));
        cache.observed_at_unix_secs = now - REMOTE_MODEL_CATALOG_CACHE_MAX_AGE_SECS - 1;
        assert!(!remote_model_catalog_cache_is_fresh(&cache, now));
        cache.observed_at_unix_secs = now + 5 * 60 + 1;
        assert!(!remote_model_catalog_cache_is_fresh(&cache, now));
    }

    #[test]
    fn remote_model_catalog_cache_rejects_forged_or_oversized_routes() {
        let safe_snapshot = jcode_provider_core::ModelCatalogSnapshot::new(
            Some("AWS Bedrock".to_string()),
            Some("us.anthropic.claude-sonnet-4-6".to_string()),
            vec!["us.anthropic.claude-sonnet-4-6".to_string()],
            vec![model_route(
                "us.anthropic.claude-sonnet-4-6",
                "AWS Bedrock",
                "bedrock",
            )],
        );
        assert!(remote_model_catalog_snapshot_is_safe(&safe_snapshot));

        let mut forged = safe_snapshot.clone();
        forged.model_routes[0].api_method = "shell:steal-credentials".to_string();
        assert!(!remote_model_catalog_snapshot_is_safe(&forged));

        let mut control = safe_snapshot.clone();
        control.model_routes[0].provider = "AWS Bedrock\nOpenAI".to_string();
        assert!(!remote_model_catalog_snapshot_is_safe(&control));

        let mut oversized = safe_snapshot;
        oversized.model_routes[0].detail = "x".repeat(REMOTE_MODEL_CATALOG_MAX_DETAIL_BYTES + 1);
        assert!(!remote_model_catalog_snapshot_is_safe(&oversized));
    }

    fn model_route(model: &str, provider: &str, api_method: &str) -> crate::provider::ModelRoute {
        crate::provider::ModelRoute {
            model: model.to_string(),
            provider: provider.to_string(),
            api_method: api_method.to_string(),
            available: true,
            detail: String::new(),
            cheapness: None,
        }
    }

    #[test]
    fn route_effort_support_covers_effort_capable_runtimes_only() {
        assert!(route_supports_reasoning_effort("claude-oauth"));
        assert!(route_supports_reasoning_effort("claude-api"));
        assert!(route_supports_reasoning_effort("openai-oauth"));
        assert!(route_supports_reasoning_effort("openai-api-key"));
        assert!(route_supports_reasoning_effort("openrouter"));
        assert!(route_supports_reasoning_effort(
            "openai-compatible:llamacpp"
        ));

        assert!(!route_supports_reasoning_effort("copilot"));
        assert!(!route_supports_reasoning_effort("bedrock"));
        assert!(!route_supports_reasoning_effort("https"));
        assert!(!route_supports_reasoning_effort("openai-compatible"));
        assert!(!route_supports_reasoning_effort("remote-catalog"));
        assert!(!route_supports_reasoning_effort("current"));
    }

    #[test]
    fn provider_allowlist_filters_routes_by_label_method_and_profile() {
        let routes = vec![
            model_route("gpt-5.5", "OpenAI", "openai-oauth"),
            model_route("claude-fable-5", "Anthropic", "claude-oauth"),
            model_route("qwen3-coder", "llama.cpp", "openai-compatible:llamacpp"),
            model_route("deepseek/deepseek-v4-pro", "auto", "openrouter"),
        ];

        // Provider label match (normalized: case/dots/spaces insensitive).
        let filtered = filter_routes_by_provider_allowlist(
            routes.clone(),
            Some(&["Llama.CPP".to_string()]),
            "unrelated-current",
        );
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].model, "qwen3-coder");

        // Bare openai-compatible profile id match.
        let filtered = filter_routes_by_provider_allowlist(
            routes.clone(),
            Some(&["llamacpp".to_string()]),
            "unrelated-current",
        );
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].provider, "llama.cpp");

        // Api-method match plus alias-aware provider label match.
        let filtered = filter_routes_by_provider_allowlist(
            routes.clone(),
            Some(&["claude-oauth".to_string(), "openrouter".to_string()]),
            "unrelated-current",
        );
        let models: Vec<&str> = filtered.iter().map(|r| r.model.as_str()).collect();
        assert_eq!(models, ["claude-fable-5", "deepseek/deepseek-v4-pro"]);
    }

    #[test]
    fn provider_allowlist_keeps_current_model_and_never_empties_picker() {
        let routes = vec![
            model_route("gpt-5.5", "OpenAI", "openai-oauth"),
            model_route("qwen3-coder", "llama.cpp", "openai-compatible:llamacpp"),
        ];

        // Current model's route survives even when its provider is filtered out.
        let filtered = filter_routes_by_provider_allowlist(
            routes.clone(),
            Some(&["llamacpp".to_string()]),
            "gpt-5.5",
        );
        let models: Vec<&str> = filtered.iter().map(|r| r.model.as_str()).collect();
        assert_eq!(models, ["gpt-5.5", "qwen3-coder"]);

        // A filter matching nothing falls back to the full list.
        let filtered = filter_routes_by_provider_allowlist(
            routes.clone(),
            Some(&["nonexistent".to_string()]),
            "unrelated-current",
        );
        assert_eq!(filtered.len(), routes.len());

        // None / empty / blank-entry allowlists are no-ops.
        assert_eq!(
            filter_routes_by_provider_allowlist(routes.clone(), None, "x").len(),
            2
        );
        assert_eq!(
            filter_routes_by_provider_allowlist(routes.clone(), Some(&[]), "x").len(),
            2
        );
        assert_eq!(
            filter_routes_by_provider_allowlist(routes, Some(&["  ".to_string()]), "x").len(),
            2
        );
    }
}
