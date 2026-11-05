//! Background cleanup for `~/.jcode/scratch`.
//!
//! The bash tool points `TMPDIR` and `JCODE_SCRATCH_DIR` at this directory and
//! tells agents to put large temporary files there. Agents routinely clone
//! repositories or set `CARGO_TARGET_DIR` under it for isolated builds, and a
//! single cold Rust `target/` directory can be tens of gigabytes. Nothing ever
//! removed those, so long-lived installs accumulated hundreds of gigabytes.
//!
//! This module removes top-level scratch entries whose newest modification
//! (searched recursively) is older than a retention window, skipping anything
//! that a live process is currently using as its working directory.

use crate::storage;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Entries with no file modified within this many days are removed.
pub const DEFAULT_SCRATCH_RETENTION_DAYS: u64 = 7;

/// Override the retention window. `0` disables scratch cleanup entirely.
pub const SCRATCH_RETENTION_ENV: &str = "JCODE_SCRATCH_RETENTION_DAYS";

/// Minimum interval between prune passes across all jcode processes.
const PRUNE_INTERVAL_SECS: u64 = 24 * 60 * 60;

const PRUNE_MARKER: &str = "scratch-prune.stamp";

/// Summary of a prune pass, mostly for logging and tests.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScratchPruneReport {
    pub removed: Vec<PathBuf>,
    pub kept: usize,
}

fn retention_from_env() -> Option<Duration> {
    let days = std::env::var(SCRATCH_RETENTION_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_SCRATCH_RETENTION_DAYS);
    (days > 0).then(|| Duration::from_secs(days * 24 * 60 * 60))
}

/// Remove stale entries from `~/.jcode/scratch`.
///
/// Best-effort and rate limited to one pass per day per machine, so it is safe
/// to call from a background thread on every startup. Only the default scratch
/// directory is cleaned. A custom `JCODE_SCRATCH_DIR` is never touched.
pub fn prune_stale_scratch() {
    let Some(retention) = retention_from_env() else {
        return;
    };
    let Ok(base) = storage::jcode_dir() else {
        return;
    };
    let scratch = base.join("scratch");
    if !scratch.is_dir() || !prune_due(&base) {
        return;
    }
    // /proc cwd links are canonical, so compare against the canonical path.
    let scratch = std::fs::canonicalize(&scratch).unwrap_or(scratch);
    let Some(busy) = busy_process_dirs() else {
        // Without working cwd detection there is no way to know which entries are
        // in use, so skip the pass entirely rather than delete a live directory.
        // This is the fail-safe direction: not pruning costs disk, pruning a live
        // build orphans processes and destroys in-progress work.
        return;
    };
    let report = prune_stale_scratch_in(&scratch, SystemTime::now(), retention, &busy);
    mark_pruned(&base);
    if !report.removed.is_empty() {
        crate::logging::info(&format!(
            "scratch cleanup removed {} stale entr{} from {} (kept {})",
            report.removed.len(),
            if report.removed.len() == 1 {
                "y"
            } else {
                "ies"
            },
            scratch.display(),
            report.kept
        ));
    }
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

/// Core of [`prune_stale_scratch`], parameterized for unit testing.
///
/// `busy` lists directories that live processes are using (for example their
/// current working directory). Any scratch entry containing one is kept.
pub fn prune_stale_scratch_in(
    scratch: &Path,
    now: SystemTime,
    retention: Duration,
    busy: &[PathBuf],
) -> ScratchPruneReport {
    let mut report = ScratchPruneReport::default();
    let Ok(entries) = std::fs::read_dir(scratch) else {
        return report;
    };
    let Some(cutoff) = now.checked_sub(retention) else {
        return report;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let in_use = busy.iter().any(|dir| dir.starts_with(&path));
        if in_use || modified_since(&path, cutoff) {
            report.kept += 1;
            continue;
        }
        let removed = match entry.file_type() {
            Ok(kind) if kind.is_dir() => std::fs::remove_dir_all(&path),
            Ok(_) => std::fs::remove_file(&path),
            Err(err) => Err(err),
        };
        if removed.is_ok() {
            report.removed.push(path);
        } else {
            report.kept += 1;
        }
    }
    report
}

/// True if `path` or anything beneath it was modified at or after `cutoff`.
/// Symlinks are inspected but never followed. Stops at the first recent hit.
fn modified_since(path: &Path, cutoff: SystemTime) -> bool {
    let mut stack = vec![path.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(metadata) = std::fs::symlink_metadata(&current) else {
            continue;
        };
        match metadata.modified() {
            Ok(modified) if modified >= cutoff => return true,
            Ok(_) => {}
            // Unknown mtime: be conservative and keep the entry.
            Err(_) => return true,
        }
        if metadata.is_dir()
            && let Ok(children) = std::fs::read_dir(&current)
        {
            stack.extend(children.flatten().map(|child| child.path()));
        }
    }
    false
}

/// Working directories of live processes, so an in-progress build or shell
/// sitting inside a scratch entry is never deleted out from under it.
#[cfg(target_os = "linux")]
fn busy_process_dirs() -> Option<Vec<PathBuf>> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Some(Vec::new());
    };
    Some(
        entries
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.bytes().all(|b| b.is_ascii_digit()))
            })
            .filter_map(|entry| std::fs::read_link(entry.path().join("cwd")).ok())
            .collect(),
    )
}

/// macOS has no `/proc`; use libproc's `PROC_PIDVNODEPATHINFO` to read each
/// process's current working directory. Without this the prune guard was inert on
/// macOS (`Vec::new()`), so directories holding live processes were deleted out
/// from under them.
#[cfg(target_os = "macos")]
fn busy_process_dirs() -> Option<Vec<PathBuf>> {
    use std::ffi::CStr;
    use std::os::raw::c_char;

    // From <libproc.h>; libc does not export these.
    const PROC_ALL_PIDS: u32 = 1;

    let mut capacity = 4096usize;
    let mut pids: Vec<libc::pid_t> = Vec::new();
    let count;
    let mut attempt = 0;
    loop {
        pids.resize(capacity, 0);
        let bytes = unsafe {
            libc::proc_listpids(
                PROC_ALL_PIDS,
                0,
                pids.as_mut_ptr().cast(),
                (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int,
            )
        };
        if bytes <= 0 {
            return None;
        }
        let filled = bytes as usize / std::mem::size_of::<libc::pid_t>();
        // A full buffer means the list may be truncated: retry with more room.
        if filled < capacity || attempt >= 3 {
            count = filled;
            break;
        }
        attempt += 1;
        capacity *= 2;
    }
    pids.truncate(count);

    let expect = std::mem::size_of::<libc::proc_vnodepathinfo>();
    let mut dirs = Vec::with_capacity(count);
    let mut readable = 0usize;
    for pid in pids {
        if pid <= 0 {
            continue;
        }
        let mut info = std::mem::MaybeUninit::<libc::proc_vnodepathinfo>::zeroed();
        let got = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDVNODEPATHINFO,
                0,
                info.as_mut_ptr().cast(),
                expect as libc::c_int,
            )
        };
        // Short read means the process vanished or we lack permission to inspect it.
        if got as usize != expect {
            continue;
        }
        readable += 1;
        let info = unsafe { info.assume_init() };
        let raw: *const c_char = info.pvi_cdir.vip_path.as_ptr().cast();
        if raw.is_null() {
            continue;
        }
        let path = unsafe { CStr::from_ptr(raw) };
        if let Ok(text) = path.to_str() {
            if !text.is_empty() {
                dirs.push(PathBuf::from(text));
            }
        }
    }

    // If not a single process was inspectable the API is not behaving as expected;
    // report that as "unknown" so the caller declines to prune.
    if readable == 0 {
        None
    } else {
        Some(dirs)
    }
}

/// Any platform without a working cwd probe reports "unknown", which makes
/// [`prune_stale_scratch`] skip the pass instead of pruning unprotected.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn busy_process_dirs() -> Option<Vec<PathBuf>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};

    const DAY: Duration = Duration::from_secs(24 * 60 * 60);

    fn age(path: &Path, by: Duration) {
        let when = SystemTime::now() - by;
        File::options()
            .read(true)
            .open(path)
            .and_then(|file| file.set_modified(when))
            .expect("set mtime");
    }

    #[test]
    fn removes_only_entries_with_no_recent_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let scratch = temp.path();

        // Old build tree: every file and directory is old.
        let old = scratch.join("old-target/debug/deps");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("libfoo.rlib"), b"x").unwrap();
        age(&old.join("libfoo.rlib"), 30 * DAY);
        age(&old, 30 * DAY);
        age(&scratch.join("old-target/debug"), 30 * DAY);
        age(&scratch.join("old-target"), 30 * DAY);

        // Old top-level dir but a recently touched file deep inside.
        let active = scratch.join("active/deep");
        fs::create_dir_all(&active).unwrap();
        fs::write(active.join("fresh.log"), b"x").unwrap();
        age(&active, 30 * DAY);
        age(&scratch.join("active"), 30 * DAY);

        // Old loose file and a fresh loose file.
        fs::write(scratch.join("old.txt"), b"x").unwrap();
        age(&scratch.join("old.txt"), 30 * DAY);
        fs::write(scratch.join("new.txt"), b"x").unwrap();

        let report = prune_stale_scratch_in(scratch, SystemTime::now(), 7 * DAY, &[]);

        assert!(!scratch.join("old-target").exists());
        assert!(!scratch.join("old.txt").exists());
        assert!(scratch.join("active/deep/fresh.log").exists());
        assert!(scratch.join("new.txt").exists());
        assert_eq!(report.removed.len(), 2);
        assert_eq!(report.kept, 2);
    }

    #[test]
    fn keeps_entries_used_by_live_processes() {
        let temp = tempfile::tempdir().expect("tempdir");
        let scratch = temp.path();
        let busy_dir = scratch.join("busy/sub");
        fs::create_dir_all(&busy_dir).unwrap();
        age(&busy_dir, 30 * DAY);
        age(&scratch.join("busy"), 30 * DAY);

        let report = prune_stale_scratch_in(
            scratch,
            SystemTime::now(),
            7 * DAY,
            std::slice::from_ref(&busy_dir),
        );

        assert!(busy_dir.exists());
        assert!(report.removed.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn never_follows_symlinks_out_of_scratch() {
        let temp = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside");
        fs::write(outside.path().join("precious"), b"x").unwrap();
        age(&outside.path().join("precious"), 30 * DAY);

        let scratch = temp.path().join("scratch");
        fs::create_dir_all(&scratch).unwrap();
        let entry = scratch.join("linkdir");
        fs::create_dir_all(&entry).unwrap();
        std::os::unix::fs::symlink(outside.path(), entry.join("link")).unwrap();

        // Evaluate from a year in the future so everything in scratch is
        // stale, then confirm removal did not reach through the symlink.
        let report = prune_stale_scratch_in(&scratch, SystemTime::now() + 365 * DAY, 7 * DAY, &[]);
        assert_eq!(report.removed.len(), 1);
        assert!(!entry.exists());
        assert!(outside.path().join("precious").exists());
    }

    /// The whole point of `busy_process_dirs` is that a live process' cwd is seen.
    /// The test process is a live process, so its own cwd must appear. Before the
    /// macOS implementation existed this probe returned an empty Vec on macOS, so
    /// the prune guard could never fire.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn sees_own_working_directory() {
        let dirs = busy_process_dirs().expect("cwd probe must succeed on this platform");
        assert!(
            !dirs.is_empty(),
            "cwd probe returned no directories for any process"
        );

        let cwd = std::env::current_dir().expect("current_dir");
        // Compare canonically: macOS reports /private/var for /var, and scratch is
        // canonicalised before comparison in prune_stale_scratch.
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let want = canon(&cwd);
        assert!(
            dirs.iter().any(|d| canon(d) == want),
            "own cwd {:?} missing from {} detected working directories",
            want,
            dirs.len()
        );
    }

    /// End-to-end acceptance for the original bug: a directory that a LIVE process is
    /// sitting inside must survive a prune pass, while an equally stale directory with
    /// nobody in it must be removed in the same pass.
    ///
    /// This exercises the real probe and the real prune together, on the real
    /// filesystem, against a real child process. Before the macOS implementation
    /// existed the probe returned an empty Vec and the busy directory was deleted.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn prune_pass_keeps_directory_with_live_process_and_removes_idle_one() {
        let temp = tempfile::tempdir().expect("tempdir");
        let scratch = temp.path().join("scratch");
        fs::create_dir_all(&scratch).unwrap();

        // Stale entry nobody is using: must be removed.
        let idle = scratch.join("idle");
        fs::create_dir_all(&idle).unwrap();
        fs::write(idle.join("f"), b"x").unwrap();
        age(&idle.join("f"), 30 * DAY);
        age(&idle, 30 * DAY);

        // Equally stale entry, but a live process will hold it as its cwd: must be kept.
        let busy = scratch.join("busy");
        let busy_inner = busy.join("work");
        fs::create_dir_all(&busy_inner).unwrap();
        fs::write(busy_inner.join("f"), b"x").unwrap();
        age(&busy_inner.join("f"), 30 * DAY);
        age(&busy_inner, 30 * DAY);
        age(&busy, 30 * DAY);

        let mut child = std::process::Command::new("sleep")
            .arg("120")
            .current_dir(&busy_inner)
            .spawn()
            .expect("spawn a live child process inside the scratch entry");

        // Production canonicalises the scratch root before comparing, because the
        // probe reports canonical cwds (/private/var on macOS vs /var). Mirror that,
        // otherwise the comparison silently never matches.
        let scratch_canon = std::fs::canonicalize(&scratch).unwrap_or_else(|_| scratch.clone());

        let dirs = busy_process_dirs().expect("cwd probe must succeed on this platform");
        assert!(
            !dirs.is_empty(),
            "probe returned no working directories; guard would be inert"
        );

        let report = prune_stale_scratch_in(&scratch_canon, SystemTime::now(), 7 * DAY, &dirs);

        let idle_gone = !idle.exists();
        let busy_kept = busy.exists();

        let _ = child.kill();
        let _ = child.wait();

        assert!(
            idle_gone,
            "a stale entry with no live process should have been removed; report={report:?}"
        );
        assert!(
            busy_kept,
            "a stale entry holding a live process cwd was deleted - this is the original bug; \
             report={report:?}"
        );
    }

    /// Exercises the scheduled entry point itself, not just the helpers beneath it.
    ///
    /// `prune_pass_keeps_directory_with_live_process_and_removes_idle_one` covers the
    /// probe + prune composition, but it hands in its own scratch root and busy list.
    /// The real caller is `prune_stale_scratch`, which additionally resolves the home
    /// via `JCODE_HOME`, honours the 24h stamp gate, canonicalises the scratch root,
    /// and writes the stamp afterwards. Any of those could reintroduce the bug while
    /// the lower-level test still passed.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn scheduled_entry_point_keeps_live_directory_and_removes_idle_one() {
        let _guard = crate::storage::lock_test_env();
        let prev_home = std::env::var_os("JCODE_HOME");
        let prev_retention = std::env::var_os(SCRATCH_RETENTION_ENV);

        // NOTE: `tempfile::tempdir()` is unusable for a JCODE_HOME here. In this harness TMPDIR
        // points at `~/.jcode/scratch`, so a default tempdir is created *inside* a jcode directory
        // and the test would prune the very scratch tree it is asserting on. Use an explicit root
        // outside all jcode paths.
        let temp = tempfile::Builder::new()
            .prefix("scratch-prune-entrypoint-")
            .tempdir_in("/private/tmp")
            .expect("tempdir outside jcode paths");
        crate::env::set_var("JCODE_HOME", temp.path());
        // Pin retention so the test cannot be defeated by an ambient value.
        crate::env::set_var(SCRATCH_RETENTION_ENV, "7");

        let scratch = temp.path().join("scratch");
        fs::create_dir_all(&scratch).unwrap();

        let idle = scratch.join("idle");
        fs::create_dir_all(&idle).unwrap();
        fs::write(idle.join("f"), b"x").unwrap();
        age(&idle.join("f"), 30 * DAY);
        age(&idle, 30 * DAY);

        let busy = scratch.join("busy");
        let busy_inner = busy.join("work");
        fs::create_dir_all(&busy_inner).unwrap();
        fs::write(busy_inner.join("f"), b"x").unwrap();
        age(&busy_inner.join("f"), 30 * DAY);
        age(&busy_inner, 30 * DAY);
        age(&busy, 30 * DAY);

        let mut child = std::process::Command::new("sleep")
            .arg("120")
            .current_dir(&busy_inner)
            .spawn()
            .expect("spawn a live child process inside the scratch entry");

        // No stamp yet, so the pass is due (prune_due is true when the marker is absent).
        assert!(
            prune_due(temp.path()),
            "a fresh home has no prune stamp, so the pass must be due"
        );
        // Localise the failure: prove the redirect and the scratch root before pruning.
        let resolved = crate::storage::jcode_dir().expect("jcode_dir must resolve");
        assert_eq!(
            resolved,
            temp.path(),
            "JCODE_HOME did not redirect storage::jcode_dir"
        );
        assert!(scratch.is_dir(), "scratch root must exist under the redirected home");
        assert!(
            retention_from_env().is_some(),
            "retention must be enabled for the pass to run"
        );

        prune_stale_scratch();

        let idle_gone = !idle.exists();
        let busy_kept = busy.exists();
        let stamped = temp.path().join(PRUNE_MARKER).exists();

        let _ = child.kill();
        let _ = child.wait();

        match prev_retention {
            Some(value) => crate::env::set_var(SCRATCH_RETENTION_ENV, value),
            None => crate::env::remove_var(SCRATCH_RETENTION_ENV),
        }
        match prev_home {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }

        assert!(
            idle_gone,
            "the scheduled pass should have removed a stale idle entry"
        );
        assert!(
            busy_kept,
            "the scheduled pass deleted a directory holding a live process cwd"
        );
        assert!(stamped, "the pass must record its stamp so it runs once a day");
    }
}
