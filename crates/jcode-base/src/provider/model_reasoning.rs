//! Native per-route reasoning ladders shared by the picker and model control.

/// The selectable levels for one exact model/API route. Empty means no level step.
pub fn model_route_reasoning_efforts(
    model: &str,
    api_method: &str,
    metadata: Option<&jcode_provider_core::ModelMetadata>,
    config: &crate::config::Config,
) -> Vec<&'static str> {
    let levels = if let Some(levels) =
        named_profile_reasoning_efforts(config, Some(api_method), Some(model))
    {
        levels
    } else if api_method == "copilot" {
        let cached = jcode_provider_core::copilot_model_metadata(model);
        metadata
            .or(cached.as_ref())
            .map_or_else(Vec::new, |metadata| {
                metadata
                    .reasoning_efforts
                    .iter()
                    .filter_map(|level| jcode_provider_core::canonical_reasoning_effort(level))
                    .collect()
            })
    } else if api_method == "chatgpt-web" {
        let values = metadata
            .map(|metadata| metadata.reasoning_efforts.clone())
            .unwrap_or_else(|| {
                super::chatgpt_web::cached_reasoning_options(model)
                    .into_iter()
                    .filter(|option| option.available)
                    .map(|option| option.value)
                    .collect()
            });
        let mut levels = Vec::new();
        for value in values {
            if let Some(level) = jcode_provider_core::canonical_reasoning_effort(&value)
                && !levels.contains(&level)
            {
                levels.push(level);
            }
        }
        if levels.len() < 2 {
            levels.clear();
        }
        levels
    } else if route_supports_reasoning_effort(api_method) {
        jcode_provider_core::inferred_reasoning_efforts(Some(api_method), Some(model))
    } else {
        Vec::new()
    };
    levels
        .into_iter()
        .filter(|level| !crate::prompt::is_swarm_mode_effort(level))
        .collect()
}

/// Return the named OpenAI-compatible profile's actual effort ladder, or None
/// when `provider_name` is not a configured named profile. `Some(vec![])`
/// means the profile explicitly disables effort for this model.
pub fn named_profile_reasoning_efforts(
    config: &crate::config::Config,
    provider_name: Option<&str>,
    model_name: Option<&str>,
) -> Option<Vec<&'static str>> {
    let provider_name = provider_name?;
    let profile_id = provider_name
        .strip_prefix("openai-compatible:")
        .unwrap_or(provider_name);
    let (_, profile) = config
        .providers
        .iter()
        .find(|(id, _)| id.eq_ignore_ascii_case(profile_id))?;
    if profile.provider_type != crate::config::NamedProviderType::OpenAiCompatible {
        return None;
    }

    let model = model_name.unwrap_or("").trim().to_ascii_lowercase();
    let model_config = profile
        .models
        .iter()
        .find(|entry| entry.id.trim().eq_ignore_ascii_case(&model));
    if model_config.is_some_and(|entry| entry.supports_chat == Some(false)) {
        return Some(Vec::new());
    }
    if let Some(configured) = model_config.filter(|entry| !entry.reasoning_efforts.is_empty()) {
        let mut efforts: Vec<&'static str> = configured
            .reasoning_efforts
            .iter()
            .filter_map(|effort| jcode_provider_core::canonical_reasoning_effort(effort))
            .collect();
        jcode_provider_core::sort_reasoning_efforts(&mut efforts);
        efforts.dedup();
        efforts.push("swarm");
        return Some(efforts);
    }
    let model_reasoning = model_config.and_then(|entry| entry.reasoning);
    // Keep Conifer's current local override policy outside this port.
    if profile_id.eq_ignore_ascii_case("conifer") {
        return if model_reasoning == Some(false)
            || profile.supports_reasoning_effort == Some(false)
            || (profile.supports_reasoning_effort.is_none()
                && model_reasoning.is_none()
                && profile.disable_reasoning_heuristics)
        {
            Some(Vec::new())
        } else if profile.supports_reasoning_effort == Some(true) {
            Some(jcode_provider_core::DEEPSEEK_SELECTABLE_EFFORTS.to_vec())
        } else if model_reasoning == Some(true) {
            Some(jcode_provider_core::OPENAI_SELECTABLE_EFFORTS.to_vec())
        } else {
            None
        };
    }
    let heuristics = !profile.disable_reasoning_heuristics;
    let deepseek_style = model_reasoning != Some(false)
        && profile.supports_reasoning_effort.unwrap_or_else(|| {
            profile_id.eq_ignore_ascii_case("deepseek")
                || (heuristics && model.contains("deepseek"))
        });
    if deepseek_style {
        return Some(jcode_provider_core::DEEPSEEK_SELECTABLE_EFFORTS.to_vec());
    }
    let openai_style = model_reasoning.unwrap_or_else(|| {
        profile.supports_reasoning_effort != Some(false)
            && (profile_id.eq_ignore_ascii_case("zai")
                || (heuristics
                    && (model.starts_with("gpt-5")
                        || model.contains("codex")
                        || ["o1", "o3", "o4", "o5"]
                            .iter()
                            .any(|prefix| model.starts_with(prefix)))))
    });
    if openai_style {
        return Some(jcode_provider_core::OPENAI_SELECTABLE_EFFORTS.to_vec());
    }

    Some(Vec::new())
}

/// Whether a picker route's runtime can apply a per-request reasoning effort.
/// Effort rows are only rendered for these routes; other routes (Copilot,
/// Bedrock, Antigravity CLI, remote-catalog placeholders, ...) get one plain
/// row per model because a picked effort could not actually be applied.
pub fn route_supports_reasoning_effort(api_method: &str) -> bool {
    use crate::provider::ModelRouteApiMethod as Method;
    match Method::parse(api_method) {
        Method::ClaudeOAuth
        | Method::AnthropicApiKey
        | Method::OpenAIOAuth
        | Method::OpenAIApiKey
        | Method::OpenRouter => true,
        // Named profiles are checked against their configured capabilities in
        // model_route_effort_levels, which opens a second-step level picker.
        Method::OpenAiCompatible { .. } => false,
        Method::JcodeSubscription
        | Method::GrokBuild
        | Method::ClaudeCode { .. }
        | Method::Copilot
        | Method::Cursor
        | Method::Bedrock
        | Method::CodeAssistOAuth
        | Method::AntigravityHttps
        | Method::RemoteCatalog
        | Method::Current
        | Method::Other(_) => false,
    }
}
