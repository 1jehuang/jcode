use crate::provider::activation::RuntimeProviderId;
use crate::provider_catalog::{
    LoginProviderDescriptor, LoginProviderSurface, LoginProviderTarget, login_providers,
    resolve_login_provider,
};

/// Cross-surface auth metadata derived from the provider catalog.
///
/// The catalog remains the source of display/order/login metadata. This facade adds
/// runtime identity so CLI, TUI, auth status, and runtime activation can assert they
/// are talking about the same provider instead of each keeping independent string
/// mappings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthProviderIntegration {
    pub descriptor: LoginProviderDescriptor,
    pub runtime_id: Option<RuntimeProviderId>,
}

impl AuthProviderIntegration {
    pub fn id(self) -> &'static str {
        self.descriptor.id
    }

    pub fn supports_surface(self, surface: LoginProviderSurface) -> bool {
        self.descriptor.order.for_surface(surface).is_some()
    }

    pub fn runtime_key(self) -> Option<&'static str> {
        self.runtime_id.map(RuntimeProviderId::key)
    }
}

pub fn auth_provider_integrations() -> Vec<AuthProviderIntegration> {
    login_providers()
        .iter()
        .copied()
        .map(|descriptor| AuthProviderIntegration {
            descriptor,
            runtime_id: runtime_id_for_login_provider(descriptor),
        })
        .collect()
}

pub fn auth_provider_integration(provider_id_or_alias: &str) -> Option<AuthProviderIntegration> {
    let descriptor = resolve_login_provider(provider_id_or_alias)?;
    Some(AuthProviderIntegration {
        descriptor,
        runtime_id: runtime_id_for_login_provider(descriptor),
    })
}

pub fn runtime_id_for_login_provider(
    provider: LoginProviderDescriptor,
) -> Option<RuntimeProviderId> {
    match provider.target {
        LoginProviderTarget::AutoImport => Some(RuntimeProviderId::AutoImport),
        LoginProviderTarget::Jcode => Some(RuntimeProviderId::Jcode),
        LoginProviderTarget::Claude => Some(RuntimeProviderId::Claude),
        LoginProviderTarget::OpenAi => Some(RuntimeProviderId::OpenAi),
        LoginProviderTarget::OpenAiApiKey => Some(RuntimeProviderId::OpenAiApiKey),
        LoginProviderTarget::OpenRouter => Some(RuntimeProviderId::OpenRouter),
        LoginProviderTarget::Bedrock => Some(RuntimeProviderId::Bedrock),
        LoginProviderTarget::Azure => Some(RuntimeProviderId::AzureOpenAi),
        LoginProviderTarget::OpenAiCompatible(_) => Some(RuntimeProviderId::OpenAiCompatible),
        LoginProviderTarget::Cursor => Some(RuntimeProviderId::Cursor),
        LoginProviderTarget::Copilot => Some(RuntimeProviderId::Copilot),
        LoginProviderTarget::Gemini => Some(RuntimeProviderId::Gemini),
        LoginProviderTarget::Antigravity => Some(RuntimeProviderId::Antigravity),
        // Google/Gmail auth is for tool access, not model-runtime routing.
        LoginProviderTarget::Google => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_catalog::{
        AZURE_LOGIN_PROVIDER, GOOGLE_LOGIN_PROVIDER, OPENAI_COMPAT_LOGIN_PROVIDER,
    };
    use std::collections::HashSet;

    #[test]
    fn integration_registry_covers_login_catalog_once() {
        let integrations = auth_provider_integrations();
        assert_eq!(integrations.len(), login_providers().len());

        let mut ids = HashSet::new();
        for (integration, descriptor) in integrations.iter().zip(login_providers()) {
            assert_eq!(integration.descriptor, *descriptor);
            assert!(
                ids.insert(integration.id()),
                "duplicate integration id: {}",
                integration.id()
            );
            assert_eq!(
                auth_provider_integration(integration.id()),
                Some(*integration)
            );
        }
    }

    #[test]
    fn integration_registry_preserves_runtime_identity() {
        assert_eq!(
            runtime_id_for_login_provider(AZURE_LOGIN_PROVIDER),
            Some(RuntimeProviderId::AzureOpenAi)
        );
        assert_eq!(
            runtime_id_for_login_provider(OPENAI_COMPAT_LOGIN_PROVIDER),
            Some(RuntimeProviderId::OpenAiCompatible)
        );
        assert_eq!(runtime_id_for_login_provider(GOOGLE_LOGIN_PROVIDER), None);
    }

    #[test]
    fn integration_surface_support_matches_catalog_order() {
        let surfaces = [
            LoginProviderSurface::CliLogin,
            LoginProviderSurface::TuiLogin,
            LoginProviderSurface::ServerBootstrap,
            LoginProviderSurface::AutoInit,
            LoginProviderSurface::AuthStatus,
        ];

        for integration in auth_provider_integrations() {
            for surface in surfaces {
                assert_eq!(
                    integration.supports_surface(surface),
                    integration.descriptor.order.for_surface(surface).is_some(),
                    "surface mismatch for {} on {:?}",
                    integration.id(),
                    surface
                );
            }
        }
    }

    #[test]
    fn model_auth_status_providers_have_runtime_identity() {
        for integration in auth_provider_integrations() {
            if !integration.supports_surface(LoginProviderSurface::AuthStatus) {
                continue;
            }
            if matches!(integration.descriptor.target, LoginProviderTarget::Google) {
                assert_eq!(integration.runtime_id, None);
            } else {
                assert!(
                    integration.runtime_id.is_some(),
                    "auth status provider {} should have a runtime identity",
                    integration.id()
                );
            }
        }
    }
}
