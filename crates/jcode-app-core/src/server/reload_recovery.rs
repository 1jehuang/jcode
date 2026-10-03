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
    Superseded,
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
                    || (record.status == ReloadRecoveryStatus::Pending
                        && pending_record_is_expired(&record, now)
                            .unwrap_or_else(|| file_is_expired(&path, now)))
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

/// A fresh prompt supersedes the interrupted turn. Remove both recovery
/// sources before releasing the pause, so History cannot replay old work.
pub(super) fn resume_passive_session_with_new_prompt(session_id: &str) -> Result<()> {
    // Keep a tombstone until a later reload writes fresh intent. Removing the
    // record alone would allow History to infer the old interrupted turn from
    // the transcript before the new prompt has been persisted.
    let record = ReloadRecoveryRecord {
        reload_id: String::new(),
        session_id: session_id.to_string(),
        role: ReloadRecoveryRole::InterruptedPeer,
        status: ReloadRecoveryStatus::Superseded,
        directive: ReloadRecoveryDirective {
            reconnect_notice: None,
            continuation_message: String::new(),
        },
        reason: "superseded by a fresh prompt after passive restore".to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        delivered_at: None,
    };
    let path = path_for_session(session_id)?;
    crate::storage::write_json(&path, &record)?;
    // Do not leave an older pending record available to JSON backup recovery.
    match std::fs::remove_file(path.with_extension("bak")) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    crate::tool::selfdev::ReloadContext::discard_for_session(session_id)?;
    crate::restart_snapshot::clear_passive_restore(session_id)
}

pub(super) fn recovery_was_superseded(session_id: &str) -> bool {
    let record = match peek_for_session(session_id) {
        Ok(Some(record)) if record.status == ReloadRecoveryStatus::Superseded => record,
        Ok(_) => return false,
        Err(_) => return true,
    };
    // Suppress the old transcript, not interruption recovery for every future
    // turn in this session. A newly persisted message belongs to later work.
    let Ok(cutoff) = chrono::DateTime::parse_from_rfc3339(&record.created_at) else {
        return true;
    };
    let newer_message = crate::session::Session::load_for_remote_startup(session_id)
        .ok()
        .and_then(|session| {
            session
                .messages
                .last()
                .and_then(|message| message.timestamp)
        })
        .is_some_and(|timestamp| timestamp > cutoff);
    !newer_message
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
    if let Err(error) = remove_record_files(&path) {
        // Delivery is already durable. Leave the consumed record for the next
        // startup sweep rather than reporting the accepted continuation as a
        // failure and risking duplicate recovery work.
        crate::logging::warn(&format!(
            "reload recovery store: could not remove delivered intent session={} path={}: {}",
            session_id,
            path.display(),
            error
        ));
    }
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
    fn passive_resume_removes_matching_legacy_context_and_backup() -> Result<()> {
        let _lock = crate::storage::lock_test_env();
        let _home = IsolatedHome::new();
        let id = "session_matching_legacy";
        let legacy = crate::storage::jcode_dir()?.join("reload-context.json");
        let context = crate::tool::selfdev::ReloadContext {
            session_id: id.into(),
            task_context: Some("old task".into()),
            version_before: "old".into(),
            version_after: "new".into(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        crate::storage::write_json(&legacy, &context)?;
        crate::storage::write_json(&legacy, &context)?;
        crate::restart_snapshot::mark_passive_restore(id)?;
        resume_passive_session_with_new_prompt(id)?;
        assert!(!legacy.exists());
        assert!(!legacy.with_extension("bak").exists());
        assert!(!crate::restart_snapshot::is_passive_restore(id)?);
        Ok(())
    }

    #[test]
    fn passive_resume_ignores_corrupt_shared_context_and_preserves_foreign_context() -> Result<()> {
        let _lock = crate::storage::lock_test_env();
        let _home = IsolatedHome::new();
        let legacy = crate::storage::jcode_dir()?.join("reload-context.json");
        for corrupt in [false, true] {
            if corrupt {
                std::fs::write(&legacy, "not json")?;
                std::fs::write(legacy.with_extension("bak"), "also not json")?;
            } else {
                crate::storage::write_json(
                    &legacy,
                    &crate::tool::selfdev::ReloadContext {
                        session_id: "another-session".into(),
                        task_context: Some("foreign task".into()),
                        version_before: "old".into(),
                        version_after: "new".into(),
                        timestamp: chrono::Utc::now().to_rfc3339(),
                    },
                )?;
            }
            let original = std::fs::read(&legacy)?;
            for _ in 0..2 {
                let id = "session_resume_with_foreign_context";
                crate::restart_snapshot::mark_passive_restore(id)?;
                persist_intent(
                    "old",
                    id,
                    ReloadRecoveryRole::InterruptedPeer,
                    directive("old work"),
                    "test",
                )?;
                resume_passive_session_with_new_prompt(id)?;
                assert!(!crate::restart_snapshot::is_passive_restore(id)?);
                assert!(!has_pending_for_session(id));
                assert!(recovery_was_superseded(id));
                assert_eq!(std::fs::read(&legacy)?, original);
            }
        }
        Ok(())
    }

    #[test]
    fn passive_restore_keeps_pause_if_old_context_cannot_be_removed() -> Result<()> {
        let _lock = crate::storage::lock_test_env();
        let _home = IsolatedHome::new();
        let id = "session_cleanup_failure";
        crate::restart_snapshot::mark_passive_restore(id)?;
        let path = crate::tool::selfdev::ReloadContext::path_for_session(id)?;
        std::fs::create_dir(&path)?;
        assert!(resume_passive_session_with_new_prompt(id).is_err());
        assert!(crate::restart_snapshot::is_passive_restore(id)?);
        std::fs::remove_dir(path)?;
        resume_passive_session_with_new_prompt(id)?;
        assert!(!crate::restart_snapshot::is_passive_restore(id)?);
        Ok(())
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

        // Consumed intents are removed immediately; startup GC covers a crash
        // between the delivered write and this deletion.
        assert!(peek_for_session(session_id)?.is_none());
        assert!(!path_for_session(session_id)?.with_extension("bak").exists());
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
