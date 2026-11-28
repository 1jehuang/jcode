//! Login-provider -> runtime provider id mapping used to route auth-refresh
//! events after a login completes.

pub(super) fn auth_provider_hint_for_login_provider(provider: &str) -> Option<&'static str> {
    let provider = provider.trim();
    // Azure's runtime id ("azure-openai") differs from its login descriptor id
    // ("azure"); keep the dedicated mapping used across the auth lifecycle.
    if provider.eq_ignore_ascii_case("azure")
        || provider.eq_ignore_ascii_case("azure-openai")
        || provider.eq_ignore_ascii_case("azure openai")
    {
        return Some("azure-openai");
    }

    use crate::provider_catalog::LoginProviderTarget;
    let descriptor = crate::provider_catalog::resolve_login_provider_loose(provider)?;
    match descriptor.target {
        LoginProviderTarget::Azure => Some("azure-openai"),
        // OpenAI-compatible profiles carry their own catalog namespace id.
        LoginProviderTarget::OpenAiCompatible(profile) => Some(profile.id),
        // Auto-import has no single runtime to attribute the refresh to.
        LoginProviderTarget::AutoImport => None,
        _ => Some(descriptor.id),
    }
}

pub(super) fn auth_changed_event_for_login_provider(
    provider: &str,
) -> Option<crate::protocol::AuthChanged> {
    use crate::provider_catalog::LoginProviderTarget;
    let provider_id = auth_provider_hint_for_login_provider(provider)?;
    let mut auth = crate::protocol::AuthChanged::new(provider_id);
    // These fields are informational; the server routes off `provider` and the
    // `expected_*` hints. Reflect the descriptor's auth kind so OAuth logins are
    // not recorded as API-key pastes.
    let descriptor = crate::provider_catalog::resolve_login_provider_loose(provider);
    let api_key_login = descriptor
        .map(|descriptor| {
            use crate::provider_catalog::LoginProviderAuthKind;
            matches!(
                descriptor.auth_kind,
                LoginProviderAuthKind::ApiKey | LoginProviderAuthKind::Hybrid
            )
        })
        .unwrap_or(true);
    if api_key_login {
        auth.auth_method = Some(crate::protocol::AuthMethod::RemoteTuiPasteApiKey);
        auth.credential_source = Some(crate::protocol::AuthCredentialSource::ApiKeyFile);
    }
    // Only logins whose descriptor actually targets the OpenAI-compatible
    // runtime claim its namespace. Do not key this off
    // `openai_compatible_profile_by_id`: native providers (`anthropic-api`,
    // `openai-api`) alias doctor-probe compat profiles with the same id, but
    // their auth activation deliberately routes through the native runtime.
    if provider_id == "azure-openai" {
        auth.expected_runtime = Some(crate::protocol::RuntimeProviderKey::new("azure-openai"));
        auth.expected_catalog_namespace =
            Some(crate::protocol::CatalogNamespace::new("azure-openai"));
    } else if descriptor
        .is_some_and(|d| matches!(d.target, LoginProviderTarget::OpenAiCompatible(_)))
    {
        auth.expected_runtime = Some(crate::protocol::RuntimeProviderKey::new(
            "openai-compatible",
        ));
        auth.expected_catalog_namespace = Some(crate::protocol::CatalogNamespace::new(provider_id));
    }
    Some(auth)
}
