//! Single source of truth for "which credential will this provider actually
//! use right now?".
//!
//! Several UI surfaces need to answer the same question -- the header provider
//! tag (`(oauth:claude) ...`), the info widget auth badge, and the model-switch
//! confirmation line. Historically each surface re-derived the answer from
//! [`AuthStatus`] plus the `JCODE_RUNTIME_PROVIDER` override on its own, keyed on
//! free-form provider strings (`"anthropic"` vs `"claude"`). That duplication is
//! exactly how the surfaces silently drifted apart and how the header tag
//! vanished entirely when one path matched `"claude"` and another `"anthropic"`.
//!
//! This module centralizes the decision so every surface stays in lockstep.
//! Presentation (a `&str` tag, an enum badge) is intentionally left to each
//! caller; only the *decision* lives here.

use crate::auth::AuthStatus;
use jcode_provider_core::ActiveProvider;

/// The credential a request will actually be sent with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActiveCredential {
    /// OAuth / subscription login (Claude subscription, Codex login, ...).
    OAuth,
    /// Direct provider API key.
    ApiKey,
}

impl From<jcode_provider_core::ResolvedCredential> for ActiveCredential {
    fn from(value: jcode_provider_core::ResolvedCredential) -> Self {
        match value {
            jcode_provider_core::ResolvedCredential::Oauth => Self::OAuth,
            jcode_provider_core::ResolvedCredential::ApiKey => Self::ApiKey,
        }
    }
}

/// Resolved auth picture for a provider that supports both OAuth and API-key
/// credentials (currently Anthropic and OpenAI).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedProviderAuth {
    /// Credential the next request will actually use.
    pub active: ActiveCredential,
    /// Whether an OAuth credential is configured at all.
    pub has_oauth: bool,
    /// Whether an API key is configured at all.
    pub has_api_key: bool,
    /// True when the active credential was pinned via `JCODE_RUNTIME_PROVIDER`
    /// rather than chosen by the auto heuristic. Lets surfaces distinguish a
    /// deliberate "use OAuth" from "auto happened to pick OAuth".
    pub explicit: bool,
}

impl ResolvedProviderAuth {
    /// True when both credential types are configured (the request still uses
    /// exactly one, reported by [`Self::active`]).
    pub fn has_both(&self) -> bool {
        self.has_oauth && self.has_api_key
    }
}

/// Resolve the credential a dual-auth provider (Anthropic / OpenAI) will use.
///
/// `runtime_provider` is the raw `JCODE_RUNTIME_PROVIDER` value (if any); it
/// lets the user pin OAuth-vs-key explicitly and always wins over the auto
/// heuristic. In "auto" mode we prefer OAuth (subscription) and fall back to the
/// API key, matching the credential the provider layer actually selects.
///
/// Returns `None` for providers without OAuth-vs-key ambiguity, or when neither
/// credential is configured.
pub fn resolve_dual_credential_auth(
    provider: ActiveProvider,
    auth: &AuthStatus,
    runtime_provider: Option<&str>,
) -> Option<ResolvedProviderAuth> {
    // Map the execution slot onto the canonical dual-auth provider. Anything
    // without an OAuth-vs-API decision (Copilot, Gemini, ...) returns None.
    let dual = jcode_provider_core::DualAuthProvider::from_active_provider(provider)?;

    // A single canonical parser decides whether `runtime_provider` explicitly
    // pins OAuth or API key for *this* provider. This replaces the per-provider
    // hand-written alias matches that used to drift apart.
    let forced =
        jcode_provider_core::pinned_mode_for(dual, runtime_provider).map(|mode| match mode {
            jcode_provider_core::AuthMode::Oauth => ActiveCredential::OAuth,
            jcode_provider_core::AuthMode::ApiKey => ActiveCredential::ApiKey,
        });

    let (has_oauth, has_api_key) = match dual {
        jcode_provider_core::DualAuthProvider::Anthropic => {
            let has_oauth = auth.anthropic.has_oauth;
            // `has_api_key` already folds in the ANTHROPIC_API_KEY env var via the
            // auth probe, but re-check defensively so an env-only key set after the
            // cached snapshot still reports honestly.
            let has_api_key =
                auth.anthropic.has_api_key || std::env::var("ANTHROPIC_API_KEY").is_ok();
            (has_oauth, has_api_key)
        }
        jcode_provider_core::DualAuthProvider::OpenAI => {
            (auth.openai_has_oauth, auth.openai_has_api_key)
        }
    };

    let active = match forced {
        // An explicit selection wins outright: it reflects what requests will use
        // even if the matching credential probe is momentarily stale.
        Some(kind) => kind,
        None if has_oauth => ActiveCredential::OAuth,
        None if has_api_key => ActiveCredential::ApiKey,
        None => return None,
    };

    Some(ResolvedProviderAuth {
        active,
        has_oauth,
        has_api_key,
        explicit: forced.is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::ProviderAuth;

    fn anthropic(has_oauth: bool, has_api_key: bool) -> AuthStatus {
        AuthStatus {
            anthropic: ProviderAuth {
                has_oauth,
                has_api_key,
                ..Default::default()
            },
            ..AuthStatus::default()
        }
    }

    #[test]
    fn anthropic_auto_prefers_oauth_when_both_present() {
        let auth = anthropic(true, true);
        let resolved = resolve_dual_credential_auth(ActiveProvider::Claude, &auth, None).unwrap();
        assert_eq!(resolved.active, ActiveCredential::OAuth);
        assert!(resolved.has_both());
    }

    #[test]
    fn anthropic_auto_falls_back_to_api_key() {
        let auth = anthropic(false, true);
        let resolved = resolve_dual_credential_auth(ActiveProvider::Claude, &auth, None).unwrap();
        assert_eq!(resolved.active, ActiveCredential::ApiKey);
        assert!(!resolved.has_both());
    }

    #[test]
    fn anthropic_explicit_selection_wins_over_auto() {
        let auth = anthropic(true, true);
        let resolved =
            resolve_dual_credential_auth(ActiveProvider::Claude, &auth, Some("claude-api"))
                .unwrap();
        assert_eq!(resolved.active, ActiveCredential::ApiKey);
        assert!(resolved.explicit);
        let resolved =
            resolve_dual_credential_auth(ActiveProvider::Claude, &auth, Some("anthropic")).unwrap();
        assert_eq!(resolved.active, ActiveCredential::OAuth);
        assert!(resolved.explicit);
    }

    #[test]
    fn auto_resolution_is_not_marked_explicit() {
        let auth = anthropic(true, false);
        let resolved = resolve_dual_credential_auth(ActiveProvider::Claude, &auth, None).unwrap();
        assert!(!resolved.explicit);
    }

    #[test]
    fn anthropic_none_when_unconfigured() {
        let auth = anthropic(false, false);
        assert!(resolve_dual_credential_auth(ActiveProvider::Claude, &auth, None).is_none());
    }

    #[test]
    fn openai_auto_reports_both_and_prefers_oauth() {
        let auth = AuthStatus {
            openai_has_oauth: true,
            openai_has_api_key: true,
            ..AuthStatus::default()
        };
        let resolved = resolve_dual_credential_auth(ActiveProvider::OpenAI, &auth, None).unwrap();
        assert_eq!(resolved.active, ActiveCredential::OAuth);
        assert!(resolved.has_both());
    }

    #[test]
    fn non_dual_providers_return_none() {
        let auth = AuthStatus::default();
        assert!(resolve_dual_credential_auth(ActiveProvider::Copilot, &auth, None).is_none());
        assert!(resolve_dual_credential_auth(ActiveProvider::Gemini, &auth, None).is_none());
    }
}
