//! Pick the "next best" model route to fall back to after a provider error.
//!
//! When the active model/route fails (auth error, rate limit, provider outage,
//! a broken API key while an OAuth login still works, ...), the UI offers the
//! user a one-keypress switch to a usable alternative and resends the turn. This
//! module is the single, pure source of truth for *which* alternative to offer.
//!
//! It is intentionally provider-agnostic and side-effect free so it can be unit
//! tested exhaustively and reused by both the local and remote TUI paths.

use crate::{ModelRoute, ModelRouteApiMethod, model_route_provider_labels_match};

/// Whether an api_method authenticates via an OAuth / subscription login rather
/// than a metered API key. Subscription logins are preferred as fallbacks: they
/// are the most likely path to "just work" when an API key is broken/unfunded,
/// and they bill against a flat subscription instead of per-token cost.
fn api_method_is_oauth(api_method: &ModelRouteApiMethod) -> bool {
    matches!(
        api_method,
        ModelRouteApiMethod::ClaudeOAuth
            | ModelRouteApiMethod::OpenAIOAuth
            | ModelRouteApiMethod::CodeAssistOAuth
    )
}

fn models_match(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

fn api_methods_match(a: &str, b: &str) -> bool {
    ModelRouteApiMethod::parse(a) == ModelRouteApiMethod::parse(b)
}

/// Extra context about the failure that triggered the fallback search.
#[derive(Debug, Clone, Copy, Default)]
pub struct FallbackPickOptions {
    /// The failure was a credential/auth failure (expired OAuth session,
    /// invalid API key, failed token refresh, ...). Every route that uses the
    /// same credential is equally broken, so:
    /// - when the failed api_method is known, all same-provider routes with
    ///   that method are excluded (not just the same model), and
    /// - when the failed api_method is unknown, all same-provider routes are
    ///   excluded because any of them could share the broken credential.
    pub credential_failure: bool,
}

/// Whether `error` looks like a credential/auth failure for the active route's
/// provider account (expired or revoked OAuth session, failed token refresh,
/// invalid API key). Used to widen the fallback exclusion set: a broken
/// credential breaks every model behind it, so offering a sibling model on the
/// same credential would fail identically.
pub fn error_looks_like_credential_failure(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    let markers = [
        "token refresh failed",
        "refresh_token_invalidated",
        "re-authenticate",
        "session has ended",
        "authentication_error",
        "invalid_grant",
        "invalid x-api-key",
        "invalid api key",
        "incorrect api key",
        "api key not valid",
        "token expired",
        "unauthorized",
        "oauth token expired",
        "no refresh token",
        "credentials have been revoked",
        "please log in again",
        "run /login",
    ];
    markers.iter().any(|marker| lower.contains(marker))
}

/// Pick the index of the best alternative route to fall back to, given the
/// currently-selected route that just failed.
///
/// Ranking (lower tier wins; ties broken to keep the result stable and to
/// prefer subscription logins, then original catalog order):
///
/// 1. **Same model, different auth method** - e.g. the active `claude-api`
///    route's key is broken but a `claude-oauth` login for the *same* model is
///    available. This is the least disruptive switch (identical model, just a
///    different credential) and is exactly the case cross-provider failover
///    cannot currently handle.
/// 2. **Same provider, different model** - stay on the provider the user chose
///    when only a sibling model is usable.
/// 3. **Different provider** - last resort cross-provider hop.
///
/// Returns `None` when no *available* route other than the current one exists.
pub fn pick_next_fallback_route(
    routes: &[ModelRoute],
    current_model: &str,
    current_provider: &str,
    current_api_method: &str,
) -> Option<usize> {
    pick_next_fallback_route_with_options(
        routes,
        current_model,
        current_provider,
        current_api_method,
        FallbackPickOptions::default(),
    )
}

/// [`pick_next_fallback_route`] with failure-classification options.
pub fn pick_next_fallback_route_with_options(
    routes: &[ModelRoute],
    current_model: &str,
    current_provider: &str,
    current_api_method: &str,
    options: FallbackPickOptions,
) -> Option<usize> {
    let unknown_method = current_api_method.trim().is_empty();
    routes
        .iter()
        .enumerate()
        .filter(|(_, route)| route.available)
        .filter_map(|(index, route)| {
            let same_model = models_match(&route.model, current_model);
            let same_method = api_methods_match(&route.api_method, current_api_method);
            let same_provider =
                model_route_provider_labels_match(&route.provider, current_provider)
                    || models_match(&route.provider, current_provider);

            // Never offer the exact route that just failed. When the caller does
            // not know which auth method the failed route used (empty
            // `current_api_method`, e.g. a remote session without route
            // bookkeeping), any same-model route on the same provider could be
            // that exact failed route, so skip those too instead of offering
            // the user a "fallback" that is guaranteed to fail identically.
            if same_model && same_provider && (same_method || unknown_method) {
                return None;
            }

            // A credential failure breaks every route that authenticates with
            // the same credential, not just the failed model. Skip them all so
            // the offer is a route that can plausibly work.
            if options.credential_failure && same_provider && (same_method || unknown_method) {
                return None;
            }

            let tier = if same_model && !same_method {
                0
            } else if same_provider {
                1
            } else {
                2
            };

            // Within a tier, prefer subscription (OAuth) logins, then preserve
            // the catalog's original ordering for determinism.
            let prefers_oauth = u8::from(!api_method_is_oauth(&ModelRouteApiMethod::parse(
                &route.api_method,
            )));

            Some((tier, prefers_oauth, index))
        })
        .min()
        .map(|(_, _, index)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(model: &str, provider: &str, api_method: &str, available: bool) -> ModelRoute {
        ModelRoute {
            model: model.to_string(),
            provider: provider.to_string(),
            api_method: api_method.to_string(),
            available,
            detail: String::new(),
            usage: None,
            cheapness: None,
        }
    }

    #[test]
    fn prefers_same_model_oauth_when_api_key_broken() {
        // The user's exact case: active Claude API-key route is broken, but the
        // same Claude model is reachable via an OAuth login.
        let routes = vec![
            route("claude-sonnet-4", "Anthropic", "claude-api", true),
            route("claude-sonnet-4", "Anthropic", "claude-oauth", true),
            route("gpt-5", "OpenAI", "openai-oauth", true),
        ];
        let pick = pick_next_fallback_route(&routes, "claude-sonnet-4", "Anthropic", "claude-api")
            .expect("a fallback should exist");
        assert_eq!(routes[pick].api_method, "claude-oauth");
        assert_eq!(routes[pick].model, "claude-sonnet-4");
    }

    #[test]
    fn falls_back_to_same_provider_sibling_model() {
        let routes = vec![
            route("claude-opus-4", "Anthropic", "claude-oauth", true),
            route("claude-sonnet-4", "Anthropic", "claude-oauth", true),
        ];
        // Active opus route failed and has no alternate method; offer the
        // sibling Anthropic model rather than jumping providers.
        let pick = pick_next_fallback_route(&routes, "claude-opus-4", "Anthropic", "claude-oauth")
            .expect("a fallback should exist");
        assert_eq!(routes[pick].model, "claude-sonnet-4");
        assert_eq!(routes[pick].provider, "Anthropic");
    }

    #[test]
    fn falls_back_cross_provider_as_last_resort() {
        let routes = vec![
            route("claude-sonnet-4", "Anthropic", "claude-oauth", true),
            route("gpt-5", "OpenAI", "openai-oauth", true),
        ];
        let pick =
            pick_next_fallback_route(&routes, "claude-sonnet-4", "Anthropic", "claude-oauth")
                .expect("a fallback should exist");
        assert_eq!(routes[pick].provider, "OpenAI");
    }

    #[test]
    fn skips_unavailable_routes() {
        let routes = vec![
            route("claude-sonnet-4", "Anthropic", "claude-api", true),
            route("claude-sonnet-4", "Anthropic", "claude-oauth", false),
            route("gpt-5", "OpenAI", "openai-oauth", false),
        ];
        // The only same-model alt is unavailable and the cross-provider option
        // is unavailable too, so there is nothing to offer.
        assert!(
            pick_next_fallback_route(&routes, "claude-sonnet-4", "Anthropic", "claude-api")
                .is_none()
        );
    }

    #[test]
    fn returns_none_when_only_current_route_exists() {
        let routes = vec![route("gpt-5", "OpenAI", "openai-oauth", true)];
        assert!(pick_next_fallback_route(&routes, "gpt-5", "OpenAI", "openai-oauth").is_none());
    }

    #[test]
    fn cross_provider_prefers_oauth_over_api_key() {
        let routes = vec![
            route("claude-sonnet-4", "Anthropic", "claude-oauth", true),
            route("gpt-5", "OpenAI", "openai-api", true),
            route("gpt-5", "OpenAI", "openai-oauth", true),
        ];
        let pick =
            pick_next_fallback_route(&routes, "claude-sonnet-4", "Anthropic", "claude-oauth")
                .expect("a fallback should exist");
        assert_eq!(routes[pick].provider, "OpenAI");
        assert_eq!(routes[pick].api_method, "openai-oauth");
    }

    #[test]
    fn unknown_method_never_offers_same_model_same_provider() {
        // Remote session without route bookkeeping: the failed api_method is
        // unknown, so a same-model/same-provider route could be the exact
        // failed route and must not be offered.
        let routes = vec![
            route("gpt-5.5", "OpenAI", "openai-oauth", true),
            route("claude-sonnet-4", "Anthropic", "claude-oauth", true),
        ];
        let pick = pick_next_fallback_route(&routes, "gpt-5.5", "OpenAI", "")
            .expect("a fallback should exist");
        assert_eq!(routes[pick].provider, "Anthropic");
    }

    #[test]
    fn credential_failure_skips_sibling_models_on_same_credential() {
        // The user's exact case: an expired OpenAI OAuth session. Every OpenAI
        // OAuth model is equally broken, so offer another provider instead of
        // a sibling model behind the same dead login.
        let routes = vec![
            route("gpt-5.5", "OpenAI", "openai-oauth", true),
            route("gpt-5.4", "OpenAI", "openai-oauth", true),
            route("claude-sonnet-4", "Anthropic", "claude-oauth", true),
        ];
        let options = FallbackPickOptions {
            credential_failure: true,
        };
        let pick = pick_next_fallback_route_with_options(
            &routes,
            "gpt-5.5",
            "OpenAI",
            "openai-oauth",
            options,
        )
        .expect("a fallback should exist");
        assert_eq!(routes[pick].provider, "Anthropic");
    }

    #[test]
    fn credential_failure_still_offers_other_method_same_provider() {
        // A broken OAuth session does not implicate the API key: same-model
        // different-method remains the best offer.
        let routes = vec![
            route("gpt-5.5", "OpenAI", "openai-oauth", true),
            route("gpt-5.5", "OpenAI", "openai-api", true),
        ];
        let options = FallbackPickOptions {
            credential_failure: true,
        };
        let pick = pick_next_fallback_route_with_options(
            &routes,
            "gpt-5.5",
            "OpenAI",
            "openai-oauth",
            options,
        )
        .expect("a fallback should exist");
        assert_eq!(routes[pick].api_method, "openai-api");
    }

    #[test]
    fn credential_failure_with_unknown_method_skips_whole_provider() {
        // Unknown failed method + credential failure: any same-provider route
        // could share the broken credential, so hop providers.
        let routes = vec![
            route("gpt-5.5", "OpenAI", "openai-oauth", true),
            route("gpt-5.5", "OpenAI", "openai-api", true),
            route("gpt-5.4", "OpenAI", "openai-oauth", true),
            route("claude-sonnet-4", "Anthropic", "claude-oauth", true),
        ];
        let options = FallbackPickOptions {
            credential_failure: true,
        };
        let pick = pick_next_fallback_route_with_options(&routes, "gpt-5.5", "OpenAI", "", options)
            .expect("a fallback should exist");
        assert_eq!(routes[pick].provider, "Anthropic");
    }

    #[test]
    fn classifies_credential_failures() {
        assert!(error_looks_like_credential_failure(
            "OpenAI token refresh failed; run /login to re-authenticate: refresh_token_invalidated"
        ));
        assert!(error_looks_like_credential_failure(
            "Your session has ended. Please log in again."
        ));
        assert!(error_looks_like_credential_failure(
            "Anthropic API error (401 Unauthorized): authentication_error invalid x-api-key"
        ));
        // Observed auth-failure wave: expired OAuth + revoked refresh token.
        // The wave detector depends on these exact shapes classifying true.
        assert!(error_looks_like_credential_failure(
            "task failed: Anthropic API error (401 Unauthorized)"
        ));
        assert!(error_looks_like_credential_failure(
            "invalid_grant: refresh token invalid"
        ));
        assert!(error_looks_like_credential_failure(
            "provider returned Unauthorized"
        ));
        assert!(!error_looks_like_credential_failure(
            "429 rate limit exceeded, retry after 30s"
        ));
        assert!(!error_looks_like_credential_failure(
            "500 internal server error"
        ));
    }
}
