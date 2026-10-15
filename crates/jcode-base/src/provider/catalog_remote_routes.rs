use super::*;

pub fn remote_model_routes_fallback(
    remote_provider_name: Option<&str>,
    remote_available_entries: &[String],
) -> Vec<ModelRoute> {
    if remote_provider_name.is_some_and(|name| {
        name.eq_ignore_ascii_case(crate::subscription_catalog::JCODE_PROVIDER_DISPLAY_NAME)
    }) {
        return remote_available_entries
            .iter()
            .filter(|model| is_listable_model_name(model))
            .map(|model| ModelRoute {
                catalog_metadata: None,
                model: model.clone(),
                provider: crate::subscription_catalog::JCODE_PROVIDER_DISPLAY_NAME.to_string(),
                api_method: crate::subscription_catalog::JCODE_ROUTE_API_METHOD.to_string(),
                available: true,
                detail: "jcode subscription routing · managed server-side".to_string(),
                usage: None,
                cheapness: None,
            })
            .collect();
    }

    let auth = AuthStatus::check_fast();
    let mut routes = Vec::new();
    for model in remote_available_entries {
        if !is_listable_model_name(model) {
            continue;
        }

        if jcode_provider_core::is_chatgpt_web_model(model) {
            routes.push(chatgpt_web_route_for_model(model));
            continue;
        }

        let openrouter_catalog_model = openrouter_catalog_model_id(model);
        let openrouter_cached = openrouter_catalog_model
            .as_deref()
            .and_then(openrouter::load_endpoints_disk_cache_public);

        if crate::provider::bedrock::BedrockProvider::is_bedrock_model_id(model) {
            let available = auth.bedrock != AuthState::NotConfigured
                || crate::provider::bedrock::BedrockProvider::has_credentials();
            routes.push(ModelRoute {
                catalog_metadata: None,
                model: model.clone(),
                provider: "AWS Bedrock".to_string(),
                api_method: "bedrock".to_string(),
                available,
                detail: if available {
                    String::new()
                } else {
                    "no Bedrock credentials or region; run /login bedrock".to_string()
                },
                usage: None,
                cheapness: None,
            });
            continue;
        }

        if model.contains('/')
            && let Some(route) = remote_openai_compatible_route_for_model(model)
        {
            routes.push(route);
            continue;
        }

        if model.contains('/') {
            let cached = openrouter_cached;
            // No cached endpoint means there is no routing annotation yet.
            let auto_detail = match cached.as_ref().and_then(|(eps, _)| eps.first()) {
                Some(endpoint) => format!("→ {}", endpoint.provider_name),
                None => String::new(),
            };
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
            append_simplified_anthropic_model_routes(&mut routes, model.clone(), &auth);
            added_any = true;
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

        if crate::provider::gemini::is_gemini_model_id(model) {
            routes.push(ModelRoute {
                catalog_metadata: None,
                model: model.clone(),
                provider: "Gemini".to_string(),
                api_method: "code-assist-oauth".to_string(),
                available: auth.gemini == AuthState::Available,
                detail: String::new(),
                usage: None,
                cheapness: None,
            });
            added_any = true;
        }

        if !added_any {
            routes.push(ModelRoute {
                catalog_metadata: None,
                model: model.clone(),
                provider: "unknown".to_string(),
                api_method: "unknown".to_string(),
                available: false,
                detail: "no matching configured provider route".to_string(),
                usage: None,
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
    let is_jcode_subscription = remote_provider_name.is_some_and(|name| {
        name.eq_ignore_ascii_case(crate::subscription_catalog::JCODE_PROVIDER_DISPLAY_NAME)
    });
    let provider = remote_provider_name
        .map(str::to_string)
        .unwrap_or_else(|| "remote".to_string());
    let mut routes = Vec::new();
    for model in remote_available_entries {
        if !is_listable_model_name(model) {
            continue;
        }
        if jcode_provider_core::is_chatgpt_web_model(model) {
            routes.push(chatgpt_web_route_for_model(model));
            continue;
        }
        routes.push(ModelRoute {
            catalog_metadata: None,
            model: model.clone(),
            provider: provider.clone(),
            api_method: if is_jcode_subscription {
                crate::subscription_catalog::JCODE_ROUTE_API_METHOD.to_string()
            } else {
                "remote-catalog".to_string()
            },
            available: true,
            detail: if is_jcode_subscription {
                "jcode subscription routing · managed server-side".to_string()
            } else {
                "refreshing route details…".to_string()
            },
            usage: None,
            cheapness: None,
        });
    }

    if routes.is_empty() && jcode_provider_core::is_chatgpt_web_model(current_model) {
        routes.push(chatgpt_web_route_for_model(current_model));
    }
    if routes.is_empty() && !current_model.is_empty() && current_model != "unknown" {
        routes.push(ModelRoute {
            catalog_metadata: None,
            model: current_model.to_string(),
            provider,
            api_method: "current".to_string(),
            available: true,
            detail: "refreshing model catalog…".to_string(),
            usage: None,
            cheapness: None,
        });
    }

    routes
}

pub fn remote_current_openai_compatible_route_for_model(
    remote_provider_name: Option<&str>,
    model: &str,
) -> Option<ModelRoute> {
    if model.trim().is_empty() || (!model.contains('/') && provider_for_model(model).is_some()) {
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
    if model.contains('/')
        && !remote_openai_compatible_profile_models(&resolved, profile)
            .iter()
            .any(|candidate| candidate.0 == model)
    {
        return None;
    }

    Some(ModelRoute {
        catalog_metadata: None,
        model: model.to_string(),
        provider: resolved.display_name,
        api_method: format!("openai-compatible:{}", resolved.id),
        available: true,
        detail: resolved.api_base,
        usage: None,
        cheapness: None,
    })
}

pub fn remote_model_should_offer_copilot_route(model: &str) -> bool {
    remote_openai_compatible_route_for_model(model).is_none()
        && (remote_model_is_server_copilot_only(model)
            || crate::provider::copilot::is_known_display_model(model))
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
            catalog_metadata: None,
            model: model.to_string(),
            provider: resolved.display_name,
            api_method: format!("openai-compatible:{}", resolved.id),
            available: true,
            detail,
            usage: None,
            cheapness: None,
        });
    }
    named_provider_profile_route_for_model(model)
}

/// Route for `model` when it belongs to a user-defined `[providers.<name>]`
/// profile from config.toml.
///
/// Built-in OpenAI-compatible profiles are handled above; without this, a
/// bare model id from a custom profile (e.g. a local MLX server) matches no
/// known provider and falls through to the Copilot heuristic, which then
/// labels it `Copilot` and builds a `copilot:<model>` id that no runtime can
/// resolve (issue #694).
pub(super) fn named_provider_profile_route_for_model(model: &str) -> Option<ModelRoute> {
    named_provider_profile_route_for_model_in(model, &crate::config::config().providers)
}

pub(super) fn named_provider_profile_route_for_model_in(
    model: &str,
    providers: &std::collections::BTreeMap<String, crate::config::NamedProviderConfig>,
) -> Option<ModelRoute> {
    let model = model.trim();
    if model.is_empty() {
        return None;
    }
    for (profile_name, profile_config) in providers {
        if !named_provider_profile_routes(profile_name, profile_config)
            .iter()
            .any(|route| route.model == model)
        {
            continue;
        }
        let detail = if profile_config.base_url.trim().is_empty() {
            "configured provider profile".to_string()
        } else {
            profile_config.base_url.trim().to_string()
        };
        return Some(ModelRoute {
            catalog_metadata: None,
            model: model.to_string(),
            provider: profile_name.clone(),
            api_method: format!("openai-compatible:{}", profile_name),
            available: true,
            detail,
            usage: None,
            cheapness: None,
        });
    }
    None
}

pub(super) fn remote_openai_compatible_profile_models(
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
