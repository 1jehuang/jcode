//! ACP model and reasoning effort configuration presentation.

use super::*;

pub(super) const CONFIG_ID_MODEL: &str = "model";
pub(super) const CONFIG_ID_EFFORT: &str = "reasoning_effort";

fn route_id(selection: &crate::provider::RouteSelection) -> String {
    use sha2::{Digest, Sha256};
    // Catalog detail/usage can change without changing the route identity.
    let identity = serde_json::to_vec(&(
        &selection.model,
        &selection.runtime_key,
        &selection.api_method,
        &selection.provider_label,
    ))
    .expect("route identity serializes");
    format!("jcode-route:{:x}", Sha256::digest(identity))
}

impl SessionUiState {
    pub(super) fn current_model_id(&self) -> Option<String> {
        let model = self.model.as_ref()?;
        if let Some(selection) = &self.selected_route {
            return Some(route_id(selection));
        }
        let mut candidates = self.available_model_routes.iter().filter(|route| {
            let selection = crate::provider::RouteSelection::from_model_route(route);
            let routed = selection.routed_model_spec();
            (routed != route.model && routed == *model)
                || (route.model == *model
                    && self
                        .provider_name
                        .as_deref()
                        .is_some_and(|provider| route.provider.eq_ignore_ascii_case(provider)))
        });
        let first = candidates.next();
        if candidates.next().is_none() {
            if let Some(route) = first {
                return Some(route_id(
                    &crate::provider::RouteSelection::from_model_route(route),
                ));
            }
        }
        Some(model.clone())
    }

    pub(super) fn model_choices(&self) -> Vec<(String, String)> {
        let mut choices = Vec::new();
        for route in self
            .available_model_routes
            .iter()
            .filter(|route| route.available)
        {
            let selection = crate::provider::RouteSelection::from_model_route(route);
            let id = route_id(&selection);
            if !choices.iter().any(|(existing, _)| existing == &id) {
                choices.push((
                    id,
                    format!("{} ({}, {})", route.model, route.provider, route.api_method),
                ));
            }
        }
        for model in &self.available_models {
            if !self
                .available_model_routes
                .iter()
                .any(|route| &route.model == model)
                && !choices.iter().any(|(id, _)| id == model)
            {
                choices.push((model.clone(), model.clone()));
            }
        }
        if let Some(current) = self.current_model_id() {
            if !choices.iter().any(|(id, _)| id == &current) {
                let name = self.selected_route.as_ref().map_or_else(
                    || self.model.clone().unwrap_or_default(),
                    |route| {
                        format!(
                            "{} ({}, {})",
                            route.model, route.provider_label, route.api_method
                        )
                    },
                );
                choices.insert(0, (current, name));
            }
        }
        choices
    }

    pub(super) fn model_request(&self, id: u64, value: &str) -> Result<Request> {
        let selection = self
            .available_model_routes
            .iter()
            .filter(|route| route.available)
            .map(crate::provider::RouteSelection::from_model_route)
            .chain(self.selected_route.clone())
            .find(|selection| route_id(selection) == value);
        if let Some(selection) = selection {
            return Ok(Request::SetRoute {
                id,
                selection,
                automatic_failover: false,
            });
        }
        if value.starts_with("jcode-route:") {
            anyhow::bail!("Unknown or expired model route. Refresh /models and select again.");
        }
        Ok(Request::SetModel {
            id,
            model: value.to_string(),
        })
    }
}

pub(super) fn acp_available_commands() -> Vec<Value> {
    vec![
        json!({
            "name": "model",
            "description": "Switch the model for this session, or show the current model",
            "input": { "hint": "model id (optional)" },
        }),
        json!({
            "name": "models",
            "description": "List models available from the active provider",
        }),
        json!({
            "name": "effort",
            "description": "Set reasoning effort, or show the current effort",
            "input": { "hint": "none|minimal|low|medium|high|xhigh|max (optional)" },
        }),
    ]
}

pub(super) fn insert_session_configuration(result: &mut Value, state: &SessionUiState) {
    let Some(object) = result.as_object_mut() else {
        return;
    };
    let config_options = session_config_options(state);
    if !config_options.is_empty() {
        object.insert("configOptions".to_string(), Value::Array(config_options));
    }
    if let Some(models) = session_models(state) {
        object.insert("models".to_string(), models);
    }
}

pub(super) fn session_models(state: &SessionUiState) -> Option<Value> {
    let current = state.current_model_id()?;
    let models = state.model_choices();
    Some(json!({
        "availableModels": models
            .into_iter()
            .map(|(id, name)| json!({ "modelId": id, "name": name }))
            .collect::<Vec<_>>(),
        "currentModelId": current,
    }))
}

pub(super) fn available_efforts(state: &SessionUiState) -> Vec<&'static str> {
    crate::provider::inferred_reasoning_efforts(
        state.provider_name.as_deref(),
        state.model.as_deref(),
    )
    .into_iter()
    // `swarm` is a TUI sentinel, not provider effort levels.
    .filter(|effort| !effort.starts_with("swarm"))
    .collect()
}

/// Build the ACP `configOptions` array (model selector plus reasoning effort)
/// from the current session provider state. Empty when the daemon reported no
/// usable model state.
pub(super) fn session_config_options(state: &SessionUiState) -> Vec<Value> {
    let mut options = Vec::new();

    if let Some(model) = state.current_model_id() {
        let select_options: Vec<Value> = state
            .model_choices()
            .into_iter()
            .map(|(id, name)| json!({ "value": id, "name": name }))
            .collect();
        options.push(json!({
            "type": "select",
            "id": CONFIG_ID_MODEL,
            "name": "Model",
            "category": "model",
            "currentValue": model,
            "options": select_options,
        }));
    }

    let efforts = available_efforts(state);
    if !efforts.is_empty() {
        let current = state
            .reasoning_effort
            .as_deref()
            .filter(|effort| efforts.contains(effort))
            .unwrap_or_else(|| {
                if efforts.contains(&"medium") {
                    "medium"
                } else {
                    efforts[0]
                }
            });
        let select_options: Vec<Value> = efforts
            .iter()
            .map(|name| json!({ "value": name, "name": name }))
            .collect();
        options.push(json!({
            "type": "select",
            "id": CONFIG_ID_EFFORT,
            "name": "Reasoning effort",
            "category": "thought_level",
            "currentValue": current,
            "options": select_options,
        }));
    }

    options
}
