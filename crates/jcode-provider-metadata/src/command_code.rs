//! Command Code provider metadata.
//!
//! Command Code's public Provider API is documented at
//! <https://commandcode.ai/docs/provider>. It exposes OpenAI Chat Completions,
//! OpenAI Responses, and Anthropic Messages wire formats behind one API key.
//!
//! The static model list is an offline fallback, not a release allowlist. The
//! live `GET /provider/v1/models` catalog is authoritative and replaces it on a
//! successful refresh. Observation date: 2026-11-10 UTC, 97 models.

use super::{
    LoginProviderAuthKind, LoginProviderAuthStateKey, LoginProviderDescriptor,
    LoginProviderSurfaceOrder, LoginProviderTarget, OpenAiCompatibleProfile,
};

pub const COMMAND_CODE_PROFILE: OpenAiCompatibleProfile = OpenAiCompatibleProfile {
    id: "command-code",
    display_name: "Command Code",
    api_base: "https://api.commandcode.ai/provider/v1",
    api_key_env: "COMMAND_CODE_API_KEY",
    env_file: "command-code.env",
    setup_url: "https://commandcode.ai/docs/provider",
    default_model: Some("deepseek/deepseek-v4-flash"),
    requires_api_key: true,
};

pub const COMMAND_CODE_LOGIN_PROVIDER: LoginProviderDescriptor = LoginProviderDescriptor {
    id: "command-code",
    display_name: "Command Code",
    auth_kind: LoginProviderAuthKind::Hybrid,
    auth_state_key: LoginProviderAuthStateKey::OpenRouterLike,
    auth_status_method: "API key",
    aliases: &["commandcode", "cmdc"],
    menu_detail: "API key, official OpenAI and Anthropic wire endpoints",
    recommended: false,
    target: LoginProviderTarget::OpenAiCompatible(COMMAND_CODE_PROFILE),
    order: LoginProviderSurfaceOrder::new(Some(15), Some(15), Some(15), Some(15), Some(15)),
};

/// A dated offline fallback. Tuple fields are model id, context window, and
/// the endpoints the model advertised in the observed catalog.
pub const COMMAND_CODE_STATIC_MODELS: &[(&str, usize, &[&str])] = &[
    (
        "deepseek/deepseek-v4-flash",
        1_000_000,
        &["/chat/completions", "/responses"],
    ),
    ("zai-org/GLM-5.3", 1_000_000, &["/chat/completions"]),
    (
        "moonshotai/Kimi-K3",
        1_000_000,
        &["/chat/completions", "/responses"],
    ),
    (
        "meta/muse-ember",
        1_048_576,
        &["/chat/completions", "/responses"],
    ),
    ("claude-sonnet-5", 1_000_000, &["/messages"]),
];

pub fn static_model_ids() -> Vec<String> {
    COMMAND_CODE_STATIC_MODELS
        .iter()
        .map(|(model, _, _)| (*model).to_string())
        .collect()
}

pub fn static_context_limit(model: &str) -> Option<usize> {
    COMMAND_CODE_STATIC_MODELS
        .iter()
        .find(|(id, _, _)| id.eq_ignore_ascii_case(model))
        .map(|(_, context, _)| *context)
}

pub fn static_supported_endpoints(model: &str) -> Option<&'static [&'static str]> {
    COMMAND_CODE_STATIC_MODELS
        .iter()
        .find(|(id, _, _)| *id == model)
        .map(|(_, _, endpoints)| *endpoints)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve_login_provider;

    #[test]
    fn command_code_profile_and_login_provider_resolve() {
        assert_eq!(COMMAND_CODE_PROFILE.id, "command-code");
        assert_eq!(
            COMMAND_CODE_PROFILE.api_base,
            "https://api.commandcode.ai/provider/v1"
        );
        assert_eq!(COMMAND_CODE_PROFILE.api_key_env, "COMMAND_CODE_API_KEY");
        assert_eq!(
            COMMAND_CODE_PROFILE.default_model,
            Some("deepseek/deepseek-v4-flash")
        );

        let provider = resolve_login_provider("command-code").expect("login provider");
        assert_eq!(provider.id, "command-code");
        assert_eq!(provider.display_name, "Command Code");
        assert_eq!(provider.auth_kind, LoginProviderAuthKind::Hybrid);
        assert_eq!(
            provider.target,
            LoginProviderTarget::OpenAiCompatible(COMMAND_CODE_PROFILE)
        );
    }

    #[test]
    fn static_fallback_carries_endpoints_and_case_sensitive_ids() {
        assert_eq!(
            static_supported_endpoints("deepseek/deepseek-v4-flash"),
            Some(&["/chat/completions", "/responses"][..])
        );
        assert_eq!(
            static_supported_endpoints("claude-sonnet-5"),
            Some(&["/messages"][..])
        );
        assert_eq!(static_context_limit("meta/muse-ember"), Some(1_048_576));
        assert_eq!(static_context_limit("Meta/Muse-Ember"), Some(1_048_576));
        assert_eq!(static_model_ids().len(), COMMAND_CODE_STATIC_MODELS.len());
    }
}
