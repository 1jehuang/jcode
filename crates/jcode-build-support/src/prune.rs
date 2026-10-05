//! Retention for the immutable version store (`builds/versions/<label>`).
//!
//! Every install, self-dev publish and `/update` adds a full binary under a new
//! label, and nothing removed old ones, so the store grew without bound.
//! [`prune_old_versions`] keeps the newest few installs plus every version that
//! is still referenced or running, and deletes the rest.

use crate::{BuildManifest, binary_name, builds_dir, launcher_binary_path};
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Unreferenced installs kept by default, newest first, for manual rollback.
pub const DEFAULT_KEEP_VERSIONS: usize = 3;
/// Environment override for [`DEFAULT_KEEP_VERSIONS`]. `0` turns automatic
/// pruning off.
pub const KEEP_VERSIONS_ENV: &str = "JCODE_KEEP_BUILDS";

/// Channels whose targets must survive: `builds/<channel>/<binary>` links and
/// `builds/<channel>-version` markers. Markers matter on Windows, where channel
/// "links" are copies.
const CHANNELS: [&str; 4] = ["current", "stable", "shared-server", "canary"];

/// Identity of a file that survives renames and hard links: (device, inode).
type FileId = (u64, u64);

#[derive(Debug, Default)]
pub struct PruneReport {
    /// Kept versions with the reasons they were kept, newest first.
    pub kept: Vec<(String, Vec<&'static str>)>,
    /// Removed versions (for a dry run, the ones that would be removed).
    pub removed: Vec<String>,
    pub freed_bytes: u64,
    /// Versions that could not be deleted, e.g. a loaded executable on Windows.
    pub failed: Vec<(String, String)>,
    /// Why nothing was pruned, when pruning was skipped entirely.
    pub skipped: Option<&'static str>,
}

impl PruneReport {
    pub fn render(&self, dry_run: bool) -> String {
        if let Some(reason) = self.skipped {
            return format!("Skipped pruning old builds: {reason}.\n");
        }
        let mut out = if self.removed.is_empty() {
            "No old builds to remove.\n".to_string()
        } else {
            format!(
                "{} {} old build(s), {}: {}\n",
                if dry_run { "Would remove" } else { "Removed" },
                self.removed.len(),
                format_size(self.freed_bytes),
                self.removed.join(", ")
            )
        };
        out.push_str(&format!("Kept {}:\n", self.kept.len()));
        for (label, why) in &self.kept {
            out.push_str(&format!("  {label}  ({})\n", why.join(", ")));
        }
        for (label, error) in &self.failed {
            out.push_str(&format!("Could not remove {label}: {error}\n"));
        }
        out
    }
}

/// Keep count from [`KEEP_VERSIONS_ENV`], or the default. `None` means
/// automatic pruning is turned off.
pub fn keep_versions_setting() -> Option<usize> {
    match std::env::var(KEEP_VERSIONS_ENV) {
        Ok(value) => parse_keep(Some(&value)),
        Err(_) => parse_keep(None),
    }
}

fn parse_keep(raw: Option<&str>) -> Option<usize> {
    let Some(value) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Some(DEFAULT_KEEP_VERSIONS);
    };
    match value.parse::<usize>() {
        Ok(0) => None,
        Ok(keep) => Some(keep),
        Err(_) => Some(DEFAULT_KEEP_VERSIONS),
    }
}

/// Prune after an install. Never fails the install: problems are only logged.
pub fn prune_old_versions_after_install() {
    prune_after_install_keeping(None);
}

/// `jcode prune-builds`: prune with `keep` (else the configured or default
/// count) and print what happened.
pub fn run_prune_builds_command(keep: Option<usize>, dry_run: bool) -> Result<()> {
    let keep = keep
        .or_else(keep_versions_setting)
        .unwrap_or(DEFAULT_KEEP_VERSIONS);
    print!("{}", prune_old_versions(keep, dry_run)?.render(dry_run));
    Ok(())
}

/// Like [`prune_old_versions_after_install`], also keeping `rollback`, the
/// version a self-dev publish may roll `current` back to.
pub(crate) fn prune_after_install_keeping(rollback: Option<&str>) {
    let Some(keep) = keep_versions_setting() else {
        return;
    };
    let result = running_executable_ids()
        .map(|running| prune_versions(keep, false, &running, rollback))
        .transpose();
    match result {
        Ok(Some(report)) if !report.removed.is_empty() || !report.failed.is_empty() => {
            jcode_logging::info(report.render(false).trim_end());
        }
        Ok(_) => {}
        Err(error) => jcode_logging::warn(&format!("Pruning old builds failed: {error:#}")),
    }
}

/// Delete installed versions except the `keep` newest and every version that a
/// channel, marker, launcher, pending activation or running process uses.
pub fn prune_old_versions(keep: usize, dry_run: bool) -> Result<PruneReport> {
    let Some(running) = running_executable_ids() else {
        return Ok(PruneReport {
            skipped: Some("running processes cannot be listed on this platform"),
            ..PruneReport::default()
        });
    };
    prune_versions(keep, dry_run, &running, None)
}

fn prune_versions(
    keep: usize,
    dry_run: bool,
    running: &HashSet<FileId>,
    rollback: Option<&str>,
) -> Result<PruneReport> {
    let versions_dir = builds_dir()?.join("versions");
    let mut report = PruneReport::default();
    let Ok(entries) = std::fs::read_dir(&versions_dir) else {
        return Ok(report);
    };
    let mut installed: Vec<(SystemTime, String, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let (Ok(label), true) = (
            entry.file_name().into_string(),
            entry.file_type().is_ok_and(|kind| kind.is_dir()),
        ) else {
            continue;
        };
        if !label.starts_with('.') {
            installed.push((installed_at(&entry.path()), label, entry.path()));
        }
    }
    installed.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));

    // Fails closed: an unreadable manifest aborts before anything is deleted.
    let mut reasons = referenced_versions(&versions_dir)?;
    if let Some(rollback) = rollback {
        reasons
            .entry(rollback.to_string())
            .or_default()
            .insert("rollback");
    }
    for (index, (_, label, path)) in installed.into_iter().enumerate() {
        let mut why: Vec<&'static str> = reasons.remove(&label).into_iter().flatten().collect();
        if index < keep {
            why.push("recent");
        }
        if contains_running_file(&path, running) {
            why.push("running");
        }
        if !why.is_empty() {
            report.kept.push((label, why));
            continue;
        }
        let size = dir_size(&path);
        let result = if dry_run {
            Ok(())
        } else {
            std::fs::remove_dir_all(&path)
        };
        match result {
            Ok(()) => {
                report.removed.push(label);
                report.freed_bytes += size;
            }
            Err(error) => report.failed.push((label, error.to_string())),
        }
    }
    Ok(report)
}

/// Versions named by a channel link, version marker, the launcher or the build
/// manifest, with the reasons.
fn referenced_versions(versions_dir: &Path) -> Result<BTreeMap<String, BTreeSet<&'static str>>> {
    let mut reasons: BTreeMap<String, BTreeSet<&'static str>> = BTreeMap::new();
    let mut protect = |label: Option<String>, why: &'static str| {
        if let Some(label) = label {
            reasons.entry(label).or_default().insert(why);
        }
    };
    let canonical_versions =
        std::fs::canonicalize(versions_dir).unwrap_or_else(|_| versions_dir.to_path_buf());
    let builds = versions_dir.parent().unwrap_or(versions_dir);
    for channel in CHANNELS {
        let link = builds.join(channel).join(binary_name());
        protect(label_in(&canonical_versions, &link), channel);
        protect(
            read_marker(&builds.join(format!("{channel}-version"))),
            channel,
        );
    }
    protect(
        label_in(&canonical_versions, &launcher_binary_path()?),
        "launcher",
    );
    let manifest = BuildManifest::load()?;
    protect(manifest.stable, "manifest");
    protect(manifest.canary, "manifest");
    if let Some(pending) = manifest.pending_activation {
        protect(Some(pending.new_version), "pending activation");
        protect(pending.previous_current_version, "pending activation");
        protect(pending.previous_shared_server_version, "pending activation");
    }
    Ok(reasons)
}

/// Version label of `path` when it resolves into `canonical_versions`. A link
/// that does not resolve names no version.
fn label_in(canonical_versions: &Path, path: &Path) -> Option<String> {
    let Ok(resolved) = std::fs::canonicalize(path) else {
        return None;
    };
    let Ok(relative) = resolved.strip_prefix(canonical_versions) else {
        return None;
    };
    let first = relative.components().next()?;
    first.as_os_str().to_str().map(str::to_string)
}

/// Trimmed contents of a version marker, if it exists and is not empty.
fn read_marker(path: &Path) -> Option<String> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return None;
    };
    let label = content.trim();
    (!label.is_empty()).then(|| label.to_string())
}

/// When a version was installed: the newest file mtime inside it (install paths
/// write or link the binary at install time), else the directory mtime.
fn installed_at(dir: &Path) -> SystemTime {
    let mut newest = match std::fs::metadata(dir).and_then(|meta| meta.modified()) {
        Ok(modified) => modified,
        Err(_) => SystemTime::UNIX_EPOCH,
    };
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if let Ok(meta) = entry.metadata()
            && meta.is_file()
            && let Ok(modified) = meta.modified()
        {
            newest = newest.max(modified);
        }
    }
    newest
}

/// File id of `path` (following symlinks), if it can be read.
fn path_file_id(path: &Path) -> Option<FileId> {
    match std::fs::metadata(path) {
        Ok(meta) => file_id(&meta),
        Err(_) => None,
    }
}

fn contains_running_file(dir: &Path, running: &HashSet<FileId>) -> bool {
    !running.is_empty()
        && std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .any(|entry| path_file_id(&entry.path()).is_some_and(|id| running.contains(&id)))
}

fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => total += dir_size(&entry.path()),
            Ok(kind) if kind.is_file() => {
                if let Ok(meta) = entry.metadata() {
                    total += meta.len();
                }
            }
            _ => {}
        }
    }
    total
}

fn format_size(bytes: u64) -> String {
    let mib = bytes as f64 / (1024.0 * 1024.0);
    if mib >= 1024.0 {
        format!("{:.1} GiB", mib / 1024.0)
    } else {
        format!("{mib:.1} MiB")
    }
}

#[cfg(unix)]
fn file_id(meta: &std::fs::Metadata) -> Option<FileId> {
    use std::os::unix::fs::MetadataExt;
    Some((meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
fn file_id(_meta: &std::fs::Metadata) -> Option<FileId> {
    None
}

/// File ids of the executables of every process this user can inspect. Ids,
/// not paths, so a version reached through a channel symlink or a hard link
/// from `target/` still counts. `None` when the platform cannot tell, in which
/// case nothing is pruned.
#[cfg(target_os = "linux")]
fn running_executable_ids() -> Option<HashSet<FileId>> {
    use std::os::unix::ffi::OsStrExt;
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return None;
    };
    let ids = entries
        .flatten()
        .filter(|entry| entry.file_name().as_bytes().iter().all(u8::is_ascii_digit))
        // `/proc/<pid>/exe` resolves to the mapped file even after it was
        // replaced or unlinked. Other users' processes are unreadable and
        // cannot be running this user's builds.
        .filter_map(|entry| path_file_id(&entry.path().join("exe")))
        .collect();
    Some(ids)
}

#[cfg(target_os = "macos")]
fn running_executable_ids() -> Option<HashSet<FileId>> {
    use std::os::unix::ffi::OsStrExt;
    // SAFETY: a null buffer only asks libproc for the number of pids.
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if count <= 0 {
        return None;
    }
    // Headroom for processes started between the two calls.
    let mut pids: Vec<libc::c_int> = vec![0; count as usize + 64];
    let buffer_bytes = (pids.len() * std::mem::size_of::<libc::c_int>()) as libc::c_int;
    // SAFETY: `pids` is writable for exactly `buffer_bytes` bytes.
    let filled = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), buffer_bytes) };
    if filled <= 0 {
        return None;
    }
    pids.truncate(filled as usize);
    let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let mut ids = HashSet::new();
    for pid in pids {
        // SAFETY: `buffer` is writable for its full length.
        let len =
            unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
        if len <= 0 {
            continue;
        }
        let path = Path::new(std::ffi::OsStr::from_bytes(&buffer[..len as usize]));
        if let Some(id) = path_file_id(path) {
            ids.insert(id);
        }
    }
    Some(ids)
}

/// Elsewhere (including Windows, where a release dir holds more than the
/// locked executable and could be half deleted) nothing is pruned.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn running_executable_ids() -> Option<HashSet<FileId>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::with_temp_jcode_home;
    use crate::{update_current_symlink, update_shared_server_symlink, update_stable_symlink};
    use std::time::Duration;

    fn install(label: &str, age_secs: u64) -> PathBuf {
        let dir = builds_dir().unwrap().join("versions").join(label);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(binary_name());
        std::fs::write(&path, format!("binary {label}")).unwrap();
        let mtime = SystemTime::now() - Duration::from_secs(age_secs);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        path
    }

    fn remaining() -> Vec<String> {
        let mut labels: Vec<String> = std::fs::read_dir(builds_dir().unwrap().join("versions"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        labels.sort();
        labels
    }

    #[test]
    fn keep_setting_defaults_disables_on_zero_and_ignores_junk() {
        assert_eq!(parse_keep(None), Some(DEFAULT_KEEP_VERSIONS));
        assert_eq!(parse_keep(Some("  ")), Some(DEFAULT_KEEP_VERSIONS));
        assert_eq!(parse_keep(Some("5")), Some(5));
        assert_eq!(parse_keep(Some("0")), None);
        assert_eq!(parse_keep(Some("lots")), Some(DEFAULT_KEEP_VERSIONS));
    }

    #[test]
    fn prune_keeps_recent_and_referenced_versions_and_removes_the_rest() {
        with_temp_jcode_home(|| {
            for (index, label) in ["v1", "v2", "v3", "v4", "v5", "v6", "v7", "v8"]
                .iter()
                .enumerate()
            {
                install(label, 1000 - index as u64 * 100);
            }
            // Channels on old versions: stable via link + marker, shared-server
            // pinned to an older build, current on the newest.
            update_stable_symlink("v2").unwrap();
            update_shared_server_symlink("v1").unwrap();
            update_current_symlink("v8").unwrap();
            let mut manifest = BuildManifest::load().unwrap_or_default();
            manifest.pending_activation = Some(crate::PendingActivation {
                session_id: "s".into(),
                new_version: "v8".into(),
                previous_current_version: Some("v3".into()),
                previous_shared_server_version: None,
                source_fingerprint: None,
                requested_at: chrono::Utc::now(),
            });
            manifest.save().unwrap();

            let dry = prune_versions(2, true, &HashSet::new(), None).unwrap();
            assert_eq!(dry.removed, vec!["v6", "v5", "v4"]);
            assert_eq!(remaining().len(), 8, "a dry run must not delete anything");

            let report = prune_versions(2, false, &HashSet::new(), Some("v5")).unwrap();
            assert_eq!(report.removed, vec!["v6", "v4"]);
            assert!(report.freed_bytes > 0);
            assert_eq!(remaining(), vec!["v1", "v2", "v3", "v5", "v7", "v8"]);
            let why = |label: &str| {
                report
                    .kept
                    .iter()
                    .find(|(kept, _)| kept == label)
                    .map(|(_, why)| why.clone())
                    .unwrap()
            };
            assert!(why("v8").contains(&"current") && why("v8").contains(&"recent"));
            assert_eq!(why("v7"), vec!["recent"]);
            assert_eq!(why("v5"), vec!["rollback"]);
            assert_eq!(why("v3"), vec!["pending activation"]);
            assert!(why("v2").contains(&"stable"));
            assert!(why("v1").contains(&"shared-server"));
        });
    }

    #[test]
    #[cfg(unix)]
    fn prune_keeps_a_version_whose_binary_is_running() {
        with_temp_jcode_home(|| {
            install("old-running", 500);
            install("old-idle", 400);
            let running_binary = install("old-running-link", 300);
            install("new", 10);
            update_current_symlink("new").unwrap();
            // A process that started from `old-running` through any hard link
            // or symlink has that binary's file id.
            let original = builds_dir()
                .unwrap()
                .join("versions/old-running")
                .join(binary_name());
            std::fs::remove_file(&original).unwrap();
            std::fs::hard_link(&running_binary, &original).unwrap();
            let running: HashSet<FileId> = [file_id(&std::fs::metadata(&running_binary).unwrap())]
                .into_iter()
                .flatten()
                .collect();

            let report = prune_versions(0, false, &running, None).unwrap();

            assert_eq!(report.removed, vec!["old-idle"]);
            assert_eq!(remaining(), vec!["new", "old-running", "old-running-link"]);
        });
    }

    #[test]
    fn running_ids_include_this_test_process() {
        let ids = running_executable_ids();
        if !cfg!(any(target_os = "linux", target_os = "macos")) {
            assert!(ids.is_none(), "unsupported platforms must not prune");
            return;
        }
        let ids = ids.expect("process listing works on this platform");
        let exe = std::env::current_exe().unwrap();
        let id = file_id(&std::fs::metadata(exe).unwrap()).unwrap();
        assert!(ids.contains(&id), "own executable missing from running set");
    }

    #[test]
    fn prune_without_a_versions_dir_is_a_no_op() {
        with_temp_jcode_home(|| {
            let report = prune_versions(1, false, &HashSet::new(), None).unwrap();
            assert!(report.removed.is_empty() && report.kept.is_empty());
        });
    }

    #[test]
    fn prune_deletes_nothing_when_the_manifest_is_unreadable() {
        with_temp_jcode_home(|| {
            install("old", 500);
            install("new", 10);
            let manifest = crate::manifest_path().unwrap();
            std::fs::write(&manifest, "{ not json").unwrap();
            std::fs::write(manifest.with_extension("bak"), "{ not json").unwrap();

            assert!(prune_versions(0, false, &HashSet::new(), None).is_err());
            assert_eq!(remaining(), vec!["new", "old"]);
        });
    }
}
