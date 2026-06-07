use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;

pub const OPENCODE_AUTH_JSON_SOURCE_ID: &str = "opencode_auth_json";
pub const PI_AUTH_JSON_SOURCE_ID: &str = "pi_auth_json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalAuthSource {
    OpenCode,
    Pi,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalOAuthTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
}

impl ExternalAuthSource {
    pub fn source_id(self) -> &'static str {
        match self {
            Self::OpenCode => OPENCODE_AUTH_JSON_SOURCE_ID,
            Self::Pi => PI_AUTH_JSON_SOURCE_ID,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::OpenCode => "OpenCode auth.json",
            Self::Pi => "pi auth.json",
        }
    }

    pub fn path(self) -> Result<PathBuf> {
        match self {
            Self::OpenCode => crate::storage::user_home_path(".local/share/opencode/auth.json"),
            Self::Pi => crate::storage::user_home_path(".pi/agent/auth.json"),
        }
    }
}

const SOURCES: [ExternalAuthSource; 2] = [ExternalAuthSource::OpenCode, ExternalAuthSource::Pi];

pub fn trust_external_auth_source(source: ExternalAuthSource) -> Result<()> {
    crate::config::Config::allow_external_auth_source_for_path(
        source.source_id(),
        &source.path()?,
    )?;
    super::AuthStatus::invalidate_cache();
    Ok(())
}

pub fn has_any_unconsented_external_auth() -> bool {
    SOURCES
        .into_iter()
        .filter(|source| source.path().map(|path| path.exists()).unwrap_or(false))
        .any(|source| !source_allowed(source) && source_has_supported_auth(source))
}

pub fn unconsented_sources() -> Vec<ExternalAuthSource> {
    SOURCES
        .into_iter()
        .filter(|source| source.path().map(|path| path.exists()).unwrap_or(false))
        .filter(|source| !source_allowed(*source) && source_has_supported_auth(*source))
        .collect()
}

pub fn source_provider_labels(source: ExternalAuthSource) -> Vec<&'static str> {
    let mut labels = Vec::new();
    if source_contains_oauth_provider(source, &["openai-codex", "openai_codex", "openai"])
        .unwrap_or(false)
    {
        labels.push("OpenAI/Codex");
    }
    if source_contains_oauth_provider(source, &["anthropic", "claude"]).unwrap_or(false) {
        labels.push("Claude");
    }
    if source_contains_oauth_provider(source, &["google-gemini-cli", "gemini-cli", "gemini"])
        .unwrap_or(false)
    {
        labels.push("Gemini");
    }
    if source_contains_oauth_provider(source, &["google-antigravity", "antigravity"])
        .unwrap_or(false)
    {
        labels.push("Antigravity");
    }
    if source_contains_oauth_provider(source, &["github-copilot", "copilot"]).unwrap_or(false) {
        labels.push("GitHub Copilot");
    }
    if source_contains_supported_api_key(source).unwrap_or(false) {
        labels.push("OpenRouter/API-key providers");
    }
    labels
}

pub fn preferred_unconsented_api_key_source() -> Option<ExternalAuthSource> {
    SOURCES
        .into_iter()
        .filter(|source| source.path().map(|path| path.exists()).unwrap_or(false))
        .find(|source| {
            !source_allowed(*source) && source_contains_supported_api_key(*source).unwrap_or(false)
        })
}

pub fn preferred_unconsented_api_key_source_for_env(env_key: &str) -> Option<ExternalAuthSource> {
    SOURCES
        .into_iter()
        .filter(|source| source.path().map(|path| path.exists()).unwrap_or(false))
        .find(|source| {
            !source_allowed(*source)
                && load_api_key_from_source(*source, env_key)
                    .map(|key| !key.trim().is_empty())
                    .unwrap_or(false)
        })
}

pub fn preferred_unconsented_openai_oauth_source() -> Option<ExternalAuthSource> {
    preferred_unconsented_oauth_source_for_candidates(&["openai-codex", "openai_codex", "openai"])
}

pub fn preferred_unconsented_anthropic_oauth_source() -> Option<ExternalAuthSource> {
    preferred_unconsented_oauth_source_for_candidates(&["anthropic", "claude"])
}

pub fn preferred_unconsented_gemini_oauth_source() -> Option<ExternalAuthSource> {
    preferred_unconsented_oauth_source_for_candidates(&[
        "google-gemini-cli",
        "gemini-cli",
        "gemini",
    ])
}

pub fn preferred_unconsented_antigravity_oauth_source() -> Option<ExternalAuthSource> {
    preferred_unconsented_oauth_source_for_candidates(&["google-antigravity", "antigravity"])
}

pub fn load_api_key_for_env(env_key: &str) -> Option<String> {
    for source in SOURCES {
        if !source_allowed(source) {
            continue;
        }
        if let Some(key) = load_api_key_from_source(source, env_key) {
            return Some(key);
        }
    }
    None
}

pub fn load_openai_oauth_tokens() -> Option<ExternalOAuthTokens> {
    load_oauth_tokens_for_candidates(&["openai-codex", "openai_codex", "openai"])
}

pub fn load_copilot_oauth_token() -> Option<String> {
    load_oauth_tokens_for_candidates(&["github-copilot", "copilot"])
        .map(|tokens| tokens.access_token)
}

pub fn source_has_copilot_oauth(source: ExternalAuthSource) -> bool {
    source_contains_oauth_provider(source, &["github-copilot", "copilot"]).unwrap_or(false)
}

pub fn load_gemini_oauth_tokens() -> Option<ExternalOAuthTokens> {
    load_oauth_tokens_for_candidates(&["google-gemini-cli", "gemini-cli", "gemini"])
}

pub fn load_antigravity_oauth_tokens() -> Option<ExternalOAuthTokens> {
    load_oauth_tokens_for_candidates(&["google-antigravity", "antigravity"])
}

pub fn load_anthropic_oauth_tokens() -> Option<ExternalOAuthTokens> {
    load_oauth_tokens_for_candidates(&["anthropic", "claude"])
}

pub fn source_allowed(source: ExternalAuthSource) -> bool {
    let Ok(path) = source.path() else {
        return false;
    };

    if crate::config::Config::external_auth_source_allowed_for_path(source.source_id(), &path) {
        return true;
    }

    match source {
        ExternalAuthSource::OpenCode => {
            crate::config::Config::external_auth_source_allowed_for_path(
                crate::auth::claude::OPENCODE_AUTH_SOURCE_ID,
                &path,
            )
        }
        ExternalAuthSource::Pi => false,
    }
}

fn load_oauth_tokens_for_candidates(provider_keys: &[&str]) -> Option<ExternalOAuthTokens> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut expired: Option<ExternalOAuthTokens> = None;

    for source in SOURCES {
        if !source_allowed(source) {
            continue;
        }

        let Ok(auth_map) = load_auth_map(source) else {
            continue;
        };
        for key in provider_keys {
            if let Some(entry) = auth_map.get(*key)
                && let Some(tokens) = extract_oauth_tokens(entry)
            {
                if tokens.expires_at > now_ms {
                    return Some(tokens);
                }
                if expired.is_none() {
                    expired = Some(tokens);
                }
            }
        }
    }

    expired
}

fn preferred_unconsented_oauth_source_for_candidates(
    provider_keys: &[&str],
) -> Option<ExternalAuthSource> {
    SOURCES
        .into_iter()
        .filter(|source| source.path().map(|path| path.exists()).unwrap_or(false))
        .find(|source| {
            !source_allowed(*source)
                && source_contains_oauth_provider(*source, provider_keys).unwrap_or(false)
        })
}

fn source_has_supported_auth(source: ExternalAuthSource) -> bool {
    source_contains_supported_api_key(source).unwrap_or(false)
        || source_contains_oauth_provider(
            source,
            &[
                "openai-codex",
                "openai_codex",
                "openai",
                "anthropic",
                "claude",
                "google-gemini-cli",
                "gemini-cli",
                "gemini",
                "google-antigravity",
                "antigravity",
                "github-copilot",
                "copilot",
            ],
        )
        .unwrap_or(false)
}

fn source_contains_supported_api_key(source: ExternalAuthSource) -> Result<bool> {
    let auth = load_auth_map(source)?;
    Ok(auth
        .values()
        .any(|entry| extract_api_key(source, entry).is_some()))
}

fn source_contains_oauth_provider(
    source: ExternalAuthSource,
    provider_keys: &[&str],
) -> Result<bool> {
    let auth = load_auth_map(source)?;
    Ok(provider_keys.iter().any(|provider_key| {
        auth.get(*provider_key)
            .and_then(extract_oauth_tokens)
            .is_some()
    }))
}

fn load_api_key_from_source(source: ExternalAuthSource, env_key: &str) -> Option<String> {
    let auth = load_auth_map(source).ok()?;
    for &provider_key in provider_keys_for_env(env_key) {
        if let Some(entry) = auth.get(provider_key)
            && let Some(key) = extract_api_key(source, entry)
            && !key.trim().is_empty()
        {
            return Some(key);
        }
    }
    None
}

fn load_auth_map(source: ExternalAuthSource) -> Result<HashMap<String, Value>> {
    let path = crate::storage::validate_external_auth_file(&source.path()?)?;
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("Failed to parse {}", path.display()))
}

fn extract_api_key(source: ExternalAuthSource, entry: &Value) -> Option<String> {
    let object = entry.as_object()?;
    match source {
        ExternalAuthSource::OpenCode => {
            if object.get("type")?.as_str()? != "api" {
                return None;
            }
            object
                .get("key")?
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        }
        ExternalAuthSource::Pi => {
            if object.get("type")?.as_str()? != "api_key" {
                return None;
            }
            resolve_pi_api_key_value(object.get("key")?.as_str()?)
        }
    }
}

fn resolve_pi_api_key_value(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.starts_with('!') {
        return None;
    }

    if let Ok(value) = std::env::var(raw) {
        let value = value.trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }

    Some(raw.to_string())
}

fn extract_oauth_tokens(entry: &Value) -> Option<ExternalOAuthTokens> {
    let object = entry.as_object()?;
    let token_type = object.get("type").and_then(Value::as_str);
    if let Some(token_type) = token_type
        && token_type != "oauth"
    {
        return None;
    }

    let access_token = object.get("access")?.as_str()?.trim().to_string();
    let refresh_token = object.get("refresh")?.as_str()?.trim().to_string();
    let expires_at = object.get("expires")?.as_i64()?;

    if access_token.is_empty() || refresh_token.is_empty() {
        return None;
    }

    Some(ExternalOAuthTokens {
        access_token,
        refresh_token,
        expires_at,
    })
}

fn provider_keys_for_env(env_key: &str) -> &'static [&'static str] {
    match env_key {
        "ANTHROPIC_API_KEY" => &["anthropic", "claude"],
        "AZURE_OPENAI_API_KEY" => &["azure-openai-responses", "azure", "azure-openai"],
        "OPENAI_API_KEY" => &["openai"],
        "GEMINI_API_KEY" => &["google", "gemini"],
        "MISTRAL_API_KEY" => &["mistral"],
        "GROQ_API_KEY" => &["groq"],
        "CEREBRAS_API_KEY" => &["cerebras"],
        "XAI_API_KEY" => &["xai"],
        "OPENROUTER_API_KEY" => &["openrouter"],
        "AI_GATEWAY_API_KEY" => &["vercel-ai-gateway"],
        "ZHIPU_API_KEY" | "ZAI_API_KEY" => &["zai"],
        "OPENCODE_API_KEY" => &["opencode"],
        "OPENCODE_GO_API_KEY" => &["opencode-go", "opencode"],
        "HF_TOKEN" => &["huggingface"],
        "KIMI_API_KEY" => &["kimi-coding", "kimi", "moonshot"],
        "MINIMAX_API_KEY" => &["minimax"],
        "MINIMAX_CN_API_KEY" => &["minimax-cn"],
        "NEBIUS_API_KEY" => &["nebius"],
        "SCALEWAY_API_KEY" => &["scaleway"],
        "STACKIT_API_KEY" => &["stackit"],
        "TOGETHER_API_KEY" => &["togetherai", "together-ai", "together"],
        "DEEPINFRA_API_KEY" => &["deepinfra"],
        "FIREWORKS_API_KEY" => &["fireworks"],
        "CHUTES_API_KEY" => &["chutes"],
        "BASETEN_API_KEY" => &["baseten"],
        "CORTECS_API_KEY" => &["cortecs"],
        "COMTEGRA_API_KEY" => &["comtegra", "cgc"],
        "DEEPSEEK_API_KEY" => &["deepseek"],
        "FIRMWARE_API_KEY" => &["firmware"],
        "MOONSHOT_API_KEY" => &["moonshotai", "moonshot"],
        "PERPLEXITY_API_KEY" => &["perplexity"],
        "BAILIAN_CODING_PLAN_API_KEY" => &["alibaba-coding-plan", "bailian"],
        _ => &[],
    }
}

#[cfg(test)]
#[path = "external_tests.rs"]
mod external_tests;
