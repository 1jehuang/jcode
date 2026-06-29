pub mod account_store;
pub mod active_method;
pub mod antigravity;
pub mod azure;
pub mod claude;
pub mod codex;
mod commands;
pub mod copilot;
pub mod cursor;
pub mod doctor;
pub mod external;
pub mod gemini;
pub mod google;
pub(crate) mod google_oauth;
pub mod integration;
pub mod lifecycle;
#[cfg(any(test, feature = "test-support"))]
// The driver's items are exercised by its internal #[cfg(test)] tests; under
// a plain `--features test-support` lib build they are intentionally unused.
#[cfg_attr(not(test), allow(dead_code, unused_imports))]
pub(crate) mod lifecycle_driver;
pub(crate) mod live_provider_probes;
pub mod login_diagnostics;
pub mod login_flows;
pub mod oauth;
pub mod provider_e2e;
pub(crate) mod refresh_coordinator;
pub mod refresh_state;
mod status_types;
#[cfg(any(test, feature = "test-support"))]
pub mod test_sandbox;
pub mod validation;

pub(crate) use commands::command_exists;
#[cfg(test)]
pub(crate) use commands::{
    command_candidates, contains_path_separator, dedup_preserve_order, has_extension,
    is_wsl2_windows_path,
};

pub use status_types::{
    AuthCredentialSource, AuthExpiryConfidence, AuthReadinessLevel, AuthRefreshSupport, AuthState,
    AuthStatus, AuthValidationMethod, ProviderAuth, ProviderAuthAssessment,
};

pub use active_method::{ActiveCredential, ResolvedProviderAuth, resolve_dual_credential_auth};

use crate::provider_catalog::LoginProviderAuthStateKey;
use crate::provider_catalog::LoginProviderDescriptor;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, RwLock};
use std::time::Instant;

/// Cached auth status plus the `JCODE_HOME` it was computed under.
///
/// Auth probes read credential files relative to `JCODE_HOME`. Tests swap
/// `JCODE_HOME` to per-test temp dirs, and a status computed under one home
/// must never be served for another (issue #361: parallel provider tests
/// intermittently observed another test's auth snapshot through this global
/// cache). In production the home never changes, so the key check is free.
type CachedAuthStatus = (AuthStatus, Instant, Option<std::ffi::OsString>);

static AUTH_STATUS_CACHE: std::sync::LazyLock<RwLock<Option<CachedAuthStatus>>> =
    std::sync::LazyLock::new(|| RwLock::new(None));
static AUTH_STATUS_FAST_CACHE: std::sync::LazyLock<RwLock<Option<CachedAuthStatus>>> =
    std::sync::LazyLock::new(|| RwLock::new(None));

fn auth_cache_home_key() -> Option<std::ffi::OsString> {
    std::env::var_os("JCODE_HOME")
}

const AUTH_STATUS_CACHE_TTL_SECS: u64 = 30;
const AUTH_STATUS_FAST_CACHE_TTL_SECS: u64 = 60;

/// Per-process cache for command existence lookups.
/// CLI tools don't get installed/uninstalled while jcode is running, so caching
/// indefinitely per process is correct and avoids repeated PATH scans.
static COMMAND_EXISTS_CACHE: std::sync::LazyLock<Mutex<HashMap<String, bool>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthProbeMode {
    Full,
    Fast,
}

pub fn browser_suppressed(cli_no_browser: bool) -> bool {
    cli_no_browser
        || env_truthy("NO_BROWSER")
        || env_truthy("JCODE_NO_BROWSER")
        || running_in_test_harness()
}

/// True when the current process is a Rust test binary (`cargo test` /
/// `cargo nextest`). Test binaries always run from `target/**/deps/`, a
/// location no installed or self-dev jcode binary ever runs from.
///
/// Used to keep tests from opening real browser windows (OAuth login pages,
/// files) on the developer's desktop: many login/onboarding flows are
/// exercised by TUI tests, and without this guard each test run could pop
/// multiple browser tabs. Set `JCODE_ALLOW_BROWSER_IN_TESTS=1` to opt out
/// (e.g. for an intentionally interactive live test).
pub fn running_in_test_harness() -> bool {
    static IN_TEST_HARNESS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *IN_TEST_HARNESS.get_or_init(|| {
        if env_truthy("JCODE_ALLOW_BROWSER_IN_TESTS") {
            return false;
        }
        std::env::current_exe()
            .ok()
            .map(|exe| {
                let path = exe.to_string_lossy().replace('\\', "/");
                path.contains("/target/") && path.contains("/deps/")
            })
            .unwrap_or(false)
    })
}

fn env_truthy(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|value| {
            let trimmed = value.trim();
            !trimmed.is_empty() && trimmed != "0" && !trimmed.eq_ignore_ascii_case("false")
        })
        .unwrap_or(false)
}

fn auth_timing_logging_enabled() -> bool {
    env_truthy("JCODE_AUTH_TIMING")
}

fn openai_api_key_configured() -> bool {
    crate::provider_catalog::load_api_key_from_env_or_config("OPENAI_API_KEY", "openai.env")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

fn auth_state_label(state: AuthState) -> &'static str {
    match state {
        AuthState::Available => "available",
        AuthState::Expired => "expired",
        AuthState::NotConfigured => "not_configured",
    }
}

fn bool_label(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

fn log_auth_status_snapshot(event: &str, status: &AuthStatus) {
    crate::logging::auth_event(
        event,
        "all",
        &[
            ("jcode", auth_state_label(status.jcode)),
            ("claude", auth_state_label(status.anthropic.state)),
            ("openai", auth_state_label(status.openai)),
            ("openrouter", auth_state_label(status.openrouter)),
            ("azure", auth_state_label(status.azure)),
            ("azure_api_auth", bool_label(status.azure_has_api_key)),
            ("azure_entra", bool_label(status.azure_uses_entra)),
            ("bedrock", auth_state_label(status.bedrock)),
            ("copilot", auth_state_label(status.copilot)),
            ("antigravity", auth_state_label(status.antigravity)),
            ("gemini", auth_state_label(status.gemini)),
            ("cursor", auth_state_label(status.cursor)),
            ("google", auth_state_label(status.google)),
        ],
    );
}

fn auth_readiness_for_provider(
    provider: LoginProviderDescriptor,
    state: AuthState,
    last_validation: Option<&crate::auth::validation::ProviderValidationRecord>,
) -> AuthReadinessLevel {
    match state {
        AuthState::NotConfigured => AuthReadinessLevel::None,
        AuthState::Expired => AuthReadinessLevel::CredentialPresent,
        AuthState::Available => {
            if last_validation.and_then(|record| record.provider_smoke_ok) == Some(true) {
                return model_smoke_readiness_for_provider(provider);
            }

            available_provider_base_readiness(provider)
        }
    }
}

fn available_provider_base_readiness(provider: LoginProviderDescriptor) -> AuthReadinessLevel {
    match provider.target {
        crate::provider_catalog::LoginProviderTarget::Claude
        | crate::provider_catalog::LoginProviderTarget::OpenAi
        | crate::provider_catalog::LoginProviderTarget::Copilot
        | crate::provider_catalog::LoginProviderTarget::Gemini
        | crate::provider_catalog::LoginProviderTarget::Antigravity
        | crate::provider_catalog::LoginProviderTarget::Google => AuthReadinessLevel::Authenticated,
        _ => AuthReadinessLevel::CredentialPresent,
    }
}

fn model_smoke_readiness_for_provider(provider: LoginProviderDescriptor) -> AuthReadinessLevel {
    match provider.target {
        // Azure model names are deployment IDs. A successful smoke call proves the
        // resource, auth, and selected deployment all work together.
        crate::provider_catalog::LoginProviderTarget::Azure => AuthReadinessLevel::DeploymentValid,
        _ => AuthReadinessLevel::RequestValid,
    }
}

fn copilot_auth_state_from_credentials() -> (AuthState, bool) {
    if !copilot::has_copilot_credentials_fast() {
        return (AuthState::NotConfigured, false);
    }

    if copilot::validation_failure_blocks_auto_use() {
        (AuthState::Expired, false)
    } else {
        (AuthState::Available, true)
    }
}

impl AuthStatus {
    /// Check all authentication sources and return their status.
    /// Results are cached for 30 seconds to avoid expensive PATH scanning on every frame.
    pub fn check() -> Self {
        let home_key = auth_cache_home_key();
        if let Ok(cache) = AUTH_STATUS_CACHE.read()
            && let Some((ref status, ref when, ref cached_home)) = *cache
            && when.elapsed().as_secs() < AUTH_STATUS_CACHE_TTL_SECS
            && *cached_home == home_key
        {
            return status.clone();
        }

        let status = Self::check_uncached();

        if let Ok(mut cache) = AUTH_STATUS_CACHE.write() {
            *cache = Some((status.clone(), Instant::now(), home_key.clone()));
        }
        if let Ok(mut cache) = AUTH_STATUS_FAST_CACHE.write() {
            *cache = Some((status.clone(), Instant::now(), home_key));
        }

        status
    }

    /// Fast auth snapshot for interactive UI surfaces like `/account`.
    ///
    /// Prefers a recent full probe, and otherwise falls back to a cheap
    /// local-files/env-only probe that avoids subprocesses such as
    /// `cursor-agent status` or `sqlite3` lookups. Do not reuse the full cache
    /// forever: external credential files may be deleted or replaced while the
    /// process is running.
    pub fn check_fast() -> Self {
        let home_key = auth_cache_home_key();
        if let Ok(cache) = AUTH_STATUS_CACHE.read()
            && let Some((ref status, ref when, ref cached_home)) = *cache
            && when.elapsed().as_secs() < AUTH_STATUS_CACHE_TTL_SECS
            && *cached_home == home_key
        {
            return status.clone();
        }

        if let Ok(cache) = AUTH_STATUS_FAST_CACHE.read()
            && let Some((ref status, ref when, ref cached_home)) = *cache
            && when.elapsed().as_secs() < AUTH_STATUS_FAST_CACHE_TTL_SECS
            && *cached_home == home_key
        {
            return status.clone();
        }

        let status = Self::check_uncached_fast();
        if let Ok(mut cache) = AUTH_STATUS_FAST_CACHE.write() {
            *cache = Some((status.clone(), Instant::now(), home_key));
        }

        status
    }

    /// Returns true if at least one provider has usable credentials.
    pub fn has_any_available(&self) -> bool {
        self.anthropic.state == AuthState::Available
            || self.jcode == AuthState::Available
            || self.openai == AuthState::Available
            || self.openrouter == AuthState::Available
            || self.azure == AuthState::Available
            || self.bedrock == AuthState::Available
            || self.copilot == AuthState::Available
            || self.antigravity == AuthState::Available
            || self.gemini == AuthState::Available
            || self.cursor == AuthState::Available
    }

    /// Emit a structured, non-secret snapshot of which providers currently have
    /// credentials configured. This is the single best line to ask a user to
    /// share when debugging "my model picker is empty / only OpenAI+Anthropic
    /// show / login silently failed" reports: it records, per provider, whether
    /// jcode believes credentials are available/expired/missing without leaking
    /// any token or key material.
    ///
    /// `surface` describes where the snapshot was taken from (for example
    /// `model_picker`, `auth_changed`, `catalog_refresh`) so logs can be
    /// correlated with the user action that triggered them.
    pub fn log_snapshot(&self, surface: &str) {
        crate::logging::event_info(
            "auth_status_snapshot",
            vec![
                ("surface", surface.to_string()),
                ("any_available", self.has_any_available().to_string()),
                ("jcode", self.jcode.label().to_string()),
                ("anthropic", self.anthropic.state.label().to_string()),
                ("anthropic_oauth", self.anthropic.has_oauth.to_string()),
                ("anthropic_api", self.anthropic.has_api_key.to_string()),
                ("openai", self.openai.label().to_string()),
                ("openai_oauth", self.openai_has_oauth.to_string()),
                ("openai_api", self.openai_has_api_key.to_string()),
                ("openrouter", self.openrouter.label().to_string()),
                ("azure", self.azure.label().to_string()),
                ("azure_api", self.azure_has_api_key.to_string()),
                ("azure_entra", self.azure_uses_entra.to_string()),
                ("bedrock", self.bedrock.label().to_string()),
                ("copilot", self.copilot.label().to_string()),
                ("copilot_cred", self.copilot_has_api_token.to_string()),
                ("antigravity", self.antigravity.label().to_string()),
                ("gemini", self.gemini.label().to_string()),
                ("cursor", self.cursor.label().to_string()),
            ],
        );
    }

    pub fn has_any_untrusted_external_auth() -> bool {
        crate::auth::codex::has_unconsented_legacy_credentials()
            || crate::auth::claude::has_unconsented_external_auth().is_some()
            || crate::auth::external::has_any_unconsented_external_auth()
            || crate::auth::gemini::has_unconsented_cli_auth()
            || crate::auth::copilot::has_unconsented_external_auth().is_some()
            || crate::auth::cursor::has_unconsented_external_auth().is_some()
    }

    pub fn state_for_key(&self, key: LoginProviderAuthStateKey) -> AuthState {
        match key {
            LoginProviderAuthStateKey::ExternalImport => {
                if Self::has_any_untrusted_external_auth() {
                    AuthState::Available
                } else {
                    AuthState::NotConfigured
                }
            }
            LoginProviderAuthStateKey::Jcode => self.jcode,
            LoginProviderAuthStateKey::Anthropic => self.anthropic.state,
            LoginProviderAuthStateKey::OpenAi => self.openai,
            LoginProviderAuthStateKey::Azure => self.azure,
            LoginProviderAuthStateKey::Bedrock => self.bedrock,
            LoginProviderAuthStateKey::OpenRouterLike => self.openrouter,
            LoginProviderAuthStateKey::Copilot => self.copilot,
            LoginProviderAuthStateKey::Antigravity => self.antigravity,
            LoginProviderAuthStateKey::Gemini => self.gemini,
            LoginProviderAuthStateKey::Cursor => self.cursor,
            LoginProviderAuthStateKey::Google => self.google,
        }
    }

    pub fn state_for_provider(&self, provider: LoginProviderDescriptor) -> AuthState {
        match provider.target {
            crate::provider_catalog::LoginProviderTarget::AutoImport => {
                if Self::has_any_untrusted_external_auth() {
                    AuthState::Available
                } else {
                    AuthState::NotConfigured
                }
            }
            crate::provider_catalog::LoginProviderTarget::Jcode => {
                if crate::subscription_catalog::has_credentials() {
                    AuthState::Available
                } else {
                    AuthState::NotConfigured
                }
            }
            crate::provider_catalog::LoginProviderTarget::OpenRouter => {
                if api_key_available("OPENROUTER_API_KEY", "openrouter.env") {
                    AuthState::Available
                } else {
                    AuthState::NotConfigured
                }
            }
            crate::provider_catalog::LoginProviderTarget::OpenAiApiKey => {
                if api_key_available("OPENAI_API_KEY", "openai.env") {
                    AuthState::Available
                } else {
                    AuthState::NotConfigured
                }
            }
            // The `anthropic-api` login provider is the *API-key* path. It must
            // report on the presence of an Anthropic API key alone, never borrow
            // the OAuth/subscription credential's availability (that is the
            // separate `claude` provider). Sharing `auth_state_key::Anthropic`
            // previously made this provider claim "available / OAuth + API key"
            // even with zero API key configured, which then failed at request
            // time because API-key mode never falls back to OAuth.
            crate::provider_catalog::LoginProviderTarget::ClaudeApiKey => {
                if api_key_available("ANTHROPIC_API_KEY", "anthropic.env") {
                    AuthState::Available
                } else {
                    AuthState::NotConfigured
                }
            }
            // The `claude` login provider is the *OAuth/subscription* path: the
            // mirror image of the `anthropic-api` rule above. It must report on
            // the OAuth credential alone, never borrow the API key's
            // availability, so the two rows never blur into one ambiguous
            // "OAuth + API key" answer.
            crate::provider_catalog::LoginProviderTarget::Claude => self.anthropic.oauth_state,
            // Same split for OpenAI: `openai` is the ChatGPT/Codex OAuth login,
            // `openai-api` (handled above) is the API-key login.
            crate::provider_catalog::LoginProviderTarget::OpenAi => self.openai_oauth_state,
            crate::provider_catalog::LoginProviderTarget::Bedrock => {
                if crate::provider::bedrock::BedrockProvider::has_credentials() {
                    AuthState::Available
                } else {
                    AuthState::NotConfigured
                }
            }
            crate::provider_catalog::LoginProviderTarget::OpenAiCompatible(profile) => {
                if crate::provider_catalog::openai_compatible_profile_is_configured(profile) {
                    AuthState::Available
                } else {
                    AuthState::NotConfigured
                }
            }
            _ => self.state_for_key(provider.auth_state_key),
        }
    }

    pub fn method_detail_for_provider(&self, provider: LoginProviderDescriptor) -> String {
        match provider.target {
            crate::provider_catalog::LoginProviderTarget::AutoImport => {
                if Self::has_any_untrusted_external_auth() {
                    "Existing external logins detected".to_string()
                } else {
                    "No importable external logins found".to_string()
                }
            }
            crate::provider_catalog::LoginProviderTarget::Jcode => {
                if self.state_for_provider(provider) == AuthState::Available {
                    if crate::subscription_catalog::has_router_base() {
                        format!(
                            "API key (`{}`) + router base",
                            crate::subscription_catalog::JCODE_API_KEY_ENV
                        )
                    } else {
                        format!(
                            "API key (`{}`), router base pending",
                            crate::subscription_catalog::JCODE_API_KEY_ENV
                        )
                    }
                } else {
                    "not configured".to_string()
                }
            }
            crate::provider_catalog::LoginProviderTarget::OpenRouter => {
                if self.state_for_provider(provider) == AuthState::Available {
                    "API key (`OPENROUTER_API_KEY`)".to_string()
                } else {
                    "not configured".to_string()
                }
            }
            crate::provider_catalog::LoginProviderTarget::OpenAiApiKey => {
                if self.state_for_provider(provider) == AuthState::Available {
                    "API key (`OPENAI_API_KEY`)".to_string()
                } else {
                    "not configured".to_string()
                }
            }
            crate::provider_catalog::LoginProviderTarget::ClaudeApiKey => {
                if self.state_for_provider(provider) == AuthState::Available {
                    "API key (`ANTHROPIC_API_KEY`)".to_string()
                } else {
                    "not configured".to_string()
                }
            }
            crate::provider_catalog::LoginProviderTarget::Bedrock => {
                if self.state_for_provider(provider) == AuthState::Available {
                    if crate::provider::bedrock::BedrockProvider::configured_bearer_token()
                        .is_some()
                    {
                        "Bedrock API key (`AWS_BEARER_TOKEN_BEDROCK`)".to_string()
                    } else {
                        "AWS credential chain".to_string()
                    }
                } else {
                    "not configured".to_string()
                }
            }
            crate::provider_catalog::LoginProviderTarget::OpenAiCompatible(profile) => {
                let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
                if self.state_for_provider(provider) == AuthState::Available {
                    if resolved.requires_api_key {
                        format!("API key (`{}`)", resolved.api_key_env)
                    } else if crate::provider_catalog::load_api_key_from_env_or_config(
                        &resolved.api_key_env,
                        &resolved.env_file,
                    )
                    .is_some()
                    {
                        format!(
                            "local endpoint (`{}`) + optional API key (`{}`)",
                            resolved.api_base, resolved.api_key_env
                        )
                    } else {
                        format!("local endpoint (`{}`)", resolved.api_base)
                    }
                } else {
                    "not configured".to_string()
                }
            }
            _ => match provider.auth_state_key {
                // The `claude` login provider is the OAuth/subscription path;
                // the API key reports through the separate `anthropic-api`
                // provider (handled above via `LoginProviderTarget::ClaudeApiKey`).
                // Describe the OAuth credential alone so the two rows never
                // blur together as "OAuth + API key".
                LoginProviderAuthStateKey::Anthropic => {
                    let detail = match self.anthropic.oauth_state {
                        AuthState::Available => "OAuth",
                        AuthState::Expired => "OAuth (expired)",
                        AuthState::NotConfigured => "not configured",
                    };

                    let accounts = crate::auth::claude::list_accounts().unwrap_or_default();
                    if accounts.len() > 1 {
                        let active = crate::auth::claude::active_account_label()
                            .unwrap_or_else(|| "?".to_string());
                        format!(
                            "{detail} ({} accounts, active: `{}`)",
                            accounts.len(),
                            active
                        )
                    } else if accounts.len() == 1 {
                        format!("{detail} (account: `{}`)", accounts[0].label)
                    } else {
                        detail.to_string()
                    }
                }
                // Same split for OpenAI: this is the ChatGPT/Codex OAuth login;
                // `openai-api` (handled above) owns the API-key answer.
                LoginProviderAuthStateKey::OpenAi => {
                    let detail = match self.openai_oauth_state {
                        AuthState::Available => "OAuth",
                        AuthState::Expired => "OAuth (expired)",
                        AuthState::NotConfigured => "not configured",
                    };

                    let accounts = crate::auth::codex::list_accounts().unwrap_or_default();
                    if accounts.len() > 1 {
                        let active = crate::auth::codex::active_account_label()
                            .unwrap_or_else(|| "?".to_string());
                        format!(
                            "{detail} ({} accounts, active: `{}`)",
                            accounts.len(),
                            active
                        )
                    } else if accounts.len() == 1 {
                        format!("{detail} (account: `{}`)", accounts[0].label)
                    } else {
                        detail.to_string()
                    }
                }
                _ => provider.auth_status_method.to_string(),
            },
        }
    }

    pub fn assessment_for_provider(
        &self,
        provider: LoginProviderDescriptor,
    ) -> ProviderAuthAssessment {
        let state = self.state_for_provider(provider);
        let method_detail = self.method_detail_for_provider(provider);
        let last_validation = crate::auth::validation::get(provider.id);
        let last_refresh = crate::auth::refresh_state::get(provider.id);

        let (
            credential_source,
            credential_source_detail,
            expiry_confidence,
            refresh_support,
            validation_method,
        ) = match provider.target {
            crate::provider_catalog::LoginProviderTarget::AutoImport => (
                if Self::has_any_untrusted_external_auth() {
                    AuthCredentialSource::TrustedExternalFile
                } else {
                    AuthCredentialSource::None
                },
                if Self::has_any_untrusted_external_auth() {
                    "untrusted external auth sources detected".to_string()
                } else {
                    "none detected".to_string()
                },
                AuthExpiryConfidence::Unknown,
                AuthRefreshSupport::ExternalManaged,
                AuthValidationMethod::TrustedImportScan,
            ),
            crate::provider_catalog::LoginProviderTarget::Jcode => {
                let (source, detail) = summarize_sources(vec![
                    env_source(crate::subscription_catalog::JCODE_API_KEY_ENV),
                    config_source(
                        crate::subscription_catalog::JCODE_API_KEY_ENV,
                        crate::subscription_catalog::JCODE_ENV_FILE,
                        "~/.config/jcode/jcode-subscription.env",
                    ),
                ]);
                (
                    source,
                    detail,
                    AuthExpiryConfidence::NotApplicable,
                    AuthRefreshSupport::NotApplicable,
                    AuthValidationMethod::PresenceCheck,
                )
            }
            crate::provider_catalog::LoginProviderTarget::OpenRouter => {
                let (source, detail) = summarize_sources(vec![
                    env_source("OPENROUTER_API_KEY"),
                    config_source(
                        "OPENROUTER_API_KEY",
                        "openrouter.env",
                        "~/.config/jcode/openrouter.env",
                    ),
                    external_api_key_source("OPENROUTER_API_KEY"),
                ]);
                (
                    source,
                    detail,
                    AuthExpiryConfidence::NotApplicable,
                    AuthRefreshSupport::NotApplicable,
                    AuthValidationMethod::PresenceCheck,
                )
            }
            crate::provider_catalog::LoginProviderTarget::OpenAiApiKey => {
                let (source, detail) = summarize_sources(vec![
                    env_source("OPENAI_API_KEY"),
                    config_source("OPENAI_API_KEY", "openai.env", "~/.config/jcode/openai.env"),
                    external_api_key_source("OPENAI_API_KEY"),
                ]);
                (
                    source,
                    detail,
                    AuthExpiryConfidence::NotApplicable,
                    AuthRefreshSupport::NotApplicable,
                    AuthValidationMethod::PresenceCheck,
                )
            }
            crate::provider_catalog::LoginProviderTarget::ClaudeApiKey => {
                // The Anthropic API key is most commonly stored in the app
                // config file (`~/.config/jcode/anthropic.env`), *not* an env
                // var and *not* `~/.jcode/auth.json` (which holds the separate
                // OAuth accounts). List every place it can live so the real
                // source is always discoverable instead of looking "absent".
                let (source, detail) = summarize_sources(vec![
                    env_source("ANTHROPIC_API_KEY"),
                    config_source(
                        "ANTHROPIC_API_KEY",
                        "anthropic.env",
                        "~/.config/jcode/anthropic.env",
                    ),
                    external_api_key_source("ANTHROPIC_API_KEY"),
                ]);
                (
                    source,
                    detail,
                    AuthExpiryConfidence::NotApplicable,
                    AuthRefreshSupport::NotApplicable,
                    AuthValidationMethod::PresenceCheck,
                )
            }
            crate::provider_catalog::LoginProviderTarget::Azure => {
                let (source, detail) = summarize_sources(vec![
                    azure_entra_source(),
                    env_source(crate::auth::azure::API_KEY_ENV),
                    config_source(
                        crate::auth::azure::API_KEY_ENV,
                        crate::auth::azure::ENV_FILE,
                        "~/.config/jcode/azure-openai.env",
                    ),
                ]);
                (
                    source,
                    detail,
                    AuthExpiryConfidence::ConfigurationOnly,
                    if crate::auth::azure::uses_entra_id() {
                        AuthRefreshSupport::Automatic
                    } else {
                        AuthRefreshSupport::NotApplicable
                    },
                    AuthValidationMethod::ConfigurationCheck,
                )
            }
            crate::provider_catalog::LoginProviderTarget::Bedrock => {
                let (source, detail) = summarize_sources(vec![
                    env_source(crate::provider::bedrock::API_KEY_ENV),
                    config_source(
                        crate::provider::bedrock::API_KEY_ENV,
                        crate::provider::bedrock::ENV_FILE,
                        "~/.config/jcode/bedrock.env",
                    ),
                    env_source("AWS_PROFILE"),
                    env_source("JCODE_BEDROCK_PROFILE"),
                    env_source("AWS_ACCESS_KEY_ID"),
                ]);
                (
                    source,
                    detail,
                    AuthExpiryConfidence::Unknown,
                    AuthRefreshSupport::ExternalManaged,
                    AuthValidationMethod::PresenceCheck,
                )
            }
            crate::provider_catalog::LoginProviderTarget::OpenAiCompatible(profile) => {
                // Prefer the active named config profile's credential location
                // (set via `--provider-profile`) over the built-in profile env
                // so the reported source matches what runtime actually uses (#402).
                let (source, detail) = if let Some((key_env, env_file)) =
                    crate::provider_catalog::active_named_provider_profile_credential_source()
                {
                    summarize_sources(vec![
                        env_source(&key_env),
                        config_source(&key_env, &env_file, format!("~/.config/jcode/{}", env_file)),
                        external_api_key_source(&key_env),
                    ])
                } else {
                    let resolved =
                        crate::provider_catalog::resolve_openai_compatible_profile(profile);
                    summarize_sources(vec![
                        env_source(&resolved.api_key_env),
                        config_source(
                            &resolved.api_key_env,
                            &resolved.env_file,
                            format!("~/.config/jcode/{}", resolved.env_file),
                        ),
                        external_api_key_source(&resolved.api_key_env),
                    ])
                };
                (
                    source,
                    detail,
                    AuthExpiryConfidence::NotApplicable,
                    AuthRefreshSupport::NotApplicable,
                    AuthValidationMethod::PresenceCheck,
                )
            }
            _ => assessment_for_key(self, provider.auth_state_key, state),
        };

        ProviderAuthAssessment {
            state,
            readiness: auth_readiness_for_provider(provider, state, last_validation.as_ref()),
            method_detail,
            credential_source,
            credential_source_detail,
            expiry_confidence,
            refresh_support,
            validation_method,
            last_validation,
            last_refresh,
        }
    }

    /// Invalidate the cached auth status so the next `check()` does a fresh probe.
    pub fn invalidate_cache() {
        if let Ok(mut cache) = AUTH_STATUS_CACHE.write() {
            *cache = None;
        }
        if let Ok(mut cache) = AUTH_STATUS_FAST_CACHE.write() {
            *cache = None;
        }
        crate::auth::copilot::invalidate_github_token_cache();
        crate::logging::auth_event("auth_status_cache_invalidated", "all", &[]);
    }

    fn check_uncached() -> Self {
        let (status, _) = build_auth_status_uncached(AuthProbeMode::Full);
        log_auth_status_snapshot("auth_status_check", &status);
        status
    }

    fn check_uncached_fast() -> Self {
        let total_start = Instant::now();
        let (status, timings) = build_auth_status_uncached(AuthProbeMode::Fast);

        let nonzero: Vec<String> = timings
            .iter()
            .filter(|(_, ms)| *ms > 0)
            .map(|(name, ms)| format!("{name}={ms}ms"))
            .collect();
        if auth_timing_logging_enabled() {
            crate::logging::info(&format!(
                "[TIMING] auth_check_fast: total={}ms, nonzero=[{}]",
                total_start.elapsed().as_millis(),
                nonzero.join(", ")
            ));
        }

        log_auth_status_snapshot("auth_status_check_fast", &status);
        status
    }
}

fn build_auth_status_uncached(mode: AuthProbeMode) -> (AuthStatus, Vec<(&'static str, u128)>) {
    let mut status = AuthStatus::default();
    let mut timings = Vec::new();

    record_auth_probe_step(&mut timings, "jcode", || probe_jcode_status(&mut status));
    record_auth_probe_step(&mut timings, "anthropic", || {
        probe_anthropic_status(&mut status)
    });
    record_auth_probe_step(&mut timings, "openrouter", || {
        probe_openrouter_status(&mut status)
    });
    record_auth_probe_step(&mut timings, "azure", || probe_azure_status(&mut status));
    record_auth_probe_step(&mut timings, "bedrock", || {
        probe_bedrock_status(&mut status)
    });
    record_auth_probe_step(&mut timings, "openai", || probe_openai_status(&mut status));
    record_auth_probe_step(&mut timings, "copilot", || {
        probe_copilot_status(&mut status)
    });
    record_auth_probe_step(&mut timings, "antigravity", || {
        status.antigravity =
            token_state(antigravity::load_tokens().map(|tokens| tokens.is_expired()))
    });
    record_auth_probe_step(&mut timings, "gemini", || {
        // An official Gemini Developer API key is a static credential with no
        // expiry handshake, so treat its presence as immediately Available and
        // fall back to OAuth token state otherwise.
        status.gemini = if gemini::has_api_key() {
            AuthState::Available
        } else {
            token_state(gemini::load_tokens().map(|tokens| tokens.is_expired()))
        }
    });
    record_auth_probe_step(&mut timings, "cursor", || {
        probe_cursor_status(&mut status, mode)
    });
    record_auth_probe_step(&mut timings, "google", || probe_google_status(&mut status));

    (status, timings)
}

fn record_auth_probe_step(
    timings: &mut Vec<(&'static str, u128)>,
    name: &'static str,
    probe: impl FnOnce(),
) {
    let step_start = Instant::now();
    probe();
    timings.push((name, step_start.elapsed().as_millis()));
}

fn token_state(result: anyhow::Result<bool>) -> AuthState {
    match result {
        Ok(is_expired) => {
            if is_expired {
                AuthState::Expired
            } else {
                AuthState::Available
            }
        }
        Err(_) => AuthState::NotConfigured,
    }
}

fn probe_jcode_status(status: &mut AuthStatus) {
    if crate::subscription_catalog::has_credentials() {
        status.jcode = AuthState::Available;
    }
}

fn probe_anthropic_status(status: &mut AuthStatus) {
    let mut anthropic = ProviderAuth::default();

    if let Ok(creds) = claude::load_credentials() {
        let now_ms = chrono::Utc::now().timestamp_millis();
        anthropic.has_oauth = true;
        if creds.expires_at > now_ms {
            anthropic.state = AuthState::Available;
        } else {
            anthropic.state = AuthState::Expired;
        }
        // Record the OAuth credential's own state before the API key below can
        // mask an expired (or absent) OAuth login in the combined `state`.
        anthropic.oauth_state = anthropic.state;
    }

    // API key overrides expired OAuth.
    if crate::provider::anthropic::has_anthropic_api_key() {
        anthropic.has_api_key = true;
        anthropic.state = AuthState::Available;
    }

    status.anthropic = anthropic;
}

fn probe_openrouter_status(status: &mut AuthStatus) {
    if crate::provider::openrouter::OpenRouterProvider::has_credentials() {
        status.openrouter = AuthState::Available;
    }
}

fn probe_azure_status(status: &mut AuthStatus) {
    status.azure_has_api_key = crate::auth::azure::has_api_key();
    status.azure_uses_entra = crate::auth::azure::uses_entra_id();
    if crate::auth::azure::has_configuration() {
        status.azure = AuthState::Available;
    }
}

fn probe_bedrock_status(status: &mut AuthStatus) {
    if crate::provider::bedrock::BedrockProvider::has_credentials() {
        status.bedrock = AuthState::Available;
    }
}

fn probe_openai_status(status: &mut AuthStatus) {
    if let Ok(creds) = codex::load_credentials() {
        if !creds.refresh_token.is_empty() {
            status.openai_has_oauth = true;
            if let Some(expires_at) = creds.expires_at {
                let now_ms = chrono::Utc::now().timestamp_millis();
                if expires_at > now_ms {
                    status.openai = AuthState::Available;
                } else {
                    status.openai = AuthState::Expired;
                }
            } else {
                // No expiry info, assume available.
                status.openai = AuthState::Available;
            }
            // Record the OAuth credential's own state before the API key below
            // can mask an expired (or absent) OAuth login in the combined state.
            status.openai_oauth_state = status.openai;
        } else if !creds.access_token.is_empty() {
            status.openai_has_api_key = true;
            status.openai = AuthState::Available;
        }
    }

    // Fall back to env/config API key, or combine with OAuth.
    if openai_api_key_configured() {
        status.openai_has_api_key = true;
        status.openai = AuthState::Available;
    }
}

fn probe_copilot_status(status: &mut AuthStatus) {
    // If auth-test recently proved that the local Copilot OAuth token cannot
    // be exchanged, keep it visible as expired for diagnostics but do not let
    // startup/default-provider selection treat it as a usable API token.
    let (copilot_state, copilot_has_api_token) = copilot_auth_state_from_credentials();
    status.copilot = copilot_state;
    status.copilot_has_api_token = copilot_has_api_token;
}

fn probe_cursor_status(status: &mut AuthStatus, mode: AuthProbeMode) {
    match mode {
        AuthProbeMode::Full => {
            let cursor_has_api_key = cursor::has_cursor_api_key();
            let cursor_has_native_auth = cursor::has_cursor_native_auth();
            let cursor_has_cli_auth =
                !cursor_has_native_auth && cursor::has_authenticated_cli_session();
            status.cursor = if cursor_has_native_auth || cursor_has_cli_auth {
                AuthState::Available
            } else if cursor_has_api_key {
                AuthState::Expired
            } else {
                AuthState::NotConfigured
            };
        }
        AuthProbeMode::Fast => {
            // Avoid the vscdb/sqlite and CLI probes in fast UI paths.
            let cursor_has_api_key = cursor::has_cursor_api_key();
            let cursor_has_file_or_env_auth = cursor::load_access_token_from_env_or_file().is_ok();
            status.cursor = if cursor_has_file_or_env_auth || cursor_has_api_key {
                AuthState::Available
            } else {
                AuthState::NotConfigured
            };
        }
    }
}

fn probe_google_status(status: &mut AuthStatus) {
    match google::load_tokens() {
        Ok(tokens) => {
            if tokens.is_expired() {
                status.google = AuthState::Expired;
            } else {
                status.google = AuthState::Available;
            }
            status.google_can_send = tokens.tier.can_send();
        }
        Err(_) => {
            status.google = AuthState::NotConfigured;
        }
    }
}

fn assessment_for_key(
    status: &AuthStatus,
    key: LoginProviderAuthStateKey,
    state: AuthState,
) -> (
    AuthCredentialSource,
    String,
    AuthExpiryConfidence,
    AuthRefreshSupport,
    AuthValidationMethod,
) {
    match key {
        // The Claude/OpenAI rows that reach here are the OAuth/subscription
        // login providers; their API-key counterparts (`anthropic-api`,
        // `openai-api`) attribute sources separately via their
        // `LoginProviderTarget` arms. Describe only the OAuth credential here so
        // an API key never makes the OAuth row look configured (or vice versa).
        LoginProviderAuthStateKey::Anthropic => {
            let (source, detail) = summarize_sources(vec![anthropic_oauth_source(status)]);
            (
                source,
                detail,
                if status.anthropic.has_oauth {
                    AuthExpiryConfidence::Exact
                } else {
                    AuthExpiryConfidence::Unknown
                },
                if status.anthropic.has_oauth {
                    AuthRefreshSupport::Automatic
                } else {
                    AuthRefreshSupport::Unknown
                },
                if status.anthropic.has_oauth {
                    AuthValidationMethod::TimestampCheck
                } else {
                    AuthValidationMethod::PresenceCheck
                },
            )
        }
        LoginProviderAuthStateKey::OpenAi => {
            let (source, detail) = summarize_sources(vec![openai_oauth_source(status)]);
            (
                source,
                detail,
                if status.openai_has_oauth {
                    AuthExpiryConfidence::Exact
                } else {
                    AuthExpiryConfidence::Unknown
                },
                if status.openai_has_oauth {
                    AuthRefreshSupport::Automatic
                } else {
                    AuthRefreshSupport::Unknown
                },
                if status.openai_has_oauth {
                    AuthValidationMethod::TimestampCheck
                } else {
                    AuthValidationMethod::PresenceCheck
                },
            )
        }
        LoginProviderAuthStateKey::Copilot => {
            let (source, detail) = summarize_sources(vec![copilot_source()]);
            (
                source,
                detail,
                if state == AuthState::Available {
                    AuthExpiryConfidence::PresenceOnly
                } else {
                    AuthExpiryConfidence::Unknown
                },
                AuthRefreshSupport::ManualRelogin,
                AuthValidationMethod::CompositeProbe,
            )
        }
        LoginProviderAuthStateKey::Antigravity => {
            let (source, detail) = summarize_sources(vec![antigravity_source()]);
            (
                source,
                detail,
                if state == AuthState::NotConfigured {
                    AuthExpiryConfidence::Unknown
                } else {
                    AuthExpiryConfidence::Exact
                },
                AuthRefreshSupport::Automatic,
                AuthValidationMethod::TimestampCheck,
            )
        }
        LoginProviderAuthStateKey::Gemini => {
            let (source, detail) = summarize_sources(vec![gemini_source()]);
            (
                source,
                detail,
                if state == AuthState::NotConfigured {
                    AuthExpiryConfidence::Unknown
                } else {
                    AuthExpiryConfidence::Exact
                },
                AuthRefreshSupport::Automatic,
                AuthValidationMethod::TimestampCheck,
            )
        }
        LoginProviderAuthStateKey::Cursor => {
            let (source, detail) = summarize_sources(vec![cursor_source()]);
            (
                source,
                detail,
                if state == AuthState::Available {
                    AuthExpiryConfidence::PresenceOnly
                } else {
                    AuthExpiryConfidence::Unknown
                },
                AuthRefreshSupport::Conditional,
                AuthValidationMethod::CompositeProbe,
            )
        }
        LoginProviderAuthStateKey::Google => {
            let (source, detail) = summarize_sources(vec![google_source()]);
            (
                source,
                detail,
                if state == AuthState::NotConfigured {
                    AuthExpiryConfidence::Unknown
                } else {
                    AuthExpiryConfidence::Exact
                },
                AuthRefreshSupport::Automatic,
                AuthValidationMethod::TimestampCheck,
            )
        }
        LoginProviderAuthStateKey::Jcode
        | LoginProviderAuthStateKey::Azure
        | LoginProviderAuthStateKey::Bedrock
        | LoginProviderAuthStateKey::OpenRouterLike
        | LoginProviderAuthStateKey::ExternalImport => (
            AuthCredentialSource::None,
            "not configured".to_string(),
            AuthExpiryConfidence::Unknown,
            AuthRefreshSupport::Unknown,
            AuthValidationMethod::Unknown,
        ),
    }
}

fn summarize_sources(
    sources: Vec<Option<(AuthCredentialSource, String)>>,
) -> (AuthCredentialSource, String) {
    let mut collected = Vec::new();
    for source in sources.into_iter().flatten() {
        if !collected.iter().any(|(_, detail)| detail == &source.1) {
            collected.push(source);
        }
    }
    match collected.len() {
        0 => (AuthCredentialSource::None, "not configured".to_string()),
        1 => {
            let mut iter = collected.into_iter();
            if let Some(only) = iter.next() {
                only
            } else {
                unreachable!("collected.len() == 1 but no source was present")
            }
        }
        _ => (
            AuthCredentialSource::Mixed,
            collected
                .into_iter()
                .map(|(_, detail)| detail)
                .collect::<Vec<_>>()
                .join(" + "),
        ),
    }
}

fn env_source(env_key: &str) -> Option<(AuthCredentialSource, String)> {
    env_var_nonempty(env_key).then(|| {
        (
            AuthCredentialSource::EnvironmentVariable,
            format!("{env_key} environment variable"),
        )
    })
}

fn config_source(
    env_key: &str,
    file_name: &str,
    path_label: impl Into<String>,
) -> Option<(AuthCredentialSource, String)> {
    config_file_has_key(file_name, env_key).then(|| {
        (
            AuthCredentialSource::AppConfigFile,
            format!("{} ({env_key})", path_label.into()),
        )
    })
}

fn external_api_key_source(env_key: &str) -> Option<(AuthCredentialSource, String)> {
    crate::auth::external::load_api_key_for_env(env_key).map(|_| {
        (
            AuthCredentialSource::TrustedExternalFile,
            format!("trusted external auth import ({env_key})"),
        )
    })
}

fn azure_entra_source() -> Option<(AuthCredentialSource, String)> {
    crate::auth::azure::uses_entra_id().then(|| {
        (
            AuthCredentialSource::AzureDefaultCredential,
            "Azure DefaultAzureCredential".to_string(),
        )
    })
}

fn anthropic_oauth_source(status: &AuthStatus) -> Option<(AuthCredentialSource, String)> {
    if !status.anthropic.has_oauth {
        return None;
    }
    if !crate::auth::claude::list_accounts()
        .unwrap_or_default()
        .is_empty()
    {
        return Some((
            AuthCredentialSource::JcodeManagedFile,
            "~/.jcode/auth.json".to_string(),
        ));
    }
    if let Some(source) = crate::auth::claude::preferred_external_auth_source()
        && let Ok(path) = source.path()
        && crate::config::Config::external_auth_source_allowed_for_path(source.source_id(), &path)
    {
        return Some((
            AuthCredentialSource::TrustedExternalFile,
            format!("trusted external file ({})", path.display()),
        ));
    }
    if crate::auth::external::load_anthropic_oauth_tokens().is_some() {
        return Some((
            AuthCredentialSource::TrustedExternalFile,
            "trusted external auth import".to_string(),
        ));
    }
    None
}

fn openai_oauth_source(status: &AuthStatus) -> Option<(AuthCredentialSource, String)> {
    if !status.openai_has_oauth {
        return None;
    }
    if !crate::auth::codex::list_accounts()
        .unwrap_or_default()
        .is_empty()
    {
        return Some((
            AuthCredentialSource::JcodeManagedFile,
            "~/.jcode/openai-auth.json".to_string(),
        ));
    }
    if crate::auth::codex::legacy_auth_allowed() && crate::auth::codex::legacy_auth_source_exists()
    {
        return Some((
            AuthCredentialSource::TrustedExternalFile,
            "trusted legacy Codex auth file".to_string(),
        ));
    }
    if crate::auth::external::load_openai_oauth_tokens().is_some() {
        return Some((
            AuthCredentialSource::TrustedExternalFile,
            "trusted external auth import".to_string(),
        ));
    }
    None
}

fn gemini_source() -> Option<(AuthCredentialSource, String)> {
    if let Ok(path) = crate::auth::gemini::tokens_path()
        && path.exists()
    {
        return Some((
            AuthCredentialSource::JcodeManagedFile,
            format!("{}", path.display()),
        ));
    }
    if let Ok(path) = crate::auth::gemini::gemini_cli_oauth_path()
        && path.exists()
        && crate::config::Config::external_auth_source_allowed_for_path(
            crate::auth::gemini::GEMINI_CLI_AUTH_SOURCE_ID,
            &path,
        )
    {
        return Some((
            AuthCredentialSource::TrustedExternalFile,
            format!("trusted Gemini CLI file ({})", path.display()),
        ));
    }
    crate::auth::external::load_gemini_oauth_tokens().map(|_| {
        (
            AuthCredentialSource::TrustedExternalFile,
            "trusted external auth import".to_string(),
        )
    })
}

fn antigravity_source() -> Option<(AuthCredentialSource, String)> {
    if let Ok(path) = crate::auth::antigravity::tokens_path()
        && path.exists()
    {
        return Some((
            AuthCredentialSource::JcodeManagedFile,
            format!("{}", path.display()),
        ));
    }
    crate::auth::external::load_antigravity_oauth_tokens().map(|_| {
        (
            AuthCredentialSource::TrustedExternalFile,
            "trusted external auth import".to_string(),
        )
    })
}

fn google_source() -> Option<(AuthCredentialSource, String)> {
    if let (Ok(tokens_path), Ok(credentials_path)) = (
        crate::auth::google::tokens_path(),
        crate::auth::google::credentials_path(),
    ) && tokens_path.exists()
        && credentials_path.exists()
    {
        return Some((
            AuthCredentialSource::JcodeManagedFile,
            format!("{} + {}", credentials_path.display(), tokens_path.display()),
        ));
    }
    None
}

fn cursor_source() -> Option<(AuthCredentialSource, String)> {
    if env_var_nonempty("CURSOR_ACCESS_TOKEN") || env_var_nonempty("CURSOR_API_KEY") {
        return Some((
            AuthCredentialSource::EnvironmentVariable,
            "CURSOR_ACCESS_TOKEN / CURSOR_API_KEY environment variable".to_string(),
        ));
    }
    if let Ok(file_path) = crate::auth::cursor::cursor_auth_file_path()
        && file_path.exists()
        && crate::config::Config::external_auth_source_allowed_for_path(
            crate::auth::cursor::CURSOR_AUTH_FILE_SOURCE_ID,
            &file_path,
        )
    {
        return Some((
            AuthCredentialSource::TrustedExternalFile,
            format!("trusted Cursor auth file ({})", file_path.display()),
        ));
    }
    if let Some(source) = crate::auth::cursor::preferred_external_auth_source()
        && matches!(
            source,
            crate::auth::cursor::ExternalCursorAuthSource::CursorVscdb
        )
        && let Ok(path) = source.path()
    {
        return Some((
            AuthCredentialSource::TrustedExternalAppState,
            format!("trusted Cursor app state ({})", path.display()),
        ));
    }
    if config_source("CURSOR_API_KEY", "cursor.env", "~/.config/jcode/cursor.env").is_some() {
        return config_source("CURSOR_API_KEY", "cursor.env", "~/.config/jcode/cursor.env");
    }
    None
}

fn copilot_source() -> Option<(AuthCredentialSource, String)> {
    if env_var_nonempty("COPILOT_GITHUB_TOKEN")
        || env_var_nonempty("GH_TOKEN")
        || env_var_nonempty("GITHUB_TOKEN")
    {
        return Some((
            AuthCredentialSource::EnvironmentVariable,
            "COPILOT_GITHUB_TOKEN / GH_TOKEN / GITHUB_TOKEN".to_string(),
        ));
    }

    for source in [
        crate::auth::copilot::ExternalCopilotAuthSource::ConfigJson,
        crate::auth::copilot::ExternalCopilotAuthSource::HostsJson,
        crate::auth::copilot::ExternalCopilotAuthSource::AppsJson,
    ] {
        let path = source.path();
        if path.exists()
            && crate::config::Config::external_auth_source_allowed_for_path(
                source.source_id(),
                &path,
            )
        {
            return Some((
                AuthCredentialSource::TrustedExternalFile,
                format!("trusted Copilot file ({})", path.display()),
            ));
        }
    }

    if crate::auth::external::load_copilot_oauth_token().is_some() {
        return Some((
            AuthCredentialSource::TrustedExternalFile,
            "trusted external auth import".to_string(),
        ));
    }

    crate::auth::copilot::load_github_token().ok().map(|_| {
        (
            AuthCredentialSource::LocalCliSession,
            "gh CLI token fallback".to_string(),
        )
    })
}

fn env_var_nonempty(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

fn config_file_has_key(file_name: &str, env_key: &str) -> bool {
    let Ok(config_dir) = crate::storage::app_config_dir() else {
        return false;
    };
    let path = config_dir.join(file_name);
    config_file_contains_assignment(&path, env_key)
}

fn config_file_contains_assignment(path: &Path, env_key: &str) -> bool {
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    let prefix = format!("{env_key}=");
    content.lines().any(|line| {
        line.strip_prefix(&prefix)
            .map(|value| !value.trim().trim_matches('"').trim_matches('\'').is_empty())
            .unwrap_or(false)
    })
}

fn api_key_available(env_key: &str, file_name: &str) -> bool {
    crate::provider_catalog::load_api_key_from_env_or_config(env_key, file_name).is_some()
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
