use super::*;

/// Resolve built-in profile credentials without mixing Muse subscription login
/// with user-created, pay-as-you-go Meta keys. Named profiles continue to use
/// the ordinary env/config resolver and never receive this stored credential.
pub fn load_openai_compatible_profile_api_key(
    profile: &ResolvedOpenAiCompatibleProfile,
) -> Option<String> {
    if profile.id == "muse-subscription" {
        if profile.api_base != "https://api.meta.ai/v1"
            || profile.api_key_env != "JCODE_MUSE_SUBSCRIPTION_API_KEY"
            || profile.env_file != "muse-subscription.env"
        {
            return None;
        }
        crate::auth::muse::ensure_enabled().ok()?;
        return crate::auth::muse::load_credentials()
            .ok()
            .map(|credentials| credentials.api_key);
    }
    load_api_key_from_env_or_config(&profile.api_key_env, &profile.env_file)
}

/// Resolve the shared compatible-transport key from its configured env name and
/// file. The Muse subscription route stores its minted key in a Jcode-managed
/// credential file rather than an env file, so credential presence checks must
/// consult that store too or a signed-in Muse account looks unconfigured.
pub fn load_runtime_api_key(key_name: &str, env_file: &str) -> Option<String> {
    if let Some(key) = load_api_key_from_env_or_config(key_name, env_file) {
        return Some(key);
    }
    if key_name == "JCODE_MUSE_SUBSCRIPTION_API_KEY" && env_file == "muse-subscription.env" {
        crate::auth::muse::ensure_enabled().ok()?;
        return crate::auth::muse::load_credentials()
            .ok()
            .map(|credentials| credentials.api_key);
    }
    None
}

pub fn openai_compatible_profile_is_configured(profile: OpenAiCompatibleProfile) -> bool {
    // A named profile or unrelated Meta API key must not make this native
    // subscription route appear configured.
    if profile.id == "muse-subscription" {
        return load_openai_compatible_profile_api_key(
            &resolve_openai_compatible_profile_endpoint(profile, None),
        )
        .is_some();
    }
    // When a named config profile (`[providers.<name>]`, selected via
    // `--provider-profile`) is active, its credentials live under the runtime
    // env vars set by `apply_named_provider_profile_env`, not the built-in
    // `openai-compatible.env`. Honor those first so auth-test does not report a
    // false `not_configured` for a correctly-configured named profile (#402).
    if let Some(configured) = active_named_provider_profile_is_configured() {
        return configured;
    }

    let resolved = resolve_openai_compatible_profile_endpoint(profile, None);
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
