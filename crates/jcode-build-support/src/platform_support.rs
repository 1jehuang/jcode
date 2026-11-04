// reference: https://doc.rust-lang.org/std/fs/fn.read_dir.html and
// https://doc.rust-lang.org/std/fs/fn.remove_file.html (Rust std directory
// enumeration and best-effort removal semantics used by the sweep helper).
use std::path::Path;
#[cfg(windows)]
use std::time::{Duration, SystemTime};

/// Prefix of the aside backup created for a live launcher during a Windows swap.
#[cfg(windows)]
const LAUNCHER_BACKUP_PREFIX: &str = ".jcode-launcher-old-";

/// Backups older than this are considered abandoned and get swept away.
#[cfg(windows)]
const STALE_LAUNCHER_BACKUP_AGE: Duration = Duration::from_secs(60 * 60);

/// Set file permissions to owner read/write/execute (0o755).
/// No-op on Windows (executability is determined by file extension).
pub fn set_permissions_executable(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(path, perms)
    }
    #[cfg(windows)]
    {
        let _ = path;
        Ok(())
    }
}

/// Atomically swap a symlink by creating a temp symlink and renaming.
///
/// On Unix: creates temp symlink, then renames over target (atomic).
/// On Windows: stages the source, renames the target aside, then moves the
/// staged file into place. This avoids the lock on a running executable but is
/// not fully atomic.
pub fn atomic_symlink_swap(src: &Path, dst: &Path, temp: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let _ = std::fs::remove_file(temp);
        std::os::unix::fs::symlink(src, temp)?;
        std::fs::rename(temp, dst)?;
    }
    #[cfg(windows)]
    {
        // Windows keeps a loaded executable open, so removing or copying over
        // the PATH launcher fails with ERROR_SHARING_VIOLATION. It does allow
        // the directory entry to be renamed while the process keeps running
        // from its existing handle. Stage the new file, rename the old entry
        // aside, then put the staged file at the stable path. This is the same
        // rename-aside strategy used by the PowerShell installer.
        let _ = std::fs::remove_file(temp);
        std::fs::copy(src, temp)?;

        // Sweep leftovers from earlier publishes before adding a new one.
        sweep_stale_launcher_backups(
            dst.parent().unwrap_or_else(|| Path::new(".")),
            STALE_LAUNCHER_BACKUP_AGE,
        );

        let operation_id = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        );
        let old = dst.parent().unwrap_or_else(|| Path::new(".")).join(format!(
            "{LAUNCHER_BACKUP_PREFIX}{operation_id}{}",
            dst.extension()
                .map(|extension| format!(".{}", extension.to_string_lossy()))
                .unwrap_or_default()
        ));
        let mut moved_old = false;
        if dst.exists() {
            match std::fs::rename(dst, &old) {
                Ok(()) => moved_old = true,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    let _ = std::fs::remove_file(temp);
                    return Err(error);
                }
            }
        }

        if let Err(error) = std::fs::rename(temp, dst) {
            if moved_old {
                let _ = std::fs::rename(&old, dst);
            }
            let _ = std::fs::remove_file(temp);
            return Err(error);
        }

        // An old loaded executable cannot be deleted until its process exits.
        // Best-effort cleanup is safe; the sweep above removes older leftovers.
        if moved_old {
            let _ = std::fs::remove_file(old);
        }
    }
    Ok(())
}

/// Delete stale `.jcode-launcher-old-*` backups left in `dir` by earlier swaps.
///
/// A backup is only removed when it is older than `max_age`; a file still held
/// open by a running process fails to delete on Windows and is skipped. Every
/// failure is ignored: the sweep must never break a publish.
#[cfg(windows)]
fn sweep_stale_launcher_backups(dir: &Path, max_age: Duration) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        if !file_name.starts_with(LAUNCHER_BACKUP_PREFIX) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        if !backup_is_stale(file_name, now, max_age) {
            continue;
        }
        // Best-effort removal: a backup still held open by a running process
        // fails to delete on Windows, and that must not abort the sweep.
        if std::fs::remove_file(entry.path()).is_err() {
            continue;
        }
    }
}

/// Whether a launcher backup is old enough to be swept.
///
/// The backup name carries the operation timestamp, so its age does not depend
/// on the mtime inherited from the renamed executable.
#[cfg(windows)]
fn backup_is_stale(file_name: &str, now: SystemTime, max_age: Duration) -> bool {
    let Some(created_at) = backup_operation_time(file_name) else {
        return false;
    };
    match now.duration_since(created_at) {
        Ok(age) => age >= max_age,
        Err(_) => false,
    }
}

/// Extract the operation timestamp encoded in `.jcode-launcher-old-<pid>-<nanos>`.
#[cfg(windows)]
fn backup_operation_time(file_name: &str) -> Option<SystemTime> {
    let rest = file_name.strip_prefix(LAUNCHER_BACKUP_PREFIX)?;
    let stem = rest.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(rest);
    let (_, nanos) = stem.rsplit_once('-')?;
    let nanos: u64 = match nanos.parse() {
        Ok(value) => value,
        Err(_) => return None,
    };
    Some(SystemTime::UNIX_EPOCH + Duration::from_nanos(nanos))
}

#[cfg(all(test, windows))]
mod tests {
    use super::{atomic_symlink_swap, sweep_stale_launcher_backups};
    use std::time::{Duration, SystemTime};

    #[test]
    fn windows_swap_replaces_existing_launcher_via_staged_file() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let src = dir.path().join("source.exe");
        let dst = dir.path().join("jcode.exe");
        let temp = dir.path().join(".jcode-current");
        std::fs::write(&src, b"new binary").expect("source");
        std::fs::write(&dst, b"old binary").expect("destination");

        atomic_symlink_swap(&src, &dst, &temp).expect("swap succeeds");

        assert_eq!(std::fs::read(&dst).expect("new launcher"), b"new binary");
        assert!(!temp.exists());
        assert!(
            !dir.path()
                .join(format!(".jcode-launcher-old-{}.exe", std::process::id()))
                .exists()
        );
    }

    #[test]
    fn sweep_removes_only_old_launcher_backups() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let now_nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let old_nanos = now_nanos - Duration::from_secs(2 * 60 * 60).as_nanos();

        let old_backup = dir
            .path()
            .join(format!(".jcode-launcher-old-1234-{old_nanos}.exe"));
        let fresh_backup = dir
            .path()
            .join(format!(".jcode-launcher-old-5678-{now_nanos}.exe"));
        let unrelated = dir.path().join("jcode.exe");
        for path in [&old_backup, &fresh_backup, &unrelated] {
            std::fs::write(path, b"payload").expect("fixture");
        }

        sweep_stale_launcher_backups(dir.path(), Duration::from_secs(60 * 60));

        assert!(!old_backup.exists(), "stale backup must be removed");
        assert!(fresh_backup.exists(), "fresh backup must be kept");
        assert!(unrelated.exists(), "unrelated file must be kept");
    }
}
