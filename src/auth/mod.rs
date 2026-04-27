pub mod account_store;
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
pub mod login_diagnostics;
pub mod login_flows;
pub mod oauth;
pub mod refresh_state;
mod status_types;
pub mod validation;

pub(crate) use commands::{command_available_from_env, command_exists};
#[cfg(test)]
pub(crate) use commands::{
    command_candidates, contains_path_separator, dedup_preserve_order, has_extension,
    is_wsl2_windows_path,
};

pub use status_types::{
    AuthCredentialSource, AuthExpiryConfidence, AuthRefreshSupport, AuthState, AuthStatus,
    AuthValidationMethod, ProviderAuth, ProviderAuthAssessment,
};

use crate::provider_catalog::LoginProviderAuthStateKey;
use crate::provider_catalog::LoginProviderDescriptor;
use crate::provider_catalog::openrouter_like_api_key_sources;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, RwLock};
use std::time::Instant;

static AUTH_STATUS_CACHE: std::sync::LazyLock<RwLock<Option<(AuthStatus, Instant)>>> =
    std::sync::LazyLock::new(|| RwLock::new(None));
static AUTH_STATUS_FAST_CACHE: std::sync::LazyLock<RwLock<Option<(AuthStatus, Instant)>>> =
    std::sync::LazyLock::new(|| RwLock::new(None));

const AUTH_STATUS_CACHE_TTL_SECS: u64 = 30;
const AUTH_STATUS_FAST_CACHE_TTL_SECS: u64 = 5;

/// Per-process cache for command existence lookups.
/// CLI tools don't get installed/uninstalled while jcode is running, so caching
/// indefinitely per process is correct and avoids repeated PATH scans.
static COMMAND_EXISTS_CACHE: std::sync::LazyLock<Mutex<HashMap<String, bool>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn browser_suppressed(cli_no_browser: bool) -> bool {
    cli_no_browser || env_truthy("NO_BROWSER") || env_truthy("JCODE_NO_BROWSER")
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

impl AuthStatus {
    /// Check all authentication sources and return their status.
    /// Results are cached for 30 seconds to avoid expensive PATH scanning on every frame.
    pub fn check() -> Self {
        if let Ok(cache) = AUTH_STATUS_CACHE.read()
            && let Some((ref status, ref when)) = *cache
            && when.elapsed().as_secs() < AUTH_STATUS_CACHE_TTL_SECS
        {
            return status.clone();
        }

        let status = Self::check_uncached();

        if let Ok(mut cache) = AUTH_STATUS_CACHE.write() {
            *cache = Some((status.clone(), Instant::now()));
        }
        if let Ok(mut cache) = AUTH_STATUS_FAST_CACHE.write() {
            *cache = Some((status.clone(), Instant::now()));
        }

        status
    }

    /// Fast auth snapshot for interactive UI surfaces like `/account`.
    ///
    /// Prefers any previously cached full probe (even if stale), and otherwise
    /// falls back to a cheap local-files/env-only probe that avoids subprocesses
    /// such as `cursor-agent status` or `sqlite3` lookups.
    pub fn check_fast() -> Self {
        if let Ok(cache) = AUTH_STATUS_CACHE.read()
            && let Some((ref status, _)) = *cache
        {
            return status.clone();
        }

        if let Ok(cache) = AUTH_STATUS_FAST_CACHE.read()
            && let Some((ref status, ref when)) = *cache
            && when.elapsed().as_secs() < AUTH_STATUS_FAST_CACHE_TTL_SECS
        {
            return status.clone();
        }

        let status = Self::check_uncached_fast();
        if let Ok(mut cache) = AUTH_STATUS_FAST_CACHE.write() {
            *cache = Some((status.clone(), Instant::now()));
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
            || self.copilot == AuthState::Available
            || self.antigravity == AuthState::Available
            || self.gemini == AuthState::Available
            || self.cursor == AuthState::Available
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
            crate::provider_catalog::LoginProviderTarget::Azure => {
                if crate::auth::azure::has_configuration() {
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
            crate::provider_catalog::LoginProviderTarget::Azure => {
                if self.state_for_provider(provider) == AuthState::Available {
                    crate::auth::azure::method_detail()
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
                LoginProviderAuthStateKey::Anthropic => {
                    let detail = if self.anthropic.has_oauth && self.anthropic.has_api_key {
                        "OAuth + API key"
                    } else if self.anthropic.has_oauth {
                        "OAuth"
                    } else if self.anthropic.has_api_key {
                        "API key"
                    } else {
                        "not configured"
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
                LoginProviderAuthStateKey::OpenAi => {
                    let detail = if self.openai_has_oauth && self.openai_has_api_key {
                        "OAuth + API key"
                    } else if self.openai_has_oauth {
                        "OAuth"
                    } else if self.openai_has_api_key {
                        "API key"
                    } else {
                        "not configured"
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
            crate::provider_catalog::LoginProviderTarget::OpenAiCompatible(profile) => {
                let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
                let (source, detail) = summarize_sources(vec![
                    env_source(&resolved.api_key_env),
                    config_source(
                        &resolved.api_key_env,
                        &resolved.env_file,
                        format!("~/.config/jcode/{}", resolved.env_file),
                    ),
                    external_api_key_source(&resolved.api_key_env),
                ]);
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
    }

    fn check_uncached() -> Self {
        let mut status = Self::default();

        if crate::subscription_catalog::has_credentials() {
            status.jcode = AuthState::Available;
        }

        // Check Anthropic (OAuth or API key)
        let mut anthropic = ProviderAuth::default();

        // Check OAuth
        if let Ok(creds) = claude::load_credentials() {
            let now_ms = chrono::Utc::now().timestamp_millis();
            anthropic.has_oauth = true;
            if creds.expires_at > now_ms {
                anthropic.state = AuthState::Available;
            } else {
                anthropic.state = AuthState::Expired;
            }
        }

        // Check API key (overrides expired OAuth)
        if std::env::var("ANTHROPIC_API_KEY").is_ok() {
            anthropic.has_api_key = true;
            anthropic.state = AuthState::Available;
        }

        status.anthropic = anthropic;

        // Check OpenRouter/OpenAI-compatible API keys (env var or config file)
        let openrouter_like_keys = openrouter_like_api_key_sources();
        let openrouter_available = openrouter_like_keys
            .iter()
            .any(|(env_key, file_name)| api_key_available(env_key, file_name));

        if openrouter_available {
            status.openrouter = AuthState::Available;
        }

        status.azure_has_api_key = crate::auth::azure::has_api_key();
        status.azure_uses_entra = crate::auth::azure::uses_entra_id();
        if crate::auth::azure::has_configuration() {
            status.azure = AuthState::Available;
        }

        // Check OpenAI (Codex OAuth or API key)
        if let Ok(creds) = codex::load_credentials() {
            // Check if we have OAuth tokens (not just API key fallback)
            if !creds.refresh_token.is_empty() {
                status.openai_has_oauth = true;
                // Has OAuth - check expiry if available
                if let Some(expires_at) = creds.expires_at {
                    let now_ms = chrono::Utc::now().timestamp_millis();
                    if expires_at > now_ms {
                        status.openai = AuthState::Available;
                    } else {
                        status.openai = AuthState::Expired;
                    }
                } else {
                    // No expiry info, assume available
                    status.openai = AuthState::Available;
                }
            } else if !creds.access_token.is_empty() {
                // API key fallback
                status.openai_has_api_key = true;
                status.openai = AuthState::Available;
            }
        }

        // Fall back to env var (or combine with OAuth)
        if std::env::var("OPENAI_API_KEY")
            .ok()
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
        {
            status.openai_has_api_key = true;
            status.openai = AuthState::Available;
        }

        // Check external/CLI auth providers (presence of installed CLI tooling)
        status.copilot = if copilot::has_copilot_credentials_fast() {
            status.copilot_has_api_token = true;
            AuthState::Available
        } else {
            AuthState::NotConfigured
        };

        status.antigravity = match antigravity::load_tokens() {
            Ok(tokens) => {
                if tokens.is_expired() {
                    AuthState::Expired
                } else {
                    AuthState::Available
                }
            }
            Err(_) => AuthState::NotConfigured,
        };

        status.gemini = match gemini::load_tokens() {
            Ok(tokens) => {
                if tokens.is_expired() {
                    AuthState::Expired
                } else {
                    AuthState::Available
                }
            }
            Err(_) => AuthState::NotConfigured,
        };

        let cursor_has_cli = cursor::has_cursor_agent_cli();
        let cursor_has_api_key = cursor::has_cursor_api_key();
        let cursor_has_native_auth = cursor::has_cursor_native_auth();
        let cursor_has_cli_auth = if cursor_has_cli {
            cursor::has_cursor_agent_auth()
        } else {
            false
        };

        status.cursor = if cursor_has_native_auth || (cursor_has_cli && cursor_has_cli_auth) {
            AuthState::Available
        } else if cursor_has_cli || cursor_has_api_key {
            AuthState::Expired
        } else {
            AuthState::NotConfigured
        };

        // Check Google/Gmail OAuth
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

        status
    }

    fn check_uncached_fast() -> Self {
        let total_start = Instant::now();
        let mut status = Self::default();
        let mut timings = Vec::new();

        let step_start = Instant::now();
        if crate::subscription_catalog::has_credentials() {
            status.jcode = AuthState::Available;
        }
        timings.push(("jcode", step_start.elapsed().as_millis()));

        let step_start = Instant::now();
        let mut anthropic = ProviderAuth::default();
        if let Ok(creds) = claude::load_credentials() {
            let now_ms = chrono::Utc::now().timestamp_millis();
            anthropic.has_oauth = true;
            if creds.expires_at > now_ms {
                anthropic.state = AuthState::Available;
            } else {
                anthropic.state = AuthState::Expired;
            }
        }
        if std::env::var("ANTHROPIC_API_KEY").is_ok() {
            anthropic.has_api_key = true;
            anthropic.state = AuthState::Available;
        }
        status.anthropic = anthropic;
        timings.push(("anthropic", step_start.elapsed().as_millis()));

        let step_start = Instant::now();
        let openrouter_like_keys = openrouter_like_api_key_sources();
        let openrouter_available = openrouter_like_keys
            .iter()
            .any(|(env_key, file_name)| api_key_available(env_key, file_name));
        if openrouter_available {
            status.openrouter = AuthState::Available;
        }
        timings.push(("openrouter", step_start.elapsed().as_millis()));

        let step_start = Instant::now();
        status.azure_has_api_key = crate::auth::azure::has_api_key();
        status.azure_uses_entra = crate::auth::azure::uses_entra_id();
        if crate::auth::azure::has_configuration() {
            status.azure = AuthState::Available;
        }
        timings.push(("azure", step_start.elapsed().as_millis()));

        let step_start = Instant::now();
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
                    status.openai = AuthState::Available;
                }
            } else if !creds.access_token.is_empty() {
                status.openai_has_api_key = true;
                status.openai = AuthState::Available;
            }
        }
        if std::env::var("OPENAI_API_KEY")
            .ok()
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
        {
            status.openai_has_api_key = true;
            status.openai = AuthState::Available;
        }
        timings.push(("openai", step_start.elapsed().as_millis()));

        let step_start = Instant::now();
        status.copilot = if copilot::has_copilot_credentials() {
            status.copilot_has_api_token = true;
            AuthState::Available
        } else {
            AuthState::NotConfigured
        };
        timings.push(("copilot", step_start.elapsed().as_millis()));

        let step_start = Instant::now();
        status.antigravity = match antigravity::load_tokens() {
            Ok(tokens) => {
                if tokens.is_expired() {
                    AuthState::Expired
                } else {
                    AuthState::Available
                }
            }
            Err(_) => AuthState::NotConfigured,
        };
        timings.push(("antigravity", step_start.elapsed().as_millis()));

        let step_start = Instant::now();
        status.gemini = match gemini::load_tokens() {
            Ok(tokens) => {
                if tokens.is_expired() {
                    AuthState::Expired
                } else {
                    AuthState::Available
                }
            }
            Err(_) => AuthState::NotConfigured,
        };
        timings.push(("gemini", step_start.elapsed().as_millis()));

        let step_start = Instant::now();
        let cursor_has_cli = cursor::has_cursor_agent_cli();
        let cursor_has_api_key = cursor::has_cursor_api_key();
        let cursor_has_file_or_env_auth = cursor::load_access_token_from_env_or_file().is_ok();

        status.cursor = if cursor_has_file_or_env_auth || cursor_has_api_key {
            AuthState::Available
        } else if cursor_has_cli {
            AuthState::Expired
        } else {
            AuthState::NotConfigured
        };
        timings.push(("cursor", step_start.elapsed().as_millis()));

        let step_start = Instant::now();
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

        timings.push(("google", step_start.elapsed().as_millis()));

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

        status
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
        LoginProviderAuthStateKey::Anthropic => {
            let (source, detail) = summarize_sources(vec![
                anthropic_oauth_source(status),
                env_source("ANTHROPIC_API_KEY"),
            ]);
            (
                source,
                detail,
                if status.anthropic.has_oauth {
                    AuthExpiryConfidence::Exact
                } else if status.anthropic.has_api_key {
                    AuthExpiryConfidence::NotApplicable
                } else {
                    AuthExpiryConfidence::Unknown
                },
                if status.anthropic.has_oauth {
                    AuthRefreshSupport::Automatic
                } else if status.anthropic.has_api_key {
                    AuthRefreshSupport::NotApplicable
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
            let (source, detail) = summarize_sources(vec![
                openai_oauth_source(status),
                openai_api_key_source(status),
            ]);
            (
                source,
                detail,
                if status.openai_has_oauth {
                    AuthExpiryConfidence::Exact
                } else if status.openai_has_api_key {
                    AuthExpiryConfidence::NotApplicable
                } else {
                    AuthExpiryConfidence::Unknown
                },
                if status.openai_has_oauth {
                    AuthRefreshSupport::Automatic
                } else if status.openai_has_api_key {
                    AuthRefreshSupport::NotApplicable
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

fn openai_api_key_source(status: &AuthStatus) -> Option<(AuthCredentialSource, String)> {
    if !status.openai_has_api_key {
        return None;
    }
    env_source("OPENAI_API_KEY").or_else(|| {
        (crate::auth::codex::legacy_auth_allowed()
            && crate::auth::codex::legacy_auth_source_exists())
        .then(|| {
            (
                AuthCredentialSource::TrustedExternalFile,
                "trusted legacy Codex API key".to_string(),
            )
        })
    })
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
    if crate::auth::cursor::has_cursor_agent_auth() {
        return Some((
            AuthCredentialSource::LocalCliSession,
            "cursor-agent authenticated session".to_string(),
        ));
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
