//! Best-effort, local-only first-paint quota hints published by the daemon.
//! This file is advisory, never an authentication or account-routing source.

use jcode_protocol::AccountUsageSnapshot;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SCHEMA: u32 = 1;
const MAX_BYTES: u64 = 64 * 1024;
// Allow normal 30-second refresh phase skew and worker latency, but never
// resurrect an old daemon snapshot just because a client read it recently.
const MAX_AGE: Duration = Duration::from_secs(90);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct SocketIdentity {
    device: u64,
    inode: u64,
    changed_secs: i64,
    changed_nanos: i64,
}

#[derive(Serialize, Deserialize)]
struct Snapshot {
    schema: u32,
    socket: PathBuf,
    identity: SocketIdentity,
    instance_id: String,
    pid: u32,
    published_ms: u64,
    anthropic: Vec<AccountUsageSnapshot>,
    openai: Vec<AccountUsageSnapshot>,
}

/// Publisher bound to the actual listening socket, not an ambient override.
#[derive(Clone)]
pub struct AccountUsageStartupPublisher {
    socket: PathBuf,
    path: PathBuf,
    identity: SocketIdentity,
    instance_id: String,
}

fn now_ms() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis()
        .try_into()
        .ok()
}

fn normalized_socket(socket: &Path) -> Option<PathBuf> {
    let absolute = if socket.is_absolute() {
        socket.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(socket)
    };
    // Match server socket ownership semantics: resolve the parent, not a
    // symlink in the last component (bind replaces that component).
    Some(
        std::fs::canonicalize(absolute.parent()?)
            .ok()?
            .join(absolute.file_name()?),
    )
}

fn cache_path(socket: &Path) -> PathBuf {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(socket.as_os_str().as_encoded_bytes());
    crate::storage::runtime_dir()
        .join("jcode-usage")
        .join(format!("{}.json", hex::encode(digest)))
}

#[cfg(unix)]
fn socket_identity(socket: &Path) -> Option<SocketIdentity> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let metadata = std::fs::symlink_metadata(socket).ok()?;
    if !metadata.file_type().is_socket() || metadata.uid() != unsafe { libc::geteuid() } {
        return None;
    }
    Some(SocketIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        changed_secs: metadata.ctime(),
        changed_nanos: metadata.ctime_nsec(),
    })
}

#[cfg(not(unix))]
fn socket_identity(_socket: &Path) -> Option<SocketIdentity> {
    None
}

impl AccountUsageStartupPublisher {
    pub fn new(socket: &Path, instance_id: &str) -> Option<Self> {
        let socket = normalized_socket(socket)?;
        let identity = socket_identity(&socket)?;
        Some(Self {
            path: cache_path(&socket),
            socket,
            identity,
            instance_id: instance_id.to_string(),
        })
    }

    /// Only called after a completed daemon cache update. No endpoint access.
    pub fn publish(
        &self,
        anthropic: &[AccountUsageSnapshot],
        openai: &[AccountUsageSnapshot],
    ) -> anyhow::Result<()> {
        if socket_identity(&self.socket).as_ref() != Some(&self.identity) {
            return Ok(());
        }
        let Some(published_ms) = now_ms() else {
            return Ok(());
        };
        let strip_email = |accounts: &[AccountUsageSnapshot]| {
            accounts
                .iter()
                .cloned()
                .map(|mut account| {
                    account.email = None;
                    account
                })
                .collect()
        };
        let snapshot = Snapshot {
            schema: SCHEMA,
            socket: self.socket.clone(),
            identity: self.identity.clone(),
            instance_id: self.instance_id.clone(),
            pid: std::process::id(),
            published_ms,
            anthropic: strip_email(anthropic),
            openai: strip_email(openai),
        };
        if serde_json::to_vec(&snapshot)?.len() as u64 > MAX_BYTES {
            return Ok(());
        }
        // Private directory and private atomic replacement. Quotas and labels
        // are not credentials, but are still private account metadata.
        crate::storage::write_json_secret(&self.path, &snapshot)
    }
}

#[cfg(unix)]
fn read_private(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let uid = unsafe { libc::geteuid() };
    let parent = std::fs::symlink_metadata(path.parent()?).ok()?;
    if !parent.is_dir() || parent.uid() != uid || parent.mode() & 0o077 != 0 {
        return None;
    }
    // Do not follow symlinks or block on a substituted FIFO/device.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file()
        || metadata.uid() != uid
        || metadata.mode() & 0o077 != 0
        || metadata.len() > MAX_BYTES
    {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= MAX_BYTES).then_some(bytes)
}

#[cfg(not(unix))]
fn read_private(_path: &Path) -> Option<Vec<u8>> {
    None
}

fn snapshot_age(
    snapshot: &Snapshot,
    socket: &Path,
    identity: &SocketIdentity,
    now: u64,
) -> Option<Duration> {
    if snapshot.schema != SCHEMA
        || snapshot.socket != socket
        || &snapshot.identity != identity
        || snapshot.instance_id.is_empty()
        || snapshot.pid == 0
        || !crate::platform::is_process_running(snapshot.pid)
    {
        return None;
    }
    let age = Duration::from_millis(now.checked_sub(snapshot.published_ms)?);
    (age <= MAX_AGE).then_some(age)
}

/// Import a small private file before first paint. Missing/old/wrong-host data
/// stays unknown. Callers must skip this entirely for SSH-forwarded sockets.
/// Returns true only when an actual (possibly empty) snapshot was imported.
pub fn hydrate_remote_account_usage(socket: &Path) -> bool {
    let Some(socket) = normalized_socket(socket) else {
        return false;
    };
    let Some((snapshot, age)) = read_snapshot(&socket, &cache_path(&socket)) else {
        return false;
    };
    super::remote_accounts::set_remote_account_usage_with_age(
        snapshot.anthropic,
        snapshot.openai,
        age,
    );
    true
}

fn read_snapshot(socket: &Path, path: &Path) -> Option<(Snapshot, Duration)> {
    let identity = socket_identity(socket)?;
    let bytes = read_private(path)?;
    let mut snapshot = serde_json::from_slice::<Snapshot>(&bytes).ok()?;
    let now = now_ms()?;
    let age = snapshot_age(&snapshot, socket, &identity, now)?;
    if socket_identity(socket).as_ref() != Some(&identity) {
        return None;
    }
    discard_expired_windows(&mut snapshot.anthropic, now);
    discard_expired_windows(&mut snapshot.openai, now);
    Some((snapshot, age))
}

fn discard_expired_windows(accounts: &mut [AccountUsageSnapshot], now_ms: u64) {
    for account in accounts {
        let expired = account
            .resets_at
            .as_deref()
            .and_then(|reset| chrono::DateTime::parse_from_rfc3339(reset).ok())
            .is_some_and(|reset| i128::from(reset.timestamp_millis()) <= i128::from(now_ms));
        if expired {
            // The compact wire snapshot carries only one reset timestamp, not
            // a timestamp per window. Conservatively discard both ratios.
            account.five_hour_ratio = None;
            account.seven_day_ratio = None;
            account.exhausted = false;
            account.resets_at = None;
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::{fs::PermissionsExt, net::UnixListener};

    #[test]
    fn generation_and_age_fail_closed() {
        let socket = Path::new("/example.sock");
        let identity = SocketIdentity {
            device: 1,
            inode: 2,
            changed_secs: 3,
            changed_nanos: 4,
        };
        let mut snapshot = Snapshot {
            schema: SCHEMA,
            socket: socket.into(),
            identity: identity.clone(),
            instance_id: "daemon-one".into(),
            pid: std::process::id(),
            published_ms: 1000,
            anthropic: vec![],
            openai: vec![],
        };
        assert_eq!(
            snapshot_age(&snapshot, socket, &identity, 1001),
            Some(Duration::from_millis(1))
        );
        assert!(snapshot_age(&snapshot, socket, &identity, 999).is_none());
        assert!(snapshot_age(&snapshot, socket, &identity, 91_001).is_none());
        assert!(snapshot_age(&snapshot, Path::new("/other.sock"), &identity, 1001).is_none());
        snapshot.identity.inode += 1;
        assert!(snapshot_age(&snapshot, socket, &identity, 1001).is_none());
    }

    #[test]
    fn private_reader_rejects_permissions_symlinks_and_oversize() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.path().join("snapshot.json");
        crate::storage::write_json_secret(&path, &serde_json::json!({"ok":true})).unwrap();
        assert!(read_private(&path).is_some());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_private(&path).is_none());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::write(&path, vec![b'x'; MAX_BYTES as usize + 1]).unwrap();
        assert!(read_private(&path).is_none());
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(read_private(&link).is_none());
    }

    #[test]
    fn bound_publisher_does_not_replace_new_socket_generation() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("daemon.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let mut publisher = AccountUsageStartupPublisher::new(&socket, "test-instance").unwrap();
        // Keep test artifacts inside the temporary directory, not global cache.
        publisher.path = dir.path().join("usage.json");
        let account = AccountUsageSnapshot {
            label: "test".into(),
            email: Some("private@example.test".into()),
            exhausted: false,
            primary_label: None,
            five_hour_ratio: Some(0.3),
            secondary_label: None,
            seven_day_ratio: None,
            resets_at: None,
            error: None,
        };
        publisher.publish(&[], &[account]).unwrap();
        let bytes = read_private(&publisher.path).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("private@example.test"));
        let (snapshot, _) = read_snapshot(&publisher.socket, &publisher.path).unwrap();
        assert_eq!(snapshot.openai[0].five_hour_ratio, Some(0.3));
        let second = dir.path().join("other.sock");
        let _other = UnixListener::bind(&second).unwrap();
        assert!(read_snapshot(&second, &publisher.path).is_none());
        drop(listener);
        std::fs::remove_file(&socket).unwrap();
        let _replacement = UnixListener::bind(&socket).unwrap();
        publisher.publish(&[], &[]).unwrap();
        assert_eq!(read_private(&publisher.path).unwrap(), bytes);
        assert!(read_snapshot(&publisher.socket, &publisher.path).is_none());
    }

    #[test]
    fn publisher_captures_generation_after_socket_permissions_are_finalized() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("daemon.sock");
        let _listener = UnixListener::bind(&socket).unwrap();
        let mut before_chmod = AccountUsageStartupPublisher::new(&socket, "before-chmod").unwrap();
        before_chmod.path = dir.path().join("old-usage.json");
        // Ensure the metadata change is visible even on coarse test clocks.
        std::thread::sleep(Duration::from_millis(2));
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
        let current = socket_identity(&socket).unwrap();
        assert_eq!(before_chmod.identity.inode, current.inode);
        assert_ne!(before_chmod.identity, current);
        before_chmod.publish(&[], &[]).unwrap();
        assert!(!before_chmod.path.exists());
        let mut publisher = AccountUsageStartupPublisher::new(&socket, "after-chmod").unwrap();
        publisher.path = dir.path().join("usage.json");
        publisher.publish(&[], &[]).unwrap();
        assert!(read_snapshot(&publisher.socket, &publisher.path).is_some());
    }

    #[test]
    fn reset_boundary_discards_exhaustion_but_preserves_error() {
        let mut accounts = vec![AccountUsageSnapshot {
            label: "test".into(),
            email: None,
            exhausted: true,
            primary_label: Some("5h".into()),
            five_hour_ratio: Some(1.0),
            secondary_label: Some("7d".into()),
            seven_day_ratio: Some(0.5),
            resets_at: Some("2026-01-01T00:00:00Z".into()),
            error: Some("Usage unavailable".into()),
        }];
        let reset = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .timestamp_millis() as u64;
        discard_expired_windows(&mut accounts, reset - 1);
        assert!(accounts[0].exhausted);
        discard_expired_windows(&mut accounts, reset);
        assert!(!accounts[0].exhausted);
        assert!(accounts[0].five_hour_ratio.is_none());
        assert!(accounts[0].seven_day_ratio.is_none());
        assert_eq!(accounts[0].error.as_deref(), Some("Usage unavailable"));
    }
}
