//! Session-addressed provider context measurements for read-only resume previews.
//! No credentials, cumulative token counts, or process-global account state.
use crate::session::Session;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SessionTelemetry {
    pub session_id: String,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub measured_at: chrono::DateTime<chrono::Utc>,
    /// Normalized prompt size for one provider request, excluding output.
    pub context_tokens: u64,
    pub context_limit: Option<usize>,
    pub compaction: Option<jcode_session_types::StoredCompactionState>,
    #[serde(default)]
    pub route_api_method: Option<String>,
    #[serde(default)]
    pub serving_provider: Option<String>,
    #[serde(default)]
    pub credential: Option<jcode_provider_core::ResolvedCredential>,
    #[serde(default)]
    pub account_label: Option<String>,
    #[serde(default)]
    pub account_usage: Option<jcode_protocol::AccountUsageSnapshot>,
}

fn path(session_id: &str) -> Option<std::path::PathBuf> {
    if session_id.is_empty()
        || !session_id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return None;
    }
    Some(
        crate::storage::durable_state_dir()
            .join("session-telemetry")
            .join(format!("{session_id}.json")),
    )
}

pub fn save_context(
    session: &Session,
    context_tokens: u64,
    context_limit: Option<usize>,
    serving_provider: &str,
    credential: Option<jcode_provider_core::ResolvedCredential>,
    account_label: Option<String>,
) {
    let Some(path) = path(&session.id) else {
        return;
    };
    let account_usage = if matches!(
        credential,
        Some(jcode_provider_core::ResolvedCredential::Oauth)
    ) {
        let provider = match serving_provider.to_ascii_lowercase().as_str() {
            "openai" => Some(crate::usage::MultiAccountProviderKind::OpenAI),
            "claude" | "anthropic" => Some(crate::usage::MultiAccountProviderKind::Anthropic),
            _ => None,
        };
        provider
            .zip(account_label.as_deref())
            .and_then(|(provider, label)| {
                crate::usage::cached_account_usage_snapshot(provider, label)
                    .filter(|usage| usage.label == label)
                    .map(|usage| jcode_protocol::AccountUsageSnapshot {
                        label: usage.label,
                        email: None,
                        exhausted: usage.exhausted,
                        primary_label: usage.primary_label,
                        five_hour_ratio: usage.five_hour_ratio,
                        secondary_label: usage.secondary_label,
                        seven_day_ratio: usage.seven_day_ratio,
                        resets_at: usage.resets_at,
                        error: usage.error,
                        banked_resets: None,
                        banked_expires_at: None,
                        banked_next_at: None,
                    })
            })
    } else {
        None
    };
    let snapshot = SessionTelemetry {
        session_id: session.id.clone(),
        model: session.model.clone(),
        provider: session.provider_key.clone(),
        measured_at: chrono::Utc::now(),
        context_tokens,
        context_limit,
        compaction: session.compaction.clone(),
        route_api_method: session.route_api_method.clone(),
        serving_provider: Some(serving_provider.to_ascii_lowercase()),
        credential,
        account_label,
        account_usage,
    };
    // Best-effort UI metadata must never fail a provider turn.
    if let Err(error) = crate::storage::write_json_fast(&path, &snapshot) {
        crate::logging::warn(&format!("Could not save session telemetry: {error:#}"));
    }
}

pub fn load_context(session: &Session) -> Option<SessionTelemetry> {
    let bytes = match std::fs::read(path(&session.id)?) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            crate::logging::warn(&format!("Could not read session telemetry: {error}"));
            return None;
        }
    };
    let snapshot: SessionTelemetry = match serde_json::from_slice(&bytes) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            crate::logging::warn(&format!("Could not decode session telemetry: {error}"));
            return None;
        }
    };
    snapshot.matches_session(session).then_some(snapshot)
}

impl SessionTelemetry {
    pub fn matches_session(&self, session: &Session) -> bool {
        self.session_id == session.id
            && self.model == session.model
            && self.provider == session.provider_key
            && self.route_api_method == session.route_api_method
            && self.compaction == session.compaction
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durable_measurement_roundtrips_and_rejects_changed_identity() {
        let _lock = crate::storage::lock_test_env();
        struct HomeGuard(Option<std::ffi::OsString>);
        impl Drop for HomeGuard {
            fn drop(&mut self) {
                if let Some(home) = &self.0 {
                    crate::env::set_var("JCODE_HOME", home);
                } else {
                    crate::env::remove_var("JCODE_HOME");
                }
            }
        }
        let _home = HomeGuard(std::env::var_os("JCODE_HOME"));
        let directory = tempfile::tempdir().unwrap();
        crate::env::set_var("JCODE_HOME", directory.path());
        let mut session = Session::create_with_id("telemetry-test".into(), None, None);
        session.model = Some("future-model".into());
        session.provider_key = Some("openai".into());
        session.route_api_method = Some("openai:account-a".into());
        save_context(
            &session,
            1234,
            Some(200_000),
            "OpenAI",
            Some(jcode_provider_core::ResolvedCredential::Oauth),
            Some("account-a".into()),
        );
        let saved = load_context(&session).unwrap();
        assert_eq!(saved.context_tokens, 1234);
        assert_eq!(saved.context_limit, Some(200_000));
        assert_eq!(saved.serving_provider.as_deref(), Some("openai"));
        assert_eq!(saved.account_label.as_deref(), Some("account-a"));
        assert!(
            saved.account_usage.is_none(),
            "missing quota cache is unknown"
        );
        session.route_api_method = Some("openai:account-b".into());
        assert!(load_context(&session).is_none());
        session.route_api_method = saved.route_api_method;
        std::fs::write(path(&session.id).unwrap(), b"{broken").unwrap();
        assert!(load_context(&session).is_none());
    }

    #[test]
    fn session_ids_cannot_escape_snapshot_directory() {
        for id in ["", "../other", "/tmp/other", "foo/bar", "foo\\bar"] {
            assert!(path(id).is_none());
        }
        assert!(path("session_fox-123").is_some());
    }
}
