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

        let openrouter_catalog_model = openrouter_catalog_model_id(model);
        let openrouter_cached = openrouter_catalog_model
            .as_deref()
            .and_then(openrouter::load_endpoints_disk_cache_public);

        if crate::provider::bedrock::BedrockProvider::is_bedrock_model_id(model) {
            let available = auth.bedrock != AuthState::NotConfigured
                || crate::provider::bedrock::BedrockProvider::has_credentials();
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
        routes.push(ModelRoute {
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

    if routes.is_empty() && !current_model.is_empty() && current_model != "unknown" {
        routes.push(ModelRoute {
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
        if let Some(route) = named_provider_profile_routes(profile_name, profile_config)
            .into_iter()
            .find(|route| route.model == model)
        {
            return Some(route);
        }
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

/// Effort ladder the client UI should offer when the session runs a named
/// provider profile with per-model reasoning configuration (`reasoning_map`
/// or the `reasoning` override).
///
/// Remote TUI sessions cannot ask the server-side provider for
/// `available_efforts()`: they only know the provider display name and the
/// model id. When that identity resolves to a configured named profile whose
/// model has a `reasoning_map`, the map is authoritative for the ladder
/// (enabled rungs in ladder order plus the swarm sentinels), so this returns
/// it. A model with `reasoning = true` but no map gets the server's OpenAI
/// ladder (see `supports_openai_reasoning_effort`), so the same ladder is
/// returned here to keep the client picker and `/effort` in sync with what
/// the server actually accepts. Returns `None` when the identity matches no
/// per-model reasoning configuration, letting callers fall back to the
/// built-in family inference.
///
/// The provider name is matched against both the profile key and the
/// profile's user-configured `display_name`, because the server reports
/// `display_name()` as the provider name for named profiles.
pub fn configured_reasoning_efforts(
    provider_name: Option<&str>,
    model_name: Option<&str>,
) -> Option<Vec<String>> {
    configured_reasoning_efforts_in(
        provider_name,
        model_name,
        &crate::config::config().providers,
    )
}

pub(super) fn configured_reasoning_efforts_in(
    provider_name: Option<&str>,
    model_name: Option<&str>,
    providers: &std::collections::BTreeMap<String, crate::config::NamedProviderConfig>,
) -> Option<Vec<String>> {
    let provider = provider_name?.trim();
    let model = model_name?.trim();
    if provider.is_empty() || model.is_empty() {
        return None;
    }
    let provider_lower = provider.to_ascii_lowercase();
    let model_lower = model.to_ascii_lowercase();
    for (profile_name, profile_config) in providers {
        let profile_label = profile_config
            .display_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(profile_name);
        if profile_name.to_ascii_lowercase() != provider_lower
            && profile_label.to_ascii_lowercase() != provider_lower
        {
            continue;
        }
        let model_config = profile_config
            .models
            .iter()
            .find(|entry| entry.id.trim().to_ascii_lowercase() == model_lower)?;
        let map = model_config.reasoning_map.as_ref();
        if let Some(map) = map.filter(|map| !map.is_empty()) {
            let mut efforts: Vec<String> = map
                .rungs()
                .filter(|(_, rung)| !rung.disabled)
                .map(|(key, _)| key.to_string())
                .collect();
            if efforts.is_empty() {
                return None;
            }
            efforts.extend(
                jcode_provider_core::SWARM_EFFORTS
                    .iter()
                    .map(|s| s.to_string()),
            );
            return Some(efforts);
        }
        // No map: the per-model `reasoning` override still enables effort on
        // the server. `reasoning = true` on a non-DeepSeek model gets the
        // OpenAI ladder there (DeepSeek-family ids keep the DeepSeek ladder,
        // matching `supports_deepseek_reasoning_effort`); mirror it so the
        // client offers the same rungs and does not report the effort as
        // fixed. The deep-swarm sentinel is only appended when the shared
        // ladder still carries it.
        if model_config.reasoning == Some(true) {
            let deepseek_family = model_lower.contains("deepseek");
            let base = if deepseek_family {
                jcode_provider_core::DEEPSEEK_SELECTABLE_EFFORTS
            } else {
                jcode_provider_core::OPENAI_SELECTABLE_EFFORTS
            };
            let mut efforts: Vec<String> = base.iter().map(|s| s.to_string()).collect();
            for sentinel in jcode_provider_core::SWARM_EFFORTS {
                if !efforts.iter().any(|effort| effort == *sentinel) {
                    efforts.push(sentinel.to_string());
                }
            }
            return Some(efforts);
        }
        return None;
    }
    None
}
