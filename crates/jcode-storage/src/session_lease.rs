//! Per-machine migration leases for sessions moved between machines
//! (`jcode cloud move` / `jcode cloud return`).
//!
//! A conversation is a single linear transcript, so exactly one machine may run
//! turns for it at a time. Each machine keeps `~/.jcode/session_leases/<id>.json`
//! recording the latest migration `epoch` it knows about and whether the session
//! currently lives here or on another host. A session copy carries the epoch of
//! the migration that delivered it (`Session::migration_epoch`). A copy may run
//! turns and persist only when the lease says the session lives here and the
//! copy is at least as new as the lease. That blocks two failure modes:
//!
//! - the old machine's in-memory agent continuing after the session moved, and
//! - a stale in-memory agent overwriting a freshly returned transcript.
//!
//! Sessions that never migrated have no lease file and are unaffected.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::jcode_dir;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLease {
    pub session_id: String,
    /// Monotonic migration counter shared by both machines.
    pub epoch: u64,
    /// `None` when the session lives on this machine, otherwise the host that
    /// currently owns it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub away_host: Option<String>,
    /// Absolute repository root the session works in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_root: Option<String>,
    /// Snapshot commit both machines last agreed on. Used as the merge base
    /// when work comes back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync_snapshot: Option<String>,
    /// Branch HEAD both machines last agreed on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sync_head: Option<String>,
    pub updated_at: String,
}

impl SessionLease {
    pub fn is_here(&self) -> bool {
        self.away_host.is_none()
    }
}

/// Why a session copy may not run a turn or persist on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionLeaseBlock {
    MovedAway { host: String, epoch: u64 },
    StaleCopy { copy_epoch: u64, lease_epoch: u64 },
}

impl std::fmt::Display for SessionLeaseBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MovedAway { host, .. } => write!(
                f,
                "this session moved to `{host}`. Attach there with `jcode cloud attach`, or bring it back with `jcode cloud return`"
            ),
            Self::StaleCopy {
                copy_epoch,
                lease_epoch,
            } => write!(
                f,
                "this in-memory copy of the session is stale (migration epoch {copy_epoch} < {lease_epoch}); reopen the session to load the current transcript"
            ),
        }
    }
}

pub fn session_leases_dir() -> Option<PathBuf> {
    jcode_dir().ok().map(|dir| dir.join("session_leases"))
}

fn lease_path(session_id: &str) -> Option<PathBuf> {
    if session_id.is_empty()
        || !session_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
        || session_id.starts_with('.')
    {
        return None;
    }
    session_leases_dir().map(|dir| dir.join(format!("{session_id}.json")))
}

pub fn read_session_lease(session_id: &str) -> Option<SessionLease> {
    let path = lease_path(session_id)?;
    let raw = std::fs::read(path).ok()?;
    serde_json::from_slice(&raw).ok()
}

pub fn write_session_lease(lease: &SessionLease) -> anyhow::Result<()> {
    let path = lease_path(&lease.session_id)
        .ok_or_else(|| anyhow::anyhow!("invalid session id for lease: {}", lease.session_id))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(lease)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

pub fn remove_session_lease(session_id: &str) {
    if let Some(path) = lease_path(session_id) {
        let _ = std::fs::remove_file(path);
    }
}

/// Check whether a session copy at `copy_epoch` may run turns / persist here.
/// Cheap when no session ever migrated: a single failed directory lookup.
pub fn session_lease_block(session_id: &str, copy_epoch: u64) -> Option<SessionLeaseBlock> {
    let lease = read_session_lease(session_id)?;
    if let Some(host) = lease.away_host {
        return Some(SessionLeaseBlock::MovedAway {
            host,
            epoch: lease.epoch,
        });
    }
    (copy_epoch < lease.epoch).then_some(SessionLeaseBlock::StaleCopy {
        copy_epoch,
        lease_epoch: lease.epoch,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_home<T>(f: impl FnOnce() -> T) -> T {
        let _guard = crate::test_jcode_home_lock();
        let temp = tempfile::tempdir().unwrap();
        jcode_core::env::set_var("JCODE_HOME", temp.path());
        let out = f();
        jcode_core::env::remove_var("JCODE_HOME");
        out
    }

    fn lease(epoch: u64, away: Option<&str>) -> SessionLease {
        SessionLease {
            session_id: "session_x".into(),
            epoch,
            away_host: away.map(str::to_string),
            repo_root: None,
            last_sync_snapshot: None,
            last_sync_head: None,
            updated_at: "now".into(),
        }
    }

    #[test]
    fn no_lease_never_blocks() {
        with_home(|| assert_eq!(session_lease_block("session_x", 0), None));
    }

    #[test]
    fn moved_away_blocks_every_copy() {
        with_home(|| {
            write_session_lease(&lease(3, Some("cloud"))).unwrap();
            assert!(matches!(
                session_lease_block("session_x", 3),
                Some(SessionLeaseBlock::MovedAway { .. })
            ));
        });
    }

    #[test]
    fn returned_session_blocks_only_stale_copies() {
        with_home(|| {
            write_session_lease(&lease(4, None)).unwrap();
            assert_eq!(
                session_lease_block("session_x", 2),
                Some(SessionLeaseBlock::StaleCopy {
                    copy_epoch: 2,
                    lease_epoch: 4
                })
            );
            assert_eq!(session_lease_block("session_x", 4), None);
        });
    }

    #[test]
    fn rejects_path_like_ids() {
        with_home(|| {
            assert!(read_session_lease("../x").is_none());
            assert!(
                write_session_lease(&SessionLease {
                    session_id: "../x".into(),
                    ..lease(1, None)
                })
                .is_err()
            );
        });
    }
}
