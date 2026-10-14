//! Garbage collection for `builds/versions/<version>/`.
//!
//! Every self-dev build, local release install, and update writes a new
//! immutable binary directory (roughly 400 MB each) and nothing used to remove
//! old ones. This keeps every version that is still referenced or running,
//! plus the most recently installed few for rollback, and deletes the rest.

use crate::{
    BuildManifest, builds_dir, read_current_version, read_shared_server_version,
    read_stable_version,
};
use anyhow::Result;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Unreferenced versions kept for rollback, newest first.
pub const DEFAULT_KEEP_RECENT_VERSIONS: usize = 5;

/// Never remove a version installed more recently than this, so an in-flight
/// build/publish sequence cannot lose its binary between install and symlink.
const MIN_VERSION_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// Override how many unreferenced versions to keep. `0` disables cleanup.
pub const KEEP_VERSIONS_ENV: &str = "JCODE_KEEP_BUILD_VERSIONS";

const PRUNE_INTERVAL_SECS: u64 = 24 * 60 * 60;
const PRUNE_MARKER: &str = "versions-prune.stamp";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct VersionPruneReport {
    pub removed: Vec<String>,
    pub kept: Vec<String>,
}

fn keep_recent_from_env() -> Option<usize> {
    let keep = std::env::var(KEEP_VERSIONS_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(DEFAULT_KEEP_RECENT_VERSIONS);
    (keep > 0).then_some(keep)
}

/// Versions that must never be removed: active channels, the build manifest's
/// canary/stable/pending-activation rollback targets.
fn protected_versions() -> HashSet<String> {
    let mut protected = HashSet::new();
    for version in [
        read_current_version(),
        read_stable_version(),
        read_shared_server_version(),
    ]
    .into_iter()
    .flatten()
    .flatten()
    {
        protected.insert(version);
    }
    if let Ok(dir) = builds_dir() {
        for channel in ["current", "stable", "shared-server", "canary"] {
            if let Some(version) = channel_target_version(&dir.join(channel)) {
                protected.insert(version);
            }
        }
    }
    if let Ok(manifest) = BuildManifest::load() {
        protected.extend(manifest.stable.clone());
        protected.extend(manifest.canary.clone());
        if let Some(pending) = manifest.pending_activation.as_ref() {
            protected.insert(pending.new_version.clone());
            protected.extend(pending.previous_current_version.clone());
            protected.extend(pending.previous_shared_server_version.clone());
        }
    }
    protected
}

/// Resolve `<channel>/jcode -> versions/<v>/jcode` to `<v>`.
fn channel_target_version(channel_dir: &Path) -> Option<String> {
    let entries = std::fs::read_dir(channel_dir).ok()?;
    for entry in entries.flatten() {
        if let Ok(target) = std::fs::read_link(entry.path())
            && let Some(version) = version_from_binary_path(&target)
        {
            return Some(version);
        }
    }
    None
}

fn version_from_binary_path(path: &Path) -> Option<String> {
    let mut components = path.components().rev();
    components.next()?; // binary file name
    let version = components.next()?.as_os_str().to_str()?.to_string();
    let parent = components.next()?.as_os_str();
    (parent == "versions").then_some(version)
}

/// Versions whose binary is the executable of a live process.
#[cfg(target_os = "linux")]
fn running_versions() -> HashSet<String> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return HashSet::new();
    };
    entries
        .flatten()
        .filter_map(|entry| std::fs::read_link(entry.path().join("exe")).ok())
        .filter_map(|exe| {
            // A deleted-but-running binary reads as "<path> (deleted)".
            let text = exe.to_string_lossy();
            let trimmed = text.strip_suffix(" (deleted)").unwrap_or(&text);
            version_from_binary_path(Path::new(trimmed))
        })
        .collect()
}

/// Versions whose binary is the executable of a live process. macOS has no
/// `/proc`; libproc reports each process's executable path instead.
#[cfg(target_os = "macos")]
fn running_versions() -> HashSet<String> {
    use std::os::unix::ffi::OsStrExt;
    // SAFETY: a null buffer only asks libproc for the number of pids.
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    let Ok(count) = usize::try_from(count) else {
        return HashSet::new();
    };
    // Headroom for processes started between the two calls.
    let mut pids: Vec<libc::c_int> = vec![0; count + 64];
    let Ok(buffer_bytes) = libc::c_int::try_from(pids.len() * std::mem::size_of::<libc::c_int>())
    else {
        return HashSet::new();
    };
    // SAFETY: `pids` is writable for exactly `buffer_bytes` bytes.
    let filled = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), buffer_bytes) };
    pids.truncate(usize::try_from(filled).unwrap_or(0));
    let mut path = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    pids.into_iter()
        .filter_map(|pid| {
            // SAFETY: `path` is writable for its full length.
            let len =
                unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
            // A failed lookup (for example another user's process) returns 0 or less.
            let exe = std::ffi::OsStr::from_bytes(path.get(..usize::try_from(len).unwrap_or(0))?);
            version_from_binary_path(Path::new(exe))
        })
        .collect()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn running_versions() -> HashSet<String> {
    HashSet::new()
}

/// Remove old unreferenced version directories. Best-effort, rate limited to
/// one pass per day per machine, safe to call from a startup thread.
pub fn prune_old_versions() {
    let Some(keep_recent) = keep_recent_from_env() else {
        return;
    };
    let Ok(dir) = builds_dir() else {
        return;
    };
    if !prune_due(&dir) {
        return;
    }
    let mut protected = protected_versions();
    protected.extend(running_versions());
    let _ = prune_old_versions_in(
        &dir.join("versions"),
        &protected,
        keep_recent,
        SystemTime::now(),
    );
    mark_pruned(&dir);
}

/// True when the last completed pass is older than the interval. The marker
/// is only written after a pass finishes (see [`mark_pruned`]), so a process
/// killed mid-walk (for example by an exec reload) does not suppress the next
/// attempt for a whole day.
fn prune_due(dir: &Path) -> bool {
    let marker = dir.join(PRUNE_MARKER);
    match std::fs::metadata(&marker).and_then(|meta| meta.modified()) {
        Ok(modified) => SystemTime::now()
            .duration_since(modified)
            .map(|age| age.as_secs() >= PRUNE_INTERVAL_SECS)
            .unwrap_or(true),
        Err(_) => true,
    }
}

fn mark_pruned(dir: &Path) {
    let _ = std::fs::write(dir.join(PRUNE_MARKER), b"");
}

/// Core of [`prune_old_versions`], parameterized for unit tests.
pub fn prune_old_versions_in(
    versions_dir: &Path,
    protected: &HashSet<String>,
    keep_recent: usize,
    now: SystemTime,
) -> Result<VersionPruneReport> {
    let mut candidates: Vec<(SystemTime, String, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(versions_dir)?.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let modified = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .unwrap_or(now);
        candidates.push((modified, name, entry.path()));
    }
    // Newest first.
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

    let mut report = VersionPruneReport::default();
    let mut recent_kept = 0usize;
    for (modified, name, path) in candidates {
        let too_new = now
            .duration_since(modified)
            .map(|age| age < MIN_VERSION_AGE)
            .unwrap_or(true);
        if protected.contains(&name) || too_new {
            report.kept.push(name);
            continue;
        }
        if recent_kept < keep_recent {
            recent_kept += 1;
            report.kept.push(name);
            continue;
        }
        if std::fs::remove_dir_all(&path).is_ok() {
            report.removed.push(name);
        } else {
            report.kept.push(name);
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};

    const DAY: Duration = Duration::from_secs(24 * 60 * 60);

    fn make_version(dir: &Path, name: &str, age: Duration) {
        let version = dir.join(name);
        fs::create_dir_all(&version).unwrap();
        fs::write(version.join("jcode"), b"bin").unwrap();
        File::open(&version)
            .and_then(|file| file.set_modified(SystemTime::now() - age))
            .unwrap();
    }

    #[test]
    fn keeps_protected_recent_and_fresh_versions() {
        let temp = tempfile::tempdir().unwrap();
        let versions = temp.path();
        // v0 is newest (installed an hour ago), v9 is oldest.
        make_version(versions, "v0", Duration::from_secs(3600));
        for i in 1..10u64 {
            make_version(versions, &format!("v{i}"), DAY * (i as u32 + 1));
        }
        let protected: HashSet<String> = ["v9".to_string()].into_iter().collect();

        let report = prune_old_versions_in(versions, &protected, 3, SystemTime::now()).unwrap();

        // v0 too fresh, v1-v3 recent, v9 protected.
        for kept in ["v0", "v1", "v2", "v3", "v9"] {
            assert!(versions.join(kept).exists(), "{kept} should be kept");
        }
        for removed in ["v4", "v5", "v6", "v7", "v8"] {
            assert!(!versions.join(removed).exists(), "{removed} should go");
        }
        assert_eq!(report.removed.len(), 5);
    }

    /// A build an older session still runs from must survive the daily pass.
    /// macOS had no process lookup, so such builds were deleted there.
    #[cfg(target_os = "macos")]
    #[test]
    fn daily_pass_keeps_a_build_a_process_runs() {
        let temp = tempfile::tempdir().unwrap();
        // running_versions() matches `.../versions/<version>/<binary>`.
        let versions = &temp.path().join("versions");
        make_version(versions, "v-idle", DAY * 3);
        make_version(versions, "v-running", DAY * 3);
        let binary = versions.join("v-running").join("jcode");
        fs::copy("/bin/sleep", &binary).unwrap();
        File::open(versions.join("v-running"))
            .and_then(|dir| dir.set_modified(SystemTime::now() - DAY * 3))
            .unwrap();
        let mut child = std::process::Command::new(&binary)
            .arg("30")
            .spawn()
            .unwrap();
        // The executable path is visible once exec completes.
        let mut running = HashSet::new();
        for _ in 0..100 {
            std::thread::sleep(Duration::from_millis(20));
            running = running_versions();
            if running.contains("v-running") {
                break;
            }
        }

        let report = prune_old_versions_in(versions, &running, 0, SystemTime::now()).unwrap();
        child.kill().unwrap();
        child.wait().unwrap();

        assert_eq!(report.removed, vec!["v-idle".to_string()]);
        assert!(
            versions.join("v-running").exists(),
            "a running build must be kept"
        );
    }

    #[test]
    fn parses_version_from_binary_path() {
        assert_eq!(
            version_from_binary_path(Path::new("/h/.jcode/builds/versions/abc123/jcode")),
            Some("abc123".to_string())
        );
        assert_eq!(
            version_from_binary_path(Path::new("/h/target/selfdev/jcode")),
            None
        );
    }
}
