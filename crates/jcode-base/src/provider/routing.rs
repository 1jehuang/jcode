pub(crate) fn anthropic_oauth_route_availability(model: &str) -> (bool, String) {
    if !anthropic_catalog_allows_route(model, true) {
        return (false, "not in OAuth model catalog".to_string());
    }
    if model.ends_with("[1m]") && !crate::usage::has_extra_usage() {
        (false, "requires extra usage".to_string())
    } else if model.contains("opus")
        && super::cached_anthropic_model_ids_for_scope(&super::anthropic_catalog_scope_for_route(
            true,
        ))
        .is_none()
        && !crate::auth::claude::is_max_subscription()
    {
        // Account-scoped OAuth discovery outranks the legacy plan-name
        // heuristic. Keep that heuristic only for the bundled fallback list.
        (false, "requires Max subscription".to_string())
    } else {
        (true, String::new())
    }
}

pub(crate) fn anthropic_api_key_route_availability(model: &str) -> (bool, String) {
    // Subscription extra usage is unrelated to API-key billing. The API
    // catalog and request-time access errors determine API model access.
    if anthropic_catalog_allows_route(model, false) {
        (true, String::new())
    } else {
        (false, "not in API-key model catalog".to_string())
    }
}

fn anthropic_catalog_allows_route(model: &str, oauth: bool) -> bool {
    let scope = super::anthropic_catalog_scope_for_route(oauth);
    super::known_anthropic_model_ids_for_scope(&scope)
        .iter()
        .any(|id| id == model)
}
