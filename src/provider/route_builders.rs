use super::pricing::{cheapness_for_route, openrouter_pricing_from_model_pricing};
use super::{ModelRoute, RouteCostConfidence, RouteCostSource, provider_for_model};
use std::collections::BTreeSet;

pub fn is_listable_model_name(model: &str) -> bool {
    let trimmed = model.trim();
    !trimmed.is_empty() && !matches!(trimmed, "copilot models" | "openrouter models")
}

pub fn openrouter_catalog_model_id(model: &str) -> Option<String> {
    let trimmed = model.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.contains('/') {
        return Some(trimmed.to_string());
    }

    match provider_for_model(trimmed) {
        Some("claude") => Some(format!("anthropic/{}", trimmed)),
        Some("openai") => Some(format!("openai/{}", trimmed)),
        Some("openrouter") => Some(trimmed.to_string()),
        _ => None,
    }
}

pub fn listable_model_names_from_routes(routes: &[ModelRoute]) -> Vec<String> {
    let mut models = Vec::new();
    let mut seen = BTreeSet::new();
    for route in routes {
        if is_listable_model_name(&route.model) && seen.insert(route.model.clone()) {
            models.push(route.model.clone());
        }
    }
    models
}

pub fn build_anthropic_oauth_route(
    model: &str,
    available: bool,
    detail: impl Into<String>,
) -> ModelRoute {
    ModelRoute {
        model: model.to_string(),
        provider: "Anthropic".to_string(),
        api_method: "claude-oauth".to_string(),
        available,
        detail: detail.into(),
        cheapness: cheapness_for_route(model, "Anthropic", "claude-oauth"),
    }
}

pub fn build_openai_oauth_route(
    model: &str,
    available: bool,
    detail: impl Into<String>,
) -> ModelRoute {
    build_openai_route(model, "openai-oauth", available, detail)
}

pub fn build_openai_api_key_route(
    model: &str,
    available: bool,
    detail: impl Into<String>,
) -> ModelRoute {
    build_openai_route(model, "openai-api-key", available, detail)
}

fn build_openai_route(
    model: &str,
    api_method: &str,
    available: bool,
    detail: impl Into<String>,
) -> ModelRoute {
    ModelRoute {
        model: model.to_string(),
        provider: "OpenAI".to_string(),
        api_method: api_method.to_string(),
        available,
        detail: detail.into(),
        cheapness: cheapness_for_route(model, "OpenAI", api_method),
    }
}

pub fn build_anthropic_vertex_route(model: &str) -> ModelRoute {
    ModelRoute {
        model: model.to_string(),
        provider: "Vertex AI".to_string(),
        api_method: "vertex".to_string(),
        available: true,
        detail: String::new(),
        cheapness: cheapness_for_route(model, "Anthropic", "vertex"),
    }
}

pub fn build_copilot_route(model: &str, available: bool, detail: impl Into<String>) -> ModelRoute {
    ModelRoute {
        model: model.to_string(),
        provider: "Copilot".to_string(),
        api_method: "copilot".to_string(),
        available,
        detail: detail.into(),
        cheapness: cheapness_for_route(model, "Copilot", "copilot"),
    }
}

pub fn build_openrouter_auto_route(
    model: &str,
    available: bool,
    auto_detail: impl Into<String>,
) -> ModelRoute {
    ModelRoute {
        model: model.to_string(),
        provider: "auto".to_string(),
        api_method: "openrouter".to_string(),
        available,
        detail: auto_detail.into(),
        cheapness: cheapness_for_route(model, "auto", "openrouter"),
    }
}

pub fn build_openrouter_endpoint_route(
    model: &str,
    endpoint: &crate::provider::openrouter::EndpointInfo,
    available: bool,
    age_suffix: Option<&str>,
) -> ModelRoute {
    let mut detail = endpoint.detail_string();
    if let Some(age_suffix) = age_suffix.map(str::trim).filter(|value| !value.is_empty()) {
        if !detail.is_empty() {
            detail = format!("{}, {}", detail, age_suffix);
        } else {
            detail = age_suffix.to_string();
        }
    }

    ModelRoute {
        model: model.to_string(),
        provider: endpoint.provider_name.clone(),
        api_method: "openrouter".to_string(),
        available,
        detail,
        cheapness: openrouter_pricing_from_model_pricing(
            &endpoint.pricing,
            RouteCostSource::OpenRouterEndpoint,
            RouteCostConfidence::High,
            Some(format!(
                "OpenRouter endpoint pricing for {}",
                endpoint.provider_name
            )),
        ),
    }
}

pub fn build_openrouter_fallback_provider_route(
    display_model: &str,
    catalog_model: &str,
    provider: &str,
) -> ModelRoute {
    ModelRoute {
        model: display_model.to_string(),
        provider: provider.to_string(),
        api_method: "openrouter".to_string(),
        available: true,
        detail: String::new(),
        cheapness: cheapness_for_route(catalog_model, provider, "openrouter"),
    }
}
