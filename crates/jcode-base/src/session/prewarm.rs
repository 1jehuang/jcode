//! Prewarmed session loads for the reconnect storm after a server reload.
//!
//! An exec-based reload keeps the daemon PID, so every session the previous
//! process image owned still has an `active_pids` marker naming this PID. Those
//! clients reconnect within a second and each restore fully parses its
//! snapshot on the critical path (~0.2s for a 35MB session under load). At
//! startup we parse them in the background, largest first, and `take` hands the
//! parsed copy to the resume path.
//!
//! Correctness: an entry is only used if the snapshot and journal files have
//! the same length and mtime as when they were parsed, so any write in between
//! falls back to a fresh load. Entries are taken once and expire, so memory is
//! bounded and a stale copy is never served later.

use super::Session;
use super::storage_paths::{session_journal_path_from_snapshot, session_path};
use std::collections::HashMap;
use std::sync::{Condvar, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// Below this, a direct parse is a few ms. Above it, the parse is the bulk
/// of resume (live: 6.4MB sessions took 98-116ms of a ~165ms resume). A
/// resume that arrives before its turn claims the queued entry and parses
/// itself, so queueing more sessions never makes one wait behind others.
const MIN_PREWARM_BYTES: u64 = 2 * 1024 * 1024;
const MAX_PREWARM_SESSIONS: usize = 64;
const PREWARM_WORKERS: usize = 2;
/// Reconnects arrive within a few seconds of startup.
const ENTRY_TTL: Duration = Duration::from_secs(30);
/// Never block a resume for long on an in-flight prewarm.
const MAX_WAIT_FOR_IN_FLIGHT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
}

fn stamp(path: &std::path::Path) -> Option<FileStamp> {
    match std::fs::metadata(path) {
        Ok(meta) => Some(FileStamp {
            len: meta.len(),
            modified: meta.modified().ok(),
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => Some(FileStamp {
            len: u64::MAX,
            modified: None,
        }),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Fingerprint {
    snapshot: Option<FileStamp>,
    journal: Option<FileStamp>,
}

fn fingerprint(session_id: &str) -> Option<Fingerprint> {
    let path = session_path(session_id).ok()?;
    let journal = session_journal_path_from_snapshot(&path);
    Some(Fingerprint {
        snapshot: stamp(&path),
        journal: stamp(&journal),
    })
}

enum Entry {
    /// Waiting for the prewarm thread. A resume that finds this claims the
    /// session and loads it itself instead of waiting behind larger ones.
    Queued,
    /// Being parsed right now. A resume waits briefly for it.
    Parsing,
    Ready {
        session: Box<Session>,
        fingerprint: Fingerprint,
        ready_at: Instant,
    },
}

struct Prewarm {
    entries: Mutex<HashMap<String, Entry>>,
    changed: Condvar,
}

static PREWARM: LazyLock<Prewarm> = LazyLock::new(|| Prewarm {
    entries: Mutex::new(HashMap::new()),
    changed: Condvar::new(),
});

fn lock_entries() -> std::sync::MutexGuard<'static, HashMap<String, Entry>> {
    PREWARM
        .entries
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Sessions whose `active_pids` marker names `pid`. Call before the startup
/// prune removes those markers.
pub fn sessions_owned_by(pid: u32) -> Vec<String> {
    let Some(dir) = crate::storage::active_pids_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            std::fs::read_to_string(entry.path())
                .ok()
                .and_then(|raw| raw.trim().parse::<u32>().ok())
                == Some(pid)
        })
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect()
}

/// Parse the given sessions in the background, largest first.
pub fn spawn_prewarm(session_ids: Vec<String>) {
    spawn_prewarm_with_min_bytes(session_ids, MIN_PREWARM_BYTES);
}

fn spawn_prewarm_with_min_bytes(session_ids: Vec<String>, min_bytes: u64) {
    let mut sized: Vec<(u64, String)> = session_ids
        .into_iter()
        .filter_map(|id| {
            let len = session_path(&id).ok().and_then(|p| std::fs::metadata(p).ok())?.len();
            (len >= min_bytes).then_some((len, id))
        })
        .collect();
    if sized.is_empty() {
        return;
    }
    sized.sort_by(|a, b| b.0.cmp(&a.0));
    sized.truncate(MAX_PREWARM_SESSIONS);
    {
        let mut entries = lock_entries();
        for (_, id) in &sized {
            entries.insert(id.clone(), Entry::Queued);
        }
    }
    // Two workers pull from one largest-first queue. Each parse is itself
    // multi-threaded (see `parallel_messages`), so more workers would only
    // compete with the reconnect handlers for CPU.
    let queue = std::sync::Arc::new(Mutex::new(sized.into_iter()));
    let start = Instant::now();
    let workers: Vec<_> = (0..PREWARM_WORKERS)
        .filter_map(|worker| {
            let queue = std::sync::Arc::clone(&queue);
            std::thread::Builder::new()
                .name(format!("jcode-session-prewarm-{worker}"))
                .spawn(move || {
                    let mut loaded = 0usize;
                    loop {
                        let next = queue
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .next();
                        let Some((_, id)) = next else { break };
                        prewarm_one(&id);
                        loaded += 1;
                    }
                    loaded
                })
                .ok()
        })
        .collect();
    if workers.is_empty() {
        lock_entries().clear();
        return;
    }
    let _ = std::thread::Builder::new()
        .name("jcode-session-prewarm-expiry".to_string())
        .spawn(move || {
            let loaded: usize = workers.into_iter().filter_map(|w| w.join().ok()).sum();
            crate::logging::info(&format!(
                "[TIMING] session_prewarm: sessions={loaded}, total={}ms",
                start.elapsed().as_millis()
            ));
            std::thread::sleep(ENTRY_TTL);
            expire_stale();
        });
}

fn prewarm_one(session_id: &str) {
    {
        let mut entries = lock_entries();
        match entries.get(session_id) {
            Some(Entry::Queued) => {
                entries.insert(session_id.to_string(), Entry::Parsing);
            }
            // Claimed by a resume that loaded it directly.
            _ => return,
        }
    }
    let before = fingerprint(session_id);
    let result = session_path(session_id).and_then(|path| Session::load_from_path(&path));
    let after = fingerprint(session_id);
    let mut entries = lock_entries();
    // A write during the parse would make this copy ambiguous. Drop it.
    match (result, before, after) {
        (Ok(session), Some(before), Some(after)) if before == after => {
            entries.insert(
                session_id.to_string(),
                Entry::Ready {
                    session: Box::new(session),
                    fingerprint: after,
                    ready_at: Instant::now(),
                },
            );
        }
        _ => {
            entries.remove(session_id);
        }
    }
    drop(entries);
    PREWARM.changed.notify_all();
}

fn expire_stale() {
    let mut entries = lock_entries();
    entries.retain(|_, entry| match entry {
        Entry::Queued | Entry::Parsing => true,
        Entry::Ready { ready_at, .. } => ready_at.elapsed() < ENTRY_TTL,
    });
}

/// Take the prewarmed copy of `session_id` if it is still identical to disk.
/// Waits briefly when that session is being parsed right now, since a fresh
/// parse would only race it for the same CPU. A still-queued session is
/// claimed and left to the caller's own load.
pub(super) fn take(session_id: &str) -> Option<Session> {
    let deadline = Instant::now() + MAX_WAIT_FOR_IN_FLIGHT;
    let mut entries = lock_entries();
    loop {
        match entries.get(session_id) {
            None => return None,
            Some(Entry::Queued) => {
                entries.remove(session_id);
                return None;
            }
            Some(Entry::Parsing) => {
                let now = Instant::now();
                if now >= deadline {
                    return None;
                }
                entries = PREWARM
                    .changed
                    .wait_timeout(entries, deadline - now)
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .0;
            }
            Some(Entry::Ready { .. }) => break,
        }
    }
    let Some(Entry::Ready {
        session,
        fingerprint: parsed,
        ready_at,
    }) = entries.remove(session_id)
    else {
        return None;
    };
    drop(entries);
    if ready_at.elapsed() >= ENTRY_TTL || fingerprint(session_id) != Some(parsed) {
        return None;
    }
    Some(*session)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{Session, StoredMessage};
    use jcode_message_types::{ContentBlock, Role};

    struct Home {
        _dir: tempfile::TempDir,
        prev: Option<std::ffi::OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Home {
        fn new() -> Self {
            let lock = crate::storage::lock_test_env();
            let dir = tempfile::tempdir().unwrap();
            let prev = std::env::var_os("JCODE_HOME");
            crate::env::set_var("JCODE_HOME", dir.path());
            lock_entries().clear();
            Self { _dir: dir, prev, _lock: lock }
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            lock_entries().clear();
            match &self.prev {
                Some(prev) => crate::env::set_var("JCODE_HOME", prev),
                None => crate::env::remove_var("JCODE_HOME"),
            }
        }
    }

    fn saved_session(id: &str, text: &str) -> Session {
        let mut session = Session::create_with_id(id.to_string(), None, None);
        session.append_stored_message(StoredMessage {
            id: format!("{id}_m1"),
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        });
        session.save().unwrap();
        session
    }

    fn wait_ready(id: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if matches!(lock_entries().get(id), Some(Entry::Ready { .. }) | None) {
                return;
            }
            assert!(Instant::now() < deadline, "prewarm did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn first_text(session: &Session) -> String {
        match &session.messages[0].content[0] {
            ContentBlock::Text { text, .. } => text.clone(),
            other => panic!("unexpected block {other:?}"),
        }
    }

    #[test]
    fn resume_uses_prewarmed_copy_once() {
        let _home = Home::new();
        saved_session("session_prewarm_hit", "hello");
        spawn_prewarm_with_min_bytes(vec!["session_prewarm_hit".into()], 0);
        wait_ready("session_prewarm_hit");
        assert!(matches!(
            lock_entries().get("session_prewarm_hit"),
            Some(Entry::Ready { .. })
        ));
        let loaded = Session::load_for_resume("session_prewarm_hit").unwrap();
        assert_eq!(first_text(&loaded), "hello");
        assert!(take("session_prewarm_hit").is_none(), "taken exactly once");
    }

    #[test]
    fn a_write_after_prewarm_invalidates_the_copy() {
        let _home = Home::new();
        let mut session = saved_session("session_prewarm_stale", "old");
        spawn_prewarm_with_min_bytes(vec!["session_prewarm_stale".into()], 0);
        wait_ready("session_prewarm_stale");
        // Ensure a distinct mtime even on coarse-timestamp filesystems.
        std::thread::sleep(Duration::from_millis(20));
        session.append_stored_message(StoredMessage {
            id: "session_prewarm_stale_m2".to_string(),
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "newer".to_string(),
                cache_control: None,
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        });
        session.save().unwrap();
        assert!(take("session_prewarm_stale").is_none());
        let loaded = Session::load_for_resume("session_prewarm_stale").unwrap();
        assert_eq!(loaded.messages.len(), 2, "fresh load sees the new message");
    }

    #[test]
    fn several_sessions_are_all_prewarmed_by_the_worker_pool() {
        let _home = Home::new();
        let ids: Vec<String> = (0..5).map(|i| format!("session_prewarm_pool_{i}")).collect();
        for id in &ids {
            saved_session(id, id);
        }
        spawn_prewarm_with_min_bytes(ids.clone(), 0);
        for id in &ids {
            wait_ready(id);
        }
        for id in &ids {
            let loaded = Session::load_for_resume(id).unwrap();
            assert_eq!(&first_text(&loaded), id);
        }
        assert!(lock_entries().is_empty(), "every entry taken exactly once");
    }

    #[test]
    fn a_queued_session_is_claimed_not_waited_on() {
        let _home = Home::new();
        saved_session("session_prewarm_queued", "queued");
        lock_entries().insert("session_prewarm_queued".into(), Entry::Queued);
        let start = Instant::now();
        assert!(take("session_prewarm_queued").is_none());
        assert!(start.elapsed() < Duration::from_millis(100), "must not wait");
        assert!(lock_entries().get("session_prewarm_queued").is_none());
        // The prewarm thread then skips it instead of parsing a claimed session.
        prewarm_one("session_prewarm_queued");
        assert!(lock_entries().get("session_prewarm_queued").is_none());
    }

    #[test]
    fn small_and_unknown_sessions_are_not_prewarmed() {
        let _home = Home::new();
        saved_session("session_prewarm_small", "tiny");
        spawn_prewarm(vec![
            "session_prewarm_small".into(),
            "session_does_not_exist".into(),
        ]);
        assert!(lock_entries().is_empty());
        let loaded = Session::load_for_resume("session_prewarm_small").unwrap();
        assert_eq!(first_text(&loaded), "tiny");
    }

    #[test]
    fn sessions_owned_by_reads_only_matching_markers() {
        let _home = Home::new();
        crate::storage::register_active_pid("session_mine", 4242);
        crate::storage::register_active_pid("session_theirs", 4343);
        assert_eq!(sessions_owned_by(4242), vec!["session_mine".to_string()]);
    }
}
