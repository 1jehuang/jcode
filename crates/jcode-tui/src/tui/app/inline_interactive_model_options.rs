//! Read the native picker choices without installing a view or replacing a draft.

use super::{
    App, ModelPickerBuildOutcome, model_effort_step_preselection, model_entry_base_name,
    model_route_effort_levels, picker_route_selection, reasoning_effort_picker_label,
    saved_default_reasoning_effort,
};

impl App {
    /// The api_method string of the route currently in use, used to exclude the
    /// failed route and to recognize same-model/different-method alternatives.
    pub(in crate::tui::app) fn current_route_api_method(&self) -> Option<String> {
        if self.is_remote
            && let Some(route) = &self.remote_model_route
        {
            return Some(route.api_method.clone());
        }
        // Session route metadata records a past selection, not necessarily the
        // credential now in use (e.g. after /accounts or a remote route switch).
        // Prefer the same authoritative credential that drives billing identity
        // so we never offer the active route as its own fallback.
        let provider =
            jcode_provider_core::parse_provider_hint(&self.current_provider_label_for_fallback());
        let credential = if self.is_remote {
            self.remote_resolved_credential
        } else {
            self.provider.active_resolved_credential()
        };
        use jcode_provider_core::{ActiveProvider, ResolvedCredential};
        let account_method = |base: &str| {
            if !self.is_remote
                && let Some(label) = self.provider.oauth_account_label()
            {
                return format!("{base}@{label}");
            }
            if let Some(method) = self.session.route_api_method.as_deref()
                && method
                    .split_once('@')
                    .is_some_and(|(prefix, _)| prefix == base)
                && jcode_provider_core::oauth_account_label(method).is_some()
            {
                return method.to_string();
            }
            let label = if self.is_remote {
                None
            } else if base == "claude-oauth" {
                crate::auth::claude::active_account_label()
            } else {
                crate::auth::codex::active_account_label()
            };
            label
                .map(|label| format!("{base}@{label}"))
                .unwrap_or_else(|| base.to_string())
        };
        match (provider, credential) {
            (Some(ActiveProvider::Claude), Some(ResolvedCredential::Oauth)) => {
                Some(account_method("claude-oauth"))
            }
            (Some(ActiveProvider::Claude), Some(ResolvedCredential::ApiKey)) => {
                Some("claude-api".to_string())
            }
            (Some(ActiveProvider::OpenAI), Some(ResolvedCredential::Oauth)) => {
                Some(account_method("openai-oauth"))
            }
            (Some(ActiveProvider::OpenAI), Some(ResolvedCredential::ApiKey)) => {
                Some("openai-api".to_string())
            }
            _ => self.session.route_api_method.clone(),
        }
    }

    pub(in crate::tui::app) fn debug_model_options_json(&mut self) -> String {
        if self.is_remote
            && self.remote_startup_phase.is_none()
            && self.remote_model_options.is_empty()
        {
            self.hydrate_remote_model_catalog_cache();
        }
        let context = self.model_picker_build_context();
        let current_model = context.current_model.clone();
        let effort = if self.is_remote {
            self.remote_reasoning_effort.clone()
        } else {
            self.provider.reasoning_effort()
        };
        let mut routes = if self.is_remote {
            self.remote_model_options.clone()
        } else if crate::perf::tui_policy().simplified_model_picker {
            self.simplified_model_routes_for_picker(&current_model)
        } else {
            self.provider.model_routes()
        };
        if self.is_remote {
            self.extend_remote_routes_for_uncovered_models(&mut routes);
        }
        if self.auth_catalog_refresh_pending
            || (self.is_remote
                && self.remote_startup_phase.is_some()
                && self.remote_model_options.is_empty()
                && self.remote_available_entries.is_empty())
        {
            routes.clear();
        }
        let entries =
            match Self::build_model_picker_entries(context, routes, 0, std::time::Instant::now()) {
                ModelPickerBuildOutcome::Built { entries, .. } => entries,
                ModelPickerBuildOutcome::Empty => Vec::new(),
            };
        let any_current = entries.iter().any(|entry| entry.is_current);
        let mut choices = Vec::new();
        for entry in &entries {
            for route in &entry.options {
                if super::placeholder_routes::is_placeholder_route_method(&route.api_method) {
                    continue;
                }
                let selection = picker_route_selection(entry, route);
                let model = model_entry_base_name(entry);
                let levels = model_route_effort_levels(&model, route);
                let saved = saved_default_reasoning_effort(&entry.name, route);
                let current = entry.is_current || (!any_current && entry.name == current_model);
                let selected = model_effort_step_preselection(
                    &levels,
                    effort.as_deref().filter(|_| current),
                    saved.as_deref(),
                );
                let reasoning_levels: Vec<_> = levels
                    .iter()
                    .map(|level| {
                        let label = (route.api_method == "chatgpt-web")
                            .then(|| super::super::chatgpt_web_effort_label(&model, level))
                            .flatten()
                            .unwrap_or_else(|| reasoning_effort_picker_label(level).to_string());
                        serde_json::json!({ "value": level, "label": label })
                    })
                    .collect();
                choices.push(serde_json::json!({
                    "id": selection.picker_choice_id(),
                    "name": entry.name, "provider": route.provider, "api_method": route.api_method,
                    "available": route.available, "detail": route.detail, "selection": selection,
                    "reasoning_effort": levels.get(selected), "reasoning_levels": reasoning_levels,
                }));
            }
        }
        serde_json::json!({ "choices": choices, "client_pid": std::process::id(), "session_id": self.active_client_session_id() }).to_string()
    }
}
