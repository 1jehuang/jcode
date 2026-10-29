use super::*;

pub fn activate_auth_change(request: &AuthActivationRequest) -> AuthActivationResult {
    let provider_id = request.provider_id();
    sync_process_env_from_saved_credentials(request, provider_id.as_deref());
    // The notification handler may have probed auth while the newly saved
    // credential was not yet reflected in the process environment. Discard
    // that snapshot after activation so catalog rebuilding sees the new auth.
    crate::auth::AuthStatus::invalidate_cache();
    let provider_label = provider_display_label(provider_id.as_deref());
    let activated_model = apply_auth_provider_runtime(provider_id.as_deref());
    AuthActivationResult {
        provider_id,
        provider_label,
        activated_model,
        expected_runtime: request
            .expected_runtime()
            .map(|runtime| runtime.as_str().to_string()),
        expected_catalog_namespace: request
            .expected_catalog_namespace()
            .map(|namespace| namespace.as_str().to_string()),
    }
}

/// Env keys and env-file names that persist API-key credentials for a
/// normalized auth provider id. Empty for OAuth/CLI providers whose
/// credentials live in token stores, not env vars.
pub(super) fn api_key_env_bindings_for_provider(provider_id: &str) -> Vec<(String, String)> {
    match provider_id {
        "claude-api" => vec![("ANTHROPIC_API_KEY".to_string(), "anthropic.env".to_string())],
        "openai-api" => vec![("OPENAI_API_KEY".to_string(), "openai.env".to_string())],
        "openrouter" => vec![(
            "OPENROUTER_API_KEY".to_string(),
            "openrouter.env".to_string(),
        )],
        "jcode" => vec![
            (
                crate::subscription_catalog::JCODE_API_KEY_ENV.to_string(),
                crate::subscription_catalog::JCODE_ENV_FILE.to_string(),
            ),
            (
                crate::subscription_catalog::JCODE_API_BASE_ENV.to_string(),
                crate::subscription_catalog::JCODE_ENV_FILE.to_string(),
            ),
        ],
        "bedrock" => vec![
            (
                crate::provider::bedrock::API_KEY_ENV.to_string(),
                crate::provider::bedrock::ENV_FILE.to_string(),
            ),
            (
                crate::provider::bedrock::REGION_ENV.to_string(),
                crate::provider::bedrock::ENV_FILE.to_string(),
            ),
        ],
        "cursor" => vec![("CURSOR_API_KEY".to_string(), "cursor.env".to_string())],
        "gemini" => crate::auth::gemini::GEMINI_API_KEY_ENV_VARS
            .iter()
            .map(|env_key| {
                (
                    env_key.to_string(),
                    crate::auth::gemini::GEMINI_API_KEY_ENV_FILE.to_string(),
                )
            })
            .collect(),
        "azure-openai" => vec![
            (
                crate::auth::azure::API_KEY_ENV.to_string(),
                crate::auth::azure::ENV_FILE.to_string(),
            ),
            (
                crate::auth::azure::ENDPOINT_ENV.to_string(),
                crate::auth::azure::ENV_FILE.to_string(),
            ),
            (
                crate::auth::azure::MODEL_ENV.to_string(),
                crate::auth::azure::ENV_FILE.to_string(),
            ),
        ],
        other => match crate::provider_catalog::openai_compatible_profile_by_id(other) {
            Some(profile) => {
                let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
                vec![(resolved.api_key_env, resolved.env_file)]
            }
            // Unknown providers have no saved API-key environment bindings.
            None => Vec::new(),
        },
    }
}

/// Make freshly saved credentials win over stale env vars inherited by this
/// process (issue #453).
///
/// `/login` persists API keys to the per-provider env file under the jcode
/// config dir, but credential resolution
/// ([`crate::provider_catalog::load_api_key_from_env_or_config`]) prefers the
/// process env var. A long-lived server that inherited a stale
/// `ANTHROPIC_API_KEY` (or similar) would therefore keep 401-ing forever even
/// though the login succeeded and the file holds a valid key. On an explicit
/// auth change, overwrite this process's env var with the env-file value when
/// the two diverge, so the just-saved credential is actually used.
pub(super) fn sync_process_env_from_saved_credentials(
    request: &AuthActivationRequest,
    provider_id: Option<&str>,
) {
    let Some(provider_id) = provider_id else {
        return;
    };
    // Only do this for auth changes that plausibly wrote an env file. OAuth
    // logins do not touch API-key env files, and process-env-preseeded auth
    // means the env var itself is the intended source of truth.
    let explicit_env_file_login = match request.auth.as_ref() {
        Some(auth) => {
            matches!(
                auth.credential_source,
                Some(crate::protocol::AuthCredentialSource::ApiKeyFile) | None
            ) && !matches!(
                auth.auth_method,
                Some(crate::protocol::AuthMethod::ProcessEnvPreseeded)
                    | Some(crate::protocol::AuthMethod::OAuthBrowser)
                    | Some(crate::protocol::AuthMethod::DeviceCode)
            )
        }
        // Legacy hint-only notifications carry no source metadata; syncing is
        // still the safe default because it only runs when the file has a
        // value that differs from the env var.
        None => true,
    };
    if !explicit_env_file_login {
        return;
    }

    for (env_key, env_file) in api_key_env_bindings_for_provider(provider_id) {
        let Some(file_value) =
            crate::provider_catalog::load_env_value_from_config_file(&env_key, &env_file)
        else {
            continue;
        };
        let env_value = match std::env::var(&env_key) {
            Ok(value) => Some(value),
            Err(std::env::VarError::NotPresent) => None,
            Err(std::env::VarError::NotUnicode(_)) => {
                crate::logging::warn(&format!(
                    "Invalid process credential variable {env_key}: non-Unicode value"
                ));
                None
            }
        };
        if env_value.as_deref() == Some(file_value.as_str()) {
            continue;
        }
        let had_stale_env = env_value.is_some();
        // ponytail: foreign-reader carve-out — a freshly /login'd key must reach
        // children and the AWS SDK (which cannot read our env files). Readers we
        // control are env-file aware (#1389); this sync replaces a stale inherited
        // var so those foreign readers see the fresh login (issue #453).
        crate::provider_catalog::track_process_env_var_override(&env_key);
        crate::env::set_var(&env_key, &file_value);
        crate::logging::auth_event(
            "auth_changed_env_synced_from_file",
            provider_id,
            &[
                ("env_key", env_key.as_str()),
                ("env_file", env_file.as_str()),
                (
                    "replaced",
                    if had_stale_env {
                        "stale_process_env"
                    } else {
                        "unset_process_env"
                    },
                ),
            ],
        );
    }
}

pub(super) fn apply_auth_provider_runtime(provider_id: Option<&str>) -> Option<String> {
    match normalized_auth_provider_id(provider_id) {
        Some("azure-openai") => match crate::provider::activation::apply_azure_openai_runtime() {
            Ok(model) => model,
            Err(error) => {
                let message = error.to_string();
                crate::logging::auth_event(
                    "auth_changed_runtime_activation_failed",
                    "azure-openai",
                    &[("reason", message.as_str())],
                );
                None
            }
        },
        Some(profile_id)
            if direct_provider_activation(profile_id).is_none()
                && crate::provider_catalog::openai_compatible_profile_by_id(profile_id)
                    .is_some() =>
        {
            let Some(profile) =
                crate::provider_catalog::openai_compatible_profile_by_id(profile_id)
            else {
                crate::logging::auth_event(
                    "auth_changed_runtime_activation_failed",
                    profile_id,
                    &[(
                        "reason",
                        "openai-compatible profile disappeared during activation",
                    )],
                );
                return None;
            };
            crate::provider_catalog::force_apply_openai_compatible_profile_env(Some(profile));
            let default_model =
                crate::provider_catalog::resolve_openai_compatible_profile(profile).default_model;
            if let Err(error) =
                crate::provider::activation::apply_openai_compatible_runtime(default_model.clone())
            {
                let message = error.to_string();
                crate::logging::auth_event(
                    "auth_changed_runtime_activation_failed",
                    profile_id,
                    &[("reason", message.as_str())],
                );
                None
            } else {
                default_model
            }
        }
        Some(provider_id) => {
            if let Some(activation) = direct_provider_activation(provider_id)
                && let Err(error) = activation.apply_env()
            {
                let message = error.to_string();
                crate::logging::auth_event(
                    "auth_changed_runtime_activation_failed",
                    provider_id,
                    &[("reason", message.as_str())],
                );
            }
            None
        }
        _ => None,
    }
}

pub(super) fn direct_provider_activation(provider_id: &str) -> Option<ProviderActivation> {
    let (runtime_id, active) = direct_provider_runtime_ids(provider_id)?;
    // A login must not override an explicit configured default for every
    // future session. Selecting the initial provider writes process-global env
    // (JCODE_ACTIVE_PROVIDER + JCODE_INITIAL_PROVIDER_EXPLICIT), and in the
    // shared daemon that env outranks `[provider] default_model`, so logging
    // into OpenAI once made every new session start on GPT instead of the
    // configured Claude default. The session that performed the login still
    // switches models through the session-local auth-change path.
    if config_has_explicit_default_provider() {
        return Some(ProviderActivation::new(
            runtime_id,
            crate::provider::activation::RuntimeSelection::Unchanged,
        ));
    }
    Some(ProviderActivation::initial(runtime_id, active))
}

pub(super) fn config_has_explicit_default_provider() -> bool {
    let provider_config = &crate::config::config().provider;
    let non_empty = |value: &Option<String>| {
        value
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
    };
    non_empty(&provider_config.default_provider) || non_empty(&provider_config.default_model)
}

pub(super) fn direct_provider_runtime_ids(
    provider_id: &str,
) -> Option<(RuntimeProviderId, ActiveProvider)> {
    Some(match normalized_login_provider_id(provider_id)? {
        "claude" => (RuntimeProviderId::Claude, ActiveProvider::Claude),
        "claude-api" => (RuntimeProviderId::ClaudeApiKey, ActiveProvider::Claude),
        "openai" => (RuntimeProviderId::OpenAi, ActiveProvider::OpenAI),
        "openai-api" => (RuntimeProviderId::OpenAiApiKey, ActiveProvider::OpenAI),
        "openrouter" => (RuntimeProviderId::OpenRouter, ActiveProvider::OpenRouter),
        "jcode" => (RuntimeProviderId::Jcode, ActiveProvider::OpenRouter),
        "bedrock" => (RuntimeProviderId::Bedrock, ActiveProvider::Bedrock),
        "cursor" => (RuntimeProviderId::Cursor, ActiveProvider::Cursor),
        "copilot" => (RuntimeProviderId::Copilot, ActiveProvider::Copilot),
        "gemini" => (RuntimeProviderId::Gemini, ActiveProvider::Gemini),
        "antigravity" => (RuntimeProviderId::Antigravity, ActiveProvider::Antigravity),
        "grok-build" => (RuntimeProviderId::GrokBuild, ActiveProvider::OpenRouter),
        _ => return None,
    })
}
