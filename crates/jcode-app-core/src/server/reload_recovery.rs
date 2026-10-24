use crate::tool::selfdev::ReloadRecoveryDirective;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

const PENDING_RECORD_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

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

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct GarbageCollectionStats {
    pub removed: usize,
    pub retained: usize,
    pub errors: usize,
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

fn remove_record_files(path: &std::path::Path) -> Result<()> {
    for candidate in [path.to_path_buf(), path.with_extension("bak")] {
        match std::fs::remove_file(&candidate) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }

    #[cfg(unix)]
    if let Some(parent) = path.parent()
        && let Ok(directory) = std::fs::File::open(parent)
    {
        let _ = directory.sync_all();
    }
    Ok(())
}

fn pending_record_is_expired(record: &ReloadRecoveryRecord, now: SystemTime) -> Option<bool> {
    let created_at = chrono::DateTime::parse_from_rfc3339(&record.created_at).ok()?;
    let created_at = SystemTime::from(created_at.with_timezone(&chrono::Utc));
    Some(
        now.duration_since(created_at)
            .map(|age| age >= PENDING_RECORD_MAX_AGE)
            .unwrap_or(false),
    )
}

fn file_is_expired(path: &std::path::Path, now: SystemTime) -> bool {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| now.duration_since(modified).ok())
        .map(|age| age >= PENDING_RECORD_MAX_AGE)
        .unwrap_or(false)
}

/// Removes consumed records and pending/corrupt records too old to be useful.
///
/// This is run synchronously before the server starts accepting clients, so it
/// cannot race in-process recovery writes. A record path is unique per session,
/// which also bounds repeated reloads for active sessions between sweeps.
pub(super) fn collect_garbage() -> Result<GarbageCollectionStats> {
    collect_garbage_at(SystemTime::now())
}

fn collect_garbage_at(now: SystemTime) -> Result<GarbageCollectionStats> {
    let dir = recovery_dir()?;
    if !dir.exists() {
        return Ok(GarbageCollectionStats::default());
    }

    let mut stats = GarbageCollectionStats::default();
    for entry in std::fs::read_dir(&dir)? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                stats.errors += 1;
                continue;
            }
        };
        let path = entry.path();
        let extension = path.extension().and_then(|extension| extension.to_str());
        if extension == Some("bak") {
            let primary = path.with_extension("json");
            if !primary.exists() && file_is_expired(&path, now) {
                match std::fs::remove_file(&path) {
                    Ok(()) => stats.removed += 1,
                    Err(error) => {
                        stats.errors += 1;
                        crate::logging::warn(&format!(
                            "reload recovery store: failed to collect orphan backup {}: {}",
                            path.display(),
                            error
                        ));
                    }
                }
            }
            continue;
        }
        if extension != Some("json") {
            continue;
        }

        let should_remove = match crate::storage::read_json::<ReloadRecoveryRecord>(&path) {
            Ok(record) => {
                record.status == ReloadRecoveryStatus::Delivered
                    || pending_record_is_expired(&record, now)
                        .unwrap_or_else(|| file_is_expired(&path, now))
            }
            Err(_) => file_is_expired(&path, now),
        };
        if !should_remove {
            stats.retained += 1;
            continue;
        }

        match remove_record_files(&path) {
            Ok(()) => stats.removed += 1,
            Err(error) => {
                stats.errors += 1;
                crate::logging::warn(&format!(
                    "reload recovery store: failed to collect {}: {}",
                    path.display(),
                    error
                ));
            }
        }
    }
    Ok(stats)
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

/// Compare only hidden reminders, never ordinary prompt content.
pub(super) fn is_delivered_continuation(session_id: &str, reminder: &str) -> Result<bool> {
    Ok(peek_for_session(session_id)?.is_some_and(|record| {
        record.status == ReloadRecoveryStatus::Delivered
            && record.directive.continuation_message == reminder
    }))
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
    // Retain a replay tombstone for stale History frames. Startup GC removes
    // it, and persist_intent replaces it when this session has a new reload.
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

        // Keep a tombstone so stale History continuations can be rejected.
        assert_eq!(
            peek_for_session(session_id)?.unwrap().status,
            ReloadRecoveryStatus::Delivered
        );
        assert!(is_delivered_continuation(session_id, continuation)?);
        assert!(!is_delivered_continuation(session_id, "ordinary reminder")?);
        Ok(())
    }

    #[test]
    fn garbage_collection_removes_delivered_and_stale_records() -> Result<()> {
        let _lock = crate::storage::lock_test_env();
        let _home = IsolatedHome::new();
        let now = SystemTime::now();
        let old = chrono::Utc::now() - chrono::Duration::days(8);

        let records = [
            ReloadRecoveryRecord {
                reload_id: "reload-delivered".to_string(),
                session_id: "session-delivered".to_string(),
                role: ReloadRecoveryRole::Initiator,
                status: ReloadRecoveryStatus::Delivered,
                directive: directive("done"),
                reason: "delivered".to_string(),
                created_at: chrono::Utc::now().to_rfc3339(),
                delivered_at: Some(chrono::Utc::now().to_rfc3339()),
            },
            ReloadRecoveryRecord {
                reload_id: "reload-stale".to_string(),
                session_id: "session-stale".to_string(),
                role: ReloadRecoveryRole::InterruptedPeer,
                status: ReloadRecoveryStatus::Pending,
                directive: directive("too late"),
                reason: "stale".to_string(),
                created_at: old.to_rfc3339(),
                delivered_at: None,
            },
            ReloadRecoveryRecord {
                reload_id: "reload-fresh".to_string(),
                session_id: "session-fresh".to_string(),
                role: ReloadRecoveryRole::Headless,
                status: ReloadRecoveryStatus::Pending,
                directive: directive("continue"),
                reason: "fresh".to_string(),
                created_at: chrono::Utc::now().to_rfc3339(),
                delivered_at: None,
            },
        ];
        for record in &records {
            crate::storage::write_json(&path_for_session(&record.session_id)?, record)?;
        }
        // Ensure backup artifacts are collected with their primary record.
        std::fs::write(
            path_for_session("session-delivered")?.with_extension("bak"),
            b"old backup",
        )?;

        let stats = collect_garbage_at(now)?;
        assert_eq!(stats.removed, 2);
        assert_eq!(stats.retained, 1);
        assert_eq!(stats.errors, 0);
        assert!(peek_for_session("session-delivered")?.is_none());
        assert!(peek_for_session("session-stale")?.is_none());
        assert!(peek_for_session("session-fresh")?.is_some());
        assert!(
            !path_for_session("session-delivered")?
                .with_extension("bak")
                .exists()
        );
        Ok(())
    }

    #[test]
    fn garbage_collection_uses_file_age_for_malformed_and_orphaned_records() -> Result<()> {
        let _lock = crate::storage::lock_test_env();
        let _home = IsolatedHome::new();
        let malformed = ReloadRecoveryRecord {
            reload_id: "reload-malformed-time".to_string(),
            session_id: "session-malformed-time".to_string(),
            role: ReloadRecoveryRole::Headless,
            status: ReloadRecoveryStatus::Pending,
            directive: directive("continue"),
            reason: "invalid timestamp".to_string(),
            created_at: "not-an-rfc3339-timestamp".to_string(),
            delivered_at: None,
        };
        crate::storage::write_json(&path_for_session(&malformed.session_id)?, &malformed)?;

        let orphan_backup = path_for_session("orphan")?.with_extension("bak");
        std::fs::write(&orphan_backup, b"orphaned backup")?;
        let corrupt_record = path_for_session("corrupt")?;
        std::fs::write(&corrupt_record, b"not json")?;

        let future = SystemTime::now() + PENDING_RECORD_MAX_AGE + Duration::from_secs(1);
        let stats = collect_garbage_at(future)?;
        assert_eq!(stats.removed, 3);
        assert_eq!(stats.retained, 0);
        assert_eq!(stats.errors, 0);
        assert!(peek_for_session(&malformed.session_id)?.is_none());
        assert!(!orphan_backup.exists());
        assert!(!corrupt_record.exists());
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
