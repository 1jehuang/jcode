use crate::auth::{AuthState, AuthStatus};

use super::pricing::cheapness_for_route;
use super::{
    ALL_OPENAI_MODELS, AccountModelAvailabilityState, ModelRoute, MultiProvider, Provider,
    anthropic_api_key_route_availability, anthropic_oauth_route_availability, bedrock,
    build_anthropic_oauth_route, build_copilot_route, build_openai_api_key_route,
    build_openai_oauth_route, build_openrouter_auto_route, build_openrouter_endpoint_route,
    build_openrouter_fallback_provider_route, configured_standard_openrouter_profile_routes,
    copilot, dedupe_model_routes, direct_openai_compatible_profile_routes,
    format_account_model_availability_detail, is_listable_model_name, known_anthropic_model_ids,
    known_openai_model_ids, model_availability_for_account, openrouter,
    openrouter_catalog_model_id, provider_for_model, standard_openrouter_profile_configured,
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

pub(super) fn multiprovider_model_routes(provider: &MultiProvider) -> Vec<ModelRoute> {
    let routes_started = std::time::Instant::now();
    provider.spawn_anthropic_catalog_refresh_if_needed();
    provider.spawn_openai_catalog_refresh_if_needed();

    let mut routes = Vec::new();
    let mut openrouter_models = 0usize;
    let mut openrouter_endpoint_cache_hits = 0usize;
    let mut openrouter_endpoint_routes = 0usize;
    let mut openrouter_scheduled_endpoint_refreshes = 0usize;
    let has_oauth = provider.has_claude_runtime();
    let has_api_key = crate::provider_catalog::load_api_key_from_env_or_config(
        "ANTHROPIC_API_KEY",
        "anthropic.env",
    )
    .is_some();
    let anthropic_models = if let Some(anthropic) = provider.anthropic_provider() {
        anthropic.available_models_for_switching()
    } else if let Some(claude) = provider.claude_provider() {
        claude.available_models_for_switching()
    } else {
        known_anthropic_model_ids()
    };
    let openai_models = if let Some(openai) = provider.openai_provider() {
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

    // OpenAI models
    let openai_auth = crate::auth::AuthStatus::check_fast();
    for model in openai_models {
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

    let active_direct_openai_compatible_api_method = provider
        .openrouter_provider()
        .and_then(|openrouter| openrouter.direct_openai_compatible_route_parts())
        .map(|(_, api_method, _)| api_method);
    let mut added_direct_openai_compatible_routes = false;
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
        added_direct_openai_compatible_routes |= !profile_routes.is_empty();
        routes.extend(profile_routes);
    }

    // GitHub Copilot models
    {
        if let Some(copilot) = provider.copilot_provider() {
            let copilot_models = copilot.available_models_display();
            let detail = copilot.model_catalog_detail();
            let copilot_models_empty = copilot_models.is_empty();
            for model in copilot_models {
                routes.push(build_copilot_route(&model, true, detail.clone()));
            }
            if copilot_models_empty && copilot::CopilotApiProvider::has_credentials() {
                routes.push(build_copilot_route("copilot models", false, detail));
            }
        } else if copilot::CopilotApiProvider::has_credentials() {
            routes.push(build_copilot_route(
                "copilot models",
                false,
                "not initialized yet",
            ));
        }
    }

    // Gemini models
    {
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

    // Antigravity models
    {
        if let Some(antigravity) = provider.antigravity_provider() {
            routes.extend(antigravity.model_routes());
        }
    }

    // Cursor models
    {
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

    // AWS Bedrock models and inference profiles
    {
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

    // OpenRouter models (with per-provider endpoints)
    let openrouter_provider = provider.openrouter_provider();
    let has_openrouter = openrouter_provider.is_some();
    let has_openrouter_provider_features = openrouter_provider
        .as_ref()
        .map(|openrouter| openrouter.supports_provider_routing_features())
        .unwrap_or(false);
    if let Some(openrouter) = openrouter_provider {
        let current_openrouter_model = openrouter.model();
        let supports_openrouter_provider_features = openrouter.supports_provider_routing_features();
        let mut scheduled_endpoint_refreshes = 0usize;
        for model in openrouter.available_models_display() {
            openrouter_models += 1;
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
                openrouter_scheduled_endpoint_refreshes += 1;
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
                openrouter_endpoint_cache_hits += 1;
                let stale_suffix = age_str.as_deref().unwrap_or("");
                for ep in endpoints {
                    openrouter_endpoint_routes += 1;
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
            routes.extend(configured_standard_openrouter_profile_routes());
        }
    }

    if !has_openrouter && !added_direct_openai_compatible_routes {
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
    if has_openrouter_provider_features {
        for model in known_anthropic_model_ids() {
            let or_model = format!("anthropic/{}", model);
            if let Some((endpoints, _)) = openrouter::load_endpoints_disk_cache_public(&or_model) {
                openrouter_endpoint_cache_hits += 1;
                for ep in &endpoints {
                    openrouter_endpoint_routes += 1;
                    routes.push(build_openrouter_endpoint_route(&model, ep, true, None));
                }
            } else {
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
                openrouter_endpoint_cache_hits += 1;
                for ep in &endpoints {
                    openrouter_endpoint_routes += 1;
                    routes.push(build_openrouter_endpoint_route(model, ep, true, None));
                }
            } else {
                routes.push(build_openrouter_fallback_provider_route(
                    model, &or_model, "OpenAI",
                ));
            }
        }
    }

    let total_ms = routes_started.elapsed().as_millis();
    if total_ms >= 250 || std::env::var("JCODE_LOG_MODEL_PICKER_TIMING").is_ok() {
        crate::logging::info(&format!(
            "[TIMING] model_routes: routes={}, openrouter_configured={}, openrouter_models={}, openrouter_endpoint_cache_hits={}, openrouter_endpoint_routes={}, openrouter_scheduled_endpoint_refreshes={}, total={}ms",
            routes.len(),
            has_openrouter,
            openrouter_models,
            openrouter_endpoint_cache_hits,
            openrouter_endpoint_routes,
            openrouter_scheduled_endpoint_refreshes,
            total_ms,
        ));
    }

    dedupe_model_routes(routes)
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

        if provider_for_model(model) == Some("claude") && auth.anthropic.has_oauth {
            let (available, detail) = anthropic_oauth_route_availability(model);
            routes.push(build_anthropic_oauth_route(model, available, detail));
            added_any = true;
        }

        if ALL_OPENAI_MODELS.contains(&model.as_str()) {
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
            match (provider_for_model(model), openrouter_cached.as_ref()) {
                (_, Some((endpoints, _age))) => {
                    for ep in endpoints {
                        routes.push(build_openrouter_endpoint_route(model, ep, true, None));
                    }
                    added_any = true;
                }
                (Some("claude"), None) => {
                    routes.push(build_openrouter_fallback_provider_route(
                        model,
                        openrouter_catalog_model.as_deref().unwrap_or(model),
                        "Anthropic",
                    ));
                    added_any = true;
                }
                (Some("openai"), None) => {
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
}
