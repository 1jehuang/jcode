use super::*;
use crate::provider_catalog::{LoginProviderDescriptor, LoginProviderTarget};
pub(super) use jcode_provider_core::{ActiveProvider, ProviderAvailability};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConfigProviderSelection {
    BuiltIn(ActiveProvider),
    OpenAiCompatibleProfile(&'static str),
    NamedProfile(String),
}

impl ConfigProviderSelection {
    pub(crate) fn active_provider(&self) -> ActiveProvider {
        match self {
            Self::BuiltIn(provider) => *provider,
            Self::OpenAiCompatibleProfile(_) | Self::NamedProfile(_) => ActiveProvider::OpenRouter,
        }
    }

    pub(crate) fn display_label(&self) -> String {
        match self {
            Self::BuiltIn(provider) => MultiProvider::provider_key(*provider).to_string(),
            Self::OpenAiCompatibleProfile(profile_id) => {
                let resolved =
                    crate::provider_catalog::resolve_openai_compatible_profile_selection(
                        profile_id,
                    )
                    .map(crate::provider_catalog::resolve_openai_compatible_profile);
                match resolved {
                    Some(profile) => format!("OpenAI-compatible profile {}", profile.display_name),
                    None => format!("OpenAI-compatible profile {}", profile_id),
                }
            }
            Self::NamedProfile(profile) => format!("provider profile '{}'", profile),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultModelSelection {
    pub model_spec: String,
    pub provider_key: Option<String>,
}

impl MultiProvider {
    pub(super) fn auto_default_provider(availability: ProviderAvailability) -> ActiveProvider {
        jcode_provider_core::auto_default_provider(availability)
    }

    pub(super) fn parse_provider_hint(value: &str) -> Option<ActiveProvider> {
        jcode_provider_core::parse_provider_hint(value)
    }

    pub(super) fn forced_provider_from_env() -> Option<ActiveProvider> {
        let force = std::env::var("JCODE_FORCE_PROVIDER")
            .ok()
            .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
            .unwrap_or(false);
        if !force {
            return None;
        }

        std::env::var("JCODE_ACTIVE_PROVIDER")
            .ok()
            .and_then(|value| Self::parse_provider_hint(&value))
    }

    pub(super) fn provider_label(provider: ActiveProvider) -> &'static str {
        jcode_provider_core::provider_label(provider)
    }

    pub(super) fn provider_key(provider: ActiveProvider) -> &'static str {
        jcode_provider_core::provider_key(provider)
    }

    pub(super) fn set_active_provider(&self, provider: ActiveProvider) {
        *self
            .active
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = provider;
    }

    pub fn config_default_provider_for_login_provider(
        provider: LoginProviderDescriptor,
    ) -> Option<&'static str> {
        match provider.target {
            LoginProviderTarget::Claude => Some("claude"),
            LoginProviderTarget::OpenAi | LoginProviderTarget::OpenAiApiKey => Some("openai"),
            LoginProviderTarget::OpenRouter => Some("openrouter"),
            LoginProviderTarget::Bedrock => Some("bedrock"),
            LoginProviderTarget::OpenAiCompatible(profile) => Some(profile.id),
            LoginProviderTarget::Cursor => Some("cursor"),
            LoginProviderTarget::Copilot => Some("copilot"),
            LoginProviderTarget::Gemini => Some("gemini"),
            LoginProviderTarget::Antigravity => Some("antigravity"),
            LoginProviderTarget::AutoImport
            | LoginProviderTarget::Jcode
            | LoginProviderTarget::Azure
            | LoginProviderTarget::Google => None,
        }
    }

    pub fn openai_compatible_profile_id_from_route<'a>(
        api_method: &'a str,
        provider_display: &str,
    ) -> Option<&'a str> {
        if let Some(("openai-compatible", profile_id)) = api_method.split_once(':') {
            let profile_id = profile_id.trim();
            if !profile_id.is_empty() {
                return Some(profile_id);
            }
        }

        if api_method == "openai-compatible" {
            return crate::provider_catalog::openai_compatible_profile_id_for_display_name(
                provider_display,
            );
        }

        None
    }

    pub fn default_model_selection_from_route(
        bare_name: &str,
        api_method: &str,
        provider_display: &str,
    ) -> DefaultModelSelection {
        let profile_id =
            Self::openai_compatible_profile_id_from_route(api_method, provider_display);
        let model_spec = if api_method == "copilot" {
            format!("copilot:{}", bare_name)
        } else if api_method == "cursor" {
            format!("cursor:{}", bare_name)
        } else if api_method == "bedrock" {
            format!("bedrock:{}", bare_name)
        } else if provider_display == "Antigravity" {
            format!("antigravity:{}", bare_name)
        } else if profile_id.is_some() {
            bare_name.to_string()
        } else if api_method == "openrouter" && provider_display != "auto" {
            if bare_name.contains('/') {
                format!("{}@{}", bare_name, provider_display)
            } else {
                format!("anthropic/{}@{}", bare_name, provider_display)
            }
        } else {
            bare_name.to_string()
        };

        let provider_key = match api_method {
            "claude-oauth" | "api-key"
                if crate::provider::provider_for_model(bare_name) == Some("claude") =>
            {
                Some("claude".to_string())
            }
            "openai-oauth" | "openai-api-key" => Some("openai".to_string()),
            "copilot" => Some("copilot".to_string()),
            "cursor" => Some("cursor".to_string()),
            "bedrock" => Some("bedrock".to_string()),
            "cli" if provider_display == "Antigravity" => Some("antigravity".to_string()),
            "openrouter" => Some("openrouter".to_string()),
            method if method.starts_with("openai-compatible") => profile_id.map(ToOwned::to_owned),
            _ => profile_id.map(ToOwned::to_owned),
        };

        DefaultModelSelection {
            model_spec,
            provider_key,
        }
    }

    pub(super) fn resolve_config_provider_selection(
        value: &str,
        cfg: &crate::config::Config,
    ) -> Option<ConfigProviderSelection> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return None;
        }

        if let Some(profile) =
            crate::provider_catalog::resolve_openai_compatible_profile_selection(trimmed)
        {
            return Some(ConfigProviderSelection::OpenAiCompatibleProfile(profile.id));
        }

        if cfg.providers.contains_key(trimmed) {
            return Some(ConfigProviderSelection::NamedProfile(trimmed.to_string()));
        }

        Self::parse_provider_hint(trimmed).map(ConfigProviderSelection::BuiltIn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_provider_defaults_are_canonical_config_keys() {
        assert_eq!(
            MultiProvider::config_default_provider_for_login_provider(
                crate::provider_catalog::CLAUDE_LOGIN_PROVIDER,
            ),
            Some("claude")
        );
        assert_eq!(
            MultiProvider::config_default_provider_for_login_provider(
                crate::provider_catalog::OPENAI_LOGIN_PROVIDER,
            ),
            Some("openai")
        );
        assert_eq!(
            MultiProvider::config_default_provider_for_login_provider(
                crate::provider_catalog::OPENAI_API_LOGIN_PROVIDER,
            ),
            Some("openai")
        );
        assert_eq!(
            MultiProvider::config_default_provider_for_login_provider(
                crate::provider_catalog::OPENCODE_LOGIN_PROVIDER,
            ),
            Some("opencode")
        );
        assert_eq!(
            MultiProvider::config_default_provider_for_login_provider(
                crate::provider_catalog::AZURE_LOGIN_PROVIDER,
            ),
            None
        );
    }

    #[test]
    fn route_defaults_are_derived_consistently() {
        let copilot = MultiProvider::default_model_selection_from_route(
            "gpt-5.1-codex",
            "copilot",
            "GitHub Copilot",
        );
        assert_eq!(copilot.model_spec, "copilot:gpt-5.1-codex");
        assert_eq!(copilot.provider_key.as_deref(), Some("copilot"));

        let bedrock = MultiProvider::default_model_selection_from_route(
            "arn:aws:bedrock:us-east-1:123:inference-profile/foo",
            "bedrock",
            "AWS Bedrock",
        );
        assert_eq!(
            bedrock.model_spec,
            "bedrock:arn:aws:bedrock:us-east-1:123:inference-profile/foo"
        );
        assert_eq!(bedrock.provider_key.as_deref(), Some("bedrock"));

        let profile = MultiProvider::default_model_selection_from_route(
            "moonshot-v1-8k",
            "openai-compatible:kimi",
            "Kimi",
        );
        assert_eq!(profile.model_spec, "moonshot-v1-8k");
        assert_eq!(profile.provider_key.as_deref(), Some("kimi"));

        let openrouter = MultiProvider::default_model_selection_from_route(
            "claude-sonnet-4-5",
            "openrouter",
            "anthropic",
        );
        assert_eq!(
            openrouter.model_spec,
            "anthropic/claude-sonnet-4-5@anthropic"
        );
        assert_eq!(openrouter.provider_key.as_deref(), Some("openrouter"));
    }

    #[test]
    fn config_provider_resolution_handles_all_config_namespaces() {
        let mut cfg = crate::config::Config::default();
        cfg.providers.insert(
            "my-api".to_string(),
            crate::config::NamedProviderConfig::default(),
        );

        assert_eq!(
            MultiProvider::resolve_config_provider_selection("claude", &cfg)
                .map(|selection| selection.active_provider()),
            Some(ActiveProvider::Claude)
        );
        assert_eq!(
            MultiProvider::resolve_config_provider_selection("kimi", &cfg)
                .map(|selection| selection.active_provider()),
            Some(ActiveProvider::OpenRouter)
        );
        assert_eq!(
            MultiProvider::resolve_config_provider_selection("my-api", &cfg)
                .map(|selection| selection.active_provider()),
            Some(ActiveProvider::OpenRouter)
        );
        assert!(MultiProvider::resolve_config_provider_selection("unknown", &cfg).is_none());
    }
}
