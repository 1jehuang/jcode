#[test]
fn auth_provider_hint_maps_openai_compatible_login_providers() {
    assert_eq!(
        auth_provider_hint_for_login_provider("Azure OpenAI"),
        Some("azure-openai")
    );
    assert_eq!(
        auth_provider_hint_for_login_provider("cerebras"),
        Some("cerebras")
    );
    assert_eq!(
        auth_provider_hint_for_login_provider("Cerebras"),
        Some("cerebras")
    );
    assert_eq!(
        auth_provider_hint_for_login_provider("minimax"),
        Some("minimax")
    );
    assert_eq!(
        auth_provider_hint_for_login_provider("not-a-provider"),
        None
    );
}

#[test]
fn auth_provider_hint_maps_direct_provider_logins_by_display_label() {
    // LoginCompleted carries the descriptor display label, which must still map
    // to the canonical server provider id so the auth-change refresh is
    // attributed correctly (regression: an Anthropic API-key login used to send
    // no hint, so the server reported "OpenAI credentials are active" and
    // skipped the post-login model switch).
    assert_eq!(
        auth_provider_hint_for_login_provider("Anthropic API"),
        Some("anthropic-api")
    );
    assert_eq!(
        auth_provider_hint_for_login_provider("anthropic-api"),
        Some("anthropic-api")
    );
    assert_eq!(
        auth_provider_hint_for_login_provider("claude-api"),
        Some("anthropic-api")
    );
    assert_eq!(
        auth_provider_hint_for_login_provider("Anthropic/Claude"),
        Some("claude")
    );
    assert_eq!(
        auth_provider_hint_for_login_provider("claude"),
        Some("claude")
    );
    assert_eq!(
        auth_provider_hint_for_login_provider("OpenAI"),
        Some("openai")
    );
    assert_eq!(
        auth_provider_hint_for_login_provider("OpenAI API"),
        Some("openai-api")
    );
    assert_eq!(
        auth_provider_hint_for_login_provider("OpenRouter"),
        Some("openrouter")
    );
    assert_eq!(
        auth_provider_hint_for_login_provider("AWS Bedrock"),
        Some("bedrock")
    );
}

#[test]
fn auth_provider_hint_resolves_every_emitted_login_completed_provider() {
    // Every string published as `LoginCompleted.provider` (see the emit sites in
    // src/tui/app/auth.rs) must resolve to a canonical server provider id so the
    // auth-change refresh is attributed to the right provider and the post-login
    // model auto-select runs. Before the loose display-name resolution, only
    // Azure and OpenAI-compatible logins resolved; every direct provider sent no
    // hint, so the server fell back to the session's active provider (the
    // "OpenAI credentials are active" bug) and skipped the model switch.
    //
    // Pairs of (emitted string, expected canonical hint). `None` is only correct
    // for auto-import, which intentionally has no single runtime to attribute to.
    let cases: &[(&str, Option<&str>)] = &[
        // OAuth logins emit lowercase descriptor ids.
        ("openai", Some("openai")),
        ("claude", Some("claude")),
        ("gemini", Some("gemini")),
        ("copilot", Some("copilot")),
        ("antigravity", Some("antigravity")),
        ("cursor", Some("cursor")),
        // API-key paste logins emit descriptor display labels.
        ("Anthropic API", Some("anthropic-api")),
        ("OpenAI API", Some("openai-api")),
        ("AWS Bedrock", Some("bedrock")),
        ("OpenRouter", Some("openrouter")),
        // Azure keeps its dedicated runtime id mapping.
        ("Azure OpenAI", Some("azure-openai")),
        // Auto-import has no single runtime to attribute the refresh to.
        ("auto-import", None),
    ];

    for (emitted, expected) in cases {
        assert_eq!(
            auth_provider_hint_for_login_provider(emitted),
            *expected,
            "login provider {emitted:?} should resolve to {expected:?}"
        );
    }

    // Every login provider descriptor must resolve to the EXACT canonical hint
    // implied by its target, across its display label, id, and every alias.
    // Asserting the exact value (not just `is_some`) is what catches a
    // wrong-attribution bug like the original "OpenAI credentials are active"
    // after an Anthropic login - a weak `is_some` check would have passed even
    // while the hint pointed at the wrong provider.
    use crate::provider_catalog::LoginProviderTarget;
    for descriptor in crate::provider_catalog::login_providers() {
        // Expected hint mirrors auth_provider_hint_for_login_provider's target
        // mapping, the single source of truth for post-login attribution.
        let expected: Option<String> = match descriptor.target {
            LoginProviderTarget::AutoImport => None,
            LoginProviderTarget::Azure => Some("azure-openai".to_string()),
            LoginProviderTarget::OpenAiCompatible(profile) => Some(profile.id.to_string()),
            _ => Some(descriptor.id.to_string()),
        };

        // The emitted string can be the descriptor id, its display label, or any
        // alias (LoginCompleted.provider varies by surface/auth path).
        let mut emitted: Vec<&str> = vec![descriptor.id, descriptor.display_name];
        emitted.extend_from_slice(descriptor.aliases);
        for label in emitted {
            assert_eq!(
                auth_provider_hint_for_login_provider(label),
                expected.as_deref(),
                "login provider {:?} (id {:?}, target {:?}) emitted as {label:?} must attribute \
                 to {expected:?}; a wrong/missing hint mislabels the catalog-refresh message and \
                 skips the post-login model switch",
                descriptor.display_name,
                descriptor.id,
                descriptor.target
            );
        }
    }
}

#[test]
fn auth_changed_event_for_anthropic_api_login_targets_claude_api_route() {
    let auth = super::auth_changed_event_for_login_provider("Anthropic API")
        .expect("Anthropic API login should produce a typed auth event");
    // The server maps the descriptor id `anthropic-api` to the `claude-api`
    // route family for model selection and labelling.
    assert_eq!(auth.provider.as_str(), "anthropic-api");
    assert_eq!(
        auth.auth_method,
        Some(crate::protocol::AuthMethod::RemoteTuiPasteApiKey)
    );
    assert_eq!(
        auth.credential_source,
        Some(crate::protocol::AuthCredentialSource::ApiKeyFile)
    );
    // Direct providers must not claim the OpenAI-compatible runtime/namespace.
    assert!(auth.expected_runtime.is_none());
    assert!(auth.expected_catalog_namespace.is_none());
}

#[test]
fn auth_changed_event_for_oauth_claude_login_is_not_marked_as_api_key_paste() {
    let auth = super::auth_changed_event_for_login_provider("claude")
        .expect("Claude OAuth login should produce a typed auth event");
    assert_eq!(auth.provider.as_str(), "claude");
    // OAuth logins are not API-key pastes.
    assert!(auth.auth_method.is_none());
    assert!(auth.credential_source.is_none());
}

#[test]
fn auth_changed_event_for_cerebras_login_carries_runtime_and_catalog_identity() {
    let auth = super::auth_changed_event_for_login_provider("Cerebras")
        .expect("Cerebras login should produce typed auth event");

    assert_eq!(auth.provider.as_str(), "cerebras");
    assert_eq!(
        auth.credential_source,
        Some(crate::protocol::AuthCredentialSource::ApiKeyFile)
    );
    assert_eq!(
        auth.auth_method,
        Some(crate::protocol::AuthMethod::RemoteTuiPasteApiKey)
    );
    assert_eq!(
        auth.expected_runtime
            .as_ref()
            .map(crate::protocol::RuntimeProviderKey::as_str),
        Some("openai-compatible")
    );
    assert_eq!(
        auth.expected_catalog_namespace
            .as_ref()
            .map(crate::protocol::CatalogNamespace::as_str),
        Some("cerebras")
    );
}
