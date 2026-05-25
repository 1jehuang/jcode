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
    if route.api_method == "copilot" {
        format!("copilot:{}", bare_name)
    } else if route.api_method == "cursor" {
        format!("cursor:{}", bare_name)
    } else if route.api_method == "bedrock" {
        format!("bedrock:{}", bare_name)
    } else if route.provider == "Antigravity" {
        format!("antigravity:{}", bare_name)
    } else if let Some(profile_id) = openai_compatible_profile_id_for_route(route) {
        format!("{}:{}", profile_id, bare_name)
    } else if route.api_method == "openrouter" && route.provider != "auto" {
        format!(
            "{}@{}",
            openrouter_route_model_id(&bare_name),
            route.provider
        )
    } else {
        bare_name
    }
}

pub(super) fn openai_compatible_profile_id_for_route(route: &PickerOption) -> Option<&str> {
    if let Some(("openai-compatible", profile_id)) = route.api_method.split_once(':') {
        let profile_id = profile_id.trim();
        if !profile_id.is_empty() {
            return Some(profile_id);
        }
    }

    if route.api_method == "openai-compatible" {
        return crate::provider_catalog::openai_compatible_profile_id_for_display_name(
            &route.provider,
        );
    }

    None
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
