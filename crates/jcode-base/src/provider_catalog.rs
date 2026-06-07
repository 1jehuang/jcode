pub use jcode_provider_env::{
    load_api_key_from_env_or_config, load_env_value_from_env_or_config,
    register_api_key_fallback_resolver, save_env_value_to_env_file,
};
pub use jcode_provider_metadata::*;
use std::collections::{HashMap, HashSet};

pub const OPENAI_COMPAT_LOCAL_ENABLED_ENV: &str = "JCODE_OPENAI_COMPAT_LOCAL_ENABLED";
pub const MINIMAX_CHINA_API_BASE: &str = "https://api.minimaxi.com/v1";
pub const MINIMAX_CHINA_SETUP_URL: &str = "https://platform.minimaxi.com/docs/llms.txt";

pub fn api_base_uses_localhost(raw: &str) -> bool {
    let Ok(parsed) = url::Url::parse(raw) else {
        return false;
    };

    matches!(
        parsed
            .host_str()
            .map(|host| host.to_ascii_lowercase())
            .as_deref(),
        Some("localhost") | Some("127.0.0.1") | Some("::1")
    )
}

pub fn resolve_openai_compatible_profile(
    profile: OpenAiCompatibleProfile,
) -> ResolvedOpenAiCompatibleProfile {
    resolve_openai_compatible_profile_with_api_key_hint(profile, None)
}

pub fn resolve_openai_compatible_profile_with_api_key_hint(
    profile: OpenAiCompatibleProfile,
    api_key_hint: Option<&str>,
) -> ResolvedOpenAiCompatibleProfile {
    let mut resolved = ResolvedOpenAiCompatibleProfile {
        id: profile.id.to_string(),
        display_name: profile.display_name.to_string(),
        api_base: profile.api_base.to_string(),
        api_key_env: profile.api_key_env.to_string(),
        env_file: profile.env_file.to_string(),
        setup_url: profile.setup_url.to_string(),
        default_model: profile.default_model.map(ToString::to_string),
        requires_api_key: profile.requires_api_key,
    };

    apply_profile_key_based_endpoint_overrides(profile, &mut resolved, api_key_hint);

    if profile.id != OPENAI_COMPAT_PROFILE.id {
        if let Some(newest_model) =
            newest_released_model_for_resolved_openai_compatible_profile(profile.id, &resolved)
        {
            resolved.default_model = Some(newest_model);
        }
        return resolved;
    }

    if let Some(base) = env_override("JCODE_OPENAI_COMPAT_API_BASE") {
        if let Some(normalized) = normalize_api_base(&base) {
            resolved.api_base = normalized;
        } else {
            eprintln!(
                "Warning: ignoring invalid JCODE_OPENAI_COMPAT_API_BASE '{}'. Use https://... (or http://localhost).",
                base
            );
        }
    }

    if let Some(key_name) = env_override("JCODE_OPENAI_COMPAT_API_KEY_NAME") {
        if is_safe_env_key_name(&key_name) {
            resolved.api_key_env = key_name;
        } else {
            eprintln!(
                "Warning: ignoring invalid JCODE_OPENAI_COMPAT_API_KEY_NAME '{}'.",
                key_name
            );
        }
    }

    if let Some(env_file) = env_override("JCODE_OPENAI_COMPAT_ENV_FILE") {
        if is_safe_env_file_name(&env_file) {
            resolved.env_file = env_file;
        } else {
            eprintln!(
                "Warning: ignoring invalid JCODE_OPENAI_COMPAT_ENV_FILE '{}'.",
                env_file
            );
        }
    }

    if let Some(setup_url) = env_override("JCODE_OPENAI_COMPAT_SETUP_URL") {
        resolved.setup_url = setup_url;
    }

    if let Some(model) = env_override("JCODE_OPENAI_COMPAT_DEFAULT_MODEL") {
        resolved.default_model = Some(model);
    }

    if api_base_uses_localhost(&resolved.api_base) {
        resolved.requires_api_key = false;
    }

    resolved
}

pub fn newest_released_model_for_openai_compatible_profile(profile_id: &str) -> Option<String> {
    let profile = openai_compatible_profile_by_id(profile_id)?;
    let resolved = resolve_openai_compatible_profile(profile);
    newest_released_model_for_resolved_openai_compatible_profile(profile_id, &resolved)
}

fn newest_released_model_for_resolved_openai_compatible_profile(
    profile_id: &str,
    resolved: &ResolvedOpenAiCompatibleProfile,
) -> Option<String> {
    openai_compatible_profile_by_id(profile_id)?;
    let cache = jcode_provider_openrouter::load_disk_cache_entry_for_namespace(&resolved.id)?;

    let source_matches = cache
        .source_api_base
        .as_deref()
        .and_then(normalize_api_base)
        == normalize_api_base(&resolved.api_base);
    if !source_matches {
        return None;
    }

    cache
        .models
        .into_iter()
        .enumerate()
        .filter_map(|(index, model)| {
            let id = model.id.trim().to_string();
            let created = model.created?;
            // Never auto-select an obviously non-chat model (TTS/speech/embeddings/
            // rerankers/image/etc.) as a profile's default. Catalogs like Groq,
            // NVIDIA NIM, and Chutes expose their entire model list, and the
            // newest-released entry is frequently a non-chat model (e.g. Groq's
            // `canopylabs/orpheus-*` TTS), which must not become the chat default.
            if id.is_empty() || !crate::provider::is_listable_model_name(&id) {
                return None;
            }
            Some((created, std::cmp::Reverse(index), id))
        })
        .max_by_key(|(created, reverse_index, _)| (*created, *reverse_index))
        .map(|(_, _, id)| id)
}

fn apply_profile_key_based_endpoint_overrides(
    profile: OpenAiCompatibleProfile,
    resolved: &mut ResolvedOpenAiCompatibleProfile,
    api_key_hint: Option<&str>,
) {
    if profile.id != MINIMAX_PROFILE.id {
        return;
    }

    let key = api_key_hint
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(ToString::to_string)
        .or_else(|| load_env_value_from_env_or_config(profile.api_key_env, profile.env_file));

    if key
        .as_deref()
        .map(|key| key.trim_start().starts_with("sk-cp-"))
        .unwrap_or(false)
    {
        resolved.api_base = MINIMAX_CHINA_API_BASE.to_string();
        resolved.setup_url = MINIMAX_CHINA_SETUP_URL.to_string();
    }
}

pub fn resolve_openai_compatible_profile_selection(input: &str) -> Option<OpenAiCompatibleProfile> {
    let provider = resolve_login_provider(input)?;
    match provider.target {
        LoginProviderTarget::OpenAiCompatible(profile) => Some(profile),
        _ => None,
    }
}

pub fn active_openai_compatible_display_name() -> Option<String> {
    if let Ok(profile_name) = std::env::var("JCODE_NAMED_PROVIDER_PROFILE") {
        let trimmed = profile_name.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    if let Ok(namespace) = std::env::var("JCODE_OPENROUTER_CACHE_NAMESPACE") {
        let trimmed = namespace.trim();
        if let Some(profile) = openai_compatible_profiles()
            .iter()
            .copied()
            .find(|profile| profile.id == trimmed)
        {
            return Some(profile.display_name.to_string());
        }
    }

    let api_base = std::env::var("JCODE_OPENROUTER_API_BASE")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| env_override("JCODE_OPENAI_COMPAT_API_BASE"));

    let api_base = api_base.and_then(|value| normalize_api_base(&value))?;

    for profile in openai_compatible_profiles().iter().copied() {
        if normalize_api_base(profile.api_base).as_deref() == Some(api_base.as_str()) {
            return Some(profile.display_name.to_string());
        }
    }

    if !api_base.contains("openrouter.ai") {
        return Some("OpenAI-compatible".to_string());
    }

    None
}

pub fn runtime_provider_display_name(provider_name: &str) -> String {
    if provider_name.eq_ignore_ascii_case("openrouter") {
        if let Ok(runtime_provider) = std::env::var("JCODE_RUNTIME_PROVIDER")
            && runtime_provider.trim().eq_ignore_ascii_case("azure-openai")
        {
            return "Azure OpenAI".to_string();
        }

        active_openai_compatible_display_name().unwrap_or_else(|| "OpenRouter".to_string())
    } else {
        provider_name.to_string()
    }
}

pub fn openai_compatible_profile_by_id(id: &str) -> Option<OpenAiCompatibleProfile> {
    let normalized = id.trim().to_ascii_lowercase();
    openai_compatible_profiles()
        .iter()
        .copied()
        .find(|profile| profile.id == normalized)
}

pub fn openai_compatible_profile_id_for_api_base(api_base: &str) -> Option<&'static str> {
    let normalized = normalize_api_base(api_base)?;
    openai_compatible_profiles()
        .iter()
        .copied()
        .find(|profile| {
            normalize_api_base(profile.api_base).as_deref() == Some(normalized.as_str())
        })
        .map(|profile| profile.id)
}

pub fn openai_compatible_profile_id_for_display_name(display_name: &str) -> Option<&'static str> {
    let normalized = display_name.trim().to_ascii_lowercase();
    openai_compatible_profiles()
        .iter()
        .copied()
        .find(|profile| {
            profile.id == normalized
                || profile
                    .display_name
                    .eq_ignore_ascii_case(display_name.trim())
        })
        .map(|profile| profile.id)
}

pub fn openai_compatible_profile_static_models(profile: OpenAiCompatibleProfile) -> Vec<String> {
    let mut models = Vec::new();
    let mut push = |model: &str| {
        let model = model.trim();
        if !model.is_empty() && !models.iter().any(|existing| existing == model) {
            models.push(model.to_string());
        }
    };

    match profile.id {
        "opencode" => {
            push("minimax-m2.7");
            push("kimi-k2.5");
            push("glm-4.7");
            push("glm-5");
            push("claude-haiku-4-5");
            push("gpt-5.1-codex-max");
        }
        "opencode-go" => {
            push("minimax-m2.7");
            push("kimi-k2.5");
            push("glm-5");
            push("glm-5.1");
            push("deepseek-v4-flash");
            push("qwen3.5-plus");
        }
        "zai" => {
            push("glm-4.5");
            push("glm-4.7");
            push("glm-5");
            push("glm-5.1");
            push("glm-4.7-flash");
            push("glm-4.7-flashx");
        }
        "302ai" => {
            push("qwen3-235b-a22b-instruct-2507");
            push("glm-4.7");
            push("glm-5.1");
            push("MiniMax-M2");
            push("kimi-k2-0905-preview");
            push("claude-haiku-4-5");
        }
        "baseten" => {
            push("zai-org/GLM-4.7");
            push("zai-org/GLM-5");
            push("openai/gpt-oss-120b");
            push("moonshotai/Kimi-K2.6");
            push("moonshotai/Kimi-K2.5");
            push("deepseek-ai/DeepSeek-V4-Pro");
        }
        "cortecs" => {
            push("minimax-m2.7");
            push("kimi-k2.5");
            push("glm-4.7");
            push("glm-5");
            push("claude-haiku-4-5");
            push("qwen3-235b-a22b-instruct-2507");
        }
        // Issue #79: DeepSeek's live model catalog is not always available during
        // TUI startup, but both models should still be selectable once the direct
        // provider is configured.
        "deepseek" => {
            push("deepseek-v4-flash");
            push("deepseek-v4-pro");
        }
        "comtegra" => {
            push("gpt-oss-120b");
            push("qwen35-122b");
            push("gte-qwen2-7b");
            push("glm-51-nvfp4");
        }
        "fpt" => {
            push("GLM-5.1");
            push("GLM-4.7");
            push("Llama-3.3-70B-Instruct");
        }
        "kimi" => {
            push("kimi-for-coding");
            push("kimi-k2.5");
            push("kimi-k2.6");
            push("kimi-k2-thinking");
            push("kimi-k2-thinking-turbo");
        }
        "firmware" => {
            push("kimi-k2.5");
            push("zai-glm-5-1");
            push("claude-haiku-4-5");
            push("claude-sonnet-4-6");
            push("grok-code-fast-1");
            push("gemini-2.5-flash");
        }
        "huggingface" => {
            push("Qwen/Qwen3-Coder-480B-A35B-Instruct");
            push("Qwen/Qwen3-Coder-Next");
            push("zai-org/GLM-4.7");
            push("zai-org/GLM-5.1");
            push("deepseek-ai/DeepSeek-V3.2");
            push("openai/gpt-oss-120b");
        }
        "moonshotai" => {
            push("kimi-k2.5");
            push("kimi-k2.6");
            push("kimi-k2-thinking");
            push("kimi-k2-thinking-turbo");
            push("kimi-k2-turbo-preview");
        }
        "nebius" => {
            push("openai/gpt-oss-120b");
            push("Qwen/Qwen3-235B-A22B-Instruct-2507");
            push("Qwen/Qwen3.5-397B-A17B");
            push("zai-org/GLM-5");
            push("meta-llama/Llama-3.3-70B-Instruct");
            push("NousResearch/Hermes-4-70B");
        }
        "scaleway" => {
            push("qwen3-coder-30b-a3b-instruct");
            push("qwen3-235b-a22b-instruct-2507");
            push("qwen3.5-397b-a17b");
            push("gpt-oss-120b");
            push("mistral-small-3.2-24b-instruct-2506");
            push("llama-3.3-70b-instruct");
        }
        "stackit" => {
            push("openai/gpt-oss-120b");
            push("Qwen/Qwen3-VL-235B-A22B-Instruct-FP8");
            push("cortecs/Llama-3.3-70B-Instruct-FP8-Dynamic");
            push("neuralmagic/Meta-Llama-3.1-8B-Instruct-FP8");
            push("google/gemma-3-27b-it");
        }
        "perplexity" => {
            push("sonar");
            push("sonar-pro");
            push("sonar-reasoning-pro");
            push("sonar-deep-research");
        }
        "deepinfra" => {
            push("moonshotai/Kimi-K2-Instruct");
            push("Qwen/Qwen3-Coder-480B-A35B-Instruct");
            push("Qwen/Qwen3-Coder-480B-A35B-Instruct-Turbo");
            push("zai-org/GLM-4.7");
            push("zai-org/GLM-5.1");
            push("meta-llama/Llama-3.1-70B-Instruct");
        }
        "fireworks" => {
            push("accounts/fireworks/routers/kimi-k2p5-turbo");
            push("accounts/fireworks/models/kimi-k2p5");
            push("accounts/fireworks/models/kimi-k2p6");
            push("accounts/fireworks/models/glm-4p7");
            push("accounts/fireworks/models/glm-5p1");
            push("accounts/fireworks/models/deepseek-v3p2");
        }
        "cerebras" => {
            push("gpt-oss-120b");
            push("zai-glm-4.7");
        }
        "xiaomi-mimo" => {
            push("mimo-v2.5");
            push("mimo-v2.5-pro");
            push("mimo-v2-pro");
            push("mimo-v2-flash");
            push("mimo-v2-omni");
        }
        // MiniMax's `/models` endpoint is authenticated and live, but post-login
        // model activation should not depend on the catalog refresh completing
        // before the picker/routes are rebuilt. Keep the documented text models
        // selectable immediately after saving a key.
        "minimax" => {
            push("MiniMax-M2.7");
            push("MiniMax-M2.7-highspeed");
            push("MiniMax-M2.5");
            push("MiniMax-M2.5-highspeed");
            push("MiniMax-M2.1");
            push("MiniMax-M2.1-highspeed");
            push("MiniMax-M2");
        }
        "alibaba-coding-plan" => {
            push("qwen3-coder-plus");
            push("qwen3.5-plus");
            push("qwen3-max-2026-01-23");
            push("qwen3-coder-next");
            push("glm-5");
            push("glm-4.7");
            push("kimi-k2.5");
            push("MiniMax-M2.5");
        }
        "gemini-api" => {
            push("gemini-2.5-flash");
            push("gemini-2.5-pro");
            push("gemini-2.0-flash");
            push("gemini-2.0-flash-lite");
        }
        _ => {}
    }

    models
}

pub fn openai_compatible_profile_model_supports_chat(_profile_id: &str, _model: &str) -> bool {
    true
}

pub fn openai_compatible_profile_static_context_limits(
    profile: OpenAiCompatibleProfile,
) -> HashMap<String, usize> {
    openai_compatible_profile_static_models(profile)
        .into_iter()
        .filter_map(|model| {
            openai_compatible_profile_context_limit(profile.id, &model).map(|limit| (model, limit))
        })
        .collect()
}

pub fn openai_compatible_profile_context_limit(profile_id: &str, model: &str) -> Option<usize> {
    let profile_id = profile_id.trim().to_ascii_lowercase();
    let model = model.trim().to_ascii_lowercase();

    match profile_id.as_str() {
        // DeepSeek V4 direct API models advertise a 1M token context window. The
        // direct profile runs through the OpenRouter/OpenAI-compatible provider
        // implementation, whose live catalog can be unavailable during startup.
        "deepseek" if model.starts_with("deepseek-v4-") => Some(1_000_000),
        _ => None,
    }
}

pub fn apply_openai_compatible_profile_env(profile: Option<OpenAiCompatibleProfile>) {
    apply_openai_compatible_profile_env_impl(profile, true);
}

pub fn force_apply_openai_compatible_profile_env(profile: Option<OpenAiCompatibleProfile>) {
    apply_openai_compatible_profile_env_impl(profile, false);
}

fn apply_openai_compatible_profile_env_impl(
    profile: Option<OpenAiCompatibleProfile>,
    respect_named_profile_lock: bool,
) {
    if respect_named_profile_lock && std::env::var_os("JCODE_PROVIDER_PROFILE_ACTIVE").is_some() {
        return;
    }

    let vars = [
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_CACHE_NAMESPACE",
        "JCODE_OPENROUTER_PROVIDER_FEATURES",
        "JCODE_OPENROUTER_TRANSPORT_STATE",
        "JCODE_OPENROUTER_ALLOW_NO_AUTH",
        "JCODE_OPENROUTER_MODEL_CATALOG",
        "JCODE_OPENROUTER_MODEL",
        "JCODE_OPENROUTER_STATIC_MODELS",
        "JCODE_OPENROUTER_AUTH_HEADER",
        "JCODE_OPENROUTER_AUTH_HEADER_NAME",
        "JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER",
        "JCODE_OPENROUTER_PROVIDER",
        "JCODE_OPENROUTER_NO_FALLBACK",
        "JCODE_NAMED_PROVIDER_PROFILE",
        "JCODE_PROVIDER_PROFILE_ACTIVE",
        "JCODE_PROVIDER_PROFILE_NAME",
    ];

    for var in vars {
        crate::env::remove_var(var);
    }

    if let Some(profile) = profile {
        let resolved = resolve_openai_compatible_profile(profile);
        crate::env::set_var("JCODE_OPENROUTER_API_BASE", &resolved.api_base);
        crate::env::set_var("JCODE_OPENROUTER_API_KEY_NAME", &resolved.api_key_env);
        crate::env::set_var("JCODE_OPENROUTER_ENV_FILE", &resolved.env_file);
        crate::env::set_var("JCODE_OPENROUTER_CACHE_NAMESPACE", &resolved.id);
        crate::env::set_var("JCODE_OPENROUTER_PROVIDER_FEATURES", "0");
        let static_models = openai_compatible_profile_static_models(profile);
        if static_models.is_empty() {
            crate::env::remove_var("JCODE_OPENROUTER_STATIC_MODELS");
        } else {
            crate::env::set_var("JCODE_OPENROUTER_STATIC_MODELS", static_models.join("\n"));
        }
        if resolved.requires_api_key {
            crate::env::remove_var("JCODE_OPENROUTER_ALLOW_NO_AUTH");
            crate::env::set_var("JCODE_OPENROUTER_TRANSPORT_STATE", "direct-api-key");
        } else {
            crate::env::set_var("JCODE_OPENROUTER_ALLOW_NO_AUTH", "1");
            crate::env::set_var("JCODE_OPENROUTER_TRANSPORT_STATE", "direct-no-auth");
        }
    }
}

fn inline_key_env_name(profile_name: &str) -> String {
    let suffix = profile_name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("JCODE_PROVIDER_{}_API_KEY", suffix)
}

pub fn apply_named_provider_profile_env(profile_name: &str) -> anyhow::Result<String> {
    let config = crate::config::Config::load_strict()?;
    apply_named_provider_profile_env_from_config(profile_name, &config)
}

pub fn apply_named_provider_profile_env_from_config(
    profile_name: &str,
    config: &crate::config::Config,
) -> anyhow::Result<String> {
    let Some(profile) = config.providers.get(profile_name) else {
        anyhow::bail!(
            "Unknown provider profile '{}'. Add [providers.{}] to config.toml.",
            profile_name,
            profile_name
        );
    };

    let api_base = normalize_api_base(&profile.base_url).ok_or_else(|| {
        anyhow::anyhow!(
            "Provider profile '{}' has invalid base_url '{}'. Use https://... or http://localhost.",
            profile_name,
            profile.base_url
        )
    })?;

    crate::env::remove_var("JCODE_PROVIDER_PROFILE_ACTIVE");
    crate::env::remove_var("JCODE_PROVIDER_PROFILE_NAME");
    crate::env::remove_var("JCODE_NAMED_PROVIDER_PROFILE");
    apply_openai_compatible_profile_env(None);
    crate::env::set_var("JCODE_OPENROUTER_API_BASE", &api_base);
    crate::env::set_var("JCODE_OPENROUTER_CACHE_NAMESPACE", profile_name);
    crate::env::set_var("JCODE_NAMED_PROVIDER_PROFILE", profile_name);
    let provider_is_openrouter = matches!(
        profile.provider_type,
        crate::config::NamedProviderType::OpenRouter
    );

    let provider_features =
        provider_is_openrouter || profile.provider_routing || profile.allow_provider_pinning;
    crate::env::set_var(
        "JCODE_OPENROUTER_PROVIDER_FEATURES",
        if provider_features { "1" } else { "0" },
    );
    crate::env::set_var(
        "JCODE_OPENROUTER_MODEL_CATALOG",
        if profile.model_catalog
            || matches!(
                profile.provider_type,
                crate::config::NamedProviderType::OpenRouter
            )
        {
            "1"
        } else {
            "0"
        },
    );

    if let Some(model) = profile
        .default_model
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        crate::env::set_var("JCODE_OPENROUTER_MODEL", model);
    }

    let static_models = profile
        .models
        .iter()
        .map(|model| model.id.trim())
        .filter(|id| !id.is_empty())
        .collect::<Vec<_>>();
    if !static_models.is_empty() {
        crate::env::set_var("JCODE_OPENROUTER_STATIC_MODELS", static_models.join("\n"));
    }

    match profile.auth {
        crate::config::NamedProviderAuth::None => {
            crate::env::set_var("JCODE_OPENROUTER_ALLOW_NO_AUTH", "1");
            crate::env::set_var("JCODE_OPENROUTER_TRANSPORT_STATE", "direct-no-auth");
        }
        crate::config::NamedProviderAuth::Bearer | crate::config::NamedProviderAuth::Header => {
            let key_env = profile
                .api_key_env
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(ToString::to_string)
                .or_else(|| {
                    profile.api_key.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(|key| {
                        let env_name = inline_key_env_name(profile_name);
                        crate::env::set_var(&env_name, key);
                        crate::logging::warn(&format!(
                            "Provider profile '{}' stores an inline API key in config.toml. Prefer api_key_env to avoid accidental leaks.",
                            profile_name
                        ));
                        env_name
                    })
                });

            if let Some(key_env) = key_env {
                if !is_safe_env_key_name(&key_env) {
                    anyhow::bail!(
                        "Provider profile '{}' has invalid api_key_env '{}'.",
                        profile_name,
                        key_env
                    );
                }
                crate::env::set_var("JCODE_OPENROUTER_API_KEY_NAME", &key_env);
            }

            if let Some(env_file) = profile
                .env_file
                .as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
            {
                if !is_safe_env_file_name(env_file) {
                    anyhow::bail!(
                        "Provider profile '{}' has invalid env_file '{}'.",
                        profile_name,
                        env_file
                    );
                }
                crate::env::set_var("JCODE_OPENROUTER_ENV_FILE", env_file);
            }

            let requires_key = profile
                .requires_api_key
                .unwrap_or(!api_base_uses_localhost(&api_base));
            if !requires_key {
                crate::env::set_var("JCODE_OPENROUTER_ALLOW_NO_AUTH", "1");
                crate::env::set_var("JCODE_OPENROUTER_TRANSPORT_STATE", "direct-no-auth");
            } else if provider_is_openrouter {
                crate::env::set_var("JCODE_OPENROUTER_TRANSPORT_STATE", "openrouter-api-key");
            } else {
                crate::env::set_var("JCODE_OPENROUTER_TRANSPORT_STATE", "direct-api-key");
            }

            match profile.auth {
                crate::config::NamedProviderAuth::Bearer => {
                    crate::env::set_var("JCODE_OPENROUTER_AUTH_HEADER", "bearer");
                }
                crate::config::NamedProviderAuth::Header => {
                    crate::env::set_var("JCODE_OPENROUTER_AUTH_HEADER", "api-key");
                    if let Some(header) = profile
                        .auth_header
                        .as_deref()
                        .map(str::trim)
                        .filter(|v| !v.is_empty())
                    {
                        crate::env::set_var("JCODE_OPENROUTER_AUTH_HEADER_NAME", header);
                    }
                }
                crate::config::NamedProviderAuth::None => {}
            }
        }
    }

    Ok(profile_name.to_string())
}

pub fn openrouter_like_api_key_sources() -> Vec<(String, String)> {
    let mut sources = Vec::with_capacity(10);
    sources.push((
        "OPENROUTER_API_KEY".to_string(),
        "openrouter.env".to_string(),
    ));

    for profile in openai_compatible_profiles() {
        if profile.requires_api_key {
            sources.push((
                profile.api_key_env.to_string(),
                profile.env_file.to_string(),
            ));
        }
    }

    if let Some(source) = configured_api_key_source(
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "OPENROUTER_API_KEY",
        "openrouter.env",
    ) {
        sources.push(source);
    }

    if let Some(source) = configured_api_key_source(
        "JCODE_OPENAI_COMPAT_API_KEY_NAME",
        "JCODE_OPENAI_COMPAT_ENV_FILE",
        OPENAI_COMPAT_PROFILE.api_key_env,
        OPENAI_COMPAT_PROFILE.env_file,
    ) {
        sources.push(source);
    }

    dedup_sources(sources)
}

fn parse_bool_like(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

pub fn openai_compatible_profile_is_configured(profile: OpenAiCompatibleProfile) -> bool {
    let resolved = resolve_openai_compatible_profile(profile);
    if load_api_key_from_env_or_config(&resolved.api_key_env, &resolved.env_file).is_some() {
        return true;
    }

    if resolved.requires_api_key {
        return false;
    }

    if profile.id == OPENAI_COMPAT_PROFILE.id && api_base_uses_localhost(&resolved.api_base) {
        return true;
    }

    load_env_value_from_env_or_config(OPENAI_COMPAT_LOCAL_ENABLED_ENV, &resolved.env_file)
        .map(|value| parse_bool_like(&value))
        .unwrap_or(false)
}

pub fn configured_api_key_source(
    key_var: &str,
    file_var: &str,
    default_key: &str,
    default_file: &str,
) -> Option<(String, String)> {
    if std::env::var_os(key_var).is_none() && std::env::var_os(file_var).is_none() {
        return None;
    }

    let env_key = std::env::var(key_var)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default_key.to_string());
    let file_name = std::env::var(file_var)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default_file.to_string());

    if !is_safe_env_key_name(&env_key) {
        crate::logging::warn(&format!(
            "Ignoring invalid {}='{}' while probing auth status",
            key_var, env_key
        ));
        return None;
    }
    if !is_safe_env_file_name(&file_name) {
        crate::logging::warn(&format!(
            "Ignoring invalid {}='{}' while probing auth status",
            file_var, file_name
        ));
        return None;
    }

    Some((env_key, file_name))
}

fn env_override(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .or_else(|| load_env_value_from_env_or_config(name, OPENAI_COMPAT_PROFILE.env_file))
}

fn dedup_sources(sources: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::with_capacity(sources.len());
    for (env_key, env_file) in sources {
        if seen.insert((env_key.clone(), env_file.clone())) {
            deduped.push((env_key, env_file));
        }
    }
    deduped
}

#[cfg(test)]
#[path = "provider_catalog_tests.rs"]
mod provider_catalog_tests;
