use super::*;

/// The outcome of re-applying a persisted session's model on resume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionModelRestore {
    /// The model-switch request that was actually applied.
    pub request: String,
    /// Set when the persisted credential route could not be honored and the
    /// session was moved onto the credential the process can actually use.
    /// Callers must write these back to the session so every later reader
    /// (swarm spawn inheritance, headed clients, the next resume) sees the
    /// route the session really runs on instead of the stale one.
    pub healed_route: Option<HealedSessionRoute>,
}

/// Route metadata to persist after a stale credential pin was recovered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealedSessionRoute {
    pub stale_request: String,
    pub provider_key: Option<String>,
    pub route_api_method: Option<String>,
}

/// When a persisted request pins a dual-auth credential (`claude-api:`,
/// `claude-oauth:`, `openai-api:`, `openai-oauth:`) that the process cannot
/// use, compute the request that keeps the same provider and model on the
/// credential the process *can* use:
///
/// - `JCODE_RUNTIME_PROVIDER` pins the same provider with the other credential
///   (the operator moved the deployment from an API key to OAuth or back):
///   follow the runtime pin.
/// - No runtime pin for that provider: drop to the bare provider prefix
///   (`claude:` / `openai:`) so the runtime's automatic mode picks whichever
///   credential exists.
///
/// Returns `None` when the request pins nothing, or when the runtime pins the
/// very credential that failed (a genuine auth failure, not a stale session).
pub fn stale_credential_route_fallback(request: &str) -> Option<(String, HealedSessionRoute)> {
    let (_, prefix, bare) = explicit_model_provider_prefix(request.trim())?;
    let bare = bare.trim();
    if bare.is_empty() {
        return None;
    }
    let stale = jcode_provider_core::AuthRoute::parse_explicit_credential_prefix(prefix)?;
    match jcode_provider_core::runtime_env_auth_route()
        .filter(|runtime| runtime.provider == stale.provider)
    {
        Some(runtime) if runtime.mode == stale.mode => None,
        Some(runtime) => Some((
            format!("{}:{bare}", runtime.model_prefix()),
            HealedSessionRoute {
                stale_request: request.to_string(),
                provider_key: Some(runtime.session_provider_key().to_string()),
                route_api_method: Some(runtime.route_api_method().to_string()),
            },
        )),
        None => {
            let bare_prefix = stale.provider.bare_model_prefix();
            Some((
                format!("{bare_prefix}:{bare}"),
                HealedSessionRoute {
                    stale_request: request.to_string(),
                    provider_key: Some(bare_prefix.to_string()),
                    route_api_method: None,
                },
            ))
        }
    }
}

/// Re-apply a persisted session's model on resume, recovering from a stale
/// credential pin.
///
/// A session records the credential route it was created on. When the
/// deployment's credential later changes type (an Anthropic API key replaced
/// by a Claude subscription, or the reverse), the persisted pin names a
/// credential that no longer exists. Restoring through it fails, and before
/// this the failure was only logged: the session kept running on the provider
/// default while its metadata still claimed the dead route, so every swarm
/// worker that inherited it failed with "No Anthropic API key found".
pub fn restore_session_model(
    provider: &dyn Provider,
    model: &str,
    provider_key: Option<&str>,
    route_api_method: Option<&str>,
) -> Result<SessionModelRestore> {
    let request = MultiProvider::model_switch_request_for_session_route(
        model,
        provider_key,
        route_api_method,
    );
    let first_err = match set_model_with_auth_refresh(provider, &request) {
        Ok(()) => {
            return Ok(SessionModelRestore {
                request,
                healed_route: None,
            });
        }
        Err(err) => err,
    };
    let Some((fallback, healed)) = stale_credential_route_fallback(&request) else {
        return Err(first_err);
    };
    set_model_with_auth_refresh(provider, &fallback).map_err(|fallback_err| {
        anyhow!("{first_err} (fallback to '{fallback}' also failed: {fallback_err})")
    })?;
    crate::logging::warn(&format!(
        "Session credential route '{request}' is no longer usable ({first_err}); \
         restored on '{fallback}' and updated the session route"
    ));
    Ok(SessionModelRestore {
        request: fallback,
        healed_route: Some(healed),
    })
}

pub fn set_model_with_auth_refresh(provider: &dyn Provider, model: &str) -> Result<()> {
    match provider.set_model(model) {
        Ok(()) => Ok(()),
        Err(first_err) => {
            let first_message = first_err.to_string();
            crate::logging::auth_event(
                "auth_changed_retry_after_set_model_failure",
                provider.name(),
                &[("reason", first_message.as_str())],
            );
            // Use the preserve-current-provider variant: this is a retry for an
            // already-open session, so refreshing auth from disk must NOT swap a
            // user-defined named OpenAI-compatible profile slot for a generic
            // OpenRouter runtime (which would lose `profile_id` and re-introduce
            // the `<profile>:<model>` prefix on the wire). See #408.
            provider.on_auth_changed_preserve_current_provider();
            provider.set_model(model).map_err(|second_err| {
                anyhow::anyhow!(
                    "{} (retried after reloading auth from disk: {})",
                    first_message,
                    second_err
                )
            })
        }
    }
}
