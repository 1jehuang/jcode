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

impl super::MultiProvider {
    pub(super) fn set_unprefixed_model(&self, model: &str) -> anyhow::Result<()> {
        if self.active_provider() == super::ActiveProvider::Antigravity
            && crate::provider::antigravity::is_known_model(model)
        {
            return self.set_model_on_provider(super::ActiveProvider::Antigravity, model);
        }

        // Detect which provider an unprefixed model belongs to.
        let target_provider = super::provider_for_model(model);
        if let Some(target_provider) = target_provider
            && let Some(target) = super::provider_from_model_key(target_provider)
        {
            if !self.reconcile_auth_if_provider_missing(target)
                && self.antigravity_provider().is_some()
                && crate::provider::antigravity::is_known_model(model)
            {
                return self.set_model_on_provider(super::ActiveProvider::Antigravity, model);
            }
            self.set_model_on_provider(target, model)
        } else if let Some(profile) = self.openai_compatible_profile_owning_model(model) {
            // Bare ids from an OpenAI-compatible catalog (`celeris-1`,
            // `mimo-v2.5`, ...) match none of the built-in model-name
            // heuristics. Without this, `/model <bare-id>` fell through to
            // whichever provider happened to be active and failed with a
            // misleading "not supported by <active provider>" error.
            self.set_model_on_openai_compatible_profile(profile, model)
        } else {
            // Unknown model - try current provider.
            self.set_model_on_provider(self.active_provider(), model)
        }
    }
}
