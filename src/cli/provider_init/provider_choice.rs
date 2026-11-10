//! Provider-choice to login-provider mapping.

use crate::provider_catalog::{
    LoginProviderDescriptor, LoginProviderTarget, OpenAiCompatibleProfile,
};

use super::{PROVIDER_CHOICE_LOGIN_PROVIDERS, ProviderChoice};

pub fn profile_for_choice(choice: &ProviderChoice) -> Option<OpenAiCompatibleProfile> {
    match login_provider_for_choice(choice)?.target {
        LoginProviderTarget::OpenAiCompatible(profile) => Some(profile),
        _ => None,
    }
}

pub fn login_provider_for_choice(choice: &ProviderChoice) -> Option<LoginProviderDescriptor> {
    PROVIDER_CHOICE_LOGIN_PROVIDERS
        .iter()
        .find(|(candidate, _)| candidate == choice)
        .map(|(_, provider)| *provider)
}
