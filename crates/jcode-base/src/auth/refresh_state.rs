use anyhow::Result;
use std::collections::BTreeMap;
use std::path::PathBuf;

const REFRESH_STATUS_FILE: &str = "auth-refresh-state.json";
const MAX_ERROR_CHARS: usize = 240;

pub use jcode_auth_types::ProviderRefreshRecord;

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
}
