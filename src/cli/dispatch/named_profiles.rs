use super::*;

pub(super) fn provider_choice_for_named_profile(profile_name: &str) -> ProviderChoice {
    // Same source as `apply_named_provider_profile_env`, which already
    // validated that the profile exists.
    match crate::config::Config::load_strict() {
        Ok(config) => provider_choice_for_named_profile_in(profile_name, &config),
        Err(_) => provider_choice_for_named_profile_in(profile_name, crate::config::config()),
    }
}

pub(super) fn provider_choice_for_named_profile_in(
    profile_name: &str,
    config: &crate::config::Config,
) -> ProviderChoice {
    match config.providers.get(profile_name) {
        Some(profile)
            if matches!(
                profile.provider_type,
                crate::config::NamedProviderType::AnthropicCompatible
            ) =>
        {
            ProviderChoice::Auto
        }
        _ => ProviderChoice::OpenaiCompatible,
    }
}
