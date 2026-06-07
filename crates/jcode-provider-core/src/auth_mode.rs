//! Canonical source of truth for the OAuth-vs-API-key credential decision of
//! the two "dual-auth" providers: Anthropic/Claude and OpenAI.
//!
//! These providers each support *both* a subscription/OAuth login and a direct
//! API key, so every request needs an explicit "which credential" decision.
//!
//! Historically jcode encoded that decision as free-form strings spread across
//! several overlapping vocabularies:
//!
//! | concept              | runtime env (`JCODE_RUNTIME_PROVIDER`) | route / stable-id    | CLI `--provider` | model prefix     |
//! |----------------------|----------------------------------------|----------------------|------------------|------------------|
//! | Claude, OAuth        | `claude`                               | `claude-oauth`       | `claude`         | `claude-oauth:`  |
//! | Claude, API key      | `claude-api`                           | `anthropic-api-key`  | `anthropic-api`  | `claude-api:`    |
//! | OpenAI, OAuth        | `openai`                               | `openai-oauth`       | `openai`         | `openai-oauth:`  |
//! | OpenAI, API key      | `openai-api`                           | `openai-api-key`     | `openai-api`     | `openai-api:`    |
//!
//! Each call site used to parse its own subset of these aliases by hand, and the
//! subsets drifted: e.g. one parser accepted `openai` but not `openai-oauth`,
//! another accepted `claude`/`anthropic` but silently ignored `claude-oauth`.
//! When a string from one vocabulary leaked into a parser that only knew
//! another, the OAuth and API-key paths got mixed up.
//!
//! This module is the single place that:
//!   * parses *any* alias from *any* vocabulary into a structured
//!     [`AuthRoute`] (`provider` + `mode`), and
//!   * emits the canonical string for each vocabulary.
//!
//! Every credential-mode parser and every UI/billing surface should go through
//! here instead of re-deriving the decision from ad-hoc string matches.

use crate::ResolvedCredential;
use crate::selection::ActiveProvider;

/// A provider that supports *both* a subscription/OAuth login and a direct
/// API-key credential, and therefore needs an explicit OAuth-vs-API decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DualAuthProvider {
    /// Anthropic / Claude (Claude subscription OAuth vs `ANTHROPIC_API_KEY`).
    Anthropic,
    /// OpenAI (ChatGPT/Codex OAuth vs `OPENAI_API_KEY`).
    OpenAI,
}

impl DualAuthProvider {
    /// The dual-auth provider backing an [`ActiveProvider`], if any. Returns
    /// `None` for providers with no OAuth-vs-API-key ambiguity.
    pub const fn from_active_provider(provider: ActiveProvider) -> Option<Self> {
        match provider {
            ActiveProvider::Claude => Some(Self::Anthropic),
            ActiveProvider::OpenAI => Some(Self::OpenAI),
            _ => None,
        }
    }

    /// The execution slot this credential decision routes through.
    pub const fn active_provider(self) -> ActiveProvider {
        match self {
            Self::Anthropic => ActiveProvider::Claude,
            Self::OpenAI => ActiveProvider::OpenAI,
        }
    }
}

/// Which credential a dual-auth provider will actually use for a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AuthMode {
    /// OAuth / subscription login (Claude subscription, ChatGPT/Codex login).
    Oauth,
    /// Direct provider API key (metered / cost-based billing).
    ApiKey,
}

impl AuthMode {
    /// True when requests bill against a subscription rather than a metered key.
    pub const fn is_subscription(self) -> bool {
        matches!(self, Self::Oauth)
    }

    /// Map to the wire-level [`ResolvedCredential`] billing identity.
    pub const fn resolved_credential(self) -> ResolvedCredential {
        match self {
            Self::Oauth => ResolvedCredential::Oauth,
            Self::ApiKey => ResolvedCredential::ApiKey,
        }
    }
}

impl From<AuthMode> for ResolvedCredential {
    fn from(mode: AuthMode) -> Self {
        mode.resolved_credential()
    }
}

/// A fully resolved dual-auth credential decision: *which provider* and *which
/// credential*. This is the structured value that every vocabulary string maps
/// to and is generated from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AuthRoute {
    pub provider: DualAuthProvider,
    pub mode: AuthMode,
}

impl AuthRoute {
    pub const fn new(provider: DualAuthProvider, mode: AuthMode) -> Self {
        Self { provider, mode }
    }

    pub const fn anthropic(mode: AuthMode) -> Self {
        Self::new(DualAuthProvider::Anthropic, mode)
    }

    pub const fn openai(mode: AuthMode) -> Self {
        Self::new(DualAuthProvider::OpenAI, mode)
    }

    /// Parse a dual-auth token from *any* of jcode's overlapping vocabularies
    /// (runtime env, route stable-id, CLI `--provider`, or bare model prefix).
    ///
    /// Returns `None` for tokens that do not pin a dual-auth credential route,
    /// including bare aliases for non-dual providers (`openrouter`, `copilot`,
    /// ...), unknown strings, and the empty string. A `None` result is what the
    /// providers treat as "auto" (no explicit OAuth-vs-API pin).
    ///
    /// A single trailing `:` is tolerated so callers can pass a model prefix
    /// such as `claude-oauth:` directly. Full prefixed model specs
    /// (`claude-oauth:model`) are *not* parsed here; resolve the prefix with
    /// `explicit_model_provider_prefix` first.
    pub fn parse(token: &str) -> Option<Self> {
        let token = token.trim().strip_suffix(':').unwrap_or(token.trim());
        match token.trim().to_ascii_lowercase().as_str() {
            // Anthropic / Claude -- OAuth / subscription.
            "claude" | "anthropic" | "claude-oauth" | "anthropic-oauth" => {
                Some(Self::anthropic(AuthMode::Oauth))
            }
            // Anthropic / Claude -- direct API key.
            //
            // Bare `api-key` historically resolves to Anthropic in the route
            // vocabulary (see `ModelRouteApiMethod::parse`), so keep that.
            "claude-api" | "anthropic-api" | "anthropic-api-key" | "claude-api-key"
            | "anthropic-key" | "claude-key" | "api-key" => Some(Self::anthropic(AuthMode::ApiKey)),
            // OpenAI -- OAuth / ChatGPT-Codex login.
            "openai" | "openai-oauth" => Some(Self::openai(AuthMode::Oauth)),
            // OpenAI -- direct API key.
            "openai-api" | "openai-api-key" | "openai-key" | "openai-apikey"
            | "openai-platform" | "platform-openai" => Some(Self::openai(AuthMode::ApiKey)),
            _ => None,
        }
    }

    /// The execution slot this route runs through.
    pub const fn active_provider(self) -> ActiveProvider {
        self.provider.active_provider()
    }

    /// The wire-level billing identity for this route.
    pub const fn resolved_credential(self) -> ResolvedCredential {
        self.mode.resolved_credential()
    }

    /// Parse a *model prefix* that explicitly pins a dual-auth credential.
    ///
    /// This differs from [`AuthRoute::parse`] in the bare-provider cases: in the
    /// model-prefix vocabulary `claude:` / `anthropic:` / `openai:` mean "route
    /// to this provider but keep the current credential (auto)", so they do NOT
    /// pin a credential and return `None` here. Only the explicit credential
    /// prefixes (`claude-oauth:`, `claude-api:`, `openai-oauth:`, `openai-api:`,
    /// and their stable-id spellings) pin one.
    ///
    /// A single trailing `:` is tolerated so callers can pass the raw prefix.
    pub fn parse_explicit_credential_prefix(prefix: &str) -> Option<Self> {
        let token = prefix.trim().strip_suffix(':').unwrap_or(prefix.trim());
        match token.trim().to_ascii_lowercase().as_str() {
            // Bare provider aliases do not pin a credential in this vocabulary.
            "claude" | "anthropic" | "openai" => None,
            other => Self::parse(other),
        }
    }

    /// Canonical `JCODE_RUNTIME_PROVIDER` value that pins this route.
    pub const fn runtime_provider_key(self) -> &'static str {
        match (self.provider, self.mode) {
            (DualAuthProvider::Anthropic, AuthMode::Oauth) => "claude",
            (DualAuthProvider::Anthropic, AuthMode::ApiKey) => "claude-api",
            (DualAuthProvider::OpenAI, AuthMode::Oauth) => "openai",
            (DualAuthProvider::OpenAI, AuthMode::ApiKey) => "openai-api",
        }
    }

    /// Canonical route `api_method` / [`crate::RuntimeKey`] stable-id.
    pub const fn route_api_method(self) -> &'static str {
        match (self.provider, self.mode) {
            (DualAuthProvider::Anthropic, AuthMode::Oauth) => "claude-oauth",
            (DualAuthProvider::Anthropic, AuthMode::ApiKey) => "anthropic-api-key",
            (DualAuthProvider::OpenAI, AuthMode::Oauth) => "openai-oauth",
            (DualAuthProvider::OpenAI, AuthMode::ApiKey) => "openai-api-key",
        }
    }

    /// Canonical model-switch prefix (without the trailing colon).
    pub const fn model_prefix(self) -> &'static str {
        match (self.provider, self.mode) {
            (DualAuthProvider::Anthropic, AuthMode::Oauth) => "claude-oauth",
            (DualAuthProvider::Anthropic, AuthMode::ApiKey) => "claude-api",
            (DualAuthProvider::OpenAI, AuthMode::Oauth) => "openai-oauth",
            (DualAuthProvider::OpenAI, AuthMode::ApiKey) => "openai-api",
        }
    }

    /// Canonical session `provider_key` (the folded, route-free form).
    pub const fn session_provider_key(self) -> &'static str {
        // Identical to the runtime-env vocabulary today; kept as its own method
        // so the session-key meaning is explicit at call sites.
        self.runtime_provider_key()
    }

    /// Canonical CLI `--provider` argument value.
    pub const fn cli_provider_arg(self) -> &'static str {
        match (self.provider, self.mode) {
            (DualAuthProvider::Anthropic, AuthMode::Oauth) => "claude",
            (DualAuthProvider::Anthropic, AuthMode::ApiKey) => "anthropic-api",
            (DualAuthProvider::OpenAI, AuthMode::Oauth) => "openai",
            (DualAuthProvider::OpenAI, AuthMode::ApiKey) => "openai-api",
        }
    }
}

/// Resolve the explicit dual-auth mode that `runtime_provider` pins for a
/// specific provider.
///
/// Returns `None` (i.e. "auto") when `runtime_provider` is absent, does not pin
/// a dual-auth route, or pins the *other* dual-auth provider.
pub fn pinned_mode_for(
    provider: DualAuthProvider,
    runtime_provider: Option<&str>,
) -> Option<AuthMode> {
    let route = AuthRoute::parse(runtime_provider?)?;
    (route.provider == provider).then_some(route.mode)
}

/// Read `JCODE_RUNTIME_PROVIDER` and return the dual-auth route it pins, if any.
pub fn runtime_env_auth_route() -> Option<AuthRoute> {
    let value = std::env::var("JCODE_RUNTIME_PROVIDER").ok()?;
    AuthRoute::parse(&value)
}

/// Read `JCODE_RUNTIME_PROVIDER` and resolve the dual-auth mode it pins for a
/// specific provider (or `None` for "auto").
pub fn runtime_env_pinned_mode(provider: DualAuthProvider) -> Option<AuthMode> {
    pinned_mode_for(
        provider,
        std::env::var("JCODE_RUNTIME_PROVIDER").ok().as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_ROUTES: [AuthRoute; 4] = [
        AuthRoute::anthropic(AuthMode::Oauth),
        AuthRoute::anthropic(AuthMode::ApiKey),
        AuthRoute::openai(AuthMode::Oauth),
        AuthRoute::openai(AuthMode::ApiKey),
    ];

    #[test]
    fn every_vocabulary_string_round_trips_back_to_the_same_route() {
        for route in ALL_ROUTES {
            for token in [
                route.runtime_provider_key(),
                route.route_api_method(),
                route.model_prefix(),
                route.cli_provider_arg(),
                route.session_provider_key(),
            ] {
                assert_eq!(
                    AuthRoute::parse(token),
                    Some(route),
                    "token {token:?} should parse back to {route:?}",
                );
                // Trailing-colon (model-prefix) form must parse identically.
                assert_eq!(
                    AuthRoute::parse(&format!("{token}:")),
                    Some(route),
                    "token {token:?}: should parse back to {route:?}",
                );
            }
        }
    }

    #[test]
    fn parse_is_case_and_whitespace_insensitive() {
        assert_eq!(
            AuthRoute::parse("  Claude-OAuth "),
            Some(AuthRoute::anthropic(AuthMode::Oauth))
        );
        assert_eq!(
            AuthRoute::parse("ANTHROPIC-API-KEY"),
            Some(AuthRoute::anthropic(AuthMode::ApiKey))
        );
    }

    #[test]
    fn bare_provider_aliases_pin_oauth() {
        assert_eq!(
            AuthRoute::parse("claude").map(|r| r.mode),
            Some(AuthMode::Oauth)
        );
        assert_eq!(
            AuthRoute::parse("anthropic").map(|r| r.mode),
            Some(AuthMode::Oauth)
        );
        assert_eq!(
            AuthRoute::parse("openai").map(|r| r.mode),
            Some(AuthMode::Oauth)
        );
    }

    #[test]
    fn cross_vocabulary_aliases_resolve_consistently() {
        // The whole point: route-vocabulary strings and runtime-env strings for
        // the same concept resolve to the same structured route.
        for (a, b) in [
            ("claude", "claude-oauth"),
            ("claude-api", "anthropic-api-key"),
            ("openai", "openai-oauth"),
            ("openai-api", "openai-api-key"),
        ] {
            assert_eq!(
                AuthRoute::parse(a),
                AuthRoute::parse(b),
                "{a:?} and {b:?} must resolve to the same route",
            );
        }
    }

    #[test]
    fn non_dual_and_unknown_tokens_are_none() {
        for token in [
            "",
            "openrouter",
            "copilot",
            "gemini",
            "bedrock",
            "jcode",
            "nonsense",
        ] {
            assert_eq!(AuthRoute::parse(token), None, "{token:?} must be None");
        }
    }

    #[test]
    fn explicit_credential_prefix_ignores_bare_provider_aliases() {
        // Bare provider prefixes route without pinning a credential.
        for token in ["claude", "claude:", "anthropic:", "openai", "openai:"] {
            assert_eq!(
                AuthRoute::parse_explicit_credential_prefix(token),
                None,
                "{token:?} must not pin a credential",
            );
        }
        // Explicit credential prefixes still pin.
        assert_eq!(
            AuthRoute::parse_explicit_credential_prefix("claude-oauth:"),
            Some(AuthRoute::anthropic(AuthMode::Oauth))
        );
        assert_eq!(
            AuthRoute::parse_explicit_credential_prefix("claude-api:"),
            Some(AuthRoute::anthropic(AuthMode::ApiKey))
        );
        assert_eq!(
            AuthRoute::parse_explicit_credential_prefix("openai-oauth:"),
            Some(AuthRoute::openai(AuthMode::Oauth))
        );
        assert_eq!(
            AuthRoute::parse_explicit_credential_prefix("openai-api:"),
            Some(AuthRoute::openai(AuthMode::ApiKey))
        );
    }

    #[test]
    fn pinned_mode_only_matches_its_own_provider() {
        assert_eq!(
            pinned_mode_for(DualAuthProvider::Anthropic, Some("claude-api")),
            Some(AuthMode::ApiKey)
        );
        // A pin for the *other* dual-auth provider is "auto" here.
        assert_eq!(
            pinned_mode_for(DualAuthProvider::Anthropic, Some("openai")),
            None
        );
        assert_eq!(
            pinned_mode_for(DualAuthProvider::OpenAI, Some("claude")),
            None
        );
        assert_eq!(pinned_mode_for(DualAuthProvider::OpenAI, None), None);
    }

    #[test]
    fn resolved_credential_mapping() {
        assert_eq!(
            AuthMode::Oauth.resolved_credential(),
            ResolvedCredential::Oauth
        );
        assert_eq!(
            AuthMode::ApiKey.resolved_credential(),
            ResolvedCredential::ApiKey
        );
    }

    #[test]
    fn route_api_method_round_trips_through_model_route_api_method() {
        use crate::ModelRouteApiMethod;
        // The route-vocabulary parser (`ModelRouteApiMethod`) must agree with the
        // canonical auth-mode parser for every dual-auth route, so routing and
        // billing never disagree about OAuth-vs-API-key.
        for route in ALL_ROUTES {
            let parsed = ModelRouteApiMethod::parse(route.route_api_method());
            assert_eq!(
                parsed,
                ModelRouteApiMethod::from_auth_route(route),
                "route {route:?} api_method must round-trip through ModelRouteApiMethod",
            );
            // And every alias vocabulary maps to the same ModelRouteApiMethod.
            for token in [
                route.runtime_provider_key(),
                route.model_prefix(),
                route.cli_provider_arg(),
                route.session_provider_key(),
            ] {
                assert_eq!(
                    ModelRouteApiMethod::parse(token),
                    parsed,
                    "token {token:?} must map to the same ModelRouteApiMethod as {route:?}",
                );
            }
        }
    }
}
