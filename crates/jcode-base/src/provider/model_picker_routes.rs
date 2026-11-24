//! Effective native picker routes, shared by TUI discovery and model control.

use std::collections::HashSet;

fn append_subscription_routes(
    remote_available_entries: &[String],
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
                    || remote_available_entries.iter().any(|available| {
                        crate::subscription_catalog::canonical_model_id(available) == Some(model.id)
                    }))
        })
    {
        routes.push(crate::provider::ModelRoute {
            catalog_metadata: None,
            model: model.id.to_string(),
            provider: crate::subscription_catalog::JCODE_PROVIDER_DISPLAY_NAME.to_string(),
            api_method: crate::subscription_catalog::JCODE_ROUTE_API_METHOD.to_string(),
            available: true,
            detail: crate::subscription_catalog::routing_policy_detail(model),
            usage: None,
            cheapness: None,
        });
    }
}

/// Add native fallback routes and subscription entitlements on the provider host.
pub fn extend_model_picker_routes(
    remote_provider_name: Option<&str>,
    remote_available_entries: &[String],
    routes: &mut Vec<crate::provider::ModelRoute>,
) {
    if remote_available_entries.is_empty() {
        // The SSH daemon's routes are authoritative. Never infer routes
        // from this computer's credentials, profiles, or model caches.
        return;
    }
    // Jcode subscription routes are a complete, server-managed catalog.
    // Do not mix in locally configured Anthropic/OpenAI credentials merely
    // because a curated model also belongs to one of those upstreams.
    let provider_is_jcode_subscription = remote_provider_name.is_some_and(|name| {
        name.eq_ignore_ascii_case(crate::subscription_catalog::JCODE_PROVIDER_DISPLAY_NAME)
    });
    if provider_is_jcode_subscription {
        routes.clear();
        append_subscription_routes(remote_available_entries, routes, false, false);
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
            remote_provider_name,
            remote_available_entries,
        );
        append_subscription_routes(remote_available_entries, routes, false, true);
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
    let missing: Vec<String> = remote_available_entries
        .iter()
        .filter(|model| match methods_by_model.get(model.as_str()) {
            None => true,
            Some(methods) => {
                // Poisoned caches pin models to placeholder rows forever;
                // see inline_interactive_placeholder_routes.rs.
                let placeholder_only = methods.iter().all(|method| {
                    matches!(
                        crate::provider::ModelRouteApiMethod::parse(method),
                        crate::provider::ModelRouteApiMethod::RemoteCatalog
                            | crate::provider::ModelRouteApiMethod::Current
                    )
                });
                let missing_anthropic_method = crate::provider::provider_for_model(model)
                    == Some("claude")
                    && !model.contains('/')
                    && ((auth.anthropic.has_api_key && !methods.contains("claude-api"))
                        || (auth.anthropic.has_oauth && !methods.contains("claude-oauth")));
                let missing_bedrock_method = bedrock_available
                    && crate::provider::bedrock::BedrockProvider::is_bedrock_model_id(model)
                    && !methods.contains("bedrock");
                placeholder_only || missing_anthropic_method || missing_bedrock_method
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
        for route in crate::provider::remote_model_routes_fallback(remote_provider_name, &missing) {
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
    append_subscription_routes(remote_available_entries, routes, true, false);
}
