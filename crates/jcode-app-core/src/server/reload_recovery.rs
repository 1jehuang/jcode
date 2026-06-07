use crate::tool::selfdev::ReloadRecoveryDirective;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ReloadRecoveryRole {
    Initiator,
    InterruptedPeer,
    Headless,
}

impl ReloadRecoveryRole {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Initiator => "initiator",
            Self::InterruptedPeer => "interrupted_peer",
            Self::Headless => "headless",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ReloadRecoveryStatus {
    Pending,
    Delivered,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct ReloadRecoveryRecord {
    pub reload_id: String,
    pub session_id: String,
    pub role: ReloadRecoveryRole,
    pub status: ReloadRecoveryStatus,
    pub directive: ReloadRecoveryDirective,
    pub reason: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered_at: Option<String>,
}

fn sanitize_session_id(session_id: &str) -> String {
    session_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn recovery_dir() -> Result<PathBuf> {
    Ok(crate::storage::jcode_dir()?.join("reload-recovery"))
}

pub(super) fn path_for_session(session_id: &str) -> Result<PathBuf> {
    Ok(recovery_dir()?.join(format!("{}.json", sanitize_session_id(session_id))))
}

pub(super) fn persist_intent(
    reload_id: &str,
    session_id: &str,
    role: ReloadRecoveryRole,
    directive: ReloadRecoveryDirective,
    reason: impl Into<String>,
) -> Result<()> {
    let role_label = role.as_str();
    let record = ReloadRecoveryRecord {
        reload_id: reload_id.to_string(),
        session_id: session_id.to_string(),
        role,
        status: ReloadRecoveryStatus::Pending,
        directive,
        reason: reason.into(),
        created_at: chrono::Utc::now().to_rfc3339(),
        delivered_at: None,
    };
    let path = path_for_session(session_id)?;
    crate::storage::write_json(&path, &record)?;
    crate::logging::info(&format!(
        "reload recovery store: persisted intent reload_id={} session={} role={} path={}",
        reload_id,
        session_id,
        role_label,
        path.display()
    ));
    Ok(())
}

pub(super) fn peek_for_session(session_id: &str) -> Result<Option<ReloadRecoveryRecord>> {
    let path = path_for_session(session_id)?;
    if !path.exists() {
        return Ok(None);
    }
    crate::storage::read_json(&path).map(Some)
}

#[cfg(test)]
pub(super) fn has_pending_for_session(session_id: &str) -> bool {
    peek_for_session(session_id)
        .ok()
        .flatten()
        .map(|record| record.status == ReloadRecoveryStatus::Pending)
        .unwrap_or(false)
}

/// Return the pending recovery directive for inclusion in a bootstrap/history
/// payload without consuming it.
///
/// A History frame can be lost if the client disconnects or re-execs after the
/// server writes the payload but before the TUI queues/sends the hidden
/// continuation. Therefore History generation must not mark the durable intent
/// delivered. Delivery is recorded only when the replacement server accepts the
/// matching continuation message.
pub(super) fn pending_directive_for_session(
    session_id: &str,
) -> Result<Option<ReloadRecoveryDirective>> {
    let path = path_for_session(session_id)?;
    if !path.exists() {
        return Ok(None);
    }

    let record: ReloadRecoveryRecord = crate::storage::read_json(&path)?;
    if record.status != ReloadRecoveryStatus::Pending {
        super::reload_trace::record_value(
            &record.reload_id,
            "intent_peek_skipped",
            serde_json::json!({
                "session_id": session_id,
                "status": format!("{:?}", record.status),
            }),
        );
        crate::logging::info(&format!(
            "reload recovery store: skipping non-pending intent session={} reload_id={} status={:?}",
            session_id, record.reload_id, record.status
        ));
        return Ok(None);
    }

    let directive = record.directive.clone();
    super::reload_trace::record_value(
        &record.reload_id,
        "intent_attached_to_history",
        serde_json::json!({
            "session_id": session_id,
            "role": record.role.as_str(),
            "path": path,
        }),
    );
    crate::logging::info(&format!(
        "reload recovery store: attached pending intent reload_id={} session={} role={} without marking delivered",
        record.reload_id,
        session_id,
        record.role.as_str()
    ));
    Ok(Some(directive))
}

pub(super) fn mark_delivered_if_matching_continuation(
    session_id: &str,
    continuation_message: &str,
    accepted_by: &str,
) -> Result<bool> {
    let path = path_for_session(session_id)?;
    if !path.exists() {
        return Ok(false);
    }

    let mut record: ReloadRecoveryRecord = crate::storage::read_json(&path)?;
    if record.status != ReloadRecoveryStatus::Pending {
        super::reload_trace::record_value(
            &record.reload_id,
            "intent_delivery_skipped",
            serde_json::json!({
                "session_id": session_id,
                "status": format!("{:?}", record.status),
                "accepted_by": accepted_by,
            }),
        );
        return Ok(false);
    }

    if record.directive.continuation_message != continuation_message {
        super::reload_trace::record_value(
            &record.reload_id,
            "intent_delivery_mismatch",
            serde_json::json!({
                "session_id": session_id,
                "accepted_by": accepted_by,
                "expected_chars": record.directive.continuation_message.len(),
                "received_chars": continuation_message.len(),
            }),
        );
        crate::logging::warn(&format!(
            "reload recovery store: continuation mismatch session={} reload_id={} accepted_by={} expected_chars={} received_chars={}",
            session_id,
            record.reload_id,
            accepted_by,
            record.directive.continuation_message.len(),
            continuation_message.len()
        ));
        return Ok(false);
    }

    record.status = ReloadRecoveryStatus::Delivered;
    record.delivered_at = Some(chrono::Utc::now().to_rfc3339());
    crate::storage::write_json(&path, &record)?;
    super::reload_trace::record_value(
        &record.reload_id,
        "intent_delivered",
        serde_json::json!({
            "session_id": session_id,
            "role": record.role.as_str(),
            "accepted_by": accepted_by,
            "path": path,
        }),
    );
    crate::logging::info(&format!(
        "reload recovery store: delivered intent reload_id={} session={} role={} accepted_by={}",
        record.reload_id,
        session_id,
        record.role.as_str(),
        accepted_by
    ));
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct IsolatedHome {
        prev_home: Option<std::ffi::OsString>,
        _temp: tempfile::TempDir,
    }

    impl IsolatedHome {
        fn new() -> Self {
            let temp = tempfile::TempDir::new().expect("jcode home");
            let prev_home = std::env::var_os("JCODE_HOME");
            crate::env::set_var("JCODE_HOME", temp.path());
            Self {
                prev_home,
                _temp: temp,
            }
        }
    }

    impl Drop for IsolatedHome {
        fn drop(&mut self) {
            if let Some(prev) = self.prev_home.take() {
                crate::env::set_var("JCODE_HOME", prev);
            } else {
                crate::env::remove_var("JCODE_HOME");
            }
        }
    }

    fn directive(message: &str) -> ReloadRecoveryDirective {
        ReloadRecoveryDirective {
            reconnect_notice: Some("reconnected".to_string()),
            continuation_message: message.to_string(),
        }
    }

    #[test]
    fn sanitize_session_id_strips_path_traversal_and_separators() {
        // A malicious or merely unusual session id must never be able to escape
        // the recovery directory or collide with sibling paths.
        assert_eq!(sanitize_session_id("../../etc/passwd"), "______etc_passwd");
        assert_eq!(sanitize_session_id("a/b\\c"), "a_b_c");
        assert_eq!(sanitize_session_id("sess.with space"), "sess_with_space");
        // Already-safe ids are preserved verbatim.
        assert_eq!(sanitize_session_id("session-abc_123"), "session-abc_123");
    }

    #[test]
    fn path_for_session_stays_inside_recovery_dir() -> Result<()> {
        let _lock = crate::storage::lock_test_env();
        let _home = IsolatedHome::new();

        let dir = recovery_dir()?;
        let evil = path_for_session("../../escape")?;
        assert!(
            evil.starts_with(&dir),
            "traversal session id escaped recovery dir: {} not under {}",
            evil.display(),
            dir.display()
        );
        assert_eq!(
            evil.file_name().and_then(|n| n.to_str()),
            Some("______escape.json")
        );
        Ok(())
    }

    #[test]
    fn persist_then_peek_roundtrips_record() -> Result<()> {
        let _lock = crate::storage::lock_test_env();
        let _home = IsolatedHome::new();

        let session_id = "session-roundtrip";
        persist_intent(
            "reload-roundtrip",
            session_id,
            ReloadRecoveryRole::Headless,
            directive("resume the headless task"),
            "headless test",
        )?;

        let record = peek_for_session(session_id)?.expect("record should exist");
        assert_eq!(record.reload_id, "reload-roundtrip");
        assert_eq!(record.session_id, session_id);
        assert_eq!(record.role, ReloadRecoveryRole::Headless);
        assert_eq!(record.status, ReloadRecoveryStatus::Pending);
        assert_eq!(
            record.directive.continuation_message,
            "resume the headless task"
        );
        assert!(record.delivered_at.is_none());
        assert!(has_pending_for_session(session_id));
        Ok(())
    }

    #[test]
    fn peek_for_missing_session_is_none() -> Result<()> {
        let _lock = crate::storage::lock_test_env();
        let _home = IsolatedHome::new();
        assert!(peek_for_session("never-persisted")?.is_none());
        assert!(!has_pending_for_session("never-persisted"));
        assert!(pending_directive_for_session("never-persisted")?.is_none());
        Ok(())
    }

    #[test]
    fn pending_directive_does_not_consume_intent() -> Result<()> {
        let _lock = crate::storage::lock_test_env();
        let _home = IsolatedHome::new();

        let session_id = "session-non-consuming";
        persist_intent(
            "reload-non-consuming",
            session_id,
            ReloadRecoveryRole::InterruptedPeer,
            directive("continue please"),
            "peek test",
        )?;

        // Reading the directive (for History payloads) must leave the durable
        // intent pending so a lost frame can be retried after reconnect.
        for _ in 0..3 {
            let directive = pending_directive_for_session(session_id)?.expect("directive present");
            assert_eq!(directive.continuation_message, "continue please");
            assert!(has_pending_for_session(session_id));
        }
        Ok(())
    }

    #[test]
    fn mark_delivered_is_idempotent_and_matches_exact_continuation() -> Result<()> {
        let _lock = crate::storage::lock_test_env();
        let _home = IsolatedHome::new();

        let session_id = "session-deliver";
        let continuation = "exact continuation body";
        persist_intent(
            "reload-deliver",
            session_id,
            ReloadRecoveryRole::Initiator,
            directive(continuation),
            "delivery test",
        )?;

        // A non-matching continuation must not consume the intent.
        assert!(!mark_delivered_if_matching_continuation(
            session_id,
            "some other message",
            "server-a",
        )?);
        assert!(
            has_pending_for_session(session_id),
            "mismatched continuation must leave intent pending"
        );

        // The exact continuation consumes it exactly once.
        assert!(mark_delivered_if_matching_continuation(
            session_id,
            continuation,
            "server-a",
        )?);
        assert!(!has_pending_for_session(session_id));

        // Re-delivery is a no-op (idempotent) even with the right body.
        assert!(!mark_delivered_if_matching_continuation(
            session_id,
            continuation,
            "server-b",
        )?);

        // And the persisted record records when it was delivered.
        let record = peek_for_session(session_id)?.expect("record should still exist");
        assert_eq!(record.status, ReloadRecoveryStatus::Delivered);
        assert!(record.delivered_at.is_some());
        Ok(())
    }

    #[test]
    fn mark_delivered_for_missing_session_is_false() -> Result<()> {
        let _lock = crate::storage::lock_test_env();
        let _home = IsolatedHome::new();
        assert!(!mark_delivered_if_matching_continuation(
            "missing-session",
            "anything",
            "server",
        )?);
        Ok(())
    }

    #[test]
    fn persist_intent_overwrites_prior_record_for_same_session() -> Result<()> {
        let _lock = crate::storage::lock_test_env();
        let _home = IsolatedHome::new();

        let session_id = "session-overwrite";
        persist_intent(
            "reload-old",
            session_id,
            ReloadRecoveryRole::InterruptedPeer,
            directive("old continuation"),
            "first",
        )?;
        persist_intent(
            "reload-new",
            session_id,
            ReloadRecoveryRole::Headless,
            directive("new continuation"),
            "second",
        )?;

        let record = peek_for_session(session_id)?.expect("record should exist");
        assert_eq!(record.reload_id, "reload-new");
        assert_eq!(record.role, ReloadRecoveryRole::Headless);
        assert_eq!(record.directive.continuation_message, "new continuation");
        assert_eq!(record.status, ReloadRecoveryStatus::Pending);
        Ok(())
    }
}
