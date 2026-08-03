use anyhow::Result;
use std::collections::BTreeMap;
use std::path::PathBuf;

const REFRESH_STATUS_FILE: &str = "auth-refresh-state.json";
const MAX_ERROR_CHARS: usize = 240;

pub use jcode_auth_types::ProviderRefreshRecord;

/// Lifecycle of one provider credential, as observed from refresh outcomes.
///
/// This is the single source of truth for "can we use / should we retry this
/// credential", replacing the per-provider ad-hoc checks that let a permanently
/// dead OpenAI refresh token get re-attempted every 15 minutes for two days
/// while the equivalent Claude token was correctly suppressed.
///
/// The important property is that [`CredState::Rejected`] is **terminal for a
/// given token fingerprint**: nothing but minting a new token clears it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredState {
    /// No credential recorded for this provider at all.
    Absent,
    /// A credential exists but has never been observed working.
    Present,
    /// A refresh succeeded at some point and nothing has failed since.
    Verified,
    /// A refresh failed transiently. Still worth retrying.
    Stale,
    /// The provider permanently rejected this credential. Terminal: no
    /// background sweep, catalog refresh, or per-turn retry may attempt it.
    Rejected,
}

impl CredState {
    /// Stable label for logs, telemetry, and UI. Closed vocabulary: these
    /// strings are safe to send verbatim in telemetry.
    pub fn label(self) -> &'static str {
        match self {
            CredState::Absent => "absent",
            CredState::Present => "present",
            CredState::Verified => "verified",
            CredState::Stale => "stale",
            CredState::Rejected => "rejected",
        }
    }

    /// Human-facing status line used by onboarding and `/login` surfaces.
    ///
    /// Deriving every label from one function is what makes it impossible to
    /// show "login expired" for a provider that was never configured.
    pub fn user_facing_label(self) -> &'static str {
        match self {
            CredState::Absent => "not configured",
            CredState::Present => "configured, not yet verified",
            CredState::Verified => "ready",
            CredState::Stale => "needs a retry",
            CredState::Rejected => "login expired, sign in again",
        }
    }

    /// Whether a refresh attempt against this credential can possibly succeed.
    pub fn is_retryable(self) -> bool {
        !matches!(self, CredState::Rejected)
    }

    /// Whether the credential is currently believed usable without user action.
    pub fn is_usable(self) -> bool {
        matches!(
            self,
            CredState::Present | CredState::Verified | CredState::Stale
        )
    }
}

/// Classify a provider's credential from its recorded refresh history.
///
/// `refresh_token` is optional because some providers (API keys) have nothing
/// to fingerprint; without it we cannot distinguish "this exact token was
/// rejected" from "some older token was", so we conservatively report
/// [`CredState::Stale`] and allow a retry.
pub fn cred_state(provider_id: &str, refresh_token: Option<&str>) -> CredState {
    let Some(record) = get(provider_id) else {
        return CredState::Absent;
    };
    if let Some(token) = refresh_token
        && refresh_token_is_known_rejected(provider_id, token)
    {
        return CredState::Rejected;
    }
    match (
        record.last_error.is_some(),
        record.last_success_ms.is_some(),
    ) {
        (true, _) => CredState::Stale,
        (false, true) => CredState::Verified,
        (false, false) => CredState::Present,
    }
}

/// Guard to call before spending a network round-trip on a token refresh.
///
/// Returns an error describing the permanent rejection when the exact token was
/// already rejected, so callers can fall through to their own fallback (API key,
/// another provider) instead of retrying something that cannot recover.
pub fn ensure_refresh_allowed(
    provider_id: &str,
    refresh_token: &str,
    relogin_hint: &str,
) -> Result<()> {
    if refresh_token_is_known_rejected(provider_id, refresh_token) {
        anyhow::bail!(
            "{provider_id} refresh token was previously rejected by the provider and cannot be refreshed. {relogin_hint}"
        );
    }
    Ok(())
}

/// Record the outcome of a refresh attempt, routing permanent rejections into
/// the terminal state and everything else into the retryable one.
///
/// Every provider must funnel failures through here rather than calling
/// [`record_failure`] directly, otherwise it silently opts out of the
/// terminal-rejection guarantee.
pub fn record_refresh_outcome<T>(provider_id: &str, refresh_token: &str, result: &Result<T>) {
    match result {
        Ok(_) => {
            let _ = record_success(provider_id);
        }
        Err(err) => {
            let message = err.to_string();
            if error_is_permanent_rejection(&message) {
                let _ = record_permanent_rejection(provider_id, refresh_token, &message);
            } else {
                let _ = record_failure(provider_id, &message);
            }
        }
    }
}

pub fn status_path() -> Result<PathBuf> {
    Ok(crate::storage::jcode_dir()?.join(REFRESH_STATUS_FILE))
}

pub fn load_all() -> BTreeMap<String, ProviderRefreshRecord> {
    let Ok(path) = status_path() else {
        return BTreeMap::new();
    };
    crate::storage::read_json(&path).unwrap_or_default()
}

pub fn get(provider_id: &str) -> Option<ProviderRefreshRecord> {
    load_all().get(provider_id).cloned()
}

pub fn record_success(provider_id: &str) -> Result<()> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    upsert(
        provider_id,
        ProviderRefreshRecord {
            last_attempt_ms: now_ms,
            last_success_ms: Some(now_ms),
            last_error: None,
            rejected_refresh_fingerprint: None,
        },
    )
}

pub fn record_failure(provider_id: &str, error: impl AsRef<str>) -> Result<()> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut message = error.as_ref().trim().to_string();
    if message.chars().count() > MAX_ERROR_CHARS {
        message = message.chars().take(MAX_ERROR_CHARS).collect::<String>();
        message.push('…');
    }
    let mut record = get(provider_id).unwrap_or(ProviderRefreshRecord {
        last_attempt_ms: now_ms,
        last_success_ms: None,
        last_error: None,
        rejected_refresh_fingerprint: None,
    });
    record.last_attempt_ms = now_ms;
    record.last_error = Some(message);
    upsert(provider_id, record)
}

/// Stable, non-reversible fingerprint of a refresh token.
///
/// Only the fingerprint is persisted, never the token itself, so a leaked
/// refresh-state file cannot be replayed against the provider.
pub fn refresh_token_fingerprint(refresh_token: &str) -> String {
    use std::hash::{DefaultHasher, Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    refresh_token.trim().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Record that the provider permanently rejected this refresh token, so
/// callers can skip re-attempting a refresh that cannot succeed.
pub fn record_permanent_rejection(
    provider_id: &str,
    refresh_token: &str,
    error: impl AsRef<str>,
) -> Result<()> {
    record_failure(provider_id, error)?;
    let mut record = get(provider_id).unwrap_or(ProviderRefreshRecord {
        last_attempt_ms: chrono::Utc::now().timestamp_millis(),
        last_success_ms: None,
        last_error: None,
        rejected_refresh_fingerprint: None,
    });
    record.rejected_refresh_fingerprint = Some(refresh_token_fingerprint(refresh_token));
    upsert(provider_id, record)
}

/// True when this exact refresh token was already rejected as unrecoverable.
///
/// A newly minted token has a different fingerprint, so re-login re-enables
/// refresh immediately with no cache to clear.
pub fn refresh_token_is_known_rejected(provider_id: &str, refresh_token: &str) -> bool {
    if refresh_token.trim().is_empty() {
        return false;
    }
    get(provider_id)
        .and_then(|record| record.rejected_refresh_fingerprint)
        .is_some_and(|fingerprint| fingerprint == refresh_token_fingerprint(refresh_token))
}

/// True when the provider's response describes an unrecoverable refresh token
/// (revoked, unknown, or otherwise permanently invalid) rather than a transient
/// network/5xx failure that is worth retrying.
pub fn error_is_permanent_rejection(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    [
        "invalid_grant",
        "refresh_token_invalidated",
        "refresh token not found",
        "invalid refresh token",
        "token has been revoked",
        // Cursor answers a dead refresh token with an explicit logout request
        // rather than an OAuth error code.
        "requested logout/login",
        // OpenAI's user-facing phrasing for a revoked session.
        "your session has ended",
        "unauthorized_client",
    ]
    .iter()
    .any(|marker| error.contains(marker))
}

pub fn format_record_label(record: &ProviderRefreshRecord) -> String {
    let age = age_label(record.last_attempt_ms);
    if let Some(error) = record.last_error.as_deref() {
        format!("failed {} ({})", age, error)
    } else if record.last_success_ms.is_some() {
        format!("ok {}", age)
    } else {
        format!("attempted {}", age)
    }
}

fn upsert(provider_id: &str, record: ProviderRefreshRecord) -> Result<()> {
    let mut records = load_all();
    records.insert(provider_id.to_string(), record);
    crate::storage::write_json(&status_path()?, &records)
}

fn age_label(checked_at_ms: i64) -> String {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let delta_ms = now_ms.saturating_sub(checked_at_ms).max(0);
    let delta_secs = delta_ms / 1000;
    match delta_secs {
        0..=89 => "just now".to_string(),
        90..=3599 => format!("{}m ago", delta_secs / 60),
        3600..=86_399 => format!("{}h ago", delta_secs / 3600),
        _ => format!("{}d ago", delta_secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_record_label_prefers_failure_details() {
        let record = ProviderRefreshRecord {
            last_attempt_ms: chrono::Utc::now().timestamp_millis(),
            last_success_ms: Some(chrono::Utc::now().timestamp_millis()),
            last_error: Some("refresh denied".to_string()),
            rejected_refresh_fingerprint: None,
        };
        assert!(format_record_label(&record).contains("failed"));
        assert!(format_record_label(&record).contains("refresh denied"));
    }

    #[test]
    fn format_record_label_reports_success() {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let record = ProviderRefreshRecord {
            last_attempt_ms: now_ms,
            last_success_ms: Some(now_ms),
            last_error: None,
            rejected_refresh_fingerprint: None,
        };
        assert!(format_record_label(&record).starts_with("ok "));
    }

    #[test]
    fn permanent_rejection_is_distinguished_from_transient_failure() {
        // Only unrecoverable responses may suppress future refresh attempts.
        // Misclassifying a transient error would strand a still-valid token.
        for permanent in [
            r#"{"error": "invalid_grant", "error_description": "Refresh token not found or invalid"}"#,
            "code: refresh_token_invalidated",
            "Invalid refresh token",
            "token has been revoked",
        ] {
            assert!(
                error_is_permanent_rejection(permanent),
                "{permanent} should be permanent"
            );
        }

        for transient in [
            "error sending request for url (https://claude.com): connection reset",
            "503 Service Unavailable",
            "operation timed out",
        ] {
            assert!(
                !error_is_permanent_rejection(transient),
                "{transient} must stay retryable"
            );
        }
    }

    #[test]
    fn refresh_token_fingerprint_never_leaks_the_token() {
        let token = "sk-ant-ort01-super-secret-value";
        let fingerprint = refresh_token_fingerprint(token);
        assert!(!fingerprint.contains("secret"));
        assert!(!token.contains(&fingerprint));
        // Stable across calls, and distinct per token, so a re-login mints a
        // different fingerprint and refresh resumes with nothing to clear.
        assert_eq!(fingerprint, refresh_token_fingerprint(token));
        assert_ne!(fingerprint, refresh_token_fingerprint("sk-ant-ort01-other"));
        // An empty token is never "known rejected"; there is nothing to skip.
        assert!(!refresh_token_is_known_rejected("claude", ""));
    }

    #[test]
    fn permanent_rejection_suppresses_retries_until_a_new_token_is_minted() {
        let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new()
            .expect("auth test sandbox should initialize");

        let dead = "sk-ant-ort01-dead";
        let fresh = "sk-ant-ort01-fresh";

        // Nothing recorded yet: refresh must still be attempted.
        assert!(!refresh_token_is_known_rejected("claude", dead));

        // A transient failure must NOT suppress retries; the token may recover.
        record_failure("claude", "connection reset").expect("record transient failure");
        assert!(!refresh_token_is_known_rejected("claude", dead));

        // A permanent rejection suppresses further attempts for that token only.
        record_permanent_rejection("claude", dead, "invalid_grant")
            .expect("record permanent rejection");
        assert!(refresh_token_is_known_rejected("claude", dead));
        assert!(
            !refresh_token_is_known_rejected("claude", fresh),
            "a different token must still be attempted"
        );
        assert!(
            !refresh_token_is_known_rejected("openai", dead),
            "suppression must not leak across providers"
        );

        // The token itself is never persisted, only its fingerprint.
        let raw = std::fs::read_to_string(status_path().expect("status path"))
            .expect("refresh state should be written");
        assert!(!raw.contains(dead), "refresh token must never be persisted");

        // Re-login records a success, clearing suppression entirely.
        record_success("claude").expect("record success");
        assert!(!refresh_token_is_known_rejected("claude", dead));
    }

    #[test]
    fn cred_state_tracks_the_credential_lifecycle() {
        let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new()
            .expect("auth test sandbox should initialize");

        let token = "openai-refresh-token";

        // Nothing recorded at all.
        assert_eq!(cred_state("openai", Some(token)), CredState::Absent);

        // A success makes it verified and usable.
        record_success("openai").expect("record success");
        assert_eq!(cred_state("openai", Some(token)), CredState::Verified);
        assert!(cred_state("openai", Some(token)).is_retryable());

        // A transient failure is stale: still worth retrying.
        record_failure("openai", "connection reset").expect("record failure");
        assert_eq!(cred_state("openai", Some(token)), CredState::Stale);
        assert!(cred_state("openai", Some(token)).is_retryable());

        // A permanent rejection is terminal for this token only.
        record_permanent_rejection("openai", token, "refresh_token_invalidated")
            .expect("record rejection");
        assert_eq!(cred_state("openai", Some(token)), CredState::Rejected);
        assert!(!cred_state("openai", Some(token)).is_retryable());
        assert!(!cred_state("openai", Some(token)).is_usable());
        assert_eq!(
            cred_state("openai", Some("freshly-minted")),
            CredState::Stale
        );
    }

    #[test]
    fn absent_and_rejected_never_share_a_user_facing_label() {
        // The live bug this prevents: the onboarding banner said
        // "GitHub Copilot - login expired" while auth status reported
        // `copilot=not_configured`. One label function, one answer.
        assert_ne!(
            CredState::Absent.user_facing_label(),
            CredState::Rejected.user_facing_label()
        );
        assert!(
            CredState::Absent
                .user_facing_label()
                .contains("not configured")
        );
        assert!(CredState::Rejected.user_facing_label().contains("expired"));
    }

    #[test]
    fn ensure_refresh_allowed_blocks_only_the_rejected_token() {
        let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new()
            .expect("auth test sandbox should initialize");

        let dead = "dead-token";
        assert!(ensure_refresh_allowed("openai", dead, "hint").is_ok());

        record_permanent_rejection("openai", dead, "your session has ended")
            .expect("record rejection");
        let err = ensure_refresh_allowed("openai", dead, "Run `jcode login`.")
            .expect_err("a rejected token must not be refreshed again");
        assert!(err.to_string().contains("Run `jcode login`."));
        assert!(ensure_refresh_allowed("openai", "new-token", "hint").is_ok());
    }

    #[test]
    fn record_refresh_outcome_routes_permanent_and_transient_failures() {
        let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new()
            .expect("auth test sandbox should initialize");

        let token = "gemini-token";

        let transient: Result<()> = Err(anyhow::anyhow!("503 Service Unavailable"));
        record_refresh_outcome("gemini", token, &transient);
        assert_eq!(cred_state("gemini", Some(token)), CredState::Stale);

        let permanent: Result<()> = Err(anyhow::anyhow!("invalid_grant"));
        record_refresh_outcome("gemini", token, &permanent);
        assert_eq!(cred_state("gemini", Some(token)), CredState::Rejected);

        // Success after a re-login clears the terminal state.
        let ok: Result<()> = Ok(());
        record_refresh_outcome("gemini", "new-token", &ok);
        assert_eq!(cred_state("gemini", Some("new-token")), CredState::Verified);
    }

    #[test]
    fn every_provider_permanent_phrasing_is_classified() {
        // Each of these is the literal wording one provider returns for a dead
        // refresh token. Missing one reintroduces the infinite-retry bug for
        // that provider specifically, which is exactly how OpenAI regressed.
        for (provider, message) in [
            (
                "openai",
                "OpenAI token refresh failed: {\"error\":{\"message\":\"Your session has ended. Please log in again.\",\"code\":\"refresh_token_invalidated\"}}",
            ),
            (
                "claude",
                "{\"error\": \"invalid_grant\", \"error_description\": \"Refresh token not found or invalid\"}",
            ),
            (
                "cursor",
                "Cursor refresh token was rejected; Cursor requested logout/login. Re-run Cursor login, then retry auth-test.",
            ),
            (
                "google",
                "invalid_grant: Token has been expired or revoked.",
            ),
        ] {
            assert!(
                error_is_permanent_rejection(message),
                "{provider} permanent failure must be terminal, got retryable: {message}"
            );
        }
    }
}
