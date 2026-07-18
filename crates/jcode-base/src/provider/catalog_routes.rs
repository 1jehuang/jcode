use crate::auth::{AuthState, AuthStatus};

use super::pricing::cheapness_for_route;
use super::{
    ALL_OPENAI_MODELS, AccountModelAvailabilityState, CHATGPT_WEB_MODEL, ModelRoute, MultiProvider,
    Provider, anthropic_api_key_route_availability, anthropic_oauth_route_availability, bedrock,
    build_anthropic_oauth_route, build_chatgpt_web_route, build_copilot_route,
    build_openai_api_key_route, build_openai_oauth_route, build_openrouter_auto_route,
    build_openrouter_endpoint_route, build_openrouter_fallback_provider_route,
    configured_standard_openrouter_profile_routes, copilot, dedupe_model_routes,
    direct_openai_compatible_profile_routes, format_account_model_availability_detail,
    is_listable_model_name, known_anthropic_model_ids, known_openai_model_ids,
    model_availability_for_account, openrouter, openrouter_catalog_model_id, provider_for_model,
    standard_openrouter_profile_configured,
};

/// Build the fast local route snapshot used by the TUI model picker while the
/// full provider catalog is hydrating.
///
/// This intentionally lives in the provider layer rather than the TUI so auth,
/// provider, and catalog policy have one source of truth. The TUI should only
/// group, sort, and render the returned routes.
pub fn simplified_model_routes_for_picker(
    current_provider_name: &str,
    current_model: &str,
    display_models: impl IntoIterator<Item = String>,
) -> Vec<ModelRoute> {
    let auth = AuthStatus::check_fast();
    let mut routes = Vec::new();

    for model in display_models {
        if model == CHATGPT_WEB_MODEL {
            routes.push(build_chatgpt_web_route());
            continue;
        }
        if !model.contains('/') && provider_for_model(&model) == Some("openai") {
            if auth.openai_has_oauth {
                routes.push(ModelRoute {
                    model: model.clone(),
                    provider: "OpenAI".to_string(),
                    api_method: "openai-oauth".to_string(),
                    available: true,
                    detail: String::new(),
                    cheapness: None,
                });
            }
            if auth.openai_has_api_key {
                routes.push(ModelRoute {
                    model: model.clone(),
                    provider: "OpenAI".to_string(),
                    api_method: "openai-api-key".to_string(),
                    available: true,
                    detail: String::new(),
                    cheapness: None,
                });
            }
            if auth.openai == AuthState::NotConfigured {
                routes.push(ModelRoute {
                    model,
                    provider: "OpenAI".to_string(),
                    api_method: "openai-oauth".to_string(),
                    available: false,
                    detail: "no credentials".to_string(),
                    cheapness: None,
                });
            }
            continue;
        }

        let (provider, api_method, available, detail) =
            if super::bedrock::BedrockProvider::is_bedrock_model_id(&model) {
                (
                    "AWS Bedrock".to_string(),
                    "bedrock".to_string(),
                    auth.bedrock != AuthState::NotConfigured,
                    if auth.bedrock == AuthState::NotConfigured {
                        "no Bedrock credentials or region; run /login bedrock".to_string()
                    } else {
                        String::new()
                    },
                )
            } else if model.contains('/') {
                (
                    "auto".to_string(),
                    "openrouter".to_string(),
                    auth.openrouter != AuthState::NotConfigured,
                    "simplified catalog".to_string(),
                )
            } else {
                match provider_for_model(&model) {
                    Some("claude") => {
                        append_simplified_anthropic_model_routes(&mut routes, model, &auth);
                        continue;
                    }
                    Some("openai") => unreachable!("OpenAI models are handled above"),
                    Some("gemini") => (
                        "Gemini".to_string(),
                        "code-assist-oauth".to_string(),
                        auth.gemini != AuthState::NotConfigured,
                        String::new(),
                    ),
                    Some("cursor") => (
                        "Cursor".to_string(),
                        "cursor".to_string(),
                        auth.cursor != AuthState::NotConfigured,
                        String::new(),
                    ),
                    Some("openrouter") => (
                        "auto".to_string(),
                        "openrouter".to_string(),
                        auth.openrouter != AuthState::NotConfigured,
                        "simplified catalog".to_string(),
                    ),
                    Some(other) => (other.to_string(), other.to_string(), true, String::new()),
                    None => (
                        current_provider_name.to_string(),
                        "current".to_string(),
                        true,
                        String::new(),
                    ),
                }
            };

        routes.push(ModelRoute {
            model,
            provider,
            api_method,
            available,
            detail,
            cheapness: None,
        });
    }

    if routes.is_empty() && !current_model.is_empty() && current_model != "unknown" {
        routes.push(ModelRoute {
            model: current_model.to_string(),
            provider: current_provider_name.to_string(),
            api_method: "current".to_string(),
            available: true,
            detail: "simplified catalog".to_string(),
            cheapness: None,
        });
    }

    routes
}

pub fn append_simplified_anthropic_model_routes(
    routes: &mut Vec<ModelRoute>,
    model: impl Into<String>,
    auth: &AuthStatus,
) {
    let model = model.into();
    if auth.anthropic.has_oauth {
        routes.push(ModelRoute {
            model: model.clone(),
            provider: "Anthropic".to_string(),
            api_method: "claude-oauth".to_string(),
            available: true,
            detail: String::new(),
            cheapness: None,
        });
    }
    if auth.anthropic.has_api_key {
        routes.push(ModelRoute {
            model: model.clone(),
            provider: "Anthropic".to_string(),
            api_method: "claude-api".to_string(),
            available: true,
            detail: String::new(),
            cheapness: None,
        });
    }
    if !auth.anthropic.has_oauth && !auth.anthropic.has_api_key {
        routes.push(ModelRoute {
            model,
            provider: "Anthropic".to_string(),
            api_method: "claude-oauth".to_string(),
            available: false,
            detail: "no credentials".to_string(),
            cheapness: None,
        });
    }
}

/// Per-build statistics for the OpenRouter section of route construction,
/// used only for the timing/summary log lines.
#[derive(Default)]
struct OpenRouterRouteStats {
    models: usize,
    endpoint_cache_hits: usize,
    endpoint_routes: usize,
    scheduled_endpoint_refreshes: usize,
}

/// Build the full multi-provider route catalog.
///
/// Orchestration only: each provider family contributes routes through its
/// own `append_*_routes` builder below, so provider-specific policy stays in
/// one place per provider instead of one 400-line function.
pub(super) fn multiprovider_model_routes(provider: &MultiProvider) -> Vec<ModelRoute> {
    let routes_started = std::time::Instant::now();
    provider.spawn_anthropic_catalog_refresh_if_needed();
    provider.spawn_openai_catalog_refresh_if_needed();

    let mut routes = Vec::new();
    let mut openrouter_stats = OpenRouterRouteStats::default();

    let has_oauth = provider.has_claude_runtime();
    let has_api_key = crate::provider_catalog::load_api_key_from_env_or_config(
        "ANTHROPIC_API_KEY",
        "anthropic.env",
    )
    .is_some();
    let openai_auth = crate::auth::AuthStatus::check_fast();

    append_anthropic_routes(provider, &mut routes, has_oauth, has_api_key);
    append_openai_routes(provider, &mut routes, &openai_auth);
    let added_direct_openai_compatible_routes =
        append_openai_compatible_profile_routes(provider, &mut routes);
    append_copilot_routes(provider, &mut routes);
    append_gemini_routes(provider, &mut routes);
    append_antigravity_routes(provider, &mut routes);
    append_cursor_routes(provider, &mut routes);
    append_bedrock_routes(provider, &mut routes);

    let has_openrouter = provider.openrouter_provider().is_some();
    let has_openrouter_provider_features = provider
        .openrouter_provider()
        .map(|openrouter| openrouter.supports_provider_routing_features())
        .unwrap_or(false);
    append_openrouter_routes(provider, &mut routes, &mut openrouter_stats);

    if !has_openrouter && !added_direct_openai_compatible_routes {
        // OpenRouter not configured - show a placeholder as unavailable.
        routes.push(ModelRoute {
            model: "openrouter models".to_string(),
            provider: "—".to_string(),
            api_method: "openrouter".to_string(),
            available: false,
            detail: "OPENROUTER_API_KEY not set".to_string(),
            cheapness: None,
        });
    }

    if has_openrouter_provider_features {
        append_openrouter_alternative_routes(&mut routes, &mut openrouter_stats);
    }

    let total_ms = routes_started.elapsed().as_millis();
    if total_ms >= 250 || std::env::var("JCODE_LOG_MODEL_PICKER_TIMING").is_ok() {
        crate::logging::info(&format!(
            "[TIMING] model_routes: routes={}, openrouter_configured={}, openrouter_models={}, openrouter_endpoint_cache_hits={}, openrouter_endpoint_routes={}, openrouter_scheduled_endpoint_refreshes={}, total={}ms",
            routes.len(),
            has_openrouter,
            openrouter_stats.models,
            openrouter_stats.endpoint_cache_hits,
            openrouter_stats.endpoint_routes,
            openrouter_stats.scheduled_endpoint_refreshes,
            total_ms,
        ));
    }

    let routes_before_filter = routes.len();

    // Drop obviously non-chat models (embeddings, speech, rerankers, etc.) that
    // some providers (Bedrock, OpenAI-compatible profiles like NVIDIA NIM / FPT
    // / Chutes) dump wholesale into their catalogs. Without this the picker is
    // flooded with hundreds of unusable entries.
    routes.retain(|route| is_listable_model_name(&route.model));

    let routes = dedupe_model_routes(routes);

    // Structured, always-on summary of catalog route building. This is the
    // single most useful line for the recurring "model picker empty / only
    // OpenAI+Anthropic appear / configured provider's models missing" reports
    // (issues #292, #268, #312, #304): it records which credentials were
    // detected and how many routes each provider contributed, so a shared log
    // explains exactly why a model was or was not offered. No secrets here.
    log_model_routes_summary(
        "build",
        &routes,
        routes_before_filter,
        has_oauth,
        has_api_key,
        openai_auth.openai_has_oauth,
        openai_auth.openai_has_api_key,
        has_openrouter,
        has_openrouter_provider_features,
        added_direct_openai_compatible_routes,
        total_ms,
    );

    routes
}

/// Anthropic models via OAuth and/or API key.
fn append_anthropic_routes(
    provider: &MultiProvider,
    routes: &mut Vec<ModelRoute>,
    has_oauth: bool,
    has_api_key: bool,
) {
    let anthropic_models = if let Some(anthropic) = provider.anthropic_provider() {
        anthropic.available_models_for_switching()
    } else if let Some(claude) = provider.claude_provider() {
        claude.available_models_for_switching()
    } else {
        known_anthropic_model_ids()
    };

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
                api_method: "claude-api".to_string(),
                available: ak_available,
                detail: ak_detail,
                cheapness: cheapness_for_route(&model, "Anthropic", "claude-api"),
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
}

/// OpenAI models via OAuth and/or API key, with per-account availability.
fn append_openai_routes(
    provider: &MultiProvider,
    routes: &mut Vec<ModelRoute>,
    openai_auth: &crate::auth::AuthStatus,
) {
    let openai_models = if let Some(openai) = provider.openai_provider() {
        openai.available_models_for_switching()
    } else {
        known_openai_model_ids()
    };

    for model in openai_models {
        if model == CHATGPT_WEB_MODEL {
            routes.push(build_chatgpt_web_route());
            continue;
        }
        let availability = model_availability_for_account(&model);
        let (available, detail) = if provider.openai_provider().is_none() {
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
        if openai_auth.openai_has_oauth {
            routes.push(build_openai_oauth_route(&model, available, detail.clone()));
        }
        if openai_auth.openai_has_api_key {
            routes.push(build_openai_api_key_route(
                &model,
                provider.openai_provider().is_some(),
                String::new(),
            ));
        }
        if !openai_auth.openai_has_oauth && !openai_auth.openai_has_api_key {
            routes.push(build_openai_oauth_route(&model, false, detail));
        }
    }
}

/// Configured OpenAI-compatible profiles (NVIDIA NIM, Groq, ...), excluding
/// the active direct profile which contributes through the OpenRouter path.
/// Returns whether any routes were added.
fn append_openai_compatible_profile_routes(
    provider: &MultiProvider,
    routes: &mut Vec<ModelRoute>,
) -> bool {
    let active_direct_openai_compatible_api_method = provider
        .openrouter_provider()
        .and_then(|openrouter| openrouter.direct_openai_compatible_route_parts())
        .map(|(_, api_method, _)| api_method);
    let mut added_any = false;
    for profile in crate::provider_catalog::openai_compatible_profiles()
        .iter()
        .copied()
    {
        if !crate::provider_catalog::openai_compatible_profile_is_configured(profile) {
            continue;
        }
        let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
        let api_method = format!("openai-compatible:{}", resolved.id);

        // The active OpenRouter/OpenAI-compatible provider contributes its own
        // live memory/disk catalog below. Do not preempt it with the generic
        // configured-profile path, because its in-memory catalog may be newer
        // than the disk snapshot that this non-active profile path can read.
        if active_direct_openai_compatible_api_method.as_deref() == Some(api_method.as_str()) {
            continue;
        }

        let profile_routes = direct_openai_compatible_profile_routes(profile);
        added_any |= !profile_routes.is_empty();
        routes.extend(profile_routes);
    }

    // User-defined named provider profiles (`[providers.<name>]` in
    // config.toml). Their statically declared `[[providers.<name>.models]]`
    // entries (and `default_model`) must surface in the picker with a route
    // back to that profile, even when the profile is not the active provider
    // (issue #444).
    for (profile_name, profile_config) in &crate::config::config().providers {
        let api_method = format!("openai-compatible:{}", profile_name);
        // The active runtime already contributes this profile's models (with
        // live-catalog freshness) via the OpenRouter slot path.
        if active_direct_openai_compatible_api_method.as_deref() == Some(api_method.as_str()) {
            continue;
        }
        let named_routes = named_provider_profile_routes(profile_name, profile_config);
        added_any |= !named_routes.is_empty();
        routes.extend(named_routes);
    }
    added_any
}

/// Picker routes for one user-defined named provider profile from config.
///
/// Text-capable static models plus the profile's `default_model` are offered;
/// models declared image-only via `input = ["image"]` are excluded.
fn named_provider_profile_routes(
    profile_name: &str,
    profile_config: &crate::config::NamedProviderConfig,
) -> Vec<ModelRoute> {
    let mut models: Vec<String> = profile_config
        .models
        .iter()
        .filter(|model| {
            // `input` empty means unspecified (assume text-capable).
            model.input.is_empty() || model.input.iter().any(|input| input == "text")
        })
        .map(|model| model.id.trim().to_string())
        .filter(|id| !id.is_empty())
        .collect();
    if models.is_empty()
        && let Some(default_model) = profile_config
            .default_model
            .as_deref()
            .map(str::trim)
            .filter(|model| !model.is_empty())
    {
        models.push(default_model.to_string());
    }

    let api_method = format!("openai-compatible:{}", profile_name);
    let detail = if profile_config.base_url.trim().is_empty() {
        "configured provider profile".to_string()
    } else {
        profile_config.base_url.trim().to_string()
    };

    let mut routes: Vec<ModelRoute> = Vec::new();
    for model in models {
        if !is_listable_model_name(&model) || routes.iter().any(|route| route.model == model) {
            continue;
        }
        routes.push(ModelRoute {
            model,
            provider: profile_name.to_string(),
            api_method: api_method.clone(),
            available: true,
            detail: detail.clone(),
            cheapness: None,
        });
    }
    routes
}

/// GitHub Copilot models, or a placeholder when credentials exist but the
/// provider is not initialized.
fn append_copilot_routes(provider: &MultiProvider, routes: &mut Vec<ModelRoute>) {
    if let Some(copilot) = provider.copilot_provider() {
        let copilot_models = copilot.available_models_display();
        let detail = copilot.model_catalog_detail();
        let copilot_models_empty = copilot_models.is_empty();
        for model in copilot_models {
            routes.push(build_copilot_route(&model, true, detail.clone()));
        }
        if copilot_models_empty && copilot::has_credentials() {
            routes.push(build_copilot_route("copilot models", false, detail));
        }
    } else if copilot::has_credentials() {
        routes.push(build_copilot_route(
            "copilot models",
            false,
            "not initialized yet",
        ));
    }
}

fn append_gemini_routes(provider: &MultiProvider, routes: &mut Vec<ModelRoute>) {
    if let Some(gemini) = provider.gemini_provider() {
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

fn append_antigravity_routes(provider: &MultiProvider, routes: &mut Vec<ModelRoute>) {
    if let Some(antigravity) = provider.antigravity_provider() {
        routes.extend(antigravity.model_routes());
    }
}

fn append_cursor_routes(provider: &MultiProvider, routes: &mut Vec<ModelRoute>) {
    if let Some(cursor) = provider.cursor_provider() {
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

/// AWS Bedrock models and inference profiles, including the
/// credentials-configured-but-uninitialized case.
fn append_bedrock_routes(provider: &MultiProvider, routes: &mut Vec<ModelRoute>) {
    if let Some(bedrock) = provider.bedrock_provider() {
        routes.extend(bedrock.model_routes());
    } else if bedrock::BedrockProvider::has_credentials() {
        let bedrock = bedrock::BedrockProvider::new();
        routes.extend(bedrock.model_routes().into_iter().map(|mut route| {
            if route.detail.trim().is_empty() {
                route.detail =
                    "credentials configured; provider will initialize on selection".to_string();
            }
            route
        }));
    }
}

/// OpenRouter models with per-provider endpoint routes, plus the direct
/// OpenAI-compatible runtime path that shares the OpenRouter transport.
fn append_openrouter_routes(
    provider: &MultiProvider,
    routes: &mut Vec<ModelRoute>,
    stats: &mut OpenRouterRouteStats,
) {
    let Some(openrouter) = provider.openrouter_provider() else {
        return;
    };
    let has_openrouter = true;
    let current_openrouter_model = openrouter.model();
    let supports_openrouter_provider_features = openrouter.supports_provider_routing_features();
    let mut scheduled_endpoint_refreshes = 0usize;
    for model in openrouter.available_models_display() {
        stats.models += 1;
        let cached = if supports_openrouter_provider_features {
            openrouter::load_endpoints_disk_cache_public(&model)
        } else {
            None
        };
        let cache_age = cached.as_ref().map(|(_, age)| *age);
        if supports_openrouter_provider_features
            && (model == current_openrouter_model || scheduled_endpoint_refreshes < 8)
            && openrouter.maybe_schedule_endpoint_refresh_for_display(
                &model,
                cache_age,
                "model picker route hydration",
            )
        {
            scheduled_endpoint_refreshes += 1;
            stats.scheduled_endpoint_refreshes += 1;
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
            .and_then(|(eps, _)| {
                eps.first().map(|ep| {
                    let endpoint_detail = ep.detail_string();
                    if endpoint_detail.trim().is_empty() {
                        format!("→ {}", ep.provider_name)
                    } else {
                        format!("→ {} · {}", ep.provider_name, endpoint_detail)
                    }
                })
            })
            .unwrap_or_default();
        if supports_openrouter_provider_features {
            routes.push(build_openrouter_auto_route(
                &model,
                has_openrouter,
                auto_detail,
            ));
        } else {
            let (provider, api_method, detail) = openrouter
                .direct_openai_compatible_route_parts()
                .unwrap_or_else(|| {
                    (
                        "OpenAI-compatible".to_string(),
                        "openai-compatible".to_string(),
                        "custom endpoint".to_string(),
                    )
                });
            routes.push(ModelRoute {
                model: model.clone(),
                provider,
                api_method,
                available: has_openrouter,
                detail,
                cheapness: None,
            });
        }
        // Add per-provider routes from endpoints cache
        if supports_openrouter_provider_features && let Some((ref endpoints, _)) = cached {
            stats.endpoint_cache_hits += 1;
            let stale_suffix = age_str.as_deref().unwrap_or("");
            for ep in endpoints {
                stats.endpoint_routes += 1;
                routes.push(build_openrouter_endpoint_route(
                    &model,
                    ep,
                    has_openrouter,
                    Some(stale_suffix),
                ));
            }
        }
    }

    // A direct OpenAI-compatible runtime (NVIDIA NIM, Groq, etc.) shares the
    // OpenRouter/OpenAI-compatible transport, but it is a distinct profile
    // from standard OpenRouter. Keep standard OpenRouter's catalog scoped to
    // the `openrouter` cache namespace so `/model` can switch back to it
    // without relabeling OpenRouter models as the active direct profile.
    if !supports_openrouter_provider_features && standard_openrouter_profile_configured() {
        // The shared OpenRouter/OpenAI-compatible slot is occupied by a
        // direct profile (e.g. NVIDIA NIM), so standard OpenRouter is never
        // the active provider and its `openrouter` namespace catalog is
        // never refreshed by the normal active-provider path. Schedule a
        // background refresh whenever that cache is missing or stale so
        // models like `openrouter/owl-alpha` appear in `/model` on the next
        // picker render, and keep self-healing after upgrades (issue #292).
        // The scheduler is internally rate-limited and a no-op when the
        // cache is already fresh.
        openrouter::maybe_schedule_standard_openrouter_catalog_refresh(
            "inactive standard OpenRouter route hydration",
        );
        routes.extend(configured_standard_openrouter_profile_routes());
    }
}

/// Claude/OpenAI models reachable via OpenRouter as alternative routes.
fn append_openrouter_alternative_routes(
    routes: &mut Vec<ModelRoute>,
    stats: &mut OpenRouterRouteStats,
) {
    for model in known_anthropic_model_ids() {
        let or_model = format!("anthropic/{}", model);
        if let Some((endpoints, _)) = openrouter::load_endpoints_disk_cache_public(&or_model) {
            stats.endpoint_cache_hits += 1;
            for ep in &endpoints {
                stats.endpoint_routes += 1;
                routes.push(build_openrouter_endpoint_route(&model, ep, true, None));
            }
        } else if openrouter::standard_catalog_lists_model(&or_model) != Some(false) {
            routes.push(build_openrouter_fallback_provider_route(
                &model,
                &or_model,
                "Anthropic",
            ));
        }
    }

    for model in ALL_OPENAI_MODELS {
        let or_model = format!("openai/{}", model);
        if let Some((endpoints, _)) = openrouter::load_endpoints_disk_cache_public(&or_model) {
            stats.endpoint_cache_hits += 1;
            for ep in &endpoints {
                stats.endpoint_routes += 1;
                routes.push(build_openrouter_endpoint_route(model, ep, true, None));
            }
        } else if openrouter::standard_catalog_lists_model(&or_model) != Some(false) {
            // Skip fallback routes for models OpenRouter definitively does not
            // serve (e.g. openai/gpt-5.3-codex-spark) so the picker never
            // offers a route that would 400 at request time.
            routes.push(build_openrouter_fallback_provider_route(
                model, &or_model, "OpenAI",
            ));
        }
    }
}

/// Count routes per provider label (lowercased, spaces removed) so the catalog
/// summary log shows where the picker entries came from.
fn provider_route_counts(routes: &[ModelRoute]) -> std::collections::BTreeMap<String, usize> {
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for route in routes {
        let key = route.provider.trim().to_ascii_lowercase().replace(' ', "_");
        let key = if key.is_empty() {
            "unknown".to_string()
        } else {
            key
        };
        *counts.entry(key).or_insert(0) += 1;
    }
    counts
}

/// Emit a structured, non-secret summary of model-route building. Callers pass
/// the credential-detection flags they already computed so the log explains why
/// each provider's routes were or were not produced.
#[allow(clippy::too_many_arguments)]
fn log_model_routes_summary(
    phase: &str,
    routes: &[ModelRoute],
    routes_before_filter: usize,
    anthropic_oauth: bool,
    anthropic_api_key: bool,
    openai_oauth: bool,
    openai_api_key: bool,
    has_openrouter: bool,
    openrouter_provider_features: bool,
    direct_openai_compatible: bool,
    total_ms: u128,
) {
    let available = routes.iter().filter(|route| route.available).count();
    let per_provider = provider_route_counts(routes)
        .into_iter()
        .map(|(provider, count)| format!("{provider}:{count}"))
        .collect::<Vec<_>>()
        .join(",");

    crate::logging::event_info(
        "model_routes_summary",
        vec![
            ("phase", phase.to_string()),
            ("routes_total", routes.len().to_string()),
            ("routes_available", available.to_string()),
            ("routes_before_filter", routes_before_filter.to_string()),
            (
                "routes_dropped",
                routes_before_filter
                    .saturating_sub(routes.len())
                    .to_string(),
            ),
            ("anthropic_oauth", anthropic_oauth.to_string()),
            ("anthropic_api", anthropic_api_key.to_string()),
            ("openai_oauth", openai_oauth.to_string()),
            ("openai_api", openai_api_key.to_string()),
            ("openrouter_configured", has_openrouter.to_string()),
            (
                "openrouter_provider_features",
                openrouter_provider_features.to_string(),
            ),
            (
                "direct_openai_compatible",
                direct_openai_compatible.to_string(),
            ),
            ("by_provider", per_provider),
            ("build_ms", total_ms.to_string()),
        ],
    );
}

pub fn remote_model_routes_fallback(
    remote_provider_name: Option<&str>,
    remote_available_entries: &[String],
) -> Vec<ModelRoute> {
    let auth = AuthStatus::check_fast();
    let mut routes = Vec::new();
    for model in remote_available_entries {
        if !is_listable_model_name(model) {
            continue;
        }

        let openrouter_catalog_model = openrouter_catalog_model_id(model);
        let openrouter_cached = openrouter_catalog_model
            .as_deref()
            .and_then(openrouter::load_endpoints_disk_cache_public);

        if super::bedrock::BedrockProvider::is_bedrock_model_id(model) {
            let available = auth.bedrock != AuthState::NotConfigured
                || super::bedrock::BedrockProvider::has_credentials();
            routes.push(ModelRoute {
                model: model.clone(),
                provider: "AWS Bedrock".to_string(),
                api_method: "bedrock".to_string(),
                available,
                detail: if available {
                    String::new()
                } else {
                    "no Bedrock credentials or region; run /login bedrock".to_string()
                },
                cheapness: None,
            });
            continue;
        }

        if model.contains('/') {
            let cached = openrouter_cached;
            let auto_detail = cached
                .as_ref()
                .and_then(|(eps, _)| eps.first().map(|ep| format!("→ {}", ep.provider_name)))
                .unwrap_or_default();
            routes.push(build_openrouter_auto_route(
                model,
                auth.openrouter != AuthState::NotConfigured,
                auto_detail,
            ));
            if let Some((endpoints, age)) = cached {
                let age_str = if age < 3600 {
                    format!("{}m ago", age / 60)
                } else if age < 86400 {
                    format!("{}h ago", age / 3600)
                } else {
                    format!("{}d ago", age / 86400)
                };
                for ep in &endpoints {
                    routes.push(build_openrouter_endpoint_route(
                        model,
                        ep,
                        auth.openrouter != AuthState::NotConfigured,
                        Some(&age_str),
                    ));
                }
            }
            continue;
        }

        let mut added_any = false;

        if provider_for_model(model) == Some("claude") {
            if auth.anthropic.has_oauth {
                let (available, detail) = anthropic_oauth_route_availability(model);
                routes.push(build_anthropic_oauth_route(model, available, detail));
                added_any = true;
            }
            // An Anthropic API key is an equally valid direct route. Without
            // this, a model that only reaches the picker via the names-only
            // fallback path (e.g. a newly released model whose detailed route
            // frame was oversized) shows an OAuth route but silently loses its
            // API-key route even though the key works.
            if auth.anthropic.has_api_key {
                let (available, detail) = anthropic_api_key_route_availability(model);
                routes.push(ModelRoute {
                    model: model.clone(),
                    provider: "Anthropic".to_string(),
                    api_method: "claude-api".to_string(),
                    available,
                    detail,
                    cheapness: cheapness_for_route(model, "Anthropic", "claude-api"),
                });
                added_any = true;
            }
        }

        if jcode_provider_core::model_id::matches_known_model(model, ALL_OPENAI_MODELS) {
            let availability = model_availability_for_account(model);
            let (available, detail) = if auth.openai == AuthState::NotConfigured {
                (false, "no credentials".to_string())
            } else {
                match availability.state {
                    AccountModelAvailabilityState::Available => (true, String::new()),
                    AccountModelAvailabilityState::Unavailable => (
                        false,
                        format_account_model_availability_detail(&availability)
                            .unwrap_or_else(|| "not available".to_string()),
                    ),
                    AccountModelAvailabilityState::Unknown => (
                        true,
                        format_account_model_availability_detail(&availability)
                            .unwrap_or_else(|| "availability unknown".to_string()),
                    ),
                }
            };
            routes.push(build_openai_oauth_route(model, available, detail));
            added_any = true;
        }

        if auth.openrouter != AuthState::NotConfigured {
            let catalog_lists_model = openrouter_catalog_model
                .as_deref()
                .and_then(openrouter::standard_catalog_lists_model);
            match (provider_for_model(model), openrouter_cached.as_ref()) {
                (_, Some((endpoints, _age))) => {
                    for ep in endpoints {
                        routes.push(build_openrouter_endpoint_route(model, ep, true, None));
                    }
                    added_any = true;
                }
                // Skip fallback routes for models the OpenRouter catalog
                // definitively does not list (e.g. gpt-5.3-codex-spark).
                (Some("claude"), None) if catalog_lists_model != Some(false) => {
                    routes.push(build_openrouter_fallback_provider_route(
                        model,
                        openrouter_catalog_model.as_deref().unwrap_or(model),
                        "Anthropic",
                    ));
                    added_any = true;
                }
                (Some("openai"), None) if catalog_lists_model != Some(false) => {
                    routes.push(build_openrouter_fallback_provider_route(
                        model,
                        openrouter_catalog_model.as_deref().unwrap_or(model),
                        "OpenAI",
                    ));
                    added_any = true;
                }
                _ => {}
            }
        }

        if let Some(route) = remote_openai_compatible_route_for_model(model) {
            routes.push(route);
            added_any = true;
        }

        if !added_any
            && let Some(route) =
                remote_current_openai_compatible_route_for_model(remote_provider_name, model)
        {
            routes.push(route);
            added_any = true;
        }

        if !added_any && remote_model_should_offer_copilot_route(model) && !model.contains("[1m]") {
            routes.push(build_copilot_route(
                model,
                auth.copilot == AuthState::Available || remote_model_is_server_copilot_only(model),
                String::new(),
            ));
            added_any = true;
        }

        if super::gemini::is_gemini_model_id(model) {
            routes.push(ModelRoute {
                model: model.clone(),
                provider: "Gemini".to_string(),
                api_method: "code-assist-oauth".to_string(),
                available: auth.gemini == AuthState::Available,
                detail: String::new(),
                cheapness: None,
            });
            added_any = true;
        }

        if !added_any {
            routes.push(ModelRoute {
                model: model.clone(),
                provider: "unknown".to_string(),
                api_method: "unknown".to_string(),
                available: false,
                detail: "no matching configured provider route".to_string(),
                cheapness: None,
            });
        }
    }
    routes
}

pub fn remote_model_routes_lightweight_fallback(
    remote_provider_name: Option<&str>,
    remote_available_entries: &[String],
    current_model: &str,
) -> Vec<ModelRoute> {
    let provider = remote_provider_name
        .map(str::to_string)
        .unwrap_or_else(|| "remote".to_string());
    let mut routes = Vec::new();
    for model in remote_available_entries {
        if !is_listable_model_name(model) {
            continue;
        }
        routes.push(ModelRoute {
            model: model.clone(),
            provider: provider.clone(),
            api_method: "remote-catalog".to_string(),
            available: true,
            detail: "refreshing route details…".to_string(),
            cheapness: None,
        });
    }

    if routes.is_empty() && !current_model.is_empty() && current_model != "unknown" {
        routes.push(ModelRoute {
            model: current_model.to_string(),
            provider,
            api_method: "current".to_string(),
            available: true,
            detail: "refreshing model catalog…".to_string(),
            cheapness: None,
        });
    }

    routes
}

pub fn remote_current_openai_compatible_route_for_model(
    remote_provider_name: Option<&str>,
    model: &str,
) -> Option<ModelRoute> {
    if model.trim().is_empty() || model.contains('/') || provider_for_model(model).is_some() {
        return None;
    }

    let provider_name = remote_provider_name?.trim();
    let profile_id =
        crate::provider_catalog::openai_compatible_profile_id_for_display_name(provider_name)?;
    let profile = crate::provider_catalog::openai_compatible_profile_by_id(profile_id)?;
    if !crate::provider_catalog::openai_compatible_profile_is_configured(profile) {
        return None;
    }
    let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);

    Some(ModelRoute {
        model: model.to_string(),
        provider: resolved.display_name,
        api_method: format!("openai-compatible:{}", resolved.id),
        available: true,
        detail: resolved.api_base,
        cheapness: None,
    })
}

pub fn remote_model_should_offer_copilot_route(model: &str) -> bool {
    remote_openai_compatible_route_for_model(model).is_none()
        && (remote_model_is_server_copilot_only(model)
            || super::copilot::is_known_display_model(model))
}

pub fn remote_openai_compatible_route_for_model(model: &str) -> Option<ModelRoute> {
    for profile in crate::provider_catalog::openai_compatible_profiles()
        .iter()
        .copied()
    {
        if !crate::provider_catalog::openai_compatible_profile_is_configured(profile) {
            continue;
        }
        let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
        let Some(from_live_catalog) = remote_openai_compatible_profile_models(&resolved, profile)
            .iter()
            .find_map(|candidate| (candidate.0 == model).then_some(candidate.1))
        else {
            continue;
        };
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
        return Some(ModelRoute {
            model: model.to_string(),
            provider: resolved.display_name,
            api_method: format!("openai-compatible:{}", resolved.id),
            available: true,
            detail,
            cheapness: None,
        });
    }
    None
}

fn remote_openai_compatible_profile_models(
    resolved: &crate::provider_catalog::ResolvedOpenAiCompatibleProfile,
    profile: crate::provider_catalog::OpenAiCompatibleProfile,
) -> Vec<(String, bool)> {
    let mut models = Vec::new();
    let mut push = |model: String, from_live_catalog: bool| {
        let model = model.trim().to_string();
        if !model.is_empty() && !models.iter().any(|(existing, _)| existing == &model) {
            models.push((model, from_live_catalog));
        }
    };

    if let Some(cache) =
        jcode_provider_openrouter::load_disk_cache_entry_for_namespace(&resolved.id)
    {
        let source_matches = cache
            .source_api_base
            .as_deref()
            .and_then(crate::provider_catalog::normalize_api_base)
            == crate::provider_catalog::normalize_api_base(&resolved.api_base);
        if source_matches {
            for model in cache.models {
                push(model.id, true);
            }
        }
    }

    for model in crate::provider_catalog::openai_compatible_profile_static_models(profile) {
        push(model, false);
    }

    models
}

pub fn remote_model_is_server_copilot_only(model: &str) -> bool {
    !model.is_empty()
        && !model.contains('/')
        && remote_openai_compatible_route_for_model(model).is_none()
        && !matches!(
            provider_for_model(model),
            Some("claude" | "openai" | "gemini" | "cursor")
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{AuthState, ProviderAuth};

    struct EnvGuard {
        vars: Vec<(&'static str, Option<std::ffi::OsString>)>,
        _temp: tempfile::TempDir,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn new() -> Self {
            let lock = crate::storage::lock_test_env();
            let temp = tempfile::tempdir().expect("tempdir");
            let vars = vec![
                ("JCODE_HOME", std::env::var_os("JCODE_HOME")),
                ("OPENCODE_API_KEY", std::env::var_os("OPENCODE_API_KEY")),
            ];
            crate::env::set_var("JCODE_HOME", temp.path());
            crate::env::set_var("OPENCODE_API_KEY", "sk-test-opencode");
            Self {
                vars,
                _temp: temp,
                _lock: lock,
            }
        }

        fn save_opencode_cache(&self, source_api_base: &str, model_ids: &[&str]) {
            let jcode_home = std::env::var_os("JCODE_HOME").expect("JCODE_HOME set");
            let cache_dir = std::path::PathBuf::from(jcode_home).join("cache");
            std::fs::create_dir_all(&cache_dir).expect("create cache dir");
            let cache = jcode_provider_openrouter::DiskCache {
                cached_at: jcode_provider_openrouter::current_unix_secs()
                    .expect("current unix time"),
                source_api_base: Some(source_api_base.to_string()),
                models: model_ids
                    .iter()
                    .map(|id| jcode_provider_openrouter::ModelInfo {
                        id: (*id).to_string(),
                        name: String::new(),
                        context_length: None,
                        pricing: jcode_provider_openrouter::ModelPricing::default(),
                        created: None,
                    })
                    .collect(),
            };
            std::fs::write(
                cache_dir.join("opencode_models.json"),
                serde_json::to_string(&cache).expect("serialize cache"),
            )
            .expect("write cache");
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in self.vars.drain(..) {
                if let Some(value) = value {
                    crate::env::set_var(key, value);
                } else {
                    crate::env::remove_var(key);
                }
            }
        }
    }

    #[test]
    fn simplified_anthropic_routes_preserve_oauth_vs_api_key_state_space() {
        for (has_oauth, has_api_key, expected_methods) in [
            (true, false, vec!["claude-oauth"]),
            (false, true, vec!["claude-api"]),
            (true, true, vec!["claude-oauth", "claude-api"]),
            (false, false, vec!["claude-oauth"]),
        ] {
            let auth = AuthStatus {
                anthropic: ProviderAuth {
                    state: if has_oauth || has_api_key {
                        AuthState::Available
                    } else {
                        AuthState::NotConfigured
                    },
                    has_oauth,
                    oauth_state: if has_oauth {
                        AuthState::Available
                    } else {
                        AuthState::NotConfigured
                    },
                    has_api_key,
                },
                ..AuthStatus::default()
            };
            let mut routes = Vec::new();

            append_simplified_anthropic_model_routes(
                &mut routes,
                "claude-opus-4-6".to_string(),
                &auth,
            );

            let methods = routes
                .iter()
                .map(|route| route.api_method.as_str())
                .collect::<Vec<_>>();
            assert_eq!(
                methods, expected_methods,
                "oauth={has_oauth} api={has_api_key}"
            );
            assert!(routes.iter().all(|route| route.provider == "Anthropic"));
            assert_eq!(
                routes.iter().all(|route| route.available),
                has_oauth || has_api_key
            );
        }
    }

    #[test]
    fn remote_compatible_route_uses_live_cache_and_does_not_mark_fallback() {
        let guard = EnvGuard::new();
        guard.save_opencode_cache("https://opencode.ai/zen/v1", &["qwen3.6-plus"]);

        let route = remote_openai_compatible_route_for_model("qwen3.6-plus")
            .expect("live-cache-only OpenCode model should be routed");

        assert_eq!(route.provider, "OpenCode Zen");
        assert_eq!(route.api_method, "openai-compatible:opencode");
        assert_eq!(route.detail, "https://opencode.ai/zen/v1");
        assert!(!route.detail.contains("fallback"));
    }

    #[test]
    fn remote_compatible_route_marks_static_model_list_fallback() {
        let _guard = EnvGuard::new();

        let route = remote_openai_compatible_route_for_model("glm-4.7")
            .expect("static OpenCode fallback model should be routed");

        assert_eq!(route.provider, "OpenCode Zen");
        assert!(
            route
                .detail
                .contains("fallback: static provider model list")
        );
    }

    #[test]
    fn remote_compatible_route_ignores_live_cache_from_wrong_api_base() {
        let guard = EnvGuard::new();
        guard.save_opencode_cache("https://wrong.example.test/v1", &["qwen3.6-plus"]);

        assert!(remote_openai_compatible_route_for_model("qwen3.6-plus").is_none());
    }

    fn save_openrouter_catalog_cache(model_ids: &[&str]) {
        let jcode_home = std::env::var_os("JCODE_HOME").expect("JCODE_HOME set");
        let cache_dir = std::path::PathBuf::from(jcode_home).join("cache");
        std::fs::create_dir_all(&cache_dir).expect("create cache dir");
        let cache = jcode_provider_openrouter::DiskCache {
            cached_at: jcode_provider_openrouter::current_unix_secs().expect("current unix time"),
            source_api_base: None,
            models: model_ids
                .iter()
                .map(|id| jcode_provider_openrouter::ModelInfo {
                    id: (*id).to_string(),
                    name: String::new(),
                    context_length: None,
                    pricing: jcode_provider_openrouter::ModelPricing::default(),
                    created: None,
                })
                .collect(),
        };
        std::fs::write(
            cache_dir.join("openrouter_models.json"),
            serde_json::to_string(&cache).expect("serialize cache"),
        )
        .expect("write cache");
    }

    /// OpenRouter alternative routes must not be fabricated for models the
    /// OpenRouter catalog definitively does not list (e.g. the
    /// ChatGPT-exclusive `gpt-5.3-codex-spark`), while staying optimistic
    /// when no catalog cache exists yet.
    #[test]
    fn openrouter_alternative_routes_skip_models_absent_from_catalog() {
        let _guard = EnvGuard::new();

        // No catalog cache: optimistic, spark gets a fallback route.
        let mut routes = Vec::new();
        let mut stats = OpenRouterRouteStats::default();
        append_openrouter_alternative_routes(&mut routes, &mut stats);
        assert!(
            routes
                .iter()
                .any(|r| r.model == "gpt-5.3-codex-spark" && r.api_method == "openrouter"),
            "without a catalog cache the fallback route stays optimistic"
        );

        // Fresh catalog listing codex but not spark: spark route is dropped.
        save_openrouter_catalog_cache(&["openai/gpt-5.3-codex", "openai/gpt-5.5"]);
        let mut routes = Vec::new();
        let mut stats = OpenRouterRouteStats::default();
        append_openrouter_alternative_routes(&mut routes, &mut stats);
        assert!(
            !routes
                .iter()
                .any(|r| r.model == "gpt-5.3-codex-spark" && r.api_method == "openrouter"),
            "catalog-confirmed-absent model must not get an OpenRouter fallback route"
        );
        assert!(
            routes
                .iter()
                .any(|r| r.model == "gpt-5.3-codex" && r.api_method == "openrouter"),
            "catalog-listed model keeps its OpenRouter fallback route"
        );
    }
}
