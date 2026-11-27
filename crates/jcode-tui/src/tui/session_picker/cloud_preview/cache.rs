//! Private, bounded cloud transcript cache. Keys never expose host or session names.
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

// Remote projection is at most 2 MiB plus bounded optional telemetry.
pub(super) const MAX_BYTES: usize = 3 * 1024 * 1024;
const CACHE_BUDGET: u64 = 24 * 1024 * 1024;
const TTL: Duration = Duration::from_secs(15);

pub(super) fn path(session_id: &str, host: &str, epoch: u64) -> Option<PathBuf> {
    let mut timer = super::StageTimer::new("cache_scope");
    let root = super::stage_result(crate::storage::jcode_dir, "cache_directory")?;
    let owner = crate::subscription_catalog::configured_api_key().unwrap_or_default();
    let endpoint = crate::subscription_api::configured_api_base();
    let path = scoped_key_path(&root, session_id, host, epoch, &owner, &endpoint);
    timer.outcome("success");
    Some(path)
}

#[cfg(test)]
fn key_path(root: &Path, session_id: &str, host: &str, epoch: u64) -> PathBuf {
    scoped_key_path(
        root,
        session_id,
        host,
        epoch,
        "test-owner",
        "https://test.invalid",
    )
}

fn scoped_key_path(
    root: &Path,
    session_id: &str,
    host: &str,
    epoch: u64,
    owner: &str,
    endpoint: &str,
) -> PathBuf {
    let mut digest = Sha256::new();
    for field in [
        owner.as_bytes(),
        endpoint.as_bytes(),
        host.as_bytes(),
        session_id.as_bytes(),
        &epoch.to_be_bytes(),
    ] {
        digest.update((field.len() as u64).to_be_bytes());
        digest.update(field);
    }
    root.join("cache/cloud-previews/v2")
        .join(format!("{:x}.blob", digest.finalize()))
}

pub(super) fn read(path: &Path) -> Option<Vec<u8>> {
    let mut timer = super::StageTimer::new("cache_read");
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            timer.outcome("miss");
            return None;
        }
        Err(_) => return None,
    };
    if super::stage_result(|| file.metadata(), "cache_metadata")?.len() > MAX_BYTES as u64 {
        timer.outcome("oversized");
        return None;
    }
    // Do not trust metadata for allocation or bounds: the file can grow concurrently.
    let mut bytes = Vec::new();
    super::stage_result(
        || file.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes),
        "cache_read_io",
    )?;
    timer.outcome(if bytes.len() <= MAX_BYTES {
        "hit"
    } else {
        "oversized"
    });
    (bytes.len() <= MAX_BYTES).then_some(bytes)
}

pub(super) fn fresh(path: &Path) -> bool {
    let mut timer = super::StageTimer::new("cache_freshness");
    let fresh = super::stage_result(
        || fs::metadata(path).and_then(|metadata| metadata.modified()),
        "cache_timestamp",
    )
    .and_then(|modified| super::stage_result(|| modified.elapsed(), "cache_age"))
    .is_some_and(|age| age < TTL);
    timer.outcome(if fresh {
        "fresh"
    } else {
        "stale_or_unavailable"
    });
    fresh
}

pub(super) fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut timer = super::StageTimer::new("cache_write");
    if bytes.len() > MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cloud preview exceeds cache size limit",
        ));
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "cache path needs a parent"))?;
    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    // NamedTempFile creates an owner-only file and removes it on every failure.
    // Persist renames within the same directory, leaving the old blob intact
    // until the complete replacement is ready.
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    prune(parent, path, CACHE_BUDGET);
    timer.outcome("success");
    Ok(())
}

/// Only our hashed blobs are eviction candidates. Never touch canonical
/// sessions, in-flight temporary files, symlinks, or unrelated cache entries.
fn prune(parent: &Path, keep: &Path, budget: u64) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    let mut blobs = Vec::new();
    let mut total = 0_u64;
    for entry in entries.flatten() {
        let path = entry.path();
        let ours = path.extension().and_then(|s| s.to_str()) == Some("blob")
            && path
                .file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()));
        if !ours || !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            total = total.saturating_add(meta.len());
            blobs.push((
                meta.modified().unwrap_or(std::time::UNIX_EPOCH),
                path,
                meta.len(),
            ));
        }
    }
    blobs.sort_unstable();
    for (_, path, size) in blobs {
        if total <= budget {
            break;
        }
        if path != keep && fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(size);
        }
    }
}

fn refreshing() -> &'static Mutex<HashSet<PathBuf>> {
    static REFRESHING: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    REFRESHING.get_or_init(|| Mutex::new(HashSet::new()))
}

pub(super) struct RefreshGuard {
    path: PathBuf,
}

pub(super) fn begin_refresh(path: &Path) -> Option<RefreshGuard> {
    let mut active = refreshing()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    active.insert(path.to_owned()).then(|| RefreshGuard {
        path: path.to_owned(),
    })
}

impl Drop for RefreshGuard {
    fn drop(&mut self) {
        refreshing()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    #[test]
    fn cold_miss_and_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/preview");
        assert_eq!(read(&path), None);
        assert!(!fresh(&path));
        write(&path, b"private transcript").unwrap();
        assert_eq!(read(&path).unwrap(), b"private transcript");
        assert!(fresh(&path));
        write(&path, b"").unwrap();
        assert_eq!(read(&path).unwrap(), b"");
    }

    #[test]
    fn atomic_replacement_keeps_open_reader_and_private_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preview");
        write(&path, b"old").unwrap();
        let mut original = File::open(&path).unwrap();
        write(&path, b"new").unwrap();
        let mut previous = Vec::new();
        original.read_to_end(&mut previous).unwrap();
        assert_eq!(previous, b"old");
        assert_eq!(read(&path).unwrap(), b"new");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn size_bounds_and_failed_write_preserve_last_good() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preview");
        let mut bytes = vec![0; MAX_BYTES];
        write(&path, &bytes).unwrap();
        assert_eq!(read(&path).unwrap().len(), MAX_BYTES);
        write(&path, b"last good").unwrap();
        bytes.push(0);
        assert_eq!(
            write(&path, &bytes).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(read(&path).unwrap(), b"last good");
        File::create(&path)
            .unwrap()
            .set_len(MAX_BYTES as u64 + 1)
            .unwrap();
        assert!(read(&path).is_none());
        let destination = dir.path().join("directory");
        fs::create_dir(&destination).unwrap();
        assert!(write(&destination, b"cannot replace directory").is_err());
        assert!(destination.is_dir());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn keys_scope_host_session_epoch_and_field_boundaries() {
        let root = Path::new("private-root");
        let baseline = key_path(root, "session", "host", 1);
        assert!(baseline.starts_with(root.join("cache/cloud-previews/v2")));
        for other in [
            key_path(root, "session2", "host", 1),
            key_path(root, "session", "host2", 1),
            key_path(root, "session", "host", 2),
        ] {
            assert_ne!(baseline, other);
        }
        assert_ne!(
            scoped_key_path(
                root,
                "session",
                "host",
                1,
                "owner-one",
                "https://api-one.invalid"
            ),
            scoped_key_path(
                root,
                "session",
                "host",
                1,
                "owner-two",
                "https://api-one.invalid"
            )
        );
        assert_ne!(
            scoped_key_path(
                root,
                "session",
                "host",
                1,
                "owner",
                "https://api-one.invalid"
            ),
            scoped_key_path(
                root,
                "session",
                "host",
                1,
                "owner",
                "https://api-two.invalid"
            )
        );
        assert_ne!(
            scoped_key_path(root, "session", "host", 1, "ab", "c"),
            scoped_key_path(root, "session", "host", 1, "a", "bc")
        );
        assert_ne!(key_path(root, "bc", "a", 0), key_path(root, "c", "ab", 0));
        let hostile = key_path(root, "../../session", "secret-host/..", 1);
        assert_eq!(
            hostile.parent(),
            Some(root.join("cache/cloud-previews/v2").as_path())
        );
        assert_eq!(hostile.file_name().unwrap().to_str().unwrap().len(), 69);
    }

    #[test]
    fn aggregate_budget_evicts_old_blobs_but_not_current_or_unrelated_files() {
        let root = tempfile::tempdir().unwrap();
        let oldest = key_path(root.path(), "oldest", "host", 1);
        let newer = key_path(root.path(), "newer", "host", 1);
        let keep = key_path(root.path(), "keep", "host", 1);
        for (path, age) in [(&oldest, 60), (&newer, 30), (&keep, 0)] {
            write(path, b"0123456789").unwrap();
            File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(SystemTime::now() - Duration::from_secs(age))
                .unwrap();
        }
        let parent = keep.parent().unwrap();
        let unrelated = parent.join("unrelated.blob");
        fs::write(&unrelated, b"leave alone").unwrap();
        prune(parent, &keep, 20);
        assert!(!oldest.exists());
        assert!(newer.exists() && keep.exists() && unrelated.exists());
        prune(parent, &keep, 5);
        assert!(!newer.exists());
        assert!(keep.exists() && unrelated.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(parent).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn stale_and_future_mtime_are_not_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preview");
        write(&path, b"blob").unwrap();
        let file = File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(16))
            .unwrap();
        assert!(!fresh(&path));
        file.set_modified(SystemTime::now() + Duration::from_secs(60))
            .unwrap();
        assert!(!fresh(&path));
        file.set_modified(SystemTime::now()).unwrap();
        assert!(fresh(&path));
    }

    #[test]
    fn single_flight_releases_on_drop_and_unwind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preview");
        let guard = begin_refresh(&path).unwrap();
        assert!(begin_refresh(&path).is_none());
        assert!(begin_refresh(&dir.path().join("other")).is_some());
        drop(guard);
        assert!(begin_refresh(&path).is_some());
        let result = std::panic::catch_unwind(|| {
            let _guard = begin_refresh(&path).unwrap();
            panic!("refresh failed");
        });
        assert!(result.is_err());
        assert!(begin_refresh(&path).is_some());
    }
}
