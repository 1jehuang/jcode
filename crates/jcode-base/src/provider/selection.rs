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
            LoginProviderTarget::Claude | LoginProviderTarget::ClaudeApiKey => Some("claude"),
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
        let parsed = ModelRouteApiMethod::parse(api_method);
        match parsed {
            ModelRouteApiMethod::OpenAiCompatible {
                profile_id: Some(_),
            } => api_method
                .split_once(':')
                .map(|(_, profile_id)| profile_id.trim())
                .filter(|profile_id| !profile_id.is_empty()),
            ModelRouteApiMethod::OpenAiCompatible { profile_id: None } => {
                crate::provider_catalog::openai_compatible_profile_id_for_display_name(
                    provider_display,
                )
            }
            _ => None,
        }
    }

    pub fn default_model_selection_from_route(
        bare_name: &str,
        api_method: &str,
        provider_display: &str,
    ) -> DefaultModelSelection {
        let api_method_kind = ModelRouteApiMethod::parse(api_method);
        let profile_id = match &api_method_kind {
            ModelRouteApiMethod::OpenAiCompatible {
                profile_id: Some(profile_id),
            } => Some(profile_id.clone()),
            ModelRouteApiMethod::OpenAiCompatible { profile_id: None } => {
                crate::provider_catalog::openai_compatible_profile_id_for_display_name(
                    provider_display,
                )
                .map(ToOwned::to_owned)
            }
            _ => None,
        };
        let model_spec = match &api_method_kind {
            ModelRouteApiMethod::Copilot => format!("copilot:{}", bare_name),
            ModelRouteApiMethod::ClaudeOAuth => format!("claude-oauth:{}", bare_name),
            ModelRouteApiMethod::AnthropicApiKey if provider_display == "Anthropic" => {
                format!("claude-api:{}", bare_name)
            }
            ModelRouteApiMethod::Cursor => format!("cursor:{}", bare_name),
            ModelRouteApiMethod::Bedrock => format!("bedrock:{}", bare_name),
            ModelRouteApiMethod::OpenAIApiKey => format!("openai-api:{}", bare_name),
            ModelRouteApiMethod::OpenAIOAuth => format!("openai-oauth:{}", bare_name),
            _ if provider_display == "Antigravity" => format!("antigravity:{}", bare_name),
            ModelRouteApiMethod::OpenAiCompatible { .. } => bare_name.to_string(),
            ModelRouteApiMethod::OpenRouter if provider_display != "auto" => {
                let model_id = crate::provider::openrouter_catalog_model_id(bare_name)
                    .unwrap_or_else(|| bare_name.to_string());
                format!("{}@{}", model_id, provider_display)
            }
            _ => bare_name.to_string(),
        };

        let provider_key = match &api_method_kind {
            ModelRouteApiMethod::AnthropicApiKey
                if provider_display == "Anthropic"
                    && crate::provider::provider_for_model(bare_name) == Some("claude") =>
            {
                Some("claude-api".to_string())
            }
            ModelRouteApiMethod::ClaudeOAuth
                if crate::provider::provider_for_model(bare_name) == Some("claude") =>
            {
                Some("claude".to_string())
            }
            ModelRouteApiMethod::OpenAIApiKey => Some("openai-api".to_string()),
            ModelRouteApiMethod::OpenAIOAuth => Some("openai".to_string()),
            ModelRouteApiMethod::Copilot => Some("copilot".to_string()),
            ModelRouteApiMethod::Cursor => Some("cursor".to_string()),
            ModelRouteApiMethod::Bedrock => Some("bedrock".to_string()),
            ModelRouteApiMethod::Other(method)
                if method == "cli" && provider_display == "Antigravity" =>
            {
                Some("antigravity".to_string())
            }
            ModelRouteApiMethod::OpenRouter => Some("openrouter".to_string()),
            ModelRouteApiMethod::OpenAiCompatible { .. } => profile_id.clone(),
            _ => profile_id.clone(),
        };

        DefaultModelSelection {
            model_spec,
            provider_key,
        }
    }

    /// Canonicalize a persisted session `provider_key` into the legacy
    /// vocabulary the reconstruction helpers below understand.
    ///
    /// Two vocabularies persist into sessions and must be treated as
    /// equivalent, otherwise the OAuth-vs-API-key auth mode is silently lost on
    /// restore/model-switch:
    ///
    /// - Legacy `/model` + login path: `claude` / `claude-api` / `openai` /
    ///   `openai-api`.
    /// - Structured model-route picker (`RouteSelection::stable_id`):
    ///   `claude-oauth` / `anthropic-api-key` / `openai-oauth` /
    ///   `openai-api-key`.
    ///
    /// Both encode the same auth route; we fold the picker forms back onto the
    /// canonical keys so a session whose `provider_key` is `anthropic-api-key`
    /// (and whose `route_api_method` was not also persisted, e.g. inherited by a
    /// child/forked session) still reconstructs the Anthropic API-key route
    /// instead of falling through to Auto (which prefers OAuth).
    pub(crate) fn canonical_session_provider_key(provider_key: &str) -> &str {
        // Fold any dual-auth (Anthropic/OpenAI OAuth-vs-API) alias onto its
        // canonical session key via the single shared parser, so this never
        // drifts from the route/runtime vocabularies. Non-dual keys pass through.
        if let Some(route) = jcode_provider_core::AuthRoute::parse(provider_key) {
            return route.session_provider_key();
        }
        provider_key.trim()
    }

    fn explicit_session_provider_key_for_model_request(model_request: &str) -> Option<String> {
        let model_request = model_request.trim();
        if let Some((prefix, rest)) = model_request.split_once(':') {
            let prefix = prefix.trim();
            if !prefix.is_empty() && !rest.trim().is_empty() {
                // Dual-auth (Anthropic/OpenAI) prefixes fold onto their canonical
                // session key via the single shared parser.
                if let Some(route) = jcode_provider_core::AuthRoute::parse(prefix) {
                    return Some(route.session_provider_key().to_string());
                }
                match prefix {
                    "copilot" | "antigravity" | "gemini" | "cursor" | "bedrock" | "openrouter" => {
                        return Some(prefix.to_string());
                    }
                    _ => {
                        if crate::provider_catalog::resolve_openai_compatible_profile_selection(
                            prefix,
                        )
                        .is_some()
                            || crate::config::config().providers.contains_key(prefix)
                        {
                            return Some(prefix.to_string());
                        }
                    }
                }
            }
        }

        if model_request.contains('@') {
            return Some("openrouter".to_string());
        }

        None
    }

    pub fn session_provider_key_for_model_request(
        model_request: &str,
        provider_name: &str,
    ) -> Option<String> {
        if let Some(provider_key) =
            Self::explicit_session_provider_key_for_model_request(model_request)
        {
            return Some(provider_key);
        }

        Self::session_provider_key_from_provider_name(provider_name)
            .or_else(|| crate::session::derive_session_provider_key(provider_name))
    }

    pub fn session_provider_key_after_model_switch(
        model_request: &str,
        provider_name: &str,
        previous_provider_key: Option<&str>,
    ) -> Option<String> {
        if let Some(provider_key) =
            Self::explicit_session_provider_key_for_model_request(model_request)
        {
            return Some(provider_key);
        }

        if let Some(previous_provider_key) = previous_provider_key
            .map(str::trim)
            .filter(|provider_key| !provider_key.is_empty())
            && Self::session_provider_key_matches_provider_name(
                previous_provider_key,
                provider_name,
            )
        {
            return Some(previous_provider_key.to_string());
        }

        Self::session_provider_key_from_provider_name(provider_name)
            .or_else(|| crate::session::derive_session_provider_key(provider_name))
    }

    fn session_provider_key_from_provider_name(provider_name: &str) -> Option<String> {
        let normalized = provider_name.trim().to_ascii_lowercase();
        let key = match normalized.as_str() {
            "jcode" => "jcode",
            "anthropic" | "claude" | "claude cli" => "claude",
            "openai" => "openai",
            "github copilot" | "copilot" => "copilot",
            "openrouter" => "openrouter",
            "cursor" => "cursor",
            "gemini" | "google" => "gemini",
            "antigravity" => "antigravity",
            "bedrock" | "aws bedrock" => "bedrock",
            "" => return None,
            _ => return None,
        };
        Some(key.to_string())
    }

    fn session_provider_key_matches_provider_name(provider_key: &str, provider_name: &str) -> bool {
        let provider_key = Self::canonical_session_provider_key(provider_key.trim());
        let Some(derived) = Self::session_provider_key_from_provider_name(provider_name)
            .or_else(|| crate::session::derive_session_provider_key(provider_name))
        else {
            return false;
        };
        match derived.as_str() {
            "claude" => matches!(
                provider_key,
                "claude" | "claude-oauth" | "claude-api" | "anthropic"
            ),
            "openai" => matches!(provider_key, "openai" | "openai-oauth" | "openai-api"),
            "openrouter" => {
                provider_key == "openrouter"
                    || crate::provider_catalog::resolve_openai_compatible_profile_selection(
                        provider_key,
                    )
                    .is_some()
                    || crate::config::config().providers.contains_key(provider_key)
            }
            other => provider_key == other,
        }
    }

    pub fn model_switch_request_for_session_model(
        model: &str,
        provider_key: Option<&str>,
    ) -> String {
        let model = model.trim();
        if model.is_empty() {
            return String::new();
        }

        if crate::provider::explicit_model_provider_prefix(model).is_some() {
            return model.to_string();
        }

        if let Some((prefix, rest)) = model.split_once(':') {
            let prefix = prefix.trim();
            if !prefix.is_empty()
                && !rest.trim().is_empty()
                && (crate::provider_catalog::resolve_openai_compatible_profile_selection(prefix)
                    .is_some()
                    || crate::config::config().providers.contains_key(prefix))
            {
                return model.to_string();
            }
        }

        let Some(provider_key) = provider_key
            .map(str::trim)
            .filter(|provider_key| !provider_key.is_empty())
        else {
            return model.to_string();
        };
        // Fold the structured-picker vocabulary (`anthropic-api-key`,
        // `openai-oauth`, ...) onto the canonical keys so the OAuth-vs-API-key
        // route survives even when only `provider_key` was persisted (e.g. a
        // forked/child session that inherited it without `route_api_method`).
        let provider_key = Self::canonical_session_provider_key(provider_key);

        // Dual-auth keys map to their canonical model prefix via the single
        // shared parser, keeping the emitted prefix in lockstep with the parsers.
        if let Some(route) = jcode_provider_core::AuthRoute::parse(provider_key) {
            return format!("{}:{model}", route.model_prefix());
        }

        match provider_key {
            "copilot" | "antigravity" | "gemini" | "cursor" | "bedrock" | "openrouter" => {
                format!("{provider_key}:{model}")
            }
            _ => {
                if crate::provider_catalog::resolve_openai_compatible_profile_selection(
                    provider_key,
                )
                .is_some()
                    || crate::config::config().providers.contains_key(provider_key)
                {
                    format!("{provider_key}:{model}")
                } else {
                    model.to_string()
                }
            }
        }
    }

    pub fn model_switch_request_for_session_route(
        model: &str,
        provider_key: Option<&str>,
        route_api_method: Option<&str>,
    ) -> String {
        let model = model.trim();
        if model.is_empty() {
            return String::new();
        }
        if let Some(api_method) = route_api_method
            .map(str::trim)
            .filter(|api_method| !api_method.is_empty())
        {
            match ModelRouteApiMethod::parse(api_method) {
                ModelRouteApiMethod::ClaudeOAuth => return format!("claude-oauth:{model}"),
                ModelRouteApiMethod::AnthropicApiKey => return format!("claude-api:{model}"),
                ModelRouteApiMethod::OpenAIOAuth => return format!("openai-oauth:{model}"),
                ModelRouteApiMethod::OpenAIApiKey => return format!("openai-api:{model}"),
                ModelRouteApiMethod::OpenRouter => return format!("openrouter:{model}"),
                ModelRouteApiMethod::OpenAiCompatible {
                    profile_id: Some(profile_id),
                } => return format!("{profile_id}:{model}"),
                ModelRouteApiMethod::Copilot => return format!("copilot:{model}"),
                ModelRouteApiMethod::Cursor => return format!("cursor:{model}"),
                ModelRouteApiMethod::Bedrock => return format!("bedrock:{model}"),
                ModelRouteApiMethod::AntigravityHttps => return format!("antigravity:{model}"),
                ModelRouteApiMethod::OpenAiCompatible { profile_id: None }
                | ModelRouteApiMethod::CodeAssistOAuth
                | ModelRouteApiMethod::RemoteCatalog
                | ModelRouteApiMethod::Current
                | ModelRouteApiMethod::Other(_) => {}
            }
        }

        Self::model_switch_request_for_session_model(model, provider_key)
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
    fn default_model_selection_preserves_route_identity_state_space() {
        for (bare, api_method, provider, expected_spec, expected_provider_key) in [
            (
                "gpt-5.5",
                "openai-oauth",
                "OpenAI",
                "openai-oauth:gpt-5.5",
                Some("openai"),
            ),
            (
                "gpt-5.5",
                "openai-api-key",
                "OpenAI",
                "openai-api:gpt-5.5",
                Some("openai-api"),
            ),
            (
                "claude-opus-4-6",
                "claude-oauth",
                "Anthropic",
                "claude-oauth:claude-opus-4-6",
                Some("claude"),
            ),
            (
                "claude-opus-4-6",
                "claude-api",
                "Anthropic",
                "claude-api:claude-opus-4-6",
                Some("claude-api"),
            ),
            (
                "glm-51-nvfp4",
                "openai-compatible:comtegra",
                "Comtegra GPU Cloud",
                "glm-51-nvfp4",
                Some("comtegra"),
            ),
            (
                "claude-sonnet-4-6",
                "copilot",
                "Copilot",
                "copilot:claude-sonnet-4-6",
                Some("copilot"),
            ),
        ] {
            let selection =
                MultiProvider::default_model_selection_from_route(bare, api_method, provider);
            assert_eq!(selection.model_spec, expected_spec, "{api_method}");
            assert_eq!(
                selection.provider_key.as_deref(),
                expected_provider_key,
                "{api_method}"
            );
        }
    }

    #[test]
    fn session_model_route_identity_helpers_preserve_auth_mode_and_profiles() {
        for (request, provider_name, previous_key, expected_key) in [
            ("openai-api:gpt-5.5", "OpenAI", None, Some("openai-api")),
            ("openai-oauth:gpt-5.5", "OpenAI", None, Some("openai")),
            (
                "claude-api:claude-opus-4-6",
                "Anthropic",
                None,
                Some("claude-api"),
            ),
            (
                "claude-oauth:claude-opus-4-6",
                "Anthropic",
                None,
                Some("claude"),
            ),
            (
                "cerebras:qwen-3-235b-a22b-instruct-2507",
                "OpenRouter",
                None,
                Some("cerebras"),
            ),
            ("gpt-5.5", "OpenAI", Some("openai-api"), Some("openai-api")),
            (
                "claude-opus-4-6",
                "Anthropic",
                Some("claude-api"),
                Some("claude-api"),
            ),
            (
                "qwen-3-235b-a22b-instruct-2507",
                "OpenRouter",
                Some("cerebras"),
                Some("cerebras"),
            ),
        ] {
            assert_eq!(
                MultiProvider::session_provider_key_after_model_switch(
                    request,
                    provider_name,
                    previous_key,
                )
                .as_deref(),
                expected_key,
                "{request} via {provider_name:?}"
            );
        }

        for (model, provider_key, expected_request) in [
            ("gpt-5.5", Some("openai-api"), "openai-api:gpt-5.5"),
            ("gpt-5.5", Some("openai"), "openai-oauth:gpt-5.5"),
            (
                "claude-opus-4-6",
                Some("claude-api"),
                "claude-api:claude-opus-4-6",
            ),
            (
                "claude-opus-4-6",
                Some("claude"),
                "claude-oauth:claude-opus-4-6",
            ),
            (
                "qwen-3-235b-a22b-instruct-2507",
                Some("cerebras"),
                "cerebras:qwen-3-235b-a22b-instruct-2507",
            ),
            ("openai-api:gpt-5.5", Some("openai"), "openai-api:gpt-5.5"),
        ] {
            assert_eq!(
                MultiProvider::model_switch_request_for_session_model(model, provider_key),
                expected_request,
                "restore {model:?} with {provider_key:?}"
            );
        }

        assert_eq!(
            MultiProvider::model_switch_request_for_session_route(
                "openrouter/owl-alpha",
                Some("openrouter"),
                Some("openrouter"),
            ),
            "openrouter:openrouter/owl-alpha"
        );
        assert_eq!(
            MultiProvider::model_switch_request_for_session_route(
                "nvidia/example",
                Some("openai-compatible:nvidia-nim"),
                Some("openai-compatible:nvidia-nim"),
            ),
            "nvidia-nim:nvidia/example"
        );
    }

    #[test]
    fn session_provider_key_picker_vocabulary_preserves_auth_mode_without_route() {
        // The structured model-route picker persists `RuntimeKey::stable_id()`
        // values (`anthropic-api-key`, `openai-oauth`, ...). When a child/forked
        // session inherits only `provider_key` without `route_api_method`, the
        // reconstruction helpers must still recover the exact OAuth-vs-API-key
        // route instead of dropping to Auto (which prefers OAuth) and silently
        // shifting an API-key user onto the subscription.
        for (model, provider_key, expected_request) in [
            (
                "claude-opus-4-8",
                Some("anthropic-api-key"),
                "claude-api:claude-opus-4-8",
            ),
            (
                "claude-opus-4-8",
                Some("claude-oauth"),
                "claude-oauth:claude-opus-4-8",
            ),
            ("gpt-5.5", Some("openai-api-key"), "openai-api:gpt-5.5"),
            ("gpt-5.5", Some("openai-oauth"), "openai-oauth:gpt-5.5"),
        ] {
            assert_eq!(
                MultiProvider::model_switch_request_for_session_model(model, provider_key),
                expected_request,
                "restore {model:?} with picker provider_key {provider_key:?}"
            );
        }

        // The same picker vocabulary must be recognized as matching its provider
        // so an auth-change rewrite keeps the persisted key instead of
        // overwriting it with the canonical name (losing the auth mode).
        for (model, provider_name, previous_key, expected_key) in [
            (
                "claude-opus-4-8",
                "Anthropic",
                Some("anthropic-api-key"),
                Some("anthropic-api-key"),
            ),
            (
                "gpt-5.5",
                "OpenAI",
                Some("openai-api-key"),
                Some("openai-api-key"),
            ),
        ] {
            assert_eq!(
                MultiProvider::session_provider_key_after_model_switch(
                    model,
                    provider_name,
                    previous_key,
                )
                .as_deref(),
                expected_key,
                "{model:?} via {provider_name:?} keeps picker key {previous_key:?}"
            );
        }
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

        let openrouter_openai =
            MultiProvider::default_model_selection_from_route("gpt-5.5", "openrouter", "OpenAI");
        assert_eq!(openrouter_openai.model_spec, "openai/gpt-5.5@OpenAI");
        assert_eq!(
            openrouter_openai.provider_key.as_deref(),
            Some("openrouter")
        );
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
