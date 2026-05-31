use super::*;
use crate::tui::{AgentModelTarget, PickerEntry, PickerOption};

pub(super) fn slash_command_preview_filter(input: &str, commands: &[&str]) -> Option<String> {
    let trimmed = input.trim_start();
    for command in commands {
        if let Some(rest) = trimmed.strip_prefix(command) {
            if rest.is_empty() {
                return Some(String::new());
            }
            if rest
                .chars()
                .next()
                .map(|ch| ch.is_whitespace())
                .unwrap_or(false)
            {
                return Some(rest.trim_start().to_string());
            }
        }
    }
    None
}

pub(super) fn catchup_candidates(
    current_session_id: &str,
) -> Vec<crate::tui::session_picker::SessionInfo> {
    session_picker::load_sessions()
        .unwrap_or_default()
        .into_iter()
        .filter(|session| session.id != current_session_id && session.needs_catchup)
        .collect()
}

pub(super) fn catchup_queue_position(
    current_session_id: &str,
    session_id: &str,
) -> Option<(usize, usize)> {
    let candidates = catchup_candidates(current_session_id);
    let total = candidates.len();
    candidates
        .iter()
        .position(|session| session.id == session_id)
        .map(|idx| (idx + 1, total))
}

pub(super) fn agent_model_target_label(target: AgentModelTarget) -> &'static str {
    match target {
        AgentModelTarget::Swarm => "Swarm / subagent",
        AgentModelTarget::Review => "Code review",
        AgentModelTarget::Judge => "Judge",
        AgentModelTarget::Memory => "Memory",
        AgentModelTarget::Ambient => "Ambient",
    }
}

pub(super) fn agent_model_target_slug(target: AgentModelTarget) -> &'static str {
    match target {
        AgentModelTarget::Swarm => "swarm",
        AgentModelTarget::Review => "review",
        AgentModelTarget::Judge => "judge",
        AgentModelTarget::Memory => "memory",
        AgentModelTarget::Ambient => "ambient",
    }
}

pub(super) fn agent_model_target_config_path(target: AgentModelTarget) -> &'static str {
    match target {
        AgentModelTarget::Swarm => "agents.swarm_model",
        AgentModelTarget::Review => "autoreview.model",
        AgentModelTarget::Judge => "autojudge.model",
        AgentModelTarget::Memory => "agents.memory_model",
        AgentModelTarget::Ambient => "ambient.model",
    }
}

pub(super) fn load_agent_model_override(target: AgentModelTarget) -> Option<String> {
    let cfg = crate::config::Config::load();
    match target {
        AgentModelTarget::Swarm => cfg.agents.swarm_model,
        AgentModelTarget::Review => cfg.autoreview.model,
        AgentModelTarget::Judge => cfg.autojudge.model,
        AgentModelTarget::Memory => cfg.agents.memory_model,
        AgentModelTarget::Ambient => cfg.ambient.model,
    }
}

pub(super) fn save_agent_model_override(
    target: AgentModelTarget,
    model: Option<&str>,
) -> anyhow::Result<()> {
    let mut cfg = crate::config::Config::load();
    let value = model
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    match target {
        AgentModelTarget::Swarm => cfg.agents.swarm_model = value,
        AgentModelTarget::Review => cfg.autoreview.model = value,
        AgentModelTarget::Judge => cfg.autojudge.model = value,
        AgentModelTarget::Memory => cfg.agents.memory_model = value,
        AgentModelTarget::Ambient => cfg.ambient.model = value,
    }
    cfg.save()
}

pub(super) fn model_entry_base_name(entry: &PickerEntry) -> String {
    if entry.effort.is_some() {
        entry
            .name
            .rsplit_once(" (")
            .map(|(base, _)| base.to_string())
            .unwrap_or_else(|| entry.name.clone())
    } else {
        entry.name.clone()
    }
}

pub(super) fn openrouter_route_model_id(model: &str) -> String {
    crate::provider::openrouter_catalog_model_id(model).unwrap_or_else(|| model.to_string())
}

pub(super) fn picker_route_model_spec(entry: &PickerEntry, route: &PickerOption) -> String {
    let bare_name = model_entry_base_name(entry);
    let api_method = crate::provider::ModelRouteApiMethod::parse(&route.api_method);
    match api_method {
        crate::provider::ModelRouteApiMethod::Copilot => format!("copilot:{}", bare_name),
        crate::provider::ModelRouteApiMethod::ClaudeOAuth => {
            format!("claude-oauth:{}", bare_name)
        }
        crate::provider::ModelRouteApiMethod::AnthropicApiKey if route.provider == "Anthropic" => {
            format!("claude-api:{}", bare_name)
        }
        crate::provider::ModelRouteApiMethod::Cursor => format!("cursor:{}", bare_name),
        crate::provider::ModelRouteApiMethod::Bedrock => format!("bedrock:{}", bare_name),
        crate::provider::ModelRouteApiMethod::OpenAIApiKey => format!("openai-api:{}", bare_name),
        crate::provider::ModelRouteApiMethod::OpenAIOAuth => {
            format!("openai-oauth:{}", bare_name)
        }
        _ if route.provider == "Antigravity" => format!("antigravity:{}", bare_name),
        crate::provider::ModelRouteApiMethod::OpenAiCompatible { .. } => {
            if let Some(profile_id) = openai_compatible_profile_id_for_route(route) {
                format!("{}:{}", profile_id, bare_name)
            } else {
                bare_name
            }
        }
        crate::provider::ModelRouteApiMethod::OpenRouter if route.provider != "auto" => format!(
            "{}@{}",
            openrouter_route_model_id(&bare_name),
            route.provider
        ),
        _ => bare_name,
    }
}

pub(super) fn openai_compatible_profile_id_for_route(route: &PickerOption) -> Option<String> {
    match crate::provider::ModelRouteApiMethod::parse(&route.api_method) {
        crate::provider::ModelRouteApiMethod::OpenAiCompatible {
            profile_id: Some(profile_id),
        } => Some(profile_id),
        crate::provider::ModelRouteApiMethod::OpenAiCompatible { profile_id: None } => {
            crate::provider_catalog::openai_compatible_profile_id_for_display_name(&route.provider)
                .map(ToOwned::to_owned)
        }
        _ => None,
    }
}

pub(super) fn model_entry_saved_spec(entry: &PickerEntry) -> String {
    let bare_name = model_entry_base_name(entry);
    let route = entry.options.get(entry.selected_option);
    if let Some(route) = route {
        picker_route_model_spec(entry, route)
    } else {
        bare_name
    }
}

pub(super) fn agent_model_inherit_fallback_label(target: AgentModelTarget) -> &'static str {
    match target {
        AgentModelTarget::Memory => "sidecar auto-select",
        AgentModelTarget::Swarm
        | AgentModelTarget::Review
        | AgentModelTarget::Judge
        | AgentModelTarget::Ambient => "provider default",
    }
}

pub(super) fn normalize_agent_model_summary(
    target: AgentModelTarget,
    summary: Option<String>,
) -> String {
    let fallback = agent_model_inherit_fallback_label(target);
    let Some(summary) = summary.map(|value| value.trim().to_string()) else {
        return fallback.to_string();
    };
    if summary.is_empty() {
        return fallback.to_string();
    }

    match summary.to_ascii_lowercase().as_str() {
        "unknown" | "(unknown)" | "unknown model" => fallback.to_string(),
        "(provider default)" => "provider default".to_string(),
        "(sidecar auto-select)" => "sidecar auto-select".to_string(),
        _ => summary,
    }
}

pub(super) fn agent_model_default_summary(target: AgentModelTarget, app: &App) -> String {
    let summary = match target {
        AgentModelTarget::Swarm => load_agent_model_override(target)
            .or_else(|| app.session.subagent_model.clone())
            .or_else(|| Some(app.provider.model())),
        AgentModelTarget::Review => load_agent_model_override(target)
            .or_else(|| super::commands::preferred_one_shot_review_override().map(|(m, _)| m))
            .or_else(|| app.session.model.clone())
            .or_else(|| Some(app.provider.model())),
        AgentModelTarget::Judge => load_agent_model_override(target)
            .or_else(|| super::commands::preferred_one_shot_review_override().map(|(m, _)| m))
            .or_else(|| app.session.model.clone())
            .or_else(|| Some(app.provider.model())),
        AgentModelTarget::Memory => load_agent_model_override(target),
        AgentModelTarget::Ambient => load_agent_model_override(target),
    };

    normalize_agent_model_summary(target, summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::{PickerAction, PickerEntry, PickerOption};

    fn entry(model: &str, route: PickerOption) -> PickerEntry {
        PickerEntry {
            name: model.to_string(),
            options: vec![route],
            action: PickerAction::Model,
            selected_option: 0,
            is_current: false,
            is_default: false,
            is_favorite: false,
            recommended: false,
            recommendation_rank: 0,
            usage_score: 0,
            old: false,
            created_date: None,
            effort: None,
        }
    }

    fn route(provider: &str, api_method: &str) -> PickerOption {
        PickerOption {
            provider: provider.to_string(),
            api_method: api_method.to_string(),
            available: true,
            detail: String::new(),
            estimated_reference_cost_micros: None,
        }
    }

    #[test]
    fn model_picker_specs_preserve_provider_and_openai_auth_state_space() {
        for (model, route, expected) in [
            (
                "gpt-5.5",
                route("OpenAI", "openai-oauth"),
                "openai-oauth:gpt-5.5",
            ),
            (
                "gpt-5.5",
                route("OpenAI", "openai-api-key"),
                "openai-api:gpt-5.5",
            ),
            (
                "claude-opus-4-6",
                route("Anthropic", "claude-oauth"),
                "claude-oauth:claude-opus-4-6",
            ),
            (
                "claude-opus-4-6",
                route("Anthropic", "claude-api"),
                "claude-api:claude-opus-4-6",
            ),
            (
                "glm-51-nvfp4",
                route("Comtegra GPU Cloud", "openai-compatible:comtegra"),
                "comtegra:glm-51-nvfp4",
            ),
            (
                "claude-sonnet-4-6",
                route("Copilot", "copilot"),
                "copilot:claude-sonnet-4-6",
            ),
        ] {
            let entry = entry(model, route.clone());
            assert_eq!(picker_route_model_spec(&entry, &route), expected);
        }
    }
}
