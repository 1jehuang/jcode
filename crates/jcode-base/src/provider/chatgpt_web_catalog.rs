//! Project observed ChatGPT browser models into provider catalogs and routes.

use super::build_chatgpt_web_route;
use super::{AccountModelAvailability, AccountModelAvailabilityState, ModelRoute, chatgpt_web};

pub(super) fn route_for_model(model: &str) -> ModelRoute {
    match chatgpt_web::find_model(model) {
        Some(entry) => build_chatgpt_web_route(model, entry.available, entry.detail),
        None => build_chatgpt_web_route(
            model,
            false,
            "not observed in the current ChatGPT browser session",
        ),
    }
}

pub(super) fn append_routes(routes: &mut Vec<ModelRoute>) {
    for model in chatgpt_web::cached_models() {
        routes.push(build_chatgpt_web_route(
            &model.model,
            model.available,
            model.detail,
        ));
    }
}

pub(super) fn append_picker_routes(
    routes: &mut Vec<ModelRoute>,
    current_provider_name: &str,
    current_model: &str,
) {
    for model in chatgpt_web::cached_models() {
        if !routes
            .iter()
            .any(|route| route.model == model.model && route.api_method == "chatgpt-web")
        {
            routes.push(build_chatgpt_web_route(
                &model.model,
                model.available,
                model.detail,
            ));
        }
    }
    if routes.is_empty() && jcode_provider_core::is_chatgpt_web_model(current_model) {
        routes.push(route_for_model(current_model));
    }
    if routes.is_empty() && !current_model.is_empty() && current_model != "unknown" {
        routes.push(ModelRoute {
            model: current_model.to_string(),
            provider: current_provider_name.to_string(),
            api_method: "current".to_string(),
            available: true,
            detail: "simplified catalog".to_string(),
            usage: None,
            cheapness: None,
        });
    }
}

pub(super) fn model_availability(model: &str) -> AccountModelAvailability {
    let entry = chatgpt_web::find_model(model);
    let available = entry.as_ref().is_some_and(|entry| entry.available);
    AccountModelAvailability {
        state: if available {
            AccountModelAvailabilityState::Available
        } else {
            AccountModelAvailabilityState::Unknown
        },
        reason: if available {
            None
        } else {
            Some(
                entry
                    .map(|entry| entry.detail)
                    .unwrap_or_else(|| "requires verified ChatGPT browser session".to_string()),
            )
        },
        source: "browser-session",
        observed_at: None,
    }
}
